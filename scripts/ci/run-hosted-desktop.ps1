[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$BinDir,
    [Parameter(Mandatory=$true)][string]$Output,
    [Parameter(Mandatory=$true)][string]$ExpectedCommit,
    [string]$CuaVersion = '0.34.0'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($CuaVersion -ne '0.34.0') { throw 'Unsupported Cua version for this gate' }
& python (Join-Path $PSScriptRoot 'hosted_desktop.py') --bin-dir $BinDir --output $Output --expected-commit $ExpectedCommit
exit $LASTEXITCODE
