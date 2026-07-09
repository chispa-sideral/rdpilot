# Phase 6: Window + Process Perception - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-07-09
**Phase:** 6-Window + Process Perception
**Areas discussed:** Per-window screenshots, launch_process contract, perception data richness, remote-action failure model

---

## Per-window screenshots

| Option | Description | Selected |
|--------|-------------|----------|
| Client-side crop of desktop framebuffer | Reuse existing `Screenshot::crop(Rect)`; no sensor round-trip; known limitation on occluded/minimized windows | ✓ |
| Sensor-side capture (PrintWindow/BitBlt) | Full fidelity regardless of occlusion, but requires new sensor capture code and round-trip this phase | |

**User's choice:** Client-side crop of the desktop framebuffer (recommended option).
**Notes:** Per-window screenshots are part of the phase goal but not one of the 4 hard success criteria, so the lean, no-new-sensor-code approach is acceptable for v1. Sensor-side capture deferred to backlog.

---

## launch_process contract

| Option | Description | Selected |
|--------|-------------|----------|
| Fire-and-forget | Sends launch, returns new PID (or semantic failure); caller confirms via follow-up `get_process_tree()` | ✓ |
| Wait-and-confirm | Sensor blocks until process/main-window appears before replying | |

**User's choice:** Fire-and-forget (recommended option).
**Notes:** Matches the success criterion's own wording ("the new process appears in a subsequent process tree query"). No wait/poll logic needed on the sensor side for v1. Wait-and-confirm variant deferred to backlog if a consumer later needs it.

---

## Perception data richness

| Option | Description | Selected |
|--------|-------------|----------|
| Success-criteria floor + high-value extras | Window: HWND, title, rect, z-order, state, class name, owning PID. Process: PID, parent PID, name, path, command line, owner user | ✓ |
| Success-criteria floor only | Minimal fields strictly required by the 4 success criteria | |
| Full Win32/WMI field set | Also include elevation/integrity level, session id, window styles, is-tool-window/cloaked flags | |

**User's choice:** Success-criteria floor + high-value extras (recommended option).
**Notes:** Window class name and owning PID are cheap (via `GetWindowThreadProcessId`) and link windows to processes. Command line and owner user add real diagnostic value for process records. Elevation/integrity, session id, window styles, and cloaked flags excluded for v1 — deferred to backlog.

---

## Remote-action failure model

| Option | Description | Selected |
|--------|-------------|----------|
| Explicit typed errors (semantic vs. transport) | Distinguish sensor-reported semantic failure (`success:false` + reason) from transport failure (no reply, timeout, DVC not open) on the owned `Error` enum | ✓ |
| Single generic error variant | One catch-all error type for any remote-action failure | |

**User's choice:** Explicit typed errors distinguishing semantic vs. transport failure (recommended option).
**Notes:** Mirrors the existing `Error::category()` pattern (`crates/rdpilot/src/error.rs:76-91,170-171`). Lets an AI consumer decide retry-vs-requery based on failure category. Maintains the no-panic / drop-never-crash discipline on both sides.

---

## Claude's Discretion

- Mechanism for generalizing `SensorShared::pending` (`Sender<()>` → payload-carrying sender) — left to the planner.
- Whether to add a distinct `RdpInputEvent` variant per new request type or unify into a single `Request(MsgType, u64, Option<Value>)` variant.
- WMI (`System.Management`) vs. raw P/Invoke (`Toolhelp32Snapshot`) for process enumeration, pending a NativeAOT-compatibility check.

## Deferred Ideas

- Sensor-side per-window capture (PrintWindow/BitBlt) for occluded/minimized windows.
- Richer process/window fields: elevation/integrity level, session id, window styles, is-tool-window/cloaked flags.
- Wait-and-confirm `launch_process` variant (block until process/main-window appears).
