---
phase: 05-sensor-bootstrap-deployment
plan: 03
subsystem: rdp-transport
tags: [ironrdp-rdpdr, rdpdr, drive-redirection, launch-bootstrap, win+r, rust]

# Dependency graph
requires:
  - phase: 05-sensor-bootstrap-deployment
    provides: "Plan 02's Key::Win, RdpilotDriveBackend (05-02-SUMMARY.md) -- this plan wires both into the live connect/launch path"
  - phase: 03-input-injection
    provides: "Session::send_key(KeyAction::Combo/Type) -- deploy_and_launch's inject_launch_sequence reuses this, never hand-builds a PDU"
  - phase: 04-dvc-transport-channel
    provides: "Session::ping() 500ms-bounded round trip -- deploy_and_launch's poll-and-retry loop calls this repeatedly"
provides:
  - "connect::connect registers ironrdp_rdpdr::Rdpdr as a sibling static channel to DrdynvcClient (before connect_begin, SC#1 seam), gated on ConnectionConfig::get_sensor_binary_path() being Some"
  - "ConnectionConfig::sensor_binary_path(impl Into<PathBuf>) builder + get_sensor_binary_path() -> Option<&Path> getter (owned, D-09)"
  - "crate::connect::SENSOR_EXE_NAME shared constant tying the RDPDR-announced filename and the launch command's copy source together"
  - "Session::deploy_and_launch(&self) -> Result<Duration> -- Win+R inject + poll-and-retry (D-5.1/D-5.2)"
  - "Error::Bootstrap(String) + Error::bootstrap() constructor + \"bootstrap\" category"
affects: [05-04-PLAN, "any future consumer of the live RDPDR launch path"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "RDPDR registered as a SIBLING static channel to DrdynvcClient at the identical connect-time seam, not routed through it -- ironrdp_rdpdr::Rdpdr fully implements SvcProcessor/SvcClientProcessor and self-dispatches inbound MS-RDPEFS IRPs to the registered backend; ActiveStage::process drives it automatically, exactly like the existing drdynvc static channel. No session_loop.rs change was needed."
    - "Shared crate::connect::SENSOR_EXE_NAME constant (not two independent string literals) so the RDPDR backend's announced filename (connect.rs) and the launch command's UNC copy source (session.rs) structurally cannot drift apart."
    - "deploy_and_launch's poll-and-retry loop distinguishes Err(Error::Dvc(_)) (transient -- keep polling within the current launch attempt) from any other Err (channel closed, lock poisoned -- surface immediately, never silently retried)."

key-files:
  created: []
  modified:
    - crates/rdpilot/src/config.rs
    - crates/rdpilot/src/connect.rs
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/error.rs

key-decisions:
  - "Task 1 read_first resolution (RESEARCH/plan open question): read at execution time from the pinned ironrdp-rdpdr-0.6.0 source (src/lib.rs). Rdpdr::new(backend: Box<dyn RdpdrBackend>, computer_name: String) -> Self and with_drives(mut self, initial_drives: Option<Vec<(u32, String)>>) -> Self match the plan's assumed signature exactly -- no deviation needed. Rdpdr implements both SvcProcessor (channel_name/compression_condition/process) and the empty marker trait SvcClientProcessor, so connector.with_static_channel(rdpdr) type-checks identically to the existing DrdynvcClient registration."
  - "Confirmed Rdpdr::process() self-dispatches: SharedHeader decode -> match on RdpdrPdu variant -> handle_device_io_request(dev_io_req, &mut src) -> (device_type lookup) -> the registered RdpdrBackend's handle_drive_io_request. This is fully internal to the ironrdp-rdpdr crate; no manual dispatch arm was added to session_loop.rs, and none was needed -- ActiveStage::process drives every registered static channel (drdynvc and rdpdr alike) the same way."
  - "Introduced crate::connect::SENSOR_EXE_NAME = \"rdpilot-sensor.exe\" as the single source of truth for the served/launched filename, referenced by both connect.rs's RdpdrDriveBackend construction and session.rs's launch_command() helper -- avoids the two call sites silently drifting apart (a plan directive: \"the served filename must match the name announced by the RDPDR backend\")."
  - "deploy_and_launch's poll-and-retry constants: LAUNCH_ATTEMPTS = 3, PINGS_PER_LAUNCH_ATTEMPT = 20 (each ping() call bounded at its own unchanged 500ms hard timeout, so ~10s of polling per launch attempt, ~30s total outer budget across all 3 attempts). Rationale: comfortably exceeds the ~10s WTSVirtualChannelOpenEx open-retry window the sensor's own DVC-open race is expected to need (Phase 4 RESEARCH Pitfall 1 / 04-03-SUMMARY.md, carried forward to the C# sensor as a hard constraint by D-5.7), while 3 re-injections gives resilience against a one-off missed keystroke/focus issue without an unbounded retry. These are offline-reasoned defaults; Plan 04's live gate is where they get empirically tuned if the real VM's timing differs."
  - "RUN_DIALOG_SETTLE = 300ms between the Win+R combo and typing the command -- gives the remote Run dialog time to render before keystrokes are injected, mirroring the empirical-tuning-target pattern already established for DOUBLE_CLICK_GAP/DRAG_STEP_GAP in this same file (Phase 3, RESEARCH-confirmed against the real VM in Plan 04's predecessor)."
  - "Chose NOT to write an offline unit test that exercises deploy_and_launch's full ping-exhaustion path (would require ~30s of real wall-clock ping timeouts per the constants above) -- the plan explicitly scopes offline testing to the pure launch_command() helper and Error::Bootstrap rendering, deferring the full inject-then-poll round trip to Plan 04's live gate. Instead added a fast (sub-second) offline test that exercises deploy_and_launch's error-propagation path via a closed input channel, proving it surfaces a non-Bootstrap error immediately without ever entering the ping-poll loop, and never hangs/panics (T-05-08)."

patterns-established: []

requirements-completed: [SENSOR-02]

# Metrics
duration: ~20min
completed: 2026-07-09
---

# Phase 5 Plan 3: RDPDR Live Wiring + Launch Bootstrap Summary

**Registered `ironrdp_rdpdr::Rdpdr` as a sibling static channel to `DrdynvcClient` at connect time (gated on a configured sensor path) and implemented `Session::deploy_and_launch` -- the Win+R inject + poll-and-retry launch bootstrap (D-5.1/D-5.2) -- completing the SDK-side primary deployment path for SENSOR-02.**

## Performance

- **Duration:** ~20 min
- **Tasks:** 2 (both auto)
- **Files modified:** 4 (`config.rs`, `connect.rs`, `session.rs`, `error.rs`), 0 created

## Accomplishments

- `ConnectionConfig` gained an owned `sensor_binary_path` builder + `get_sensor_binary_path` getter (D-09), carrying the local path of the sensor exe to serve; `None` by default keeps the connect path byte-for-byte unchanged from pre-Phase-5 behavior.
- `connect::connect` registers `ironrdp_rdpdr::Rdpdr` (announcing a `RDPILOT` drive backed by `RdpilotDriveBackend`, Plan 02) as a sibling static channel to `DrdynvcClient`, at the identical `connect_begin`-preceding seam, only when a sensor path is configured.
- **`with_static_channel`/`Rdpdr` API-signature read resolution:** read at execution time from the pinned `ironrdp-rdpdr-0.6.0` source (`src/lib.rs`). `Rdpdr::new(backend: Box<dyn RdpdrBackend>, computer_name: String) -> Self` and `with_drives(mut self, initial_drives: Option<Vec<(u32, String)>>) -> Self` match the plan's assumed signature exactly -- **no deviation needed**. `Rdpdr` implements `SvcProcessor` (`channel_name`/`compression_condition`/`process`) plus the empty marker `SvcClientProcessor`, so `connector.with_static_channel(rdpdr)` type-checks identically to the existing `DrdynvcClient` registration.
- **Auto-dispatch confirmed:** `Rdpdr::process()` fully self-dispatches -- decodes the `SharedHeader`, matches the `RdpdrPdu` variant, and for `DeviceIoRequest` calls `handle_device_io_request` -> (device-type lookup) -> the registered `RdpdrBackend`'s `handle_drive_io_request`. This is entirely internal to `ironrdp-rdpdr`; **no `session_loop.rs` manual dispatch arm was added or needed** -- `ActiveStage::process` drives every registered static channel (drdynvc and rdpdr alike) the same way.
- `Session::deploy_and_launch(&self) -> Result<Duration>` implemented: `inject_launch_sequence` (Win+R -> settle -> typed copy-and-start command -> Enter, entirely via the existing `Session::send_key` path -- no separate PDU-building code) followed by a poll-and-retry loop over `Session::ping()`. The elapsed returned is from the **first successful pong**, not the first keystroke (SC4 semantics, D-5.2).
- `Error::Bootstrap(String)` + `Error::bootstrap()` constructor + `"bootstrap"` category added, mirroring the existing `Error::Dvc` shape.
- Introduced `crate::connect::SENSOR_EXE_NAME` as the single source of truth for the served/launched filename, referenced by both the RDPDR backend construction (`connect.rs`) and `launch_command()` (`session.rs`) -- structurally prevents the two from drifting apart.

## Task Commits

Each task was committed atomically:

1. **Task 1: Register the RDPDR static channel at connect time + carry the sensor-exe path in config**
   - `65785bb` (feat) -- `ConnectionConfig::sensor_binary_path`/`get_sensor_binary_path`; `connect::connect` registers `Rdpdr` as a sibling static channel to `DrdynvcClient`, gated on a configured sensor path.
2. **Task 2: Session::deploy_and_launch -- Win+R inject + poll-and-retry (D-5.1/D-5.2)**
   - `bb093ce` (feat) -- `Error::Bootstrap`; `Session::deploy_and_launch` + `inject_launch_sequence` + pure `launch_command()` helper; shared `SENSOR_EXE_NAME` constant added to `connect.rs`.

## Files Created/Modified

- `crates/rdpilot/src/config.rs` -- `sensor_binary_path` field + builder + `get_sensor_binary_path` getter, `Debug` impl updated, 2 new/updated unit tests
- `crates/rdpilot/src/connect.rs` -- doc comment updated with the RDPDR seam rationale; `SENSOR_EXE_NAME` const added; RDPDR static-channel registration gated on `cfg.get_sensor_binary_path()`
- `crates/rdpilot/src/session.rs` -- `LAUNCH_ATTEMPTS`/`PINGS_PER_LAUNCH_ATTEMPT`/`RUN_DIALOG_SETTLE` consts, pure `launch_command()` helper, `Session::deploy_and_launch` + `inject_launch_sequence` methods, 2 new unit tests
- `crates/rdpilot/src/error.rs` -- `Error::Bootstrap` variant, `Error::bootstrap()` constructor, `"bootstrap"` category arm, 1 new unit test module

## Decisions Made

See frontmatter `key-decisions` for the full list. Summary: the `Rdpdr::new`/`with_drives` signature matched the plan's assumption exactly (no deviation); `Rdpdr` self-dispatches IRPs internally so no `session_loop.rs` change was needed; a shared `SENSOR_EXE_NAME` constant was introduced to prevent the announced-filename/launch-command drift the plan explicitly warned against; `deploy_and_launch`'s poll-and-retry constants (3 attempts x 20 polls x 500ms = ~30s total) were reasoned from Phase 4's empirically-documented ~10s `WTSVirtualChannelOpenEx` retry window, to be tuned live in Plan 04 if needed; the full ping-exhaustion path is deliberately left offline-untested (would cost ~30s of real wall-clock time per test run) per the plan's explicit "the full inject-then-poll loop is exercised live in Plan 04" scoping -- a fast closed-channel error-propagation test covers the "never hangs/panics" (T-05-08) property instead.

## Deviations from Plan

None -- plan executed exactly as written. The `read_first` API-signature confirmation matched the plan's stated assumption with zero drift (a pleasant contrast to Plan 02's `ServerDriveIoRequest` 11-vs-4-variant surprise).

## Auth Gates

None -- this plan is entirely offline Rust work; no external service configuration or authentication was needed.

## Known Stubs

None. Both `sensor_binary_path`/`get_sensor_binary_path` and `deploy_and_launch` are fully wired, not placeholders -- they are exercised end-to-end only by Plan 04's live gate (by design, per this plan's explicit offline scope), not because any part of the implementation is missing.

## Live-Gate Watch-Items for Plan 04

- **Carried forward from 05-02 (unchanged by this plan):** `RdpilotDriveBackend`'s `Create` handler infers file-vs-directory purely from the normalized path (empty = root dir), not from `create_options`/`desired_access` (e.g. `FILE_DIRECTORY_FILE`). Sufficient for this plan's offline scope; Plan 04's live gate should confirm a real Windows RDP client's actual `Create` sequence against the redirected drive root behaves as expected.
- **`deploy_and_launch`'s poll-and-retry constants are offline-reasoned, not live-tuned:** `LAUNCH_ATTEMPTS = 3`, `PINGS_PER_LAUNCH_ATTEMPT = 20` (~30s total outer budget) were derived from Phase 4's documented ~10s `WTSVirtualChannelOpenEx` retry-window finding, extrapolated to the RDPDR in-band launch path. If the live gate finds the C# sensor's own DVC-open race, cold-start latency, or AV/EDR scan time exceeds this budget, these constants are the first place to widen (mirroring the Phase 4 precedent of `RETRY_BUDGET` growing from 15s to 60s after live diagnostics, `04-03-SUMMARY.md`).
- **`RUN_DIALOG_SETTLE = 300ms` is an offline-reasoned default**, not empirically confirmed against a real Windows Run dialog's render latency -- if Plan 04 finds keystrokes are being typed before the dialog has focus, this is the tuning knob (same pattern as `DOUBLE_CLICK_GAP`/`DRAG_STEP_GAP` in this file, which were themselves live-tuned in a Phase 3 predecessor to this plan).
- **The launch command's `cmd /c copy \\tsclient\RDPILOT\<name> %TEMP%\<name> && start "" %TEMP%\<name>` shape is unverified against a real `RDPILOT` redirected-drive mount** -- Plan 04's live gate is the first point this string is ever actually typed into a live Run dialog and executed by a real `cmd.exe`.
- **D-5.6 reminder (mandatory for phase completion):** RDPDR-path success (SC2) is not conditionally waivable -- Plan 04 must prove the full connect -> RDPDR-registered -> Win+R-launched -> ping/pong-within-1s chain on the real lab VM.

## Threat Flags

None beyond what this plan's own `<threat_model>` already covers (T-05-06/07/08) -- no new network endpoints, auth paths, or trust-boundary schema changes were introduced beyond the RDPDR static-channel registration and the in-band launch-command construction the plan explicitly scoped.

## Self-Check: PASSED

All claimed files exist on disk and all claimed commit hashes (`65785bb`, `bb093ce`) are present in `git log --oneline --all`. `cargo build -p rdpilot` and `cargo test -p rdpilot` both exit 0 offline under `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu --target x86_64-unknown-linux-gnu` (67 unit/integration tests pass, 11 live tests correctly `#[ignore]`d).

## Self-Check: PASSED (re-verified post-write)

All 5 claimed files present on disk; both claimed commit hashes present in `git log --oneline --all`.

---
*Phase: 05-sensor-bootstrap-deployment*
*Completed: 2026-07-09*
