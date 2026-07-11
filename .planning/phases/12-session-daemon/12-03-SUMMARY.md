---
phase: 12-session-daemon
plan: 03
subsystem: rdpilot-daemon
tags: [registry, concurrency, thread-safety, soak-test]
dependency-graph:
  requires:
    - rdpilot_daemon::{SessionConnector, ManagedSession, ReconciliationSink, SessionEntry, DaemonError} (12-02)
  provides:
    - rdpilot_daemon::Registry (open/close/list/seed_orphan/len/is_empty)
    - rdpilot_daemon::registry::generate_auto_id (D-29, private fn, tested via registry:: inline tests)
  affects:
    - crates/rdpilot-daemon/src/dispatch.rs (Plan 12-04, will call Registry::open/close/list)
    - crates/rdpilot-daemon/src/server.rs (Plan 12-06, production wiring of Registry with RealConnector)
tech-stack:
  added: []
  patterns:
    - "Atomic claim-then-connect: synchronous claim under Mutex, connect OUTSIDE the lock, re-lock to upgrade/rollback (research Pattern 1)"
    - "Close-not-drop teardown: every registry removal path awaits session.close() (research Pattern 2)"
    - "tokio::task::LocalSet + spawn_local for concurrency tests, since SessionConnector futures are not Send"
    - "Per-test dedicated Tokio runtime with short thread_keep_alive to avoid blocking-pool false-positive leak signals in soak tests"
key-files:
  created:
    - crates/rdpilot-daemon/tests/registry_concurrency.rs
    - crates/rdpilot-daemon/tests/thread_leak_soak.rs
  modified:
    - crates/rdpilot-daemon/src/registry.rs
    - crates/rdpilot-daemon/src/lib.rs (pub use registry::Registry)
decisions:
  - "Registry exported from the crate's public API (pub use registry::Registry) so integration tests (and later Plan 12-04/06) can construct it"
  - "SC#1 concurrency test uses tokio::task::LocalSet + spawn_local, not tokio::spawn, per 12-02's BoxFuture non-Send finding"
  - "SC#3 soak test builds its own Tokio runtime per test with thread_keep_alive=10ms and serializes the two tests via a shared Mutex, to avoid Tokio blocking-pool lingering threads and cross-test thread-count contamination within one process"
metrics:
  duration: "~90m"
  completed: "2026-07-11"
---

# Phase 12 Plan 03: Registry — Atomic Insert + Close-Not-Drop + SC#1/SC#3 Proofs Summary

Implemented the in-memory session registry (`Registry`): atomic claim-then-connect insert, close-not-drop teardown, D-29 auto-id minting, and a credential-free `list` snapshot — then proved the two hardest offline-testable success criteria: N-simultaneous-same-name concurrency (SC#1, exactly one winner) and the thread+RSS return-to-baseline soak (SC#3, BLOCKING).

## What was built

**Task 1 — `registry.rs`:** `Registry { sessions: Mutex<HashMap<SessionId, SessionEntry>>, connector: Arc<dyn SessionConnector>, sink: Arc<dyn ReconciliationSink> }`. `open(name, host, cfg)` implements research Pattern 1 exactly: `claim()` synchronously inserts a `Connecting` placeholder under the lock (Occupied -> `DuplicateSession`, Vacant -> insert) with the lock released before any `.await`; `connector.connect(cfg).await` then runs with no lock held; on success the registry re-locks to upgrade to `Live`, then calls `sink.record_open` outside the lock; on failure it re-locks to remove the claim so the name is immediately reusable. `close(id)` implements Pattern 2: extracts the entry under a scoped lock, then matches — `Live` awaits `session.close()` before calling `sink.record_closed` (never a bare drop), `Connecting` puts the placeholder back and returns `StillConnecting` (never interrupts an in-flight connect), `Orphaned` reclaims cleanly with no close call, `None` is `SessionNotFound`. `generate_auto_id()` mints short adjective/noun pairs (D-29) via a `DefaultHasher`-mixed seed (nanos + pid + monotonic counter, no new crate); `claim_with_auto_id()` reuses the exact same `claim()` atomic-insert path, retrying up to 32 times on collision (research "Don't Hand-Roll": no second "generate and hope" path). `iso8601_now()`/`civil_from_days()` are a small, offline-testable, dependency-free ISO-8601/RFC-3339 UTC formatter (Howard Hinnant's reference civil-calendar algorithm) feeding `ReconciliationSink::record_open`. 12 inline tests cover every `<behavior>` bullet from the plan.

**Task 2 — `tests/registry_concurrency.rs` (SC#1):** N=16 concurrent `open(Some("same-name"), ..)` calls against a `SlowFakeConnector` (sleeps 5ms before succeeding, widening the race window) yield exactly 1 `Ok` and 15 `DuplicateSession` rejections, with the registry holding exactly one entry afterward — verified deterministic across 5 repeated runs. A second test proves N=16 unnamed opens racing concurrently mint N distinct auto-ids with zero collisions leaking through.

**Task 3 — `tests/thread_leak_soak.rs` (SC#3, BLOCKING):** `ThreadOwningFakeSession` spawns a real `std::thread` on connect that parks on a channel `recv()` until `close()` sends the stop signal and joins it via `spawn_blocking` (mirroring `rdpilot::Session::close`'s exact join shape). Baseline/after thread count comes from a filtered read of `/proc/self/status`'s `Threads:` line; RSS from `sysinfo::System::refresh_processes` + `Process::memory()`. A fast N=3 smoke test runs in the default suite; the full N=50 soak is `#[ignore]`-gated (matching `crates/rdpilot/tests/live_session.rs`'s convention), asserting thread count returns EXACTLY to baseline and RSS within an 8 MiB tolerance band (allocator retention).

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu` — 25 lib tests + 2 concurrency tests + 1 fast soak smoke, all green (29/29 non-ignored).
- `... --test thread_leak_soak -- --include-ignored` — 2/2 green (full N=50 soak included), verified stable across 3 repeated runs.
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build --workspace --target x86_64-unknown-linux-gnu` — full workspace builds clean.
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --workspace --target x86_64-unknown-linux-gnu` — all crates (rdpilot, rdpilot-ipc, rdpilot-config, rdpilot-daemon) green.
- Manual grep gate: `crates/rdpilot-daemon/src/registry.rs`'s `guard.remove(id)` (line 256, the `close()` extraction) is immediately followed by `session.close().await?` on the `Live` arm (line 261) — confirmed via `grep -n '\.remove(\|\.close()\.await'`.

## Deviations from Plan

**1. [Rule 3 — blocking issue] SC#1's concurrency test uses `tokio::task::LocalSet` + `spawn_local`, not `tokio::spawn`.**

- **Found during:** Task 2 (a direct, expected consequence of Plan 12-02's already-documented `BoxFuture` non-Send finding — see `12-02-SUMMARY.md`).
- **Issue:** The plan's suggested shape ("`tokio::task::JoinSet` or `futures::future::join_all` via `tokio::spawn`") requires the spawned future to be `Send`. Since `Registry::open` awaits `SessionConnector::connect`, whose returned future is deliberately not `Send` (12-02 finding), a task that awaits `registry.open(..)` cannot be hosted by a bare `tokio::spawn`.
- **Fix:** Both concurrency tests drive their N tasks via `tokio::task::LocalSet::run_until` + `tokio::task::spawn_local`, which explicitly supports `!Send` futures. This is still genuinely concurrent (cooperative interleaving across `.await` points on one OS thread) — exactly what proves the atomic-insert race, since the fake connector's sleep-then-succeed shape widens the window so every task reaches the claim step before any completes. Verified deterministic across 5 repeated runs.
- **Files modified:** `crates/rdpilot-daemon/tests/registry_concurrency.rs`; also added `pub use registry::Registry;` to `crates/rdpilot-daemon/src/lib.rs` (the module was previously private, blocking any external integration test from reaching it).
- **Commit:** `1248659`

**2. [Rule 1 — bug, live-diagnosed during authoring] SC#3 soak test needed a dedicated per-test Tokio runtime with a short `thread_keep_alive`, and the two tests in the file needed serialization.**

- **Found during:** Task 3, first test run.
- **Issue A (blocking-pool lingering threads):** `Registry::close`'s `spawn_blocking(move || thread.join())` call (mirroring `Session::close`'s own join shape, as the plan specifies) runs on Tokio's blocking thread pool, whose threads linger for Tokio's 10-SECOND default keep-alive after finishing work before being torn down. The initial implementation (using `#[tokio::test]`'s default runtime with a 20ms settle) observed `after=3, baseline=2` on the very first run — a false-positive "leak" caused by a still-lingering, harmless blocking-pool thread, not the actual `ThreadOwningFakeSession` thread under test.
- **Fix A:** Each test now builds its OWN `tokio::runtime::Builder::new_multi_thread()` with `.thread_keep_alive(Duration::from_millis(10))`, and settles 200ms after the cycle loop — long enough for the short keep-alive to expire and the blocking-pool thread to be reaped, so the assertion observes the TRUE post-cycle thread count.
- **Issue B (cross-test contamination):** After fixing A, running both tests together (`-- --include-ignored`) still failed with `after=4 < baseline=7` — `/proc/self/status`'s `Threads:` is a WHOLE-PROCESS metric, and Rust's default test harness runs `#[test]` functions in PARALLEL within one process; the fast-smoke test's own still-shutting-down runtime threads were captured as part of the soak test's "baseline" reading.
- **Fix B:** Added a shared `static SOAK_SERIALIZE: std::sync::Mutex<()>` that both tests acquire for their full duration, serializing them against each other within this test binary (each `tests/*.rs` file is its own process, so this does not affect `registry_concurrency.rs` or the lib's inline tests).
- **Verification:** Both tests green across 3 repeated runs of `--include-ignored`, and stable as part of the default (non-ignored) suite and the full workspace test run.
- **Files modified:** `crates/rdpilot-daemon/tests/thread_leak_soak.rs`.
- **Commit:** `2ecd639`

**Offline/target substitution (environment note, not a deviation from plan content):** All verification ran on `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu --target x86_64-unknown-linux-gnu` — this host has no `x86_64-pc-windows-gnu` target installed, exactly as Phases 10/11 and this plan's own `<verify>` blocks specify. The soak test's Linux-native `/proc/self/status` thread-count read is explicitly the offline-testable path the research/plan designed for; the Windows equivalent is out of this plan's scope (deferred to a future live/pinned-machine gate per research).

## Known Stubs

None — `Registry` is fully implemented and tested per the plan's scope; `dispatch.rs`/`server.rs` (still stubs from Plan 12-02, unchanged by this plan) are the intended future consumers, per the plan's own module ownership map.

## Threat Flags

None — this plan's `<threat_model>` (T-12-06 non-atomic insert, T-12-07 thread/RSS leak, T-12-08 credential leak via `list`) is fully covered by the implementation and its tests exactly as described; no new surface outside that register was introduced.

## Self-Check: PASSED

- FOUND: crates/rdpilot-daemon/src/registry.rs (contains `struct Registry`, `.close().await` after `.remove(` on the Live arm, `generate_auto_id`)
- FOUND: crates/rdpilot-daemon/tests/registry_concurrency.rs (spawns N=16 concurrent same-name opens via LocalSet/spawn_local)
- FOUND: crates/rdpilot-daemon/tests/thread_leak_soak.rs (reads `/proc/self/status` Threads:, uses sysinfo, N=50 `#[ignore]`-gated + N=3 default smoke)
- FOUND commit c539116 (Task 1: registry.rs)
- FOUND commit 1248659 (Task 2: registry_concurrency.rs)
- FOUND commit 2ecd639 (Task 3: thread_leak_soak.rs)
