---
phase: 08-public-sdk-api-worldstate
plan: 03
subsystem: testing
tags: [live-test, world_state, uia, rdp, capture_span, gated-test]

requires:
  - phase: 08-public-sdk-api-worldstate (Plan 02)
    provides: "Session::world_state(), WorldStateOptions, UiaMode, WorldState (public API, re-exported, 99/99 offline unit tests, clean clippy under deny gates)"
provides:
  - "Two #[ignore]'d, require_target!-gated live tests in tests/live_session.rs exercising Session::world_state() against a real target for SC#2"
  - "Default-options test asserting screenshot.is_some()/window_list.is_some()/uia.is_none() and eprintln!-recording the measured capture_span (no hard 500ms assertion, D-8.2 best-effort)"
  - "UiaMode::Foreground test exercising the internal window-list fetch + foreground heuristic (Pitfall 3), asserting the single (hwnd, elements) group matches the titled window with minimum z_order"
affects: [phase-08-verification, future-live-gate-runs]

tech-stack:
  added: []
  patterns:
    - "Reused the established require_target!/#[ignore]/deploy_and_launch() gated live-test fixture pattern (Phases 4-7) for a new public-API surface (world_state)"
    - "Best-effort timing: eprintln! the measured span for the record without a hard assert!(< bound) — same pattern as Phase 7's measured 30.36ms report"

key-files:
  created: []
  modified:
    - "crates/rdpilot/tests/live_session.rs — added world_state_default_options_reports_capture_span and world_state_foreground_uia_matches_focused_window; added WorldState/WorldStateOptions/UiaMode to the public-API-only use list"

key-decisions:
  - "Live VM measurement (SC#2 empirical capture_span) is DEFERRED, not executed in this run — no disposable Azure VM is currently provisioned/reachable (see Deviations); the offline-authored gated test is the primary deliverable per the orchestrator's live-run policy and is complete"

requirements-completed: [API-01, API-02]

duration: ~25min
completed: 2026-07-09
---

# Phase 8 Plan 3: Gated live `world_state` capture-span test Summary

**Added two `#[ignore]`'d, `require_target!`-gated live tests exercising `Session::world_state()` (default options and `UiaMode::Foreground`) against a real target, recording the measured `capture_span` for SC#2 without a hard 500ms assertion; the live VM run itself is deferred (no provisioned/reachable target at execution time).**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-07-09T21:xx:xxZ
- **Completed:** 2026-07-09T21:41:16Z
- **Tasks:** 1/1 completed (offline authoring + compile verification; live measurement deferred per plan's explicit deferral allowance)
- **Files modified:** 1

## Accomplishments

- Added `world_state_default_options_reports_capture_span`: launches Notepad, calls `session.world_state(WorldStateOptions::default())`, asserts the SC#2 default shape (`screenshot.is_some()`, `window_list.is_some()`, `uia.is_none()`), asserts `capture_span > Duration::ZERO` (catches a broken measurement only), and `eprintln!`s the measured span in ms for the record — no `assert!(capture_span < 500ms)`.
- Added `world_state_foreground_uia_matches_focused_window`: launches Notepad, independently computes the expected foreground hwnd via the same titled-min-`z_order` heuristic the SDK uses internally (Phase 6 live-diagnosed heuristic — reused rather than re-derived, to avoid a circular assertion), calls `world_state` with `WorldStateOptions { screenshot: true, window_list: true, uia: UiaMode::Foreground }`, asserts exactly one `(hwnd, elements)` group whose hwnd matches the independently-computed expected hwnd and whose element list is non-empty, and `eprintln!`s the measured span.
- Both tests open with `let Some(cfg) = require_target!("...") else { return };` so the default `cargo test -p rdpilot` run stays green with no target present — verified directly (all 22 live tests, including the 2 new ones, report `ignored` and the suite reports `ok. 0 passed; 0 failed; 22 ignored`).
- Confirmed only public `rdpilot` types are imported in the new tests (`WorldState`, `WorldStateOptions`, `UiaMode`, plus the existing `WindowInfo`) — no `ironrdp`/`image`/`rustls` type appears (SC#1 reinforced).
- Confirmed `cargo clippy -p rdpilot --lib --target x86_64-unknown-linux-gnu` still exits 0 with only the two pre-existing, out-of-scope warnings (`input.rs` unused import from Phase 5, `rdpdr_backend.rs` `unnecessary_get_then_check`) — this plan touches only the `tests/` integration file, not `src/`, so no new lint surface was introduced.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add gated live world_state capture-span test(s)** - `7bac3ec` (test)

**Plan metadata:** (this SUMMARY + STATE/ROADMAP/REQUIREMENTS update, committed separately per protocol)

## Files Created/Modified

- `crates/rdpilot/tests/live_session.rs` - Added `WorldState`/`WorldStateOptions`/`UiaMode` to the public-API-only `use rdpilot::{...}` list; appended `world_state_default_options_reports_capture_span` and `world_state_foreground_uia_matches_focused_window`, both `#[ignore]`'d and `require_target!`-gated.

## Verification Performed

- `RUSTUP_TOOLCHAIN=stable cargo test -p rdpilot --target x86_64-unknown-linux-gnu --test live_session` — compiles clean, `test result: ok. 0 passed; 0 failed; 22 ignored; 0 measured; 0 filtered out` (the 2 new tests report `ignored` alongside the 20 pre-existing live tests, confirming the default no-target run stays green).
- `RUSTUP_TOOLCHAIN=stable cargo clippy -p rdpilot --lib --target x86_64-unknown-linux-gnu` — exits 0, only the 2 pre-existing out-of-scope warnings noted above (confirmed identical before/after this plan's change via `git stash`).
- Manually confirmed (via `git stash`) that `cargo clippy -p rdpilot --tests --all-features -- -D warnings` produces ~121 `expect_used`/`unwrap_used` errors on `src/session.rs`'s **existing offline unit test module** identically with and without this plan's changes — this is a pre-existing, documented artifact of applying `--tests`/`-D warnings` at the wrong scope (08-RESEARCH.md explicitly reproduces "116 errors" this way and documents `--lib`-only as the correct gate scope); not caused by, or in scope for, this plan.
- Grepped the modified file to confirm only public `rdpilot` types are imported and no `ironrdp`/`image`/`rustls` symbol appears in the new tests.

## Deviations from Plan

### Live VM run: DEFERRED (not a Rule 1-4 deviation — an explicitly plan-sanctioned deferral)

- **What was checked:** `.secrets/connection.json` and a previously-published `.secrets/sensor-build/rdpilot-sensor.exe` exist on disk from an earlier session, and `az account show` confirms an active, authenticated Azure CLI session (subscription "Chispa Sideral"). However, a direct TCP probe of `.secrets/connection.json`'s recorded host (`20.101.90.244:3389`) shows the RDP port is **not reachable** — the VM referenced by that stale credentials file is not currently running (most likely deallocated/torn down after a prior phase's live gate, per the project's disposable-VM pattern, T-08-05).
- **Decision:** Per the orchestrator's explicit live-run policy for this plan ("DO NOT attempt a costly live provisioning unless you can confirm the environment supports it end-to-end... If you defer the live run, say so explicitly") and the plan's own acceptance criteria ("Deferral is acceptable if no target is available"), the live VM provisioning (`infra/manage-env.ps1 up`) was **not** attempted in this run. Provisioning a new disposable Azure VM is a cost-incurring, environment-scoped decision appropriately left to the orchestrator/user rather than auto-executed by the plan-level task loop (Rule 4 territory: significant infrastructure action, not a code fix).
- **Impact:** SC#2's empirical `capture_span` measurement against a real target is **not yet recorded**. All other success criteria for this plan and phase (SC#1 public-API-only, SC#3 single coordinate space, SC#4 strict-lint compile-clean) are already closed offline by Plans 01–02 and reconfirmed here. The gated test authoring — the primary deliverable per the orchestrator's instructions — is complete and compiles clean.
- **Follow-up:** A future live run should: (1) `pwsh infra/manage-env.ps1 up` (VM size `Standard_B2s_v2`, westeurope) or confirm/refresh `.secrets/connection.json` if a VM is already up, (2) confirm `.secrets/sensor-build/rdpilot-sensor.exe` is current (re-publish if the sensor source changed since the last publish), (3) `RDPILOT_LIVE=1 cargo test -p rdpilot --test live_session -- --include-ignored --test-threads=1 world_state`, (4) record the two measured `capture_span` values here or in a phase verification note, (5) `pwsh infra/manage-env.ps1 down`.

### Auto-fixed Issues

None - the offline authoring, imports, and both new tests were written directly against the existing Plan 01/02 public API and existing live-test fixture conventions with no bugs, missing functionality, or blocking issues encountered.

## Auth Gates

None encountered — no authentication step was reached in this run (the live provisioning step that would require Azure/RDP credentials was deferred per above, not attempted).

## Known Stubs

None. Both new tests are fully wired against the real public `Session::world_state()` API; no hardcoded/mocked data paths were introduced.

## Threat Flags

None. This plan's threat model (T-08-05 disposable-VM cost, T-08-04 hwnd input validation) covers exactly the live-VM lifecycle and hwnd-validation surface these tests exercise — no new network endpoint, auth path, file access pattern, or schema change was introduced beyond what Plans 01–02 already established.

## Self-Check: PASSED

- FOUND: `crates/rdpilot/tests/live_session.rs` contains `world_state_default_options_reports_capture_span` and `world_state_foreground_uia_matches_focused_window`.
- FOUND: commit `7bac3ec` in `git log --oneline`.
- FOUND: `cargo test -p rdpilot --target x86_64-unknown-linux-gnu --test live_session` compiles and reports `22 ignored` (20 pre-existing + 2 new).
