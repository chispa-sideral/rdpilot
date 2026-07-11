---
phase: 12-session-daemon
plan: 06
subsystem: infra
tags: [tokio, localset, unix-socket, watch-channel, process-spawn, daemon-lifecycle]

# Dependency graph
requires:
  - phase: 12-session-daemon (12-02/12-03/12-04/12-05)
    provides: Registry (claim-then-connect, close-not-drop), Unix IPC (0700 dir + peer-uid + framing + dispatch), JsonReconciliationSink (scan_orphans/seed_into)
provides:
  - "server::run(RunConfig) -- the full daemon assembly: bind-as-mutex, startup orphan seed, accept loop, idle reaper, empty-registry grace-period self-shutdown"
  - "lifecycle::{LifecycleConfig, ShutdownSignal, idle_reaper, empty_watcher} -- D-31's config-overridable idle timeout + anti-thrash grace period"
  - "autostart::connect_or_spawn -- client-side bind-as-mutex auto-start helper (Unix-only for now), consumed later by Phase 13/14"
  - "registry::Registry::live_idle_durations() -- Instant-based staleness accessor the idle reaper needed"
  - "tests/autostart_lifecycle.rs -- SC#5 [BLOCKING] offline proof against the REAL compiled binary"
affects: [phase-13-cli-surface, phase-14-mcp-server-surface, phase-12-07-live-gate]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "tokio::task::LocalSet + spawn_local for every non-Send code path (Session::connect's future is not Send) -- never a bare tokio::spawn in this crate's daemon-side code"
    - "tokio::sync::watch-backed ShutdownSignal (not Notify) -- send_replace (not send) so fire() works even before the first subscriber, and a late wait() caller still observes an already-fired signal"
    - "env-var cross-process config injection (RDPILOT_DAEMON_*_MS, RDPILOT_DAEMON_TEST_CONNECTOR, RDPILOT_DAEMON_SINK_PATH, XDG_RUNTIME_DIR) -- the only channel connect_or_spawn's detached child process inherits"
    - "runtime-selected fake in-process connector (env-gated) compiled into the real binary, so the BLOCKING lifecycle test drives the actual compiled daemon offline with no RDP target"

key-files:
  created:
    - crates/rdpilot-daemon/tests/autostart_lifecycle.rs
  modified:
    - crates/rdpilot-daemon/src/lifecycle.rs
    - crates/rdpilot-daemon/src/autostart.rs
    - crates/rdpilot-daemon/src/server.rs
    - crates/rdpilot-daemon/src/registry.rs
    - crates/rdpilot-daemon/src/lib.rs
    - crates/rdpilot-daemon/src/main.rs

key-decisions:
  - "ShutdownSignal uses tokio::sync::watch::Sender::send_replace, not send -- send() silently no-ops when zero receivers are subscribed (exactly ShutdownSignal::new()'s initial state), which caused a real test failure during authoring (fire() appeared to succeed but the value never updated); send_replace unconditionally updates regardless of receiver count."
  - "RunConfig does not parameterize the well-known IPC socket path -- ipc::unix::bind (Plan 12-04, out of this plan's file scope) always resolves it via BaseDirs::runtime_dir() (the standard XDG_RUNTIME_DIR env var). The BLOCKING integration test achieves isolation by setting XDG_RUNTIME_DIR to a fresh temp dir before spawning the daemon, exactly as any other process on the host would, rather than adding a parallel parameterized bind path."
  - "idle_reaper/empty_watcher never use tokio::spawn (would require Send, which Registry::close's session.close().await chain is not, per seams.rs's BoxFuture doc comment) -- server::run wraps everything in a tokio::task::LocalSet and uses spawn_local throughout, leaving main.rs's #[tokio::main] multi-thread runtime untouched (LocalSet::run_until works regardless of runtime flavor)."
  - "Self-shutdown is proven via the daemon's own socket-file removal + a subsequent refused connection attempt, not via tracking a child-process PID/Child handle -- connect_or_spawn's signature (io::Result<UnixStream>) never exposes the spawned Child, and this is a cleaner black-box proof of the same DAEMON-03 guarantee."

patterns-established:
  - "Pattern: env-var-only cross-process test config injection for a detached-spawn daemon (no IPC handshake exists yet to pass structured config across the process boundary)."
  - "Pattern: env-gated fake connector compiled into the production binary (not #[cfg(test)]) so a real-binary integration test can exercise the full lifecycle offline."

requirements-completed: [DAEMON-03]

# Metrics
duration: ~50min (estimated -- no precise start timestamp captured at spawn)
completed: 2026-07-11
---

# Phase 12 Plan 06: Daemon Server Assembly + Auto-Start/Idle-Shutdown Summary

**`server::run()` assembles bind-as-mutex + startup orphan seed + idle reaper + empty-registry grace-period self-shutdown, all driven via `tokio::task::LocalSet`/`spawn_local` (never `tokio::spawn`, since `Session::connect`'s future is not `Send`); `connect_or_spawn` proves DAEMON-03 auto-start against the real compiled binary.**

## Performance

- **Duration:** ~50 min (estimated)
- **Completed:** 2026-07-11
- **Tasks:** 3/3 completed
- **Files modified:** 6 (4 new/substantially-rewritten: `lifecycle.rs`, `autostart.rs`, `server.rs`, `tests/autostart_lifecycle.rs`; 2 small: `registry.rs`, `lib.rs`, `main.rs`)

## Accomplishments

- `lifecycle::{LifecycleConfig, ShutdownSignal, idle_reaper, empty_watcher}` — idle sessions reap via the awaited `Registry::close` (never a bare removal, DAEMON-01 discipline); the empty-registry watcher honors D-31's anti-thrash grace period (a session appearing during the grace window cancels the pending shutdown).
- `autostart::connect_or_spawn` — client-side bind-as-mutex auto-start: connect first, spawn detached on failure, bounded backoff retry (research Pattern 3's exact 50/100/200/400/800ms sequence). Unix-only for now, no PID-file logic.
- `server::run(RunConfig)` — the full assembly: bind (AddrInUse → benign loser exit) → registry + reconciliation-sink construction (env-selected fake connector for offline testing) → `scan_orphans`/`seed_into` BEFORE the accept loop → spawn the lifecycle tasks → accept+authorize+serve until shutdown fires → clean up the socket file.
- `tests/autostart_lifecycle.rs` — the SC#5 [BLOCKING] offline proof, driving the REAL compiled `rdpilot-daemon` binary via `env!("CARGO_BIN_EXE_rdpilot-daemon")`: auto-start via `connect_or_spawn`, a full Connect→List→Disconnect wire round trip against the env-selected fake connector, then a bounded poll proving the daemon self-exits (socket file removed, reconnection refused) once the registry empties past the grace period. Passed 4/4 consecutive runs in ~0.31s each.
- A lighter, always-on regression guard in `server.rs` itself (`run_with_an_immediately_empty_registry_and_a_short_grace_self_shuts_down`) exercises the identical lifecycle assembly without spawning a process, so a regression is caught in every default `cargo test` pass, not only the `--include-ignored` lane.

## Task Commits

Each task was committed atomically:

1. **Task 1: Idle reaper + empty-registry grace-period self-shutdown** — `c52a031` (feat)
2. **Task 2: connect-or-spawn auto-start helper (bind-as-mutex client side)** — `c32b529` (feat)
3. **Task 3: server::run() assembly + auto-start/self-shutdown test — SC#5 [BLOCKING]** — `fa4193d` (feat)

**Plan metadata:** (this commit, `docs(12-06)`, follows this SUMMARY)

## Files Created/Modified

- `crates/rdpilot-daemon/src/lifecycle.rs` — `LifecycleConfig` (env-overridable durations), `ShutdownSignal` (watch-backed), `idle_reaper`, `empty_watcher`
- `crates/rdpilot-daemon/src/autostart.rs` — `connect_or_spawn` (Unix-only)
- `crates/rdpilot-daemon/src/server.rs` — `RunConfig`, `run()`, the env-gated `FakeTestConnector`/`FakeTestSession`, the lighter always-on regression test
- `crates/rdpilot-daemon/src/registry.rs` — added `Registry::live_idle_durations()` (Rule 3 deviation — see below)
- `crates/rdpilot-daemon/src/lib.rs` — re-exports `RunConfig` and (Unix) `connect_or_spawn`
- `crates/rdpilot-daemon/src/main.rs` — `run()` now takes `RunConfig::from_env()`
- `crates/rdpilot-daemon/tests/autostart_lifecycle.rs` — the BLOCKING SC#5 offline integration test

## Decisions Made

- **`send_replace`, not `send`, for `ShutdownSignal::fire`.** `tokio::sync::watch::Sender::send` silently returns `Err` (swallowed by the discarded `Result`) when zero receivers are currently subscribed — exactly the state right after `ShutdownSignal::new()` (which does not retain the constructor's initial receiver). This produced a real, reproducible test failure during authoring (`empty_watcher` called `fire()`, but `is_fired()` read back `false`). Fixed by switching to `send_replace`, which unconditionally updates the held value.
- **Env vars, not a `RunConfig` field, for the well-known socket path.** `ipc::unix::bind` (Plan 12-04, not touched by this plan) always resolves the socket via `BaseDirs::runtime_dir()` → `$XDG_RUNTIME_DIR`. Rather than add a parallel, never-otherwise-exercised parameterized `bind_at(path)` to `ipc::unix.rs` (out of this plan's declared file scope), the integration test isolates itself the same way any other process on the host would: setting `XDG_RUNTIME_DIR` before spawning. This also means every code path the production `bind()` exercises (stale-socket cleanup, `AddrInUse` detection) is exercised by the test in its real, unmodified form.
- **`LocalSet`/`spawn_local` throughout `server::run`, never `tokio::spawn`.** `rdpilot::Session::connect`'s future is not `Send` (documented in `seams.rs`), so `Registry::close`'s awaited `session.close()` chain — and therefore `idle_reaper`, `ipc::serve_connection`, and every per-connection task — is not `Send` either. `run()` wraps everything in a `tokio::task::LocalSet` and drives it via `LocalSet::run_until`, which works regardless of the ambient runtime's flavor — `main.rs`'s `#[tokio::main]` multi-thread runtime is untouched.
- **Self-shutdown proven via socket-file removal + refused reconnect, not a tracked child PID.** `connect_or_spawn`'s signature (`io::Result<UnixStream>`, matching the plan's own spec) never exposes the spawned `Child`. Proving DAEMON-03's self-shutdown via the daemon's own socket cleanup + a subsequent refused connection is an equally strong (arguably cleaner, black-box) proof of the same guarantee.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added `Registry::live_idle_durations()`**
- **Found during:** Task 1 (idle reaper implementation)
- **Issue:** `idle_reaper` needs to identify `Live` sessions whose `last_activity` (an `Instant`) exceeds `idle_timeout`. `Registry` previously exposed only `list()` (credential-free `SessionStatus` DTOs with wall-clock ISO-8601 *strings*, no `Instant`) — there was no accessor `idle_reaper` could use to compute elapsed idle duration.
- **Fix:** Added `pub fn live_idle_durations(&self) -> Vec<(SessionId, Duration)>`, returning each `Live` entry's id paired with `last_activity.elapsed()`. Minimal, additive, does not change any existing `Registry` behavior or the `SessionEntry` enum's shape.
- **Files modified:** `crates/rdpilot-daemon/src/registry.rs`
- **Verification:** New inline test `live_idle_durations_reports_only_live_entries_and_grows_with_elapsed_time` (asserts `Orphaned` entries are excluded and the reported duration is monotonically non-decreasing); full `cargo test -p rdpilot-daemon` green.
- **Committed in:** `c52a031` (Task 1 commit)

**2. [Rule 3 - Blocking] `#[derive(Default)]` for `RunConfig` instead of a manual `impl`**
- **Found during:** Task 3, post-implementation `cargo clippy --all-targets` pass
- **Issue:** `clippy::derivable_impls` flagged the hand-written `impl Default for RunConfig` as mechanically derivable (every field's own `Default` already matches the manual impl's values).
- **Fix:** Replaced the manual `impl` with `#[derive(Default)]` on the struct.
- **Files modified:** `crates/rdpilot-daemon/src/server.rs`
- **Verification:** `cargo clippy -p rdpilot-daemon --all-targets` clean for this file; `cargo test` still green.
- **Committed in:** `fa4193d` (Task 3 commit)

**3. [Rule 3 - Blocking] Module-level `#[allow(clippy::expect_used, clippy::unwrap_used)]` on this plan's own new test modules**
- **Found during:** Task 3, post-implementation `cargo clippy --all-targets` pass
- **Issue:** The crate denies `clippy::expect_used`/`unwrap_used` crate-wide (`lib.rs`), which — when linted with `--all-targets` (never previously run as part of this crate's `<verify>` blocks, which only specify `cargo test`) — also applies inside `#[cfg(test)] mod tests` blocks. This plan's own new test code (`lifecycle.rs`, `autostart.rs`, `server.rs`) used `.expect()` freely, matching every OTHER test module already in this crate (`registry.rs`, `dispatch.rs`, `reconcile.rs`, `ipc/framing.rs`, `ipc/unix.rs` — none of which have ever been clippy-clean under `--all-targets`, a pre-existing, crate-wide condition — see "Discovered but Out-of-Scope" below).
- **Fix:** Added a targeted `#[allow(clippy::expect_used, clippy::unwrap_used)]` directly on each of this plan's three `mod tests` declarations, with a comment explaining the crate-wide precedent. This keeps this plan's own new files clippy-clean under `--all-targets` without touching the dozens of pre-existing violations in files outside this plan's scope.
- **Files modified:** `crates/rdpilot-daemon/src/lifecycle.rs`, `crates/rdpilot-daemon/src/autostart.rs`, `crates/rdpilot-daemon/src/server.rs`
- **Verification:** `cargo clippy -p rdpilot-daemon --target x86_64-unknown-linux-gnu --all-targets` shows zero remaining diagnostics in these three files (confirmed via `grep`).
- **Committed in:** `fa4193d` (Task 3 commit)

---

**Total deviations:** 3 auto-fixed (all Rule 3 — blocking issues discovered while implementing/verifying this plan's own tasks).
**Impact on plan:** All three are small, additive, and necessary for correctness (`live_idle_durations`) or for this plan's own new code to meet the crate's declared lint policy. No scope creep into other plans' files.

## Discovered but Out-of-Scope

`cargo clippy -p rdpilot-daemon --target x86_64-unknown-linux-gnu --all-targets` surfaces ~27 pre-existing `clippy::expect_used`/`clippy::unwrap_used`/`clippy::expect_err` violations in test modules across `registry.rs`, `dispatch.rs`, `reconcile.rs`, `ipc/framing.rs`, and `ipc/unix.rs` (Waves 1-4, Plans 12-02 through 12-05). These predate this plan and are outside its file scope (per the SCOPE BOUNDARY rule — pre-existing issues in unrelated files are not auto-fixed). Root cause: no prior plan's `<verify>` block in this phase has run `cargo clippy --all-targets` (only `cargo test`), so the crate-wide `#![deny(clippy::expect_used)]`/`#![deny(clippy::unwrap_used)]` in `lib.rs` has never actually been enforced against test code. **Recommendation:** a future cleanup task should either (a) add a crate-wide `#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]` in `lib.rs` (the simplest fix, matching how `tests/*.rs` integration-test files — a separate compilation unit — are already exempt), or (b) retrofit per-module allows across the five affected files, and add `cargo clippy --all-targets` to this phase's standard verification gate going forward.

`cargo fmt -p rdpilot-daemon -- --check` also reports widespread pre-existing diffs across nearly every file in this crate (no `rustfmt.toml` exists; default 100-char line width disagrees with the codebase's established wider style). This is consistent across every prior wave's files too, confirming `cargo fmt --check` has likewise never been part of this phase's verification gate. Not touched, for the same reason.

## Issues Encountered

- **`ShutdownSignal::fire()` silently no-op'd on a fresh signal.** Root-caused during Task 1's own test authoring (see "Decisions Made" above) — not a design flaw discovered later, but a genuine bug caught by the plan's own TDD discipline (`empty_watcher_fires_shutdown_after_the_grace_period_elapses_while_still_empty` failed deterministically until the `send_replace` fix landed). No lingering impact — fixed within Task 1's own commit.
- **`tokio::spawn` on `idle_reaper` fails to compile (non-`Send` future).** Expected, per the plan's own binding constraint (`rdpilot::Session`'s connect future is not `Send`) — confirmed the two `idle_reaper` unit tests needed `tokio::join!` (cooperative, same-task polling) instead of `tokio::spawn`, and `server::run` itself needed `LocalSet`/`spawn_local`. No workaround needed beyond following the documented constraint.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- **DAEMON-03 is fully proven OFFLINE** (auto-start on first client connect + idle reap + self-shutdown-on-empty with the anti-thrash grace period), against the REAL compiled binary, not a mock.
- **What remains for Plan 12-07 (the live gate):** (a) the Windows explicit-DACL named-pipe transport (`ipc/windows.rs` is still a `#![cfg(windows)]` stub — DAEMON-02's Windows half, deferred since Plan 12-02); (b) the real remote-liveness orphan confirmation against a genuinely still-connected Windows session (DAEMON-04's live half — this plan's own `crash_restart_reconcile.rs`/DAEMON-04 offline proof only proves the on-disk record survives an unclean drop, not that the remote Windows session is actually still live server-side); (c) an end-to-end live session verify through the daemon (Connect/List/Disconnect against a REAL RDP target via `RealConnector`, not `FakeTestConnector`); (d) the `windows-permissions` crate legitimacy checkpoint (currently `[ASSUMED]`, flagged in research, requires explicit human verification before install per the package-legitimacy protocol).
- The full SC#5 (Phase 12's success criterion 5) combines this plan's offline DAEMON-03 proof with Plan 12-07's live DAEMON-04 remote-liveness proof — Phase 12 is not fully complete until 12-07 lands.
- `rdpilot-daemon`'s `main.rs` is now a real, runnable daemon binary (`RunConfig::from_env()`), ready for Phase 13's CLI to drive via `connect_or_spawn` once the Windows transport (or a Unix-only v1 scope decision) is settled.

---
*Phase: 12-session-daemon*
*Completed: 2026-07-11*

## Self-Check: PASSED

All 7 created/modified source files plus this SUMMARY.md verified present on disk; all 3 task commit hashes (`c52a031`, `c32b529`, `fa4193d`) verified present in `git log --oneline --all`.
