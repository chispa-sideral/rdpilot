---
phase: 13-cli-surface
plan: 02
subsystem: infra
tags: [serde, wire-protocol, rdpilot-ipc, perception, input, session-scoped]

# Dependency graph
requires:
  - phase: 13-cli-surface
    provides: "Plan 13-01's rdpilot-ipc (SessionScoped exhaustive match, WireResponse exhaustive-match forcing function, transport relocated) as the extension point"
provides:
  - "rdpilot_ipc::perception module: WireRect, WireWindowState, WireWindowInfo, WireProcessInfo, WireUiaScope, WireUiaElement, WireUiaMode, WireWorldStateOptions — owned, credential-free, rdpilot-independent mirrors"
  - "rdpilot_ipc::input module: WireButton, WireMouseAction, WireKey, WireKeyAction — owned mirrors of rdpilot's input vocabulary"
  - "Request::{WindowList,ProcessList,Uia,WorldState,Mouse,Key} — six new session-scoped verbs, each with a required non-Option session: SessionId field, added to SessionScoped's exhaustive match (SESSION-02 preserved)"
  - "WireResponse::{WindowList,ProcessList,Uia,WorldState} — four new response variants, added to sample_all_response_variants' exhaustive-match forcing function"
affects: ["13-cli-surface (Plan 13-04, daemon dispatch; Plan 13-06, the CLI itself — both build against this shared wire vocabulary)"]

# Tech tracking
tech-stack:
  added: []
  patterns: ["Every SDK type the wire needs gets its own owned Wire-prefixed mirror struct/enum in rdpilot-ipc (never imported from rdpilot) — the transfer.rs convention, now applied to the full perception/input vocabulary."]

key-files:
  created: [crates/rdpilot-ipc/src/perception.rs, crates/rdpilot-ipc/src/input.rs]
  modified: [crates/rdpilot-ipc/src/lib.rs, crates/rdpilot-ipc/src/request.rs, crates/rdpilot-ipc/src/response.rs]

key-decisions:
  - "Deviation (Rule 1 - plan bug): the plan's must_haves table and Task 1's <behavior> block both assert WireKey should have exactly 68 variants. The actual rdpilot::Key enum (crates/rdpilot/src/input.rs) has 67 variants (3 modifiers + 26 letters + 10 digits + 12 F-keys + 6 control keys + 4 arrows + 5 navigation-cluster keys + 1 Win key = 67, verified by direct source count). Since the plan's own primary truth is '1:1 mirror', WireKey was written with the actual 67-variant set and the test asserts count == 67, not 68. Documented here rather than padding WireKey with a phantom 68th variant that would break the 1:1 mirror guarantee."
  - "WorldState's screenshot field on WireResponse::WorldState reuses the existing base64-PNG-as-String convention from WireResponse::Screenshot (no new binary framing)."
  - "timestamp/capture_span_ms on WireResponse::WorldState are String/u64 (ISO-8601 / milliseconds) rather than SystemTime/Duration, per research: those SDK types have no serde impl, and the conversion is explicitly deferred to daemon-side dispatch (Plan 13-04), never performed in rdpilot-ipc."

patterns-established:
  - "New request verbs are added to Request's enum AND to SessionScoped's exhaustive no-wildcard match in the same commit — the match is a compile-time forcing function, not a convention, so both must move together or the crate fails to build."

requirements-completed: [CLI-02]

# Metrics
duration: ~20min
completed: 2026-07-11
---

# Phase 13 Plan 02: Extend rdpilot-ipc with perception/input wire DTOs Summary

**Added owned serde mirrors of rdpilot's window/process/UIA/world-state and mouse/key types to rdpilot-ipc, plus the six new session-scoped Request verbs and four new WireResponse variants they need — all rdpilot/IronRDP-free, all session-required (SESSION-02).**

## Performance

- **Duration:** ~20 min
- **Started:** 2026-07-11T09:05:00Z (approx.)
- **Completed:** 2026-07-11T09:25:00Z (approx.)
- **Tasks:** 2/2 completed
- **Files modified:** 5 (2 created, 3 modified)

## Accomplishments

- `rdpilot-ipc::perception` now carries `WireRect`, `WireWindowState` (lowercase-string serialization), `WireWindowInfo`, `WireProcessInfo`, `WireUiaScope`, `WireUiaElement`, `WireUiaMode`, `WireWorldStateOptions` — structurally identical to the `rdpilot::Session` types they mirror, defined independently per the `transfer.rs` owned-mirror convention.
- `rdpilot-ipc::input` now carries `WireButton`, `WireMouseAction`, `WireKey` (67 variants, 1:1 with `rdpilot::Key`), `WireKeyAction`.
- `Request` gained six new session-scoped verbs — `WindowList`, `ProcessList`, `Uia`, `WorldState`, `Mouse`, `Key` — each with a required non-`Option` `session: SessionId` field, added to `SessionScoped::session`'s exhaustive no-wildcard match. All three existing SESSION-02 tests (`every_operational_request_verb_rejects_a_missing_session_field`, `every_operational_request_verb_accepts_a_present_session_field`, `session_scoped_returns_the_embedded_id_for_every_operational_variant`) were extended with JSON cases for all six new verbs; every missing-session case still hard-rejects.
- `WireResponse` gained four new variants — `WindowList`, `ProcessList`, `Uia`, `WorldState` — added to `sample_all_response_variants`'s exhaustive-match forcing function and sample list, so the existing round-trip and CONFIG-03 planted-secret tests automatically cover them.
- `cargo tree -p rdpilot-ipc` confirmed empty for `ironrdp`/`rustls`/`rdpilot` — the crate stays dependency-free after the extension.
- All 38 `rdpilot-ipc` unit tests pass (up from 24 baseline after Plan 13-01); `cargo clippy -p rdpilot-ipc --all-targets` is clean.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add the perception + input wire mirror types** - `41c0b23` (feat)
2. **Task 2: Extend Request + WireResponse with the six new verbs and four new responses, preserving SESSION-02** - `2a8b21f` (feat)

**Plan metadata:** commit pending (this step)

## Files Created/Modified

- `crates/rdpilot-ipc/src/perception.rs` - New: WireRect, WireWindowState, WireWindowInfo, WireProcessInfo, WireUiaScope, WireUiaElement, WireUiaMode, WireWorldStateOptions + one round-trip test per type
- `crates/rdpilot-ipc/src/input.rs` - New: WireButton, WireMouseAction, WireKey (67 variants), WireKeyAction + round-trip tests + variant-count assertion
- `crates/rdpilot-ipc/src/lib.rs` - `mod input; mod perception;` + `pub use` re-exports of all new public types
- `crates/rdpilot-ipc/src/request.rs` - Six new `Request` variants (WindowList/ProcessList/Uia/WorldState/Mouse/Key), each added to `SessionScoped`'s exhaustive match; three SESSION-02 tests extended with the new verbs' JSON cases
- `crates/rdpilot-ipc/src/response.rs` - Four new `WireResponse` variants (WindowList/ProcessList/Uia/WorldState); `sample_all_response_variants`'s exhaustive-match forcing function and sample list extended

## Decisions Made

- **[Rule 1 - plan bug] WireKey variant count is 67, not 68.** The plan text (must_haves truths, Task 1 acceptance criteria, and the research doc) states "WireKey has exactly 68 variants" and enumerates the categories as "Ctrl/Alt/Shift + 26 letters + 10 digits + 12 F-keys + Enter/Esc/Tab/Space/Backspace/Delete + Up/Down/Left/Right + Home/End/PageUp/PageDown/Insert + Win". Summing that category breakdown gives 3+26+10+12+6+4+5+1 = 67, and a direct count of `pub enum Key { ... }` in `crates/rdpilot/src/input.rs` confirms 67 variants, not 68. Because the plan's binding requirement is "the SAME variant set as rdpilot::Key 1:1" (the count is a derived check, not the primary truth), `WireKey` was written with the actual 67-variant set copied verbatim from the SDK source, and the test asserts `count == 67`. Padding `WireKey` with an extra phantom variant to hit "68" would have broken the 1:1-mirror guarantee the plan itself prioritizes.
- `WireResponse::WorldState.screenshot: Option<String>` reuses the base64-PNG-as-plain-`String` convention already established by `WireResponse::Screenshot { png_base64 }` — no new binary-blob framing introduced.
- `WireResponse::WorldState.timestamp`/`capture_span_ms` are `String` (ISO-8601)/`u64` (milliseconds) rather than `SystemTime`/`Duration`, per the research doc's explicit note that those SDK types have no serde impl and the conversion belongs in daemon-side dispatch (Plan 13-04), never in this crate.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug in plan] WireKey variant count corrected from stated 68 to actual 67**

- **Found during:** Task 1
- **Issue:** The plan and research doc assert `WireKey` should have exactly 68 variants, but summing the plan's own stated category breakdown, and a direct line-count of `rdpilot::Key`'s definition in `crates/rdpilot/src/input.rs`, both give 67.
- **Fix:** Wrote `WireKey` with the SDK's actual 67-variant set (copied 1:1, variant-for-variant) and asserted `count == 67` in the test, satisfying the plan's primary "SAME variant set... 1:1" truth rather than the secondary, arithmetically-inconsistent count claim.
- **Files modified:** `crates/rdpilot-ipc/src/input.rs`
- **Commit:** `41c0b23`

No other deviations — plan executed as written otherwise.

## Issues Encountered

None. All verification steps (`cargo test -p rdpilot-ipc`, `cargo clippy -p rdpilot-ipc --all-targets`, `cargo tree -p rdpilot-ipc` dependency-free check) passed on the first attempt after each task's implementation.

## Toolchain Note

Per the execution context's offline substitution instructions: built and tested against the `x86_64-unknown-linux-gnu` substitute target (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-ipc --target x86_64-unknown-linux-gnu`) rather than the repo's normal Windows-hosted toolchain, since only the Linux stable toolchain is installed on this host. Same substitution already established by Plan 13-01.

## Requirements Coverage

- **CLI-02**: The full perception/input wire vocabulary (WindowList, ProcessList, Uia, WorldState, Mouse, Key) now exists on the wire as owned, credential-free, session-required DTOs — the shared contract Plan 13-04 (dispatch) and Plan 13-06 (CLI) both build against.

## Self-Check: PASSED

- FOUND: `crates/rdpilot-ipc/src/perception.rs`
- FOUND: `crates/rdpilot-ipc/src/input.rs`
- FOUND: commit `41c0b23` in `git log --oneline --all`
- FOUND: commit `2a8b21f` in `git log --oneline --all`
- FOUND: `crates/rdpilot-ipc/src/lib.rs` re-exports `WireButton, WireKey, WireKeyAction, WireMouseAction` and `WireProcessInfo, WireRect, WireUiaElement, WireUiaMode, WireUiaScope, WireWindowInfo, WireWindowState, WireWorldStateOptions`
- FOUND: `cargo test -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — 38/38 passed
- FOUND: `cargo tree -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — no `ironrdp`/`rustls`/`rdpilot` entries
