---
phase: 03-input-injection
plan: 01
subsystem: api
tags: [rust, ironrdp-input, input-injection, mouse, keyboard, scancode]

# Dependency graph
requires:
  - phase: 02-rdp-session-framebuffer-core
    provides: "Session handle, owned-SDK-types-only public API rule (D-09), Error enum + CropOutOfBounds precedent"
provides:
  - "Owned public input vocabulary: MouseAction, KeyAction, Button, Key"
  - "MouseAction::coordinates() for the Plan-02 bounds check (SC#4)"
  - "Error::CoordinateOutOfBounds mirroring CropOutOfBounds exactly (D-3.2)"
  - "Pure, offline-tested translation: mouse_operations()/key_operations()/scancode() -> ironrdp_input::Operation batches"
  - "Pitfall 1 guard (wheel |dy|>255 split, sign preserved) and Pitfall 5 guard (MouseMove always precedes WheelRotations)"
affects: [03-02-mouse-session-wiring, 03-03-keyboard-session-wiring, 03-04-live-input-verification]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pure translation functions (mouse_operations/key_operations) kept crate-internal (pub(crate)); only the owned enums cross the public boundary (D-09)"
    - "Outer Vec<Vec<Operation>> batching: each inner Vec is one Database::apply() call; multiple outer batches exist only where D-3.7/D-3.8 require an inter-batch sleep (DoubleClick, Drag) — timing itself is the caller's job (Session, a later plan), never done here"

key-files:
  created:
    - crates/rdpilot/src/input.rs
  modified:
    - crates/rdpilot/src/error.rs
    - crates/rdpilot/src/lib.rs

key-decisions:
  - "Wheel-magnitude split caps each Operation::WheelRotations at 240 (2 full WHEEL_DELTA notches), not 255, for a clean multiple-of-120 remainder — still comfortably inside the as-u8 wire ceiling (Pitfall 1)"
  - "Drag interpolates 5 intermediate MouseMove batches (DRAG_INTERPOLATION_STEPS), each its own timed batch, ending exactly at (to_x, to_y) so the trailing release needs no extra move"
  - "Combo modifier/non-modifier partition uses Vec::partition (order-preserving) rather than positional slicing, so empty/all-modifier/modifier-free Combos are handled generically with no index-panic risk (API-01, T-03-04)"

patterns-established:
  - "TDD RED commits for this crate may be compile-failures (referencing not-yet-defined functions/variants) rather than only assertion failures, when the task is introducing new types/functions from scratch — documented per-task in Task Commits below"

requirements-completed: [INPUT-01, INPUT-02]

# Metrics
duration: ~55min
completed: 2026-07-08
---

# Phase 3 Plan 1: Input Injection Contract Layer Summary

**Owned MouseAction/KeyAction vocabulary plus a pure, offline-unit-tested translation into ironrdp_input::Operation batches, with the Pitfall 1 (wheel-magnitude wraparound) and Pitfall 5 (stale-position scroll) guards regression-tested.**

## Performance

- **Duration:** ~55 min
- **Started:** 2026-07-08T19:25:00Z (approx.)
- **Completed:** 2026-07-08T20:20:15Z
- **Tasks:** 2 (both `tdd="true"`, each executed as RED → GREEN)
- **Files modified:** 3 (1 created, 2 modified)

## Accomplishments
- Owned `MouseAction`/`KeyAction`/`Button`/`Key` enums re-exported at the crate root — no `ironrdp`/`Scancode`/`FastPathInputEvent` type appears in any public signature (D-09)
- The four D-3.1-locked call sites (`Click{x,y,button}`, `Scroll{x,y,dy}`, `Type("h".into())`, `Combo(vec![Ctrl,A])`) compile and construct, unit-tested verbatim
- `Error::CoordinateOutOfBounds` added, mirroring `Error::CropOutOfBounds`'s shape exactly, ready for Plan 02's bounds-check enforcement (SC#4)
- Hand-written IBM PC/AT Scan Code Set 1 table (`scancode()`) covering modifiers, A-Z, Digit0-9, F1-F12, and the extended nav/arrow cluster
- Pure `mouse_operations`/`key_operations` translation: Scroll always emits `MouseMove` before `WheelRotations` in the same batch (Pitfall 5, T-03-03); any `|dy| > 255` is split into multiple <=240-magnitude `WheelRotations` with sign preserved, never reaching the library's silent `as u8` wire-truncation bug (Pitfall 1, T-03-01); `Combo` presses modifiers then key and releases in reverse order (D-3.5); `DoubleClick`/`Drag` batch boundaries are pre-built for Plan 02's inter-batch sleeps (D-3.7, D-3.8)
- All keyboard construction routes through `ironrdp_input::Operation`, never a hand-built `FastPathInputEvent::KeyboardEvent` (Pitfall 2, T-03-02, grep-guarded)
- Degenerate `Combo` inputs (empty / all-modifier / modifier-free) never panic (API-01, T-03-04)

## Task Commits

Each TDD task was committed as a RED (`test`) commit followed by a GREEN (`feat`) commit:

1. **Task 1: Owned input vocabulary + Error::CoordinateOutOfBounds + re-export**
   - `1cd1014` (test) — Button/Key/MouseAction/KeyAction enums + coordinates() + failing tests referencing the not-yet-implemented `Error::coordinate_out_of_bounds` (RED: compile failure)
   - `2f21d7c` (feat) — `Error::CoordinateOutOfBounds` variant/constructor/category + `lib.rs` re-export (GREEN: 24 tests pass)
2. **Task 2: Scancode table + pure MouseAction/KeyAction -> Operation translation**
   - `e5c834c` (test) — scancode/mouse_operations/key_operations regression tests referencing not-yet-implemented functions (RED: 21 compile errors)
   - `be548a9` (feat) — scancode table, mouse_button, wheel-split, mouse_operations, key_operations, combo_operations implementation (GREEN: 36 tests pass)

**Plan metadata:** commit pending (this SUMMARY + STATE/ROADMAP update)

_Note: both tasks used a compile-failure RED (new types/functions referenced by tests before they exist), not an assertion-failure RED — appropriate for foundational scaffolding tasks; see "TDD Gate Compliance" below._

## Files Created/Modified
- `crates/rdpilot/src/input.rs` — new: owned input vocabulary, scancode table, pure Operation-batch translation, 16 unit tests
- `crates/rdpilot/src/error.rs` — added `Error::CoordinateOutOfBounds`, `coordinate_out_of_bounds()` constructor, `category()` arm
- `crates/rdpilot/src/lib.rs` — added `mod input;` and `pub use input::{MouseAction, KeyAction, Button, Key};`

## Decisions Made
- Wheel-magnitude split caps at 240 (not 255) per operation, per the plan's own "cap or split at 2 notches (240)" guidance — keeps outputs on clean 120-unit-notch boundaries while staying inside the 255 wire ceiling
- Drag interpolation uses 5 evenly-spaced intermediate moves ending exactly at `(to_x, to_y)`, so the trailing release batch needs no additional move — the live-tuning of this step count is explicitly deferred to Plan 04 per D-3.8/RESEARCH §4
- `combo_operations` uses `Iterator::partition` (order-preserving stable partition) instead of positional slicing, generalizing correctly to any modifier/non-modifier arrangement without special-casing degenerate inputs

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking issue] No Rust toolchain available in the execution sandbox for the pinned target**
- **Found during:** Task 1 (before any code changes, while establishing a baseline `cargo test` run)
- **Issue:** This Linux sandbox had no `cargo`/`rustc` installed at all, and the repo's `rust-toolchain.toml` pins `stable-x86_64-pc-windows-gnu` with a hard-coded Windows-only MinGW linker path (`C:\Users\marc\scoop\...\gcc.exe`) — neither runnable nor buildable on this Linux host.
- **Fix:** Installed a native Linux `rustup` toolchain (`stable-x86_64-unknown-linux-gnu`, matching the version already fetched by this repo's `Cargo.lock`-pinned dependency graph) and invoked every verification command with `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> -p rdpilot --target x86_64-unknown-linux-gnu` to override both the toolchain-file pin and the `.cargo/config.toml` `[build] target = "x86_64-pc-windows-gnu"` default, without editing either file. The `rdpilot` crate has zero `cfg(windows)`/`cfg(target_os)` gates, so this is a verification-environment-only substitution — no production code, `Cargo.toml`, or toolchain pin was changed.
- **Files modified:** none (environment-only; no repo files touched by this fix)
- **Verification:** Baseline `cargo test -p rdpilot` was green (21 tests) *before* any of this plan's code changes, confirming the substitute toolchain reproduces the same test surface the pinned Windows toolchain would; all subsequent `cargo build`/`cargo test` runs in this plan used the identical override.
- **Committed in:** N/A (no repo change — documented here for traceability; future plans/executors in this same sandbox will need the identical `rustup` install + override until a container image with the pinned toolchain is provisioned)

---

**Total deviations:** 1 auto-fixed (Rule 3, environment-only, no repo files touched)
**Impact on plan:** No effect on code correctness or scope. All of this plan's acceptance criteria were verified against the identical dependency graph (`Cargo.lock` unchanged) the pinned Windows toolchain would use; the crate is platform-agnostic Rust with no target-specific code paths.

## Issues Encountered
`ironrdp_input::Operation` derives only `Debug`/`Clone`, not `PartialEq` (confirmed by reading the pinned 0.6.0 source directly) — so `assert_eq!` could not compare `Vec<Operation>` directly. Resolved by adding a small test-only `op_kind()` discriminant helper plus explicit per-field match-based assertions (scancode `as_u8()`, char payloads) wherever exact op-sequence or payload comparison was needed. No production code was affected.

## User Setup Required
None — no external service configuration required. No `Cargo.toml` change (no new dependencies; `ironrdp-input`/`ironrdp-pdu` were already pinned and unused since Phase 1/2).

## Next Phase Readiness
- Plan 02 (mouse `Session` wiring) can now consume `mouse_operations()`, `Error::coordinate_out_of_bounds()`, and `MouseAction::coordinates()` directly — the bounds-check and `Database`/channel wiring are the only remaining work for SC#2/SC#4's mouse half.
- Plan 03 (keyboard `Session` wiring) can consume `key_operations()` directly for SC#3.
- `scancode`, `mouse_button`, `wheel_rotation_operations`, `mouse_operations`, `click_batch`, `drag_batches`, `interpolate`, `key_operations`, `combo_operations`, and `Error::coordinate_out_of_bounds` are currently exercised only by this plan's unit tests, producing expected (non-blocking) `dead_code`/`unused_imports` warnings on a plain `cargo build` — these disappear once Plans 02/03 add the real `Session::send_mouse`/`send_key` call sites. Not a stub in the sense of missing functionality: this plan's entire purpose (per its `<objective>`) was to build this translation layer as pure, standalone, offline-tested code, consumed by later plans.
- No blockers. `cargo test -p rdpilot` is green (36 offline unit tests, 5 live tests correctly `#[ignore]`'d); no live-VM interaction occurred in this plan (offline-only wave, as scoped).

## TDD Gate Compliance

Both tasks' RED gates were **compile-failure RED** rather than assertion-failure RED: since Task 1/2 introduce brand-new types and functions, the tests referencing them (`Error::coordinate_out_of_bounds`, `scancode`, `mouse_operations`, `key_operations`) could not even compile until the following GREEN commit added the implementation. Verified in git log:

```
be548a9 feat(03-01): implement scancode table + MouseAction/KeyAction translation
e5c834c test(03-01): add failing tests for scancode table + Operation translation
2f21d7c feat(03-01): add Error::CoordinateOutOfBounds and re-export input vocabulary
1cd1014 test(03-01): add failing test for owned input vocabulary + CoordinateOutOfBounds
```

Both `test(...)` commits precede their corresponding `feat(...)` commit — RED then GREEN gate order confirmed for both tasks. No REFACTOR commit was needed (no follow-up cleanup after GREEN).

---
*Phase: 03-input-injection*
*Completed: 2026-07-08*

## Self-Check: PASSED

All claimed files confirmed on disk (`crates/rdpilot/src/input.rs`, `crates/rdpilot/src/error.rs`, `crates/rdpilot/src/lib.rs`, this SUMMARY) and all four task commits (`1cd1014`, `2f21d7c`, `e5c834c`, `be548a9`) confirmed present in `git log --oneline --all`.
