---
phase: 13-cli-surface
plan: 01
subsystem: infra
tags: [tokio, unix-socket, ipc, transport, rdpilot-ipc, rdpilot-daemon]

# Dependency graph
requires:
  - phase: 12-session-daemon
    provides: rdpilot-daemon's autostart.rs (connect_or_spawn/BACKOFF_MS) and ipc module (framing.rs, unix.rs socket_dir/socket_path) built and tested in Plan 12-04/12-06
provides:
  - "rdpilot_ipc::transport module: socket_path/socket_dir, read_frame/write_frame (length-prefixed JSON framing, 16 MiB cap), connect_or_spawn/BACKOFF_MS, TransportError — all reachable from a rdpilot/IronRDP-free crate"
  - "rdpilot_daemon::connect_or_spawn and rdpilot_daemon::socket_path now transparent re-exports of the rdpilot-ipc implementation (stable public API, no breaking change for existing consumers)"
affects: [13-cli-surface (Plan 13-05, the thin rdpilot-cli binary that will link only rdpilot-ipc)]

# Tech tracking
tech-stack:
  added: []
  patterns: ["Shared client<->daemon transport lives in the dependency-free wire-protocol crate (rdpilot-ipc), not the SDK-heavy daemon crate, so any future thin client can reuse it without pulling IronRDP/rustls."]

key-files:
  created: [crates/rdpilot-ipc/src/transport.rs]
  modified: [crates/rdpilot-ipc/src/lib.rs, crates/rdpilot-ipc/Cargo.toml, crates/rdpilot-daemon/src/ipc/mod.rs, crates/rdpilot-daemon/src/ipc/unix.rs, crates/rdpilot-daemon/src/lib.rs]

key-decisions:
  - "Introduced a crate-local TransportError (Io/Encode/Decode variants) in rdpilot-ipc::transport instead of reusing rdpilot-daemon's DaemonError, since rdpilot-ipc cannot depend on rdpilot-daemon (dependency direction runs the other way)."
  - "Added directories + tokio to rdpilot-ipc strictly under [target.'cfg(unix)'.dependencies], not the crate's unconditional [dependencies], preserving a smaller dependency footprint on non-Unix targets pending the deferred Windows named-pipe transport."
  - "Deviation: built/tested against the x86_64-unknown-linux-gnu substitute target (RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --target x86_64-unknown-linux-gnu) instead of the repo's pinned x86_64-pc-windows-gnu toolchain, which is not installed on this host (see rust-toolchain.toml's own note about the ARM64 Windows dev machine this repo is normally built on)."

patterns-established:
  - "A wire-protocol/transport crate that must stay dependency-free of the SDK it serves (D-17 thin-client invariant) gains platform-specific transport deps under `[target.'cfg(unix)'.dependencies]`/`[target.'cfg(windows)'.dependencies]` tables, never the unconditional `[dependencies]` table."

requirements-completed: [CLI-01]

# Metrics
duration: ~15min
completed: 2026-07-11
---

# Phase 13 Plan 01: Relocate auto-start transport into rdpilot-ipc Summary

**Moved socket-path resolution, length-prefixed JSON framing, and the connect_or_spawn auto-start helper out of rdpilot-daemon into a new `rdpilot_ipc::transport` module, so a future thin CLI can reach and auto-start the daemon without ever depending on rdpilot/IronRDP.**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-07-11T07:09:00Z (approx.)
- **Completed:** 2026-07-11T07:13:02Z
- **Tasks:** 2/2 completed
- **Files modified:** 6 (1 created, 5 modified/deleted)

## Accomplishments
- `rdpilot-ipc::transport` now owns `socket_path`/`socket_dir`, `read_frame`/`write_frame` (with the 16 MiB pre-allocation length cap preserved exactly, T-13-01), and `connect_or_spawn`/`BACKOFF_MS` — all six original test cases moved and passing (24/24 `rdpilot-ipc` unit tests green).
- `rdpilot-daemon` deleted `autostart.rs` and `ipc/framing.rs` outright rather than keeping duplicate/forwarding copies, and now consumes the shared implementation directly — closing the socket-path-drift tampering surface (T-13-02) with a single resolver instead of two synchronized copies.
- Every pre-existing public re-export path (`rdpilot_daemon::connect_or_spawn`, `rdpilot_daemon::socket_path`) stayed stable, so both BLOCKING integration tests (`tests/autostart_lifecycle.rs` SC#5 DAEMON-03, `tests/ipc_security.rs` SC#4 DAEMON-02) compiled unchanged and passed against the real compiled `rdpilot-daemon` binary.
- Confirmed `cargo tree -p rdpilot-ipc` carries `directories`/`tokio` but zero `ironrdp`/`rustls` — the thin-client invariant (D-17) holds after gaining the transport code.

## Task Commits

Each task was committed atomically:

1. **Task 1: Create rdpilot-ipc::transport with the relocated framing, socket-path, and connect-or-spawn primitives** - `162c265` (feat)
2. **Task 2: Rewire rdpilot-daemon to consume the relocated primitives and delete the originals** - `c2733e4` (refactor)

**Plan metadata:** commit pending (this step)

## Files Created/Modified
- `crates/rdpilot-ipc/src/transport.rs` - New module: TransportError, MAX_FRAME_LEN/read_frame/write_frame, socket_dir/socket_path, BACKOFF_MS/try_connect/connect_or_spawn, plus all 6 moved tests
- `crates/rdpilot-ipc/src/lib.rs` - `#[cfg(unix)] pub mod transport;` + re-exports of connect_or_spawn/read_frame/write_frame/socket_path/TransportError
- `crates/rdpilot-ipc/Cargo.toml` - New `[target.'cfg(unix)'.dependencies]` table: `directories = "6.0.0"`, `tokio = { version = "1", features = ["rt", "net", "io-util", "time", "macros"] }`
- `crates/rdpilot-daemon/src/autostart.rs` - Deleted (moved to rdpilot-ipc::transport)
- `crates/rdpilot-daemon/src/ipc/framing.rs` - Deleted (moved to rdpilot-ipc::transport)
- `crates/rdpilot-daemon/src/ipc/mod.rs` - `serve_connection` now imports `read_frame`/`write_frame` from `rdpilot_ipc::transport`; `socket_path` re-exported from `rdpilot_ipc::transport` instead of the local `unix` module
- `crates/rdpilot-daemon/src/ipc/unix.rs` - Removed local `socket_dir`/`socket_path` definitions and the `directories::BaseDirs` import; `bind()` now calls `rdpilot_ipc::transport::socket_path()`; `accept_and_authorize`/`authorize_uid`/`effective_uid` untouched (T-13-03: listener-side security stays daemon-only)
- `crates/rdpilot-daemon/src/lib.rs` - Removed `mod autostart;`; `#[cfg(unix)] pub use rdpilot_ipc::connect_or_spawn;` replaces the old `pub use autostart::connect_or_spawn;`; module-map doc comment updated to describe the relocation

## Decisions Made
- Crate-local `TransportError` (not a shared error type) introduced in `rdpilot-ipc::transport` because `rdpilot-ipc` structurally cannot depend on `rdpilot-daemon`'s `DaemonError` — the dependency graph runs the opposite direction. `write_frame`'s serialize/length-prefix failures map to `TransportError::Encode`, all I/O and the oversized-length-cap rejection map to `Io`, and JSON-decode failures map to `Decode` (a slightly finer split than the original single `DaemonError::Io` catch-all, but behaviorally equivalent for every existing test assertion, which only checked `Err(Io(_))` on the I/O-shaped failure paths that remained `Io`).
- New `directories`/`tokio` dependencies added to `rdpilot-ipc` strictly under `[target.'cfg(unix)'.dependencies]`, matching the crate's existing `#[cfg(unix)] pub mod transport;` gating and keeping any future non-Unix build free of transport-only deps until the Windows named-pipe transport lands.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking, toolchain substitution per task instructions] Built/tested on the Linux substitute target instead of the pinned Windows-gnu toolchain**
- **Found during:** Task 1 verification
- **Issue:** `rust-toolchain.toml` pins `stable-x86_64-pc-windows-gnu` (this repo is normally developed on an ARM64 Windows host under x64 emulation, per that file's own comment) — that toolchain/target is not installed on this Linux execution host.
- **Fix:** Ran all build/test/tree verification via `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test,tree} --target x86_64-unknown-linux-gnu`, exactly as instructed by the executor's toolchain/offline note. No source changes resulted from this substitution — the relocated code is target-agnostic (`#[cfg(unix)]`, no Windows-specific paths touched).
- **Files modified:** None (build invocation only).
- **Verification:** `cargo test --workspace --target x86_64-unknown-linux-gnu` green; `cargo test -p rdpilot-daemon --test autostart_lifecycle --target x86_64-unknown-linux-gnu -- --include-ignored` and `--test ipc_security` both green.
- **Committed in:** N/A (no code change, tooling invocation only).

---

**Total deviations:** 1 auto-fixed (1 blocking/toolchain-substitution, explicitly pre-authorized by the executor's own instructions).
**Impact on plan:** No scope creep, no behavior change. The relocation is a pure move as specified — same test assertions, same error semantics (module-local `TransportError` in place of `DaemonError`, mapped 1:1 at every call site), same `#[cfg(unix)]` gating.

## Issues Encountered
- `cargo clippy -p rdpilot-ipc -p rdpilot-daemon --all-targets` surfaces pre-existing `clippy::expect_used`/`unwrap_used` violations in `rdpilot-daemon` test modules (`registry.rs`, `dispatch.rs`, `reconcile.rs`, and the two `.expect()`/`.expect_err()` calls this plan's own `ipc/unix.rs` test module inherited unchanged from before the relocation). These are documented in the codebase's own comments as never having been enforced via `cargo clippy --all-targets` and are out of this plan's scope (Scope Boundary: only auto-fix issues directly caused by this task's changes) — no new violations were introduced by the relocation; the moved `transport.rs` test module carries its own pre-existing `#[allow(clippy::expect_used, clippy::unwrap_used)]` annotation, matching the crate's established convention.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- `rdpilot-ipc::transport::connect_or_spawn`/`socket_path`/`read_frame`/`write_frame` are now available to Plan 13-05 (the thin `rdpilot-cli` binary) without pulling `rdpilot`/IronRDP/rustls — the CLI-01 prerequisite this plan exists for is satisfied.
- No blockers for the remaining Phase 13 plans (13-02 through 13-07).

---
*Phase: 13-cli-surface*
*Completed: 2026-07-11*

## Self-Check: PASSED

- FOUND: crates/rdpilot-ipc/src/transport.rs
- FOUND (correctly absent): crates/rdpilot-daemon/src/autostart.rs deleted
- FOUND (correctly absent): crates/rdpilot-daemon/src/ipc/framing.rs deleted
- FOUND: .planning/phases/13-cli-surface/13-01-SUMMARY.md
- FOUND commit: 162c265 (Task 1: feat(13-01))
- FOUND commit: c2733e4 (Task 2: refactor(13-01))
