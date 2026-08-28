[CmdletBinding()]
param(
    [ValidateRange(1, 120)]
    [int]$TestTimeoutMinutes = 30,
    [switch]$SetupOnly
)

$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'Initialize-BuildHost.ps1')

function New-TemporaryPassword {
    $lower = 'abcdefghijkmnopqrstuvwxyz'
    $upper = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
    $digit = '23456789'
    $symbol = '!@#$%^&*()-_=+'
    $all = ($lower + $upper + $digit + $symbol).ToCharArray()

    $chars = [System.Collections.Generic.List[char]]::new()
    foreach ($group in @($lower, $upper, $digit, $symbol)) {
        $chars.Add($group[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($group.Length)])
    }
    while ($chars.Count -lt 32) {
        $chars.Add($all[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($all.Length)])
    }
    -join ($chars | Sort-Object { [System.Security.Cryptography.RandomNumberGenerator]::GetInt32([int]::MaxValue) })
}

foreach ($command in 'cargo', 'rustc', 'cl.exe', 'link.exe', 'schtasks.exe') {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Windows DACL gate requires '$command' on PATH after build-host setup."
    }
}

if (-not (Test-Path (Join-Path $PSScriptRoot '..\..\Cargo.toml'))) {
    throw 'Run-WindowsDacl.ps1 must run from a Crabbox-synced rdpilot checkout.'
}

$hostTriple = (& rustc -vV | Select-String '^host:').ToString().Split(':', 2)[1].Trim()
if ($hostTriple -notmatch 'pc-windows-msvc$') {
    throw "Windows DACL gate requires an MSVC Rust host, found '$hostTriple'."
}

if ($SetupOnly) {
    Write-Host 'Windows Rust/MSVC build-host readiness check passed.'
    return
}

$testAccount = "rdpDacl$PID$(Get-Random -Minimum 1000 -Maximum 9999)"
$taskName = "RdpilotDacl-$testAccount"
$accountCreated = $false

try {
    $password = New-TemporaryPassword
    $securePassword = ConvertTo-SecureString $password -AsPlainText -Force
    New-LocalUser -Name $testAccount -Password $securePassword -PasswordNeverExpires | Out-Null
    $accountCreated = $true

    $env:RDPILOT_LIVE = '1'
    $env:RDPILOT_SECOND_WINDOWS_ACCOUNT = $testAccount
    $env:RDPILOT_SECOND_WINDOWS_PASSWORD = $password
    $env:RDPILOT_DACL_TASK_NAME = $taskName

    $cargo = (Get-Command cargo -ErrorAction Stop).Source
    $testProcess = Start-Process -FilePath $cargo `
        -ArgumentList @('test', '-p', 'rdpilot-daemon', '--test', 'live_daemon_windows_dacl', '--', '--ignored', '--test-threads=1') `
        -PassThru -NoNewWindow
    if (-not $testProcess.WaitForExit($TestTimeoutMinutes * 60 * 1000)) {
        Stop-Process -Id $testProcess.Id -Force -ErrorAction SilentlyContinue
        throw "Windows DACL test exceeded its $TestTimeoutMinutes minute limit."
    }
    if ($testProcess.ExitCode -ne 0) {
        throw "Windows DACL test command failed with exit code $($testProcess.ExitCode)."
    }
}
finally {
    # The test normally deletes its one-shot task. This also covers test failures.
    & schtasks.exe /delete /tn $taskName /f *> $null
    if ($accountCreated) {
        Remove-LocalUser -Name $testAccount -ErrorAction SilentlyContinue
    }
    Remove-Item Env:RDPILOT_SECOND_WINDOWS_PASSWORD -ErrorAction SilentlyContinue
}
