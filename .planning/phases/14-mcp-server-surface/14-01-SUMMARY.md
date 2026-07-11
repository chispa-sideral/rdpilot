---
phase: 14-mcp-server-surface
plan: 01
subsystem: api
tags: [rdpilot-ipc, rdpilot-daemon, rdpilot-cli, serde, wire-protocol, session-scoped]

# Dependency graph
requires:
  - phase: 13-cli-surface
    provides: rdpilot-ipc Request/WireResponse wire vocabulary, ManagedSession seam, rdpilot-cli parse_key_name
provides:
  - "Request::DesktopSize { session: SessionId } — session-scoped wire verb, SESSION-02-preserved"
  - "WireResponse::DesktopSize { width: u16, height: u16 } — round-trips through serde_json"
  - "rdpilot_ipc::parse_wire_key(name: &str) -> Result<WireKey, String> — the ONE canonical key-name table"
  - "ManagedSession::desktop_size(&self) -> (u32, u32) seam method, forwarding to rdpilot::Session::desktop_size()"
  - "daemon dispatch arm: Request::DesktopSize -> WireResponse::DesktopSize via registry.call"
affects: [14-02-rdpilot-mcp-scaffold, 14-03-computer-mega-tool, 14-04-native-tools]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "rdpilot-ipc stays dependency-free of rdpilot/ironrdp/rustls even as new wire verbs are added (cargo tree re-verified)"
    - "One canonical key-name parser lives in rdpilot-ipc; consumer crates (CLI now, MCP later) delegate rather than re-copying the table"

key-files:
  created: []
  modified:
    - crates/rdpilot-ipc/src/request.rs
    - crates/rdpilot-ipc/src/response.rs
    - crates/rdpilot-ipc/src/input.rs
    - crates/rdpilot-ipc/src/lib.rs
    - crates/rdpilot-cli/src/verbs/input.rs
    - crates/rdpilot-daemon/src/seams.rs
    - crates/rdpilot-daemon/src/dispatch.rs
    - crates/rdpilot-daemon/src/lifecycle.rs
    - crates/rdpilot-daemon/src/registry.rs
    - crates/rdpilot-daemon/src/server.rs
    - crates/rdpilot-daemon/tests/crash_restart_reconcile.rs
    - crates/rdpilot-daemon/tests/registry_concurrency.rs
    - crates/rdpilot-daemon/tests/thread_leak_soak.rs

key-decisions:
  - "DesktopSize sources native desktop dimensions over the wire (a small Request/WireResponse verb pair) rather than PNG-header-sniffing the last screenshot — no call-ordering coupling, no PNG-parsing dependency, per research Open Question 1's recommendation"
  - "parse_wire_key promoted into rdpilot-ipc as the single canonical 67-entry key-name table; rdpilot-cli delegates instead of holding a second copy, per research 'Don't Hand-Roll'"
  - "ManagedSession::desktop_size is a plain synchronous getter (no BoxFuture) since it is a cheap stored-field read on rdpilot::Session, unlike every other ManagedSession method"

patterns-established:
  - "Wire dims (WireResponse::DesktopSize) are u16, matching the existing WireRect/config width/height convention — RDP desktop dims never exceed 65535"

requirements-completed: [MCP-02, MCP-04]

# Metrics
duration: ~25min
completed: 2026-07-11
---

# Phase 14 Plan 01: Upstream wire/daemon extensions Summary

**Added `Request::DesktopSize`/`WireResponse::DesktopSize` to `rdpilot-ipc` (SESSION-02-preserved) so MCP-04's coordinate bridge can source a session's real native desktop dimensions over the wire, and promoted the CLI's 67-entry key-name parser into `rdpilot_ipc::parse_wire_key` as the one canonical table shared with the future MCP `computer` tool.**

## Performance

- **Duration:** ~25 min
- **Tasks:** 3 completed
- **Files modified:** 13

## Accomplishments

- `Request::DesktopSize { session: SessionId }` is a session-scoped verb following the exact SESSION-02 convention (required non-`Option` field, hard `serde_json` deserialize rejection when `session` is omitted) — extended both inline SESSION-02 test vectors (`every_operational_request_verb_rejects_a_missing_session_field` / `..._accepts_a_present_session_field`) plus the `session_scoped_returns_the_embedded_id_for_every_operational_variant` test.
- `WireResponse::DesktopSize { width: u16, height: u16 }` round-trips through `serde_json` and is covered by the `sample_all_response_variants` exhaustive-match compile-time forcing function (a future variant added without updating that function is now a compile error, unchanged pattern).
- `rdpilot_ipc::parse_wire_key(name: &str) -> Result<WireKey, String>` ports the CLI's key-name table VERBATIM (same case-insensitive lowercasing, same alias spellings: `esc`/`escape`, `del`/`delete`, `page-up`/`pageup`, `win`/`windows`/`super`, `digitN`/`N`), exported from `lib.rs`. `rdpilot-cli/src/verbs/input.rs::parse_key_name` now delegates (`rdpilot_ipc::parse_wire_key(name).map_err(CliError::Internal)`) — confirmed by grep that no standalone key-name match table remains in the CLI.
- `ManagedSession::desktop_size(&self) -> (u32, u32)` added to the daemon seam trait as a plain synchronous getter (no `.await`/`BoxFuture`, unlike every other `ManagedSession` method — `desktop_size` is a cheap stored-field read on `rdpilot::Session`), implemented on the real `Session` wrapper by forwarding to the SDK's existing `Session::desktop_size()` (already present, `(u32, u32)`, matched the plan's assumed signature exactly).
- `dispatch`'s exhaustive `Request` match gained the `DesktopSize` arm: routes through `registry.call` to the live session and returns `WireResponse::DesktopSize { width: w as u16, height: h as u16 }`, or `WireResponse::Error(SessionNotFound)` for an unknown session (`Registry::call`'s existing guarantee) — proven by two new dispatch tests.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add the DesktopSize request/response verb pair to rdpilot-ipc** - `ba33f36` (feat)
2. **Task 2: Promote parse_wire_key into rdpilot-ipc and delegate the CLI parser to it** - `5bd42f8` (feat)
3. **Task 3: Add the desktop_size seam + real forward + daemon dispatch arm** - `0d96d88` (feat)

_No plan-metadata commit was made in this run — a subsequent finalization step for the shared Phase 14 wave (which includes the concurrently-running 14-02) may add one covering both plans' ROADMAP/STATE updates._

## Files Created/Modified

- `crates/rdpilot-ipc/src/request.rs` - `Request::DesktopSize { session }` + `SessionScoped` exhaustive-match arm + extended SESSION-02 test vectors
- `crates/rdpilot-ipc/src/response.rs` - `WireResponse::DesktopSize { width, height }` + `sample_all_response_variants` sample/match arm
- `crates/rdpilot-ipc/src/input.rs` - `pub fn parse_wire_key` (the promoted canonical table) + inline test
- `crates/rdpilot-ipc/src/lib.rs` - `pub use input::parse_wire_key`
- `crates/rdpilot-cli/src/verbs/input.rs` - `parse_key_name` now delegates to `rdpilot_ipc::parse_wire_key`, standalone table deleted
- `crates/rdpilot-daemon/src/seams.rs` - `ManagedSession::desktop_size` trait method + real `Session` forward
- `crates/rdpilot-daemon/src/dispatch.rs` - `DesktopSize` dispatch arm + `FakeSession`/`RichPerceptionSession`/`WithScreenshotSession` fakes updated + 2 new tests
- `crates/rdpilot-daemon/src/lifecycle.rs` - inline `FakeSession` fake updated with `desktop_size`
- `crates/rdpilot-daemon/src/registry.rs` - inline `FakeSession` fake updated with `desktop_size`
- `crates/rdpilot-daemon/src/server.rs` - `FakeTestSession` (the runtime `RDPILOT_DAEMON_TEST_CONNECTOR` fake) updated with `desktop_size`
- `crates/rdpilot-daemon/tests/crash_restart_reconcile.rs` - integration-test `FakeSession` updated with `desktop_size`
- `crates/rdpilot-daemon/tests/registry_concurrency.rs` - integration-test `FakeSession` updated with `desktop_size`
- `crates/rdpilot-daemon/tests/thread_leak_soak.rs` - integration-test `ThreadOwningFakeSession` updated with `desktop_size`

## Decisions Made

- `WireResponse::DesktopSize` fields are `u16` (not `u32`, despite `Session::desktop_size()` returning `(u32, u32)`): matches the plan's explicit rationale (RDP desktop dims never exceed 65535, and the existing `WireRect`/config width/height fields are already `u16`) — the dispatch arm narrows with `as u16`.
- `desktop_size` on the `ManagedSession` trait is deliberately synchronous (`fn desktop_size(&self) -> (u32, u32)`, no `BoxFuture`), diverging from every other trait method's async shape — justified because it is a cheap stored-field read on `rdpilot::Session`, not an I/O-bound SDK call.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] ManagedSession::desktop_size needed adding to five fakes outside the plan's files_modified scope**
- **Found during:** Task 3 (`cargo test -p rdpilot-daemon`)
- **Issue:** The plan's `files_modified` for Task 3 listed only `seams.rs`/`dispatch.rs`. `ManagedSession` has no default trait implementation, and is implemented by five additional fakes the plan did not name: `lifecycle.rs`'s inline `FakeSession`, `registry.rs`'s inline `FakeSession`, `server.rs`'s `FakeTestSession` (the runtime `RDPILOT_DAEMON_TEST_CONNECTOR` fake), and the `crash_restart_reconcile.rs`/`registry_concurrency.rs`/`thread_leak_soak.rs` integration-test fakes. Without `desktop_size` on all of them, the crate (and its integration tests) fail to compile with `E0046: not all trait items implemented`.
- **Fix:** Added `fn desktop_size(&self) -> (u32, u32) { (1920, 1080) }` to each of the five fakes, using the plan's own suggested canned value for the `dispatch.rs` fakes — no behavior change to any existing fake beyond the new method.
- **Files modified:** `crates/rdpilot-daemon/src/lifecycle.rs`, `crates/rdpilot-daemon/src/registry.rs`, `crates/rdpilot-daemon/src/server.rs`, `crates/rdpilot-daemon/tests/crash_restart_reconcile.rs`, `crates/rdpilot-daemon/tests/registry_concurrency.rs`, `crates/rdpilot-daemon/tests/thread_leak_soak.rs`
- **Verification:** `cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu` green (68 lib tests + all integration test binaries), including `--include-ignored` re-runs of `thread_leak_soak` (both `thread_count_returns_to_baseline_after_a_few_cycles` and the heavier `thread_and_rss_return_to_baseline_after_fifty_cycles`) and `registry_concurrency` (`n_simultaneous_same_name_connects_yield_exactly_one_winner`, `n_simultaneous_unnamed_connects_yield_n_distinct_auto_ids`)
- **Committed in:** `0d96d88` (Task 3 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Necessary for the crate to compile at all once the trait method was added — no scope creep beyond the mechanical fake-completion the compiler forces.

## Issues Encountered

- Toolchain: `cargo`/`rustup` are not on `PATH` by default in this shell (`~/.cargo/bin` had to be added explicitly). All builds/tests in this plan ran with `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> --target x86_64-unknown-linux-gnu` per the offline-substitution instructions (the repo-pinned `x86_64-pc-windows-gnu` toolchain is not installed on this host); `crates/rdpilot-ipc`/`rdpilot-daemon`/`rdpilot-cli` have no `cfg(windows)` code, so this is a safe substitution consistent with prior phases' documented pattern.
- A concurrent executor (Plan 14-02) is running against the same working tree, adding `crates/rdpilot-mcp/*` and a new workspace member to the root `Cargo.toml`/`Cargo.lock`. Neither was touched or staged by this plan's commits — `git add` was scoped to only the specific files this plan modified in each task, and the untracked `crates/rdpilot-mcp/` directory was left alone.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `Request::DesktopSize`/`WireResponse::DesktopSize` are ready for Plan 14-03's `scale_to_native` coordinate bridge to consume via a daemon round trip.
- `rdpilot_ipc::parse_wire_key` is ready for Plan 14-03's `computer` tool `key`/`hold_key` actions and Plan 14-04's native tools to call directly — no second key-name table should be written.
- `ManagedSession::desktop_size` is wired end-to-end (seam -> dispatch -> wire); no further daemon-side work needed for MCP-04.
- Verified: `cargo test -p rdpilot-ipc` (39/39), `-p rdpilot-daemon` (68 lib tests + all integration tests, including `--include-ignored` thread-leak-soak/registry-concurrency non-regression suites), `-p rdpilot-cli` (all green) — all on the `x86_64-unknown-linux-gnu` substitute target. `cargo tree -p rdpilot-ipc --target x86_64-unknown-linux-gnu` reconfirmed dependency-free of `rdpilot`/`ironrdp`/`rustls` (only `serde`/`serde_json`/`thiserror`/`tokio`/`directories` in the graph).
- ROADMAP.md's 14-01 checkbox ticked; the shared Phase-14 plan-counter/progress-table row was deliberately left untouched (concurrency boundary with 14-02) for a later finalization step to reconcile once both Wave-1 plans land.

---
*Phase: 14-mcp-server-surface*
*Completed: 2026-07-11*

## Self-Check: PASSED

All 13 modified files confirmed present on disk; all 3 task commit hashes (`ba33f36`, `5bd42f8`, `0d96d88`) confirmed in `git log`.
