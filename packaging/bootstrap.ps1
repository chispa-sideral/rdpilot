param([Parameter(Mandatory=$true)][UInt64]$Generation)
$ErrorActionPreference = 'Stop'
$source = '\\tsclient\RDPILOT\bundle'
$manifest = Get-Content -LiteralPath "$source\manifest.json" -Raw | ConvertFrom-Json
if ($manifest.bundle_id -notmatch '^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}$') { throw 'Unsupported bundle' }
$base = Join-Path $env:LOCALAPPDATA 'rdpilot'
New-Item -ItemType Directory -Force -Path $base | Out-Null
# Named mutex prevents duplicate launch of the SAME session generation. A different
# generation may coexist briefly while an old RDP channel closes; never kill by name.
$mutex = New-Object System.Threading.Mutex($false, "Local\rdpilot-cua-$Generation")
if (-not $mutex.WaitOne(0)) { exit 0 }
try {
    $destination = Join-Path $base ($manifest.bundle_id + '-' + $manifest.bridge_sha256.Substring(0,16))
    $archive = Join-Path $destination 'cua.zip'
    $bridge = Join-Path $destination 'rdpilot-bridge.exe'
    function Assert-Hash($path, $expected) {
        if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $expected) { throw "Payload integrity failure: $path" }
    }
    $ready = Test-Path -LiteralPath (Join-Path $destination 'complete.json')
    if ($ready) {
        Assert-Hash $bridge $manifest.bridge_sha256
        Assert-Hash $archive $manifest.archive_sha256
        foreach ($entry in $manifest.files.PSObject.Properties) { Assert-Hash (Join-Path $destination $entry.Name) $entry.Value }
    } else {
        $stage = Join-Path $base ('.staging-' + $Generation)
        if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
        New-Item -ItemType Directory -Path $stage | Out-Null
        try {
            Copy-Item -LiteralPath "$source\cua.zip" -Destination "$stage\cua.zip"
            Copy-Item -LiteralPath "$source\rdpilot-bridge.exe" -Destination "$stage\rdpilot-bridge.exe"
            Assert-Hash "$stage\cua.zip" $manifest.archive_sha256
            Assert-Hash "$stage\rdpilot-bridge.exe" $manifest.bridge_sha256
            Expand-Archive -LiteralPath "$stage\cua.zip" -DestinationPath $stage
            foreach ($entry in $manifest.files.PSObject.Properties) { Assert-Hash (Join-Path $stage $entry.Name) $entry.Value }
            Copy-Item -LiteralPath "$source\manifest.json" -Destination "$stage\manifest.json"
            Copy-Item -LiteralPath "$source\manifest.json" -Destination "$stage\complete.json"
            # Directory publication is atomic. Concurrent installations either use
            # the already verified winner or fail, never overwrite running binaries.
            if (Test-Path -LiteralPath $destination) { throw 'Incomplete or concurrent installation; retry with a fresh connection' }
            Move-Item -LiteralPath $stage -Destination $destination
        } finally { if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force } }
    }
    $env:CUA_DRIVER_RS_TELEMETRY_ENABLED = 'false'
    & $bridge --generation $Generation
    if ($LASTEXITCODE -ne 0) { throw "Bridge exited: $LASTEXITCODE" }
} finally {
    $mutex.ReleaseMutex()
    $mutex.Dispose()
}
