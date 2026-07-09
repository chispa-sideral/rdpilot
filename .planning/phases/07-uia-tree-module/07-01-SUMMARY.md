---
phase: 07-uia-tree-module
plan: 01
subsystem: sdk-transport
tags: [rust, serde, uia, wire-protocol, dvc]

# Dependency graph
requires:
  - phase: 06-window-process-perception
    provides: "Session::sensor_request round-trip helper, get_window_list/get_process_tree template, WindowInfo/WindowInfoWire + RectWire pattern, D-6.4 success/degrade contract"
provides:
  - "MsgType::Uia wire enum variant"
  - "Public owned UiaElement type (id/role/name/bbox/enabled/visible/focusable/focused/depth/parent_id)"
  - "Crate-internal UiaElementWire deserialize-only mirror with into_owned()"
  - "control_type_to_role(id) — 41-entry ControlType->role map, unknown->\"Unknown\""
  - "runtime_id_to_string(ids) — deterministic \"-\"-joined RuntimeId string"
  - "Session::get_uia_tree(hwnd) public async round-trip method"
affects: ["07-02-csharp-scaffolding", "07-04-uia-handler", "07-05-live-gate"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Owned/Wire type pair mirrored exactly from WindowInfo/WindowInfoWire (Phase 6 pattern)"
    - "RuntimeId join + ControlType map performed Rust-side (SDK), not sensor-side, for offline testability"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/sensor.rs
    - crates/rdpilot/src/perception.rs
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/lib.rs

key-decisions:
  - "Wire contract ships RAW runtime_id/parent_runtime_id int arrays and RAW control_type int; Rust into_owned() does the D-7.2 join and D-7.3 map (locks the C# UiaElementRecord shape for 07-04)"
  - "get_uia_tree reuses ENUMERATION_TIMEOUT_MS (2000ms) as the transport timeout — SC#3's 500ms sensor-side walk budget is a separate, live-verified constraint, not tightened here"
  - "Built/tested on native x86_64-unknown-linux-gnu (RUSTUP_TOOLCHAIN + --target override) instead of the repo-pinned x86_64-pc-windows-gnu toolchain, which is not installed on this host — substitution valid per plan's own verification note since perception.rs/session.rs/sensor.rs have no cfg(windows) code"

patterns-established:
  - "Owned-public / Wire-deserialize-only type pair with an into_owned() conversion is now the established shape for every sensor-backed perception type (WindowInfo, ProcessInfo, UiaElement)"

requirements-completed: [PERC-03]

# Metrics
duration: ~20min
completed: 2026-07-09
---

# Phase 7 Plan 1: UIA Wire Foundation Summary

**Rust-side MsgType::Uia + owned UiaElement/UiaElementWire pair with the 41-entry ControlType->role map and RuntimeId join, plus Session::get_uia_tree(hwnd), fully offline.**

## Performance

- **Duration:** ~20 min
- **Completed:** 2026-07-09T18:29:08Z
- **Tasks:** 2/2
- **Files modified:** 4

## Accomplishments
- `MsgType::Uia` added to the wire enum, riding the existing generic req_id-keyed fulfilment path with zero dispatch changes.
- Public owned `UiaElement` (D-7.1 field set) plus crate-internal `UiaElementWire` mirroring the `WindowInfo`/`WindowInfoWire` pattern exactly, reusing the existing `RectWire`.
- `control_type_to_role` (full 41-entry table, `_ => "Unknown"`) and `runtime_id_to_string` (`"-"`-joined, deterministic) helpers, invoked from `UiaElementWire::into_owned`.
- `Session::get_uia_tree(hwnd)` round-trips a `Uia` request via the shared `sensor_request` helper, bounded at `ENUMERATION_TIMEOUT_MS`, returning owned `Vec<UiaElement>`.
- Three offline unit tests (wire round-trip / SC#4, RuntimeId determinism / D-7.2, ControlType mapping / D-7.3) plus the full 88-test offline lib suite, all green.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add MsgType::Uia and the UiaElement owned/wire type pair with conversions** - `abcf5c2` (feat)
2. **Task 2: Add Session::get_uia_tree(hwnd) on the existing round-trip helper** - `af606f2` (feat)

_Note: Task 1 had `tdd="true"` behavior tests specified in the plan; both RED and GREEN were authored together (test file + implementation in the same commit) since the plan's test-writing and implementation instructions were combined in one `<action>` block rather than split into separate RED/GREEN task boundaries — no separate `test(...)` commit exists for this plan. Both tests and implementation are present and passing in `abcf5c2`._

## Files Created/Modified
- `crates/rdpilot/src/sensor.rs` - Added `MsgType::Uia` variant and updated doc comments
- `crates/rdpilot/src/perception.rs` - Added `UiaElement`, `UiaElementWire`, `control_type_to_role`, `runtime_id_to_string`, `UiaElementWire::into_owned`, and 3 offline unit tests
- `crates/rdpilot/src/session.rs` - Added `Session::get_uia_tree(hwnd)`, imported `UiaElement`
- `crates/rdpilot/src/lib.rs` - Re-exported `UiaElement` (public); `UiaElementWire` stays crate-internal

## Decisions Made
- Wire contract puts the RuntimeId join / ControlType map in Rust (SDK-side), not C# (sensor-side), per the plan's `<artifacts_produced>` — chosen for offline testability and to keep the AOT sensor binary logic-thin. This locks the shape 07-04's C# handler must produce (raw `runtime_id`/`control_type`/`parent_runtime_id` ints).
- `runtime_id_to_string` uses `"-"` as the join delimiter (Claude's Discretion per D-7.2), matching RESEARCH's recommendation.
- Kept `ENUMERATION_TIMEOUT_MS` (2000ms) as `get_uia_tree`'s transport timeout rather than introducing a tighter bound — the plan explicitly warns not to conflate the transport timeout with SC#3's 500ms sensor-side walk budget (verified live in 07-05).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - dead-code hygiene] Added `#[allow(dead_code)]` to the new UIA helpers/wire types in Task 1's commit**
- **Found during:** Task 1, verifying the build was warning-clean
- **Issue:** `control_type_to_role`, `runtime_id_to_string`, `UiaElementWire`, and `UiaElementWire::into_owned` are `pub(crate)` and unused until Task 2 wires them into `session.rs`, producing `dead_code` warnings on an interim build.
- **Fix:** Added `#[allow(dead_code)] // Consumed by Task 2's Session::get_uia_tree (interface-first).` to each, mirroring the exact precedent already established for `RectWire`/`WindowStateWire`/`WindowInfoWire` in the same file (which carry an equivalent "Consumed by Plan 02's ..." comment from Phase 6).
- **Files modified:** `crates/rdpilot/src/perception.rs`
- **Commit:** `abcf5c2` (part of Task 1 commit)

---

**Total deviations:** 1 auto-fixed (Rule 2, cosmetic/hygiene — no correctness impact)
**Impact on plan:** No scope creep; matches an existing in-repo convention exactly.

## Issues Encountered

**Toolchain substitution required for verification.** The repo pins `stable-x86_64-pc-windows-gnu` via `rust-toolchain.toml` and forces `target = "x86_64-pc-windows-gnu"` via `.cargo/config.toml`, but only the `stable-x86_64-unknown-linux-gnu` toolchain is installed on this host (no `x86_64-pc-windows-gnu` target/toolchain present). Per the plan's own `<verification>` note ("native linux-x64 target substituted for windows-gnu per the Phase 6 precedent — the crate has no `cfg(windows)` code"), all `cargo build`/`cargo test` invocations were run with `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo ... --target x86_64-unknown-linux-gnu`. All 88 offline lib tests pass under this substitution; no `cfg(windows)` code exists in `sensor.rs`/`perception.rs`/`session.rs`/`lib.rs` so this is a valid stand-in, not a scope reduction.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

The wire contract is locked: `MsgType::Uia`, the raw `runtime_id`/`control_type`/`parent_runtime_id`/`name`/`bbox`/`enabled`/`visible`/`focusable`/`focused`/`depth` field shape, and the owned `UiaElement` public type are all in place and offline-tested. 07-04 (the C# `UiaElementRecord` handler) has an exact target to produce; 07-05's live gate can call `Session::get_uia_tree(hwnd)` directly once the sensor-side handler exists. No blockers.

---
*Phase: 07-uia-tree-module*
*Completed: 2026-07-09*

## Self-Check: PASSED

All 4 modified source files and the SUMMARY.md were verified present on disk;
both task commits (`abcf5c2`, `af606f2`) were verified present in `git log`.
