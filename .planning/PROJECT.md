# rdpilot

AI-driven computer use over RDP — a reusable Windows SDK that lets a local agent perceive and drive a remote Windows desktop as if it were local hardware.

## What This Is

`rdpilot` is a reusable **SDK/library** that opens and manages an RDP session to a remote **Windows** machine and exposes that session as a controllable, *perceivable* surface. It is the foundation; AI computer use is its first and most demanding consumer.

The library provides two layers of perception over a single managed remote desktop:

- **Pixels (RDP-native):** screenshots of the full desktop or an individual window, plus mouse/keyboard input injection mapped to remote coordinates.
- **Structured (helper-sourced):** the remote process tree, window list/geometry, and the UI Automation (accessibility) tree — data the RDP protocol does *not* carry natively, retrieved via a **thin remote "sensor" helper** (a dumb perception/control component, explicitly NOT the AI agent) running on the target over an RDP virtual channel and/or out-of-band Windows remote management (WinRM/WMI).

The deliberately rejected alternative is the "obvious" path of running the AI agent inside the RDP session. The whole point is that intelligence stays on the **local** workstation and operates the remote desktop through `rdpilot`.

## Core Value

A local AI agent can connect to a remote Windows desktop over RDP and **read/inspect a program that is only reachable via RDP** — navigating it and reporting what it sees — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.

## Current Milestone: v1.1 — Consumer Surfaces & File Transfer

**Goal:** Turn the proven rdpilot SDK into agent- and human-drivable surfaces — an MCP server and a CLI over a persistent session daemon — with bidirectional file transfer, proven both by scripted per-surface harnesses and a live-LLM MCP demo.

**Target features:**
- Session daemon: long-lived background service holding live RDP sessions + keepalive, exposing a local IPC (socket/named pipe); a shared session registry keyed by name-or-auto-id.
- Session identity: sessions referenced by user-supplied name (else auto-generated id); no implicit default target — every command explicitly names its session.
- CLI surface: thin client over the daemon — session lifecycle (connect/list/disconnect) + perception/input/launch/file verbs, each explicitly targeting a session.
- MCP server surface: dual tool surface — Anthropic computer-use-compatible tools plus rdpilot-native tools; a daemon client.
- Bidirectional file transfer: upload local→remote and download remote→local, exposed through both surfaces.
- Layered connection config: config file + env + flags for target host + credentials.
- Proof: scripted proof harness per surface plus a capstone live-LLM demo driving a read/inspect + file-transfer task through the MCP surface.

## Context

- **Author / audience:** Solo author (Marc). **Personal tooling first** — built for real use, clean enough to open-source later (LGPL-3.0 leaning), but v1 does not carry the burden of public API stability or polished published-package docs.
- **Motivation:** Real recurring need to operate remote Windows environments from a local agent — checking something in a program only available over RDP, performing manual software updates, occasionally remote-assisting a person. Computer use for AI is nascent; doing it *over RDP* is unsolved.
- **Stance on prior art:** Assume nothing exists in this space (the author doubts it does). Prior-art discovery is an explicit research dimension, not an assumption.
- **v1 focus:** Read / inspect tasks — low blast radius, a safe and provable first loop. Write-heavy automation comes later.
- **v1 finish line:** The SDK, proven end-to-end by a **scripted/manual harness** that connects, screenshots, reads the UIA tree, and navigates a real remote-only program. No live LLM agent is required to call v1 "done."

## Requirements

### Validated

- ✓ **ENV-01** Bicep template provisions an Azure Windows VM configured for RDP automation — v1.0 (live-verified 2026-06-04, all six ENV-01 assertions green)
- ✓ **ENV-02** A `.ps1` script brings the test environment up and tears it down on demand — v1.0 (live-verified 2026-06-04)
- ✓ **ENV-03** Scheduled auto-destroy safeguard tears down the VM/resource group automatically — v1.0 (live-verified 2026-06-05, runbook fired unattended and reaped `rdpilot-test`)
- ✓ **SESS-01** Connect to and authenticate (NLA / credentials) an RDP session to a Windows target — v1.0
- ✓ **SESS-02** Manage the session lifecycle (open, keepalive, teardown); keep session rendered so perception stays live — v1.0
- ✓ **CAP-01** Capture a full-desktop screenshot from the RDP framebuffer — v1.0
- ✓ **CAP-02** Capture a per-window cropped screenshot — v1.0
- ✓ **INPUT-01** Inject mouse actions (move, click variants, scroll, drag) at remote coordinates — v1.0 (10/10 live tests pass, 2026-07-08)
- ✓ **INPUT-02** Inject keyboard input (type text and key combinations/modifiers) — v1.0 (10/10 live tests pass, 2026-07-08)
- ✓ **SENSOR-03** DVC request/response transport channel carries structured-perception data — v1.0 (live round trip 165ms, 2026-07-09)
- ✓ **SENSOR-01** Thin C# .NET 8 NativeAOT sensor helper exposes structured-perception queries — v1.0 (self-contained, 2.57 MiB, no external runtime, 2026-07-09)
- ✓ **SENSOR-02** SDK bootstraps/deploys and launches the sensor on the target (RDPDR primary, WinRM fallback) — v1.0 (RDPDR 22.61ms, WinRM 21.62ms, both live-verified 2026-07-09)
- ✓ **PERC-01** Enumerate the remote process tree — v1.0
- ✓ **PERC-02** Enumerate remote windows (titles, geometry, foreground/z-order) — v1.0
- ✓ **PERC-04** Query and set the foreground window (focus) — v1.0
- ✓ **PROC-01** Launch and observe a remote process — v1.0
- ✓ **PERC-03** Retrieve the UI Automation tree as a flat `UiaElement[]` — v1.0 (30.36ms TreeScope_Children walk, live-verified 2026-07-09)
- ✓ **API-01** Clean, typed SDK API surface exposing control + perception — v1.0
- ✓ **API-02** Coherent `WorldState` correlating screenshot + window list + UIA snapshot in one coordinate space — v1.0 (capture_span 23-73ms, live-verified 2026-07-10)
- ✓ **PROOF-01** Scripted harness proves the full read/inspect loop end-to-end against a real remote-only Windows program — v1.0 (live-verified against real 7-Zip File Manager, 2026-07-10, `PROOF: PASS`)

All 20 v1 requirements validated. v1.0 milestone shipped 2026-07-10.

### Active

- [ ] Session daemon: long-lived background service holding live RDP sessions + keepalive, exposing a local IPC (socket/named pipe); a shared session registry keyed by name-or-auto-id
- [ ] Session identity: sessions referenced by user-supplied name (else auto-generated id); no implicit default target
- [ ] CLI surface: thin client over the daemon — session lifecycle + perception/input/launch/file verbs, each explicitly targeting a session
- [ ] MCP server surface: dual tool surface — Anthropic computer-use-compatible tools plus rdpilot-native tools; a daemon client
- [ ] Bidirectional file transfer: upload local→remote and download remote→local, exposed through both surfaces
- [ ] Layered connection config: config file + env + flags for target host + credentials
- [ ] Proof: scripted proof harness per surface plus a capstone live-LLM demo through the MCP surface

### Out of Scope

- Running the AI agent on the remote session — explicitly rejected; intelligence stays local
- MCP server / Anthropic computer-use shim / CLI packaging — deferred to v2 (see REQUIREMENTS.md v2 Requirements); v1's first consumer was a scripted harness
- Non-Windows RDP targets (Linux/xrdp, macOS) — Windows-only v1 to lean on UIA/WinRM/WMI/RAIL; reasoning still valid, no v1.0 finding invalidated it
- Heavy write/destructive automation and its guardrails — beyond what read/inspect needs; reasoning still valid
- Published-package polish — public API stability guarantees, comprehensive docs, multi-registry distribution; reasoning still valid, this remains personal tooling first
- Multi-session orchestration / concurrency at scale — reasoning still valid, not exercised by v1.0's single-session proof harness
- Remote-assist co-driving UX — reasoning still valid; captured as Backlog Phase 999.4 (Remote Assistance / Shadowing) for future consideration, not silently dropped
- Clipboard read/write over CLIPRDR — deferred to v2 (see REQUIREMENTS.md v2 Requirements)
- File transfer to/from the remote target — deferred to v2 (see REQUIREMENTS.md v2 Requirements)
- PyO3 / NAPI-RS bindings for TypeScript/Python consumers — deferred to v2 (see REQUIREMENTS.md v2 Requirements)

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| The SDK/library is the product; AI is its first consumer | Decouples core RDP perception/control from packaging (MCP/shim/CLI), which become later concerns | ✅ Good — v1.0 proof harness consumed the SDK with zero IronRDP/sensor-protocol leakage (API-01) |
| A thin remote "sensor" helper is permitted on the target | Only viable way to get structured perception (process/UIA tree) over RDP; it is a dumb sensor, not the agent | ✅ Good — C# NativeAOT sensor shipped, DVC+RDPDR+WinRM transport all live-proven |
| Windows-only for v1 | Lets the SDK use Windows-native APIs (UI Automation, WinRM, WMI, RAIL, pwsh) instead of lowest-common-denominator pixel scraping | ✅ Good — UIA/RDPDR/WinRM all used directly with no abstraction tax |
| Pixels + structured perception (not vision-only) | Richer grounding for the agent than screenshots alone | ✅ Good — WorldState correlates screenshot + window list + UIA tree in one coordinate space (API-02) |
| **Primary language: Rust + IronRDP** | IronRDP is the only actively-maintained library giving programmatic framebuffer + input injection + custom DVC cleanly, with zero FFI on the hot path. FreeRDP rejected (stale Rust bindings, unsafe C, painful Windows builds); MS ActiveX rejected (rendering control only — headless framebuffer + custom DVC + input are inadequate for an SDK). Rust surface kept thin; typed consumer API exposed later via PyO3/Python or JSON-RPC socket. | ✅ Decided (2026-06-04) — held for the whole milestone; IronRDP versions turned out non-uniform across crates (corrected in Phase 2 Plan 01), otherwise no regrets |
| **Sensor language: C# .NET 8 NativeAOT vs all-Rust** | C# NativeAOT: most ergonomic UIA, self-contained native exe, no runtime on target; cost: 2nd language + .NET SDK in build. All-Rust (`windows` crate + `uiautomation-rs`): one toolchain, no .NET dependency; cost: rougher UIA/COM code. DVC channel (Phase 4) and sensor (Phase 5) are several phases out — best decided with hands-on context. | ✅ Good — C# NativeAOT chosen at Phase 5; shipped as a 2.57 MiB self-contained win-x64 exe with no external .NET runtime; UIA COM interop (Phase 7) would have been materially rougher in raw Rust |
| v1 "done" = scripted proof, no live LLM | Isolates the genuinely hard problem (RDP perception fidelity) from agent/packaging work | ✅ Good — `examples/proof_harness.rs` proved the full connect→screenshot→UIA→navigate→verify loop against real 7-Zip with zero LLM involvement (PROOF-01) |
| Build toolchain: `x86_64-pc-windows-gnu` (MinGW) not `-msvc` | Host is ARM64 Windows with no MSVC/Windows SDK; GNU cross-toolchain produces functionally equivalent x64 artifacts for a pure-Rust RDP client | ✅ Good — held for the whole milestone, no MSVC-only blocker ever surfaced |
| `rdpsnd` stub static channel required alongside RDPDR | MS-RDPEFS Appendix A footnote: a Windows RDP server withholds the RDPDR Server Announce Request unless `rdpsnd` is also advertised/joined — discovered live during the Phase 5 gate | ✅ Good, but a permanent architectural addition (non-functional presence-only stub), not a temporary hack — carried forward as a Phase 5 residual note |
| `UiaScope::Subtree{max_depth}` added mid-milestone (Phase 9 Plan 02) | `TreeScope_Children`-only (D-7.4, Phase 7) proved insufficient for 7-Zip's meaningful UI elements — discovered live at the Phase 9 D-9.1 spike gate | ✅ Good — bounded BFS walk (`UIA_MAX_WALK_DEPTH` cap), live-tuned from depth 4 to 3 at the terminal gate to bring latency from 555.2ms to 130.2ms with zero coverage loss |
| **D-16:** v1.1 ships exactly two consumer surfaces — MCP server + CLI; PyO3/NAPI-RS bindings and clipboard/CLIPRDR deferred | Focus the milestone on the first non-harness consumers; bindings are a separate concern | — Pending |
| **D-17:** CLI and MCP server are both thin clients of a single long-lived session daemon over local IPC | The CLI is invoke-and-exit and cannot hold an RDP session/keepalive; the daemon owns session state so both surfaces share one registry | — Pending |
| **D-18:** Session identity is always explicit (name or auto-id), no implicit default target, enforced as a required wire-schema field | An AI agent acting on the wrong session is dangerous; a required field prevents convenience shortcuts from reintroducing a default | — Pending |
| **D-19:** Daemon uses an in-memory session registry with orphan-cleanup on restart; durable reattach across daemon restart deferred | Balanced cost/safety for v1.1; avoids RDP-reconnect-semantics complexity while preventing zombie-session accumulation | — Pending |
| **D-20:** Bidirectional file transfer generalizes the existing RDPDR path, not CLIPRDR | RDPDR already deploys the sensor; CLIPRDR is clipboard-shaped, reserved for the deferred clipboard feature | — Pending |
| **D-21:** MCP presents a dual surface: one Anthropic computer-use-compatible `computer` tool plus a small rdpilot-native tool set | Matches Anthropic's own single-tool design and avoids degrading model tool-selection | — Pending |
| **D-22:** MCP file put/get operate on local disk paths, returning path/size/checksum metadata, never inline file bytes | Real MCP clients corrupt base64 binary content above ~12KB (multiple open upstream bugs) | — Pending |
| **D-23:** Consumers use a conventionally-named, discoverable, gitignored config file layered with env vars + CLI flags / MCP init params | Follow common CLI-tool conventions so the file is self-explanatory | — Pending |
| **D-24:** IPC/MCP wire and logs must redact credentials on Serialize paths (v1.0's D-14 only covered Debug) | derive(Serialize) does not inherit Debug redaction; a naive list-sessions response would leak the password | — Pending |
| **D-25:** Dual finish line — scripted proof harness per surface (no live LLM) AND a capstone live-LLM demo through the MCP surface | Keep v1's provable-without-LLM rigor while proving the namesake AI-consumer path once end-to-end | — Pending |
| **D-26:** Rust stack additions: rmcp 2.2.0 (MCP SDK), interprocess 2.4.2 (IPC), clap 4.6.1 (CLI), config 0.15.25 (layered config) | Current, well-maintained, cross-platform; anti-recommend jsonrpsee/daemonize/axum/tonic for local IPC | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

## Current State

**v1.0 MVP shipped 2026-07-10.** All 9 phases (35/35 plans, 68 tasks) complete; all 20 v1 requirements validated, most proven live against a real disposable Azure Windows VM. ~41,877 LOC added over 36 days (2026-06-04 → 2026-07-10, 188 commits, 168 files touched) across two languages:

- **Rust SDK** (`crates/rdpilot`): IronRDP-based session management, framebuffer capture, mouse/keyboard input injection with an enforced 96-DPI physical-pixel coordinate contract, DVC/RDPDR transport, and a typed public API (`Session`, `WorldState`) with strict lint gates (no `unsafe`, no `unwrap`/`expect` in library code).
- **C# .NET 8 NativeAOT sensor** (`sensor/`): a self-contained win-x64 executable (2.57 MiB, no external runtime) exposing window enumeration, process-tree enumeration, foreground focus control, process launch, and a bounded/depth-capped UI Automation tree walk, all served over the RDPILOT_SENSOR DVC channel.
- **Disposable test infrastructure** (`infra/`): Bicep-provisioned Azure Windows VM with scheduled auto-destroy, used as the live validation target for every phase gate.

The full read/inspect loop (connect → screenshot → enumerate windows/processes → read UIA tree → navigate → verify) was proven live end-to-end against a real, remote-only Windows program (7-Zip File Manager) with no LLM involved (PROOF-01), closing the v1.0 milestone. Next: define v2 scope (candidates already recorded in REQUIREMENTS.md "v2 Requirements (Deferred)": clipboard/CLIPRDR, file transfer, MCP/CLI packaging, PyO3/NAPI-RS bindings) or promote a Backlog item (see ROADMAP.md Backlog, 999.1-999.4).

---
*Last updated: 2026-07-10 after milestone v1.1 started*
