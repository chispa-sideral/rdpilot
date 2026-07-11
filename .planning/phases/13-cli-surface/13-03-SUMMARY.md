---
phase: 13-cli-surface
plan: 03
subsystem: daemon
tags: [rust, tokio, async-trait-objects, mutex, deadlock-avoidance, session-registry]

# Dependency graph
requires:
  - phase: 12-session-daemon
    provides: "ManagedSession trait (close/describe only), SessionEntry/Registry storage, DaemonError, SessionConnector"
  - phase: 13-cli-surface (13-02, concurrent)
    provides: "rdpilot-ipc::Request gained six new session-scoped verbs (WindowList/ProcessList/Uia/WorldState/Mouse/Key) that dispatch.rs's exhaustive match had to be extended to cover"
provides:
  - "ManagedSession extended with 12 operational &self methods (screenshot/world_state/get_window_list/get_process_tree/get_uia_tree/send_mouse/send_key/set_foreground_window/launch_process/upload_file/download_file/ping), impl for rdpilot::Session"
  - "Registry storage changed to Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>> with a lock-free status snapshot for list/to_status"
  - "Registry::call<T>: deadlock-free operation dispatch (clone Arc under sync lock, drop lock, await inner tokio Mutex)"
  - "Canned-value FakeTestSession in server.rs so Plan 13-06's offline CLI-02 proof has real renderable data"
affects: [13-04-dispatch-wiring, 13-06-cli-perception-proof, 14-mcp-server-surface]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Arc<tokio::sync::Mutex<Option<Box<dyn Trait>>>> for reclaiming ownership of an unsized trait object from behind shared ownership (Box is what moves, never the trait object)"
    - "Registry::call: clone-under-sync-lock, drop-lock, then await-inner-tokio-lock — never hold a std::sync::MutexGuard across an .await on this daemon's single-LocalSet-OS-thread model"

key-files:
  created: []
  modified:
    - crates/rdpilot-daemon/src/seams.rs
    - crates/rdpilot-daemon/src/registry.rs
    - crates/rdpilot-daemon/src/dispatch.rs
    - crates/rdpilot-daemon/src/server.rs
    - crates/rdpilot-daemon/src/lifecycle.rs
    - crates/rdpilot-daemon/tests/registry_concurrency.rs
    - crates/rdpilot-daemon/tests/thread_leak_soak.rs
    - crates/rdpilot-daemon/tests/crash_restart_reconcile.rs

key-decisions:
  - "BoxFuture (seams.rs) widened from private to pub(crate) so server.rs/registry.rs can name the exact type the trait declares for the new &self methods"
  - "server.rs's FakeTestSession fully fleshed out with canned values in Task 1's commit (ahead of its Task 2 plan slot) because it is not #[cfg(test)]-gated and blocked Task 1's own `cargo build` verify gate"
  - "dispatch.rs's exhaustive Request match extended to route the concurrently-added WindowList/ProcessList/Uia/WorldState/Mouse/Key verbs through the existing not_implemented_for deferred stub -- no dispatch wiring added, that stays Plan 13-04's job"
  - "CLI-02/CLI-03 marked In Progress (not Complete) in REQUIREMENTS.md -- this plan builds the daemon-side seam only; dispatch is not wired to it until 13-04"

patterns-established:
  - "Pattern: unsized trait object behind shared ownership always wraps the sized Box in Option so ownership can be .take()n later, never attempts Arc::try_unwrap/into_inner on a dyn Trait directly"

requirements-completed: []  # CLI-02/CLI-03 remain In Progress -- see REQUIREMENTS.md; this plan is infrastructure-only (no dispatch wiring), so neither requirement is fully satisfied yet.

# Metrics
duration: ~40min
completed: 2026-07-11
---

# Phase 13 Plan 03: Daemon Operational Seam + Deadlock-Free Registry::call Summary

**Extended `ManagedSession` with the full `&self` operational method set and moved registry session storage to `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` behind a new deadlock-free `Registry::call`, closing Phase 12's two pre-existing architecture gaps.**

## Performance

- **Duration:** ~40 min
- **Tasks:** 2
- **Files modified:** 8 (`crates/rdpilot-daemon/src/{seams,registry,dispatch,server,lifecycle}.rs` + `crates/rdpilot-daemon/tests/{registry_concurrency,thread_leak_soak,crash_restart_reconcile}.rs`)

## Accomplishments

- `ManagedSession` trait extended with 12 operational `&self` methods (screenshot, world_state, get_window_list, get_process_tree, get_uia_tree, send_mouse, send_key, set_foreground_window, launch_process, upload_file, download_file, ping), each returning the crate's manual `BoxFuture<'_, Result<T, DaemonError>>` shape with **no** `+ Send` bound (the real `Session`'s futures are not `Send` — preserved verbatim per the crate's own non-Send/`LocalSet` execution model).
- `impl ManagedSession for rdpilot::Session` delegates each one-line to the matching SDK method, mapping errors via the existing `DaemonError::Sdk` variant — identical pattern to `RealConnector::connect`'s existing body.
- `SessionEntry::Live.session` changed from `Box<dyn ManagedSession>` to `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` — the naive `Arc<dyn ManagedSession>` + `Arc::into_inner`/`try_unwrap` design does not compile (`dyn ManagedSession` is unsized; those APIs require `T: Sized`). A new `status: SessionLifecycle` field snapshots the lifecycle at insert time so `SessionEntry::to_status`/`Registry::list` never take the inner `tokio::sync::Mutex` at all.
- `Registry::call<T>` added: clones the `Arc` under the synchronous outer `std::sync::Mutex`, **drops the guard**, then `.await`-locks the inner `tokio::sync::Mutex` before running the caller's operation — the outer lock is never held across an `.await`, matching the research-verified deadlock-free shape exactly. Calls to the same session serialize; calls to different sessions never block each other (pre-satisfies Phase 14's MCP-06).
- `Registry::close`'s `Live` arm now `.take()`s the sized `Box` out of the inner `Option` (never moves the unsized trait object itself) and awaits `close()` exactly as before — close-not-drop (DAEMON-01) preserved bit-for-bit; a concurrent double-close (session already taken) degrades to a clean success rather than a panic.
- Every `ManagedSession` test double in the crate updated to compile against the widened trait: the non-test `FakeTestSession` in `server.rs` (used by the real compiled binary's `RDPILOT_DAEMON_TEST_CONNECTOR` path) now returns plausible, non-empty canned values for every operational verb, giving Plan 13-06's offline CLI-02 rendering proof real data to render against; every other fake (registry.rs/dispatch.rs/lifecycle.rs inline `#[cfg(test)]` fakes plus the three `tests/*.rs` integration-test fakes) returns trivial canned/`Ok(())` values.

## Task Commits

1. **Task 1: Extend the ManagedSession trait with operational &self methods and implement them for rdpilot::Session** — `a078296` (feat)
2. **Task 2: Change registry storage to Arc<TokioMutex<Option<Box>>>, add Registry::call, and update every ManagedSession test fake** — `d390f5e` (feat)

_Note: Task 1's commit also includes the minimal fixes required to satisfy its own `cargo build -p rdpilot-daemon` verify gate (see Deviations) — `server.rs`'s FakeTestSession and `dispatch.rs`'s exhaustive match, both originally scoped to Task 2/out-of-plan._

## Files Created/Modified

- `crates/rdpilot-daemon/src/seams.rs` — `ManagedSession` trait extended with 12 operational methods + `impl for Session`; `SessionEntry::Live` storage changed to `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` + `status: SessionLifecycle` snapshot; `BoxFuture` widened to `pub(crate)`
- `crates/rdpilot-daemon/src/registry.rs` — `open` wraps the connected session in `Arc::new(Mutex::new(Some(session)))`; `close`'s `Live` arm `.take()`s the Box before awaiting `close()`; new `pub async fn call<T>(...)` deadlock-free dispatch helper
- `crates/rdpilot-daemon/src/dispatch.rs` — deferred-verb match arm extended to cover 13-02's six new `Request` variants (routed through the existing `not_implemented_for` stub, no wiring); inline test fake updated
- `crates/rdpilot-daemon/src/server.rs` — `FakeTestSession` fully implemented with plausible canned operational values (2x1 `Screenshot`, one `WindowInfo`/`ProcessInfo`/`UiaElement`, fixed PID 4242, fixed `TransferOutcome`)
- `crates/rdpilot-daemon/src/lifecycle.rs` — inline test fake `FakeSession` updated with trivial canned stubs
- `crates/rdpilot-daemon/tests/registry_concurrency.rs`, `tests/thread_leak_soak.rs`, `tests/crash_restart_reconcile.rs` — each gained a local `OpFuture<'a, T>` alias (lifetime-parameterized, since `BoxFuture` is `pub(crate)` and these are separate external-crate integration-test binaries) and trivial canned stubs on their respective `ManagedSession` fakes

## Decisions Made

- `BoxFuture` widened from private to `pub(crate)` in `seams.rs` so `server.rs`/`registry.rs` (same crate) can name the exact trait-declared return type for the new `&self` methods without redefining an incompatible local alias.
- `server.rs`'s `FakeTestSession` was fully fleshed out with canned values inside Task 1's commit rather than Task 2's, because it is NOT `#[cfg(test)]`-gated (it backs the runtime `RDPILOT_DAEMON_TEST_CONNECTOR` env-var path) and therefore blocked Task 1's own `cargo build -p rdpilot-daemon` verify gate. Doing the full canned-value implementation once (rather than a trivial stub now + real values later) avoided touching the same block twice.
- CLI-02/CLI-03 marked "In Progress" (not "Complete") in `REQUIREMENTS.md` — this plan is infrastructure-only; the actual dispatch wiring onto these new `ManagedSession` methods is Plan 13-04's scope, so neither requirement's user-visible behavior changed yet.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `dispatch.rs`'s exhaustive `Request` match needed arms for six concurrently-added verbs**
- **Found during:** Task 1 (`cargo build -p rdpilot-daemon` verify step)
- **Issue:** The concurrently-running Plan 13-02 added `WindowList`/`ProcessList`/`Uia`/`WorldState`/`Mouse`/`Key` to `rdpilot_ipc::Request` after this plan's context (and `13-RESEARCH.md`) was captured. `dispatch.rs`'s deliberately-exhaustive `match req { ... }` (no wildcard arm, a documented forcing function) failed to compile against the widened enum.
- **Fix:** Added the six new variants to the existing deferred-verb match arm, routing them through `not_implemented_for` exactly like the pre-existing Phase-12 verbs (`Ping`/`Screenshot`/`LaunchProcess`/`SetForeground`/`Put`/`Get`). No dispatch behavior was wired — this stays Plan 13-04's job.
- **Files modified:** `crates/rdpilot-daemon/src/dispatch.rs`
- **Verification:** `cargo build -p rdpilot-daemon` succeeds; `cargo test -p rdpilot-daemon` still green (existing dispatch tests unaffected — no arm's behavior changed, only match exhaustiveness).
- **Committed in:** `a078296` (Task 1 commit)

**2. [Rule 3 - Blocking] `server.rs`'s non-test `FakeTestSession` needed the 12 new trait methods to compile at all**
- **Found during:** Task 1 (`cargo build -p rdpilot-daemon` verify step)
- **Issue:** `FakeTestSession` (and `FakeTestConnector` that produces it) live in `server.rs`'s main module body, NOT inside `#[cfg(test)] mod tests` — they back the runtime `RDPILOT_DAEMON_TEST_CONNECTOR` env-var connector-selection path that `tests/autostart_lifecycle.rs` exercises against the real compiled binary. `cargo build` (which does NOT skip this code, unlike genuine `#[cfg(test)]` fakes) failed with `E0046: not all trait items implemented`.
- **Fix:** Implemented all 12 methods on `FakeTestSession` immediately, with plausible non-empty canned values (originally Task 2's stated scope for exactly this fake) rather than a placeholder stub, since Task 2 would otherwise have needed to touch the same block a second time.
- **Files modified:** `crates/rdpilot-daemon/src/server.rs`
- **Verification:** `cargo build -p rdpilot-daemon` succeeds; `cargo test -p rdpilot-daemon --test autostart_lifecycle -- --include-ignored` (real-binary auto-start/self-shutdown lifecycle) still passes.
- **Committed in:** `a078296` (Task 1 commit)

**3. [Rule 3 - Blocking] Every remaining `ManagedSession` test fake in the crate needed the 12 new methods**
- **Found during:** Task 2 (`cargo test -p rdpilot-daemon --no-run`)
- **Issue:** Beyond the plan's stated Task 2 scope (registry.rs/dispatch.rs inline fakes), `lifecycle.rs`'s own inline `#[cfg(test)]` fake and three separate `tests/*.rs` integration-test crates (`registry_concurrency.rs`, `thread_leak_soak.rs`, `crash_restart_reconcile.rs`) each define their own local `ManagedSession` fake and failed to compile once the trait grew.
- **Fix:** Added trivial canned-value stub methods to every one of them. The three `tests/*.rs` integration-test files (separate compilation units, external to the crate) needed a local `OpFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>` type alias alongside their existing `TestFuture<T>` (implicitly `'static`, used by `close`/`connect`) since `BoxFuture` is `pub(crate)` and not visible from outside the crate.
- **Files modified:** `crates/rdpilot-daemon/src/lifecycle.rs`, `crates/rdpilot-daemon/tests/registry_concurrency.rs`, `crates/rdpilot-daemon/tests/thread_leak_soak.rs`, `crates/rdpilot-daemon/tests/crash_restart_reconcile.rs`
- **Verification:** `cargo test -p rdpilot-daemon --no-run` succeeds; full `cargo test -p rdpilot-daemon -- --include-ignored` (minus the pre-existing `RDPILOT_SECOND_UID`-gated test) passes, including the DAEMON-01 50-cycle thread/RSS soak and the SC#1 registry-concurrency test.
- **Committed in:** `d390f5e` (Task 2 commit)

---

**Total deviations:** 3 auto-fixed (all Rule 3 — blocking build/test-compile issues; two caused by the concurrently-running sibling plan 13-02 widening `rdpilot_ipc::Request`, one caused by a non-`#[cfg(test)]` fake this plan's own trait extension broke)
**Impact on plan:** All three necessary to satisfy the plan's own stated verify gates (`cargo build`/`cargo test -p rdpilot-daemon` green). No functional dispatch wiring was added anywhere — every deferred verb (old and newly-covered) still resolves to `not_implemented_for`/a canned test value; Plan 13-04 remains the plan that wires real dispatch behavior. No scope creep beyond what was required to compile.

## Issues Encountered

None beyond the deviations documented above.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- The daemon now has a live, deadlock-free operational seam (`Registry::call` + the extended `ManagedSession` trait) onto every held session, ready for Plan 13-04 to wire the actual `dispatch.rs` arms for `Ping`/`Screenshot`/`LaunchProcess`/`SetForeground`/`Put`/`Get`/`WindowList`/`ProcessList`/`Uia`/`WorldState`/`Mouse`/`Key`.
- `server.rs`'s `FakeTestSession` already returns realistic canned values for every verb, so Plan 13-06's offline CLI-02 integration proof can render real, non-empty tables the moment 13-04 wires dispatch — no further fake-session work needed there.
- Plan 13-04 also still needs to resolve the pre-existing Phase-12 `share_root`-not-configured-on-`Connect` gap (research Pitfall 6, `dispatch.rs`'s `Request::Connect` arm) before `Put`/`Get` can functionally succeed — flagged in `13-RESEARCH.md`, out of this plan's scope.
- No blockers for 13-04. This plan's file scope (`rdpilot-daemon` only) never touched `rdpilot-ipc`, respecting the concurrency boundary with Plan 13-02 (which finished first and already landed the six new `Request`/`WireResponse` variants this plan had to accommodate).

---
*Phase: 13-cli-surface*
*Completed: 2026-07-11*

## Self-Check: PASSED

- FOUND: commit `a078296` (Task 1)
- FOUND: commit `d390f5e` (Task 2)
- FOUND: `crates/rdpilot-daemon/src/seams.rs`
- FOUND: `crates/rdpilot-daemon/src/registry.rs`
- FOUND: `crates/rdpilot-daemon/src/dispatch.rs`
- FOUND: `crates/rdpilot-daemon/src/server.rs`
- FOUND: `crates/rdpilot-daemon/src/lifecycle.rs`
- FOUND: `crates/rdpilot-daemon/tests/registry_concurrency.rs`
- FOUND: `crates/rdpilot-daemon/tests/thread_leak_soak.rs`
- FOUND: `crates/rdpilot-daemon/tests/crash_restart_reconcile.rs`
- FOUND: `pub async fn call` in `registry.rs`
