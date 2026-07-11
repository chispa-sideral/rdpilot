---
phase: 12-session-daemon
plan: 04
subsystem: daemon
tags: [tokio, unix-socket, peer_cred, serde_json, ipc, dispatch]

# Dependency graph
requires:
  - phase: 12-session-daemon (12-02, 12-03)
    provides: rdpilot-daemon crate scaffold (seams.rs traits, error_map.rs), Registry (atomic claim-then-connect open/close/list)
provides:
  - "ipc::unix::{socket_dir, socket_path, bind, accept_and_authorize, authorize_uid, effective_uid} — 0700 runtime-dir Unix listener + per-connection peer-uid gate (DAEMON-02 Unix path)"
  - "ipc::framing::{write_frame, read_frame} — 4-byte-BE length-prefixed serde_json framing with a 16 MiB max-frame-length cap"
  - "ipc::serve_connection — the read_frame -> dispatch -> write_frame per-connection loop Plan 12-06's accept loop will call"
  - "dispatch::dispatch — Request -> Registry -> WireResponse, exhaustive match (no wildcard), including the SESSION-03 list wiring"
  - "SessionEntry::Live now carries connected_since_wall/last_activity_wall (ISO-8601), closing the None-hardcoded list gap registry.rs/seams.rs's own doc comments had flagged as this plan's job"
  - "tests/ipc_security.rs — SC#4 [BLOCKING] DAEMON-02 offline proof (authorize_uid rejection/acceptance + mode-0700 dir), plus a RDPILOT_SECOND_UID-gated real cross-account variant"
affects: [12-06-server-assembly, 12-07-live-gate-windows-dacl, 13-cli-surface, 14-mcp-server-surface]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Pure decision-function extraction (authorize_uid(peer_uid, our_uid) -> io::Result<()>) factored out of the actual accept_and_authorize I/O path, so the security-critical comparison is unit-testable without a real socket or a second OS account"
    - "Stale-socket cleanup via connect-then-unlink-then-rebind (never blind reuse of an on-disk socket path) — a live listener causes AddrInUse, a dead one is removed before rebinding"
    - "Length-prefixed framing generic over any AsyncRead+AsyncWrite, shared verbatim between the Unix UnixStream (this plan) and the future Windows named pipe (Plan 12-07)"
    - "Exhaustive Request match with no wildcard arm in dispatch — a future wire verb added without an explicit arm is a compile error, not a silent runtime gap"

key-files:
  created:
    - crates/rdpilot-daemon/tests/ipc_security.rs
  modified:
    - crates/rdpilot-daemon/src/ipc/unix.rs
    - crates/rdpilot-daemon/src/ipc/mod.rs
    - crates/rdpilot-daemon/src/ipc/framing.rs
    - crates/rdpilot-daemon/src/dispatch.rs
    - crates/rdpilot-daemon/src/registry.rs
    - crates/rdpilot-daemon/src/seams.rs
    - crates/rdpilot-daemon/src/lib.rs

key-decisions:
  - "Extended registry.rs/seams.rs beyond the plan's stated files_modified list (disjoint from the concurrently-executing 12-05 plan's reconcile.rs/tests/crash_restart_reconcile.rs) to finish SESSION-03's list field-completeness — both files' own doc comments explicitly flagged the None-hardcoded connected_since/last_activity as outstanding work for this plan."
  - "authorize_uid is a pure fn(peer_uid, our_uid) -> io::Result<()>, not folded into accept_and_authorize, so the BLOCKING SC#4 decision-logic assertion runs offline with no real socket."
  - "Re-exported accept_and_authorize/authorize_uid/bind/socket_path from lib.rs (cfg(unix)) so tests/ipc_security.rs — a separate integration-test crate — can reach the Unix IPC primitives across the crate boundary; the ipc module itself stays private."
  - "Operational verbs (Ping/Screenshot/LaunchProcess/SetForeground/Put/Get) resolve to SessionNotFound for an unknown session or an explicit Internal not-implemented error for a known one — never a silent no-op, never a wildcard match arm — deferring full wiring to Phase 13/14 per research Open Question 3."

requirements-completed: [SESSION-03]

# Metrics
duration: ~40min
completed: 2026-07-11
---

# Phase 12 Plan 04: Unix IPC Transport + Framing + Dispatch Summary

**Unix `0700` runtime-dir socket with per-connection `peer_cred()` uid authorization, 4-byte-length-prefixed JSON framing, and exhaustive `Request`→`Registry`→`WireResponse` dispatch delivering the SESSION-03 `list` with real wall-clock timestamps.**

## Performance

- **Duration:** ~40 min
- **Tasks:** 3
- **Files modified:** 7 (2 new: `tests/ipc_security.rs`; the plan's own 5 stub files plus `registry.rs`/`seams.rs`/`lib.rs` extended beyond the plan's `files_modified` list)

## Accomplishments

- `ipc::unix`: `socket_dir()` creates `<XDG_RUNTIME_DIR>/rdpilot` (falling back to `cache_dir()`) with mode `0700` applied atomically at creation via `DirBuilder::mode` — never a post-hoc `chmod`. `bind()` performs connect-then-unlink-then-rebind stale-socket cleanup, returning `AddrInUse` if a live daemon already holds the path. `accept_and_authorize()` rejects any accepted connection whose `peer_cred().uid()` does not match the daemon's own `libc::geteuid()` (the crate's one localized, `#[allow(unsafe_code)]`-annotated exception to `#![deny(unsafe_code)]`), via the pure `authorize_uid(peer_uid, our_uid)` decision function.
- `ipc::framing`: `write_frame`/`read_frame` round-trip any `Serialize`/`DeserializeOwned` value over any `AsyncRead`/`AsyncWrite` stream with a 4-byte big-endian length prefix; a declared length over 16 MiB is rejected before any body buffer is allocated (T-12-11).
- `dispatch::dispatch`: routes `Connect`→`Registry::open`, `List{}`→`Registry::list`, `Disconnect`→`Registry::close`, and the six operational verbs (Ping/Screenshot/LaunchProcess/SetForeground/Put/Get) to a `SessionNotFound`/`Internal`-not-implemented decision — an exhaustive match with no wildcard arm, and never logs the raw `Request` (D-31, `Connect` carries a plaintext password).
- **SESSION-03 finished, not just structurally represented:** `SessionEntry::Live` (in `seams.rs`) now carries `connected_since_wall`/`last_activity_wall` ISO-8601 strings alongside its existing monotonic `Instant`s, populated once at insert time in `registry.rs`'s `open()`. `to_status()` now returns `Some(...)` for both fields instead of the previously hardcoded `None` — both files' own doc comments explicitly named this as outstanding work for "Plan 12-03/12-04" to finish. A `dispatch.rs` test and a `registry.rs` test both assert field completeness (non-`None`) for a live session's `list` entry.
- `tests/ipc_security.rs` (SC#4 **[BLOCKING]**, DAEMON-02): `authorize_uid` rejects a peer uid one greater than the daemon's effective uid (`PermissionDenied`) and accepts a matching uid; the socket directory's mode-0700 is asserted as *accompanying* (not sole) evidence, per research Pitfall 5's explicit warning that a stat-only test is insufficient. A real cross-account variant (`sudo -n -u $RDPILOT_SECOND_UID` connecting, then asserting `accept_and_authorize` rejects it) exists gated behind `RDPILOT_SECOND_UID` + `#[ignore]` — never runs in a default `cargo test`, re-confirmed on Windows (DACL/other-account) in the live-gate Plan 12-07.
- `ipc::serve_connection`: the `read_frame::<Request>` → `dispatch` → `write_frame::<WireResponse>` per-connection loop, generic over any stream, ready for Plan 12-06's accept loop to call.

## Task Commits

Each task was committed atomically:

1. **Task 1: Length-prefixed JSON framing + dispatch** — `2b95d21` (feat) — `ipc/framing.rs`, `dispatch.rs`, plus the `registry.rs`/`seams.rs` SESSION-03 wiring it depends on
2. **Task 2: Unix `0700` runtime-dir listener + peer-uid authorization** — `ba451a1` (feat) — `ipc/unix.rs`, `ipc/mod.rs`
3. **Task 3: Different-uid client rejected — SC#4 [BLOCKING]** — `a1b92e5` (test) — `tests/ipc_security.rs`, `lib.rs` (Unix IPC re-exports for the integration-test crate boundary)

## Files Created/Modified

- `crates/rdpilot-daemon/src/ipc/framing.rs` — length-prefixed `serde_json` read/write framing, 16 MiB cap
- `crates/rdpilot-daemon/src/ipc/unix.rs` — `0700` runtime-dir socket + `peer_cred()` uid gate
- `crates/rdpilot-daemon/src/ipc/mod.rs` — cfg-gated Unix/Windows re-exports + `serve_connection`
- `crates/rdpilot-daemon/src/dispatch.rs` — `Request` → `Registry` → `WireResponse` dispatch
- `crates/rdpilot-daemon/src/registry.rs` — `open()` now captures a shared wall-clock timestamp for `connected_since_wall`/`last_activity_wall`
- `crates/rdpilot-daemon/src/seams.rs` — `SessionEntry::Live` gains the two wall-clock fields; `to_status()` populates them
- `crates/rdpilot-daemon/src/lib.rs` — `#[cfg(unix)] pub use ipc::{accept_and_authorize, authorize_uid, bind, socket_path};`
- `crates/rdpilot-daemon/tests/ipc_security.rs` — SC#4 [BLOCKING] offline proof + gated real cross-account variant

## Decisions Made

- Extended `registry.rs`/`seams.rs` beyond the plan's stated `files_modified` list to finish SESSION-03's `list` wiring — both files' own doc comments (from Plans 12-02/12-03) explicitly said this was Plan 12-04's job; the files are disjoint from the concurrently-executing 12-05 plan's `reconcile.rs`/`tests/crash_restart_reconcile.rs`.
- `authorize_uid` is a standalone pure function, not inlined into `accept_and_authorize`, specifically so the BLOCKING SC#4 assertion is offline-testable without a real socket connection.
- Added a small `lib.rs` re-export (`accept_and_authorize`/`authorize_uid`/`bind`/`socket_path`, `cfg(unix)`) — required for `tests/ipc_security.rs` (a separate integration-test crate) to reach these items; the `ipc` module itself remains private, mirroring the existing `Registry`/`DaemonError` re-export pattern.
- Deferred operational verbs return `SessionNotFound` (unknown session) or an explicit `Internal` "not implemented in Phase 12" error (known session) rather than silently dropping the request — keeps the `dispatch` match exhaustive with zero scope creep into Phase 13/14's actual verb implementations.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing Critical Functionality] Finished SESSION-03's `list` wall-clock wiring in `registry.rs`/`seams.rs`**
- **Found during:** Task 1 (framing + dispatch)
- **Issue:** `SessionEntry::to_status()` hardcoded `connected_since: None, last_activity: None` for `Live` entries — the plan's own success criteria (SC#2, "`list` reports ... connected-since, last-activity") and the task's `<behavior>` bullet ("each carrying id/name/host/status/connected_since/last_activity") require real values, and both `registry.rs`/`seams.rs`'s existing doc comments explicitly said finishing this was Plan 12-04's job. `dispatch.rs` alone cannot synthesize these values — the registry is the only place with access to the connect-time wall clock.
- **Fix:** Added `connected_since_wall`/`last_activity_wall: String` (ISO-8601) fields to `SessionEntry::Live`, captured once (a single `iso8601_now()` call, shared with the reconciliation sink's `record_open`) at insert time in `Registry::open()`; `to_status()` now returns `Some(...)` for both.
- **Files modified:** `crates/rdpilot-daemon/src/seams.rs`, `crates/rdpilot-daemon/src/registry.rs`
- **Verification:** New `registry.rs` test `list_reports_connected_since_and_last_activity_for_a_live_session` and `dispatch.rs` test `list_returns_a_session_list_with_every_field_populated` both assert `.is_some()` for both fields; full `cargo test -p rdpilot-daemon` green.
- **Committed in:** `2b95d21` (Task 1 commit)

**2. [Rule 3 - Blocking] Re-exported Unix IPC primitives from `lib.rs` for the integration-test crate boundary**
- **Found during:** Task 3 (SC#4 BLOCKING test)
- **Issue:** `tests/ipc_security.rs` (and its `authorize_uid`/`bind`/`socket_path`/`accept_and_authorize` calls) is compiled as a separate crate that only sees `pub` items reachable from the library's crate root; the `ipc` module and its `unix` submodule are private, so the BLOCKING test could not compile without a re-export.
- **Fix:** Added `#[cfg(unix)] pub use ipc::{accept_and_authorize, authorize_uid, bind, socket_path};` to `lib.rs`, mirroring the crate's existing `pub use registry::Registry; pub use seams::{...};` re-export pattern (both of which already re-export items from otherwise-private modules).
- **Files modified:** `crates/rdpilot-daemon/src/lib.rs`
- **Verification:** `cargo test -p rdpilot-daemon --test ipc_security` compiles and the BLOCKING offline assertions pass.
- **Committed in:** `a1b92e5` (Task 3 commit)

---

**Total deviations:** 2 auto-fixed (1 missing critical functionality, 1 blocking compile-visibility fix)
**Impact on plan:** Both were necessary to satisfy the plan's own stated success criteria (SESSION-03 field completeness) and to make the BLOCKING SC#4 test compile at all. No scope creep — no operational verb was wired, no Windows code was touched, `reconcile.rs` was never touched.

## Issues Encountered

- `cargo clippy -p rdpilot-daemon --all-targets` surfaces pre-existing `clippy::expect_used` violations across the crate's test code (including in `reconcile.rs`, owned by the concurrently-executing 12-05 plan, and in `registry.rs` test code predating this plan) — `#![deny(clippy::expect_used)]` is a crate-level attribute that clippy (but not `cargo build`/`cargo test`) enforces even inside `#[cfg(test)]` modules. This is out of this plan's scope per the executor's SCOPE BOUNDARY rule (pre-existing violations in unrelated code, and `reconcile.rs` is explicitly off-limits) and out of the plan's stated verification (`cargo build` + `cargo test` only, no `cargo clippy` gate). Not fixed; not a regression introduced by this plan (this plan's own new test code follows the same `.expect(...)`-in-tests convention already established by `registry.rs`'s Plan 12-03 tests).

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- `ipc::serve_connection`, `ipc::unix::bind`/`accept_and_authorize`, and `dispatch::dispatch` are all implemented, tested, and ready for Plan 12-06's server assembly (`server.rs`) to wire into a real accept loop on a `LocalSet`/`spawn_local` (the non-`Send` constraint from Waves 1-3 was preserved throughout — no `tokio::spawn` was introduced, no `+ Send` bound was added anywhere in the IPC/dispatch path).
- `ipc/windows.rs` remains a `#[cfg(windows)]`-only stub; the Windows explicit-DACL named-pipe path (`create_with_security_attributes_raw` + `first_pipe_instance(true)`) is deferred to the live-gate Plan 12-07 as planned — untouched by this plan.
- DAEMON-02 is now "In Progress" rather than "Pending" in REQUIREMENTS.md: the Unix path is complete and BLOCKING-tested; only the Windows DACL path remains, scoped to 12-07.
- SESSION-03 is marked Complete in REQUIREMENTS.md.
- No blockers for Plan 12-06.

---
*Phase: 12-session-daemon*
*Completed: 2026-07-11*

## Self-Check: PASSED

All created/modified files confirmed present on disk (`ipc/framing.rs`, `ipc/unix.rs`, `ipc/mod.rs`, `dispatch.rs`, `tests/ipc_security.rs`, this SUMMARY.md). All three task commit hashes (`2b95d21`, `ba451a1`, `a1b92e5`) confirmed present in `git log --oneline --all`.
