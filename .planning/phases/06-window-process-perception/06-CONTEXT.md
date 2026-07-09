# Phase 6: Window + Process Perception - Context

**Gathered:** 2026-07-09
**Status:** Ready for planning

<domain>
## Phase Boundary

The sensor returns window list, process tree, per-window screenshots, focus control (`set_foreground_window`), and remote process launch over the existing `RDPILOT_SENSOR` DVC channel. Phase 6 is the FIRST phase to move real structured data through the sensor request/response protocol (prior phases only implemented the Version/Ping/Pong handshake).

**Requirements covered:** PERC-01, PERC-02, PERC-04, PROC-01, CAP-02.

</domain>

<decisions>
## Implementation Decisions

### Per-window screenshots
- **D-6.1:** Implement as CLIENT-SIDE CROP of the already-captured desktop framebuffer to the window's rect — reuse the existing Phase-2 `Screenshot::crop(Rect)` (`crates/rdpilot/src/screenshot.rs:103-130`). No sensor round-trip and no new sensor capture code this phase. KNOWN/DOCUMENTED LIMITATION: an occluded or minimized window returns a clipped/stale/blank crop because only the RDP-rendered desktop frame is available. Sensor-side capture (PrintWindow/BitBlt for occluded/minimized windows) is DEFERRED to backlog. Note: per-window screenshots are in the phase GOAL but are NOT one of the 4 hard success criteria, so this lean approach is acceptable.

### launch_process contract
- **D-6.2:** FIRE-AND-FORGET. `launch_process()` sends the launch and returns the new PID (or a sensor-reported semantic failure per D-6.4). Accepts: exe path + optional args + optional working directory. The success criterion "the new process appears in a subsequent process tree query" is satisfied by the caller doing a follow-up `get_process_tree()` — matches the criterion's own wording; no wait/poll logic on the sensor side.

### Perception data richness
- **D-6.3:** Success-criteria FLOOR + high-value extras.
  - WINDOW record: HWND, title, bounding rect (in physical virtual-desktop pixels — same space as framebuffer), z-order, window state (normal/minimized/maximized), PLUS window class name and owning PID (via `GetWindowThreadProcessId` — cheap, and links windows↔processes).
  - PROCESS record: PID, parent PID, name, path, PLUS command line and owner user.
  - EXCLUDED for v1 (deferred): elevation/integrity level, session id, window styles, is-tool-window/cloaked flags.

### Remote-action failure model
- **D-6.4:** EXPLICIT TYPED ERRORS on the owned SDK `Error` enum, distinguishing (a) sensor-reported SEMANTIC failure — a reply arrived carrying `success:false` + a reason (e.g. window closed since last list, `CreateProcess` failed, exe not found) — from (b) TRANSPORT failure — the sensor never replied (500ms/1s timeout, DVC not open). Two distinct error categories so an AI consumer can decide retry-vs-requery. Mirrors the existing `Error::category()` pattern (`crates/rdpilot/src/error.rs:76-91,170-171`) and the no-panic / drop-never-crash discipline on both sides.

### Claude's Discretion
- Generalizing `SensorShared::pending` from `Sender<()>` to carry response payloads back to the caller — mechanism left to the planner (see Implementation notes below).
- Whether to add a per-message `RdpInputEvent` variant per new request or unify into a single `Request(MsgType, u64, Option<Value>)` variant — planner's call.
- WMI (`System.Management`) vs. raw P/Invoke (Toolhelp32Snapshot) for process enumeration — subject to a NativeAOT-compatibility check during planning/implementation.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Stack / prior research
- `CLAUDE.md` — stack research: `windows` crate (`EnumWindows`, `GetWindowRect`, `GetWindowThreadProcessId`), Win32_Process via WMI (`System.Management` / `Get-CimInstance`), `CreateProcess`/`Start-Process`; `uiautomation` crate (Phase 7, not this phase).

### Protocol / prior phase context
- `.planning/phases/04-dvc-transport-channel/04-CONTEXT.md` — wire protocol is FIXED (envelope `{version,req_id,type,payload}`, single `RDPILOT_SENSOR` channel, `PROTOCOL_VERSION=1`, snake_case/bare-string serde); "Anti-Pattern 2: Per-Window DVC Channels" explicitly rejected.
- `.planning/phases/05-sensor-bootstrap-deployment/05-CONTEXT.md` — D-5.4: COM-under-NativeAOT unproven, deferred to Phase 7; NativeAOT `[LibraryImport]`-only P/Invoke discipline (no attribute-based `DllImport`), `JsonSerializerIsReflectionEnabledByDefault=false`.

### Rust SDK (client side)
- `crates/rdpilot/src/sensor.rs` — `Envelope` + `MsgType` enum + `SensorShared` pending-correlation map + `RdpilotSensorProcessor::process()` match (insertion point for new response arms).
- `crates/rdpilot/src/session.rs:389-431` — `Session::ping()` round-trip pattern (req_id alloc, pending map, `input_tx` send, 500ms timeout) that new `get_window_list`/`get_process_tree`/`launch_process`/`set_foreground_window` calls follow.
- `crates/rdpilot/src/session_loop.rs` — `RdpInputEvent` enum + `build_ping_frame` (template for new request frames; DVC-not-open is dropped, not fatal).
- `crates/rdpilot/src/screenshot.rs` — `Screenshot`/`Rect` + `crop`; coordinate space = "physical virtual-desktop pixels".

### Sensor (C# side)
- `sensor/Program.cs` — `RunHandshakeAndPingPongLoop` dispatch loop (insertion point for new request handlers), `ReadEnvelope`/`WriteEnvelope`, WTS P/Invokes.
- `sensor/EnvelopeJsonContext.cs` — every new payload DTO MUST get a `[JsonSerializable(typeof(...))]` entry or NativeAOT serialization fails (no reflection fallback).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `Envelope.payload` is already `Option<serde_json::Value>` specifically so new payload shapes slot in without reworking framing.
- `SensorShared::pending` is `HashMap<u64, oneshot::Sender<()>>` keyed by `req_id` — already structurally supports concurrent in-flight requests; only `ping()` (sequential) exercises it today.
- `JsonDvcMessage` wraps any `Serialize` value into an `ironrdp-dvc` `DvcEncode` message — reusable for any new response type.
- C# side: `Envelope.cs` mirror record with `[JsonPropertyName]` pinning wire names; source-gen JSON context (no reflection).

### Established Patterns
- Wire schema is snake_case with matching Rust struct / C# record pairs, registered in the source-gen JSON context on the C# side (no reflection fallback under NativeAOT).
- `Session::ping()` establishes the req_id-allocate → pending-map-insert → `input_tx`-send → timeout-await round-trip shape that all new sensor-backed calls should mirror.

### Integration Points
- Zero Win32 window/process/COM APIs are wired in the sensor yet (only WTSVirtualChannel P/Invokes); the sensor csproj has no `PackageReference` yet — adding `System.Management`/WMI for `Win32_Process` needs a NativeAOT-compatibility evaluation before use.

</code_context>

<specifics>
## Specific Ideas

No specific UI/UX requirements beyond the four locked decisions above. Field-level ideas (which exact struct fields to carry) are captured under D-6.3.

</specifics>

<deferred>
## Deferred Ideas

- Sensor-side per-window capture (PrintWindow/BitBlt) for occluded/minimized windows — enables full-fidelity per-window screenshots regardless of desktop occlusion.
- Richer process/window fields: elevation/integrity level, session id, window styles, is-tool-window/cloaked flags.
- Wait-and-confirm `launch_process` variant (block until process/main-window appears) if fire-and-forget proves insufficient for a consumer.

</deferred>

---

## Implementation notes for planner

These are Claude-decided plumbing considerations for the planner to weigh — NOT user decisions, and open to the planner's judgment:

- Generalize `SensorShared::pending` from `oneshot::Sender<()>` to carry the response payload back to the caller (e.g. `Sender<serde_json::Value>` or `Sender<Envelope>`) — `ping()`'s unit signal is a special case that predates this need.
- Each new outbound request needs an `RdpInputEvent` variant (like `Ping(u64)`) plus a `session_loop` `build_*_frame` arm, OR unify into a single `Request(MsgType, u64, Option<Value>)` variant to avoid enum sprawl — planner's call.
- Snake_case wire schema with matching Rust struct / C# record pairs; register every new DTO in `EnvelopeJsonContext.cs`.
- Win32 for Phase 6 (`EnumWindows`, `GetWindowRect`, `GetWindowThreadProcessId`, `CreateProcess`, `Win32_Process`) is plain P/Invoke — no COM — so the Phase-7 COM-under-NativeAOT risk does not block Phase 6; but WMI via `System.Management` must be checked for NativeAOT compatibility (P/Invoke to the raw process-enumeration APIs or `Toolhelp32Snapshot` may be the AOT-safe alternative).
- Window rects MUST already be emitted in physical virtual-desktop pixels (a Phase 7 success criterion depends on window-list rects sharing the framebuffer coordinate space — no remap later).
- New `Session` methods return owned SDK types only (D-09) — never leak `ironrdp` / `serde_json::Value`.

---

*Phase: 6-Window + Process Perception*
*Context gathered: 2026-07-09*
