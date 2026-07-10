---
phase: 03-input-injection
plan: 02
subsystem: api
tags: [rust, ironrdp-input, ironrdp-pdu, session, mouse, fast-path, coordinate-bounds]

# Dependency graph
requires:
  - phase: 03-input-injection
    provides: "Plan 01's owned MouseAction/KeyAction/Button/Key vocabulary, Error::CoordinateOutOfBounds, and the pure mouse_operations() translation into ironrdp_input::Operation batches"
provides:
  - "RdpInputEvent::FastPath(Vec<FastPathInputEvent>) variant + session_loop.rs select! arm forwarding to active_stage.process_fastpath_input"
  - "Session fields input_db: Mutex<ironrdp_input::Database> and desktop_size: (u32,u32), captured at connect time before connection_result moves into the loop thread"
  - "Session::desktop_size() -> (u32,u32) public accessor (D-3.2, SC#4)"
  - "Session::check_bounds() private coordinate-bounds enforcement, rejecting with Error::CoordinateOutOfBounds before any PDU is built"
  - "Session::send_mouse(MouseAction) -> Result<()> covering Move/Click(L/R/M)/DoubleClick/Scroll/Drag, with all inter-batch timing (DOUBLE_CLICK_GAP, DRAG_STEP_GAP) on the caller's async context"
affects: [03-03-keyboard-session-wiring, 03-04-live-input-verification]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Mutex<ironrdp_input::Database> lives in Session, locked only for the synchronous apply() call, guard dropped before input_tx.send().await — never held across an .await (Pitfall 3, T-03-07)"
    - "All inter-batch sleeps (DoubleClick ~100ms, Drag ~15ms/step) run on the caller's async context inside send_mouse; session_loop.rs's select! never sleeps and only forwards pre-built FastPathInputEvent batches"
    - "Coordinate bounds check runs before mouse_operations() translation and before any Operation/PDU exists, mirroring the screenshot.rs crop_out_of_bounds precedent"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/session_loop.rs
    - crates/rdpilot/src/session.rs

key-decisions:
  - "desktop_size is a deliberate v1 static capture at connect time (RESEARCH Open Q1 / Assumption A1), not live-updated on a server-driven reactivation resize; documented in code comments as a known, accepted staleness gap for this phase's success criteria"
  - "DOUBLE_CLICK_GAP (100ms) and DRAG_STEP_GAP (15ms) are named constants with doc comments flagging them as LIVE-VERIFY tuning targets to be confirmed/adjusted against the real VM in Plan 04, not final values"
  - "send_mouse decides the inter-batch gap by matching on the MouseAction variant (DoubleClick/Drag get a gap; Move/Click/Scroll get none), rather than inferring timing needs from the translated batch shape alone"

requirements-completed: []

# Metrics
duration: ~40min
completed: 2026-07-08
---

# Phase 3 Plan 2: Mouse Session Wiring Summary

**Session gains a Mutex<ironrdp_input::Database> + connect-time desktop_size and a full send_mouse (move/click/double-click/scroll/drag) implementation, wired through the new RdpInputEvent::FastPath seam with all inter-event timing kept off the session-loop thread.**

## Performance

- **Duration:** ~40 min
- **Started:** 2026-07-08T20:25:00Z (approx.)
- **Completed:** 2026-07-08T21:05:00Z (approx.)
- **Tasks:** 2 (both `tdd="true"`, each executed as RED → GREEN)
- **Files modified:** 2

## Accomplishments
- `RdpInputEvent::FastPath(Vec<FastPathInputEvent>)` added exactly where Phase 2's own doc comment predicted it; the `session_loop.rs` `select!` input arm forwards the batch to `active_stage.process_fastpath_input` unchanged — no construction logic and no `tokio::time::sleep` added to the loop (Pitfall 3, grep-verified)
- `Session` gains `input_db: Mutex<ironrdp_input::Database>` and `desktop_size: (u32, u32)`, the latter captured from `connection_result.desktop_size` immediately after `connect::connect()` returns and *before* `connection_result` moves into the spawned session-loop thread
- `Session::desktop_size()` public accessor and a private `check_bounds()` that rejects any out-of-range `MouseAction` coordinate with `Error::CoordinateOutOfBounds` before any `ironrdp_input::Operation`/PDU is constructed (D-3.2, SC#4 — "enforced", not merely documented)
- `Session::send_mouse(MouseAction) -> Result<()>` implements the full mouse surface (Move, Click L/R/M, DoubleClick, Scroll, Drag): bounds-check first, translate via Plan 01's `mouse_operations`, lock `input_db` only for the synchronous `apply()` call, drop the guard, then `input_tx.send(RdpInputEvent::FastPath(events)).await`
- `DoubleClick`/`Drag` space their batches with `tokio::time::sleep` on `send_mouse`'s own async context (`DOUBLE_CLICK_GAP` ~100ms, `DRAG_STEP_GAP` ~15ms) — never inside the session-loop `select!`; both constants are documented LIVE-VERIFY tuning targets for Plan 04's canonical live run
- A poisoned `input_db` mutex maps to `Error::Session`, never `unwrap`/`expect`/`panic` in non-test code (API-01, grep-verified)
- Offline drain tests assert: an in-range `Click` sends exactly one non-empty `FastPath` batch; `Scroll` sends a batch containing a wheel event (`PointerFlags::VERTICAL_WHEEL`); an out-of-range `Click` (x=9000 on a 1920-wide desktop) is rejected with `Error::CoordinateOutOfBounds` and nothing reaches the channel; `DoubleClick` emits exactly two `FastPath` messages

## Task Commits

Each TDD task was committed as a RED (`test`) commit followed by a GREEN (`feat`) commit:

1. **Task 1: RdpInputEvent::FastPath seam + Mutex<Database>/desktop_size in Session + accessor + bounds check**
   - `2065073` (test) — failing tests referencing not-yet-existing `desktop_size()`/`check_bounds()` and the new `input_db`/`desktop_size` construction fields (RED: compile failure, E0599/E0560)
   - `609578b` (feat) — `RdpInputEvent::FastPath` variant + select! arm in `session_loop.rs`; `input_db`/`desktop_size` fields, `desktop_size()`, `check_bounds()` in `session.rs` (GREEN: 38 tests pass)
2. **Task 2: Session::send_mouse — move/click/double-click/scroll/drag with caller-side timing**
   - `951768f` (test) — failing offline drain tests referencing not-yet-existing `Session::send_mouse` (RED: compile failure, E0599/E0433)
   - `5981a91` (feat) — `send_mouse` implementation + `DOUBLE_CLICK_GAP`/`DRAG_STEP_GAP` constants (GREEN: 42 tests pass)

**Plan metadata:** commit pending (this SUMMARY + STATE/ROADMAP update)

## Files Created/Modified
- `crates/rdpilot/src/session_loop.rs` — `RdpInputEvent` gains `FastPath(Vec<FastPathInputEvent>)`; the `input_rx.recv()` `select!` arm gains a match arm dispatching to `active_stage.process_fastpath_input`; doc comment updated to describe both variants
- `crates/rdpilot/src/session.rs` — `Session` gains `input_db`/`desktop_size` fields; `connect()` captures `desktop_size` before the thread-move; new `desktop_size()`, `check_bounds()`, and `send_mouse()` methods; three existing `#[cfg(test)]` `Session { .. }` literals updated with the two new fields; 6 new unit tests (2 for Task 1, 4 for Task 2)

## Decisions Made
- `desktop_size` is a deliberate v1 static capture, documented in code as such (not live-updated on a server-driven reactivation resize) — matches RESEARCH's Open Q1/Assumption A1 recommendation exactly; Phase 1 fixes the VM resolution so this is sufficient for this phase's success criteria
- `send_mouse` selects the inter-batch gap (`None`/`DOUBLE_CLICK_GAP`/`DRAG_STEP_GAP`) by matching on the `MouseAction` variant directly, rather than trying to infer timing needs from the shape of the translated `Vec<Vec<Operation>>` batches — simpler and keeps the timing decision colocated with the public API surface
- `check_bounds` is private (not part of the public API) — callers only ever see its effect via `send_mouse`'s `Result`

## Deviations from Plan

None — plan executed exactly as written. The TDD RED/GREEN split for both tasks was constructed by scaffolding the tests against the prior (pre-task) implementation state to obtain a genuine compile-failure RED, then restoring the full implementation for GREEN — the same "compile-failure RED" pattern Plan 01 established for foundational scaffolding tasks (new fields/methods/variants that tests reference before they exist).

## Issues Encountered
None. The sandbox's Linux `rustup`/`cargo` toolchain override established in Plan 01 (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> -p rdpilot --target x86_64-unknown-linux-gnu`) was reused verbatim for every verification command in this plan; no repo files (`rust-toolchain.toml`, `.cargo/config.toml`, `Cargo.toml`) were touched.

## User Setup Required
None — no external service configuration required. No `Cargo.toml` change (no new dependencies; `ironrdp-input`/`ironrdp-pdu` were already pinned and already used by Plan 01/`keepalive.rs`).

## Requirements Status

**INPUT-01 is NOT marked complete by this plan.** Per phase scope, INPUT-01 (mouse actions) is only provably satisfied once the live, screenshot-diff-verified suite in Plan 04 (Wave 4) passes against a real VM — this plan delivers the offline-verified mechanism (SC#2's construction half, SC#4's rejection half) but SC#1 (click activates a menu) and the empirical D-3.7/D-3.8 tuning have no offline observable. `requirements-completed` is intentionally left empty in this SUMMARY's frontmatter; `REQUIREMENTS.md`/`ROADMAP.md` requirement checkboxes are NOT updated by this plan. This is plan 2 of 4 in Phase 3 — Plan 03 adds the keyboard half (`send_key`), and Plan 04's canonical live run is the only point at which INPUT-01/INPUT-02 can be honestly marked complete.

## Next Phase Readiness
- Plan 03 (keyboard `Session` wiring) can now follow the identical pattern this plan established for `send_key`/`KeyAction`: bounds-check is not needed for keyboard (no coordinates), but the lock-scope-then-send-then-optionally-sleep shape is directly reusable.
- Plan 04 (gated live suite) can now exercise `Session::send_mouse` end-to-end against the real VM; the `DOUBLE_CLICK_GAP`/`DRAG_STEP_GAP` constants are the two values Plan 04's checkpoint should tune if the live double-click/drag behavior is unreliable at the current 100ms/15ms defaults.
- No blockers. `cargo test -p rdpilot` is green (42 offline unit tests, 5 live tests correctly `#[ignore]`'d); no live-VM interaction occurred in this plan (offline-only wave, as scoped).

## TDD Gate Compliance

Both tasks' RED gates were **compile-failure RED**, consistent with Plan 01's established pattern for foundational scaffolding (new fields/methods/variants the tests reference before they exist). Verified in git log:

```
5981a91 feat(03-02): implement Session::send_mouse (move/click/double-click/scroll/drag)
951768f test(03-02): add failing tests for send_mouse (move/click/double-click/scroll)
609578b feat(03-02): wire RdpInputEvent::FastPath + Mutex<Database>/desktop_size seam
2065073 test(03-02): add failing tests for desktop_size/check_bounds seam
```

Both `test(...)` commits precede their corresponding `feat(...)` commit — RED then GREEN gate order confirmed for both tasks. No REFACTOR commit was needed (no follow-up cleanup after GREEN).

---
*Phase: 03-input-injection*
*Completed: 2026-07-08*

## Self-Check: PASSED

All claimed files confirmed on disk (`crates/rdpilot/src/session_loop.rs`, `crates/rdpilot/src/session.rs`, this SUMMARY) and all four task commits (`2065073`, `609578b`, `951768f`, `5981a91`) confirmed present in `git log --oneline --all`.
