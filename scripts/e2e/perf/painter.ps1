# Test-only painter launcher for the performance workload. It starts inside the
# test user's RDP session (logon Run key, or the Run dialog) under Windows
# PowerShell 5.1 and compiles PixelClock.cs from this directory.
param(
    [Parameter(Mandatory)][string]$User,
    [int]$Port = 47653,
    [int]$IntervalMs = 20,
    [int]$Step = 24
)
# The logon entry runs for every user; only the test user paints.
if ($env:USERNAME -ne $User) { exit 0 }
$log = Join-Path $PSScriptRoot "painter-$User.log"
try {
    "start $(Get-Date -Format o) pid=$PID user=$env:USERNAME ps=$($PSVersionTable.PSVersion)" | Out-File $log -Append
    $source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'PixelClock.cs'))
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition $source -Language CSharp
    "compiled $(Get-Date -Format o)" | Out-File $log -Append
    [PixelClock]::Run($Port, $IntervalMs, $Step)
    "exit $(Get-Date -Format o)" | Out-File $log -Append
} catch {
    "failed: $($_.Exception.ToString())" | Out-File $log -Append
    exit 1
}
