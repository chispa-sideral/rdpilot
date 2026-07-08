#Requires -Version 5.1
<#
.SYNOPSIS
    Idempotent in-guest hardening for the rdpilot disposable Windows Server 2022
    test target. Invoked once by the main.bicep CustomScriptExtension (Plan 02)
    as: powershell -ExecutionPolicy Unrestricted -File Configure-Target.ps1

.DESCRIPTION
    Performs all ENV-01 in-guest configuration in a single pass, with NO reboot:
      1. WinRM HTTPS (5986) self-signed listener + inbound firewall rule.
      2. 96 DPI (LogPixels=96, Win8DpiScaling=1) + RemoteDesktop_SuppressWhenMinimized=2
         written to the DEFAULT user hive (C:\Users\Default\NTUSER.DAT) -- NOT the
         current-user hive, because the automation user's profile does not exist
         at provision time (RESEARCH.md Pitfall 1).
      3. Machine-wide RemoteDesktop_SuppressWhenMinimized=2 at HKLM + Wow6432Node.
      4. Server Manager auto-launch suppressed (DoNotOpenServerManagerAtLogon=1)
         written to the DEFAULT user hive -- Server Manager otherwise auto-opens
         maximized at every RDP logon and covers the full desktop, breaking any
         automation that assumes a bare desktop is reachable (found live during
         Phase 3 input-injection validation: it absorbed clicks/Alt+F4 intended
         for the desktop and blocked Start-menu-scroll/shutdown-dialog checks).
      5. 7-Zip silent install, gated by a pinned SHA-256 verification.

    Every mutation is guarded for idempotency: the CustomScriptExtension may
    re-execute on reboot, so a second run is a no-op (RESEARCH.md Pattern 1).

.NOTES
    7-Zip pin (approved via blocking human-verify checkpoint, Plan 01-03 Task 1,
    2026-06-04):
      Version:  26.01 (released 2026-04-27)
      URL:      https://github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe
      SHA-256:  d64a0468f5b5b0b0fc5b2188450bcd655b70809d97b1c4535f2884635094377d
      Source of published hash: official ip7z/7zip GitHub release asset digest
        (api.github.com/repos/ip7z/7zip/releases/tags/26.01). Computed hash of the
        downloaded file matched the published digest; the 7-zip.org/a/ mirror
        served a byte-for-byte identical binary. Do NOT change without re-verifying.
#>

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

# --- Pinned 7-Zip constants (approved Plan 01-03 Task 1; see NOTES above) -----
$SevenZipVersion  = '26.01'
$SevenZipUrl      = 'https://github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe'
$SevenZipSha256   = 'D64A0468F5B5B0B0FC5B2188450BCD655B70809D97B1C4535F2884635094377D'
$SevenZipFmExe    = 'C:\Program Files\7-Zip\7zFM.exe'

Write-Host "[Configure-Target] Starting idempotent in-guest hardening (no reboot)."

# =============================================================================
# 1. WinRM HTTPS listener (self-signed) + inbound firewall rule
#    Listener is guarded (created only if no HTTPS listener exists); the firewall
#    rule uses -ErrorAction SilentlyContinue so a re-run is a harmless no-op.
#    BOTH are required -- a listener without the firewall rule is unreachable
#    (RESEARCH.md Pitfall 3).
# =============================================================================
Write-Host "[Configure-Target] (1/4) WinRM HTTPS listener + firewall rule..."

$httpsListener = Get-ChildItem WSMan:\localhost\Listener -ErrorAction SilentlyContinue |
    Where-Object { $_.Keys -match 'Transport=HTTPS' }

if (-not $httpsListener) {
    $cert = New-SelfSignedCertificate -DnsName $env:COMPUTERNAME `
        -CertStoreLocation Cert:\LocalMachine\My
    New-Item -Path WSMan:\localhost\Listener -Transport HTTPS -Address * `
        -CertificateThumbPrint $cert.Thumbprint -Force | Out-Null
    Write-Host "[Configure-Target]   created HTTPS listener (cert thumbprint $($cert.Thumbprint))."
} else {
    Write-Host "[Configure-Target]   HTTPS listener already present -- skipped."
}

New-NetFirewallRule -DisplayName 'WinRM HTTPS' -Direction Inbound -LocalPort 5986 `
    -Protocol TCP -Action Allow -ErrorAction SilentlyContinue | Out-Null
Write-Host "[Configure-Target]   firewall rule 'WinRM HTTPS' ensured (TCP 5986 inbound)."

# =============================================================================
# 2. Per-user DPI + SuppressWhenMinimized into the DEFAULT user hive
#    NEVER the current-user hive: the automation user has no profile yet at
#    provision time, so a current-user write would land in the provisioning
#    context's hive instead (RESEARCH.md Pitfall 1).
#    Handles MUST be released ([gc]::Collect) before reg unload or it fails.
#    reg add /f is inherently idempotent.
# =============================================================================
Write-Host "[Configure-Target] (2/4) Default-user-hive DPI + SuppressWhenMinimized..."

$defaultHive = 'HKLM\DEFAULT_USER'
$defaultNtUser = 'C:\Users\Default\NTUSER.DAT'

reg load $defaultHive $defaultNtUser | Out-Null
try {
    reg add "$defaultHive\Control Panel\Desktop" /v LogPixels      /t REG_DWORD /d 96 /f | Out-Null
    reg add "$defaultHive\Control Panel\Desktop" /v Win8DpiScaling /t REG_DWORD /d 1  /f | Out-Null
    reg add "$defaultHive\Software\Microsoft\Terminal Server Client" `
        /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
    Write-Host "[Configure-Target]   wrote LogPixels=96, Win8DpiScaling=1, SuppressWhenMinimized=2 to default hive."
} finally {
    # Release any handles into the loaded hive before unloading, or unload fails.
    [gc]::Collect()
    [gc]::WaitForPendingFinalizers()
    reg unload $defaultHive | Out-Null
    Write-Host "[Configure-Target]   default user hive unloaded."
}

# =============================================================================
# 3. Machine-wide RemoteDesktop_SuppressWhenMinimized=2 (HKLM + Wow6432Node)
#    reg add /f is idempotent.
# =============================================================================
Write-Host "[Configure-Target] (3/5) Machine-wide SuppressWhenMinimized..."

reg add "HKLM\Software\Microsoft\Terminal Server Client" `
    /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
reg add "HKLM\Software\Wow6432Node\Microsoft\Terminal Server Client" `
    /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
Write-Host "[Configure-Target]   SuppressWhenMinimized=2 set at HKLM + Wow6432Node."

# =============================================================================
# 4. Suppress Server Manager auto-launch at logon (DEFAULT user hive).
#    Server Manager otherwise opens maximized on every interactive logon and
#    covers the full desktop -- discovered live during Phase 3 input-injection
#    validation, where it silently absorbed clicks and Alt+F4 intended for the
#    bare desktop (a click at "desktop center" landed on Server Manager's own
#    window; Alt+F4 targeted its window instead of opening the Shut Down
#    Windows dialog). Written to the DEFAULT hive (same rationale as DPI/
#    SuppressWhenMinimized above: the automation user's profile does not exist
#    at provision time). reg add /f is idempotent.
# =============================================================================
Write-Host "[Configure-Target] (4/5) Suppress Server Manager auto-launch..."

reg load $defaultHive $defaultNtUser | Out-Null
try {
    reg add "$defaultHive\Software\Microsoft\ServerManager" `
        /v DoNotOpenServerManagerAtLogon /t REG_DWORD /d 1 /f | Out-Null
    Write-Host "[Configure-Target]   wrote DoNotOpenServerManagerAtLogon=1 to default hive."
} finally {
    [gc]::Collect()
    [gc]::WaitForPendingFinalizers()
    reg unload $defaultHive | Out-Null
    Write-Host "[Configure-Target]   default user hive unloaded."
}

# =============================================================================
# 5. 7-Zip silent install -- ONLY after SHA-256 verification of the download.
#    Guarded by the presence of 7zFM.exe so a re-run is a no-op. The expected
#    hash is the pinned, human-approved constant; a mismatch throws BEFORE any
#    installer runs (RESEARCH.md Package Audit / anti-pattern: never "latest").
# =============================================================================
Write-Host "[Configure-Target] (5/5) 7-Zip $SevenZipVersion install (SHA-256 gated)..."

if (-not (Test-Path $SevenZipFmExe)) {
    $installer = Join-Path $env:TEMP "7z$($SevenZipVersion.Replace('.',''))-x64.exe"

    # TLS 1.2 for the download (older Windows PowerShell defaults can be too low).
    [Net.ServicePointManager]::SecurityProtocol = `
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    Write-Host "[Configure-Target]   downloading $SevenZipUrl ..."
    Invoke-WebRequest -Uri $SevenZipUrl -OutFile $installer -UseBasicParsing

    $actualHash = (Get-FileHash -Path $installer -Algorithm SHA256).Hash
    if ($actualHash -ne $SevenZipSha256) {
        Remove-Item $installer -Force -ErrorAction SilentlyContinue
        throw "7-Zip SHA-256 mismatch: expected $SevenZipSha256 but got $actualHash. Aborting install."
    }
    Write-Host "[Configure-Target]   SHA-256 verified ($actualHash) -- installing silently."

    Start-Process -FilePath $installer -ArgumentList '/S' -Wait
    Remove-Item $installer -Force -ErrorAction SilentlyContinue

    if (-not (Test-Path $SevenZipFmExe)) {
        throw "7-Zip install completed but $SevenZipFmExe was not found."
    }
    Write-Host "[Configure-Target]   7-Zip installed."
} else {
    Write-Host "[Configure-Target]   7-Zip already present ($SevenZipFmExe) -- skipped."
}

Write-Host "[Configure-Target] Done -- all in-guest hardening applied (no reboot performed)."
