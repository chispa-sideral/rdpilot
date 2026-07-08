---
phase: 04-dvc-transport-channel
plan: 02
subsystem: transport
tags: [ironrdp-dvc, active-stage, session-loop, ping-pong, req-id-correlation]

# Dependency graph
requires:
  - phase: 04-dvc-transport-channel
    plan: 01
    provides: "RdpilotSensorProcessor, SensorShared, HandshakeState, encode_ping(), Error::Dvc — all consumed, none re-created here"
provides:
  - "RdpilotSensorProcessor registered via DrdynvcClient::with_dynamic_channel before connect_begin (SC#1 met at the code level)"
  - "connect() returns Arc<SensorShared>; Session stores it plus an AtomicU64 req_id counter"
  - "RdpInputEvent::Ping(u64) session-loop arm: proactive DVC send via ActiveStage::get_dvc + ironrdp_dvc::encode_dvc_messages"
  - "Session::ping() -> Result<Duration>: handshake fast-fail (SC#3) + 500ms timeout bound (SC#2), no ironrdp type leak (D-09)"
affects: [04-dvc-transport-channel-plan-03]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Proactive DVC send from the session loop (not the reactive DvcProcessor) via ActiveStage::get_dvc::<T>() + channel_id() + channel_processor_downcast_ref::<T>(), mirroring IronRDP's own internal ActiveStage::encode_resize (Display Control DVC) pattern"
    - "Block-scoped borrow: the &mut active_stage borrow from get_dvc() is extracted into owned (channel_id, Vec<DvcMessage>) inside a block that ends before the second &mut active_stage call to encode_dvc_messages (Pitfall 3 borrow ordering)"
    - "req_id-keyed oneshot correlation map (SensorShared::pending) crossing the dedicated session-loop OS-thread boundary — oneshot is runtime-agnostic, so Sender/Receiver halves need not share an executor"
    - "tokio::time::timeout(500ms) around the oneshot receiver, with explicit pending-map cleanup on the timeout branch to avoid a leaked entry"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/connect.rs
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/session_loop.rs
    - crates/rdpilot/src/sensor.rs

key-decisions:
  - "SensorShared::pending and SensorShared::handshake fields promoted from module-private to pub(crate): Session::ping() (session.rs) needs the same direct lock access RdpilotSensorProcessor::process() (sensor.rs) already had. Both call sites are already crate-internal-only (D-09), so pub(crate) fields (no getter indirection) match the module's existing internal-only design instead of adding an unnecessary accessor layer."
  - "This session's sandbox is the same native Fedora Linux VM documented in the 04-01 SUMMARY (not the ARM64-Windows/scoop host rust-toolchain.toml assumes) — verified/built/tested offline with RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test} -p rdpilot --target x86_64-unknown-linux-gnu, an environment-only override touching no committed file."

requirements-completed: []

# Metrics
duration: ~35min
completed: 2026-07-09
---

# Phase 4 Plan 2: DVC Transport Channel — Ping Wiring Summary

**Registered `RdpilotSensorProcessor` on `DrdynvcClient` before `connect_begin` and built the full in-process outbound ping path (`RdpInputEvent::Ping` loop arm + `Session::ping() -> Result<Duration>` with handshake fast-fail and a 500ms timeout) — all offline-buildable and offline-unit-tested; the live network round-trip is Plan 03's job.**

## Performance

- **Duration:** ~35 min
- **Tasks:** 2 completed
- **Files modified:** 4 (0 created, 4 modified)

## Accomplishments

- `connect.rs`: `DrdynvcClient::new().with_dynamic_channel(RdpilotSensorProcessor::new(sensor.clone()))` registered before `connect_begin` (SC#1, the hard IronRDP registration-before-connect constraint); `connect()`'s return type is now a 3-tuple whose third element is `Arc<SensorShared>`; the obsolete `let _ = RDPILOT_SENSOR;` placeholder anchor removed
- `session.rs`: `Session` gained `sensor: Arc<SensorShared>` and `next_req_id: AtomicU64` fields (populated from `connect()`'s new return value); every offline test-only `Session { .. }` literal updated via a `test_sensor()` helper so the existing 55 Phase 2/3 tests still compile and pass unchanged
- `session_loop.rs`: `RdpInputEvent::Ping(u64)` variant + `select!` match arm that builds the outbound ping bytes itself (block-scoped `get_dvc::<RdpilotSensorProcessor>()` → `channel_id()` → `channel_processor_downcast_ref` → `encode_ping(req_id)`, borrow ending before `ironrdp_dvc::encode_dvc_messages` + `active_stage.encode_dvc_messages`), yielding a `ResponseFrame` the existing output loop writes — the exact `encode_resize` precedent from RESEARCH Q1, no hand-rolled `DataFirst`/`Data` PDU chunking
- `session.rs`: `Session::ping() -> Result<Duration>` — checks `HandshakeState` first and fails fast with `Error::Dvc` (no send) on `Mismatched` (SC#3); otherwise allocates a `req_id`, inserts a `oneshot::Sender` into `SensorShared::pending`, sends `RdpInputEvent::Ping(req_id)`, and bounds the reply wait with `tokio::time::timeout(Duration::from_millis(500))` (SC#2), removing the leaked pending entry on timeout
- 2 new offline unit tests: a `Mismatched` handshake fast-fails `ping()` without anything appearing on the input channel (SC#3 negative path); a `ping()` call with no responder draining the channel times out at 500ms with an `Error::Dvc` mentioning the timeout (SC#2 bound) — both deterministic, no VM
- `cargo build -p rdpilot` and `cargo test -p rdpilot` both exit 0; 57 unit/integration tests pass (55 carried over + 2 new), 10 live tests correctly `#[ignore]`d offline

## Task Commits

1. **Task 1: Register the sensor processor before connect_begin + return Arc<SensorShared> onto Session (SC#1)** - `12515fa` (feat)
2. **Task 2: RdpInputEvent::Ping arm + Session::ping() with handshake fast-fail + 500ms timeout (SC#2, SC#3)** - `432170b` (feat)

## Files Created/Modified

- `crates/rdpilot/src/connect.rs` - `with_dynamic_channel` registration before `connect_begin`; `connect()` returns `Arc<SensorShared>` as a third tuple element; doc comments updated to reflect the seam is now built, not reserved
- `crates/rdpilot/src/session.rs` - `sensor`/`next_req_id` fields; `Session::ping()`; test literals updated; 2 new offline unit tests
- `crates/rdpilot/src/session_loop.rs` - `RdpInputEvent::Ping(u64)` variant + proactive-send `select!` arm
- `crates/rdpilot/src/sensor.rs` - `SensorShared::pending`/`handshake` fields promoted to `pub(crate)` so `Session::ping()` can share the correlation state

## Decisions Made

- **`SensorShared` field visibility widened to `pub(crate)`:** the plan's `read_first` pointed at `sensor.rs`'s existing types without flagging that `pending`/`handshake` were module-private (no visibility modifier). `Session::ping()` in `session.rs` needs the identical direct-lock access `RdpilotSensorProcessor::process()` already has. Rather than add getter/setter indirection for two fields that are both already crate-internal-only (D-09), the fields were simply marked `pub(crate)` — matching the module's own established "no accessor layer for purely internal bookkeeping" style noted in the 04-01 SUMMARY.
- **Toolchain override repeated from Plan 01:** same `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test} -p rdpilot --target x86_64-unknown-linux-gnu` session-local environment override as Plan 01 documented (this sandbox is a native Fedora Linux VM, not the ARM64-Windows/scoop host the committed toolchain pins target). No repo file touched.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Widened `SensorShared::pending`/`handshake` from private to `pub(crate)`**
- **Found during:** Task 2 (`Session::ping()`)
- **Issue:** `sensor.rs`'s `SensorShared` struct fields had no visibility modifier (private to the `sensor` module), so `session.rs` could not compile `self.sensor.handshake.lock()` / `self.sensor.pending.lock()` as the plan's own `<action>` text directly specifies.
- **Fix:** Added `pub(crate)` to both fields in `crates/rdpilot/src/sensor.rs`. Both call sites remain crate-internal only (module is not re-exported from `lib.rs`, D-09 untouched).
- **Files modified:** `crates/rdpilot/src/sensor.rs`
- **Verification:** `cargo build -p rdpilot` exits 0
- **Committed in:** `432170b` (Task 2 commit)

**2. [Rule 3 - Blocking] Environment-only Rust toolchain override for this session's sandbox**
- **Found during:** Pre-Task-1 setup
- **Issue:** Same as documented in the 04-01 SUMMARY: this sandbox is a native x86_64 Fedora Linux VM with no scoop/MinGW, while `rust-toolchain.toml`/`.cargo/config.toml` pin an ARM64-Windows-GNU host toolchain.
- **Fix:** `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test} -p rdpilot --target x86_64-unknown-linux-gnu` — session-local env var only, no committed file touched.
- **Files modified:** none
- **Verification:** both commands exit 0, 57 tests pass

---

**Total deviations:** 2 auto-fixed (both Rule 3 — blocking issues); no scope creep, no architectural changes, no locked CONTEXT/RESEARCH decision altered.

## Issues Encountered

None beyond the two auto-fixed items above.

## User Setup Required

None. Live-VM validation (the actual ping/pong network round-trip and the throwaway PowerShell responder, D-4.1) is Plan 03's job.

## Next Phase Readiness

Plan 03 can now stand up the throwaway PowerShell DVC responder on the Phase 1 Azure VM and drive `Session::ping()` against it for the live proof of SC#2 (round trip under 500ms) and SC#3's positive path (matching-version handshake completing cleanly over real RDP). No blockers. The one caveat carried forward from Plan 01: this sandbox cannot run the live gate itself (no route to a Windows/ARM64 toolchain or the real Azure VM from here) — Plan 03 needs the actual dev machine or an equivalent windows-gnu cross-compile setup.

---
*Phase: 04-dvc-transport-channel*
*Completed: 2026-07-09*

## Self-Check: PASSED

- FOUND: `crates/rdpilot/src/connect.rs` (with_dynamic_channel + 3-tuple return)
- FOUND: `crates/rdpilot/src/session.rs` (Session::ping, sensor/next_req_id fields)
- FOUND: `crates/rdpilot/src/session_loop.rs` (RdpInputEvent::Ping arm)
- FOUND: `crates/rdpilot/src/sensor.rs` (pub(crate) pending/handshake)
- FOUND: commit `12515fa`
- FOUND: commit `432170b`
