# Phase 4: DVC Transport Channel - Research

**Researched:** 2026-07-09
**Domain:** IronRDP Dynamic Virtual Channel (client side) + Windows WTS API DVC server-side opening (PowerShell)
**Confidence:** HIGH — every load-bearing API claim below is grounded in the actual pinned crate source (`ironrdp-dvc 0.6.0`, `ironrdp-session 0.9.0`, `ironrdp-core 0.2.0` read directly from `~/.cargo/registry/src`) or official Microsoft Learn docs, not training-data recall.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-4.1 (ping-validation strategy):** A minimal, disposable server-side responder is deployed to the live Azure VM (Phase 1). It opens `RDPILOT_SENSOR` server-side, answers pong, and participates in the version handshake. It is discarded before Phase 5's real sensor exists. This is the real end-to-end live gate for success criteria 2 and 3. Rejected: local `ironrdp-dvc-pipe-proxy` loopback (does not prove real-RDP transport); deferring proof to Phase 5 (leaves SENSOR-03 unproven on its own merits).
- **D-4.2 (sensor language — unchanged):** The C# .NET 8 NativeAOT vs. all-Rust sensor decision remains DEFERRED per PROJECT.md. This phase's throwaway responder is written in whatever is cheapest and must NOT prejudice the Phase 5 choice. Recommended: PowerShell, since WinRM + `pwsh` are already present on the target from Phase 1.
- **D-4.3 (protocol scope):** Design a durable request/response envelope now — `{ version, req_id, type, payload }` (`req_id` is a correlation id reserved for later concurrent queries) — but implement ONLY Version, Ping, and Pong message types this phase. Phases 6-7 add WindowList/ProcessTree/Uia message types without reworking framing. Reuse `ironrdp-dvc`'s chunking/reassembly (`encode_dvc_messages`, DataFirst/Data PDUs) rather than hand-rolling framing.
- **D-4.4 (encoding):** JSON via `serde`/`serde_json`. Promote `serde` + `serde_json` from dev-dependencies to runtime `[dependencies]` and add the `derive` feature. Rejected: compact binary encoding.
- **D-4.5 (sensor delivery direction — spans Phase 4/5):** This phase's throwaway responder ships via WinRM (`Copy-Item -ToSession` or inline base64 write) + `Start-Process`. Zero new Rust crates; the DVC channel is the thing under test, so delivery must not also exercise unproven RDPDR plumbing. (Phase 5's real-sensor delivery mechanism — RDPDR primary, WinRM fallback, blob+SAS tertiary — is recorded in CONTEXT.md for that phase's planning, not decided here.)

### Claude's Discretion

- Exact throwaway-responder script structure/naming (as long as it opens the DVC channel server-side and answers pong + version handshake).
- Internal `Error::Dvc(String)` message wording, mirroring the existing `Error` enum's style.
- Exact reply-channel design for the outbound ping — this was the open research item; **resolved below** (see "Resolved Open Questions").

### Deferred Ideas (OUT OF SCOPE)

- Sensor implementation language (C# .NET 8 NativeAOT vs. all-Rust) — Phase 5.
- Real sensor bootstrap/deployment (RDPDR drive redirection primary, WinRM fallback, blob+SAS tertiary) — Phase 5 (SENSOR-01/SENSOR-02).
- CLIPRDR clipboard file-copy delivery — dropped entirely, not deferred.
- Azure Trusted Signing / Authenticode — Phase 5 concern.
- WindowList / ProcessTree / Uia message types — Phases 6-7. The envelope (D-4.3) accommodates them without reworking framing but they are not implemented here.
- Application-layer channel authentication (Pitfall m3) — not addressed this phase; DVC remains unauthenticated transport.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SENSOR-03 | "A DVC request/response transport channel carries structured-perception data between the SDK and the sensor" | Resolved Open Questions #1-#3 below give the concrete `DvcProcessor`/`ActiveStage` wiring, wire envelope, and server-side WTS opening mechanism that together satisfy this requirement's three success criteria (registration-before-connect, ping/pong ≤500ms, version handshake first-message-with-clear-error-on-mismatch). |
</phase_requirements>

## Summary

Phase 4 turns the empty DVC seam left at `connect.rs:87-96` into a working bidirectional channel. Three concrete engineering questions blocked planning; all three are now resolved against the **actual pinned API surface** (`ironrdp-dvc 0.6.0` / `ironrdp-session 0.9.0` / `ironrdp-core 0.2.0`, read directly from the local cargo registry, not training-data guesswork):

1. **Outbound ping design:** `ironrdp-dvc`'s `DvcProcessor::process()`/`start()` are *reactive only* — a `DvcProcessor` cannot push data on its own initiative. The correct proactive-send mechanism is `ActiveStage::encode_dvc_messages(&mut self, messages: Vec<SvcMessage>) -> SessionResult<Vec<u8>>` combined with `ActiveStage::get_dvc::<T>()` + `DynamicVirtualChannel::channel_id()` — **this is the exact pattern IronRDP itself uses internally for `ActiveStage::encode_resize()` (display-control DVC)**, giving a directly-precedented, HIGH-confidence design rather than a novel one. Correlate requests/replies via a `req_id`-keyed map of `tokio::sync::oneshot::Sender`s shared (`Arc<Mutex<...>>`) between the `Session` handle and the `RdpilotSensorProcessor`; `Session::ping()` wraps the `oneshot::Receiver` in `tokio::time::timeout(Duration::from_millis(500), rx)`.
2. **Version handshake wire format:** The client (SDK) side *always* sends first — `DvcProcessor::start()` fires the instant the server's DYNVC_CREATE_REQ succeeds, which is strictly before any inbound data can arrive. `{"version":1,"req_id":0,"type":"Version","payload":null}` is the first bytes on the wire; the server-side PowerShell responder echoes its own version back in the same shape. A mismatch is detected client-side in `process()`, is recorded in an `Arc<Mutex<HandshakeState>>` (no fourth message type needed), and every subsequent `Session::ping()` call fails fast with `Error::Dvc(...)` rather than attempting a round trip against a channel the SDK knows is unusable.
3. **Server-side channel opening:** PowerShell **can** open the DVC directly via `Add-Type` + P/Invoke of `wtsapi32.dll`'s `WTSVirtualChannelOpenEx`/`Read`/`Write`/`Close` — no compiled helper is required for the throwaway responder. This is standard, unprivileged (same-session), well-documented Win32 API surface; confirmed via official Microsoft Learn docs and the `windows-sys` Rust binding for the exact `WTS_CHANNEL_OPTION_DYNAMIC = 1` flag value. The OS-level WTS API already reassembles/frames DVC messages server-side — the PowerShell script does **not** need to reimplement DataFirst/Data chunking (that machinery is purely a client-side `ironrdp-dvc` concern, confirmed by reading `CompleteData`/`encode_dvc_messages` in the crate source).

**Primary recommendation:** Implement `RdpilotSensorProcessor` as a struct holding `Arc<SensorShared>` (pending-pings map + handshake state), register it via `DrdynvcClient::with_dynamic_channel(RdpilotSensorProcessor::new(shared.clone()))` at `connect.rs:95`, thread a second `Arc<SensorShared>` clone through to `Session`, add `RdpInputEvent::Ping(u64)` to `session_loop.rs`'s existing `select!`, and write the throwaway responder as a single `.ps1` file using raw WTS P/Invoke — no compiled binary, no new Rust crate, no prejudice to the Phase 5 sensor-language decision.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| DVC channel registration (`DrdynvcClient::with_dynamic_channel`) | SDK / Session Manager (local, `connect.rs`) | — | Must happen before `connector.connect()` completes — hard IronRDP constraint (already the locked seam from Phase 2). |
| Outbound ping construction + reply correlation | SDK / Session Loop (local, `session_loop.rs` + `session.rs`) | — | The session loop owns the only mutable `ActiveStage`; `Session::ping()` is a thin async caller that message-passes into it (mirrors `send_mouse`/`send_key`). |
| Version-handshake enforcement | SDK / DVC Processor (local, new `RdpilotSensorProcessor`) | Session Loop (reads shared handshake state to gate ping) | `process()`/`start()` are the only hooks IronRDP gives a `DvcProcessor`; the mismatch verdict must be readable from outside that struct, hence shared state. |
| Server-side DVC endpoint (WTS API) | Remote target (throwaway PowerShell, in-session) | — | `WTSVirtualChannelOpenEx` must run inside the RDP user session (Session > 0) — this is a Windows OS constraint, not an SDK design choice. |
| Delivery of the throwaway responder script | WinRM (existing Phase 1 channel) | — | D-4.5: reuses proven Phase 1 WinRM plumbing; does not exercise unproven RDPDR. |

## Resolved Open Questions

### Q1 — Outbound reply-channel design for `Session::ping()`

**The core constraint (verified in `ironrdp-dvc-0.6.0/src/lib.rs`):**

```rust
pub trait DvcProcessor: AsAny + Send {
    fn channel_name(&self) -> &str;
    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>>;   // fires ONCE, on channel creation
    fn process(&mut self, channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>>; // fires ONLY on inbound data
    fn close(&mut self, _channel_id: u32) {}
}
```

Both hooks are **reactive**: `start()` fires once when the server creates the channel; `process()` fires only in response to inbound bytes. Neither gives the processor a way to originate a send on its own schedule — there is no `fn tick(&mut self)` or similar. A `Session::ping()` call must therefore inject the outbound bytes from *outside* the `DvcProcessor`, through the session loop that owns `ActiveStage`.

**The mechanism (verified in `ironrdp-session-0.9.0/src/active_stage.rs`) — and it is a directly-precedented pattern, not novel:**

```rust
// ActiveStage (ironrdp-session 0.9.0), public API:
pub fn get_dvc<T: DvcProcessor + 'static>(&mut self) -> Option<&DynamicVirtualChannel>;
pub fn get_dvc_by_channel_id(&mut self, channel_id: u32) -> Option<&DynamicVirtualChannel>;
pub fn encode_dvc_messages(&mut self, messages: Vec<SvcMessage>) -> SessionResult<Vec<u8>>;

// DynamicVirtualChannel (ironrdp-dvc 0.6.0), public API:
pub fn channel_id(&self) -> Option<DynamicChannelId>; // DynamicChannelId = u32
pub fn channel_processor_downcast_ref<T: DvcProcessor>(&self) -> Option<&T>;

// Free function (ironrdp-dvc 0.6.0):
pub fn encode_dvc_messages(channel_id: u32, messages: Vec<DvcMessage>, flags: ironrdp_svc::ChannelFlags)
    -> EncodeResult<Vec<SvcMessage>>;  // handles DataFirst/Data chunking automatically
```

`ActiveStage::encode_resize()` (lines 272-304 of `active_stage.rs`) is IronRDP's **own** internal implementation of "proactively push a message on a DVC channel from outside the reactive `process()`/`start()` hooks" — it does this for the Display Control Virtual Channel (`DisplayControlClient`). Our `Session::ping()` should copy this exact shape:

```rust
// session_loop.rs — new RdpInputEvent::Ping(req_id) arm, mirrors encode_resize's structure:
Some(RdpInputEvent::Ping(req_id)) => {
    let (channel_id, dvc_messages) = {
        let dvc = active_stage
            .get_dvc::<RdpilotSensorProcessor>()
            .ok_or_else(|| Error::Dvc("sensor channel not registered".to_owned()))?;
        let channel_id = dvc
            .channel_id()
            .ok_or_else(|| Error::Dvc("sensor channel not yet open".to_owned()))?;
        let processor = dvc
            .channel_processor_downcast_ref::<RdpilotSensorProcessor>()
            .ok_or_else(|| Error::Dvc("sensor processor downcast failed".to_owned()))?;
        (channel_id, processor.encode_ping(req_id).map_err(|e| Error::Dvc(e.to_string()))?)
    }; // the `dvc`/`&mut active_stage` borrow ends here — required before the next `&mut` call
    let svc_messages = ironrdp_dvc::encode_dvc_messages(channel_id, dvc_messages, ironrdp_svc::ChannelFlags::empty())
        .map_err(|e| Error::Dvc(e.to_string()))?;
    vec![ActiveStageOutput::ResponseFrame(
        active_stage.encode_dvc_messages(svc_messages).map_err(|e| Error::Dvc(e.to_string()))?,
    )]
}
```

The block-scoped borrow is load-bearing: `get_dvc()` takes `&mut self` and returns a borrow of `active_stage`; `encode_dvc_messages()` also needs `&mut self`, so the first borrow must end (owned `channel_id`/`Vec<DvcMessage>` extracted) before the second call — this exact sequencing is what `encode_resize()` itself does internally.

**Crossing the OS-thread boundary (the actual open question):** `Session::ping()` is called from the caller's async context; the session loop runs on a **separate dedicated OS thread** with its own current-thread Tokio runtime (`session.rs:126-136`, documented reason: a higher-ranked-lifetime limitation in `tokio::spawn`'s auto-`Send` inference during reactivation). `tokio::sync::oneshot` channels are runtime-agnostic — the `Sender` half can be created and stored on the caller's thread/runtime and `.send()`'d from the session-loop thread's runtime without either side needing to be on the same executor. Recommended shared state:

```rust
// New small module, e.g. src/sensor.rs
pub(crate) struct SensorShared {
    pending: Mutex<HashMap<u64, oneshot::Sender<()>>>,
    handshake: Mutex<HandshakeState>,
}
pub(crate) enum HandshakeState { Pending, Ok, Mismatched { local: u32, remote: u32 } }
```

- `connect.rs` constructs one `Arc<SensorShared>`, clones it into `RdpilotSensorProcessor::new(shared.clone())` (registered via `.with_dynamic_channel(...)` at the existing seam), and returns the other clone up through `connect::connect()`'s return tuple so `Session::connect()` can store it on `Session`.
- `Session::ping()`: check `handshake` state first (fail fast with `Error::Dvc` if `Mismatched`, per Q2 below); otherwise allocate `req_id` from an `AtomicU64` on `Session`, insert a fresh `oneshot::channel()`'s sender into `pending`, send `RdpInputEvent::Ping(req_id)` into the loop, then:
  ```rust
  match tokio::time::timeout(Duration::from_millis(500), rx).await {
      Ok(Ok(())) => Ok(elapsed),
      Ok(Err(_)) => Err(Error::Dvc("sensor channel closed before replying".to_owned())),
      Err(_) => {
          self.sensor.pending.lock()...remove(&req_id); // avoid a leaked entry on timeout
          Err(Error::Dvc("ping timed out after 500ms".to_owned()))
      }
  }
  ```
- `RdpilotSensorProcessor::process()` (fires when the Pong's bytes arrive), on `type == "Pong"`: `pending.lock()...remove(&req_id)` and `.send(())` on the recovered sender (ignore the `Result` — the receiver may already be gone if the timeout fired first).

This is the hybrid of the CONTEXT.md's candidate (a) (`oneshot` carried per-request) and (b) (shared state the `DvcProcessor` updates) — using a **map** rather than a single `Notify` is what makes the `req_id` field in D-4.3's envelope actually useful (future concurrent Phase 6-7 queries reuse the same correlation map without redesign).

**Confidence:** HIGH — `ActiveStage::encode_resize` is read directly from the pinned `ironrdp-session-0.9.0` source and is functionally identical to what `Session::ping()` needs to do.

### Q2 — Version-handshake wire format specifics

**Which side sends first:** The client (SDK). `DvcProcessor::start()` "returns any messages that should be sent immediately upon the channel being created" (doc comment, `ironrdp-dvc-0.6.0/src/lib.rs:49-51`) and fires as soon as the server's `DYNVC_CREATE_REQ` succeeds — this is strictly before any inbound payload can reach `process()`. This directly satisfies D-4.3/CONTEXT's "start() must send the version handshake as the very first outbound message" and answers "which side sends first": **always the client, unconditionally, with no server prompt.**

**Exact JSON shape** (envelope per D-4.3: `{ version, req_id, type, payload }`):

```jsonc
// SDK -> responder, sent from start(), req_id 0 reserved for the handshake
{"version": 1, "req_id": 0, "type": "Version", "payload": null}

// responder -> SDK, echoes back its OWN version in the same shape (no new message type)
{"version": 1, "req_id": 0, "type": "Version", "payload": null}

// SDK -> responder, Session::ping()
{"version": 1, "req_id": 7, "type": "Ping", "payload": null}

// responder -> SDK
{"version": 1, "req_id": 7, "type": "Pong", "payload": null}
```

`req_id` is a `u64` counter on `Session`, starting at 1 for real pings (0 is reserved/unused for the handshake exchange, which needs no correlation since it's a fixed one-shot at channel open). No fourth "VersionMismatch" message type is needed — the client detects the mismatch itself by comparing the reply's `version` field against its own `PROTOCOL_VERSION` constant, so the implemented type set stays exactly Version/Ping/Pong as D-4.3 specifies.

**Mismatch handling → `Error::Dvc` + "closes the channel":** `RdpilotSensorProcessor::process()` on receiving the `Version` reply:

```rust
MsgType::Version => {
    let mut hs = self.shared.handshake.lock()...;
    *hs = if envelope.version == PROTOCOL_VERSION {
        HandshakeState::Ok
    } else {
        HandshakeState::Mismatched { local: PROTOCOL_VERSION, remote: envelope.version }
    };
    Ok(Vec::new()) // no reply needed; the handshake is a terminal 2-message exchange
}
```

Recommendation: treat "closes the channel with a clear error" as **the SDK marking the channel unusable for its own callers**, not necessarily tearing down the whole RDP session. `Session::ping()` checks `handshake` state before attempting any round trip and returns `Error::Dvc("sensor version mismatch: local vN vs remote vM")` immediately, without sending anything, if `Mismatched`. This satisfies success criterion 3 ("a mismatch closes the channel with a clear error, not silent corruption") at the point where a caller actually tries to use the channel, which is both simpler and testable without a live version-skewed target (the phase's own throwaway responder always matches, since the same author writes both sides — a live test asserting a real mismatch is not practically constructible in-phase; document this as a known coverage gap, see Open Questions below).

A stronger (optional, stretch) variant exists if the planner wants to actively send a `DYNVC_CLOSE` PDU on mismatch: `DrdynvcClient::close_channel(channel_id) -> Option<SvcMessage>` is reachable via `active_stage.get_svc_processor_mut::<DrdynvcClient>()` from the session loop (NOT from inside `RdpilotSensorProcessor::process()`, which has no handle to `DrdynvcClient`). This requires the session loop's `read_pdu` arm to check the shared `handshake` state after every `active_stage.process()` call and, if freshly `Mismatched`, encode+send the close PDU and terminate the loop with `Error::Dvc(...)`. Recommended as a v2 hardening, not required for this phase's success criteria.

**Confidence:** HIGH for the `start()`-fires-first mechanics (read directly from crate source); MEDIUM for the "session stays alive after mismatch, only future pings error" design choice (a reasonable interpretation of ambiguous CONTEXT wording, not itself sourced from an external spec) — flagged in Assumptions Log as A1.

### Q3 — How the throwaway PowerShell responder opens the server-side DVC channel

**Confirmed: PowerShell via `Add-Type` + raw P/Invoke is viable and is the recommended approach — no compiled helper needed.**

`WTSVirtualChannelOpenEx` (`wtsapi32.dll`) is an ordinary unprivileged Win32 API call when opening a channel in **your own** session (`WTS_CURRENT_SESSION`); no elevation or special permission is required for that case (the "Virtual Channels permission" MS docs mention is only for opening *another* user's session — [MS Learn](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelopenex)). "After a DVC is created, you can use the same functions for Read, Write, Query, or Close that are used for the SVC" (same doc) — i.e. plain `WTSVirtualChannelRead`/`WTSVirtualChannelWrite`/`WTSVirtualChannelClose` directly on the returned `HANDLE`, **no** `WTSVirtualChannelQuery(..., WTSVirtualFileHandle, ...)` conversion to a raw file handle is required for a simple synchronous script (that conversion is only needed for overlapped/async I/O, used by the native C/C++ samples, not needed here).

**Exact signatures and constants (verified: MS Learn function pages + `windows-sys` crate's generated bindings for the flag value):**

```csharp
[DllImport("wtsapi32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
static extern IntPtr WTSVirtualChannelOpenEx(uint SessionId, string pVirtualName, uint flags);

[DllImport("wtsapi32.dll", SetLastError = true)]
static extern bool WTSVirtualChannelRead(IntPtr hChannelHandle, uint TimeOut, byte[] Buffer, uint BufferSize, out uint pBytesRead);

[DllImport("wtsapi32.dll", SetLastError = true)]
static extern bool WTSVirtualChannelWrite(IntPtr hChannelHandle, byte[] Buffer, uint Length, out uint pBytesWritten);

[DllImport("wtsapi32.dll", SetLastError = true)]
static extern bool WTSVirtualChannelClose(IntPtr hChannelHandle);

const uint WTS_CURRENT_SESSION = 0xFFFFFFFF;      // (DWORD)-1
const uint WTS_CHANNEL_OPTION_DYNAMIC = 0x1;       // confirmed via windows-sys generated constant
```

Sources: [WTSVirtualChannelOpenEx](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelopenex), [WTSVirtualChannelRead](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelread), [WTSVirtualChannelWrite](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelwrite), [windows-sys WTS_CHANNEL_OPTION_DYNAMIC = 1](https://docs.rs/windows-sys/latest/windows_sys/Win32/System/RemoteDesktop/constant.WTS_CHANNEL_OPTION_DYNAMIC.html).

**Minimal script structure** (`.ps1`, delivered via WinRM per D-4.5, run via `Start-Process pwsh -ArgumentList "-File responder.ps1"` or `Invoke-Command`):

```powershell
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class Wts {
    public const uint WTS_CURRENT_SESSION = 0xFFFFFFFF;
    public const uint WTS_CHANNEL_OPTION_DYNAMIC = 0x1;
    [DllImport("wtsapi32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
    public static extern IntPtr WTSVirtualChannelOpenEx(uint SessionId, string pVirtualName, uint flags);
    [DllImport("wtsapi32.dll", SetLastError = true)]
    public static extern bool WTSVirtualChannelRead(IntPtr h, uint TimeOut, byte[] Buf, uint BufSize, out uint BytesRead);
    [DllImport("wtsapi32.dll", SetLastError = true)]
    public static extern bool WTSVirtualChannelWrite(IntPtr h, byte[] Buf, uint Len, out uint BytesWritten);
    [DllImport("wtsapi32.dll", SetLastError = true)]
    public static extern bool WTSVirtualChannelClose(IntPtr h);
}
'@

$channelName = "RDPILOT_SENSOR"
$protocolVersion = 1
$handle = [IntPtr]::Zero

# Retry: the client's DVC listener may not be negotiated the instant this
# script starts (documented ERROR_GEN_FAILURE / 0x31 timing race — see
# Common Pitfalls). Poll rather than fail on the first attempt.
for ($i = 0; $i -lt 20 -and $handle -eq [IntPtr]::Zero; $i++) {
    $handle = [Wts]::WTSVirtualChannelOpenEx([Wts]::WTS_CURRENT_SESSION, $channelName, [Wts]::WTS_CHANNEL_OPTION_DYNAMIC)
    if ($handle -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 500 }
}
if ($handle -eq [IntPtr]::Zero) { throw "WTSVirtualChannelOpenEx failed after retries (GetLastError=$([Runtime.InteropServices.Marshal]::GetLastWin32Error()))" }

function Read-Envelope {
    $buf = New-Object byte[] 4096
    $bytesRead = 0
    if (-not [Wts]::WTSVirtualChannelRead($handle, 5000, $buf, $buf.Length, [ref]$bytesRead)) { return $null }
    if ($bytesRead -eq 0) { return $null }
    return [System.Text.Encoding]::UTF8.GetString($buf, 0, $bytesRead) | ConvertFrom-Json
}
function Write-Envelope($obj) {
    $json = $obj | ConvertTo-Json -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $written = 0
    [Wts]::WTSVirtualChannelWrite($handle, $bytes, $bytes.Length, [ref]$written) | Out-Null
}

# First message from the SDK is ALWAYS the Version handshake (start() fires
# unconditionally on channel creation) — read it, echo our own version back.
$version = Read-Envelope
Write-Envelope @{ version = $protocolVersion; req_id = 0; type = "Version"; payload = $null }

while ($true) {
    $msg = Read-Envelope
    if ($null -eq $msg) { continue }
    if ($msg.type -eq "Ping") {
        Write-Envelope @{ version = $protocolVersion; req_id = $msg.req_id; type = "Pong"; payload = $null }
    }
}
```

**Why no compiled helper is required, and why this doesn't prejudice D-4.2:** the script uses only `wtsapi32.dll` (present on every Windows target) and built-in PowerShell JSON cmdlets — nothing about it implies or forecloses a C#/.NET-8-NativeAOT vs. all-Rust choice for the Phase 5 real sensor; it is explicitly throwaway (D-4.1) and deleted before Phase 5 exists.

**Confidence:** HIGH — signatures and flag value confirmed against official Microsoft Learn docs and an authoritative Windows-metadata-generated Rust binding, not solely community blog posts.

## Standard Stack

### Core (already pinned in `Cargo.toml`/`Cargo.lock` — no new crates)

| Library | Pinned Version | Purpose | Why |
|---------|-----|---------|-----|
| `ironrdp-dvc` | 0.6.0 [VERIFIED: Cargo.lock] | `DvcProcessor` trait, `encode_dvc_messages` free fn, `DrdynvcClient` | Already a runtime dependency; this phase implements the seam it left open |
| `ironrdp-session` | 0.9.0 [VERIFIED: Cargo.lock] | `ActiveStage::{get_dvc, encode_dvc_messages}` — the proactive-send mechanism (Q1) | Transitively pulled in by `ironrdp = { features = ["session", ...] }`; not a direct dependency to add, but its public surface is what `session_loop.rs` calls |
| `ironrdp-core` | 0.2.0 [VERIFIED: Cargo.lock] | `Encode` trait (for the `DvcMessage` JSON wrapper), `WriteCursor::write_slice`, `ensure_size!` macro | Needed to implement `DvcEncode`/`Encode` for the outbound JSON envelope |
| `serde` (+ `derive`) | 1.x [ASSUMED — version not re-verified this session, was already dev-dependency-pinned] | Envelope struct (de)serialization | D-4.4: promote from dev-dep to runtime dep |
| `serde_json` | 1.x [ASSUMED — same as above] | JSON encode/decode of the envelope | D-4.4 |

**Cargo.toml change required:** move the existing `serde_json = "1"` dev-dependency line to `[dependencies]`, and add `serde = { version = "1", features = ["derive"] }` to `[dependencies]` (currently absent entirely — `serde_json` alone doesn't provide `Serialize`/`Deserialize` derive macros).

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tokio::sync::oneshot` | (part of `tokio`, already a dependency) | Per-`req_id` ping/pong correlation across the OS-thread boundary | `Session::ping()` reply path (Q1) |
| `std::sync::{Arc, Mutex}` | stdlib | Shared `SensorShared` state (pending map + handshake state) between `RdpilotSensorProcessor` and `Session` | Same as above |

### Package Legitimacy Audit

No new external packages are introduced this phase — `ironrdp-dvc`, `ironrdp-session`, `ironrdp-core`, `serde`, `serde_json` are all already present in `Cargo.lock` (pinned, already resolved from crates.io in a prior phase) or are `tokio`/stdlib. `slopcheck`/registry-verification is not applicable: nothing new is being added to the dependency graph, only a `dev-dependency` → `dependency` promotion of two already-vetted crates (`serde`, `serde_json`).

| Package | Registry | Status | Disposition |
|---------|----------|--------|-------------|
| `serde` | crates.io | Not previously in `[dependencies]`; already resolvable (transitively present via other deps, `serde_json` requires it) | Approved — promote to `[dependencies]` |
| `serde_json` | crates.io | Already a dev-dependency (`Cargo.toml:47`), pinned `"1"` | Approved — promote to `[dependencies]` |

**Packages removed due to slopcheck [SLOP] verdict:** none.
**Packages flagged as suspicious [SUS]:** none.

## Architecture Patterns

### System Architecture Diagram

```
LOCAL (rdpilot SDK)                                    REMOTE (Azure VM, Phase 1)
────────────────────                                    ──────────────────────────
Session::ping()                                          throwaway-responder.ps1
   │ 1. check handshake state (fail fast if Mismatched)      │ (delivered via WinRM,
   │ 2. alloc req_id, insert oneshot into SensorShared        │  D-4.5; Start-Process)
   │ 3. send RdpInputEvent::Ping(req_id) ──┐                  │
   │ 4. await oneshot w/ 500ms timeout      │                 │
   ▼                                        ▼                 │
session_loop::run() tokio::select! ── Ping(req_id) arm        │
   │ get_dvc::<RdpilotSensorProcessor>()                      │
   │ .channel_id() + .encode_ping(req_id)                     │
   │ ironrdp_dvc::encode_dvc_messages(...)  (DataFirst/Data)  │
   │ active_stage.encode_dvc_messages(...) -> bytes           │
   │ writer.write_all(bytes) ─────────────────────────────►  WTSVirtualChannelRead()
   │                                                           │ parse JSON {"type":"Ping",...}
   │                                                           │ WTSVirtualChannelWrite(Pong)
   │  reader.read_pdu() ◄─────────────────────────────────────┘
   │ active_stage.process(...) -> RdpilotSensorProcessor::process()
   │   parses {"type":"Pong","req_id":N}
   │   pending.remove(N).send(())  ───► fulfills the oneshot Session::ping() awaits
   ▼
Session::ping() returns Ok(elapsed) or Err(Error::Dvc("timed out"))

CHANNEL-CREATION PATH (runs once, before any Ping):
connect.rs: DrdynvcClient::new().with_dynamic_channel(RdpilotSensorProcessor::new(shared))
   registered BEFORE connect_begin (hard IronRDP constraint, already the Phase 2 seam)
      │
server sends DYNVC_CREATE_REQ (triggered by the PS script's WTSVirtualChannelOpenEx call)
      │
RdpilotSensorProcessor::start() fires ─► sends {"type":"Version",...} FIRST, unconditionally
      │
server echoes its own Version envelope ─► process() sets HandshakeState::Ok or ::Mismatched
```

### Recommended Project Structure

```
crates/rdpilot/src/
├── connect.rs         # existing — DrdynvcClient::with_dynamic_channel(...) wired here (Q1/Q2)
├── error.rs           # existing — add Error::Dvc(String) variant + category() arm
├── session.rs         # existing — add Session::ping() -> Result<Duration>
├── session_loop.rs     # existing — add RdpInputEvent::Ping(u64) select! arm
├── sensor.rs          # NEW — SensorShared, HandshakeState, RdpilotSensorProcessor, Envelope/MsgType, JsonMessage (DvcEncode wrapper)
└── ...
crates/rdpilot/tests/
├── live_session.rs    # existing — add sensor_ping_pong_under_500ms (gated, RDPILOT_LIVE)
└── fixtures/
    └── sensor-responder.ps1  # NEW — throwaway responder, deployed via WinRM (D-4.5), deleted before Phase 5
```

### Pattern 1: Proactive DVC send via `ActiveStage::encode_dvc_messages`

**What:** The only supported way to push data on a DVC channel that wasn't triggered by an inbound message.
**When to use:** Any future outbound-initiated DVC request (this phase's Ping; Phases 6-7's WindowList/ProcessTree/Uia *requests*, since those too are client-initiated).
**Example:** see the full code block in Resolved Open Questions #1 above — sourced directly from `ironrdp-session-0.9.0/src/active_stage.rs:257-308` (`encode_resize`/`encode_dvc_messages`).

### Pattern 2: Shared correlation state across the session-loop OS-thread boundary

**What:** `Arc<Mutex<HashMap<req_id, oneshot::Sender<T>>>>` constructed once at connect time, one clone living inside the `DvcProcessor` (mutated from the session-loop thread when a reply's bytes are decoded), one clone living on `Session` (mutated from whatever thread calls `Session::ping()`).
**When to use:** Any async request/response pattern that must cross from `Session`'s calling context into the dedicated session-loop OS thread and back. This is the general pattern Phases 6-7 will reuse for `get_window_list()`/`get_process_tree()`/`get_uia_tree()`, which is exactly why D-4.3 reserved `req_id` now.

### Anti-Patterns to Avoid

- **Trying to call `DvcProcessor::process()` or `.start()` directly to "send a ping":** these are inbound-triggered callbacks owned by IronRDP's internal PDU dispatch; they cannot be invoked by the SDK on demand. Always go through `ActiveStage::encode_dvc_messages` + `get_dvc`.
- **Hand-rolling DataFirst/Data PDU chunking:** `ironrdp_dvc::encode_dvc_messages(channel_id, messages, flags)` already does this (splits at `DrdynvcDataPdu::MAX_DATA_SIZE = 1590` bytes) — never construct `pdu::DataFirstPdu`/`pdu::DataPdu` by hand (D-4.3 is explicit about this).
- **Converting the WTS handle via `WTSVirtualChannelQuery(..., WTSVirtualFileHandle, ...)` in the PowerShell responder:** unnecessary complexity for a synchronous throwaway script — plain `WTSVirtualChannelRead`/`Write` work directly on the handle from `WTSVirtualChannelOpenEx`.
- **Opening the DVC on the server side without a retry loop:** `WTSVirtualChannelOpenEx` can transiently fail (`ERROR_GEN_FAILURE`/0x31) if called before the client's registered listener has fully negotiated — always poll a few times with a short sleep (see Common Pitfalls).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| DVC message chunking/reassembly (client side) | Custom length-prefixing over raw DVC payloads | `ironrdp_dvc::encode_dvc_messages` (outbound) + the framework's internal `CompleteData` (inbound, automatic — `process()` already receives fully-reassembled payloads) | D-4.3 explicit; `ironrdp-dvc` already handles DataFirst/Data PDU splitting correctly against the real `MAX_DATA_SIZE=1590` limit |
| Server-side DVC message framing | A custom length-prefix protocol inside the WTS read/write loop | Rely on WTS's own message-stream semantics — one `WTSVirtualChannelWrite` call is presented as one logical unit to a corresponding `WTSVirtualChannelRead` on the OS-managed pipe | The OS-level WTS driver already does this; re-framing on top is redundant and a source of subtle desync bugs |
| Cross-thread async correlation | A custom polling loop / spin-wait on a shared flag | `tokio::sync::oneshot` (runtime-agnostic, works across the dedicated session-loop OS thread boundary) | `oneshot` is exactly designed for this; a hand-rolled polling loop wastes CPU and adds latency variance that would eat into the 500ms budget |

**Key insight:** Every piece of "protocol plumbing" this phase needs (chunking, request/response framing at the OS-channel layer, cross-thread signaling) already has a correct, tested implementation one layer down (`ironrdp-dvc`, WTS API, `tokio`). The only genuinely new code is: the JSON envelope shape, the `RdpilotSensorProcessor` glue, and the throwaway PowerShell script.

## Common Pitfalls

### Pitfall 1: `WTSVirtualChannelOpenEx` timing race (ERROR_GEN_FAILURE / 0x31)

**What goes wrong:** The server-side `WTSVirtualChannelOpenEx` call can fail intermittently with `ERROR_GEN_FAILURE` (0x31) if it runs before the client's registered DVC listener has completed negotiation with the server (documented on a Microsoft Q&A thread for exactly this API).
**Why it happens:** DVC channel creation is a handshake (`DYNVC_CREATE_REQ`/`RESP`) that takes a small but non-zero amount of time after the main RDP session activates; if the responder script starts (e.g., via WinRM `Start-Process`) before that handshake would even be attempted, the OS has nothing to attach to yet.
**How to avoid:** Retry `WTSVirtualChannelOpenEx` in a short poll loop (e.g., 20 attempts × 500ms) rather than failing on the first attempt — see the script sketch in Q3 above.
**Warning signs:** The responder throws immediately on a fresh VM but works fine when manually re-run a few seconds later.

### Pitfall 2: DVC is unauthenticated transport (Pitfall m3, PITFALLS.md)

**What goes wrong:** Any process in the same RDP session as the throwaway responder can, in principle, also call `WTSVirtualChannelOpenEx` on `RDPILOT_SENSOR` and intercept/inject messages — DVC channels are accessible to any code in the session by design.
**Why it happens:** DVC has no built-in application-layer authentication; it inherits only RDP's own TLS/CredSSP transport security.
**How to avoid:** Out of scope for this phase per CONTEXT's Deferred Ideas (application-layer auth is explicitly deferred). Document the limitation; do not treat the channel as a trust boundary. This is acceptable for a single-tenant lab VM (the only target in this phase).
**Warning signs:** N/A for this phase — flagged here purely so the planner does not silently regress trust assumptions in a later phase without revisiting this note.

### Pitfall 3: Borrow-checker ordering when mixing `get_dvc` and `encode_dvc_messages`

**What goes wrong:** `ActiveStage::get_dvc::<T>()` takes `&mut self` and returns a borrow tied to that call; calling `active_stage.encode_dvc_messages(...)` (also `&mut self`) while the first borrow is still alive will not compile.
**Why it happens:** Both methods need mutable access to the same `ActiveStage`, and Rust's borrow checker enforces exclusivity.
**How to avoid:** Extract owned data (the `channel_id: u32` and `Vec<DvcMessage>`) inside a block scope that ends before calling `encode_dvc_messages` — see the exact code shape in Q1 above (mirrors `encode_resize`'s own internal structure).
**Warning signs:** Compile error "cannot borrow `active_stage` as mutable more than once at a time."

### Pitfall 4: Forgetting `impl_as_any!`/`AsAny` on the new processor

**What goes wrong:** `DvcProcessor: AsAny + Send` — `RdpilotSensorProcessor` must implement `AsAny` (via `ironrdp_core::impl_as_any!(RdpilotSensorProcessor);`) or `channel_processor_downcast_ref::<RdpilotSensorProcessor>()` / `get_dvc::<RdpilotSensorProcessor>()` (which does a `TypeId`-keyed lookup) will not compile or will silently fail to find the channel.
**Why it happens:** `DrdynvcClient`'s internal `DynamicChannelSet` type-erases the processor into `Box<dyn DvcProcessor>` and relies on `AsAny::as_any()` + `TypeId::of::<T>()` for the typed lookups (`get_dvc_by_type_id`, `channel_processor_downcast_ref`).
**How to avoid:** Add `ironrdp_core::impl_as_any!(RdpilotSensorProcessor);` right after the struct definition (mirrors `DrdynvcClient`'s own `impl_as_any!(DrdynvcClient);` at `ironrdp-dvc-0.6.0/src/client.rs:177`).
**Warning signs:** Compile error (trait bound not satisfied) if omitted entirely; if using a different registration path that bypasses the type check, `get_dvc::<RdpilotSensorProcessor>()` silently returns `None`.

## Code Examples

### The `DvcEncode`-wrapping JSON message

```rust
// Source: pattern derived from ironrdp_core::Encode trait (ironrdp-core-0.2.0/src/encode.rs:150-159)
// and ironrdp_dvc::DvcEncode (ironrdp-dvc-0.6.0/src/lib.rs:36)
use ironrdp_core::{ensure_size, Encode, EncodeResult, WriteCursor};
use ironrdp_dvc::DvcEncode;

struct JsonDvcMessage(Vec<u8>);

impl JsonDvcMessage {
    fn new<T: serde::Serialize>(v: &T) -> Result<Self, serde_json::Error> {
        Ok(Self(serde_json::to_vec(v)?))
    }
}

impl Encode for JsonDvcMessage {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ensure_size!(in: dst, size: self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }
    fn name(&self) -> &'static str { "rdpilot-sensor-json" }
    fn size(&self) -> usize { self.0.len() }
}

impl DvcEncode for JsonDvcMessage {}
```

### `RdpilotSensorProcessor::start()` — the version handshake, sent unconditionally

```rust
// Source: pattern verified against ironrdp-dvc-0.6.0/src/lib.rs's DvcProcessor::start() contract
fn start(&mut self, _channel_id: u32) -> ironrdp_pdu::PduResult<Vec<ironrdp_dvc::DvcMessage>> {
    let envelope = Envelope { version: PROTOCOL_VERSION, req_id: 0, msg_type: MsgType::Version, payload: None };
    let msg = JsonDvcMessage::new(&envelope)
        .map_err(|e| ironrdp_core::other_err!("rdpilot-sensor", "envelope encode failed: {e}"))?;
    Ok(vec![Box::new(msg)])
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| STACK.md's original recommendation of a compiled Rust/C# remote sensor for even the v1 validation transport | This phase's D-4.1 throwaway PowerShell responder | Decided in CONTEXT.md (D-4.1/D-4.2, 2026-07-08) | Defers the compiled-sensor build entirely to Phase 5, letting Phase 4 prove the DVC transport in isolation with zero cross-language build tooling |

**Deprecated/outdated:** ARCHITECTURE.md's suggestion of C# NativeAOT as "the pragmatic choice" for the sensor (§Component 6) still stands for the **real** Phase 5 sensor, but does not apply to this phase's disposable responder (explicitly not the same artifact, per D-4.2).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | On a version mismatch, the RDP session stays alive and only future `Session::ping()` calls fail with `Error::Dvc`, rather than the SDK actively tearing down the whole connection | Q2 / Resolved Open Questions | If the planner/verifier expects a harder failure (e.g., `Session::connect()` itself erroring, or the whole session terminating), the live test's assertion shape would need to change; low blast radius since it's an internal design choice, not a wire-format fact |
| A2 | `serde`/`serde_json` version `"1"` (unpinned minor) is still current and compatible; not re-verified via `cargo search`/`npm view`-equivalent this session (only confirmed present in `Cargo.lock` from a prior phase) | Standard Stack | Extremely low risk — these are among the most stable, widely-used crates in the Rust ecosystem; a version bump would not be a breaking surprise |
| A3 | No live-testable version-mismatch scenario exists within this phase (both sides of the handshake are written/versioned by the same author in the same commit), so the mismatch path is unit-tested only, not live-verified | Q2 / Validation Architecture | If a future real-world version-skew bug exists in the mismatch-detection logic itself, it would not be caught until Phase 5+ when the sensor and SDK versions can actually diverge |

## Open Questions

1. **Should the mismatch path actively send a DYNVC_CLOSE PDU (the "stretch" variant in Q2), or is the simpler poisoned-state-only design (A1) sufficient for this phase's success criteria?**
   - What we know: both designs satisfy "closes the channel with a clear error, not silent corruption" from the caller's perspective; the active-close variant is more literally "closing the channel" but requires additional session-loop plumbing (`get_svc_processor_mut::<DrdynvcClient>()`) not otherwise needed this phase.
   - What's unclear: whether the phase's success-criteria wording implies an actual wire-level channel closure is required, or whether "the SDK refuses to use it further" satisfies the intent.
   - Recommendation: implement the simpler poisoned-state design (A1) as the plan's primary task; leave the active-close variant as an optional stretch task if time permits, since it is additive and doesn't block success criteria 1-2.

2. **Exact `.ps1` deployment path via WinRM (D-4.5) — `Copy-Item -ToSession` vs. inline base64 write?**
   - What we know: both mechanisms were already proven in Phase 1's WinRM plumbing; D-4.5 names both as acceptable options without picking one.
   - What's unclear: which one the existing Phase 1 helper/fixtures already have wired up (this research did not re-audit Phase 1's exact WinRM helper code, only confirmed it exists per CONTEXT.md).
   - Recommendation: the planner should grep the Phase 1 test fixtures/common module (`tests/common.rs` or similar) for existing WinRM copy helpers before writing a new one; reuse whichever primitive already exists there.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Azure VM (Phase 1 target) | Live gated test (`sensor_ping_pong_under_500ms`) | Assumed ✓ (per CONTEXT.md — reused from Phase 1) | — | None — this phase's success criteria 2/3 require the real live target per D-4.1 (local loopback proxy explicitly rejected) |
| `pwsh`/WinRM on the target | Delivery + execution of the throwaway responder | Assumed ✓ (per CONTEXT.md: "already reachable from Phase 1") | — | None needed — already the locked mechanism |
| `wtsapi32.dll` | Server-side DVC opening | ✓ (present on every Windows target since Vista/Server 2008) | OS built-in | None needed |

**Missing dependencies with no fallback:** none identified — this phase reuses infrastructure already proven in Phase 1/3.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | `cargo test` (built-in), gated live suite in `crates/rdpilot/tests/live_session.rs` |
| Config file | none — plain `#[test]`/`#[ignore]` gating via `RDPILOT_LIVE` env var (existing pattern) |
| Quick run command | `cargo test -p rdpilot` (offline unit tests only — new envelope/handshake-state logic must be unit-testable without a VM) |
| Full suite command | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SENSOR-03 (SC#1) | `DvcProcessor` registered before `connect_begin` | unit | `cargo test -p rdpilot dvc_seam_name_is_reserved` | ✅ (existing, `connect.rs:362`) — extend with a new offline test asserting `RdpilotSensorProcessor::channel_name() == "RDPILOT_SENSOR"` |
| SENSOR-03 (SC#2) | Ping over `RDPILOT_SENSOR` returns pong within 500ms | live (gated) | `RDPILOT_LIVE=1 cargo test -p rdpilot sensor_ping_pong_under_500ms -- --ignored` | ❌ Wave 0 — new test, mirrors `live_session.rs`'s existing `require_target!`/`block_on` pattern |
| SENSOR-03 (SC#3, positive path) | Version handshake is first message; matching versions succeed | live (gated) | same live test as above (handshake is a precondition for the ping to succeed at all) | ❌ Wave 0 — covered implicitly by the ping test succeeding |
| SENSOR-03 (SC#3, negative path) | Version mismatch produces a clear error, not silent corruption | unit | `cargo test -p rdpilot version_mismatch_marks_handshake_state` | ❌ Wave 0 — offline test: construct `RdpilotSensorProcessor`, call `process()` with a hand-built mismatched-version JSON payload, assert `HandshakeState::Mismatched`, then assert a subsequent simulated `ping()`-style check returns `Error::Dvc` |

### Sampling Rate

- **Per task commit:** `cargo test -p rdpilot` (offline; the envelope/handshake-state logic must not require a live VM to unit-test)
- **Per wave merge:** `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1`
- **Phase gate:** Full suite green before `/gsd-verify-work`, per the existing Phase 2/3 convention.

### Wave 0 Gaps

- [ ] `crates/rdpilot/src/sensor.rs` — new module: `SensorShared`, `HandshakeState`, `Envelope`/`MsgType`, `RdpilotSensorProcessor`, `JsonDvcMessage` — covers SENSOR-03 SC#1/#3
- [ ] Offline unit tests for `Envelope` JSON round-trip and `HandshakeState` transitions (no VM needed) — covers SC#3 negative path
- [ ] `crates/rdpilot/tests/live_session.rs::sensor_ping_pong_under_500ms` — covers SC#2/#3 positive path
- [ ] `crates/rdpilot/tests/fixtures/sensor-responder.ps1` (or equivalent path used by the live-test harness) — the throwaway responder itself, deployed via existing WinRM plumbing (D-4.5)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | Out of scope — DVC inherits RDP's own CredSSP/TLS authentication; no new auth surface this phase |
| V3 Session Management | No | No new session concept introduced |
| V4 Access Control | No | N/A — single-tenant lab VM, no multi-user access-control surface added |
| V5 Input Validation | Yes | `serde_json` deserialization of the inbound envelope IS the input-validation boundary — malformed/oversized JSON from the (untrusted-by-design, per Pitfall m3) DVC channel must be rejected via `Result`/`Error::Dvc`, never `unwrap`/`panic` (existing API-01 convention already enforces this) |
| V6 Cryptography | No | DVC traffic rides the existing RDP TLS tunnel (already covered by Phase 2's TLS work); no new crypto primitive introduced this phase |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Malformed/oversized JSON payload on the DVC channel causing a panic or resource exhaustion | Denial of Service | `serde_json::from_slice` returns `Result`; map any decode error to `Error::Dvc(...)` and drop the message rather than `unwrap`/`expect` (per existing crate-wide API-01 convention already enforced in `error.rs`) |
| Any process in the same RDP session opening `RDPILOT_SENSOR` and impersonating either endpoint (Pitfall m3) | Spoofing / Tampering | Explicitly out of scope this phase (Deferred Ideas); documented as a known, accepted limitation for the single-tenant lab target — do not silently claim it's mitigated |
| `req_id` reuse/prediction leading to a stale `oneshot` being fulfilled with the wrong reply (cross-talk between concurrent future requests) | Tampering | Not exploitable this phase (only one `Session::ping()` call is in flight at a time in the test harness), but the `pending` map's `remove()`-then-`send()` pattern already ensures each `req_id` is consumed exactly once — safe to extend to real concurrency in Phases 6-7 without redesign |

## Sources

### Primary (HIGH confidence)

- `ironrdp-dvc-0.6.0` crate source, read directly from `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ironrdp-dvc-0.6.0/` (`src/lib.rs`, `src/client.rs`) — `DvcProcessor` trait, `encode_dvc_messages`, `DrdynvcClient`, `MAX_DATA_SIZE = 1590`
- `ironrdp-session-0.9.0` crate source, read directly from `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ironrdp-session-0.9.0/src/active_stage.rs` — `ActiveStage::{get_dvc, encode_dvc_messages, encode_resize}`
- `ironrdp-core-0.2.0` crate source, read directly from `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ironrdp-core-0.2.0/` (`src/encode.rs`, `src/macros.rs`, `src/as_any.rs`) — `Encode` trait, `ensure_size!`, `impl_as_any!`
- [WTSVirtualChannelOpenEx (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelopenex) — signature, flags, session/permission semantics
- [WTSVirtualChannelRead (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelread) / [WTSVirtualChannelWrite](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsvirtualchannelwrite)
- [windows-sys WTS_CHANNEL_OPTION_DYNAMIC constant = 1](https://docs.rs/windows-sys/latest/windows_sys/Win32/System/RemoteDesktop/constant.WTS_CHANNEL_OPTION_DYNAMIC.html) — authoritative Windows-metadata-generated Rust binding
- This repo's `crates/rdpilot/src/{connect.rs,error.rs,session.rs,session_loop.rs}` and `crates/rdpilot/tests/live_session.rs`, `crates/rdpilot/Cargo.toml` — the existing seams and conventions this phase extends
- `.planning/phases/04-dvc-transport-channel/04-CONTEXT.md`, `.planning/research/{ARCHITECTURE,PITFALLS,STACK}.md` — canonical phase-scope and prior-research inputs

### Secondary (MEDIUM confidence)

- [Microsoft Q&A: WTSVirtualChannelOpenEx sometimes failed with err 0x31](https://learn.microsoft.com/en-us/answers/questions/163134/wtsvirtualchannelopenex-sometimes-failed-with-err) — community-reported but Microsoft-hosted; corroborates the timing-race pitfall
- [microsoft/rdp-dvc-plugin-samples](https://github.com/microsoft/rdp-dvc-plugin-samples) — confirms the WTS server-side pattern generally (not PowerShell-specific, but same underlying API)

### Tertiary (LOW confidence)

- None used for load-bearing claims in this document.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new crates; existing pinned versions read from `Cargo.lock`
- Architecture (Q1/Q2 proactive-send + handshake design): HIGH — grounded in actual crate source, not training-data recall; the `encode_resize` precedent is a directly-analogous, already-shipped IronRDP pattern
- Architecture (Q3 WTS/PowerShell): HIGH — official MS Learn docs + authoritative Rust binding for the exact flag constant
- Pitfalls: MEDIUM-HIGH — the timing-race pitfall (Pitfall 1) is corroborated by a real (if informal) MS Q&A report rather than a formal spec statement
- Assumption A1 (mismatch-handling severity): MEDIUM — a reasonable but not spec-mandated interpretation; flagged for planner/discuss-phase confirmation if desired

**Research date:** 2026-07-09
**Valid until:** ~30 days (IronRDP releases weekly per prior research, but the specific `0.6.0`/`0.9.0`/`0.2.0` versions are already pinned in this repo's `Cargo.lock` and will not drift underneath this phase; the WTS API surface is stable/unchanged since Windows Vista)
