# Project Retrospective

*A living document updated after each milestone. Lessons feed forward into future planning.*

## Milestone: v1.0 — MVP

**Shipped:** 2026-07-10
**Phases:** 9 | **Plans:** 35 | **Tasks:** 68

### What Was Built
- A disposable Azure Windows test environment (Bicep + `manage-env.ps1` up/down + scheduled auto-destroy) so every later phase had a real, reproducible RDP target to validate against.
- An IronRDP-based Rust session core: NLA/CredSSP connect, an SDK-owned async session loop, live framebuffer decoding, full-desktop and per-window PNG screenshots, and a 10-minute-idle-proof keepalive.
- Mouse and keyboard input injection (move, click variants, scroll, drag, typed text, key combos) under an enforced 96-DPI physical-pixel coordinate contract that every later phase (windows, UIA, WorldState) reused without re-deriving.
- The RDPILOT_SENSOR dynamic virtual channel (DVC) transport, established and version-handshake-verified before any sensor functionality existed, plus a C# .NET 8 NativeAOT sensor helper (2.57 MiB, no external runtime) deployed via RDPDR drive redirection (primary) or WinRM (fallback).
- Sensor-backed window list, process tree, per-window screenshot, foreground-focus control, and remote process launch, all served over the DVC request/response protocol.
- A UI Automation (UIA) sensor module returning a flat, JSON-serializable `UiaElement[]` — first scoped to direct children, later extended (Phase 9) with a bounded, caller-configurable deeper walk (`UiaScope::Subtree{max_depth}`) after a live spike proved children-only scope insufficient for real applications.
- A clean, typed public SDK API (`Session`) hiding all IronRDP/sensor-protocol details, and a `WorldState` snapshot correlating screenshot + window list + UIA tree in one coordinate space with a single timestamp.
- A scripted proof harness (`examples/proof_harness.rs` + a gated end-to-end test) that connects, screenshots, reads the UIA tree, navigates, and verifies against a real, remote-only Windows program (7-Zip File Manager) — no live LLM involved — closing PROOF-01 and the milestone.

### What Worked
- **Live-gate-per-phase discipline.** Every phase ended with a canonical run against a real disposable Azure VM before being marked complete, catching integration bugs (session-loop panics, focus-lock timeouts, RDPDR handshake prerequisites) that no amount of offline unit testing would have surfaced.
- **Disposable, scriptable test infrastructure built first (Phase 1).** Having `manage-env.ps1 up/down` and scheduled auto-destroy in place before any RDP code existed meant every later phase could provision a clean target on demand and tear it down immediately after, keeping cost and drift under control across 9 phases.
- **Risk-gate spikes before committing to a design.** Phase 7's D-7.5 UIA marshalling spike and Phase 9's D-9.1 7-Zip UIA-fidelity spike both surfaced hard blockers (SAFEARRAY/BSTR marshalling crashes; `TreeScope_Children` insufficiency) *before* full handler code was written, turning what could have been late-stage rework into an early, contained design correction.
- **A locked coordinate contract early (Phase 3).** Enforcing 96 DPI / physical-pixel coordinates at the input-injection phase meant every later structured-perception phase (windows, UIA, WorldState) could assume one coordinate space with no silent scaling — never revisited or debated again.
- **Two-language split (Rust SDK / C# NativeAOT sensor) paid off.** Deferring the sensor-language decision until Phase 4/5 (with hands-on DVC context) rather than locking it at project start avoided a premature commitment; C# NativeAOT's UIA COM ergonomics (Phase 7) would have been materially rougher in raw Rust.

### What Was Inefficient
- **WinRM from the Linux dev host never worked** (Negotiate/NTLM auth failures, missing GSS mechanism plugin) — every live gate from Phase 5 onward had to fall back to `az vm run-command invoke` plus a Storage-blob SAS relay to build and retrieve the sensor binary, an extra indirection that recurred every single phase instead of being solved once.
- **Recurring "first-RDP-login `deploy_and_launch` transient"** appeared across multiple phases (6, 7, 9) — the same self-resolving-on-retry timing issue was independently rediscovered and reasoned about several times rather than being permanently documented as an expected condition after its first occurrence.
- **`set_foreground_window`-before-click was independently diagnosed twice** — once in Phase 6 (SC#3) and again in Phase 9 (SC#3 navigation), the second time as a *missed* application of the exact same lesson from three phases earlier.
- **STATE.md's Performance Metrics section drifted** — velocity/session tables show partial, inconsistent data (e.g. "Total plans completed: 10" against a milestone total of 35) rather than a maintained running total, reducing its value as a cost-tracking artifact by milestone end.

### Patterns Established
- End-of-phase "live gate" as a hard phase-completion gate, not an optional nice-to-have — no phase 2-9 was marked complete without a live run against a real Azure target.
- Spike-first risk gates (throwaway code, human-verify checkpoint, delete after) for any capability with unverified third-party marshalling/API behavior (COM/UIA, DVC framing) before writing production handler code.
- Owned-type boundary discipline: the public SDK surface (Error, ConnectionConfig, Screenshot, Rect, WindowInfo, UiaElement, WorldState) never leaks IronRDP, sensor-wire, or C#-side types — enforced by compiler-level lint gates (`#![deny(unsafe_code, unwrap_used, expect_used)]`) rather than review discipline alone.
- A single enforced coordinate space (96 DPI, physical virtual-desktop pixels) threaded through every later structured-perception feature from Phase 3 onward.

### Key Lessons
1. When a live-environment transient is diagnosed and confirmed self-resolving (e.g. the first-RDP-login `deploy_and_launch` timeout), write it down once as an expected condition — it recurred three times across the milestone and was independently re-diagnosed more than once.
2. A fix discovered in one phase (e.g. Phase 6's `SetForegroundWindow`/`AttachThreadInput` focus-lock workaround) should be checked against *every* other navigation/interaction call site in the same milestone, not just the one that surfaced it — Phase 9 shipped without it and had to live-diagnose the identical bug again.
3. Live-testing infrastructure that only exists in `infra/` and a `manage-env.ps1` script is easy to forget mid-milestone if it isn't surfaced automatically — this became Backlog Phase 999.3 precisely because the gap was felt during Phase 3 execution.
4. Cross-platform build/test asymmetry (WinRM unreachable from the Linux dev host for the entire milestone) is worth solving once, early, rather than re-routing around it via VM-side builds and blob relays on every subsequent phase.

### Cost Observations
- Model mix and per-session cost were not tracked in a form suitable for aggregation this milestone — STATE.md's Performance Metrics table has partial per-plan duration entries but no consistent session or model-mix ledger.
- Sessions: not precisely tracked; work spanned 36 days (2026-06-04 → 2026-07-10) across 9 phases, 35 plans, 68 tasks, 188 commits, and ~41,877 lines added.
- Notable: every phase from 2 onward required at least one live Azure VM provisioning/gate cycle; disposable-environment tooling (Phase 1) made this a repeatable ~few-minutes-per-cycle cost rather than a manual, error-prone setup each time.

---

## Cross-Milestone Trends

### Process Evolution

| Milestone | Sessions | Phases | Key Change |
|-----------|----------|--------|------------|
| v1.0 | not tracked | 9 | Established live-gate-per-phase discipline against disposable Azure infrastructure; introduced risk-gate spikes before committing to unverified third-party API behavior (COM/UIA marshalling, DVC framing) |

### Cumulative Quality

| Milestone | Tests | Coverage | Zero-Dep Additions |
|-----------|-------|----------|-------------------|
| v1.0 | not centrally tallied (offline unit + gated live suites per phase) | not tracked | — |

### Top Lessons (Verified Across Milestones)

1. Fixes discovered live in one phase must be checked against every other call site sharing the same root cause before the phase is closed — a lesson learned and then re-learned within v1.0 itself (Phase 6 → Phase 9, `set_foreground_window`).
2. Disposable, scriptable test infrastructure built before any protocol code exists pays for itself across every subsequent phase of a milestone.