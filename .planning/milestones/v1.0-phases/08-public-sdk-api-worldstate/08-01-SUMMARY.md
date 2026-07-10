---
phase: 08-public-sdk-api-worldstate
plan: 01
subsystem: api
tags: [serde, clippy, lint-gates, rust]

# Dependency graph
requires:
  - phase: 06-window-process-perception
    provides: owned WindowInfo/WindowState/ProcessInfo types and *Wire deserialization structs
  - phase: 07-uia-tree-module
    provides: owned UiaElement type and UiaElementWire deserialization struct
provides:
  - "lib.rs inner #![deny(unsafe_code)]/#![deny(clippy::unwrap_used)]/#![deny(clippy::expect_used)] gates — API-01/SC#4 is now compiler-enforced"
  - "Serialize derives on Rect, Screenshot (dims-only, rgba skipped), WindowInfo, WindowState (lowercase), ProcessInfo, UiaElement"
  - "serde round-trip unit tests proving the dims-only Screenshot contract and the lowercase WindowState wire-convention parity"
affects: [08-02-worldstate-plan, 08-03-live-gate-plan]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "lib.rs inner #![deny(...)] attributes for crate-scoped strict lints (never a Cargo.toml [lints] table, which is package-scoped and would break tests/live_session.rs's 116 legitimate .expect() calls)"
    - "#[serde(skip)] on a Serialize-only field to exclude large/sensitive data from JSON output without requiring Deserialize + Default"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/lib.rs
    - crates/rdpilot/src/screenshot.rs
    - crates/rdpilot/src/perception.rs

key-decisions:
  - "Confirmed no Cargo.toml [lints] table was added — inner lib.rs attributes only, per RESEARCH Pitfall 1"
  - "Screenshot does not derive Deserialize — to_png() remains the only path bytes leave the type; #[serde(skip)] on a Serialize-only derive needs no Default bound"
  - "No *Wire struct gained Serialize — D-09 boundary (owned types serde-independent of wire format) stays intact"

requirements-completed: [API-01, API-02]

# Metrics
duration: ~5min
completed: 2026-07-09
---

# Phase 8 Plan 1: Strict Lint Gates + Owned-Type Serialize Summary

**Compiler-enforced API-01/SC#4 via lib.rs inner deny attributes, plus Serialize derives on all five owned SDK types with a dims-only Screenshot contract.**

## Performance

- **Duration:** ~5 min
- **Started:** 2026-07-09T21:27:16Z
- **Completed:** 2026-07-09T21:29:37Z
- **Tasks:** 2/2 completed
- **Files modified:** 3

## Accomplishments
- `lib.rs` now carries `#![deny(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]` as inner attributes — any future `unsafe`/`unwrap`/`expect` in library code is now a compile/lint failure, not a convention that can silently regress.
- `Rect` and `Screenshot` (dims-only, `rgba` field `#[serde(skip)]`) derive `Serialize` in `screenshot.rs`.
- `WindowInfo`, `ProcessInfo`, `UiaElement` derive `Serialize`; `WindowState` derives `Serialize` with `#[serde(rename_all = "lowercase")]`, matching the existing `WindowStateWire` deserialization convention exactly.
- Added 6 new serde round-trip unit tests (2 in `screenshot.rs`, 4 in `perception.rs`) proving the dims-only `Screenshot` contract, all-four-field `Rect` serialization, lowercase `WindowState` rendering, and field-complete serialization of `WindowInfo`/`ProcessInfo`/`UiaElement`.
- No third-party (`image`/`ironrdp`) type and no crate-internal `*Wire` struct gained a `Serialize` impl — the D-09 public/wire boundary is intact.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add crate-level strict lint gates as lib.rs inner attributes** - `c0a6c43` (feat)
2. **Task 2: Derive Serialize on owned types with dims-only Screenshot, plus serde round-trip tests** - `0f20285` (feat)

**Plan metadata:** (this commit, see below)

## Files Created/Modified
- `crates/rdpilot/src/lib.rs` - Added the three inner `#![deny(...)]` lint-gate attributes with an explanatory comment (Pitfall 1 rationale) before the first `mod` declaration.
- `crates/rdpilot/src/screenshot.rs` - `Rect` derives `Serialize`; `Screenshot` derives `Serialize` with `#[serde(skip)]` on `rgba`; 2 new round-trip tests.
- `crates/rdpilot/src/perception.rs` - `WindowInfo`/`ProcessInfo`/`UiaElement` derive `Serialize`; `WindowState` derives `Serialize` + `#[serde(rename_all = "lowercase")]`; 4 new round-trip tests.

## Decisions Made
- Confirmed the plan's mandated constraint held: no `Cargo.toml [lints]` table was added anywhere — the strict gates are `lib.rs` inner attributes only.
- Followed the plan's Pattern 3 guidance exactly: `Screenshot` gets `Serialize` only (no `Deserialize`), so the `#[serde(skip)]` field needs no `Default` bound; `to_png()` remains the sole byte-egress path.
- `WindowState`'s `#[serde(rename_all = "lowercase")]` was applied to match `WindowStateWire`'s existing convention exactly (verified both directions serialize/deserialize to the same lowercase strings).

## Deviations from Plan

None - plan executed exactly as written. The plan's own contingency ("if clippy flags `.expect()` in the new in-crate tests, scope them with an inner `#![allow(clippy::expect_used)]`") was not needed — clippy's default test-context allowance for `unwrap_used`/`expect_used` held, confirmed empirically: `cargo clippy -p rdpilot --lib` exits 0 with the new test module's `.expect(...)` calls present.

## Issues Encountered

The plan's own `<verify>` shell snippet (`grep -qi "error"` against clippy's combined stdout+stderr) produces a false-positive match against a **pre-existing, out-of-scope** warning: `crate::error::Error` (an unused import in `input.rs`, introduced in Phase 5 commit `c077aea`, untouched by this plan) contains the substring "error" case-insensitively. The actual `cargo clippy` process exit code is `0` (verified directly), and no new warnings or errors were introduced by this plan's changes. Per the deviation-rules SCOPE BOUNDARY, this pre-existing warning is out of scope and was not fixed. This is a documentation note about the verify script's grep pattern, not a deviation from the plan's substantive requirements.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- Wave 1 is fully complete: SC#4 (strict-lint compile-time enforcement) and D-8.4 (owned-type `Serialize`) are both structurally in place.
- `08-02-PLAN.md` (WorldState + `world_state()`) is unblocked — it can now build a `WorldState` aggregate over these five already-`Serialize` owned types without any further serde plumbing.
- `08-03-PLAN.md` (live gate, SC#2) is unaffected by this plan and remains unblocked per its own dependency chain.
- No blockers or concerns carried forward from this plan.

---
*Phase: 08-public-sdk-api-worldstate*
*Completed: 2026-07-09*

## Self-Check: PASSED

- FOUND: crates/rdpilot/src/lib.rs
- FOUND: crates/rdpilot/src/screenshot.rs
- FOUND: crates/rdpilot/src/perception.rs
- FOUND: commit c0a6c43 (Task 1)
- FOUND: commit 0f20285 (Task 2)
