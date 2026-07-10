---
phase: 03-input-injection
plan: 03
subsystem: api
tags: [rust, ironrdp-input, session, keyboard, fast-path, unicode, scancode]

# Dependency graph
requires:
  - phase: 03-input-injection
    provides: "Plan 01's key_operations pure translation (Type -> Unicode press/release pairs, Combo -> scancode down/up with D-3.5 modifier ordering, already unit-tested); Plan 02's Mutex<Database> + RdpInputEvent::FastPath seam and the send_mouse lock/apply/drop/send shape"
provides:
  - "Session::send_key(KeyAction) -> Result<()> covering the Type (Unicode) and Combo (scancode + modifier ordering) paths"
affects: [03-04-live-input-verification]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "send_key mirrors send_mouse's lock-only-for-apply()/drop-before-send shape, but with no coordinate bounds check (keyboard carries no coordinates) and no inter-batch timing (a Type/Combo is always exactly one Operation batch, unlike send_mouse's multi-batch DoubleClick/Drag)"
    - "All keyboard PDU construction is delegated to crate::input::key_operations (Plan 01) -- Session never hand-builds a KeyboardEvent/UnicodeKeyboardEvent, avoiding the 8-bit vs 16-bit KeyboardFlags collision structurally (Pitfall 2)"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/session.rs

key-decisions:
  - "No tracing/log statements were added to send_key at all (matching send_mouse's precedent), which trivially satisfies 'never log the typed content' (Security V5, D-14 parity) -- there is no log call to redact in the first place, rather than adding a redacted-summary log line"
  - "Type/Combo are each translated by key_operations into a single Vec<Operation> (already the case since Plan 01), so send_key applies it in exactly one apply()/send() pair -- no inter-event timing decision was needed, unlike send_mouse's DoubleClick/Drag gap logic"

requirements-completed: []

# Metrics
duration: ~15min
completed: 2026-07-08
---

# Phase 3 Plan 3: Keyboard Session Wiring Summary

**Session gains send_key(KeyAction) -> Result<()>, routing Type(text) through per-character Unicode operations and Combo(keys) through scancode down/up with press-mods -> press-key -> release-key -> release-mods-in-reverse ordering, both delivered as a single FastPath batch through the existing Mutex<Database> + input_tx seam.**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-07-08 (this session)
- **Completed:** 2026-07-08
- **Tasks:** 1 (`tdd="true"`, executed as RED -> GREEN)
- **Files modified:** 1

## Accomplishments
- `Session::send_key(action: KeyAction) -> Result<()>` implemented in `crates/rdpilot/src/session.rs`, mirroring the Plan-02 `send_mouse` lock/apply/drop/send shape exactly, minus the coordinate bounds check (keyboard has no coordinates) and minus inter-event timing (Type/Combo is always a single `apply()` batch, per Plan 01's `key_operations`)
- Routes entirely through `crate::input::key_operations` (Plan 01, already unit-tested for D-3.5 modifier ordering, Unicode per-char emission, and degenerate-Combo safety) and the shared `Mutex<Database>` -- no hand-built `FastPathInputEvent::KeyboardEvent`/`UnicodeKeyboardEvent` anywhere in `session.rs` (Pitfall 2, grep-verified)
- The `input_db` lock is held only for the synchronous `db.apply(ops)` call and dropped before `input_tx.send(...).await` (never held across an `.await`, matching `send_mouse`'s established pattern)
- A poisoned lock or a closed input channel maps to `Error::Session`; no `unwrap`/`expect`/`panic` was added to non-test code (API-01, grep-verified against the `#[cfg(test)]` module boundary)
- Public signature is `send_key(&self, action: KeyAction) -> Result<()>` -- no `ironrdp`/`Operation`/`Scancode`/`FastPathInputEvent` type appears (D-09)
- Four new offline drain tests: `Type("hi")` and `Combo([Ctrl, A])` and `Combo([Alt, F4])` each produce exactly one non-empty `FastPath` message on the channel; `Combo(vec![])` returns `Ok` without panicking

## Task Commits

TDD RED then GREEN:

1. **Task 1: Session::send_key -- Type (Unicode) + Combo (scancode, modifier ordering)**
   - `6439551` (test) -- failing offline tests referencing not-yet-existing `Session::send_key` (RED: E0599 "no method named `send_key`" compile failure -- the same compile-failure RED pattern Plans 01/02 established for foundational scaffolding)
   - `ed15496` (feat) -- `Session::send_key` implementation (GREEN: 46 tests pass, up from 42 baseline)

**Plan metadata:** commit pending (this SUMMARY + STATE/ROADMAP update)

## Files Created/Modified
- `crates/rdpilot/src/session.rs` -- adds `use crate::input::KeyAction` (alongside the existing `MouseAction` import), a new `pub async fn send_key` method, and four new `#[cfg(test)]` unit tests (`send_key_type_sends_nonempty_fastpath_batch`, `send_key_combo_ctrl_a_sends_nonempty_fastpath_batch`, `send_key_combo_alt_f4_sends_nonempty_fastpath_batch`, `send_key_empty_combo_returns_ok_without_panic`); the test module's `use crate::input::{Button, ...}` import gained `Key, KeyAction`

## Decisions Made
- No tracing log statements were added for `send_key` at all -- consistent with `send_mouse`'s precedent (which also does not log). This satisfies "never log the typed content" (Security V5, D-14 redaction parity) trivially: there is no log call in the method to leak content from, rather than adding a redacted-summary log line that would need to be maintained.
- Because `key_operations` (Plan 01) already collapses both `Type` and `Combo` into a single `Vec<Operation>`, `send_key` needed no inter-batch gap logic (unlike `send_mouse`'s `DOUBLE_CLICK_GAP`/`DRAG_STEP_GAP` matching) -- it is exactly one `apply()`/`send()` pair, as scoped by the plan.

## Deviations from Plan

None -- plan executed exactly as written. The TDD RED/GREEN split used the same "compile-failure RED" pattern (tests reference a not-yet-existing method) that Plans 01 and 02 established for foundational scaffolding tasks.

## Issues Encountered
None. The sandbox's Linux `rustup`/`cargo` toolchain override (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> -p rdpilot --target x86_64-unknown-linux-gnu`) established in Plans 01/02 was reused verbatim for every verification command; no repo files (`rust-toolchain.toml`, `.cargo/config.toml`, `Cargo.toml`) were touched. One pre-existing, out-of-scope `clippy`/`rustc` warning (`unused import: crate::error::Error` in `input.rs`, introduced by Plan 01 and unrelated to this task's files/changes) remains untouched per the scope-boundary rule (only auto-fix issues directly caused by the current task's changes).

## User Setup Required
None -- no external service configuration required. No `Cargo.toml` change (no new dependencies; `ironrdp-input` was already pinned and already used by Plans 01/02).

## Requirements Status

**INPUT-02 is NOT marked complete by this plan.** Per phase scope and Plan 02's established precedent, INPUT-02 (keyboard type/combo injection) is only provably satisfied once the live, screenshot-diff-verified suite in Plan 04 (Wave 4) passes against a real VM -- this plan delivers the offline-verified mechanism (both the Type/Unicode path and the Combo/scancode path with correct modifier ordering) but success criterion #3 ("typed text and key combinations received by the remote application") has no offline observable. `requirements-completed` is intentionally left empty in this SUMMARY's frontmatter; `REQUIREMENTS.md`/`ROADMAP.md` requirement checkboxes are NOT updated by this plan. This is plan 3 of 4 in Phase 3 -- Plan 04's canonical live run is the only point at which INPUT-01/INPUT-02 can be honestly marked complete.

## Next Phase Readiness
- Plan 04 (gated live suite) can now exercise `Session::send_key` end-to-end against the real VM for both the Type (typed text renders) and Combo (Ctrl+A selects, Alt+F4 closes) success-criterion checks.
- No blockers. `cargo test -p rdpilot` is green (46 offline unit tests, 5 live tests correctly `#[ignore]`'d); no live-VM interaction occurred in this plan (offline-only wave, as scoped). `cargo build -p rdpilot` exits 0.

## TDD Gate Compliance

The task's RED gate was **compile-failure RED**, consistent with Plans 01/02's established pattern for foundational scaffolding (a method the tests reference before it exists). Verified in git log:

```
ed15496 feat(03-03): implement Session::send_key (Type + Combo keyboard wiring)
6439551 test(03-03): add failing offline tests for Session::send_key
```

The `test(...)` commit precedes the `feat(...)` commit -- RED then GREEN gate order confirmed. No REFACTOR commit was needed (no follow-up cleanup after GREEN).

---
*Phase: 03-input-injection*
*Completed: 2026-07-08*

## Self-Check: PASSED

All claimed files confirmed on disk (`crates/rdpilot/src/session.rs`, this SUMMARY) and both task commits (`6439551`, `ed15496`) confirmed present in `git log --oneline --all`.
