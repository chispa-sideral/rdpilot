---
phase: 05-sensor-bootstrap-deployment
verified: 2026-07-09T00:00:00Z
status: passed
score: 4/4 success criteria verified
overrides_applied: 0
---

# Phase 5: Sensor Bootstrap + Deployment Verification Report

**Phase Goal:** The C# NativeAOT sensor helper is built as a self-contained executable, deployed to a real remote Windows target, and confirms the DVC channel is live from its end
**Verified:** 2026-07-09
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (ROADMAP Success Criteria)

| # | Truth (SC) | Status | Evidence |
|---|------------|--------|----------|
| SC1 | `rdpilot-sensor.exe` builds as a NativeAOT self-contained executable, no external runtime dependency | ✓ VERIFIED | `sensor/RdpilotSensor.csproj` sets `PublishAot=true`, `SelfContained=true`, `RuntimeIdentifier=win-x64`, `InvariantGlobalization=true`, `JsonSerializerIsReflectionEnabledByDefault=false`. `sensor/Program.cs` uses `[LibraryImport]` exclusively (4 P/Invoke declarations, zero `[DllImport]`), no COM. `sensor/EnvelopeJsonContext.cs` is a source-generated `JsonSerializerContext` (AOT-safe, no reflection). 05-01-SUMMARY.md records the live publish: exit 0, publish dir contains only `rdpilot-sensor.exe` + `.pdb` (no hostfxr/hostpolicy/coreclr), runs with no .NET runtime installed, 2,699,264 bytes (~2.57 MiB), SHA256-identical VM-built vs. locally retrieved. |
| SC2 | SDK deploys the sensor via drive-redirection copy and launches it within the RDP session (RDPDR primary, mandatory per D-5.6) | ✓ VERIFIED | `connect.rs` registers `ironrdp_rdpdr::Rdpdr` (backed by `crate::rdpdr_backend::RdpilotDriveBackend`) as a sibling static channel to `DrdynvcClient` before `connect_begin`, gated on `cfg.get_sensor_binary_path().is_some()` — and **unconditionally also registers `RdpsndStub`** in the same `if` branch (no separate opt-in/conditional pass exists that would let SC2 be waived, matching D-5.6). `Session::deploy_and_launch` (session.rs) does Win+R inject → settle → chunked typed copy-and-start command referencing `\\tsclient\RDPILOT\<name>` → Enter → poll-and-retry over `Session::ping()`, returning elapsed from the **first successful pong** (not first keystroke), matching D-5.2 semantics. `sensor_rdpdr_deploy_and_ping_within_1s` (live_session.rs) drives this end-to-end and asserts `< Duration::from_secs(1)`. 05-04-SUMMARY.md records the canonical live run: 22.612044 ms. |
| SC3 | When WinRM is available, the WinRM bootstrap path also successfully deploys and launches the sensor | ✓ VERIFIED | `crates/rdpilot/tests/fixtures/deploy-winrm.ps1` exists, copies the real `rdpilot-sensor.exe` (not a throwaway PowerShell responder) via `Copy-Item -ToSession` and arms an AtLogOn Interactive-principal Scheduled Task to launch it in the interactive session. `sensor_winrm_deploy_and_ping_within_1s` (live_session.rs) drives a bounded-retry `ping()` against the WinRM-deployed sensor and asserts `< Duration::from_secs(1)`. 05-04-SUMMARY.md records the canonical live run: 21.62078 ms. |
| SC4 | The deployed sensor opens the RDPILOT_SENSOR DVC channel and responds to a ping within 1 second of launch, on both paths | ✓ VERIFIED | Both gated live tests assert `elapsed < std::time::Duration::from_secs(1)` — `sensor_rdpdr_deploy_and_ping_within_1s` (line ~790-793) and `sensor_winrm_deploy_and_ping_within_1s` (line ~859-861) — measured from the first successful pong per D-5.2 (RDPDR: `deploy_and_launch`'s returned `Duration`; WinRM: the elapsed of the first successful `ping()` call after the AtLogOn setup-retry budget settles, explicitly documented as measured separately from that one-time setup wait). Both canonical measurements (~22.6 ms RDPDR, ~21.6 ms WinRM) are comfortably under the 1s bound. |

**Score:** 4/4 success criteria verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `sensor/RdpilotSensor.csproj` | NativeAOT self-contained win-x64 console project | ✓ VERIFIED | `PublishAot=true`, `SelfContained=true`, `RuntimeIdentifier=win-x64`, `AllowUnsafeBlocks=true` (documented as permitting only the LibraryImport generator's own pinned-pointer marshalling for byte[] params, not hand-written unsafe) |
| `sensor/Program.cs` | WTS-based Version/Ping/Pong server, LibraryImport-only P/Invoke | ✓ VERIFIED | 4 `[LibraryImport("wtsapi32.dll", ...)]` declarations, zero `[DllImport]`, zero COM usage. Handshake-first loop, req_id-correlated Ping→Pong, framing-prefix scan for `{`, malformed JSON dropped (never throws out of `ReadEnvelope`) |
| `sensor/Envelope.cs` + `sensor/EnvelopeJsonContext.cs` | AOT-safe envelope matching Rust wire shape | ✓ VERIFIED | Present; `EnvelopeJsonContext` is a `JsonSerializerContext` (source-generated, reflection-free) |
| `crates/rdpilot/src/connect.rs` | RDPDR + rdpsnd stub static-channel registration before `connect_begin` | ✓ VERIFIED | Lines 120-145: registers `Rdpdr` and `RdpsndStub` together, gated only on `sensor_binary_path` being configured — no separate conditional that could waive RDPDR (D-5.6) |
| `crates/rdpilot/src/session.rs` | `Session::deploy_and_launch` (Win+R inject + poll-retry) | ✓ VERIFIED | Lines 460-519: `SESSION_SETTLE` (3s, one-time), `inject_launch_sequence` (chunked typed command via existing `send_key` path, no hand-built PDUs), poll-and-retry over `ping()`, elapsed measured from first successful pong |
| `crates/rdpilot/src/rdpdr_backend.rs` | `RdpilotDriveBackend` (Create/Close/Read/QueryDirectory + QueryInformation/QueryVolumeInformation) | ✓ VERIFIED | All 11 `ServerDriveIoRequest` variants exhaustively matched; hard path allow-list before any `std::fs` call; QueryInformation/QueryVolumeInformation implemented (live-diagnosed hard requirement, not the plan's original 4-IRP scope) |
| `crates/rdpilot/src/rdpsnd_stub.rs` | Join-only rdpsnd stub required for RDPDR handshake | ✓ VERIFIED | `SvcProcessor`/`SvcClientProcessor` impl, `process()` drops every payload, never replies. Doc comment cites MS-RDPEFS Appendix A footnote <1> as the live-diagnosed root cause for its necessity |
| `crates/rdpilot/src/input.rs` | `Key::Win` variant (Set-1 extended 0x5B) | ✓ VERIFIED | `Key::Win => (true, 0x5B)`; offline test `win_key_maps_to_extended_0x5b` passes |
| `crates/rdpilot/tests/live_session.rs` | Gated `sensor_rdpdr_deploy_and_ping_within_1s` + `sensor_winrm_deploy_and_ping_within_1s` | ✓ VERIFIED | Both present, `#[ignore]`d without `RDPILOT_LIVE`, assert `< 1s` bound |
| `crates/rdpilot/tests/fixtures/deploy-winrm.ps1` | WinRM deploy/arm/teardown for the real sensor exe | ✓ VERIFIED | Present, 11KB, copies the real exe (not a throwaway responder), AtLogOn Interactive Scheduled Task, `-Remove` teardown mode |
| `crates/rdpilot/tests/fixtures/sensor-responder.ps1` | Should be DELETED (Phase 4 throwaway asset, D-5.7) | ✓ VERIFIED ABSENT | Confirmed absent from `crates/rdpilot/tests/fixtures/` (only `deploy-winrm.ps1` present) |
| `crates/rdpilot/tests/fixtures/deploy-responder.ps1` | Should be DELETED (Phase 4 throwaway asset, D-5.7) | ✓ VERIFIED ABSENT | Confirmed absent |

### Key Link Verification

| From | To | Via | Status | Details |
|------|-----|-----|--------|---------|
| `connect.rs` | `ironrdp_rdpdr::Rdpdr` | `connector.with_static_channel(rdpdr)` before `connect_begin` | WIRED | Registered as a sibling static channel to `DrdynvcClient` at the identical connect-time seam |
| `connect.rs` | `RdpilotDriveBackend` | `Rdpdr::new(Box::new(drive_backend), ...)` | WIRED | Backend boxed into the `Rdpdr` processor; `Rdpdr::process()` self-dispatches inbound IRPs internally (confirmed in 05-03-SUMMARY.md, no `session_loop.rs` change needed) |
| `connect.rs` | `RdpsndStub` | `connector.with_static_channel(RdpsndStub::new())` | WIRED | Registered unconditionally alongside `Rdpdr`, not independently toggleable — matches the live-diagnosed structural requirement |
| `session.rs::launch_command()` | `connect.rs::SENSOR_EXE_NAME` | shared `pub(crate) const` | WIRED | Single source of truth for the served/launched filename; regression-guarded by `launch_command_references_redirected_drive_temp_dest_and_start` (offline test asserts the command string contains `SENSOR_EXE_NAME`) |
| `Session::deploy_and_launch` | `Session::send_key` | `inject_launch_sequence` calls `self.send_key(...)` for Win+R, chunks, Enter | WIRED | No hand-built PDU/direct `ironrdp_input` construction — reuses the existing Phase-3 input path |
| `Session::deploy_and_launch` | `Session::ping` | poll-and-retry loop calls `self.ping().await` up to `LAUNCH_ATTEMPTS × PINGS_PER_LAUNCH_ATTEMPT` times | WIRED | Distinguishes transient `Error::Dvc` (keep polling) from other errors (surface immediately) |
| `sensor/Program.cs` | `crates/rdpilot/src/connect.rs::RDPILOT_SENSOR` | channel name string literal `"RDPILOT_SENSOR"` | WIRED | Doc comment explicitly requires byte-for-byte match; both sides confirmed identical |

### Data-Flow Trace (Level 4)

Not applicable in the strict "renders dynamic UI data" sense — Phase 5 delivers a transport/deployment path, not a data-rendering surface. The equivalent trace here is **timing measurement provenance**: `deploy_and_launch`'s returned `Duration` and the WinRM test's `ping()`-loop `Duration` both originate from `Instant::now()` captured immediately before the input-channel send in `Session::ping` (session.rs line 412) and `.elapsed()` on a successful pong (line 420) — not a hardcoded/static value. Both live-run numbers recorded in 05-04-SUMMARY.md (22.612044 ms, 21.62078 ms) are consistent with this codepath and are far below the 500ms per-ping timeout, confirming the numbers are real measured round trips, not clipped/timed-out values.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Offline unit/integration suite passes | `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot --target x86_64-unknown-linux-gnu` | `test result: ok. 72 passed; 0 failed; 0 ignored` (lib+integration) | ✓ PASS |
| Both new gated live tests exist and are correctly ignored offline | same run, `tests/live_session.rs` | `test result: ok. 0 passed; 0 failed; 12 ignored` — includes `sensor_rdpdr_deploy_and_ping_within_1s` and `sensor_winrm_deploy_and_ping_within_1s`, both listed as `ignored, live: requires ... (RDPILOT_LIVE=1)` | ✓ PASS |
| `ironrdp-rdpdr` dependency present, `-native` sibling absent | `grep ironrdp-rdpdr Cargo.toml` | `ironrdp-rdpdr = "0.6"` present; no `ironrdp-rdpdr-native` dependency line | ✓ PASS |
| No unwrap/expect/panic in non-test library code (D-09) | scripted scan of `rdpdr_backend.rs`, `rdpsnd_stub.rs`, `session.rs`, `connect.rs`, `Program.cs` excluding `#[cfg(test)]`/`mod tests` bodies | zero matches outside test modules | ✓ PASS |
| `sensor-responder.ps1` / `deploy-responder.ps1` deleted (D-5.7) | `ls crates/rdpilot/tests/fixtures/` | only `deploy-winrm.ps1` present | ✓ PASS |
| All 15 claimed commit hashes exist in history | `git log --oneline --all \| grep -E "<15 hashes>"` | all 15 found (1fe5582, 491fdbb, 2c1237a, f4a48f3, 95f31e4, c077aea, 7355f36, f383313, 65785bb, bb093ce, d18bbb3, 5d7dc9d, 405f47c, d07e710, e2fedf1) | ✓ PASS |

Live-run measurements (SC2 22.61ms, SC3 21.62ms, SC1 2.57 MiB) were **not** re-run — per task instructions, no VM was provisioned. These are accepted on the strength of the committed code paths that produce them (traced above) plus SUMMARY-documented artifact hashes (SHA256 comparison for SC1).

### Probe Execution

Not applicable — Phase 5 uses the established gated-live-test pattern (`RDPILOT_LIVE` env + `#[ignore]`), not a standalone `scripts/*/tests/probe-*.sh` convention. No conventional probe scripts found under `scripts/`.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|--------------|--------|----------|
| SENSOR-01 | 05-01-PLAN.md | Thin C# .NET 8 NativeAOT sensor helper exposing structured-perception queries | ✓ SATISFIED | `sensor/` project builds NativeAOT self-contained (verified above); Version/Ping/Pong protocol implemented server-side; live SHA256-verified 2.57 MiB artifact |
| SENSOR-02 | 05-02/03/04-PLAN.md | SDK bootstraps/deploys and launches the sensor (drive-redirection primary, WinRM fallback) | ✓ SATISFIED | Both RDPDR (`deploy_and_launch`) and WinRM (`deploy-winrm.ps1`) paths coded, tested via gated live tests, live-verified 22.6ms/21.6ms |

No orphaned requirements found for Phase 5 in REQUIREMENTS.md's traceability table — both SENSOR-01 and SENSOR-02 are correctly mapped and both are claimed by plans in this phase.

### Anti-Patterns Found

Scanned all Phase-5-touched files (`sensor/*`, `crates/rdpilot/src/connect.rs`, `session.rs`, `rdpdr_backend.rs`, `rdpsnd_stub.rs`, `input.rs`, `lib.rs`, `config.rs`, `error.rs`, `Cargo.toml`, `tests/live_session.rs`, `tests/fixtures/deploy-winrm.ps1`) for `TBD|FIXME|XXX|TODO|HACK|PLACEHOLDER` and placeholder-language patterns.

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `crates/rdpilot/src/rdpdr_backend.rs` | 612 | string literal `"rdpilot-rdpdr-red-placeholder"` | ℹ️ Info | Test-only temp-dir naming convention for a RED-phase TDD test path (never a served file, never touched by real code) — not a stub marker, no functional meaning |
| `crates/rdpilot/src/error.rs` | 59 | doc comment `"placeholder for the session loop introduced by a later plan"` | ℹ️ Info | Pre-existing doc comment from an earlier phase (Phase 2/3), not touched by Phase 5 — the "later plan" it refers to already landed; stale wording, out of scope for this phase's verification, no functional impact |

No blocker-level debt markers found. Both hits are informational and do not affect goal achievement.

### Human Verification Required

None. All four success criteria have direct code evidence plus a documented, SHA256/wire-trace-corroborated live run recorded in the SUMMARYs; no visual/UX/subjective judgment call remains for this phase's scope (transport + deployment, not perception rendering).

### Non-Blocking Observations (carried forward per plan)

- **Empirically-tuned timing constants** (`SESSION_SETTLE=3s`, `RUN_DIALOG_SETTLE=800ms`, `TYPE_CHUNK_LEN=16`, `TYPE_CHUNK_GAP=150ms`, `LAUNCH_ATTEMPTS=3`, `PINGS_PER_LAUNCH_ATTEMPT=20`) are documented in code comments as live-tuned against one specific VM/Windows-build combination, not root-caused to a specific Windows readiness signal. Explicitly flagged as a residual concern in 05-04-SUMMARY.md ("may need revisiting on a differently-provisioned target"). Non-blocking — this is the same empirical-tuning pattern already established and accepted in Phases 3/4.
- **`RdpsndStub` is a permanent architectural addition**, not a temporary hack — intentionally non-functional (channel presence only, no audio redirection), required by MS-RDPEFS Appendix A footnote <1> for RDPDR to start at all. Documented as such in 05-04-SUMMARY.md and in the module's own doc comment. Non-blocking; correctly scoped as permanent, not framed as a stub to be later "completed."

### Gaps Summary

No gaps found. All four ROADMAP success criteria are backed by both static code evidence (registration, wiring, no-unwrap discipline, artifact existence, throwaway-fixture deletion) and the offline test suite (72/72 passing, live tests correctly gated/ignored). The live-run numbers claimed in SUMMARY.md/ROADMAP.md (SC1 2.57 MiB, SC2 22.61ms, SC3 21.62ms, SC4 <1s both paths) are consistent with the code paths that would produce them and were not re-run per the task's explicit no-VM-provisioning instruction.

---

*Verified: 2026-07-09*
*Verifier: Claude (gsd-verifier)*
