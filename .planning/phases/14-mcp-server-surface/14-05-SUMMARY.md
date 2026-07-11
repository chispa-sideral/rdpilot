---
phase: 14-mcp-server-surface
plan: 05
subsystem: mcp-server
tags: [rmcp, tokio, mcp-06, non-blocking-isolation, daemon-fake-connector, integration-test]

# Dependency graph
requires:
  - phase: 14-mcp-server-surface (14-02)
    provides: round_trip_bounded + per-verb-class timeouts (FAST/CONNECT/LIFECYCLE/TRANSFER) + McpError::Timeout
  - phase: 14-mcp-server-surface (14-04)
    provides: the 11 rdpilot_* native tools (rdpilot_connect/rdpilot_put/rdpilot_list) on RdpilotMcpHandler
provides:
  - "An env-gated slowness hook (RDPILOT_DAEMON_TEST_SLOW_MS) in rdpilot-daemon's FakeTestConnector/FakeTestSession, gated strictly behind the existing RDPILOT_DAEMON_TEST_CONNECTOR test flag"
  - "MCP-06 [BLOCKING] proof: a slow rdpilot_put does not block a concurrent fast rdpilot_list, measured against the real compiled rdpilot-daemon binary"
affects: [15-proof-harnesses-live-llm-capstone]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Env-gated TEST-only slowness injected into an existing fake connector's session methods, read once at daemon startup and threaded through as a plain struct field -- never touching the production RealConnector"
    - "Pre-start the real daemon via a directly-and-correctly-resolved sibling binary path (CARGO_BIN_EXE_<same-package-bin>) before making any in-process handler call, sidestepping current_exe()-based sibling resolution mismatches between a test binary (target/.../deps/) and the installed binary layout (target/.../debug/)"
    - "tokio::task::JoinHandle::is_finished() as the concrete non-blocking overlap proof (not just latency measurement) -- assert false immediately after a concurrent fast call already returned Ok"

key-files:
  created:
    - crates/rdpilot-mcp/tests/non_blocking.rs
  modified:
    - crates/rdpilot-daemon/src/server.rs

key-decisions:
  - "Task 1's env-unset regression guard is proven as unit tests directly against FakeTestSession (bypassing env var mutation entirely, avoiding parallel-test env-var flakiness), not as a second full-daemon-subprocess integration test"
  - "The isolation test pre-starts the real daemon itself via CARGO_BIN_EXE_rdpilot-mcp's sibling path rather than relying on connect::open_stream's own current_exe()-based resolution, which would incorrectly resolve against the TEST BINARY's own directory (target/.../deps/) when driven in-process"
  - "#[tokio::test(flavor = \"multi_thread\", worker_threads = 2)] used (not the single-threaded default) so the proven concurrency mirrors main.rs's own #[tokio::main] multi-thread runtime shape"

requirements-completed: [MCP-06]

# Metrics
duration: 12min
completed: 2026-07-11
---

# Phase 14 Plan 05: MCP-06 Non-Blocking Isolation Proof Summary

**A slow `rdpilot_put` call (delayed 2000ms by a new env-gated daemon-side slowness hook) provably does not block a concurrent fast `rdpilot_list` call — fast call measured returning in ~323µs while the slow call was confirmed still pending (`JoinHandle::is_finished() == false`), against the real compiled `rdpilot-daemon` binary, fully offline.**

## Performance

- **Duration:** 12 min
- **Started:** 2026-07-11T12:15:00+02:00 (approx, first commit 12:27)
- **Completed:** 2026-07-11T12:39:33+02:00
- **Tasks:** 2/2
- **Files modified:** 2 (1 modified, 1 created)

## Accomplishments

- Added `RDPILOT_DAEMON_TEST_SLOW_MS` to `rdpilot-daemon`'s existing `FakeTestConnector`/`FakeTestSession` (behind the existing `RDPILOT_DAEMON_TEST_CONNECTOR` gate): when set, the fake session's three slow-class methods (`upload_file`/`download_file`/`launch_process`) genuinely `tokio::time::sleep` that many ms before resolving. Unset/invalid → `0` (byte-for-byte identical to prior behavior).
- Added `crates/rdpilot-mcp/tests/non_blocking.rs`: the MCP-06 [BLOCKING] integration test, driving `RdpilotMcpHandler`'s public `#[tool]` methods in-process against the real compiled `rdpilot-daemon` binary (with the fake connector + the new slowness hook), proving a slow `rdpilot_put` (2000ms) does not block a concurrently-issued fast `rdpilot_list`.
- Confirmed the thin-client invariant (`cargo tree -p rdpilot-mcp` contains only `rdpilot-config`/`rdpilot-ipc`/`rdpilot-mcp` — never `rdpilot` SDK or `rdpilot-daemon`).
- Confirmed no regressions: `cargo test -p rdpilot-daemon` (default + `--include-ignored`, except one unrelated pre-existing environment-gated test), `cargo test --workspace` all green.

## Task Commits

Each task was committed atomically:

1. **Task 1: Env-gated slowness in the daemon's fake test connector** - `a9e5a98` (feat)
2. **Task 2: MCP-06 BLOCKING isolation proof — slow tool ∥ fast tool against the real daemon** - `fa8a7d2` (test)

**Plan metadata:** (this commit)

## Files Created/Modified

- `crates/rdpilot-daemon/src/server.rs` — `TEST_SLOW_MS_ENV` const + `slow_ms: u64` field on `FakeTestSession`/`FakeTestConnector`; `upload_file`/`download_file`/`launch_process` conditionally `tokio::time::sleep`; `run_inner` reads the env var once and threads it through; new unit tests (`fake_session_slow_class_methods_resolve_immediately_when_slow_ms_is_zero`, `fake_session_slow_class_methods_sleep_the_configured_slow_ms`).
- `crates/rdpilot-mcp/tests/non_blocking.rs` — the MCP-06 isolation proof (`slow_tool_call_does_not_block_a_concurrent_fast_tool_call_mcp06`): pre-starts the real daemon, `rdpilot_connect`s a fake-connector session, spawns a slow `rdpilot_put` and races a fast `rdpilot_list`, asserting overlap via `JoinHandle::is_finished()` plus wall-clock bounds.

## Decisions Made

- **Env-unset regression guard as direct unit tests, not a second daemon subprocess.** The objective's "assert that with the env var UNSET there is no slowdown" is satisfied by two new `#[tokio::test]`s directly against `FakeTestSession` (constructed with `slow_ms: 0` vs. `slow_ms: N`), avoiding the flakiness of mutating a global env var across a `cargo test` binary's parallel test threads, and avoiding an unnecessary second real-daemon-subprocess spin-up. `cargo test -p rdpilot-daemon`'s existing suite (which never sets the env var) is the corroborating "no regression to any existing test" evidence.
- **Pre-start the real daemon via `CARGO_BIN_EXE_rdpilot-mcp`'s sibling path, not `connect::open_stream`'s internal resolution.** `open_stream()` resolves the sibling `rdpilot-daemon` binary via `std::env::current_exe().with_file_name(..)`. When `non_blocking.rs`'s own handler calls run in-process, `current_exe()` is the TEST binary's path (`target/<triple>/debug/deps/non_blocking-<hash>`), not `rdpilot-mcp`'s own installed-alongside-the-daemon directory (`target/<triple>/debug/`) — `with_file_name` on the wrong directory would look for a nonexistent `deps/rdpilot-daemon`. Since `connect_or_spawn` only consults `daemon_exe` if its first plain `try_connect` fails, this test pre-starts the daemon itself (via the correctly-resolved `CARGO_BIN_EXE_rdpilot-mcp`'s sibling path, mirroring `rdpilot-cli/tests/cli_lifecycle.rs`'s identical same-package `CARGO_BIN_EXE_*` convention) so every subsequent internal call's `try_connect` always succeeds immediately, making the mismatched internal resolution irrelevant.
- **`flavor = "multi_thread", worker_threads = 2`** on the isolation test (rather than `#[tokio::test]`'s single-threaded default), so the proven concurrency shape matches `main.rs`'s own `#[tokio::main]` multi-thread runtime, not merely single-thread cooperative interleaving at await points.

## Deviations from Plan

None — plan executed exactly as written. Both tasks' `<done>` criteria were met without any Rule 1-4 fixes; the only design choices beyond the plan's literal text were the two documented above (regression-guard test shape, daemon pre-start mechanism), both within the plan's own stated flexibility ("the test must place/point the MCP test process at the built rdpilot-daemon binary the same way").

## Issues Encountered

- **Toolchain substitution (environment-level, not a plan issue):** this sandbox has no Windows GNU Rust target installed; per this plan's binding "Toolchain / offline note", all builds/tests ran via `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo ... --target x86_64-unknown-linux-gnu` (native-Linux substitute), not the project's eventual Windows target. Recorded here as the standing deviation note, not a plan deviation.
- One pre-existing, unrelated `rdpilot-daemon` test (`ipc_security::cross_account_peer_is_rejected_end_to_end`) fails under `--include-ignored` in this sandbox because it requires `RDPILOT_SECOND_UID` (a second local account + passwordless sudo) that isn't configured here — untouched by this plan, out of scope per the scope-boundary rule, and it already explicitly documents this requirement in its own panic message.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- **MCP-06 is Complete.** Phase 14 (MCP Server Surface) is now 5/5 plans complete, offline-verifiable in full: MCP-01 (tool registration/schema), MCP-02 (computer mega-tool mapping), MCP-03 (11 native tools), MCP-04 (scale_to_native, pure-function BLOCKING proof), MCP-05 (metadata-only put/get), and MCP-06 (this plan's non-blocking isolation BLOCKING proof) are all Complete offline.
- **Deferred to the Phase 15 batched live gate** (per Phase 14's own ROADMAP note, unchanged by this plan): real screenshot pixel content, real click landing near edges/corners (the live half of MCP-04), and the live-LLM capstone (PROOF-04). This plan's own MCP-06 proof is fully self-contained and does NOT need re-exercising at the live gate — the daemon-side isolation mechanism it proves (per-connection/per-session daemon tasks + fresh per-call `UnixStream` + explicit timeouts) is architecture-level, not target-host-dependent.
- Phase 15 (Proof Harnesses & Live-LLM Capstone) can now proceed — its own `<context>`/dependency on "Phase 14 validates the combination of every prior phase functioning end-to-end" is satisfied.

---
*Phase: 14-mcp-server-surface*
*Completed: 2026-07-11*

## Self-Check: PASSED

- FOUND: crates/rdpilot-daemon/src/server.rs
- FOUND: crates/rdpilot-mcp/tests/non_blocking.rs
- FOUND commit: a9e5a98
- FOUND commit: fa8a7d2
