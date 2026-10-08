#Requires -Version 7
# Run one live desktop proof against this machine's own loopback RDP, from the
# repository root after a release build. It changes machine settings and
# creates users, then restores them: use it only on a disposable Windows
# machine (see README.md).
[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('cua', 'viewer', 'takeover')][string]$Proof,
    [string]$BinDir = 'target/x86_64-pc-windows-msvc/release',
    # Keep this below the CI step timeout, so that cleanup runs when a harness hangs.
    [int]$TimeLimitMinutes = 35
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$evidence = Join-Path $base "hosted-desktop-evidence/$Proof"
$private = Join-Path $base "hosted-desktop-private-$Proof"
$bundle = Join-Path $private 'bundle'
$bin = (Resolve-Path $BinDir).Path
$ts = 'HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server'
$tcp = Join-Path $ts 'WinStations\RDP-Tcp'
$rule = "RdpilotDesktopProof-$PID"
$users = [ordered]@{}
$credentials = @{}
$remaining = @()
$oldDeny = (Get-ItemProperty $ts).fDenyTSConnections
$oldNla = (Get-ItemProperty $tcp).UserAuthentication
$service = Get-CimInstance Win32_Service -Filter "Name='TermService'"
$cache = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'rdpilot\cache'
$initialCachePresent = Test-Path $cache
$result = [ordered]@{ proof = $Proof; stage = 'setup'; harness_exit_code = $null; summary_status = $null; passed = $false }
# Only processes started from this build, never an unrelated rdpilot.
$ownProcesses = {
    Get-Process rdpilot, rdpilot-daemon, rdpilot-mcp -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($bin + '\', [StringComparison]::OrdinalIgnoreCase) }
}
New-Item -ItemType Directory -Force $evidence | Out-Null
try {
    if ($initialCachePresent) { throw 'The live desktop proof requires an empty native rdpilot cache' }
    New-Item -ItemType Directory -Force $private, $bundle | Out-Null
    # Credential files are never evidence; restrict them to this process owner.
    & icacls $private /inheritance:r /grant:r "$($env:USERDOMAIN)\$($env:USERNAME):(OI)(CI)F" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'Unable to restrict the private directory' }
    Copy-Item (Join-Path $bin 'rdpilot-bridge.exe') $bundle
    [ordered]@{
        proof = $Proof
        source_commit = (& git rev-parse HEAD)
        source_tree = (& git rev-parse 'HEAD^{tree}')
        rustc = (& rustc --version)
        source_bridge_sha256 = (Get-FileHash (Join-Path $bundle 'rdpilot-bridge.exe')).Hash.ToLower()
        os = (Get-CimInstance Win32_OperatingSystem).Caption
        image_os = $env:ImageOS
        image_version = $env:ImageVersion
        auth = 'NLA/CredSSP, TLS, self-signed certificate accepted explicitly by the harness relay'
        graphics = 'rdpilot product defaults'
        account_role = 'local standard users (Users, Remote Desktop Users), not administrators'
        target = '127.0.0.1:3389'
        native_cache_initially_absent = $true
        not_executed_live = @(
            'recording proof: needs three concurrent users and POSIX client checks; offline fake proof only'
            'tailnet and cross-machine viewing'
            'UAC and the secure desktop'
            'Windows Job containment over RDP'
            'download of a published bridge release'
            'domain or production authentication'
        )
    } | ConvertTo-Json | Set-Content (Join-Path $evidence 'environment.json')

    $labels = if ($Proof -eq 'takeover') { @('a') } else { @('a', 'b') }
    foreach ($label in $labels) {
        $name = "rdp$Proof$label"
        $password = 'Rdp1!' + [Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes(24)) + 'aA1!'
        if ($env:GITHUB_ACTIONS -eq 'true') { Write-Host "::add-mask::$password" }
        $user = New-LocalUser -Name $name -Password (ConvertTo-SecureString $password -AsPlainText -Force) -PasswordNeverExpires
        $users[$name] = $user.SID.Value
        Add-LocalGroupMember -Group 'Remote Desktop Users' -Member $name
        Add-LocalGroupMember -Group 'Users' -Member $name
        $credentials[$label] = [ordered]@{ host = '127.0.0.1'; port = 3389; username = $name; domain = $env:COMPUTERNAME; password = $password }
    }

    $result.stage = 'listener'
    Set-ItemProperty $ts -Name fDenyTSConnections -Value 0
    Set-ItemProperty $tcp -Name UserAuthentication -Value 1
    Set-Service TermService -StartupType Manual
    Start-Service TermService
    New-NetFirewallRule -Name $rule -DisplayName 'rdpilot desktop proof loopback' -Direction Inbound -Action Allow -Protocol TCP -LocalPort 3389 -RemoteAddress '127.0.0.1' | Out-Null
    $ready = $false
    for ($i = 0; $i -lt 30; $i++) {
        $client = [Net.Sockets.TcpClient]::new()
        try {
            $task = $client.ConnectAsync('127.0.0.1', 3389)
            if ($task.Wait(1000) -and $client.Connected) { $ready = $true; break }
        } catch {} finally { $client.Dispose() }
        Start-Sleep -Seconds 1
    }
    if (-not $ready) { throw 'Loopback RDP listener unavailable after 30 seconds' }
    Write-Host 'Loopback RDP listener ready with NLA enabled'

    # Passwords reach the harness only through a private file or its own environment.
    $start = [Diagnostics.ProcessStartInfo]::new((Get-Command python).Source)
    $start.UseShellExecute = $false
    $script = @{ cua = 'run-cua-e2e.py'; viewer = 'run-viewer-proof.py'; takeover = 'run-takeover-proof.py' }[$Proof]
    $arguments = @((Join-Path $PSScriptRoot "../e2e/$script"), '--bin-dir', $bin, '--bundle', $bundle,
        '--output', (Join-Path $evidence 'proof'), '--connect-timeout', '600')
    if ($Proof -eq 'cua') {
        foreach ($label in $labels) {
            $credentials[$label] | ConvertTo-Json -Compress | Set-Content (Join-Path $private "$label.json")
            $arguments += "--credentials-$label", (Join-Path $private "$label.json")
        }
    } else {
        $arguments += '--allow-no-tailnet'
        foreach ($label in $labels) {
            $prefix = if ($Proof -eq 'viewer') { "RDPILOT_VIEW_$($label.ToUpper())_" } else { 'RDPILOT_TAKE_' }
            foreach ($key in $credentials[$label].Keys) { $start.Environment[$prefix + $key.ToUpper()] = [string]$credentials[$label][$key] }
        }
    }
    foreach ($argument in $arguments) { $start.ArgumentList.Add($argument) }
    $result.stage = 'proof'
    $harness = [Diagnostics.Process]::Start($start)
    if (-not $harness.WaitForExit($TimeLimitMinutes * 60000)) {
        $result.stage = 'proof_timeout'
        Write-Host "Proof exceeded $TimeLimitMinutes minutes; stopping the harness process tree"
        $harness.Kill($true)
        $harness.WaitForExit(30000) | Out-Null
    }
    $result.harness_exit_code = if ($harness.HasExited) { $harness.ExitCode } else { $null }
    & quser 2>&1 | Out-File (Join-Path $evidence 'windows-sessions.txt')
} catch {
    $_.Exception.Message | Set-Content (Join-Path $evidence 'setup-failure.txt')
    Write-Host $_.Exception.Message
} finally {
    # One failed restore must not skip the others; the readbacks below report it.
    $ErrorActionPreference = 'Continue'
    & $ownProcesses | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($name in $users.Keys) {
        # Terminate only the disposable users' sessions.
        $sessions = & quser $name 2>$null
        foreach ($line in ($sessions | Select-Object -Skip 1)) {
            if ($line -match '^\s*>?\S+\s+(?:(?:\S+)\s+)?(\d+)\s+(Active|Disc)') {
                & logoff $Matches[1] 2>$null
            }
        }
    }
    # Logoff is asynchronous; a profile can be removed only once it is unloaded.
    $deadline = (Get-Date).AddSeconds(60)
    do {
        $remaining = @($users.Keys | Where-Object { & quser $_ 2>$null })
        if ($remaining.Count) { Start-Sleep -Seconds 2 }
    } while ($remaining.Count -and (Get-Date) -lt $deadline)
    foreach ($sid in $users.Values) {
        for ($i = 0; $i -lt 10; $i++) {
            $userProfile = Get-CimInstance Win32_UserProfile -Filter "SID='$sid'"
            if (-not $userProfile) { break }
            if (-not $userProfile.Loaded) { $userProfile | Remove-CimInstance -ErrorAction SilentlyContinue }
            Start-Sleep -Seconds 3
        }
    }
    foreach ($name in $users.Keys) { Remove-LocalUser $name -ErrorAction SilentlyContinue }
    Set-ItemProperty $ts -Name fDenyTSConnections -Value $oldDeny
    Set-ItemProperty $tcp -Name UserAuthentication -Value $oldNla
    Remove-NetFirewallRule -Name $rule -ErrorAction SilentlyContinue
    if ($service.State -eq 'Stopped') { Stop-Service TermService -Force -ErrorAction SilentlyContinue }
    $startup = switch ($service.StartMode) { 'Auto' { 'Automatic' } 'Disabled' { 'Disabled' } default { 'Manual' } }
    Set-Service TermService -StartupType $startup -ErrorAction SilentlyContinue
    Remove-Item $private -Recurse -Force -ErrorAction SilentlyContinue
    if (-not $initialCachePresent) { Remove-Item $cache -Recurse -Force -ErrorAction SilentlyContinue }
    $restoredService = Get-CimInstance Win32_Service -Filter "Name='TermService'"
    # The harness redacts its own evidence; check again, also after a timeout.
    $leaked = @(Get-ChildItem $evidence -Recurse -File | Where-Object {
        $text = [IO.File]::ReadAllText($_.FullName, [Text.Encoding]::Latin1)
        @($credentials.Values | Where-Object { $text.Contains($_.password) }).Count
    })
    $leaked | Remove-Item -Force -ErrorAction SilentlyContinue
    $cleanup = [ordered]@{
        evidence_free_of_passwords = $leaked.Count -eq 0
        processes_stopped = @(& $ownProcesses).Count -eq 0
        sessions_ended = $remaining.Count -eq 0
        profiles_removed = @($users.Values | Where-Object { Get-CimInstance Win32_UserProfile -Filter "SID='$_'" }).Count -eq 0
        users_removed = @($users.Keys | Where-Object { Get-LocalUser $_ -ErrorAction SilentlyContinue }).Count -eq 0
        private_removed = -not (Test-Path $private)
        registry_restored = ((Get-ItemProperty $ts).fDenyTSConnections -eq $oldDeny) -and ((Get-ItemProperty $tcp).UserAuthentication -eq $oldNla)
        firewall_removed = -not (Get-NetFirewallRule -Name $rule -ErrorAction SilentlyContinue)
        service_state_restored = $restoredService.State -eq $service.State
        service_start_mode_restored = $restoredService.StartMode -eq $service.StartMode
        cache_removed = $initialCachePresent -or -not (Test-Path $cache)
    }
    $cleanup | ConvertTo-Json | Set-Content (Join-Path $evidence 'cleanup.json')
    $summary = Join-Path $evidence 'proof/summary.json'
    try { $result.summary_status = (Get-Content $summary -Raw | ConvertFrom-Json).status } catch { $result.summary_status = 'missing' }
    $result.passed = ($result.harness_exit_code -eq 0) -and ($result.summary_status -eq 'passed') -and
        -not @($cleanup.Values | Where-Object { $_ -ne $true }).Count
    $result | ConvertTo-Json | Set-Content (Join-Path $evidence 'result.json')
}
if ($result.passed) { exit 0 }
exit 1
