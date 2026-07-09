# rdpilot — Requirements

Milestone: v1 — Windows-only RDP computer-use SDK, optimized for read/inspect, proven by a scripted harness (no live LLM required for "done"). Personal tooling first.

Stack (from research): IronRDP 0.14 (Rust) client core + primary language; C# .NET 8 NativeAOT sensor helper; DVC transport (WinRM as bootstrap fallback). See `.planning/research/SUMMARY.md`.

20 total v1 requirements.

## v1 Requirements

### Environment

- [ ] **ENV-01**: A Bicep template provisions an Azure Windows VM configured for RDP automation — RDP/NLA reachable, WinRM enabled, `RemoteDesktop_SuppressWhenMinimized=2` set, display forced to 96 DPI (100%) for the automation user, and a sample remote-only Windows program installed to test against
- [ ] **ENV-02**: A PowerShell (`.ps1`) script brings the test environment up and tears it down on demand
- [ ] **ENV-03**: A scheduled auto-destroy safeguard automatically tears down the VM / resource group to prevent runaway cost if teardown is forgotten

### Session

- [x] **SESS-01**: User can connect to and authenticate (NLA / credentials) an RDP session to a Windows target
- [x] **SESS-02**: User can manage the session lifecycle (open, keepalive, teardown) and keep the session rendered so perception stays live (apply `RemoteDesktop_SuppressWhenMinimized=2`, prevent lock/minimize stalls)

### Capture

- [x] **CAP-01**: User can capture a full-desktop screenshot from the RDP framebuffer
- [x] **CAP-02**: User can capture a per-window cropped screenshot

### Input

- [x] **INPUT-01**: User can inject mouse actions (move, click variants, scroll, drag) at remote coordinates, mapped to the Anthropic/OpenAI computer-use action vocabulary
- [x] **INPUT-02**: User can inject keyboard input (type text and key combinations/modifiers)

### Perception

- [x] **PERC-01**: User can enumerate the remote process tree
- [x] **PERC-02**: User can enumerate remote windows (titles, geometry, foreground/z-order)
- [x] **PERC-03**: User can retrieve the UI Automation tree as a flat `UiaElement[]` (id, role, name, bbox, enabled, visible, focusable, focused, value?, depth, parentId)
- [x] **PERC-04**: User can query and set the foreground window (focus)

### Remote Sensor

- [x] **SENSOR-01**: A thin C# .NET 8 NativeAOT sensor helper exposes the structured-perception queries on the target
- [x] **SENSOR-02**: The SDK can bootstrap/deploy and launch the sensor on the target (drive-redirection copy primary, WinRM fallback)
- [x] **SENSOR-03**: A DVC request/response transport channel carries structured-perception data between the SDK and the sensor

### Process

- [x] **PROC-01**: User can launch and observe a remote process (e.g. `pwsh.exe`)

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

| Requirement | Phase | Status |
|-------------|-------|--------|
| ENV-01 | Phase 1: Test Environment | Pending |
| ENV-02 | Phase 1: Test Environment | Pending |
| ENV-03 | Phase 1: Test Environment | Pending |
| SESS-01 | Phase 2: RDP Session + Framebuffer Core | Complete |
| SESS-02 | Phase 2: RDP Session + Framebuffer Core | Complete |
| CAP-01 | Phase 2: RDP Session + Framebuffer Core | Complete |
| CAP-02 | Phase 6: Window + Process Perception | Complete |
| INPUT-01 | Phase 3: Input Injection | Complete — proven live in Phase 3 Plan 04, 10/10 tests pass 2026-07-08 |
| INPUT-02 | Phase 3: Input Injection | Complete — proven live in Phase 3 Plan 04, 10/10 tests pass 2026-07-08 |
| SENSOR-03 | Phase 4: DVC Transport Channel | Complete — Plans 01-03 code-complete (envelope, RdpilotSensorProcessor, Session::ping(), throwaway WTS responder, WinRM deploy helper, gated live test) and LIVE-VERIFIED against a real disposable Azure Windows VM: `sensor_ping_pong_under_500ms` PASSED, measured round trip 165ms (SC#2), Version handshake proven first (SC#3 positive). Two bugs found and fixed during the live run: a session_loop.rs fix (transient DVC-not-ready Ping no longer kills the whole session) and a sensor-responder.ps1 fixture fix (JSON-start scan past a DVC framing prefix); test's outer setup-retry budget widened 15s→60s (empirically-measured AtLogOn scheduled-task latency). VM torn down after the run (`rdpilot-test` RG deleted). |
| SENSOR-01 | Phase 5: Sensor Bootstrap + Deployment | Complete — `rdpilot-sensor.exe` built and LIVE-VERIFIED: NativeAOT win-x64 self-contained publish (2,699,264 bytes / ~2.57 MiB), no external .NET runtime dependency, runs with none installed; SHA256 identical VM-built vs locally-retrieved. See `05-01-SUMMARY.md`. |
| SENSOR-02 | Phase 5: Sensor Bootstrap + Deployment | Complete — both deployment paths LIVE-VERIFIED against a real disposable Azure Windows VM (2026-07-09): RDPDR primary (mandatory, D-5.6) measured 22.612044ms; WinRM fallback measured 21.62078ms; both well under the 1s SC4 bound. Required an `rdpsnd` stub static channel (MS-RDPEFS Appendix A footnote <1>) for Windows to initiate the RDPDR handshake at all, plus QueryInformation/QueryVolumeInformation IRP support and launch-timing fixes (session settle + chunked typing). See `05-04-SUMMARY.md`. |
| PERC-01 | Phase 6: Window + Process Perception | Complete |
| PERC-02 | Phase 6: Window + Process Perception | Complete |
| PERC-04 | Phase 6: Window + Process Perception | Complete |
| PROC-01 | Phase 6: Window + Process Perception | Complete |
| PERC-03 | Phase 7: UIA Tree Module | Complete |
| API-01 | Phase 8: Public SDK API + WorldState | Pending |
| API-02 | Phase 8: Public SDK API + WorldState | Pending |
| PROOF-01 | Phase 9: Scripted Proof Harness | Pending |
