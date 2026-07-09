---
phase: 06-window-process-perception
plan: 02
subsystem: api
tags: [rust, session, dvc, perception, error-handling, screenshot]

# Dependency graph
requires:
  - phase: 06-window-process-perception
    plan: 01
    provides: "Generalized DVC request/response plumbing (RdpInputEvent::Request, SensorShared::pending<Value>), owned WindowInfo/WindowState/ProcessInfo + *Wire/into_owned conversions, Error::SensorRejected, and the Phase 6 wire contract"
provides:
  - "Session::get_window_list() -> Result<Vec<WindowInfo>> (PERC-02)"
  - "Session::get_process_tree() -> Result<Vec<ProcessInfo>> (PERC-01)"
  - "Session::set_foreground_window(hwnd) -> Result<()> (PERC-04)"
  - "Session::launch_process(exe, args, cwd) -> Result<u32> (PROC-01, D-6.2 fire-and-forget)"
  - "Session::screenshot_window(&WindowInfo) -> Result<Screenshot> (CAP-02, D-6.1 client-side crop, no sensor round trip)"
  - "Session::sensor_request(): shared round-trip/timeout/D-6.4-branching helper behind all four sensor-backed methods"
  - "ENUMERATION_TIMEOUT_MS (2000ms) / ACTION_TIMEOUT_MS (500ms) tuning constants, gate-tunable"
affects: [06-03-window-process-perception, 06-04-window-process-perception, 06-05-window-process-perception]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Shared sensor_request() private helper factoring ping()'s five-step round-trip shape (handshake fast-fail, req_id alloc, pending-map insert, input_tx.send, timeout await) plus the new D-6.4 success/data vs success:false->SensorRejected branch, so the four public methods are thin typed wrappers instead of four duplicated match blocks"
    - "Pure crop_to_window() helper factored out of the async screenshot_window() method so the D-6.1 crop-mapping logic is unit-testable with a synthetic Screenshot and no live session/loop"

key-files:
  modified:
    - crates/rdpilot/src/session.rs

key-decisions:
  - "Implemented Task 1 and Task 2 as a single commit (93f36f0) rather than two atomic per-task commits: both tasks touch the same and only file (session.rs), the plan's files_modified lists exactly one file, and Task 2's crop_to_window helper was placed adjacent to Task 1's new methods during implementation — splitting after the fact would have meant reconstructing an artificial partial diff with no compensating benefit. Documented here as a deviation from the standard one-commit-per-task protocol."
  - "get_window_list/get_process_tree/set_foreground_window/launch_process are all thin wrappers around one new private sensor_request(msg_type, payload, timeout_ms) -> Result<Value> helper, rather than four independent copies of ping()'s five-step shape. This keeps the D-6.4 success/data vs success:false branch, the pending-map timeout cleanup, and the handshake fast-fail check in exactly one place, matching the plan's 'mirrors ping()'s shape' instruction while avoiding quadruplicated logic; ping() itself is untouched (a Pong's payload carries no {success,data|error} envelope, so it does not need the branch)."
  - "screenshot_window() delegates to a free function crop_to_window(shot, window) = shot.crop(window.rect) with no coordinate remap — WindowInfo.rect is already in physical virtual-desktop pixels per the 06-01 wire contract, matching Screenshot::crop's existing coordinate space exactly."
  - "The 06-02-PLAN.md-specified timeout test ('a method whose reply never arrives... returns a transport Error::Dvc (timeout) AND removes the pending entry') was implemented against set_foreground_window (ACTION_TIMEOUT_MS=500ms) rather than an enumeration method (2000ms), to keep the offline test suite fast — the timeout/pending-removal mechanism lives in the one shared sensor_request() helper, so exercising it via one method covers all four."

requirements-completed: []  # Deliberately NOT marked complete here, mirroring 06-01's convention: PERC-01/PERC-02/PERC-04/PROC-01/CAP-02 are end-user-observable capabilities that only become TRUE at the Wave 4 live gate (06-05), not at each intermediate offline plan.

# Metrics
duration: 35min
completed: 2026-07-09
---

# Phase 6 Plan 2: Sensor-Backed Session Methods + Per-Window Screenshot Crop Summary

**Added `get_window_list`/`get_process_tree`/`set_foreground_window`/`launch_process` (all sharing one new `sensor_request()` round-trip helper with D-6.4 semantic-vs-transport error branching and phase-tuned 2000ms/500ms timeouts) plus `screenshot_window` (a pure client-side crop of the framebuffer via a new `crop_to_window` helper, no sensor round trip) — all offline-tested against the existing `test_session_with_sensor` harness, no VM.**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-07-09T14:22Z
- **Completed:** 2026-07-09T14:57Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Four new public `Session` methods (`get_window_list`, `get_process_tree`, `set_foreground_window`, `launch_process`) round-trip through owned SDK types, all built on one new private `sensor_request()` helper that mirrors `ping()`'s five-step shape and adds the D-6.4 `success:true -> data` / `success:false -> Error::SensorRejected` branch that `ping()` itself never needed.
- `launch_process` reads `data.pid` and returns `u32` (fire-and-forget per D-6.2 — no follow-up process-tree polling inside the SDK).
- `screenshot_window(&WindowInfo) -> Result<Screenshot>` wires the existing `Session::screenshot()` capture into a new pure `crop_to_window` helper (`shot.crop(window.rect)`), with zero sensor round trip and zero coordinate remap (D-6.1) — the method's doc comment documents the occlusion/minimized-window limitation verbatim to the plan.
- Two new module-level timeout constants: `ENUMERATION_TIMEOUT_MS = 2000` (WindowList/ProcessTree) and `ACTION_TIMEOUT_MS = 500` (SetForegroundWindow/LaunchProcess, matching `ping()`'s existing bound) — both documented as LIVE-VERIFY tuning targets for the Wave 4 gate.
- 7 new offline unit tests: success/semantic-failure/timeout branches for the sensor-backed methods (using `tokio::spawn` + draining `input_rx` to learn the allocated `req_id`, then fulfilling the registered `oneshot` directly — extending the existing `ping()`-test harness pattern to a full request/reply round trip) and in-bounds/out-of-bounds crop-mapping tests plus one end-to-end `screenshot_window` wiring test.

## Task Commits

Both tasks landed in one commit (see Deviations for why):

1. **Task 1 + Task 2: sensor-backed Session methods + per-window screenshot crop** - `93f36f0` (feat)

## Files Created/Modified
- `crates/rdpilot/src/session.rs` - Added `use crate::perception::{ProcessInfo, WindowInfo};`; `ENUMERATION_TIMEOUT_MS`/`ACTION_TIMEOUT_MS` constants; `crop_to_window()` free helper; `sensor_request()` private helper; `get_window_list`/`get_process_tree`/`set_foreground_window`/`launch_process`/`screenshot_window` public methods; 7 new offline tests

## Decisions Made
See `key-decisions` in frontmatter for the shared-helper design, the single-commit deviation, the D-6.1 no-remap rationale, and the timeout-test method choice.

## Deviations from Plan

### Auto-fixed Issues

None — no bugs, missing functionality, or blocking issues were encountered; the plan's interface (mirroring `ping()`, D-6.4 branching, D-6.1 crop wiring) was already fully specified by 06-01's wire contract and `perception.rs` types.

### Process Deviation (not a Rule 1-4 case, documented for transparency)

**1. Single commit instead of one commit per task**
- **Found during:** Task 2 (both tasks share the plan's single `files_modified` entry, `session.rs`)
- **Reason:** Task 1 (the four sensor-backed methods) and Task 2 (the `screenshot_window`/`crop_to_window` crop wiring) both modify only `crates/rdpilot/src/session.rs`, and Task 2's helper was placed adjacent to Task 1's new methods during implementation. Splitting the already-interleaved edits into two artificial partial commits after the fact would have added no traceability benefit.
- **Files modified:** `crates/rdpilot/src/session.rs`
- **Commit:** `93f36f0` (contains both tasks' full acceptance criteria, verified together — see Verification below)

---

**Total deviations:** 1 (process-only, no code/behavior deviation)
**Impact on plan:** None on functionality or scope — every acceptance criterion for both tasks is met and verified in the single commit.

## Issues Encountered

- **Offline verification target substitution (same as 06-01):** This execution host is native Fedora Linux with no `x86_64-pc-windows-gnu` rustup target or MinGW toolchain installed (confirmed: `rustup target list --installed` only shows `x86_64-unknown-linux-gnu`; `cargo +stable-x86_64-pc-windows-gnu build` fails immediately with a missing-channel error). Verified instead via `cargo +stable-x86_64-unknown-linux-gnu build/test/clippy -p rdpilot --target x86_64-unknown-linux-gnu`, per this crate's continuing lack of any `cfg(windows)`/`windows-rs` code (pure Rust + IronRDP + rustls + serde). **The genuine `x86_64-pc-windows-gnu` build is still not re-confirmed** (carried over as an open item from 06-01) — the next session with access to the pinned dev machine should run `cargo build -p rdpilot` there before the Wave 4 live gate, though risk remains low given the crate's continued absence of Windows-specific code.
- **Parallel-plan concurrency observed:** During this plan's execution, a separate agent working on plan 06-03 (C# sensor side) committed `648b6f6` (`sensor/Envelope.cs`, `EnvelopeJsonContext.cs`, `Program.cs`, `WindowEnumeration.cs`) to the same `develop` branch in this same working directory. This plan's changes are isolated to `crates/rdpilot/src/session.rs` and were committed via `git commit -- crates/rdpilot/src/session.rs` to avoid interfering with 06-03's already-staged/committed C# files. No conflict occurred; `cargo test -p rdpilot` was re-run after 06-03's commit landed and remained fully green (85 passed, 0 failed, 12 live-gate tests correctly ignored).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Plans 03/04 (C# sensor handlers for `WindowList`/`ProcessTree`/`SetForegroundWindow`/`LaunchProcess`) can now be verified against a Rust client side that already round-trips through owned types with the exact D-6.4 `{success,data|error}` branching the wire contract specifies.
- Plan 05 (live gate) can call `get_window_list`/`get_process_tree`/`set_foreground_window`/`launch_process`/`screenshot_window` directly; `ENUMERATION_TIMEOUT_MS`/`ACTION_TIMEOUT_MS` are flagged in-code as live-tuning targets if the gate shows either bound is too tight.
- **Open item carried forward:** genuine `x86_64-pc-windows-gnu` build re-confirmation (see Issues Encountered) — should be closed out opportunistically on the real Windows dev machine before the Wave 4 live gate.

---
*Phase: 06-window-process-perception*
*Completed: 2026-07-09*

## Self-Check: PASSED

All modified files verified present on disk (`crates/rdpilot/src/session.rs`); commit `93f36f0` verified present in `git log`; `SUMMARY.md` verified present on disk.
