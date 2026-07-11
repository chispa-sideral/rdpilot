---
phase: 12-session-daemon
plan: 05
subsystem: daemon
tags: [rust, serde_json, atomic-write, crash-recovery, reconciliation, orphan-detection]

# Dependency graph
requires:
  - phase: 12-session-daemon (12-02, 12-03)
    provides: "seams::ReconciliationSink trait + SessionEntry::Orphaned variant (12-02); registry::Registry::seed_orphan + close()'s Orphaned-reclaim arm (12-03)"
provides:
  - "reconcile::ReconciliationRecord (id/host/connected_since, credential-free)"
  - "reconcile::JsonReconciliationSink implementing seams::ReconciliationSink with atomic (temp-write + rename) whole-file rewrite"
  - "reconcile::scan_orphans(path) -- startup read, empty on absent/corrupt file"
  - "reconcile::seed_into(records, registry) -- startup bridge into Registry::seed_orphan"
  - "tests/crash_restart_reconcile.rs -- SC#5 [BLOCKING] offline crash-restart-surfaces-orphan proof"
affects: [12-06-session-daemon-server-assembly, 12-07-session-daemon-live-gate]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Atomic disk-state rewrite: sibling temp file + fs::rename over the target (never a direct in-place write), so a kill -9 mid-write cannot produce a torn file"
    - "Best-effort-and-log disk I/O in a synchronous, infallible-signature trait (ReconciliationSink): a failed write degrades to a missed-orphan visibility gap, never a daemon panic"
    - "No tempfile crate dependency for test-only unique paths: std::env::temp_dir() + process id + a monotonic AtomicU64 counter, mirroring registry.rs's own generate_auto_id no-new-dependency convention"

key-files:
  created:
    - crates/rdpilot-daemon/src/reconcile.rs
    - crates/rdpilot-daemon/tests/crash_restart_reconcile.rs
  modified:
    - crates/rdpilot-daemon/src/lib.rs

key-decisions:
  - "reconcile.rs's module-level #[allow(dead_code)] mirrors registry.rs's own justification: production wiring (Plan 12-06's server run()) hasn't landed yet, this plan's own tests exercise every item"
  - "lib.rs gained one additive pub use line (outside the plan's declared files_modified) so the crash-restart integration test -- a separate compilation unit -- can reach JsonReconciliationSink/ReconciliationRecord/scan_orphans/seed_into; reconcile's mod declaration itself stays private, matching the existing registry::Registry / seams::{...} re-export pattern"

patterns-established:
  - "Pattern: crash-restart proof shape (kill -9 surrogate -> restart -> surface -> explicit reconcile) -- open a session through Registry-A on a shared disk sink path, drop Registry-A WITHOUT calling close() (models an unclean process kill), construct a fresh Registry-B on the SAME path, run scan_orphans + seed_into, assert non-empty Orphaned list, then assert an explicit close() both de-lists it and clears the disk record"

requirements-completed: [DAEMON-04]

# Metrics
duration: ~20min
completed: 2026-07-11
---

# Phase 12 Plan 5: Crash-Restart Orphan Reconciliation Summary

**JSON-disk `ReconciliationSink` (atomic temp-write+rename) plus a startup `scan_orphans`/`seed_into` bridge, proven offline via a kill-9-then-restart integration test that surfaces the prior session as `SessionLifecycle::Orphaned` and reconciles it only on explicit `close()`**

## Performance

- **Duration:** ~20 min
- **Tasks:** 2 completed (both TDD: RED then GREEN)
- **Files modified:** 3 (2 created, 1 additively modified)

## Accomplishments

- `reconcile::JsonReconciliationSink` implements `seams::ReconciliationSink`: `record_open` upserts, `record_closed` removes a record keyed by session id, via a whole-file `Vec<ReconciliationRecord>` rewrite
- Every rewrite is atomic: a sibling temp file is written then `fs::rename`d over the target — a crash mid-write leaves either the old or the new complete file, never a torn one (T-12-15)
- `reconcile::scan_orphans` / internal `load_records` return `Vec::new()` (logged, not panicked) for an absent or corrupt/unparseable state file — a garbage file can never block daemon startup (T-12-16)
- `reconcile::seed_into` bridges scanned records into `Registry::seed_orphan`, surfacing each as `SessionEntry::Orphaned` — never auto-torn-down (T-12-17, D-31)
- `tests/crash_restart_reconcile.rs` proves the full offline loop (SC#5 [BLOCKING] offline portion): Registry-A opens a session (disk record written) → Registry-A is dropped WITHOUT `close()` (kill -9 surrogate — no `record_closed` ever runs) → a fresh Registry-B on the same sink path runs `scan_orphans` + `seed_into` → `Registry-B.list()` is NOT empty and shows the session as `Orphaned` (the exact DAEMON-04 failure mode — silent forgetting — is asserted against) → `Registry-B.close(&id)` explicitly reconciles it, removing both the registry entry and the disk record
- 10 inline unit tests in `reconcile.rs` cover open-then-scan, open-then-close-then-scan, absent-file safety, garbage-file safety, two-id upsert, same-id upsert-not-duplicate, atomic-write hygiene (no leftover `.tmp-` sibling), and `seed_into`'s registry bridge (including a malformed-id skip, never a panic)

## Task Commits

Each task was committed atomically, with a genuine RED-then-GREEN split for both TDD tasks:

1. **Task 1: JSON reconciliation sink + startup orphan scan**
   - `30c6caa` — test(12-05): add failing tests for JSON reconciliation sink (RED — sink stubbed as no-ops, 3/10 tests fail on assertion, not compile error)
   - `dda2941` — feat(12-05): implement JSON reconciliation sink + startup orphan scan (GREEN — all 10 tests pass)
2. **Task 2: Crash → restart → orphan surfaced in list — SC#5 [BLOCKING] offline**
   - `deb3783` — test(12-05): add failing crash-restart orphan reconciliation test (RED — fails to COMPILE: `reconcile`'s public items were not yet reachable from an external integration-test crate)
   - `2dba09d` — feat(12-05): export reconcile module items so the crash-restart integration test can reach them (GREEN — one additive `pub use` line in `lib.rs`; test compiles and passes)

_TDD gate compliance: both tasks show a genuine failing state before the corresponding fix — Task 1 via assertion failures, Task 2 via a compile failure (E0432/E0425, missing exports) — followed by a passing GREEN commit._

## Files Created/Modified

- `crates/rdpilot-daemon/src/reconcile.rs` — `ReconciliationRecord`, `JsonReconciliationSink` (+ `ReconciliationSink` impl), `scan_orphans`, `seed_into`, atomic temp-write+rename helpers, 10 inline unit tests
- `crates/rdpilot-daemon/tests/crash_restart_reconcile.rs` — the offline SC#5 [BLOCKING] crash-restart-surfaces-orphan integration test
- `crates/rdpilot-daemon/src/lib.rs` — one additive `pub use reconcile::{JsonReconciliationSink, ReconciliationRecord, scan_orphans, seed_into};` line (deviation, see below)

## Decisions Made

- Reused `directories::BaseDirs::cache_dir()` (already a `rdpilot-daemon` dependency) rather than adding any new crate, matching the RESEARCH's exact recommended path and `rdpilot-config::paths::config_file_path`'s established `BaseDirs` pattern.
- No `tempfile` crate dependency: test-unique paths are `std::env::temp_dir()` + process id + a monotonic `AtomicU64` suffix, mirroring `registry.rs`'s own `generate_auto_id` "no new dependency for a solved-enough problem" convention.
- `record_open`/`record_closed` swallow-and-log I/O errors rather than propagating (the `ReconciliationSink` trait's methods are synchronous and infallible in signature) — documented in a doc comment as an intentional best-effort tradeoff: the worst case is a missed orphan surface on a future crash (a visibility gap), never a data-destroying action or a daemon panic.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `lib.rs` needed one additive `pub use` line to export `reconcile`'s public items**
- **Found during:** Task 2 (`crash_restart_reconcile.rs`)
- **Issue:** `reconcile.rs`'s public items (`JsonReconciliationSink`, `ReconciliationRecord`, `scan_orphans`, `seed_into`) were reachable only from within the `rdpilot-daemon` crate itself — the `mod reconcile;` declaration in `lib.rs` is private, and there was no re-export. `tests/crash_restart_reconcile.rs` is a separate compilation unit (an integration test) and could not reach these items, so the RED test failed to *compile* (E0432 `unresolved import`, E0425 `cannot find function`) rather than merely failing an assertion.
- **Fix:** Added a single additive line to `lib.rs`: `pub use reconcile::{JsonReconciliationSink, ReconciliationRecord, scan_orphans, seed_into};`, matching the exact established pattern already used for `pub use registry::Registry;` and `pub use seams::{DaemonError, ManagedSession, ReconciliationSink, SessionConnector, SessionEntry};`. The `mod reconcile;` declaration itself stays private (unchanged), consistent with `seams`/`registry`'s own private-mod-plus-pub-use shape.
- **Files modified:** `crates/rdpilot-daemon/src/lib.rs` (outside this plan's declared `files_modified: [reconcile.rs, tests/crash_restart_reconcile.rs]`, but a single-line additive export with no behavioral change to anything else — does not touch code owned by the concurrently-running Plan 12-04, whose declared surface is `ipc/{mod,unix,framing}.rs`, `dispatch.rs`, `tests/ipc_security.rs`)
- **Verification:** `cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu --test crash_restart_reconcile` — 1/1 passed
- **Committed in:** `2dba09d`

---

**Total deviations:** 1 auto-fixed (1 blocking — necessary crate-root export, additive-only)
**Impact on plan:** Required for Task 2 to compile at all; no scope creep — the change is a single new `pub use` line reusing an existing, already-established re-export pattern.

## Issues Encountered

- **Concurrent Plan 12-04 execution on shared files:** While this plan ran, another executor concurrently modified `crates/rdpilot-daemon/src/seams.rs` and `crates/rdpilot-daemon/src/registry.rs` (adding `connected_since_wall`/`last_activity_wall` wall-clock fields to `SessionEntry::Live` for SESSION-03's `list` wiring). Neither file is in this plan's scope and neither was touched by this plan's work. A final full-crate `cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu` (run after 12-04's in-flight changes had landed) confirms all 46 lib unit tests + 4 integration-test binaries (including this plan's 10 `reconcile::` tests and the 1 `crash_restart_reconcile` test) pass together with zero conflicts.
- **Pre-existing clippy findings, out of scope:** `cargo clippy -p rdpilot-daemon --all-targets` reports 16 pre-existing `clippy::expect_used` errors on the `develop` branch BEFORE this plan's changes (confirmed via `git stash`), all inside `#[cfg(test)]` modules across `registry.rs`/`seams.rs`/`tests/*` — the crate's `#![deny(clippy::expect_used)]` is already violated by established test-code convention throughout the codebase. This plan's own inline/integration tests follow the exact same established convention (`.expect(...)` in test-only code), adding 5 more instances of the same pre-existing, out-of-scope pattern — not a regression, and not fixed here per the SCOPE BOUNDARY rule (pre-existing conditions in files/patterns not introduced by this plan's task are out of scope). `cargo build`/`cargo test` (this plan's actual acceptance criteria) are both clean with only pre-existing, unrelated warnings (an unused import in `crates/rdpilot/src/input.rs`, unused `ipc/framing.rs` items awaiting Plan 12-04's wiring).
- **Toolchain substitution (per orchestrator instruction):** Built/tested fully offline on the native Linux substitute target (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --target x86_64-unknown-linux-gnu`), not the repo-pinned `x86_64-pc-windows-gnu` toolchain (not installed on this host, and no Azure VM available this session). Valid substitution: `reconcile.rs` and `crash_restart_reconcile.rs` have no `cfg(windows)` code — pure `std::fs`/`serde_json` disk I/O and in-memory registry logic, identical to the pattern already used by 12-01/02/03's own offline verification.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- **DAEMON-04's offline mechanics are proven and ready for Plan 12-06 to wire up:** `reconcile::scan_orphans(&path)` followed by `reconcile::seed_into(records, &registry)` is the exact two-call sequence Plan 12-06's `server::run()` startup path needs to call once, before accepting any client connections.
- **Deliberately NOT proven here (by design, per this plan's own binding constraints):** that the remote Windows session surfaced as `Orphaned` is genuinely STILL LIVE server-side (vs. having actually logged off between the crash and the restart) — that claim requires a real RDP target and is explicitly deferred to Plan 12-07's live gate, which also owns DAEMON-02's Windows-side explicit-DACL verification.
- No blockers for Plan 12-06 (server assembly / idle reaper / auto-start) or Plan 12-07 (live gate).

## Self-Check: PASSED

- FOUND: `crates/rdpilot-daemon/src/reconcile.rs`
- FOUND: `crates/rdpilot-daemon/tests/crash_restart_reconcile.rs`
- FOUND: `crates/rdpilot-daemon/src/lib.rs`
- FOUND: `.planning/phases/12-session-daemon/12-05-SUMMARY.md`
- FOUND commit `30c6caa` (test RED — reconciliation sink)
- FOUND commit `dda2941` (feat GREEN — reconciliation sink)
- FOUND commit `deb3783` (test RED — crash-restart integration test)
- FOUND commit `2dba09d` (feat GREEN — lib.rs export)

---
*Phase: 12-session-daemon*
*Completed: 2026-07-11*
