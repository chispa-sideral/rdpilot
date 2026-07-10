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
  - "Live VM measurement (SC#2 empirical capture_span) executed 2026-07-10 against a fresh disposable Azure VM (rdpilot-vm, Standard_B2s_v2, westeurope). Both gated world_state tests PASS live: default-options capture_span measured 23.524043ms (23ms); UiaMode::Foreground capture_span measured 73.194179ms (73ms) after one retry of the documented first-RDP-login deploy_and_launch transient (Phase 6/7 finding — self-resolved). Both are >6x under the D-8.2 best-effort 500ms bound. SC#2 is now empirically CLOSED. VM torn down and confirmed absent (`az group exists -n rdpilot-test` → false; `rdpilot-mgmt` persists)."

requirements-completed: [API-01, API-02]

duration: ~25min (offline authoring) + ~35min (2026-07-10 live gate: provision + sensor build/relay + test run + teardown)
completed: 2026-07-09 (offline) / 2026-07-10 (live gate)
---

# Phase 8 Plan 3: Gated live `world_state` capture-span test Summary

**Added two `#[ignore]`'d, `require_target!`-gated live tests exercising `Session::world_state()` (default options and `UiaMode::Foreground`) against a real target, recording the measured `capture_span` for SC#2; both tests were subsequently run live (2026-07-10) against a fresh disposable Azure VM and PASSED, with capture_span measured at 23ms (default options) and 73ms (Foreground UIA) — both well under the D-8.2 best-effort 500ms bound. SC#2 is now empirically CLOSED.**

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

## Live Gate Result (2026-07-10) — SC#2 CLOSED

The deferred live VM run (below, historical) was executed in a follow-up session:

1. **Provisioned** a fresh disposable Azure VM (`infra/manage-env.ps1 up -VmSize Standard_B2s_v2`, westeurope) — the stale `.secrets/connection.json` host (`20.101.90.244`) referenced in the original deferral was confirmed unreachable and NOT reused; a brand-new VM (`rdpilot-vm`, public IP `20.123.147.255`) was deployed clean.
2. **Built** the win-x64 NativeAOT sensor ON the VM via `az vm run-command invoke` (WinRM still unavailable from this Linux host — same Phase 5/6/7 substitution), embedding the sensor source as a base64 tarball directly in the script body (not as a `--parameters` value — a `--parameters`-based attempt failed near-instantly, consistent with an undocumented CLI parameter-size limit; embedding in the script body, the pattern Phase 6/7 already used, worked cleanly). Installed .NET 8 SDK 8.0.422 and VC++ Build Tools fresh (new VM image had neither). `dotnet publish -c Release -r win-x64 -p:PublishAot=true --self-contained` succeeded, zero warnings: `rdpilot-sensor.exe`, 3,037,696 bytes, SHA256 `7a775b990cca3b40e72d4cd8ce910ebfc8e14262dd660089a4e5c62355ec34c2`. Relayed back via a short-lived (3h) account-key Azure Storage blob SAS in a throwaway `relay` container inside the run's own storage account — the locally-downloaded copy's SHA256 is byte-identical to the VM-built copy.
3. **Ran** `RDPILOT_LIVE=1 RDPILOT_SENSOR_EXE=.secrets/sensor-build/rdpilot-sensor.exe cargo test -p rdpilot --target x86_64-unknown-linux-gnu --test live_session -- --ignored --test-threads=1 --nocapture world_state` (RUSTUP_TOOLCHAIN=stable override — this Linux host has no `x86_64-pc-windows-gnu` toolchain installed, and the crate has no `cfg(windows)` code, so the linux-gnu substitute is valid, matching the Phase 6/7 precedent).
   - `world_state_default_options_reports_capture_span`: **PASS**, measured `capture_span = 23.524043ms (23ms)`.
   - `world_state_foreground_uia_matches_focused_window`: **PASS** on retry (first attempt hit `Dvc("request timed out after 2000ms")` while polling `get_window_list` for Notepad — this is the documented first-RDP-login `deploy_and_launch` transient first diagnosed in Phase 7, which self-resolves on a single retry; it did here, no code change needed), measured `capture_span = 73.194179ms (73ms)`.
   - Both measured spans are **more than 6x under** the D-8.2 best-effort 500ms bound — no bound violation to record.
4. **Tore down** the VM (`pwsh infra/manage-env.ps1 down`) and confirmed `az group exists -n rdpilot-test` → `false` (direct `az resource list -g rdpilot-test` independently confirms `ResourceGroupNotFound`); `rdpilot-mgmt` persists as designed.

**SC#2 is now empirically CLOSED.** No code changes were required — this was a pure live-verification run of the Plan 03 offline-authored tests.

### Live VM run: DEFERRED (historical — resolved above; not a Rule 1-4 deviation — an explicitly plan-sanctioned deferral at original authoring time)

- **What was checked:** `.secrets/connection.json` and a previously-published `.secrets/sensor-build/rdpilot-sensor.exe` exist on disk from an earlier session, and `az account show` confirms an active, authenticated Azure CLI session (subscription "Chispa Sideral"). However, a direct TCP probe of `.secrets/connection.json`'s recorded host (`20.101.90.244:3389`) shows the RDP port is **not reachable** — the VM referenced by that stale credentials file is not currently running (most likely deallocated/torn down after a prior phase's live gate, per the project's disposable-VM pattern, T-08-05).
- **Decision:** Per the orchestrator's explicit live-run policy for this plan ("DO NOT attempt a costly live provisioning unless you can confirm the environment supports it end-to-end... If you defer the live run, say so explicitly") and the plan's own acceptance criteria ("Deferral is acceptable if no target is available"), the live VM provisioning (`infra/manage-env.ps1 up`) was **not** attempted in this run. Provisioning a new disposable Azure VM is a cost-incurring, environment-scoped decision appropriately left to the orchestrator/user rather than auto-executed by the plan-level task loop (Rule 4 territory: significant infrastructure action, not a code fix).
- **Impact (at the time):** SC#2's empirical `capture_span` measurement against a real target was not yet recorded. All other success criteria for this plan and phase (SC#1 public-API-only, SC#3 single coordinate space, SC#4 strict-lint compile-clean) were already closed offline by Plans 01–02. **Resolved in the 2026-07-10 live gate above.**

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
- FOUND (2026-07-10 live gate): both `world_state` tests report `ok` under `RDPILOT_LIVE=1 --ignored --nocapture`, with measured `capture_span` lines `23.524043ms (23 ms)` and `73.194179ms (73 ms)` in the captured stdout.
- FOUND: `az group exists -n rdpilot-test` → `false` post-teardown; `az resource list -g rdpilot-test` → `ResourceGroupNotFound`; `az group exists -n rdpilot-mgmt` → `true`.
