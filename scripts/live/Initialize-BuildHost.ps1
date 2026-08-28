[CmdletBinding()]
param(
    [int]$DownloadTimeoutSeconds = 900,
    [int]$RustInstallTimeoutMinutes = 15,
    [int]$VisualStudioInstallTimeoutMinutes = 45
)

# Dot-source this script from a Windows gate. It installs only missing
# prerequisites, then imports the VS developer environment into the current
# PowerShell process so `cargo` can find link.exe/cl.exe.
$ErrorActionPreference = 'Stop'

function Invoke-BoundedProcess {
    param(
        [Parameter(Mandatory)] [string]$FilePath,
        [string[]]$ArgumentList = @(),
        [Parameter(Mandatory)] [int]$TimeoutMinutes,
        [Parameter(Mandatory)] [string]$Description
    )

    $process = Start-Process -FilePath $FilePath -ArgumentList $ArgumentList -PassThru
    if (-not $process.WaitForExit($TimeoutMinutes * 60 * 1000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "$Description exceeded its $TimeoutMinutes minute limit."
    }
    if ($process.ExitCode -ne 0) {
        throw "$Description failed with exit code $($process.ExitCode)."
    }
}

function Get-VisualStudioInstallPath {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return $null }

    $installPath = & $vswhere -latest -products '*' `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($installPath)) { return $null }
    return $installPath.Trim()
}

function Import-VisualStudioEnvironment {
    param([Parameter(Mandatory)] [string]$InstallPath)

    $devCmd = Join-Path $InstallPath 'Common7\Tools\VsDevCmd.bat'
    if (-not (Test-Path $devCmd)) {
        throw "Visual Studio Build Tools was found but VsDevCmd.bat is missing: $devCmd"
    }

    # `set` emits the post-VsDevCmd environment. Import only valid entries;
    # this keeps the setup scoped to the gate's PowerShell process.
    $cmdArgument = 'call "{0}" -no_logo -arch=x64 -host_arch=x64 >nul && set' -f $devCmd
    $environment = & cmd.exe /d /s /c $cmdArgument
    if ($LASTEXITCODE -ne 0) { throw 'Failed to initialize the Visual Studio developer environment.' }
    foreach ($entry in $environment) {
        $separator = $entry.IndexOf('=')
        if ($separator -gt 0) {
            [Environment]::SetEnvironmentVariable($entry.Substring(0, $separator), $entry.Substring($separator + 1), 'Process')
        }
    }
}

function Test-MsvcRust {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    $rustc = Get-Command rustc -ErrorAction SilentlyContinue
    if (-not $cargo -or -not $rustc) { return $false }
    $host = (& rustc -vV | Select-String '^host:').ToString().Split(':', 2)[1].Trim()
    return $host -match 'x86_64-pc-windows-msvc$'
}

if (-not ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Windows build-host setup requires an elevated Crabbox Windows session to install Visual Studio Build Tools.'
}

$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

$downloadDirectory = Join-Path $env:TEMP 'rdpilot-build-host'
New-Item -ItemType Directory -Force -Path $downloadDirectory | Out-Null
$rustInstaller = Join-Path $downloadDirectory 'rustup-init.exe'
$vsInstaller = Join-Path $downloadDirectory 'vs_BuildTools.exe'

try {
    if (-not (Test-MsvcRust)) {
        Write-Host 'Installing Rust stable (MSVC host) from the official Rust installer...'
        Invoke-WebRequest -Uri 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' `
            -OutFile $rustInstaller -TimeoutSec $DownloadTimeoutSeconds
        Invoke-BoundedProcess -FilePath $rustInstaller `
            -ArgumentList @('-y', '--profile', 'minimal', '--default-toolchain', 'stable-x86_64-pc-windows-msvc') `
            -TimeoutMinutes $RustInstallTimeoutMinutes -Description 'Rust installation'
        $env:PATH = "$cargoBin;$env:PATH"
    }

    $vsInstallPath = Get-VisualStudioInstallPath
    if (-not $vsInstallPath) {
        Write-Host 'Installing Visual Studio Build Tools C++ workload from the official Microsoft installer...'
        Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_BuildTools.exe' `
            -OutFile $vsInstaller -TimeoutSec $DownloadTimeoutSeconds
        Invoke-BoundedProcess -FilePath $vsInstaller `
            -ArgumentList @('--quiet', '--wait', '--norestart', '--nocache', '--add', 'Microsoft.VisualStudio.Workload.VCTools', '--includeRecommended') `
            -TimeoutMinutes $VisualStudioInstallTimeoutMinutes -Description 'Visual Studio Build Tools installation'
        $vsInstallPath = Get-VisualStudioInstallPath
    }
    if (-not $vsInstallPath) {
        throw 'Visual Studio Build Tools C++ workload was not detected after installation.'
    }

    Import-VisualStudioEnvironment -InstallPath $vsInstallPath
    foreach ($command in 'cargo', 'rustc', 'cl.exe', 'link.exe') {
        if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
            throw "Build-host setup completed but '$command' is unavailable on PATH."
        }
    }
    if (-not (Test-MsvcRust)) {
        throw 'Build-host setup completed but Rust is not using the x86_64-pc-windows-msvc host.'
    }
    Write-Host 'Windows Rust/MSVC build prerequisites are ready.'
}
finally {
    Remove-Item -LiteralPath $rustInstaller, $vsInstaller -Force -ErrorAction SilentlyContinue
}
