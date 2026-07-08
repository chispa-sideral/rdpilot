<#
    sensor-responder.ps1 — THROWAWAY Phase 4 DVC validation responder ONLY.

    Deliberately written in PowerShell (D-4.2): this script exists solely to
    prove the RDPILOT_SENSOR dynamic virtual channel transport end-to-end
    (SENSOR-03, SC#2/SC#3) against a live RDP target. It is DISCARDED before
    Phase 5's real sensor exists and its language choice MUST NOT be read as
    a preference for (or against) either candidate in Phase 5's C# .NET 8
    NativeAOT vs. all-Rust sensor-language decision (D-4.2, still DEFERRED).

    What it does:
      1. Opens the server-side end of the RDPILOT_SENSOR dynamic virtual
         channel via WTSVirtualChannelOpenEx (wtsapi32.dll, WTS_CURRENT_SESSION,
         WTS_CHANNEL_OPTION_DYNAMIC), in a retry poll loop that tolerates the
         documented ERROR_GEN_FAILURE / 0x31 timing race (RESEARCH Pitfall 1):
         the client's DVC listener may not have finished negotiating with the
         server the instant this script starts, so the OS call can transiently
         fail — polling a few times a half-second apart resolves it.
      2. Reads the SDK's Version handshake (always the FIRST message on the
         wire — DvcProcessor::start() fires unconditionally on channel
         creation, RESEARCH Q2) and echoes its own version back in the exact
         same envelope shape, `{ version, req_id, type, payload }`.
      3. Loops reading further envelopes; for each `type -eq "Ping"`, replies
         with `type = "Pong"` carrying the SAME req_id (correlation id) so the
         SDK's per-request oneshot map resolves the right caller.

    NOTE on session targeting (the primary live-verify integration risk):
    WTSVirtualChannelOpenEx(WTS_CURRENT_SESSION, ...) must execute INSIDE the
    interactive RDP session the SDK's connection creates (Session > 0) — NOT
    inside a WinRM remoting session, which always runs in Session 0. This
    script does not care which session it happens to be launched in; it is
    deploy-responder.ps1's job (via a scheduled task at logon/session-connect)
    to arm this script so the OS actually starts it there. If every
    WTSVirtualChannelOpenEx attempt in the retry loop below fails, the first
    thing to check is which session this process is running in
    (`(Get-Process -Id $PID).SessionId` from an elevated troubleshooting
    session) — session 0 means deploy-responder.ps1's launch mechanism needs
    adjustment (T-04-08).

    Not compiled, not run by `cargo` — a text fixture only. Deleted before
    Phase 5 (D-4.1).
#>
[CmdletBinding()]
param(
    # Number of WTSVirtualChannelOpenEx retry attempts before giving up.
    [int]$OpenRetries = 20,

    # Delay between retry attempts, in milliseconds.
    [int]$OpenRetryDelayMs = 500
)

$ErrorActionPreference = 'Stop'

# --- WTS API P/Invoke surface (RESEARCH Q3 — verified against MS Learn docs +
#     the windows-sys crate's generated WTS_CHANNEL_OPTION_DYNAMIC constant). ---
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class RdpilotWts {
    public const uint WTS_CURRENT_SESSION = 0xFFFFFFFF;
    public const uint WTS_CHANNEL_OPTION_DYNAMIC = 0x1;

    [DllImport("wtsapi32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
    public static extern IntPtr WTSVirtualChannelOpenEx(uint SessionId, string pVirtualName, uint flags);

    [DllImport("wtsapi32.dll", SetLastError = true)]
    public static extern bool WTSVirtualChannelRead(IntPtr hChannelHandle, uint TimeOut, byte[] Buffer, uint BufferSize, out uint pBytesRead);

    [DllImport("wtsapi32.dll", SetLastError = true)]
    public static extern bool WTSVirtualChannelWrite(IntPtr hChannelHandle, byte[] Buffer, uint Length, out uint pBytesWritten);

    [DllImport("wtsapi32.dll", SetLastError = true)]
    public static extern bool WTSVirtualChannelClose(IntPtr hChannelHandle);
}
'@

# Must match crates/rdpilot/src/connect.rs's RDPILOT_SENSOR const byte-for-byte.
$channelName = "RDPILOT_SENSOR"
$protocolVersion = 1
$handle = [IntPtr]::Zero

Write-Host "[sensor-responder] opening '$channelName' (retrying up to $OpenRetries times, $OpenRetryDelayMs ms apart)..."

# Retry loop: survives the ERROR_GEN_FAILURE (0x31) timing race documented in
# RESEARCH Pitfall 1 — do NOT fail on the first attempt.
for ($i = 0; $i -lt $OpenRetries -and $handle -eq [IntPtr]::Zero; $i++) {
    $handle = [RdpilotWts]::WTSVirtualChannelOpenEx(
        [RdpilotWts]::WTS_CURRENT_SESSION, $channelName, [RdpilotWts]::WTS_CHANNEL_OPTION_DYNAMIC)
    if ($handle -eq [IntPtr]::Zero) {
        Start-Sleep -Milliseconds $OpenRetryDelayMs
    }
}
if ($handle -eq [IntPtr]::Zero) {
    $err = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
    throw "WTSVirtualChannelOpenEx failed after $OpenRetries retries (GetLastError=$err). Is this process running in the INTERACTIVE RDP session (Session > 0), not WinRM's Session 0? See the NOTE at the top of this script."
}
Write-Host "[sensor-responder] channel open."

function Read-Envelope {
    # Do NOT call WTSVirtualChannelQuery to convert this handle to a raw file
    # handle — unnecessary for a synchronous script (RESEARCH Q3 Anti-Pattern).
    # Do NOT re-frame reads — the OS-level WTS layer already presents one
    # WTSVirtualChannelWrite as one logical unit to the paired Read call
    # (RESEARCH "Don't Hand-Roll": server-side DVC message framing).
    $buf = New-Object byte[] 4096
    [uint32]$bytesRead = 0
    $ok = [RdpilotWts]::WTSVirtualChannelRead($handle, 5000, $buf, $buf.Length, [ref]$bytesRead)
    if (-not $ok -or $bytesRead -eq 0) { return $null }
    $json = [System.Text.Encoding]::UTF8.GetString($buf, 0, $bytesRead)
    try {
        return $json | ConvertFrom-Json
    } catch {
        Write-Host "[sensor-responder] dropped malformed envelope: $($_.Exception.Message)"
        return $null
    }
}

function Write-Envelope {
    param($Obj)
    $json = $Obj | ConvertTo-Json -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    [uint32]$written = 0
    [RdpilotWts]::WTSVirtualChannelWrite($handle, $bytes, $bytes.Length, [ref]$written) | Out-Null
}

try {
    # The FIRST message on the wire is always the SDK's Version handshake
    # (DvcProcessor::start() fires unconditionally the instant the channel is
    # created — RESEARCH Q2). Read it, then echo OUR OWN version back in the
    # identical envelope shape (no separate "handshake ack" message type).
    $version = $null
    while ($null -eq $version) {
        $version = Read-Envelope
    }
    Write-Host "[sensor-responder] received Version handshake (peer version=$($version.version)); echoing version=$protocolVersion"
    Write-Envelope @{ version = $protocolVersion; req_id = 0; type = "Version"; payload = $null }

    Write-Host "[sensor-responder] handshake complete; answering Ping -> Pong..."
    while ($true) {
        $msg = Read-Envelope
        if ($null -eq $msg) { continue }
        if ($msg.type -eq "Ping") {
            Write-Envelope @{ version = $protocolVersion; req_id = $msg.req_id; type = "Pong"; payload = $null }
        }
        # Any other message type this v1 protocol doesn't define is silently
        # ignored rather than crashing the responder (mirrors the SDK's own
        # malformed-envelope-drops-not-panics convention, API-01).
    }
} finally {
    if ($handle -ne [IntPtr]::Zero) {
        [RdpilotWts]::WTSVirtualChannelClose($handle) | Out-Null
    }
}
