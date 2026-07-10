---
phase: 09-scripted-proof-harness
plan: 03
subsystem: testing
tags: [uia, rdp, 7-zip, proof-harness, composition, offline]

# Dependency graph
requires:
  - phase: 09-scripted-proof-harness (09-01)
    provides: "HUMAN-APPROVED window predicate (exact class_name == \"7-Zip::FM\"), D-9.6 seeding form (full quoted 7zFM.exe path + quoted seed arg), and the recorded depth-1-only TreeScope_Children dump (5 containers, no menu items/toolbar buttons/listview rows)"
  - phase: 09-scripted-proof-harness (09-02)
    provides: "UiaScope::Subtree { max_depth } deeper-walk capability on get_uia_tree, sensor-side UIA_MAX_WALK_DEPTH = 4 safety cap"
provides:
  - "Single shared async run_proof_harness(&Session) -> rdpilot::Result<ProofReport> in crates/rdpilot/tests/support/proof_harness.rs, called by both examples/proof_harness.rs and the gated proof_harness_end_to_end test via #[path]"
  - "SC#2 assertion logic asserting real deeper 7-Zip elements (named MenuItem/Button, or ListItem/DataItem) reachable only via UiaScope::Subtree, scoped to depth > 1 and inside the window rect"
  - "D-9.2 navigation default: click on the deeper \"File\" MenuItem if present, else the first matched deeper element; D-9.5 bounded poll-with-timeout screenshot-diff verify"
  - "examples/proof_harness.rs human-facing v1 demo binary (main() -> ExitCode, stdout PASS/FAIL trace, non-zero exit on failure)"
affects: [09-04-live-gate-plan]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Shared #[path]-included module compiled twice (once per crate root, examples/ and tests/) — self-contained, reaches the SDK only via `use rdpilot::{...}`, never `crate::`, and duplicates its own copies of region_changed/changed_fraction/clamped_rect rather than reaching into live_session.rs's private helpers via super:: (Pitfall 5)"
    - "Logical assertion failures (window not found, no deeper element matched, navigation didn't visibly change the desktop) are recorded as a failed ProofStep and returned inside Ok(ProofReport{passed:false,..}); only a hard rdpilot::Error (transport/sensor failure) propagates as Err — matches the plan's stated error-handling split"

key-files:
  created:
    - crates/rdpilot/tests/support/proof_harness.rs
    - crates/rdpilot/examples/proof_harness.rs
  modified:
    - crates/rdpilot/tests/live_session.rs

key-decisions:
  - "SC2_MAX_DEPTH chosen as 4 (matches the 09-02-recorded sensor-side UIA_MAX_WALK_DEPTH safety cap exactly) — since no live SC#2 dump exists yet at any Subtree depth, requesting the sensor's full budget maximizes the chance the 09-04 live gate reaches all three recorded deeper-element categories (menu items under MenuBar at depth 2, toolbar buttons under ToolBar at depth 2, and the seeded C:\\Program Files listview rows, which may sit deeper under Pane). The wire max_depth never bypasses the sensor's own Math.Min(caller,4) clamp, so this cannot regress the Phase 7 SC#3 latency budget."
  - "SC#2 assertion accepts ANY of the three recorded deeper-element categories (menu item / toolbar button / listview row) rather than requiring all three present — matches the plan's must_haves wording (\"and/or\") and stays defensible without a live dump to pin exact category availability at this depth."
  - "D-9.2 navigation target: prefer the deeper \"File\" MenuItem (the CONTEXT-recommended safe default, menu-open); fall back to the first matched deeper element (a toolbar button or listview row) if \"File\" isn't among the matched set — keeps the harness robust even if the live tree shape differs slightly from the 09-01 spike's screenshot."
  - "A doc-comment literal `crate::Session` (used to explain the Pitfall-5 anti-pattern) had to be reworded to `a crate::-qualified Session` to avoid tripping the plan's own acceptance grep (`! grep -q 'crate::Session'` scans the whole file, comments included) — same class of self-referential gate friction 09-02-SUMMARY already documented for its TreeScope.Subtree wording."

requirements-completed: []

# Metrics
duration: ~55min
completed: 2026-07-10
---

# Phase 9 Plan 3: Shared Proof-Harness Composition + Assertion Layer Summary

**Built the single shared `run_proof_harness` async function (D-9.3) — connect->screenshot->launch 7-Zip (D-9.6 seeding)->find its exact `"7-Zip::FM"` window->a deeper `UiaScope::Subtree` UIA walk asserting REAL 09-01-recorded elements (menu items/toolbar buttons/listview rows, not the 5 depth-1 containers)->navigate (D-9.2 menu-open default)->bounded-poll verify (D-9.5)->`ProofReport` — wired via `#[path]` into both a new `examples/proof_harness.rs` human-facing demo binary and a gated `#[ignore]` `proof_harness_end_to_end` test in `tests/live_session.rs`. Everything compiles and lists offline; no live target was touched.**

## Performance

- **Duration:** ~55 min
- **Completed:** 2026-07-10
- **Tasks:** 3/3 completed
- **Files modified:** 3 (2 created, 1 appended-to)

## Accomplishments

- Created `crates/rdpilot/tests/support/proof_harness.rs`: `pub struct ProofReport { steps: Vec<(&'static str, bool, String)>, passed: bool }` and `pub async fn run_proof_harness(session: &Session) -> rdpilot::Result<ProofReport>` running the 5 recorded steps (`screenshot`, `launch_7zip`, `uia_deeper_walk`, `navigate`, `verify_navigation`), all reached via `use rdpilot::{...}` (never `crate::`), self-contained (no `super::` reach-back) so it compiles identically inside two different crate roots (Pitfall 5).
- `launch_7zip_and_find_window` launches with the exact D-9.6 seeding form (`exe = "\"C:\\Program Files\\7-Zip\\7zFM.exe\""`, `args = Some("\"C:\\Program Files\"")`) and bounded-polls `get_window_list` for an EXACT `class_name == "7-Zip::FM"` match (never a title substring), returning `Ok(None)` — a recordable step failure, not a panic — if the window never appears.
- SC#2's `find_deeper_elements` filters the `UiaScope::Subtree { max_depth: 4 }` result to `depth > 1`, nonzero-area bbox, inside the window rect, AND matching one of the 09-01-recorded deeper categories: a named `MenuItem` (`File`/`Edit`/`View`/`Favorites`/`Tools`/`Help`), a named `Button` (`Add`/`Extract`/`Test`/`Copy`/`Move`/`Delete`/`Info`), or a `ListItem`/`DataItem` (listview row) — explicitly excluding the 5 depth-1 containers (`ToolBar`/`Pane`/`TitleBar`/`MenuBar`/`Window`) the 09-01 dump proved were all `TreeScope_Children` returns.
- SC#3 clicks the D-9.2 default target (`"File"` MenuItem if matched, else the first matched deeper element) via `MouseAction::Click`, then a bounded (15-attempt, 300ms-interval) poll loop with `eprintln!` progress crops a region around the target and calls a self-contained `region_changed`/`changed_fraction`/`clamped_rect` (duplicated from `live_session.rs`'s idiom, not `super::`-referenced) to confirm the click visibly changed the desktop — never a lone fixed sleep (D-9.5).
- Created `crates/rdpilot/examples/proof_harness.rs`: `main() -> ExitCode`, current-thread tokio runtime, modeled on `examples/screenshot.rs`; loads `.secrets/connection.json`, resolves the published sensor exe locally (mirrors `tests/common::sensor_exe_path`'s `RDPILOT_SENSOR_EXE` override + default path, since `examples/` cannot `mod common;`), `connect`->`deploy_and_launch`->`proof_harness::run_proof_harness`, prints each step's PASS/FAIL + detail followed by a single `PROOF: PASS`/`PROOF: FAIL` line, maps `report.passed` to the process exit code, and carries `examples/screenshot.rs`'s password-redaction discipline verbatim.
- Appended `proof_harness_end_to_end` to `tests/live_session.rs`, mirroring `uia_tree_returns_populated_elements`'s exact preamble (sensor-exe existence assertion, `sensor_binary_path`, `connect`->`deploy_and_launch`), then calling the shared harness and asserting `report.passed`; added the `#[path = "support/proof_harness.rs"] mod proof_harness;` decl next to the existing `mod common;`. None of the 5 existing `UiaScope::Children` call sites 09-02 migrated were touched.

## Task Commits

Each task was committed atomically:

1. **Task 1: Create the shared harness module** - `102f1cc` (feat)
2. **Task 2: Create examples/proof_harness.rs** - `1e8190c` (feat)
3. **Task 3: Append the gated proof_harness_end_to_end live test** - `e69bc2d` (test)

**Plan metadata:** (this SUMMARY + STATE/ROADMAP update, committed separately per protocol)

## Files Created/Modified

- `crates/rdpilot/tests/support/proof_harness.rs` - Created: `ProofReport`, `run_proof_harness`, `launch_7zip_and_find_window`, SC#2's `find_deeper_elements`/`is_recorded_deeper_element`/`rect_contains`, SC#3's `pick_navigation_target`/`bbox_center`, and self-contained `region_changed`/`changed_fraction`/`clamped_rect` copies
- `crates/rdpilot/examples/proof_harness.rs` - Created: `main`, `run`, local `load_config`/`sensor_exe_path`, `#[path]` decl into the shared module
- `crates/rdpilot/tests/live_session.rs` - Appended `proof_harness_end_to_end` (`#[ignore]` + `require_target!`-gated) and the `#[path = "support/proof_harness.rs"] mod proof_harness;` decl; no existing test modified

## Decisions Made

- `SC2_MAX_DEPTH = 4`, the recorded sensor-side `UIA_MAX_WALK_DEPTH` cap itself — requests the full budget since no live Subtree dump exists yet to pin a tighter sufficient value; the 09-04 live gate is where this gets empirically validated (or live-tuned) against the real tree.
- SC#2 passes on ANY recorded deeper-element category being present (menu item OR toolbar button OR listview row), matching the plan's "and/or" wording rather than requiring all three.
- D-9.2's navigation target prefers `"File"` MenuItem, falling back to the first matched deeper element for robustness against live-tree variance.
- A doc-comment literal `crate::Session` tripped the plan's own `! grep -q 'crate::Session'` acceptance gate (which scans the whole file, not just code) and was reworded — a documentation-wording adjustment only, not a behavior change (same class of friction 09-02-SUMMARY records for its `TreeScope.Subtree` comment wording).

## Deviations from Plan

None — plan executed exactly as written. One phrasing adjustment was needed to satisfy the plan's own acceptance gate (see key-decisions above: the doc comment's literal `crate::Session` string, used only to explain the Pitfall-5 anti-pattern, had to be reworded since the acceptance gate's `! grep -q 'crate::Session'` scans comments too). This is a documentation-wording adjustment, not a behavior or scope deviation, and required no deviation-rule invocation.

## Issues Encountered

- `cargo clippy -p rdpilot --examples --tests -- -D warnings` fails with 138 pre-existing `clippy::expect_used` errors, ALL inside `crates/rdpilot/src/session.rs`'s own `#[cfg(test)]` unit-test module (the lib crate's `#![deny(clippy::expect_used)]` inner attribute apparently applies clippy-wide to the whole compilation unit including its inline test module, not just non-test code). Confirmed via `grep -c "session.rs"` = 74 occurrences vs `grep -c "proof_harness"` = 0 — none of these errors touch any file this plan created or modified. This plan's `<verification>` section does not name a clippy gate (only `cargo build --examples --tests`, `cargo test -- --list`, and the specific grep checks, all of which pass clean); per the Scope Boundary rule this pre-existing condition is out of scope for this plan and was not fixed.

## User Setup Required

None - no external service configuration required (all verification is offline: `cargo build`/`cargo test`/`cargo clippy` via the established Linux cross-host substitution `RUSTUP_TOOLCHAIN=stable cargo ... --target x86_64-unknown-linux-gnu`).

## Next Phase Readiness

- **Ready for 09-04 (the live gate):** the shared harness, the example binary, and the gated test all compile offline and are wired together via `#[path]`. The 09-04 plan needs to: rebuild the sensor (to pick up 09-02's `UiaScope::Subtree` deeper-walk support — the Phase 8 cached sensor binary predates it and will still return children-only), provision a disposable VM, run `RDPILOT_LIVE=1 cargo test -p rdpilot --test live_session -- --include-ignored --test-threads=1 proof_harness_end_to_end` (or the `examples/proof_harness` binary), and — critically — empirically validate/tune `SC2_MAX_DEPTH` and the SC#2 deeper-element category matching against the REAL live tree shape, since no live Subtree dump exists yet at any depth.
- **No blockers.** All offline verification is green (`cargo build -p rdpilot --examples --tests`, `cargo test -p rdpilot --test live_session -- --list` shows `proof_harness_end_to_end`, `cargo test -p rdpilot --lib` — 103 passed, 0 failed). No live target was needed or provisioned for this plan.
- **PROOF-01 remains "Pending"** in REQUIREMENTS.md — this plan is `requirements: [PROOF-01]` per frontmatter but the actual pass/fail loop is unverified against a live target until 09-04's live gate closes it; `requirements-completed: []` here deliberately, consistent with 09-01/09-02's same non-completion.

## Known Stubs

None. `SC2_MAX_DEPTH` and the SC#2 element-category matching are explicit, documented, live-tunable choices (not stubs) — flagged above under "Next Phase Readiness" as the specific things 09-04 must empirically validate.

## Threat Flags

None beyond what this plan's own `<threat_model>` (T-09-04, T-09-05, T-09-06, T-09-SC) already anticipated and mitigated in the implementation — no new network endpoint, auth path, or schema change outside that register. `examples/proof_harness.rs` carries `examples/screenshot.rs`'s password-redaction discipline verbatim (T-09-04); the shared harness consumes only re-exported owned SDK types incl. `UiaScope` and adds no new public API surface (T-09-05); the D-9.5 bounded poll is grep-confirmed non-lone-sleep (T-09-06); zero new dependencies (T-09-SC).

## Self-Check: PASSED

- FOUND: commit `102f1cc` in `git log --oneline`.
- FOUND: commit `1e8190c` in `git log --oneline`.
- FOUND: commit `e69bc2d` in `git log --oneline`.
- CONFIRMED: `crates/rdpilot/tests/support/proof_harness.rs` exists on disk.
- CONFIRMED: `crates/rdpilot/examples/proof_harness.rs` exists on disk.
- CONFIRMED: `grep -q 'pub async fn run_proof_harness' crates/rdpilot/tests/support/proof_harness.rs` succeeds.
- CONFIRMED: `grep -q 'use rdpilot::' ...` succeeds and `! grep -q 'crate::Session' ...` succeeds.
- CONFIRMED: `grep -q 'UiaScope::Subtree' ...` and `grep -q '7-Zip::FM' ...` both succeed.
- CONFIRMED: `cargo build -p rdpilot --examples --tests` (Linux cross-host target) compiles clean (pre-existing unrelated `input.rs` unused-import warning, out of scope).
- CONFIRMED: `cargo test -p rdpilot --test live_session -- --list` lists `proof_harness_end_to_end` (23 tests total, up from 22).
- CONFIRMED: `cargo test -p rdpilot --lib` — 103 passed, 0 failed (unchanged from 09-02).
- CONFIRMED: all 5 existing `UiaScope::Children` call sites in `live_session.rs` are untouched (`grep -c` = 5).

---
*Phase: 09-scripted-proof-harness*
*Completed: 2026-07-10*
