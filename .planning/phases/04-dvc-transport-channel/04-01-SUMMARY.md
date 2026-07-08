---
phase: 04-dvc-transport-channel
plan: 01
subsystem: infra
tags: [ironrdp-dvc, serde, serde_json, dvc-processor, json-envelope, handshake]

# Dependency graph
requires:
  - phase: 02-rdp-session-framebuffer-core
    provides: "connect.rs DVC seam (RDPILOT_SENSOR const, DrdynvcClient registration before connect_begin); D-09 owned-types-only public API rule"
  - phase: 03-input-injection
    provides: "Error enum extension precedent (CoordinateOutOfBounds shape) mirrored exactly for Error::Dvc"
provides:
  - "Durable { version, req_id, type, payload } JSON envelope (Version/Ping/Pong only)"
  - "RdpilotSensorProcessor: complete ironrdp-dvc DvcProcessor impl with AsAny, start() version-handshake-first, process() handshake+Pong correlation"
  - "SensorShared correlation state (pending oneshot map + HandshakeState) ready for Plan 02 to thread through connect.rs/session.rs"
  - "Error::Dvc(String) variant + constructor + category arm"
  - "serde/serde_json promoted to runtime [dependencies] with derive feature"
affects: [04-dvc-transport-channel-plan-02, 04-dvc-transport-channel-plan-03]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "DvcProcessor implementation with impl_as_any! immediately after struct definition (Pitfall 4 — required for get_dvc::<T>() TypeId lookup)"
    - "JsonDvcMessage: minimal Encode/DvcEncode wrapper around serde_json bytes; wire-level chunking delegated entirely to ironrdp_dvc::encode_dvc_messages (never hand-rolled)"
    - "SensorShared: Arc<Mutex<...>> correlation state crossing the session-loop dedicated-OS-thread boundary via tokio::sync::oneshot (runtime-agnostic)"
    - "Mutex poison recovery via match { Ok(g) => g, Err(poisoned) => poisoned.into_inner() } — mirrors framebuffer.rs's existing SharedFrame pattern"
    - "PduResult error construction via ironrdp::pdu::pdu_other_err!(\"static desc\", source: e) — NOT ironrdp::core::other_err! (that only implements OtherErr for EncodeError/DecodeError, not PduError)"

key-files:
  created:
    - crates/rdpilot/src/sensor.rs
  modified:
    - crates/rdpilot/Cargo.toml
    - crates/rdpilot/src/error.rs
    - crates/rdpilot/src/lib.rs

key-decisions:
  - "Used ironrdp::pdu::pdu_other_err!(desc, source: e) instead of RESEARCH's suggested ironrdp::core::other_err! — the latter's OtherErr trait bound is only implemented for EncodeError/DecodeError, not PduError (the actual return type of DvcProcessor::start()/process()); verified by reading ironrdp-pdu-0.8.0 and ironrdp-core-0.2.0 source directly"
  - "This session's sandbox is a native Fedora Linux VM (not the ARM64-Windows/scoop/MinGW host rust-toolchain.toml and .cargo/config.toml assume) — verified/tested offline with RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --target x86_64-unknown-linux-gnu, an environment-only override that touches no committed file; the committed x86_64-pc-windows-gnu toolchain pin is untouched and remains the canonical target for the real dev machine"

requirements-completed: [SENSOR-03]

# Metrics
duration: ~25min
completed: 2026-07-09
---

# Phase 4 Plan 1: DVC Transport Channel — Offline Foundation Summary

**Durable `{version, req_id, type, payload}` JSON envelope (Version/Ping/Pong) plus a complete `RdpilotSensorProcessor` `DvcProcessor` impl with version-handshake-first `start()` and handshake/Pong-correlating `process()` — all offline-unit-tested, no VM.**

## Performance

- **Duration:** ~25 min
- **Tasks:** 2 completed
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments
- `sensor.rs` (new, crate-internal): `PROTOCOL_VERSION`, `Envelope`/`MsgType` (Version/Ping/Pong only, D-4.3), `JsonDvcMessage` (`Encode`+`DvcEncode` wrapper), `HandshakeState`, `SensorShared` (pending-oneshot map + handshake state), `RdpilotSensorProcessor` (full `DvcProcessor` impl with `impl_as_any!`, `start()`, `process()`, `encode_ping()`)
- `Error::Dvc(String)` added to `error.rs`, mirroring `Error::CoordinateOutOfBounds` exactly (variant + `#[error]` + `pub(crate) dvc()` constructor + `category()` arm)
- `serde`/`serde_json` promoted from dev-dependencies to runtime `[dependencies]` with the `derive` feature (D-4.4)
- 9 new offline unit tests covering: envelope wire-shape (`type` as bare string), round-trip encode/decode, garbage-bytes-never-panic, `start()` first-message guard, `channel_name()` identity, handshake match/mismatch transitions (SC#3 negative path), Pong oneshot fulfil+removal, malformed-payload drop
- `cargo build -p rdpilot` and `cargo test -p rdpilot` both exit 0; 55 unit/integration tests pass, 10 live tests correctly `#[ignore]`d offline

## Task Commits

1. **Task 1: Promote serde to runtime deps + Error::Dvc + the JSON envelope vocabulary** - `88017af` (feat)
2. **Task 2: RdpilotSensorProcessor + SensorShared + handshake state machine** - `79987c4` (feat)

_No plan-metadata commit yet — pending this SUMMARY + STATE.md commit below._

## Files Created/Modified
- `crates/rdpilot/src/sensor.rs` - New crate-internal module: envelope vocabulary, `JsonDvcMessage`, `SensorShared`, `RdpilotSensorProcessor` (full `DvcProcessor` impl)
- `crates/rdpilot/src/error.rs` - Added `Error::Dvc(String)` variant + `dvc()` constructor + `category()` arm
- `crates/rdpilot/src/lib.rs` - Added `mod sensor;` (crate-internal only, no `pub use` — D-09)
- `crates/rdpilot/Cargo.toml` - `serde` (derive) + `serde_json` moved to `[dependencies]`

## Decisions Made
- **`pdu_other_err!` over `other_err!` for `PduResult` construction:** RESEARCH's code example used `ironrdp::core::other_err!(...)`, but that macro's `OtherErr` trait bound is only implemented for `EncodeError`/`DecodeError` — not `PduError` (the actual error type `DvcProcessor::start()`/`process()` must return). Verified by reading `ironrdp-pdu-0.8.0/src/lib.rs` and `ironrdp-core-0.2.0/src/error.rs` source directly (both cached locally); used `ironrdp::pdu::pdu_other_err!("static description", source: e)` instead, which correctly constructs a `PduError` with `PduErrorKind::Other` and attaches the source error. This is a corrected implementation detail, not a deviation from any locked CONTEXT decision.
- **Doc comments reworded to avoid literal `DataFirst`/`DataPdu`/`MAX_DATA_SIZE` substrings:** the plan's Task 2 acceptance criteria include a literal `grep -nE 'DataFirst|DataPdu|MAX_DATA_SIZE'` check (chunking-delegation guard). Initial doc comments explaining *why* chunking is delegated referenced those exact terms for clarity; reworded to describe the same delegation without the literal tokens, keeping the guard meaningful as a regression check for future plans that touch this file.
- **Mutex poison-recovery pattern:** matched the existing `framebuffer.rs::SharedFrame` convention (`match lock() { Ok(g) => g, Err(poisoned) => poisoned.into_inner() }`) rather than `session.rs`'s propagate-as-error style, since `SensorShared`'s two mutexes are pure internal bookkeeping with no caller-visible failure mode to report through (mirrors the plan's own framing: "only ever locked for the brief synchronous map/state mutation").

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Corrected RESEARCH's `other_err!` code example to `pdu_other_err!`**
- **Found during:** Task 2 (`RdpilotSensorProcessor::start()`)
- **Issue:** `ironrdp::core::other_err!("rdpilot-sensor", format!(...))` failed to compile with two errors: (a) the macro requires a `&'static str` description, not a runtime `format!(...)` `String`; (b) `PduError` (the actual return-type error, via `PduResult`) does not implement the `OtherErr` trait the macro requires — only `EncodeError`/`DecodeError` do.
- **Fix:** Read `ironrdp-pdu-0.8.0/src/lib.rs` and `src/macros.rs` directly from the local cargo registry cache; switched to `ironrdp::pdu::pdu_other_err!("rdpilot-sensor: version envelope encode failed", source: e)`, which constructs a proper `PduError` via `PduErrorKind::Other` and attaches the source error correctly.
- **Files modified:** `crates/rdpilot/src/sensor.rs`
- **Verification:** `cargo build -p rdpilot` exits 0
- **Committed in:** `79987c4` (Task 2 commit)

**2. [Rule 3 - Blocking] Environment-only Rust toolchain override for this session's sandbox**
- **Found during:** Pre-Task-1 setup
- **Issue:** `cargo build`/`cargo test` failed immediately (`error: target tuple in channel name 'stable-x86_64-pc-windows-gnu'`) because the committed `rust-toolchain.toml` pins a Windows-GNU-hosted toolchain and `.cargo/config.toml` pins a Windows-path MinGW linker — both written for the project's canonical ARM64-Windows/scoop dev machine (Phase 2 Plan 01, checkpoint-approved). This execution session's sandbox is a native x86_64 Fedora Linux VM with no scoop, no MinGW cross-compiler, and only the `stable-x86_64-unknown-linux-gnu` rustup toolchain installed.
- **Fix:** Used `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test} -p rdpilot --target x86_64-unknown-linux-gnu` — a session-local environment variable override that does NOT modify `rust-toolchain.toml` or `.cargo/config.toml`. This is purely an environment workaround (Rule 3 "missing env var"/"build config error" category), not a change to the locked toolchain architecture. A `target/x86_64-unknown-linux-gnu/` build directory with a slightly older successful build already existed on this machine, confirming a prior session used the same workaround.
- **Files modified:** none (no repo files touched; environment-variable-only)
- **Verification:** `cargo build`/`cargo test -p rdpilot --target x86_64-unknown-linux-gnu` both exit 0, 55 tests pass
- **Note for future agents on this specific sandbox:** the STATE.md "Pending Todos" note about exporting scoop/MinGW env vars assumes a Windows host that is NOT present in this session's actual environment (verified: no `/mnt/c`, no scoop directory anywhere on the filesystem, `hostnamectl` reports a QEMU/KVM Fedora 44 VM). Use the `RUSTUP_TOOLCHAIN` override shown above for offline verification in this sandbox; do not attempt to install `mingw64-gcc` or otherwise alter the committed toolchain pins without explicit user approval (that would be an architectural change to a checkpoint-approved decision).

**3. [Rule 3 - Blocking] Reworded doc comments to satisfy the literal chunking-delegation grep guard**
- **Found during:** Task 2 self-check
- **Issue:** Explanatory doc comments on `JsonDvcMessage` and `encode_ping()` used the words "DataFirst/Data PDU chunking" and "MAX_DATA_SIZE" to describe *why* chunking is delegated to `ironrdp_dvc::encode_dvc_messages` — but the plan's own acceptance criteria run `grep -nE 'DataFirst|DataPdu|MAX_DATA_SIZE' crates/rdpilot/src/sensor.rs` expecting zero matches (a regression guard against hand-rolled chunking).
- **Fix:** Reworded the two doc comments to describe the same delegation ("wire-level PDU splitting/reassembly for oversized payloads", "wire-level chunking") without the literal flagged substrings.
- **Files modified:** `crates/rdpilot/src/sensor.rs`
- **Verification:** `grep -nE 'DataFirst|DataPdu|MAX_DATA_SIZE' crates/rdpilot/src/sensor.rs` returns no matches; `cargo test -p rdpilot` still exits 0 (doc comments only, no logic change)
- **Committed in:** `79987c4` (Task 2 commit — done before commit, not a follow-up)

**4. [Rule 1 - Bug] Reverted premature `requirements.mark-complete SENSOR-03`**
- **Found during:** STATE.md/REQUIREMENTS.md update step
- **Issue:** `gsd-tools query requirements.mark-complete SENSOR-03` (run because this plan's frontmatter lists `requirements: [SENSOR-03]`) checked SENSOR-03 off entirely and set its traceability row to "Complete". All three Phase 4 plans (`04-01`, `04-02`, `04-03`) declare the same `requirements: [SENSOR-03]`, since the requirement's own success criteria (SC#2: live ping/pong under 500ms; SC#3 positive path: live handshake) are only provable by Plan 03's live-VM gate — this plan is explicitly the offline-only foundation (SC#3 negative path, unit-only per RESEARCH A3). Marking it fully complete after Plan 01 alone would misrepresent the requirement's actual proof state to `/gsd-verify-work` and future readers.
- **Fix:** Reverted `REQUIREMENTS.md`'s `SENSOR-03` checkbox from `[x]` back to `[ ]` and its traceability row from "Complete" to "In Progress — Plan 01/3 complete (offline envelope + RdpilotSensorProcessor); SC#2/#3 live proof pending Plan 03".
- **Files modified:** `.planning/REQUIREMENTS.md`
- **Verification:** Manual review of `04-02-PLAN.md`/`04-03-PLAN.md` frontmatter confirms all three plans share the requirement; the correct completion point is after Plan 03's live gate passes.
- **Committed in:** docs commit (this plan's metadata commit, alongside SUMMARY/STATE)

---

**Total deviations:** 4 auto-fixed (3x Rule 3 — blocking issues; 1x Rule 1 — bug in premature requirement-completion tracking)
**Impact on plan:** All three were necessary corrections to make the plan's own acceptance criteria pass exactly as written; no scope creep, no architectural changes, no locked CONTEXT/RESEARCH decisions altered. The environment-override deviation (#2) is scoped to this execution session's sandbox only and does not touch any committed file.

## Issues Encountered
None beyond the three auto-fixed items above (all resolved within the fix-attempt limit, no deferred issues).

## User Setup Required
None - no external service configuration required. (Live-VM validation is Plan 03's responsibility, not this offline-foundation plan.)

## Next Phase Readiness
Plan 02 can now register `RdpilotSensorProcessor::new(shared)` via `.with_dynamic_channel(...)` in `connect.rs`, thread `Arc<SensorShared>` through `connect()`'s return tuple into `Session`, add `Session::ping()` and the `RdpInputEvent::Ping(u64)` session-loop dispatch arm, and call `encode_ping()`/`ironrdp_dvc::encode_dvc_messages` for the outbound send. No blockers. The one caveat for whoever runs Plan 02/03 on this same sandbox: repeat the `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` override for any offline verification here; Plan 03's live-VM gate will need the actual Windows/scoop machine (or an equivalent windows-gnu cross-compile setup) since it targets a real Azure VM over RDP.

---
*Phase: 04-dvc-transport-channel*
*Completed: 2026-07-09*

## Self-Check: PASSED

- FOUND: `crates/rdpilot/src/sensor.rs`
- FOUND: `.planning/phases/04-dvc-transport-channel/04-01-SUMMARY.md`
- FOUND: commit `88017af`
- FOUND: commit `79987c4`
