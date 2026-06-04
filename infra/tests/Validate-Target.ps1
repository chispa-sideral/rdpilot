<#
    Validate-Target.ps1 — Phase 1 post-deploy assertion suite (ENV-01 phase gate).

    One assertion per ENV-01 criterion, executed against a live target over WinRM/HTTPS
    after `manage-env.ps1 up`:
      1. RDP reachable        : Test-NetConnection <host> -Port 3389
      2. WinRM reachable      : Test-NetConnection <host> -Port 5986
      3. NLA enforced (VERIFY): WinStations\RDP-Tcp UserAuthentication -eq 1  (Pitfall 2 — verify, not set)
      4. 96 DPI (default hive): LogPixels -eq 96 AND Win8DpiScaling -eq 1
      5. SuppressWhenMinimized: RemoteDesktop_SuppressWhenMinimized -eq 2 at HKLM AND default-hive user path
      6. Sample program       : Test-Path 'C:\Program Files\7-Zip\7zFM.exe'

    SKELETON MODE: until a live .secrets/connection.json exists, every check prints a
    single "PENDING — requires live target from manage-env.ps1 up" line and the script
    exits 0. The real assertion commands below are the active code path once the
    connection file IS present (run for real by Plan 04's phase gate).

    WinRM client convention (Pitfall 3): self-signed cert on the target, so client
    sessions use  New-PSSessionOption -SkipCACheck -SkipCNCheck  with  -UseSSL.

    SECURITY: the admin password is read from the connection file into a session
    credential and is NEVER written to the console or any log.
#>
[CmdletBinding()]
param(
    [string]$ConnectionFile = '.secrets/connection.json',

    # Explicit target override. When supplied, takes precedence over the host in the
    # connection file (useful for ad-hoc validation of a known IP). Validated as IPv4.
    [Alias('TargetIp')]
    [string]$PublicIp
)

$ErrorActionPreference = 'Stop'

# ENV-01 criteria, in order. Used to emit a PENDING line per check in skeleton mode.
$env01Checks = @(
    'RDP reachable on TCP 3389',
    'WinRM reachable on TCP 5986',
    'NLA enforced (RDP-Tcp UserAuthentication = 1)',
    'Forced 96 DPI in default user hive (LogPixels = 96, Win8DpiScaling = 1)',
    'RemoteDesktop_SuppressWhenMinimized = 2 (HKLM + default-hive user path)',
    'Sample program 7-Zip File Manager present (7zFM.exe)'
)

# SKELETON MODE only applies when there is NEITHER an explicit -PublicIp NOR a
# connection file. With -PublicIp we run live against that target regardless.
if (-not $PublicIp -and -not (Test-Path $ConnectionFile)) {
    # ---- SKELETON MODE: no live target yet. Emit PENDING per check and exit 0. ----
    Write-Host "Validate-Target.ps1 — SKELETON MODE (no '$ConnectionFile' found and no -PublicIp)."
    Write-Host "Run 'manage-env.ps1 -Action up' first to provision a live target, then re-run."
    foreach ($check in $env01Checks) {
        Write-Host "  [PENDING] $check — requires live target from manage-env.ps1 up"
    }
    exit 0
}

# ---- LIVE MODE: an explicit -PublicIp or a connection file exists. -----------------
# Activated by Plan 04's phase gate. The connection file shape (written by manage-env.ps1):
#   { "host": "...", "user": "...", "password": "...", "rdpPort": 3389, "winrmPort": 5986 }
$conn = $null
if (Test-Path $ConnectionFile) {
    $conn = Get-Content $ConnectionFile -Raw | ConvertFrom-Json
}

# Resolve the target host: explicit -PublicIp wins, else the connection file's host.
$targetHost = if ($PublicIp) { $PublicIp } else { $conn.host }
$rdpPort   = if ($conn -and $conn.rdpPort)   { $conn.rdpPort }   else { 3389 }
$winrmPort = if ($conn -and $conn.winrmPort) { $conn.winrmPort } else { 5986 }

# FAIL-FAST on a missing/empty target. Without this guard, Test-NetConnection with an
# empty host silently enumerates LOCAL adapters (fe80::, 192.168.x, 100.x, etc.) and
# reports nonsense — this is exactly the cascade from a deploy that never captured an
# IP. Refuse to probe anything but a real, well-formed remote IPv4.
if (-not $targetHost) {
    throw "No target IP — is the env up? '$ConnectionFile' has an empty 'host' and no -PublicIp was supplied. Run 'manage-env.ps1 -Action up' (and confirm it captured a public IP), or pass -PublicIp <remote-ip>."
}
if ($targetHost -notmatch '^(\d{1,3}\.){3}\d{1,3}$') {
    throw "Target host '$targetHost' is not a valid remote IPv4 address. Refusing to probe (this guards against falling back to local interfaces). Check '$ConnectionFile' or pass -PublicIp <remote-ip>."
}

Write-Host "Validating REMOTE target $targetHost (RDP $rdpPort, WinRM $winrmPort)..."

# Build a WinRM credential WITHOUT echoing the password. Only possible when a
# connection file with user+password is present; with -PublicIp alone we can still
# run the TCP reachability checks but the in-guest WinRM assertions are skipped.
$cred = $null
if ($conn -and $conn.password) {
    $securePwd = ConvertTo-SecureString $conn.password -AsPlainText -Force
    $cred      = [System.Management.Automation.PSCredential]::new($conn.user, $securePwd)
    Remove-Variable securePwd
}

# Self-signed cert on the target (Pitfall 3): skip CA/CN validation, use SSL.
$soPss = New-PSSessionOption -SkipCACheck -SkipCNCheck

$failures = @()
function Assert-Env01 {
    param([string]$Name, [scriptblock]$Test)
    try {
        if (& $Test) { Write-Host "  [PASS] $Name" }
        else { Write-Host "  [FAIL] $Name"; $script:failures += $Name }
    } catch {
        Write-Host "  [FAIL] $Name — $($_.Exception.Message)"
        $script:failures += $Name
    }
}

# (1) RDP reachable on 3389.
Assert-Env01 'RDP reachable (TCP 3389)' {
    (Test-NetConnection $targetHost -Port $rdpPort).TcpTestSucceeded
}

# (2) WinRM reachable on 5986.
Assert-Env01 'WinRM reachable (TCP 5986)' {
    (Test-NetConnection $targetHost -Port $winrmPort).TcpTestSucceeded
}

# Single WinRM session for all in-guest registry / filesystem assertions. Requires a
# credential; without one (e.g. -PublicIp only) the in-guest checks cannot run.
if (-not $cred) {
    Write-Host ""
    Write-Host "No credential available (ran with -PublicIp but no connection file): skipping in-guest WinRM assertions (NLA/DPI/SuppressWhenMinimized/7-Zip)."
    if ($failures.Count -gt 0) { exit 1 }
    exit 0
}

$session = New-PSSession -ComputerName $targetHost -Port $winrmPort -UseSSL `
    -Credential $cred -SessionOption $soPss -Authentication Negotiate

try {
    # (3) NLA enforced — VERIFICATION only (Pitfall 2). NLA is on by default; assert it.
    Assert-Env01 'NLA enforced (UserAuthentication = 1)' {
        Invoke-Command -Session $session -ScriptBlock {
            (Get-ItemProperty 'HKLM:\System\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp').UserAuthentication -eq 1
        }
    }

    # (4) Forced 96 DPI in the DEFAULT user hive (load / read / unload — Pitfall 1).
    Assert-Env01 'Forced 96 DPI in default hive (LogPixels=96, Win8DpiScaling=1)' {
        Invoke-Command -Session $session -ScriptBlock {
            $hive = 'HKLM\DEFAULT_VALIDATE'
            reg load $hive C:\Users\Default\NTUSER.DAT | Out-Null
            try {
                $desk = Get-ItemProperty "Registry::$hive\Control Panel\Desktop"
                ($desk.LogPixels -eq 96) -and ($desk.Win8DpiScaling -eq 1)
            } finally {
                [gc]::Collect(); reg unload $hive | Out-Null
            }
        }
    }

    # (5) RemoteDesktop_SuppressWhenMinimized = 2 at HKLM AND the default-hive user path.
    Assert-Env01 'RemoteDesktop_SuppressWhenMinimized = 2 (HKLM + default hive)' {
        Invoke-Command -Session $session -ScriptBlock {
            $hklm = (Get-ItemProperty 'HKLM:\Software\Microsoft\Terminal Server Client' -ErrorAction Stop).RemoteDesktop_SuppressWhenMinimized
            $hive = 'HKLM\DEFAULT_VALIDATE'
            reg load $hive C:\Users\Default\NTUSER.DAT | Out-Null
            try {
                $user = (Get-ItemProperty "Registry::$hive\Software\Microsoft\Terminal Server Client").RemoteDesktop_SuppressWhenMinimized
                ($hklm -eq 2) -and ($user -eq 2)
            } finally {
                [gc]::Collect(); reg unload $hive | Out-Null
            }
        }
    }

    # (6) Sample remote-only program present: 7-Zip File Manager.
    Assert-Env01 'Sample program present (7zFM.exe)' {
        Invoke-Command -Session $session -ScriptBlock {
            Test-Path 'C:\Program Files\7-Zip\7zFM.exe'
        }
    }
}
finally {
    if ($session) { Remove-PSSession $session }
}

if ($failures.Count -gt 0) {
    Write-Host ""
    Write-Host "ENV-01 validation FAILED: $($failures.Count) check(s) did not pass."
    exit 1
}

Write-Host ""
Write-Host "ENV-01 validation PASSED: all checks green."
exit 0
