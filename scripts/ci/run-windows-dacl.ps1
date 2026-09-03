[CmdletBinding()]
param(
    [ValidateRange(1, 60)]
    [int]$TestTimeoutMinutes = 20
)

$ErrorActionPreference = 'Stop'

foreach ($command in 'cargo', 'rustc', 'cl.exe', 'link.exe', 'schtasks.exe') {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Windows DACL gate requires '$command' on the hosted runner PATH."
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not (Test-Path (Join-Path $repoRoot 'Cargo.toml'))) {
    throw 'run-windows-dacl.ps1 must run from an rdpilot checkout.'
}

$hostTriple = (& rustc -vV | Select-String '^host:').ToString().Split(':', 2)[1].Trim()
if ($hostTriple -notmatch 'pc-windows-msvc$') {
    throw "Windows DACL gate requires an MSVC Rust host, found '$hostTriple'."
}

$testAccount = "rdpDacl$PID$(Get-Random -Minimum 1000 -Maximum 9999)"
$taskName = "RdpilotDacl-$testAccount"
$accountCreated = $false

try {
    $passwordBytes = New-Object byte[] 24
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($passwordBytes)
    $password = 'Aa1!' + [Convert]::ToBase64String($passwordBytes)
    $securePassword = ConvertTo-SecureString $password -AsPlainText -Force
    New-LocalUser -Name $testAccount -Password $securePassword -PasswordNeverExpires | Out-Null
    $accountCreated = $true

    $env:RDPILOT_LIVE = '1'
    $env:RDPILOT_SECOND_WINDOWS_ACCOUNT = $testAccount
    $env:RDPILOT_SECOND_WINDOWS_PASSWORD = $password
    $env:RDPILOT_DACL_TASK_NAME = $taskName

    $test = Start-Process -FilePath (Get-Command cargo).Source -WorkingDirectory $repoRoot `
        -ArgumentList @('test', '-p', 'rdpilot-daemon', '--test', 'live_daemon_windows_dacl', '--', '--ignored', '--test-threads=1') `
        -PassThru -NoNewWindow
    if (-not $test.WaitForExit($TestTimeoutMinutes * 60 * 1000)) {
        Stop-Process -Id $test.Id -Force -ErrorAction SilentlyContinue
        throw "Windows DACL test exceeded its $TestTimeoutMinutes minute limit."
    }
    if ($test.ExitCode -ne 0) { throw "Windows DACL test failed with exit code $($test.ExitCode)." }
}
finally {
    & schtasks.exe /delete /tn $taskName /f *> $null
    if ($accountCreated) { Remove-LocalUser -Name $testAccount -ErrorAction SilentlyContinue }
    Remove-Item Env:RDPILOT_SECOND_WINDOWS_PASSWORD -ErrorAction SilentlyContinue
}
