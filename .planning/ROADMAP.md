# Roadmap: rdpilot

## Overview

Nine dependency-ordered phases build rdpilot from the ground up: a disposable Azure Windows test environment, an IronRDP session with live framebuffer, input injection, a DVC transport channel, sensor bootstrap and deployment, Win32 window/process perception, UIA tree retrieval, a clean public SDK API with WorldState, and finally a scripted proof harness that exercises the full read/inspect loop against a real remote-only Windows program.

## Phases

**Phase Numbering:**

- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Test Environment** - Provision a disposable Azure Windows VM pre-configured for RDP automation, with up/down script and scheduled auto-destroy — ENV-01/02/03 all verified (ENV-01/02 live gate 2026-06-04; ENV-03 auto-destroy validated 2026-06-05)
- [x] **Phase 2: RDP Session + Framebuffer Core** - Connect, authenticate, keep session rendered, and produce full-desktop screenshots — all 5 success criteria proven live (canonical run 5/5 pass on a real Azure VM with the full 10-min idle, 2026-06-05; SESS-01/SESS-02/CAP-01)
- [ ] **Phase 3: Input Injection** - Inject mouse and keyboard actions at remote coordinates with a locked DPI contract
- [ ] **Phase 4: DVC Transport Channel** - Establish and verify the RDPILOT_SENSOR dynamic virtual channel before any sensor modules exist
- [ ] **Phase 5: Sensor Bootstrap + Deployment** - Build the C# NativeAOT sensor helper and deploy it onto a real target via drive redirection or WinRM
- [ ] **Phase 6: Window + Process Perception** - Retrieve window list, process tree, per-window screenshots, focus control, and remote process launch over DVC
- [ ] **Phase 7: UIA Tree Module** - Add the UI Automation sensor module and return a flat UiaElement[] over DVC
- [ ] **Phase 8: Public SDK API + WorldState** - Expose a clean typed Session API and a coherent WorldState snapshot correlating framebuffer, windows, and UIA
- [ ] **Phase 9: Scripted Proof Harness** - Prove the full read/inspect loop end-to-end against a real remote-only Windows program

## Phase Details

### Phase 1: Test Environment

**Goal**: A disposable, reproducible Azure Windows target, provisioned via Bicep + a `.ps1` up/down script, pre-configured for RDP automation, with scheduled auto-destroy — so every later phase has a real box to test against
**Depends on**: Nothing (first phase)
**Requirements**: ENV-01, ENV-02, ENV-03
**Success Criteria** (what must be TRUE):

  1. `bicep`/`.ps1` provisions an Azure Windows VM and outputs its connection details
  2. The VM is reachable over RDP with NLA and authenticates with the provisioned credentials
  3. Automation prerequisites are verified on the VM: `RemoteDesktop_SuppressWhenMinimized=2`, 96 DPI for the automation user, WinRM enabled/reachable, sample remote-only program present
  4. The `.ps1` teardown removes all provisioned resources cleanly
  5. The scheduled auto-destroy fires and removes the resource group without manual action**Plans**: 4 plans

**Wave 1**

  - [x] 01-01-PLAN.md — Repo baseline (.gitignore) + validation scaffolding (Pester, Validate-Target skeleton)

**Wave 2** *(blocked on Wave 1 completion)*

  - [x] 01-02-PLAN.md — Core infra Bicep: network + NSG (IP-scoped 3389/5986) + WS2022 VM + CustomScriptExtension
  - [x] 01-03-PLAN.md — In-guest Configure-Target.ps1: WinRM HTTPS, default-hive DPI + SuppressWhenMinimized, SHA-verified 7-Zip

**Wave 3** *(blocked on Wave 2 completion)*

  - [x] 01-04-PLAN.md — Auto-destroy (runbook + schedule + RBAC) + manage-env.ps1 up/down + phase-gate live run *(all complete; live up→validate→down passed 2026-06-04; ENV-03 auto-destroy validated 2026-06-05 — schedule fired runbook unattended, MI reaped rdpilot-test)*

### Phase 2: RDP Session + Framebuffer Core

**Goal**: A working IronRDP session produces live screenshots of the remote desktop and stays rendered while the local window is minimized or hidden
**Depends on**: Phase 1
**Requirements**: SESS-01, SESS-02, CAP-01
**Success Criteria** (what must be TRUE):

  1. The SDK connects and authenticates (NLA/CredSSP) to a Windows target and the session reaches an active, interactive state
  2. A screenshot PNG of the full remote desktop is produced from the IronRDP DecodedImage framebuffer with correct colors (RGB, not YUV-grey)
  3. A per-window cropped screenshot is produced by cropping the framebuffer to a given bounding rect
  4. The session stays full-resolution and non-blank with no visible client window, verified behaviorally (RemoteDesktop_SuppressWhenMinimized is mstsc-only and does not apply to a headless IronRDP client, so this is satisfied by construction)
  5. Session keepalive prevents idle-timeout disconnection during a 10-minute idle period

**Plans**: 3 plans
Plans:
**Wave 1**

- [x] 02-01-PLAN.md — Toolchain + workspace scaffold + no-VM foundation types (Error, ConnectionConfig, Screenshot/crop)

**Wave 2** *(blocked on Wave 1 completion)*

- [x] 02-02-PLAN.md — Connect/auth (TLS/CredSSP, DVC seam) + SDK-owned session loop + framebuffer snapshot + keepalive + Session handle

**Wave 3** *(blocked on Wave 2 completion)*

- [x] 02-03-PLAN.md — Example binary + gated 5-criteria integration suite + canonical live validation run *(complete; canonical run on a fresh Azure VM passed 5/5 with the full 10-min idle 2026-06-05; A1 zero-delta keepalive sufficient; VM torn down)*

### Phase 3: Input Injection

**Goal**: Mouse and keyboard actions are delivered to the remote session at the correct coordinates, and the DPI coordinate contract is defined and enforced for all future phases
**Depends on**: Phase 2
**Requirements**: INPUT-01, INPUT-02
**Success Criteria** (what must be TRUE):

  1. A mouse click sent to a known remote coordinate activates the target element (e.g. clicking a Notepad menu opens it)
  2. All mouse action types work: move, left/right/middle click, double-click, scroll, and drag
  3. Typed text and key combinations (e.g. Ctrl+A, Alt+F4) are received by the remote application
  4. The coordinate contract is documented and enforced: remote session is forced to 96 DPI (100%), and all coordinate values are in physical virtual-desktop pixels

**Plans**: 1/4 plans executed
Plans:
**Wave 1**

- [x] 03-01-PLAN.md — Owned input vocabulary (MouseAction/KeyAction/Button/Key) + Set-1 scancode table + pure Operation translation with the wheel-truncation and MouseMove-before-Wheel guards + Error::CoordinateOutOfBounds (offline, no VM)

**Wave 2** *(blocked on Wave 1 completion)*

- [ ] 03-02-PLAN.md — Session input seam (RdpInputEvent::FastPath + Mutex<Database> + desktop_size accessor + coordinate bounds check) and send_mouse: move/click/double-click/scroll/drag with caller-side timing

**Wave 3** *(blocked on Wave 2 completion)*

- [ ] 03-03-PLAN.md — Session::send_key: Type (Unicode) + Combo (scancode, modifier ordering) — Ctrl+A / Alt+F4 expressible

**Wave 4** *(blocked on Wave 3 completion)*

- [ ] 03-04-PLAN.md — Gated live screenshot-diff suite covering all 4 success criteria + canonical live-validation checkpoint (double-click / drag empirically tuned)

### Phase 4: DVC Transport Channel

**Goal**: The RDPILOT_SENSOR dynamic virtual channel is open and bidirectional by the time the RDP session is fully established, confirmed by a ping/heartbeat round-trip before any sensor modules exist
**Depends on**: Phase 2
**Requirements**: SENSOR-03
**Success Criteria** (what must be TRUE):

  1. The DVC client plugin (DvcProcessor) is registered with DrdynvcClient before connector.connect() completes (IronRDP hard constraint met)
  2. A ping request sent over the RDPILOT_SENSOR channel returns a pong response from the server-side endpoint within 500 ms
  3. A version handshake is the first message on the channel, and a mismatch causes the channel to close with a clear error (not silent data corruption)

**Plans**: TBD

### Phase 5: Sensor Bootstrap + Deployment

**Goal**: The C# NativeAOT sensor helper is built as a self-contained executable, deployed to a real remote Windows target, and confirms the DVC channel is live from its end
**Depends on**: Phase 4
**Requirements**: SENSOR-01, SENSOR-02
**Success Criteria** (what must be TRUE):

  1. The rdpilot-sensor.exe binary builds as a NativeAOT self-contained executable with no external runtime dependency
  2. The SDK deploys the sensor binary to the remote target via drive-redirection copy and launches it within the RDP session
  3. When WinRM is available, the WinRM bootstrap path also successfully deploys and launches the sensor
  4. The deployed sensor opens the RDPILOT_SENSOR DVC channel and responds to a ping within 1 second of launch

**Plans**: TBD

### Phase 6: Window + Process Perception

**Goal**: The sensor returns window list, process tree, per-window screenshots, process launch, and window focus over DVC, proving the request/response protocol with real data before the UIA module is added
**Depends on**: Phase 5
**Requirements**: PERC-01, PERC-02, PERC-04, PROC-01, CAP-02
**Success Criteria** (what must be TRUE):

  1. A get_window_list() call returns HWND, title, bounding rect, z-order, and state for all visible windows in the remote session
  2. A get_process_tree() call returns PID, parent PID, name, and path for all running processes
  3. The SDK brings a specified window to the foreground (set_foreground_window) and confirms the focus change in a subsequent window list query
  4. A launch_process() call starts a remote process (e.g. notepad.exe) and the new process appears in a subsequent process tree query

**Plans**: TBD

### Phase 7: UIA Tree Module

**Goal**: The sensor's UIA module returns a flat UiaElement[] for a specified window handle with correct bounding boxes, scoped by default to direct children to stay within latency bounds
**Depends on**: Phase 6
**Requirements**: PERC-03
**Success Criteria** (what must be TRUE):

  1. A get_uia_tree(hwnd) call for a Notepad window returns a flat UiaElement[] with id, role, name, bounding rect, enabled, visible, focusable, and depth populated
  2. All bounding box coordinates in the UIA response are in the same physical virtual-desktop pixel space as the framebuffer and window list (coordinate alignment verified)
  3. A tree walk scoped to TreeScope_Children completes within 500 ms for a standard Win32 application
  4. The response is valid JSON-serializable UiaElement[] (round-trips through serde_json without loss)

**Plans**: TBD

### Phase 8: Public SDK API + WorldState

**Goal**: A clean typed Session struct hides all IronRDP internals and sensor protocol details, and a WorldState snapshot combines framebuffer, window list, and optional UIA tree in one timestamped structure with a single enforced coordinate space
**Depends on**: Phase 7
**Requirements**: API-01, API-02
**Success Criteria** (what must be TRUE):

  1. A consumer can drive a full read/inspect workflow using only the public Session API without importing any IronRDP types or sensor protocol details
  2. Session::world_state() returns a WorldState struct containing a screenshot, window list, and optional UIA tree captured within 500 ms of each other
  3. All coordinate values in WorldState (screenshot dimensions, window rects, UIA bounding boxes, mouse input targets) are in the same virtual-desktop pixel space with no silent scaling
  4. The API compiles clean under strict Rust settings (no `unsafe` in public surface, no `unwrap` in library code)

**Plans**: TBD
**UI hint**: yes

### Phase 9: Scripted Proof Harness

**Goal**: A scripted harness connects to a real remote-only Windows program, takes screenshots, reads the UIA tree, performs navigation actions, and asserts that expected UI elements and behaviors are found — with no live LLM involved
**Depends on**: Phase 8
**Requirements**: PROOF-01
**Success Criteria** (what must be TRUE):

  1. The harness connects, authenticates, and produces a screenshot of a real remote-only Windows program (not a toy or localhost target)
  2. The harness reads the UIA tree for the target program's main window and asserts specific named elements are present with valid bounding boxes
  3. The harness injects a navigation action (e.g. menu open, button click, or text entry) and verifies the result via a follow-up screenshot or UIA query
  4. The harness completes the full loop (connect → screenshot → get_windows → get_uia_tree → navigate → verify) and exits with a pass/fail report, all assertions documented

**Plans**: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Test Environment | 4/4 | Complete | 2026-06-05 |
| 2. RDP Session + Framebuffer Core | 3/3 | Complete    | 2026-06-05 |
| 3. Input Injection | 1/4 | In Progress|  |
| 4. DVC Transport Channel | 0/? | Not started | - |
| 5. Sensor Bootstrap + Deployment | 0/? | Not started | - |
| 6. Window + Process Perception | 0/? | Not started | - |
| 7. UIA Tree Module | 0/? | Not started | - |
| 8. Public SDK API + WorldState | 0/? | Not started | - |
| 9. Scripted Proof Harness | 0/? | Not started | - |

## Backlog

### Phase 999.1: Harden live RDP test suite against frame-timing races (BACKLOG)

**Goal:** Replace fixed-time settles / blank-frame tolerances in the live integration suite with deterministic frame-readiness signals (poll-until-shell-ready, wait-for-first-real-paint), reducing flakiness like the screenshot_is_rgb_correct failure observed during Phase 2 live validation (fixed in commit 03302a0 as a band-aid).
**Requirements:** TBD
**Plans:** 3/3 plans complete

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.2: subagent tool grants — ensure file-producing agents have Write (BACKLOG)

**Goal:** Audit GSD subagent definitions so agents expected to produce files (researchers, planners, doc-writers, etc.) are granted Write/Edit tools. Observed failure: a researcher subagent burned ~6 turns trying to write a file via fs/bash/pwsh/python workarounds because it lacked Write().
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)
