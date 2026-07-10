# Phase 7: UIA Tree Module - Context

**Gathered:** 2026-07-09
**Status:** Ready for planning

<domain>
## Phase Boundary

The sensor's UIA module returns a flat `UiaElement[]` for a specified window
handle with correct bounding boxes, scoped by default to direct children to
stay within latency bounds.

**Requirement:** PERC-03 ("User can retrieve the UI Automation tree as a flat
`UiaElement[]`").

**Success Criteria** (what must be TRUE):
1. `get_uia_tree(hwnd)` for a Notepad window returns a flat `UiaElement[]`
   with id, role, name, bounding rect, enabled, visible, focusable, and depth
   populated.
2. All bounding box coordinates in the UIA response are in the same physical
   virtual-desktop pixel space as the framebuffer and window list (coordinate
   alignment verified).
3. A tree walk scoped to `TreeScope_Children` completes within 500 ms for a
   standard Win32 application.
4. The response is valid JSON-serializable `UiaElement[]` (round-trips
   through `serde_json` without loss).

**Explicitly NOT in this phase** (belongs elsewhere — redirect scope creep):
- `WorldState` correlation of screenshot + window list + UIA snapshot — Phase 8
  (API-02).
- The public typed `Session` API surface hiding all sensor-protocol details —
  Phase 8 (API-01).
- The scripted end-to-end proof harness — Phase 9 (PROOF-01).
- `ValuePattern`/`value` field retrieval — deferred to backlog (D-7.1).
- A caller-configurable `max_depth` / deeper-than-Children tree walk —
  deferred to backlog (D-7.4).

</domain>

<decisions>
## Implementation Decisions

### UiaElement field set
- **D-7.1:** `UiaElement` fields: `id`, `role`, `name`, `bbox` (physical
  virtual-desktop pixels, `u32 x/y/w/h` matching `crate::Rect`), `enabled`,
  `visible`, `focusable`, `focused`, `depth`, `parentId`. `value`
  (`ValuePattern`) is DEFERRED to backlog. Rationale: mirrors the D-6.3
  "floor + high-value extras, exclude the hard one" pattern already used for
  window/process records; resolves the REQUIREMENTS(PERC-03)-vs-ROADMAP-SC#1
  field discrepancy toward the fuller REQUIREMENTS list, minus the
  pattern-dependent `value`.

### `id` mapping
- **D-7.2:** `id` = stringified UIA `RuntimeId` (join the int array to a
  stable string). Rationale: `RuntimeId` is UIA's native stable-ish identity
  mechanism; `AutomationId` is empty on non-instrumented Win32 apps like
  Notepad — the literal SC#1 test target.

### `role` mapping
- **D-7.3:** `role` = UIA `ControlType` id mapped to a stable English
  friendly string (e.g. `UIA_ButtonControlTypeId` → `"Button"`). NOT
  `LocalizedControlType`. Rationale: deterministic, locale-independent,
  serde-stable, matches the framebuffer/window-list style of plain,
  predictable string fields.

### Tree scope and depth representation
- **D-7.4:** Tree scope = `TreeScope_Children` ONLY, fixed (no `max_depth`
  parameter in v1). A single scoped COM call, not `TreeWalker` recursion.
  `depth`/`parentId` are flat-array bookkeeping fields (depth ~0/1 for a
  children-only walk) that future-proof the wire shape for a later phase. A
  caller-configurable `max_depth` is an explicit BACKLOG item if Phase 9's
  harness needs deeper trees. Rationale: ROADMAP's "scoped by default to
  direct children" reads as a v1 literal constraint, not a default-with-
  override; matches the project's consistent v1-minimalism (D-6.1/D-6.2).

### COM interop approach (highest-risk entry condition)
- **D-7.5:** SPIKE-FIRST with C# .NET 8 `[GeneratedComInterface]` (the
  AOT-safe COM source generator), hand-authoring the `IUIAutomation` /
  `IUIAutomationElement` / `IUIAutomationTreeWalker` interface declarations —
  no official Microsoft-shipped `[GeneratedComInterface]` UIA bindings exist
  (RESEARCH PHASE MUST VERIFY this via Context7/WebSearch), so the vtable
  shape must be hand-ported from `UIAutomationClient.idl`. Gate on a
  throwaway `dotnet publish -p:PublishAot=true` smoke test BEFORE any real
  UIA handler code is written — mirroring the Phase 6 AOT-smoke-test-before-
  real-code risk gate (`sensor/Program.cs`'s `RunAotSmokeTest`, ~lines
  69-159). Do NOT use classic managed `System.Windows.Automation`
  (reflection-heavy, expected AOT-incompatible) and do NOT use `ComImport`
  (documented not-AOT/trim-safe). CsWin32 was considered and NOT chosen
  (avoids adding a new build-time NuGet dependency). This resolves the D-5.4
  COM-under-NativeAOT deferral and is **this phase's highest-risk entry
  condition** — RESEARCH must treat it as such.
  - Rejected: `System.Windows.Automation` — reflection-heavy managed
    assembly, not designed for trimming/AOT.
  - Rejected: `ComImport`-based interop — documented NOT AOT/trim-safe.
  - Rejected (for now): CsWin32 source generator — avoids a new build-time
    NuGet dependency; `[GeneratedComInterface]` hand-authored is preferred.

### Per-element error/degrade behavior
- **D-7.6:** Per-element failure = skip-the-failing-element, don't sink the
  response. Mirrors the Phase 6 `WindowEnumeration` per-window defensive skip
  pattern for elements destroyed mid-walk / `COMException`
  `UIA_E_ELEMENTNOTAVAILABLE`. The top-level try/catch still degrades the
  WHOLE response to `{success:false, error}` only on total handler failure
  (existing D-6.4 contract, unchanged).

### Latency approach
- **D-7.7:** Naive uncached per-property COM reads first; verify against the
  SC#3 500 ms budget empirically at the live gate. Reach for
  `IUIAutomation::CreateCacheRequest` bulk-cached retrieval ONLY if the live
  gate shows the 500 ms bound is actually at risk for Notepad. Rationale:
  matches the project's consistent "reason offline, live-tune empirically"
  methodology (every timing constant in `session.rs` — `RUN_DIALOG_SETTLE`,
  `SESSION_SETTLE`, `ENUMERATION_TIMEOUT_MS` — was reasoned offline then
  live-tuned against the real VM, never pre-optimized).

### Claude's Discretion
- Exact `IUIAutomation`/`IUIAutomationElement` interface member subset
  hand-authored in the `[GeneratedComInterface]` declarations (only what
  D-7.1's field set requires — no need to bind the entire UIA COM surface).
- Exact wire-contract JSON key names for the new `Uia`/`UiaElement` payload
  (follow the established snake_case convention; planner's call on exact
  naming as long as it matches D-7.1's field list).
- Whether `RuntimeId`-to-string join uses a delimiter like `.` or `-` (D-7.2)
  — any stable, deterministic format is acceptable.

</decisions>

<specifics>
## Specific Ideas

No specific UX/behavioral references beyond the locked decisions above —
this is a sensor/protocol phase with no user-facing surface. The Notepad
target app (already used as the SC#1 test target in ROADMAP) is the concrete
validation case referenced throughout the decisions above (D-7.2's
`AutomationId`-is-empty rationale, D-7.6's per-element skip rationale).

</specifics>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope & requirements
- `.planning/ROADMAP.md` §"Phase 7: UIA Tree Module" — goal, 4 success
  criteria, requirement mapping (PERC-03).
- `.planning/REQUIREMENTS.md` — full text of PERC-03 (note: lists `focused`,
  `value?`, `parentId` — RESOLVED toward D-7.1's fuller field list, see
  "REQUIREMENTS vs ROADMAP field-list discrepancy" below).
- `.planning/PROJECT.md` — locked stack decisions.
- `.planning/STATE.md` — accumulated decisions and current position.

### Prior phase context (inherited constraints — do not re-litigate)
- `.planning/phases/04-dvc-transport-channel/04-CONTEXT.md` — wire envelope
  is FIXED: `{version, req_id, type, payload}` JSON, bare externally-tagged
  `MsgType` unit-string form, `payload: Option<serde_json::Value>` /
  `JsonElement?`. Adding `MsgType::Uia` follows this exact shape with zero
  framing rework.
- `.planning/phases/05-sensor-bootstrap-deployment/05-CONTEXT.md` — D-5.4:
  "the COM-under-NativeAOT compatibility question (needed for Phase 7
  UIA/IUIAutomation) is recorded as an explicit Phase-7
  entry-condition/risk" — this phase (via D-7.5) is where that deferral is
  finally resolved.
- `.planning/phases/06-window-process-perception/06-CONTEXT.md` — D-6.3
  "floor + high-value extras" field-richness pattern (mirrored by D-7.1);
  D-6.4 success/degrade contract (`{success:true,data}` /
  `{success:false,error}`, semantic-vs-transport error split) — unchanged,
  reused verbatim by `get_uia_tree`.

</canonical_refs>

<code_context>
## Existing Code Insights

### Already Decided / Inherited Constraints (do not re-derive — act on these)
- **Wire envelope + `MsgType::Uia` extension:** the envelope shape
  (`{version, req_id, type, payload}`) is fixed and already extensible;
  adding `MsgType::Uia` to both `crates/rdpilot/src/sensor.rs`'s `MsgType`
  enum and `sensor/Envelope.cs`'s mirror enum is the only framing change
  needed — no rework of `Envelope`, `JsonDvcMessage`, or the DVC
  chunking/reassembly path.
- **Generic req_id-keyed fulfilment:** `RdpilotSensorProcessor::process()`'s
  non-`Version` match arm (`crates/rdpilot/src/sensor.rs:291-312`) already
  handles ANY future `MsgType` including `Uia` with zero changes to the
  dispatch/correlation machinery (RESEARCH Pattern 1, established at Phase
  6).
- **`Session::sensor_request()` round-trip helper is MANDATORY** for
  `get_uia_tree(hwnd)`: handshake fast-fail → alloc `req_id` → register
  `oneshot` in `SensorShared::pending` → send
  `RdpInputEvent::Request(msg_type, req_id, payload)` → `tokio::time::timeout`
  → `success`/`data` vs `error` branch (`crates/rdpilot/src/session.rs:487-566`).
  `get_uia_tree` follows this exact shape, same as `get_window_list`/
  `get_process_tree` (`session.rs:583-618`).
- **Owned `UiaElement` + `UiaElementWire` deserialize-only pair** is
  mandatory (D-09, owned-SDK-types-only public API): mirror the
  `WindowInfo`/`WindowInfoWire` pattern in
  `crates/rdpilot/src/perception.rs` exactly. The wire struct never leaves
  the crate; only the owned `UiaElement` is public.
- **`[LibraryImport]`-only + source-gen-JSON + `[JsonSerializable]`
  registration discipline is FIXED:** never attribute-based `DllImport`,
  never reflection-based `JsonSerializer.Serialize<T>`. Every new DTO
  (`UiaElement`, `UiaTreeResponse`, any request DTO) MUST get a
  `[JsonSerializable(typeof(...))]` entry in
  `sensor/EnvelopeJsonContext.cs`, mirroring `WindowListResponse`'s
  registration.
- **No WMI / `System.Management`** anywhere in the sensor — this precedent
  generalizes to avoiding managed reflection-heavy APIs categorically under
  NativeAOT, directly informing D-7.5's rejection of
  `System.Windows.Automation`.
- **`RdpilotSensor.csproj` currently has ZERO `<PackageReference>` entries**
  — confirmed via codebase inspection. D-7.5's `[GeneratedComInterface]`
  approach is a source-generator built into the .NET 8 SDK itself (no new
  NuGet package needed), consistent with keeping the csproj
  dependency-free; if research surfaces a need for a package after all, that
  is a deviation to flag, not assume.

### Reusable Assets
- `crates/rdpilot/src/sensor.rs` — `Envelope`/`MsgType` + generic fulfilment
  path (insertion point: add `Uia` to `MsgType`).
- `crates/rdpilot/src/perception.rs` — `WindowInfo`/`WindowInfoWire` +
  `RectWire`/`WindowStateWire` conversion pattern — the direct template for
  `UiaElement`/`UiaElementWire`.
- `crates/rdpilot/src/session.rs:487-618` — `sensor_request()` helper +
  `get_window_list`/`get_process_tree` — the direct template for
  `get_uia_tree(hwnd)`.
- `sensor/Program.cs` — dispatch loop (`RunHandshakeAndPingPongLoop`,
  insertion point for a new `MsgType.Uia` arm) and the AOT smoke-test
  risk-gate pattern (`RunAotSmokeTest`, lines 69-159) that D-7.5 says to
  mirror before writing real UIA handler code.
- `sensor/WindowEnumeration.cs` — `[LibraryImport]`/`[UnmanagedCallersOnly]`
  P/Invoke discipline, bounded-buffer string reads, and the per-element
  defensive skip pattern (`BuildWindowListResponse`, "destroyed between
  EnumWindows and here — skip, don't fail") that D-7.6 mirrors.
- `sensor/RdpilotSensor.csproj` — the project file any UIA
  dependency/generator choice touches; currently zero `<PackageReference>`.

### Established Patterns
- D-6.4 success/degrade contract is unchanged and reused verbatim:
  `{success:true, data:[...]}` / `{success:false, error:"..."}`, with a
  top-level try/catch degrading the WHOLE response only on total handler
  failure (D-7.6 adds a per-element skip layer underneath this, it does not
  replace it).
- Coordinate space is FIXED: physical virtual-desktop pixels, `u32 x/y/w/h`,
  matching `crate::Rect` and the existing framebuffer/window-list space
  (`crates/rdpilot/src/perception.rs:17-19`) — `bbox` in `UiaElement` MUST
  follow this exactly (SC#2).
- No `unwrap`/`expect`/`panic!` in library code (API-01) — applies to any
  new Rust-side `UiaElement`/`get_uia_tree` code exactly as it did in
  Phase 6.

### Integration Points
- New C# `[GeneratedComInterface]` COM interop code is entirely new surface
  area with zero prior art in this repo — the risk gate (D-7.5) must run
  BEFORE any handler code depends on it compiling under `PublishAot=true`.
- `sensor/EnvelopeJsonContext.cs` needs new `[JsonSerializable]` entries for
  every new UIA-related DTO.

</code_context>

<deferred>
## Deferred Ideas

- **`value` / `ValuePattern` field** on `UiaElement` — deferred to backlog
  (D-7.1). Requires per-control-pattern branching logic in the handler;
  explicitly excluded from this phase's field set.
- **Caller-configurable `max_depth` parameter / deeper tree scopes than
  `TreeScope_Children`** — deferred to backlog (D-7.4). Revisit only if
  Phase 9's scripted proof harness needs a deeper tree than a single
  children-level walk provides.

</deferred>

---

## REQUIREMENTS vs ROADMAP field-list discrepancy — RESOLVED

`REQUIREMENTS.md`'s PERC-03 full text lists a fuller field set
(`id, role, name, bbox, enabled, visible, focusable, focused, value?, depth,
parentId`) than `ROADMAP.md`'s Phase 7 SC#1
(`id, role, name, bounding rect, enabled, visible, focusable, depth`) — SC#1
omits `focused`, `value`, and `parentId`. **Resolved per D-7.1:** the field
set is the fuller REQUIREMENTS list minus `value` (deferred to backlog as a
pattern-dependent, higher-effort field). `focused` and `parentId` ARE in
scope for this phase.

## Settled — do not re-litigate

- **Wire envelope shape and DVC transport mechanics** — locked in Phase 4;
  this phase only adds a new `MsgType::Uia` variant and payload shape on top
  of the existing, unchanged framing.
- **D-6.4 success/degrade contract** (`{success:true,data}` /
  `{success:false,error}`, semantic-`SensorRejected`-vs-transport-`Dvc`
  error split) — locked in Phase 6; reused verbatim, not re-decided.
- **Sensor language = C# .NET 8 NativeAOT** — locked at Phase 5;
  unaffected by this phase's COM-interop decision (D-7.5 chooses HOW to do
  COM under NativeAOT, not whether to use C#/NativeAOT at all).

---

*Phase: 07-uia-tree-module*
*Context gathered: 2026-07-09*
