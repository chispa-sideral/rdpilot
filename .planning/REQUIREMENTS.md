# rdpilot — Requirements

Milestone: v1 — Windows-only RDP computer-use SDK, optimized for read/inspect, proven by a scripted harness (no live LLM required for "done"). Personal tooling first.

Stack (from research): IronRDP 0.14 (Rust) client core + primary language; C# .NET 8 NativeAOT sensor helper; DVC transport (WinRM as bootstrap fallback). See `.planning/research/SUMMARY.md`.

## v1 Requirements

### Session

- [ ] **SESS-01**: User can connect to and authenticate (NLA / credentials) an RDP session to a Windows target
- [ ] **SESS-02**: User can manage the session lifecycle (open, keepalive, teardown) and keep the session rendered so perception stays live (apply `RemoteDesktop_SuppressWhenMinimized=2`, prevent lock/minimize stalls)

### Capture

- [ ] **CAP-01**: User can capture a full-desktop screenshot from the RDP framebuffer
- [ ] **CAP-02**: User can capture a per-window cropped screenshot

### Input

- [ ] **INPUT-01**: User can inject mouse actions (move, click variants, scroll, drag) at remote coordinates, mapped to the Anthropic/OpenAI computer-use action vocabulary
- [ ] **INPUT-02**: User can inject keyboard input (type text and key combinations/modifiers)

### Perception

- [ ] **PERC-01**: User can enumerate the remote process tree
- [ ] **PERC-02**: User can enumerate remote windows (titles, geometry, foreground/z-order)
- [ ] **PERC-03**: User can retrieve the UI Automation tree as a flat `UiaElement[]` (id, role, name, bbox, enabled, visible, focusable, focused, value?, depth, parentId)
- [ ] **PERC-04**: User can query and set the foreground window (focus)

### Remote Sensor

- [ ] **SENSOR-01**: A thin C# .NET 8 NativeAOT sensor helper exposes the structured-perception queries on the target
- [ ] **SENSOR-02**: The SDK can bootstrap/deploy and launch the sensor on the target (drive-redirection copy primary, WinRM fallback)
- [ ] **SENSOR-03**: A DVC request/response transport channel carries structured-perception data between the SDK and the sensor

### Process

- [ ] **PROC-01**: User can launch and observe a remote process (e.g. `pwsh.exe`)

### SDK / WorldState

- [ ] **API-01**: A clean, typed SDK API surface exposes control + perception so a consumer can drive a session programmatically
- [ ] **API-02**: A coherent `WorldState` correlates screenshot + window list + UIA snapshot in one coordinate space, emitting both pixel and logical/DPI-scaled coordinates

### Proof

- [ ] **PROOF-01**: A scripted harness proves the full read/inspect loop end-to-end on a real remote-only Windows program (connect → screenshot → read UIA tree → navigate → report findings)

## v2 Requirements (Deferred)

- [ ] Clipboard read/write over CLIPRDR (text extraction from remote apps)
- [ ] File transfer to/from the remote target
- [ ] MCP server / Anthropic computer-use shim / CLI packaging (first non-harness consumer)
- [ ] PyO3 / NAPI-RS bindings exposing the SDK to TypeScript and Python consumers

## Out of Scope (v1)

- Running the AI agent on the remote session — explicitly rejected; intelligence stays local
- Non-Windows RDP targets (Linux/xrdp, macOS) — Windows-only v1 to lean on UIA/WinRM/WMI/RAIL
- Heavy write/destructive automation and its guardrails — beyond what read/inspect needs
- Published-package polish — public API stability guarantees, comprehensive docs, multi-registry distribution
- Multi-session orchestration / concurrency at scale
- Remote-assist co-driving UX

## Traceability

(Filled by roadmap — maps each requirement to its phase.)
