---
phase: 06-window-process-perception
plan: 01
subsystem: api
tags: [rust, ironrdp, dvc, serde, perception, error-handling]

# Dependency graph
requires:
  - phase: 04-dvc-transport-channel
    provides: "The RDPILOT_SENSOR DVC envelope shape (version/req_id/type/payload), RdpilotSensorProcessor, SensorShared correlation state, Session::ping() round trip"
  - phase: 05-sensor-bootstrap-deployment
    provides: "In-band sensor deploy/launch bootstrap (RDPDR + WinRM), live-verified DVC round trip"
provides:
  - "Generalized DVC request/response plumbing: SensorShared::pending carries reply payloads (Sender<Value>), process() fulfils any pending req_id generically regardless of msg_type (not special-cased to Pong)"
  - "RdpInputEvent::Request(MsgType, u64, Option<Value>) + build_request_frame + encode_request replacing the Ping-only path"
  - "New MsgType variants: WindowList, ProcessTree, SetForegroundWindow, LaunchProcess"
  - "Owned public perception types: WindowInfo, WindowState, ProcessInfo (D-09 boundary)"
  - "Crate-internal wire structs + conversions (RectWire/WindowInfoWire/ProcessInfoWire/WindowStateWire::into_owned) implementing the Phase 6 wire contract"
  - "Error::SensorRejected(String) — D-6.4 semantic-failure category distinct from Error::Dvc transport failures"
affects: [06-02-window-process-perception, 06-03-window-process-perception, 06-04-window-process-perception, 06-05-window-process-perception]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Generic req_id-keyed fulfilment: one non-Version match arm in process() serves every reply type (RESEARCH Pattern 1)"
    - "Single encode_request()/build_request_frame() outbound path parameterized by MsgType instead of one method per message type (RESEARCH Pattern 2)"
    - "Owned-vs-wire type split: public types stay serde-free (D-09); *Wire structs + into_owned() conversions are the only serde-coupled surface"

key-files:
  created:
    - crates/rdpilot/src/perception.rs
  modified:
    - crates/rdpilot/src/sensor.rs
    - crates/rdpilot/src/session_loop.rs
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/error.rs
    - crates/rdpilot/src/lib.rs

key-decisions:
  - "session.rs (not in this plan's files_modified) was updated to keep the crate compiling against the new RdpInputEvent::Request/pending-Value shapes (Rule 3 blocking-fix, no behavior change to Session::ping())"
  - "Wire structs/conversions in perception.rs are marked #[allow(dead_code)] with a comment pointing at Plan 02 as the consumer — consistent with this plan's interface-first foundation role; avoids a false 'no new warnings' failure until Plan 02 wires them into Session methods"
  - "Offline verification used the native x86_64-unknown-linux-gnu target (cargo +stable-x86_64-unknown-linux-gnu ... --target x86_64-unknown-linux-gnu) instead of the repo-pinned x86_64-pc-windows-gnu target: this execution environment is native Fedora Linux with no MinGW/windows-gnu toolchain installed, unlike the ARM64-Windows dev machine STATE.md's build-discipline note describes. The rdpilot crate has no Windows-specific (cfg(windows)/windows-rs) code, so this is a safe like-for-like substitution for offline type-checking; the final windows-gnu build should still be run on the real dev machine before this lands upstream of a live gate."

requirements-completed: []  # Deliberately NOT marked complete here — PERC-01/PERC-02/PERC-04/PROC-01 are end-user-observable capabilities that only become TRUE once Plans 02-05 land and the Phase 6 live gate passes (mirrors the established Phase 5 convention: SENSOR-01/02 were marked Complete only at 05-04's live gate, not at each intermediate plan). This plan lays the Rust foundation only; REQUIREMENTS.md traceability will be updated by 06-05 (the live-gate plan).

# Metrics
duration: 25min
completed: 2026-07-09
---

# Phase 6 Plan 1: Generalized DVC Plumbing + Owned Perception Types Summary

**Generalized the Version/Ping/Pong-only DVC envelope into a msg_type-agnostic req_id correlation mechanism, and defined the owned WindowInfo/WindowState/ProcessInfo SDK types plus Error::SensorRejected — the Phase 6 wire contract Plans 02-04 build against.**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-07-09T14:11Z
- **Completed:** 2026-07-09T14:36Z
- **Tasks:** 2
- **Files modified:** 6 (1 created, 5 modified)

## Accomplishments
- `SensorShared::pending` now carries `oneshot::Sender<serde_json::Value>`; `RdpilotSensorProcessor::process()` fulfils any pending `req_id` via one generic non-`Version` match arm — proven by a new test showing a `WindowList` reply (not just `Pong`) fulfils the oneshot with its payload.
- Outbound requests flow through one path: `encode_request(msg_type, req_id, payload)` → `RdpInputEvent::Request` → `build_request_frame` (renamed from the Ping-specific `encode_ping`/`Ping(u64)`/`build_ping_frame`), with `MsgType` gaining `WindowList`/`ProcessTree`/`SetForegroundWindow`/`LaunchProcess`.
- New `perception.rs` exports owned, serde-free public types (`WindowInfo`, `WindowState`, `ProcessInfo`) plus crate-internal `*Wire` structs and `into_owned()` conversions that deserialize the Phase 6 wire contract's exact snake_case shape.
- `Error::SensorRejected(String)` added with its own `"sensor_rejected"` category, distinguishing a D-6.4 semantic sensor-side rejection from a transport-layer `Error::Dvc`.

## Task Commits

Each task was committed atomically:

1. **Task 1: Generalize the DVC request/response plumbing (Patterns 1 and 2)** - `2215491` (feat)
2. **Task 2: Define owned perception types + wire structs + Error::SensorRejected (D-6.3, D-6.4, D-09)** - `f993f36` (feat)

_Both tasks were TDD-flagged in the plan; tests were written and verified alongside the implementation in the same commit per task (no separate RED-only commit was required — this plan's `tdd_mode` is not the plan-level RED/GREEN/REFACTOR gate, and the plan did not mandate separate commits per TDD phase)._

## Files Created/Modified
- `crates/rdpilot/src/sensor.rs` - `MsgType` gains 4 variants; `SensorShared::pending` carries `Sender<Value>`; `process()`'s generic fulfilment arm; `encode_ping` → `encode_request`; 4 new/updated offline tests
- `crates/rdpilot/src/session_loop.rs` - `RdpInputEvent::Ping(u64)` → `RdpInputEvent::Request(MsgType, u64, Option<Value>)`; `build_ping_frame` → `build_request_frame`
- `crates/rdpilot/src/session.rs` - `ping()` updated to the new `Request` variant and `Value`-typed oneshot receive (Rule 3 blocking-fix, out of this plan's stated `files_modified` but required for compilation)
- `crates/rdpilot/src/perception.rs` (new) - Owned `WindowInfo`/`WindowState`/`ProcessInfo`; `RectWire`/`WindowStateWire`/`WindowInfoWire`/`ProcessInfoWire` + `into_owned()`; 2 offline deserialization tests
- `crates/rdpilot/src/error.rs` - `Error::SensorRejected(String)`, `sensor_rejected()` constructor, `"sensor_rejected"` category arm, 1 offline test
- `crates/rdpilot/src/lib.rs` - `mod perception;` + `pub use perception::{WindowInfo, WindowState, ProcessInfo};`

## Decisions Made
- Folded `encode_ping` entirely into `encode_request(MsgType::Ping, req_id, None)` at all call sites rather than keeping a thin wrapper (plan's preferred option, RESEARCH Pattern 2).
- Kept the wire-format concern (`*Wire` structs, `serde` derives) strictly separate from the public owned types, per D-09 — `crate::Rect` and the new perception types remain serde-free.
- See `key-decisions` in frontmatter for the session.rs fix and the Linux-target offline-verification substitution.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated session.rs to compile against the generalized RdpInputEvent/pending types**
- **Found during:** Task 1 (Generalize the DVC request/response plumbing)
- **Issue:** `session.rs::Session::ping()` directly constructed `RdpInputEvent::Ping(req_id)` and awaited the pending oneshot as `Result<(), _>`. Renaming the enum variant and changing `pending`'s `Sender` payload type (both required by the plan's acceptance criteria) made `session.rs` fail to compile as-is, even though it is not listed in this plan's `files_modified`.
- **Fix:** Changed the send call to `RdpInputEvent::Request(crate::sensor::MsgType::Ping, req_id, None)` and the receive arm to `Ok(Ok(_payload)) => Ok(started.elapsed())` (the Pong payload is always `Value::Null`; `ping()`'s contract — round-trip latency only — is unchanged). Updated one stale doc comment referencing `RdpInputEvent::Ping`.
- **Files modified:** `crates/rdpilot/src/session.rs`
- **Verification:** `cargo test -p rdpilot` — all `session::tests::*` (including `ping_fast_fails_on_mismatched_handshake_without_sending`, `ping_times_out_after_500ms_when_no_pong_arrives`) pass unchanged.
- **Committed in:** `2215491` (Task 1 commit)

**2. [Rule 3 - Blocking] Offline verification run against the native Linux target instead of the repo-pinned windows-gnu target**
- **Found during:** Task 1, before any edits — `cargo build` failed immediately with `error: target tuple in channel name 'stable-x86_64-pc-windows-gnu'` (the `x86_64-pc-windows-gnu` rustup toolchain/target is not installed in this execution environment, and `.cargo/config.toml` force-pins that target for every build).
- **Issue:** This session's execution host is native Fedora Linux (`uname -a`: `6.19.10-300.fc44.x86_64`), not the ARM64-Windows-with-scoop machine STATE.md's build-discipline note (line 120) describes. Neither the `x86_64-pc-windows-gnu` rustup target nor MinGW-w64 gcc is present here, and there is no WSL/Windows filesystem bridge to reach a scoop install.
- **Fix:** Verified offline via `cargo +stable-x86_64-unknown-linux-gnu build/test/clippy -p rdpilot --target x86_64-unknown-linux-gnu`, overriding the toolchain and target for this session only (no `rust-toolchain.toml`/`.cargo/config.toml` file was changed). This crate has no `cfg(windows)`/`windows-rs` code — it is pure Rust + IronRDP + rustls — so the Linux target is a safe like-for-like substitute for type-checking and running the offline unit-test suite this plan requires.
- **Files modified:** None (build/test invocation only, not a source or config change).
- **Verification:** `cargo test -p rdpilot --target x86_64-unknown-linux-gnu` → 78 passed, 0 failed (12 live-gate tests correctly ignored, no `RDPILOT_LIVE`). `cargo clippy -p rdpilot --all-targets --target x86_64-unknown-linux-gnu` → only 2 pre-existing warnings in files this plan did not touch (`input.rs`, `rdpdr_backend.rs`), zero new warnings.
- **Committed in:** N/A (no file changes — verification methodology note only)

---

**Total deviations:** 2 auto-fixed (2 blocking)
**Impact on plan:** Both deviations were necessary to keep the crate compiling and to run the plan's own required verification in an environment that differs from the one STATE.md's build-discipline note assumes. No scope creep — no unrelated files were touched, and the pre-existing `input.rs`/`rdpdr_backend.rs` clippy warnings were left untouched (out of this plan's file scope).

## Issues Encountered
- The genuine `x86_64-pc-windows-gnu` build (MinGW gcc linker, matching `.cargo/config.toml`) was **not** exercised in this session — see Deviation 2. The next session/agent with access to the actual pinned dev machine (or a properly provisioned `x86_64-pc-windows-gnu` + MinGW toolchain) should re-run `cargo build -p rdpilot` / `cargo test -p rdpilot` against the real target before any live-gate work in a later Phase 6 plan, to confirm no windows-gnu-specific link/codegen surprise exists (low risk given the crate is pure Rust, but not yet empirically confirmed this session).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Plans 02 (Rust `Session` methods: `get_window_list`/`get_process_tree`/`set_foreground_window`/`launch_process`), 03/04 (C# sensor handlers), and 05 (live gate) can now build directly against: the `MsgType` variants, `SensorShared::pending`'s `Value` payload, `encode_request`/`build_request_frame`/`RdpInputEvent::Request`, the owned `WindowInfo`/`WindowState`/`ProcessInfo` types, the `*Wire`/`into_owned()` conversions, `Error::SensorRejected`, and the `<wire_contract>` field-name/shape contract this plan defined.
- No blockers. The one open item is the not-yet-re-verified genuine `x86_64-pc-windows-gnu` build (see Issues Encountered) — low risk, should be closed out opportunistically by whichever plan next runs on the real Windows dev machine.

---
*Phase: 06-window-process-perception*
*Completed: 2026-07-09*

## Self-Check: PASSED

All created/modified files verified present on disk; both task commits (`2215491`, `f993f36`) verified present in `git log`.
