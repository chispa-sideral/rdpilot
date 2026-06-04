# Research Summary: rdpilot — AI-Driven Computer Use over RDP

**Synthesized:** 2026-06-04
**Sources:** STACK.md · FEATURES.md · ARCHITECTURE.md · PITFALLS.md
**Overall confidence:** HIGH on core stack and architecture; MEDIUM on remote helper language trade-offs

---

## Decisions the Roadmap Depends On

These are locked conclusions from the research. The roadmap should treat them as constraints, not options.

| Decision | Verdict |
|----------|---------|
| **Prior art** | `agent-rdp` (thisnick) exists: IronRDP + DVC + PowerShell UIA CLI. Proves the pattern. rdpilot differentiates by being an embeddable SDK (not a CLI), exposing a typed library API, using a compiled sensor helper, and providing clean process/window data. |
| **RDP library** | **IronRDP 0.14** (Rust, Apache-2.0/MIT). Production use by Devolutions, Teleport, Cloudflare. First-class framebuffer (`DecodedImage` RGBA32), DVC trait, and input PDU APIs. FreeRDP rejected: no maintained Rust bindings, 3.x screenshot API broken, unsafe C FFI. |
| **Primary language** | **Rust**. IronRDP is Rust-native; any other language adds FFI on the hot framebuffer and input paths. |
| **Remote sensor language** | **C# .NET 8 NativeAOT** (pragmatic choice). UIA managed `UIAutomationClient.dll` is well-tested in C#; Microsoft official DVC server sample is C#; `--self-contained` produces a zero-dependency `.exe`. Rust via `windows` crate is viable but less ergonomic for UIA tree traversal. |
| **Sensor transport** | **RDP Dynamic Virtual Channel (DVC)** on port 3389 — no extra firewall ports. WinRM as bootstrap fallback only. Channel name: `RDPILOT_SENSOR`. |
| **Perception shape** | **Flat `UiaElement[]` array** (id, role, name, bbox, enabled, visible, focusable, focused, value?, depth, parentId), not a raw tree. Matches how Anthropic, OpenAI, and UFO agent frameworks consume accessibility data. |
| **Action vocabulary** | Map directly to Anthropic/OpenAI computer-use spec: `screenshot`, `left_click`, `right_click`, `middle_click`, `double_click`, `mouse_move`, `scroll`, `left_click_drag`, `type`, `key`, `wait`. No translation layer needed for AI consumers. |
| **Hard constraint** | **Minimized/disconnected RDP session silently breaks UIA and screenshots.** Registry key `RemoteDesktop_SuppressWhenMinimized=2` (client-side) is a Phase 1 prerequisite, not an optional note. |
| **DPI contract** | Force 96 DPI (100%) on remote session; sensor sets `DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2`; emit both logical and physical coordinate spaces in perception data. |
| **v1 scope** | Windows-only; read/inspect; no live LLM required; done = scripted harness proves end-to-end. |

---

## Executive Summary

rdpilot occupies a nearly empty space: no general-purpose "RDP session as programmable SDK" existed before mid-2025. The closest prior art, `agent-rdp`, proves that IronRDP + DVC + Windows UI Automation is a viable pattern, but it is a CLI tool, not a library. rdpilot differentiates with a clean embeddable Rust SDK with a typed API surface, a compiled (not interpreted) remote sensor, structured process and window data, and a consumption shape compatible with existing AI computer-use frameworks (Anthropic, OpenAI, UFO) without translation.

The recommended stack is highly convergent across all four research dimensions: IronRDP 0.14 as the Rust-native RDP client, a C# NativeAOT sensor helper for the Windows-side UIA/Win32 work, DVC as the in-band structured-data transport, and a flat `UiaElement[]` perception shape that agent frameworks can consume directly. The build order is dependency-locked: the RDP session and framebuffer must work before input injection; the DVC channel must work before the sensor is deployed; the sensor window/process modules must be stable before the UIA module is added (UIA is the most expensive and most complex module).

The most critical non-obvious risk is session rendering state: a minimized or disconnected RDP session silently pauses the display pipeline and invalidates all UIA coordinates. This must be addressed as a Phase 1 prerequisite. Secondary risks are DPI coordinate mismatches (force 96 DPI and emit both coordinate spaces), UIPI integrity level blocking input injection against elevated processes (scope v1 to standard-integrity targets), and AV/EDR flagging the sensor binary as a RAT (minimize binary surface, plan for code signing, document exclusion requirement from day one).

---

## Key Findings

### From STACK.md

- **IronRDP 0.14** is the unambiguous RDP library choice. Version confirmed active (Jan 2026), dual-licensed, used in production by three major companies. `DecodedImage` RGBA32 framebuffer directly accessible as `&[u8]`. DVC is a first-class trait, not a plugin hack.
- **FreeRDP explicitly rejected.** No maintained Rust FFI, 3.x removed screenshot proxy module with no replacement, known crash on screenshot (issue #8735).
- **Rust is the correct primary language.** `agent-rdp` (the only proven prior art) is Rust. Any other language adds FFI on the hot loop.
- **C# NativeAOT for the sensor.** Produces a single `.exe` with no runtime dependency. Microsoft own DVC sample repo is 33.6% C#. Managed UIA wrapper is better-tested than `windows-rs` for tree traversal.
- **WinRM is bootstrap infrastructure, not the primary sensor transport.** Use it to deploy and launch the sensor binary; use DVC for ongoing structured data exchange.
- **Key Cargo dependencies:** `ironrdp 0.14`, `ironrdp-tokio 0.14`, `ironrdp-tls 0.14`, `ironrdp-dvc 0.14`, `ironrdp-input 0.14`, `tokio 1`, `image 0.25`, `serde + serde_json 1`, `windows 0.58` (sensor), `uiautomation 0.22` (sensor, optional Rust path).

### From FEATURES.md

**Must-have (all 13 table stakes):**
- TS-1/TS-2: Connect (NLA/CredSSP) + session lifecycle
- TS-3/TS-4: Full-desktop screenshot + per-window crop
- TS-5/TS-6: Mouse input (move/click/scroll/drag) + keyboard input (type/combos)
- TS-7: Window enumeration (HWND, title, rect, z-order, state)
- TS-8: UIA tree as flat `UiaElement[]`
- TS-9: Process tree enumeration
- TS-10: Sensor bootstrap + DVC transport
- TS-11: Remote process launch + observe
- TS-12: Wait/idle detection (screen-stable + window-ready tiers)
- TS-13: Clipboard read/write

**Strongly recommended additions to v1 (near-zero cost):**
- D-8: JSON-serializable `UiaElement[]` snapshot (falls out of TS-8)
- D-9: Window focus/bring-to-front (one sensor call, needed before nearly every interaction)

**Defer to v2+:** UIA event streaming (D-1), Set-of-Marks overlay (D-3), UIA InvokePattern/ValuePattern (D-4), session reconnect/resume (D-5), multi-monitor (D-6).

**Anti-features (never or not v1):** AI logic on remote machine, MCP/CLI packaging, non-Windows targets, write guardrails, published-package stability, multi-session orchestration, co-driving UX.

**Action vocabulary must match Anthropic/OpenAI spec:** `screenshot`, `left_click`, `right_click`, `middle_click`, `double_click`, `triple_click`, `mouse_move`, `left_click_drag`, `scroll`, `type`, `key`, `hold_key`, `wait`. This is the harness contract for AI consumers.

### From ARCHITECTURE.md

**Six components, two transports, one process boundary:**

| Component | Location | Responsibility |
|-----------|----------|----------------|
| RDP Session Manager | Local | IronRDP connection lifecycle; DVC registration at connect time (hard constraint: must register before `connector.connect()` completes) |
| Framebuffer/Pixel Layer | Local | `DecodedImage` maintained by `ActiveStage`; `screenshot()` and `screenshot_window(hwnd, rect)` |
| Input Injector | Local | Abstract mouse/key actions to `FastPathInputEvent` PDUs; all coordinates are absolute virtual-desktop space |
| Sensor Client | Local | DVC `DvcProcessor` impl; typed async calls: `get_process_tree()`, `get_window_list()`, `get_uia_tree(hwnd?)`, `launch_process(cmd)` |
| Public SDK API | Local | `Session` struct; hides all IronRDP internals and sensor protocol |
| rdpilot-sensor | Remote | C# NativeAOT `.exe` in the RDP user session; opens `RDPILOT_SENSOR` DVC; executes Win32/UIA/WMI queries; returns JSON responses; zero agent logic |

**Coordinate system:** One space — virtual desktop pixels (0,0) top-left. Screenshot, `GetWindowRect`, UIA `BoundingRectangle`, and RDP mouse input are all in this space. The only exception is DPI scaling (mitigated by forcing 96 DPI).

**WorldState snapshot pattern:** Single sensor request returns window list + optional UIA tree atomically; SDK immediately captures framebuffer and packages both with timestamps. Staleness < 50 ms for window list, < 500 ms for UIA.

**Anti-patterns to avoid:** Intelligence in the sensor; per-window DVC channels; full-desktop UIA dump by default; out-of-band TCP socket instead of DVC.

### From PITFALLS.md

**Critical — must address in the phase they occur:**

| Pitfall | Phase | Prevention |
|---------|-------|------------|
| **C1: Minimized/disconnected RDP kills UIA + screenshots** | Phase 1 | `RemoteDesktop_SuppressWhenMinimized=2` registry key on client machine; verify session state before asserting UIA results |
| **C2: Console vs. virtual session collision** | Phase 1 | Dedicated service account never used for interactive logins; detect session ID at connect time |
| **C3: NLA/CredSSP cert trust failures** | Phase 1 | Pin cert thumbprint; support `DisableNLA` for lab targets; test domain-joined and workgroup separately |
| **C4: UIPI blocks input injection to elevated processes** | Phase 2 | v1 read-only scope limits exposure; document that v1 targets must run at standard integrity |
| **C5: AV/EDR flags sensor as RAT** | Phase 3/4 | Minimal binary surface; plan code signing from day one; document AV exclusion requirement in deployment guide |
| **C6: DPI scaling + coordinate mismatch** | Phase 2 | Force 96 DPI on remote session; sensor sets per-monitor DPI awareness; emit both coordinate spaces |

**Moderate — design around, not retrofit:**
- M1: RDP idle timeout — keep-alive (synthetic null input every ~60 s) in Phase 1 session loop
- M2: UIA full-tree walk performance (5-30 s for complex apps) — default `TreeScope_Children`; cache with TTL; never full-desktop dump by default
- M3: Applications without UIA (DirectX, legacy owner-draw, some Electron) — UIA is opportunistic enrichment; screenshot is always ground truth
- M4: DVC version skew — version handshake as first message on channel

---

## Implications for Roadmap

The four research dimensions converge on the same 8-phase dependency-ordered build sequence.

### Suggested Phase Structure

**Phase 1 — RDP Session + Framebuffer Core**
- Deliverable: connect (NLA/CredSSP), session loop, full-desktop screenshot, keepalive
- Features: TS-1, TS-2, TS-3
- Must address: C1 (`RemoteDesktop_SuppressWhenMinimized=2`), C2 (service account model), C3 (NLA cert pinning), M1 (keep-alive), m2 (YUV pixel format conversion)
- Rationale: Everything else depends on a working session and framebuffer. C1 must be fixed here or all UIA tests in Phase 6 will be flaky.

**Phase 2 — Input Injection**
- Deliverable: mouse (move/click/scroll/drag) + keyboard (type/combo); DPI coordinate contract defined
- Features: TS-5, TS-6
- Must address: C6 (DPI coordinate contract defined and enforced), C4 (document UIPI scope limit for v1)
- Rationale: Proves the perception+action round-trip. DPI contract must be defined here before any consumer uses coordinates.

**Phase 3 — Sensor Transport (DVC channel)**
- Deliverable: DVC channel (`RDPILOT_SENSOR`) established end-to-end; ping/heartbeat confirmed
- Features: transport half of TS-10
- Must address: C5 (design sensor binary surface now — minimize footprint), M4 (version handshake on first message)
- Rationale: DVC must be registered at connect time (IronRDP hard constraint). Testing as a standalone phase before sensor has real modules isolates protocol bugs.

**Phase 4 — Sensor Bootstrap + Deployment**
- Deliverable: `rdpilot-sensor.exe` (C# NativeAOT) built, deployed via drive-redirection or WinRM, running in RDP session and confirming DVC channel is live
- Features: deployment half of TS-10
- Research needed: NativeAOT build pipeline, binary size, bootstrap reliability on real target
- Rationale: Real target deployment surfaces integration issues (AV, permissions, session targeting) before any perception modules are added.

**Phase 5 — Window + Process Perception**
- Deliverable: `get_window_list()` (HWND, title, rect, z-order, state) and `get_process_tree()` over DVC
- Features: TS-7, TS-9, TS-4 (per-window screenshot crop), TS-11 (remote process launch), TS-12 (window-ready wait), D-9 (window focus)
- Rationale: Win32 window/process APIs are fast and low-risk. Proves DVC request/response protocol with simple data before adding expensive UIA.

**Phase 6 — UIA Tree Module**
- Deliverable: `get_uia_tree(hwnd?)` returning flat `UiaElement[]` over DVC; scope defaulting to focused window; opt-in depth limit
- Features: TS-8, D-8 (JSON snapshot)
- Must address: M2 (tree walk performance — default `TreeScope_Children`, `CacheRequest`, TTL cache)
- Research needed: UIA tree walk strategy and performance profile against actual target application
- Rationale: UIA is the most complex and expensive module. Separate phase keeps the debugging surface manageable.

**Phase 7 — Public SDK API + WorldState**
- Deliverable: Clean typed `Session` struct with all methods; `WorldState` snapshot (framebuffer + window list + optional UIA); coordinate-space contract enforced in API types; TS-13 (clipboard)
- Features: all TS features complete; public API surface stable for harness consumption
- Rationale: Integrates all components into a coherent library before the proof harness.

**Phase 8 — Scripted Proof Harness**
- Deliverable: Harness that connects, screenshots, get_windows, get_uia_tree, navigates a real remote-only program, asserts expected UIA properties found
- Features: v1 finish line
- Research needed: depends on the specific remote-only target application UIA fidelity
- Rationale: Proves end-to-end against a real (non-toy) target. No live LLM required.

### Research Flags

| Phase | Needs deeper research during planning? | Notes |
|-------|----------------------------------------|-------|
| Phase 1 | No | IronRDP framebuffer + NLA well-documented; official screenshot example exists |
| Phase 2 | No | IronRDP input PDU APIs verified; coordinate system straightforward |
| Phase 3 | No | IronRDP DVC traits documented; MS DVC sample repo covers server side |
| Phase 4 | Yes | C# NativeAOT build pipeline, binary size, bootstrap reliability on real target |
| Phase 5 | No | Win32 `EnumWindows`/`GetWindowRect` are standard APIs |
| Phase 6 | Yes | Optimal UIA tree-walk strategy; performance profile on target application(s) |
| Phase 7 | No | API design follows from completed components |
| Phase 8 | Depends on target | Harness content depends on the specific remote-only program |

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack (IronRDP, Rust, C# sensor) | HIGH | Version-verified, production deployments confirmed, official samples exist |
| Features (action vocabulary, UiaElement shape) | HIGH | Derived from official Anthropic/OpenAI specs and Windows UIA docs |
| Architecture (6 components, DVC transport, WorldState) | HIGH | Verified against IronRDP ARCHITECTURE.md, MS-RDPEDYC spec, agent-rdp prior art |
| Pitfalls (C1 session rendering, C6 DPI, AV/EDR) | HIGH | Confirmed by RPA vendor docs (UiPath, SmartBear, Power Automate), issue trackers, MS docs |
| Sensor language (C# vs Rust) | MEDIUM | C# is the pragmatic choice but Rust is viable; can revisit after Phase 3 DVC prototype |
| UIA performance (tree walk latency) | MEDIUM | 100-500 ms estimate from RPA docs; actual profile against target application unknown |
| Bootstrap reliability (drive redirection vs WinRM) | MEDIUM | WinRM clean but disabled by default on desktop Windows; drive redirection keyboard-macro launch is fragile |

### Gaps to Address During Planning

1. **Target application UIA fidelity** — identify and test the actual remote-only target application UIA coverage before designing Phase 8 harness assertions. If it uses DirectX/OpenGL or owner-draw controls, UIA will be sparse.
2. **AV/EDR environment on the target** — whether the target has EDR determines how aggressively the sensor must be hardened. Clarify before Phase 4.
3. **Drive redirection GPO policy** — if the target is a managed machine with drive redirection disabled, the Phase 4 bootstrap must fall back to WinRM. Clarify the target RDP policy before Phase 4.
4. **NativeAOT binary size** — C# NativeAOT for a sensor with UIA + DVC can range 5-30+ MB. Benchmark during Phase 4; matters if deployed over a slow link.

---

## Sources (Aggregated)

- IronRDP GitHub (Devolutions): https://github.com/Devolutions/IronRDP
- IronRDP screenshot example: https://github.com/Devolutions/IronRDP/blob/master/crates/ironrdp/examples/screenshot.rs
- MS-RDPEDYC (DVC spec): https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpedyc/
- microsoft/rdp-dvc-plugin-samples: https://github.com/microsoft/rdp-dvc-plugin-samples
- agent-rdp (thisnick/agent-rdp): https://github.com/thisnick/agent-rdp
- uiautomation-rs (leexgone): https://github.com/leexgone/uiautomation-rs
- Anthropic Computer Use Tool Docs: https://platform.claude.com/docs/en/docs/agents-and-tools/tool-use/computer-use-tool
- OpenAI Computer Use Guide: https://developers.openai.com/api/docs/guides/tools-computer-use
- UFO / UFO2 papers: https://arxiv.org/html/2402.07939v1 and https://arxiv.org/html/2504.14603v1
- Windows UIA Property IDs: https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-automation-element-propids
- UiPath RDP minimized window docs: https://docs.uipath.com/robot/standalone/2024.10/admin-guide/executing-automations-in-minimized-rdp-windows
- SmartBear TestComplete RDP docs: https://support.smartbear.com/testcomplete/docs/testing-with/running/via-rdp/keeping-computer-unlocked.html
- Microsoft Power Automate UIPI issues: https://learn.microsoft.com/en-us/troubleshoot/power-platform/power-automate/desktop-flows/ui-automation/uipi-issues
- FreeRDP issues #11765 and #8735: https://github.com/FreeRDP/FreeRDP/issues/11765
- Windows Session 0 Isolation: https://techcommunity.microsoft.com/blog/askperf/application-compatibility---session-0-isolation/372361
