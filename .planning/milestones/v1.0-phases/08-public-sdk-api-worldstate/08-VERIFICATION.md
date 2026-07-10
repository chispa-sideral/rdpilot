---
phase: 08-public-sdk-api-worldstate
verified: 2026-07-09T22:15:00Z
status: human_needed
score: 4/4 must-haves verified (offline); 1 empirical measurement deferred per D-8.2
overrides_applied: 0
gaps: []
deferred: []
human_verification:
  - test: "Run the two gated live_session.rs tests (world_state_default_options_reports_capture_span, world_state_foreground_uia_matches_focused_window) against a provisioned Azure Windows target and record the measured capture_span"
    expected: "Both tests pass; capture_span is recorded (best-effort, no hard 500ms gate per D-8.2) — ideally under or near 500ms for a normal desktop"
    why_human: "Requires a live, reachable RDP target (Azure VM) and a published rdpilot-sensor.exe; no target is currently provisioned/reachable (verified: connection.json host 20.101.90.244:3389 unreachable). This is an environment/cost-incurring action outside static code verification."
---

# Phase 8: Public SDK API + WorldState Verification Report

**Phase Goal:** A clean typed `Session` struct hides all IronRDP internals and sensor protocol details, and a `WorldState` snapshot combines framebuffer screenshot, window list, and optional UIA tree in one timestamped structure with a single enforced coordinate space.
**Verified:** 2026-07-09T22:15:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (ROADMAP Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | A consumer can drive a full read/inspect workflow using only the public `Session` API without importing any IronRDP types or sensor protocol details | ✓ VERIFIED | `lib.rs` re-export list is exactly: `ConnectionConfig, Error, Result, Button, Key, KeyAction, MouseAction, ProcessInfo, UiaElement, WindowInfo, WindowState, Rect, Screenshot, Session, UiaMode, WorldState, WorldStateOptions`. `connect`/`framebuffer`/`keepalive`/`session_loop`/`sensor`/`rdpdr_backend`/`worldstate` are private `mod`s. Grepped all `pub fn`/`pub async fn` signatures in `session.rs` and `worldstate.rs` for `ironrdp|DecodedImage|image::|rustls` — zero matches. `worldstate.rs` module doc explicitly states "no ironrdp, image, or sensor-wire type ever crosses this module's public surface" and this was independently confirmed by direct grep, not merely trusted. |
| 2 | `Session::world_state()` returns a `WorldState` struct containing a screenshot, window list, and optional UIA tree captured within 500 ms of each other | ✓ VERIFIED (offline structure) / ? PENDING (empirical live measurement) | `session.rs:688` implements `pub async fn world_state(&self, opts: WorldStateOptions) -> Result<WorldState>`, sequencing `screenshot()` → at-most-one `get_window_list()` → per-mode `get_uia_tree()` calls, measuring `capture_span` via `Instant::now()`/`.elapsed()` and stamping `timestamp: SystemTime::now()`. 5 offline unit tests (`session.rs` `mod tests`) exercise all `UiaMode` variants plus error propagation — all pass (see below). D-8.2 makes the 500ms bound explicitly best-effort/non-hard-fail (documented on the `capture_span` field and in the method doc), consistent with prior-phase precedent (Phase 7 SC#3 "measured 30.36ms" live-only pattern). Two gated live tests exist in `tests/live_session.rs` (`world_state_default_options_reports_capture_span`, `world_state_foreground_uia_matches_focused_window`), correctly `#[ignore]`'d + `require_target!`-gated, asserting the SC#2 default shape and recording (not hard-asserting) `capture_span` via `eprintln!`. Confirmed via direct read of both test bodies — no `assert!(capture_span < 500ms)` exists, matching the SUMMARY's claim exactly. **The live VM run itself was not executed** (no reachable Azure target at verification time — same state SUMMARY 08-03 reported). The empirical capture_span number remains unrecorded. |
| 3 | All coordinate values in `WorldState` (screenshot dimensions, window rects, UIA bounding boxes, mouse input targets) are in the same virtual-desktop pixel space with no silent scaling | ✓ VERIFIED | `worldstate.rs` `WorldState` struct fields are `screenshot: Option<Screenshot>`, `window_list: Option<Vec<WindowInfo>>`, `uia: Option<Vec<(u64, Vec<UiaElement>)>>` — all reusing the exact `crate::Screenshot`/`crate::WindowInfo`/`crate::UiaElement` types already used elsewhere in the SDK (`session.rs`'s `desktop_size()`/`check_bounds()` share the same `crate::Rect`). No new coordinate/scale type introduced anywhere in `worldstate.rs` or `session.rs`'s `world_state()` body — confirmed by direct read, not just doc-comment trust. |
| 4 | The API compiles clean under strict Rust settings (no `unsafe` in public surface, no `unwrap` in library code) | ✓ VERIFIED | `lib.rs:28-30` carries `#![deny(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]` as crate-inner attributes. Independently ran `cargo clippy -p rdpilot --lib --target x86_64-unknown-linux-gnu` — **exit code 0**, with exactly 2 pre-existing warnings (`input.rs:11` unused import, `rdpdr_backend.rs:370` `unnecessary_get_then_check`) confirmed via `git blame` to predate Phase 8 (introduced Phase 5 commit `c077aea` / 2026-07-08, and commit `d07e710` respectively — both before the first Phase 8 commit `c0a6c43`). Manually grepped every `.unwrap()`/`.expect()` occurrence across `src/*.rs` and confirmed each falls within a `#[cfg(test)] mod tests` block (checked line-number ranges against each file's `#[cfg(test)]` marker) — none exist in production code paths. `grep -rn "unsafe"` across `src/` shows zero `unsafe` blocks (only doc-comment text "PDU-unsafe" and the `#![deny(unsafe_code)]` attribute itself). |

**Score:** 4/4 truths structurally VERIFIED offline; SC#2's empirical live measurement is PENDING (not a failure — explicitly deferred per D-8.2 + prior-phase precedent, gated test exists and is correctly authored).

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rdpilot/src/lib.rs` | Strict lint gates + clean re-export boundary | ✓ VERIFIED | Inner `#![deny(...)]` x3 present; re-export list matches D-09 owned-types-only contract exactly |
| `crates/rdpilot/src/worldstate.rs` | `WorldStateOptions`/`UiaMode`/`WorldState` types | ✓ VERIFIED | Exists, substantive (137 lines, full doc comments, correct field types), wired (imported into `lib.rs` and `session.rs`) |
| `crates/rdpilot/src/session.rs` (`world_state` method) | Composite sequencing method | ✓ VERIFIED | Exists at line 688, substantive (69 lines of real sequencing logic, not a stub), wired (5 offline unit tests + 2 live gated tests exercise it directly) |
| `crates/rdpilot/src/screenshot.rs`, `perception.rs` (Serialize derives) | `Serialize` on `Rect`/`Screenshot`/`WindowInfo`/`WindowState`/`ProcessInfo`/`UiaElement` | ✓ VERIFIED | Confirmed derives present; `Screenshot` correctly `#[serde(skip)]`s the `rgba` buffer (dims-only per D-8.4) |
| `crates/rdpilot/tests/live_session.rs` (2 new gated tests) | Live SC#2 capture-span tests | ✓ VERIFIED (exists, substantive, wired, correctly gated) — ⚠ NOT YET RUN against a live target | `#[ignore]` + `require_target!`-gated as claimed; compiles clean (`cargo test --test live_session` shows the 2 new tests among `22 ignored`, confirmed via SUMMARY and independently re-derivable from the file's `#[ignore]` attributes) |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| `Session::world_state()` | `Session::screenshot()`/`get_window_list()`/`get_uia_tree()` | direct `.await?` calls, sequenced | ✓ WIRED | Errors propagate via `?` (never silently downgraded to `None` — confirmed by the `world_state_error_propagates_as_err_not_none` offline test, which passes) |
| `lib.rs` | `worldstate.rs` | `mod worldstate;` + `pub use worldstate::{UiaMode, WorldState, WorldStateOptions};` | ✓ WIRED | Confirmed present |
| `WorldState.screenshot/window_list/uia` | `crate::Screenshot`/`WindowInfo`/`UiaElement` | direct field reuse, no new type | ✓ WIRED | No coordinate translation layer exists — same `Rect` end-to-end |

### Behavioral Spot-Checks (offline, run by verifier independently)

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Full offline unit test suite passes (99/99, includes 5 new `world_state` tests) | `cargo test -p rdpilot --target x86_64-unknown-linux-gnu --lib` | `test result: ok. 99 passed; 0 failed; 0 ignored` | ✓ PASS |
| Clippy exits 0 under the deny gates, no new warnings introduced by Phase 8 | `cargo clippy -p rdpilot --lib --target x86_64-unknown-linux-gnu` | exit 0; 2 pre-existing warnings only, confirmed pre-dating Phase 8 via `git blame` | ✓ PASS |
| No `unsafe`/production `.unwrap()`/`.expect()` in `src/` | manual grep + `#[cfg(test)]` boundary check | zero production occurrences | ✓ PASS |

### Probe Execution

No `scripts/*/tests/probe-*.sh` convention exists in this repo; this phase's runnable-check equivalent is the gated `tests/live_session.rs` integration tests, covered under Human Verification below (they require a live Azure target and were not executed by the verifier — running them incurs cost and requires provisioning, correctly out of scope for static verification).

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|--------------|--------|----------|
| API-01 | 08-01, 08-02, 08-03 | Clean, typed SDK API surface | ✓ SATISFIED | `lib.rs` re-export boundary, zero third-party type leakage, compiler-enforced no-unsafe/no-unwrap/no-expect gates, all independently confirmed |
| API-02 | 08-01, 08-02, 08-03 | Coherent `WorldState` correlating screenshot + window list + UIA in one coordinate space | ✓ SATISFIED (single-space scope) — note below | `WorldState`/`world_state()` exist, correlate under one timestamp/span, reuse the single shared `Rect` coordinate space |

**Note on API-02's literal REQUIREMENTS.md text:** REQUIREMENTS.md's full API-02 description reads "...emitting both pixel and logical/DPI-scaled coordinates." This dual-representation clause was explicitly evaluated and deferred during Phase 8 context-gathering (D-8.3, `08-CONTEXT.md`): logical==physical while every v1 target is forced to 96 DPI, so a second coordinate representation would be a zero-benefit abstraction today. This is a documented, deliberate, pre-implementation scope decision (not a post-hoc gap), and it does not contradict ROADMAP's operative SC#3 wording ("single...pixel space with no silent scaling"), which the code satisfies exactly. Flagged here for visibility, not as a gap — the roadmap SC#3 is the enforceable contract for this phase and it is fully met.

### Anti-Patterns Found

None. Scanned all phase-8-modified files (`lib.rs`, `screenshot.rs`, `perception.rs`, `worldstate.rs`, `session.rs`) for `TBD|FIXME|XXX|TODO|HACK|PLACEHOLDER` and stub-style empty-return patterns — zero blocker/warning hits. The single "placeholders" text match in `session.rs` is a benign test-helper doc comment (`test_window()` helper, filling irrelevant `WindowInfo` fields for a rect-only test), not a production stub.

### Human Verification Required

### 1. Live `world_state()` capture-span measurement against a real target

**Test:** Provision (or confirm) a reachable Azure Windows target and a current `rdpilot-sensor.exe` publish, then run `RDPILOT_LIVE=1 cargo test -p rdpilot --test live_session -- --include-ignored --test-threads=1 world_state`.
**Expected:** Both `world_state_default_options_reports_capture_span` and `world_state_foreground_uia_matches_focused_window` pass; the measured `capture_span` (printed via `eprintln!`) is recorded for the phase record. Per D-8.2 there is no hard pass/fail line at 500ms — the test only fails on a genuine transport/semantic error or a broken (zero) span measurement.
**Why human:** Requires a live, network-reachable RDP target and incurs real Azure cost to provision. The verifier confirmed no target is currently reachable (the stale `.secrets/connection.json` host is not answering on port 3389) and correctly did not attempt provisioning as part of static code verification.

### Gaps Summary

No code-level gaps found. All 4 ROADMAP success criteria are structurally implemented and independently re-verified by the verifier (not merely trusted from SUMMARY.md): the public API boundary is clean (SC#1), `world_state()` correctly sequences and correlates all three components under one timestamp/span with the 500ms bound correctly implemented as best-effort/non-blocking rather than hard-failing (SC#2 structure), the coordinate space is provably unified with no new scaling layer (SC#3), and the strict lint gates are both declared and empirically confirmed to hold (clippy exit 0, zero production unwrap/expect/unsafe) (SC#4).

The only open item is SC#2's **empirical** live capture-span number, which requires a provisioned, reachable Azure VM. This is consistent with the project's established "reason offline, live-tune empirically" methodology used in every prior phase (e.g., Phase 7's live-measured UIA walk time), and D-8.2 explicitly designed the bound to be best-effort specifically so a transient slow live measurement never blocks the API's correctness. The gated test that will produce this measurement is authored, compiles clean, and is correctly wired (`#[ignore]` + `require_target!`) so it does not block the default test suite.

**Recommendation:** Phase 8 can be considered code-complete and the offline-verifiable portions of SC#1–SC#4 are PASSED with no gaps. A live Azure VM run is NOT required to close the phase's structural/code deliverables, but IS required before the *empirical* half of SC#2 (the actual measured capture_span number) can be marked fully closed — this is a data-collection task, not an implementation gap. Recommend routing the live run as a human/operator follow-up (via `infra/manage-env.ps1 up`, per the 08-03 SUMMARY's documented follow-up steps) either now or bundled with Phase 9's live gate, rather than blocking Phase 8 from being marked complete in the roadmap (which ROADMAP.md already reflects — Phase 8 is checked `[x]` complete, consistent with treating the live measurement as a deferred data point rather than an unmet success criterion).

---

*Verified: 2026-07-09T22:15:00Z*
*Verifier: Claude (gsd-verifier)*
