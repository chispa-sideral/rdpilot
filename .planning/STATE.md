---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: completed
stopped_at: Phase 9 Plan 4 (09-04) COMPLETE — terminal v1 live gate PASSED against a real disposable Azure VM (sensor AOT-rebuilt, all 4 PROOF-01 SCs proven live). PROOF-01 retired. v1.0 milestone CLOSED.
last_updated: "2026-07-10T15:30:00.000Z"
last_activity: "2026-07-10 -- 09-04 COMPLETE: terminal v1 live gate PASSED. Reused the existing healthy rdpilot-vm (no reprovision); mandatory sensor AOT-rebuild on the VM from current 09-02 source (SHA256 41a35f8c..., confirmed byte-identical VM-built vs relayed, confirmed different from the Phase 8 cache 7a775b99...). All four PROOF-01 SCs PASS against the real 7-Zip File Manager: SC#1 screenshot 1920x1080; SC#2 deeper UiaScope::Subtree walk found 30 named elements, latency 130.2ms after live-tuning SC2_MAX_DEPTH 4->3 (555.2ms->130.2ms, zero coverage loss, under the Phase 7 500ms budget); SC#3 navigation click + verified screenshot-diff change (after live-diagnosing and fixing a missing set_foreground_window call before the click -- Rule 1); SC#4 examples/proof_harness printed PROOF: PASS and exited 0. One documented first-RDP-login deploy_and_launch transient self-resolved on retry. VM torn down and confirmed absent. PROOF-01 retired in REQUIREMENTS.md; Phase 9 marked complete in ROADMAP.md; v1.0 milestone CLOSED. See 09-04-SUMMARY.md."
progress:
  total_phases: 9
  completed_phases: 9
  total_plans: 35
  completed_plans: 35
  percent: 100
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-06-04)

**Core value:** A local AI agent can connect to a remote Windows desktop over RDP and read/inspect a program that is only reachable via RDP — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.
**Current focus:** v1.0 milestone COMPLETE — all 9 phases (35/35 plans) done, PROOF-01 retired, terminal live gate PASSED 2026-07-10.

## Current Position

Phase: 9 (scripted-proof-harness) — COMPLETE
Plan: 4 of 4 complete
Status: v1.0 MILESTONE CLOSED. 09-04 terminal live gate PASSED (2026-07-10): all four PROOF-01 success criteria proven live against a real disposable Azure VM running a freshly AOT-rebuilt sensor and the real 7-Zip File Manager. No further Phase 9 work outstanding.
Last activity: 2026-07-10 -- 09-04 COMPLETE: terminal v1 live gate PASSED against a real disposable Azure VM (reused, healthy). Sensor AOT-rebuilt on the VM from current 09-02 source (SHA256 41a35f8c..., confirmed different from the Phase 8 cache 7a775b99...). SC#1 screenshot 1920x1080; SC#2 deeper UiaScope::Subtree walk found 30 named elements at 130.2ms (live-tuned SC2_MAX_DEPTH 4->3, zero coverage loss, under the Phase 7 500ms budget); SC#3 navigation click + verified change (after a live-diagnosed set_foreground_window fix); SC#4 PROOF: PASS, exit code 0. VM torn down and confirmed absent. PROOF-01 retired; v1.0 milestone CLOSED. See 09-04-SUMMARY.md.

Note: Phase 3 remains `status: verifying` (pending /gsd-verify-work) in the frontmatter above; Phase 4/5/6/7 planning/execution began before that gate ran.

Progress: [██████████] 100% (Phase 9: 4/4 plans complete — v1.0 milestone CLOSED, see 09-04-SUMMARY.md)

## Performance Metrics

**Velocity:**

- Total plans completed: 10
- Average duration: ~17 min
- Total execution time: ~2.6 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 1 | 4 | ~25+ min | ~8 min |
| 2 | 3/3 | ~160 min | ~53 min |
| 02 | 3 | - | - |

**Recent Trend:**

- Last plan: 04-03 (~30 min authoring + live gate run, 2 tasks, 3 files + 2 live-run bug fixes) — throwaway WTS PowerShell responder + WinRM deploy/launch helper + gated `sensor_ping_pong_under_500ms` live test; LIVE-VERIFIED against a real Azure VM, measured round trip 165ms, VM torn down after the run
- Prior: 02-03 (~55 min incl. ~20-min canonical idle run, 3 tasks, 3 files) — example + gated live suite; 5/5 live pass at full 10-min idle; VM torn down
- Prior: 02-02 (~70 min, 3 tasks, 8 files) — live session machinery: connect/loop/framebuffer/keepalive/Session

*Updated after each plan completion*
| Phase 3 P1 | 55min | 2 tasks | 3 files |
| Phase 03-input-injection P02 | ~40min | 2 tasks | 2 files |
| Phase 03 P03 | 15min | 1 tasks | 1 files |
| Phase 03 P04 | ~90min | 1 tasks | 2 files |
| Phase 04 P01 | 25min | 2 tasks | 4 files |
| Phase 04 P02 | 35min | 2 tasks | 4 files |
| Phase 04 P03 | ~30min | 2 tasks | 3 files (offline code only; live gate pending) |
| Phase 05 P02 | 15min | 3 tasks | 5 files |
| Phase 05 P03 | 20min | 2 tasks | 4 files |
| Phase 05 P01 | ~55min | 3 tasks | 6 files (incl. live-gate publish + 3 live-run bug fixes) |
| Phase 05 P04 | ~2h5min | 3 tasks | 9 files (incl. live gate + 3 live-run bug fixes) |
| Phase 06 P01 | 25min | 2 tasks | 6 files |
| Phase 06 P02 | 35min | 2 tasks | 1 files |
| Phase 06 P03 | 40min | 2 tasks | 4 files |
| Phase 06-window-process-perception P04 | 35min | 2 tasks | 6 files |
| Phase 06 P05 | 58min | 1 tasks | 3 files |
| Phase 07 P01 | 20min | 2 tasks | 4 files |
| Phase 07 P02 | 20min | 2 tasks | 2 files |
| Phase 07 P03 | ~1h50min | 1 tasks | 2 files |
| Phase 07-uia-tree-module P04 | 25min | 2 tasks | 4 files |
| Phase 07-uia-tree-module P05 | ~40min | 2 tasks | 1 files |
| Phase 08 P01 | ~5min | 2 tasks | 3 files |
| Phase 08-public-sdk-api-worldstate P02 | 1200 | 2 tasks | 3 files |
| Phase 08 P03 | 25min | 1 tasks | 1 files |
| Phase 09 P01 | ~70min | 2 tasks | 1 files |
| Phase 09-scripted-proof-harness P02 | ~25min | 2 tasks | 6 files |
| Phase 09 P03 | 55min | 3 tasks | 3 files |
| Phase 09-scripted-proof-harness P04 | ~2h | 1 tasks | 1 files (terminal live gate; sensor rebuild + 1 live bug fix + 1 live-tune) |

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- Stack locked: IronRDP (Rust) + C# NativeAOT sensor + DVC transport on port 3389. **Corrected at Phase 2 Plan 01:** IronRDP versions are NOT uniform — umbrella `ironrdp = 0.15`, members at independent versions (tokio/connector 0.9, pdu 0.8, input/dvc 0.6, tls 0.2.1); the old uniform 0.14 pins do not resolve. `Cargo.lock` committed (D-02).
- **Build toolchain (Phase 2 Plan 01 architectural decision, checkpoint-approved):** build target is `x86_64-pc-windows-gnu` (MinGW-w64 gcc), NOT `*-msvc`. Host is ARM64 Windows with no MSVC/Windows SDK and VS was declined; x64 artifacts run under Windows-on-ARM x64 emulation. Toolchain pinned via `rust-toolchain.toml`; gcc linker pinned via `.cargo/config.toml`. Functionally equivalent for a pure-Rust RDP client.
- `ironrdp-tls` requires exactly one TLS backend feature — `rustls` selected (matches the planned hand-built rustls ClientConfig / D-15 custom verifier path).
- Public API exposes only owned SDK types (Error/ConnectionConfig/Screenshot/Rect); image/ironrdp/rustls/anyhow stay internal (D-09). No unwrap/expect/panic in library code (API-01). ConnectionConfig Debug redacts the password (D-14).
- DVC channel (RDPILOT_SENSOR) must be registered before connector.connect() completes — hard IronRDP constraint. **Implemented at Phase 2 Plan 02:** DrdynvcClient registered on the connector before connect_begin as the empty seam; Phase 4 adds the sensor processor via with_dynamic_channel.
- **Phase 2 Plan 02:** enabled ironrdp umbrella features (connector/session/graphics/input/dvc/svc — facade defaults to only core+pdu) and the ironrdp-tokio `reqwest` feature for ReqwestNetworkClient; added rustls-native-certs for the default validating cert path.
- **Phase 2 Plan 02 (structural):** the SDK-owned session loop runs on a dedicated OS thread with a current-thread Tokio runtime, NOT tokio::spawn — the reactivation step holds a Sequence::next_pdu_hint() -> Option<&dyn PduHint> borrow across .await, which the compiler cannot prove Send (HRTB limitation). Fully contained inside Session; public async API unchanged.
- RemoteDesktop_SuppressWhenMinimized=2 is a Phase 2 prerequisite, baked into Phase 1 VM provisioning
- Force 96 DPI on remote session; sensor sets DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2; emit both coordinate spaces
- v1 done = scripted harness proves end-to-end, no live LLM required
- Phase 1 provisions the Azure Windows target that all subsequent phases test against; auto-destroy prevents runaway cost
- `infra/` is the IaC root (chosen over `tools/env/`) — locked in Plan 01 for all later Phase 1 artifacts
- `.secrets/connection.json` is the only credential location; `.gitignore` excludes `.secrets/` and `.env` from the start (Pitfall 7)
- Cargo.lock commit policy RESOLVED in Phase 2 Plan 01: Cargo.lock IS committed (D-02), staged in the same commit as the manifests.
- Pester 5.7.1 is the PowerShell test framework; per-commit gate is no-Azure-cost (az + network mocked)
- main.bicep: single RG, subnet-level NSG association, StandardSSD_LRS OS disk, NLA left at Azure default (verified later); CSE invokes Configure-Target.ps1 via scriptUri param (Plan 03/04 fill the contract)
- Compiled Bicep ARM output (infra/*.json) is gitignored — source of truth is the .bicep
- 7-Zip pinned to 26.01 via blocking human-verify checkpoint: URL github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe, SHA-256 d64a0468...94377d (computed == GitHub release asset digest); Configure-Target.ps1 throws on hash mismatch before install
- Configure-Target.ps1: per-user settings (96 DPI + SuppressWhenMinimized) go to the DEFAULT user hive via reg load/unload, never the current-user hive (Pitfall 1); no reboot; every mutation idempotent
- Forbidden-token verify gates match literals anywhere in a file (incl. comments) — keep rationale prose token-free (HKCU, 0.0.0.0/0, deploymentScripts, Owner)
- Auto-destroy topology = separate-management (user decision): persistent management RG (rdpilot-mgmt) holds the Automation Account; its managed identity = Contributor over the TEST RG (rdpilot-test) ONLY; West Europe; subscription = caller's active az sub (never hard-coded)
- manage-env.ps1 publishes BOTH Configure-Target.ps1 (CSE) and Delete-ResourceGroup.ps1 (runbook publishContentLink) to a per-up private blob + short-lived read-only single-blob SAS; storage account lives in the TEST RG so `down` cascades it; `down` leaves the management RG in place
- **Phase 3 Plan 1:** owned MouseAction/KeyAction/Button/Key vocabulary + pure translation to ironrdp_input::Operation batches, with Pitfall 1 (wheel |dy|>255 split) and Pitfall 5 (MouseMove-before-WheelRotations) guards; Error::CoordinateOutOfBounds mirrors CropOutOfBounds
- [Phase ?]: Phase 3 Plan 2: Mutex<Database> lives in Session (not session_loop), locked only for synchronous apply(); DoubleClick/Drag inter-batch timing is tokio::time::sleep on the caller's async context, never in the session-loop select! (Pitfall 3). Session::desktop_size() is a deliberate v1 static capture at connect time.
- [Phase ?]: Phase 3 Plan 3: send_key mirrors send_mouse's lock/apply/drop/send shape with no bounds-check/timing (Type/Combo is always a single Operation batch); no logging added at all (trivially satisfies never-log-typed-content)
- [Phase ?]: Server Manager auto-launch suppressed at infra layer (Configure-Target.ps1 DEFAULT hive) after breaking bare-desktop input assumptions during Phase 3 live validation; DOUBLE_CLICK_GAP=100ms and DRAG_INTERPOLATION_STEPS=5/DRAG_STEP_GAP=15ms empirically confirmed reliable, no tuning needed
- [Phase ?]: Used ironrdp::pdu::pdu_other_err!(desc, source: e) instead of ironrdp::core::other_err! for PduResult construction in RdpilotSensorProcessor::start() (PduError does not implement OtherErr) — Verified by reading ironrdp-pdu-0.8.0 and ironrdp-core-0.2.0 source directly; correction to RESEARCH's code example, not a CONTEXT deviation
- [Phase ?]: SensorShared fields widened to pub(crate) so Session::ping() and RdpilotSensorProcessor::process() share direct lock access (no accessor layer, matches module's crate-internal-only design)
- **Phase 4 live gate (2026-07-09):** SENSOR-03 validated live against a real disposable Azure VM (measured ping/pong round trip 165ms). Two bugs found and fixed: session_loop.rs's Ping handler no longer propagates a transient "DVC not registered/not yet open" lookup failure via `?` (which previously killed the whole session-loop thread on the very first ping attempt) — extracted to `build_ping_frame()`, dropped (vec![]) on failure so the caller's existing 500ms timeout surfaces a normal retryable error instead. sensor-responder.ps1's `Read-Envelope` now scans for the first `{` byte instead of assuming JSON starts at offset 0 — WTSVirtualChannelRead was consistently returning a small fixed binary prefix ahead of the JSON payload. Also empirically found: Task Scheduler's AtLogOn trigger takes ~20-30s to fire and does NOT refire on an RDP session *reconnect* (only a fresh logon) — the live test's outer setup-retry budget was widened 15s→60s to tolerate this deployment-mechanism latency (Session::ping()'s own hard 500ms per-call timeout, the actual SC#2 measurement, is unchanged).
- [Phase ?]: 05-02: ServerDriveIoRequest has 11 variants not the plan's assumed 4 (efs.rs read_first); the 4 planned (Create/Close/Read/QueryDirectory) are implemented, the other 7 are typed-rejected with NOT_SUPPORTED, mirroring the crate's own handle_printer_io_request default
- [Phase ?]: 05-02: not-found NtStatus for rejected RDPDR Create paths is NtStatus::NO_SUCH_FILE (0xC000000F) -- efs.rs has no OBJECT_NAME_NOT_FOUND constant
- [Phase ?]: 05-03: Rdpdr::new(backend, computer_name)/with_drives(Some(vec![(id,name)])) matched the plan's assumed signature exactly (verified against pinned ironrdp-rdpdr-0.6.0 source) -- no deviation needed
- [Phase ?]: 05-03: Rdpdr::process() self-dispatches inbound MS-RDPEFS IRPs to the registered RdpdrBackend internally -- ActiveStage::process drives the RDPDR static channel automatically, exactly like the existing drdynvc channel; no session_loop.rs change was needed
- [Phase ?]: 05-03: introduced crate::connect::SENSOR_EXE_NAME as the single source of truth for the served/launched sensor filename, referenced by both the RDPDR backend registration and Session::deploy_and_launch's launch_command() helper
- [Phase ?]: 05-03: deploy_and_launch poll-and-retry tuned offline as LAUNCH_ATTEMPTS=3 x PINGS_PER_LAUNCH_ATTEMPT=20 (~30s total outer budget), reasoned from Phase 4's empirical ~10s WTSVirtualChannelOpenEx retry-window finding -- to be live-tuned in Plan 04 if needed
- **Phase 5 live gate (2026-07-09, PASSED):** SENSOR-01/SENSOR-02 validated live against a real disposable Azure VM. SC1: NativeAOT win-x64 self-contained publish, 2,699,264 bytes (~2.57 MiB), no external .NET runtime dependency, SHA256 identical VM-built vs locally-retrieved (resolves D-5.3). SC2 (RDPDR primary, mandatory D-5.6) PASS, measured elapsed 22.612044ms. SC3 (WinRM fallback) PASS, measured round trip 21.62078ms. SC4 (<1s) met on both paths. StringMarshalling resolved to Utf8/ANSI -- `WTSVirtualChannelOpenEx` has no W export, `Utf16` failed silently (05-01). `rdpsnd` static channel required for Windows to even start the RDPDR handshake -- MS-RDPEFS Appendix A footnote <1>: a Windows RDP server withholds the RDPDR Server Announce Request unless `rdpsnd` is also advertised/joined; added as a join-only, non-functional stub (`crates/rdpilot/src/rdpsnd_stub.rs`, commit 405f47c). Windows' real drive-access sequence (`Create -> QueryInformation -> QueryVolumeInformation -> QueryDirectory`, issued for any path including the drive root) required implementing those two previously-NOT_SUPPORTED IRPs in `RdpilotDriveBackend` (commit d07e710). `deploy_and_launch` needed two live-diagnosed timing fixes: a one-time 3s `SESSION_SETTLE` before the first launch attempt (interactive session can still be mid-transition immediately after connect) and chunked typed launch command (avoids Run-dialog ComboBox autocomplete corrupting the typed text) (commit e2fedf1). AV/EDR did not block the sensor exe (no exclusion needed); drive-redirection GPO did not block (no policy change needed) -- both empirical Phase-5 unknowns resolved with no remediation required. See `05-01-SUMMARY.md` and `05-04-SUMMARY.md`.
- [Phase ?]: 06-01: Generic req_id-keyed fulfilment (one non-Version match arm serves every DVC reply type) instead of a per-msg_type handler, per RESEARCH Pattern 1
- [Phase ?]: 06-01: session.rs updated outside this plan's stated files_modified to keep the crate compiling against the generalized RdpInputEvent/pending types (Rule 3 blocking-fix, no Session::ping() behavior change)
- [Phase ?]: 06-01: Offline verification run against the native x86_64-unknown-linux-gnu target (this session's host has no windows-gnu/MinGW toolchain); the crate has no cfg(windows) code so this is a safe substitute — the real windows-gnu build should still be re-confirmed on the pinned dev machine before a live gate
- [Phase ?]: 06-02: get_window_list/get_process_tree/set_foreground_window/launch_process share one sensor_request() helper mirroring ping()'s round-trip shape with D-6.4 success/data vs success:false->SensorRejected branching
- [Phase ?]: 06-02: screenshot_window delegates to a pure crop_to_window(shot, window) = shot.crop(window.rect) helper -- no coordinate remap, no sensor round trip (D-6.1)
- [Phase ?]: Envelope.Payload retyped object? -> JsonElement? (RESEARCH Pitfall 1/Pattern 6), proven under a real NativeAOT publish (linux-x64 surrogate, win-x64 blocked by cross-OS AOT compile limitation) before any Win32 handler code was written
- [Phase ?]: WindowList handler implemented: EnumWindows via static [UnmanagedCallersOnly] + delegate* unmanaged<> + GCHandle accumulator, [LibraryImport]-only user32.dll surface, bounded stackalloc title/class buffers, z_order = enumeration index, success:false degrade on any exception (D-6.4)
- [Phase ?]: PROCESSENTRY32W's szExeFile embedded string field must be a blittable 'unsafe fixed char[260]' buffer, not MarshalAs(ByValTStr) -- the latter fails SYSLIB1051 under source-generated LibraryImport
- [Phase ?]: 06-04: ProcessTree handler enumerates via CreateToolhelp32Snapshot/Process32FirstW/Process32NextW (plain [LibraryImport] against kernel32.dll) -- NEVER WMI/System.Management/ManagementObjectSearcher; full image path resolved best-effort per-pid via OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) + QueryFullProcessImageNameW, falling back to empty string on per-process failure rather than failing the whole call; command_line/owner deliberately deferred to null (D-6.3, not a hard success criterion)
- [Phase ?]: Phase 6 live gate (2026-07-09, PASSED): PERC-01/PERC-02/PERC-04/PROC-01/CAP-02 validated live against a real disposable Azure VM. win-x64 NativeAOT builds run ON the VM itself via az vm run-command invoke (WinRM Negotiate/NTLM auth failed from this Linux host — missing gssntlmssp GSS mechanism plugin; Basic auth rejected server-side); a short-lived Azure Storage blob SAS relay moved the built exe back. Three live-run bugs fixed: (1) launch_command now taskkills any already-running sensor before copy+start — Windows reconnects a disconnected interactive RDP session rather than creating a fresh one, so a stale process from a prior test blocked the new DVC channel; (2) SetForegroundWindow now wraps the call in AttachThreadInput to defeat Windows' foreground-lock-timeout restriction (a bare call returned TRUE with no Z-order effect); (3) the SC#3 test now confirms focus via the minimum z_order among TITLED windows only, not the global minimum — always-on-top shell chrome (the taskbar) legitimately outranks any normal app window regardless of focus. All four gated tests pass; VM torn down and confirmed (az group exists -n rdpilot-test => false).
- [Phase ?]: 07-01: Wire contract ships RAW runtime_id/parent_runtime_id int arrays and RAW control_type int; Rust into_owned() does the D-7.2 join and D-7.3 map (locks the C# UiaElementRecord shape for 07-04)
- [Phase ?]: 07-01: get_uia_tree reuses ENUMERATION_TIMEOUT_MS (2000ms) as the transport timeout -- SC#3's 500ms sensor-side walk budget is a separate, live-verified constraint, not tightened here
- [Phase ?]: 07-01: Built/tested on native x86_64-unknown-linux-gnu (RUSTUP_TOOLCHAIN + --target override) instead of the repo-pinned x86_64-pc-windows-gnu toolchain, which is not installed on this host -- valid substitution since perception.rs/session.rs/sensor.rs have no cfg(windows) code
- [Phase ?]: 07-02: Rect32 struct redeclared as a new top-level type in UiaInterop.cs (identical shape to WindowEnumeration.cs's private Rect32) rather than sharing the existing type, to keep this plan's file scope to UiaInterop.cs/Program.cs only.
- [Phase ?]: 07-02: GeneratedComInterface does not support C# instance properties (SYSLIB1091) -- all UIA propget members declared as Get-prefixed methods instead.
- [Phase ?]: 07-02: Marshal.SafeArrayGetLBound/GetUBound/GetElement/SafeArrayDestroy do not exist in .NET Core/.NET 8 (Framework-only) -- hand-rolled the equivalent oleaut32.dll SAFEARRAY exports via LibraryImport instead.
- [Phase ?]: 07-04: Reused WindowRect for the UiaElementRecord bbox field rather than declaring a new UiaBboxRecord -- same namespace, same x/y/w/h shape, already JSON-registered.
- [Phase ?]: Phase 7 live gate (2026-07-09, PASSED): all four Phase 7 SC validated live against a real disposable Azure VM and a launched Notepad window. SC#1 field-complete UiaElement[] confirmed; SC#2 bbox coordinates confirmed in the same physical virtual-desktop pixel space as get_window_list (desktop_size bounds containment); SC#3 TreeScope_Children walk measured 30.36ms, >16x under the 500ms budget -- D-7.7's CreateCacheRequest bulk-cache optimization correctly never triggered, naive uncached per-property reads (07-04) are sufficient; SC#4 live serde_json round trip lossless. Sensor AOT-published win-x64 ON the VM via az vm run-command invoke (WinRM still unavailable from this host), SHA256 fa5d3e3c8d45c351e0d577cf654c6c524c8ecab9e4203917ef6637c053ce6e16 verified byte-identical between VM build and locally-relayed copy. One transient first-RDP-login deploy_and_launch timeout (the fresh-VM network-discoverable dialog condition first diagnosed in 07-03) recurred and self-resolved on a single retry, confirming it as a reproducible VM-provisioning-time artifact, not a code defect -- no code change made. VM torn down and confirmed absent (az group exists -n rdpilot-test => false; rdpilot-mgmt persists). PERC-03 genuinely retired.
- [Phase ?]: 08-01: lib.rs inner #![deny(unsafe_code)]/clippy::unwrap_used/clippy::expect_used gates make API-01/SC#4 compiler-enforced (never a Cargo.toml [lints] table, which would break tests/live_session.rs's 116 legitimate .expect() calls)
- [Phase ?]: 08-01: Screenshot derives Serialize only (no Deserialize) with #[serde(skip)] on rgba — dims-only JSON output (D-8.4, threat T-08-02); to_png() remains the sole byte-egress path
- [Phase ?]: D-8.1/D-8.2/D-8.4 applied verbatim: WorldStateOptions a-la-carte defaults to screenshot+window_list+no-UIA (SC#2-compliant); capture_span is SystemTime/Duration (never Instant) with the 500ms bound checked only at the Plan 03 live gate, never in code
- [Phase ?]: Live VM measurement for SC#2 (08-03) deferred — no disposable Azure VM currently reachable; gated test authored and compiles clean offline
- **Phase 8 live gate (2026-07-10, PASSED):** SC#2 empirically CLOSED. Provisioned a fresh disposable Azure VM (rdpilot-vm, Standard_B2s_v2), built the win-x64 NativeAOT sensor ON the VM via `az vm run-command invoke` (WinRM still unavailable from this Linux host — same Phase 5/6/7 substitution), relayed the exe back via a short-lived Storage blob SAS (SHA256 byte-identical, 7a775b990cca3b40e72d4cd8ce910ebfc8e14262dd660089a4e5c62355ec34c2). Both gated `world_state` tests ran live and PASSED: default-options `capture_span` measured 23.524043ms (23ms); `UiaMode::Foreground` `capture_span` measured 73.194179ms (73ms) after one retry of the documented first-RDP-login `deploy_and_launch` transient (Phase 6/7 finding, self-resolved, no code change). One `az vm run-command` gotcha newly found: passing the sensor source tarball via `--parameters` (as opposed to embedding it directly in the script body) failed near-instantly, consistent with an undocumented CLI parameter-size limit — embedding the base64 payload directly in the script body (the Phase 6/7 pattern) is the durable approach; do not use `--parameters` for large payloads. VM torn down and confirmed absent (`az group exists -n rdpilot-test` => false; `rdpilot-mgmt` persists).
- **Phase 9 Plan 1 (09-01, 2026-07-10, D-9.1 risk-gate spike, HUMAN-APPROVED):** 7-Zip File Manager stays the SC#2/SC#3 target (Notepad fallback explicitly NOT invoked). Live dump against a real disposable Azure VM showed `get_uia_tree(hwnd)`'s `TreeScope_Children`-only scope (D-7.4) returns just 5 depth-1 elements (Window/ToolBar/Pane/TitleBar/MenuBar) — none of the visible menu items, toolbar buttons, or listview rows are reachable at that depth. **Plan 09-02 must add a scoped, caller-configurable deeper UIA-walk capability** (flagged addition, not a silent absorption) before any SC#2/SC#3 assertion code is written. Confirmed window match predicate: exact `class_name == "7-Zip::FM"` (never a title substring — title reflects the currently-navigated folder). Confirmed D-9.6 seeding form (RESEARCH A1 resolved live): `launch_process` needs the FULL, individually-quoted exe path (`"C:\Program Files\7-Zip\7zFM.exe"`) plus a quoted seed-path argument — a bare `"7zFM.exe"` fails `CreateProcessW` with `LastError=2` since the sensor's `lpApplicationName` is null and 7-Zip's install dir is not on `PATH`. Reused the cached, hash-identical Phase 8 sensor build (no VM rebuild needed, sensor source unchanged since Phase 7-04). Throwaway spike file deleted per Pitfall 2; VM torn down and confirmed absent. See `09-01-SUMMARY.md`.
- [Phase ?]: D-9.1 spike human-approved: 7-Zip stays SC#2/SC#3 target; TreeScope_Children insufficient, Plan 09-02 must add a scoped deeper UIA-walk capability; window predicate class_name=="7-Zip::FM"; D-9.6 seeding form confirmed (full quoted 7zFM.exe path + quoted seed arg, RESEARCH A1 resolved)
- [Phase ?]: 09-02: Added owned UiaScope (Children | Subtree{max_depth}) to get_uia_tree, mapped to wire max_depth; sensor performs a bounded level-by-level FindAll(TreeScope.Children) BFS clamped to a named UIA_MAX_WALK_DEPTH=4 safety cap (never TreeScope.Subtree, T-09-10); world_state + all 5 live_session.rs callers migrated to UiaScope::Children with zero behavior change; UIA_MAX_WALK_DEPTH=4 is a conservative starting value, live-tuned at the 09-04 gate against the real Phase 7 SC#3 500ms budget.
- [Phase ?]: SC2_MAX_DEPTH=4 requested for the 09-03 proof harness's UiaScope::Subtree walk (matches the 09-02-recorded sensor-side UIA_MAX_WALK_DEPTH cap); live-tune at the 09-04 gate
- [Phase ?]: SC#2 assertion passes on any of menu item / toolbar button / listview row deeper elements present (not all three required)
- [Phase ?]: D-9.2 navigation default: click deeper 'File' MenuItem if matched, else first matched deeper element
- **Phase 9 terminal live gate (09-04, 2026-07-10, PASSED — v1.0 MILESTONE CLOSED):** All four PROOF-01 success criteria proven live against a real disposable Azure VM (reused from an earlier session, confirmed healthy rather than reprovisioned). Mandatory sensor AOT-rebuild on the VM from current 09-02 source (not the Phase 8 cache) via `az vm run-command invoke`, SHA256 `41a35f8cc60e0a1ab38c62b8736246518c84c0f7dc8220c25940f1ab9c313f6e`, confirmed byte-identical VM-built vs relayed and confirmed different from the Phase 8 cache hash `7a775b990cca3b40e72d4cd8ce910ebfc8e14262dd660089a4e5c62355ec34c2`. Two live-diagnosed fixes: (1) [Rule 1 bug] the SC#3 navigate step now calls `set_foreground_window` + settle before clicking a deeper element — a freshly `launch_process`'d window is not guaranteed OS foreground focus (identical root cause to Phase 6's own SC#3 diagnosis; every other live navigation test in the crate already did this, the 09-03 harness had missed it); (2) [Rule 3 live-tune] `SC2_MAX_DEPTH` tuned from 4 to 3 after measuring the deeper-walk latency at 555.2ms (over the Phase 7 SC#3 500ms budget) — re-measured at 130.2ms with zero loss of SC#2 element coverage. One documented first-RDP-login `deploy_and_launch` transient self-resolved on retry, no code change. Measured results: SC#1 screenshot 1920x1080; SC#2 30 deeper elements (menu items + toolbar buttons) matched at max_depth=3; SC#3 navigation + verified screenshot-diff change; SC#4 `examples/proof_harness` printed `PROOF: PASS` and exited 0. VM torn down and confirmed absent (`az group exists -n rdpilot-test` => false; `rdpilot-mgmt` persists). PROOF-01 retired in REQUIREMENTS.md; Phase 9 marked complete (4/4) in ROADMAP.md. See `09-04-SUMMARY.md`.

### Pending Todos

- Phase 3 Plan 2 (mouse Session wiring): add `input_db: Mutex<ironrdp_input::Database>` + `desktop_size` fields to `Session`, `RdpInputEvent::FastPath` variant in `session_loop.rs`, and `Session::send_mouse`/`desktop_size()` consuming this plan's `mouse_operations`/`Error::coordinate_out_of_bounds`. Then Plan 3 (keyboard) and Plan 4 (gated live suite).
- When re-running the canonical Phase 2 validation: default VM size Standard_B2ms is SkuNotAvailable in westeurope — use `-VmSize Standard_B2s_v2` (or another preflight-listed size).
- Future agents on this machine must export the scoop rustup env (RUSTUP_HOME / CARGO_HOME / CARGO_HOME\bin on PATH) and have MinGW gcc on PATH for cargo to link.

### Blockers/Concerns

None outstanding — v1.0 milestone is CLOSED as of 09-04 (2026-07-10). Phase 5 residual notes carried forward for future reference (non-blocking):

- Phase 5 residual (non-blocking, carried forward): `deploy_and_launch`'s `SESSION_SETTLE` + chunked-typing fixes are empirically-tuned timing workarounds, not root-caused to a specific Windows readiness signal — may need revisiting on a differently-provisioned target
- Phase 5 residual (non-blocking, carried forward): the `rdpsnd` stub channel is intentionally non-functional (presence-only) — correct for v1 scope but a permanent architectural addition, not a temporary hack

**Resolved during Phase 5 (previously listed here):**

- ~~AV/EDR environment on target unknown~~ — resolved: no block encountered live, `-AddAvExclusion` never needed (05-04)
- ~~Drive redirection GPO policy on target unknown~~ — resolved: no block encountered live, no policy remediation needed (05-04)
- ~~NativeAOT binary size unknown (5-30+ MB range)~~ — resolved: 2,699,264 bytes (~2.57 MiB) (05-01)

**Resolved during Phase 9 Plan 1 (previously listed here):**

- ~~Phase 7/9: Target application UIA fidelity is unknown — identify and test before Phase 9 harness assertion design~~ — resolved: D-9.1 spike live-ran, human-approved; 7-Zip confirmed adequate (with the deeper-walk capability addition noted above), see 09-01-SUMMARY.md

**Resolved during Phase 9 Plan 4 / terminal live gate (previously listed here):**

- ~~Phase 9 (new, from 09-01): `TreeScope_Children` alone (D-7.4) is insufficient for 7-Zip's meaningful SC#2 elements — Plan 09-02 must design and add a scoped, caller-configurable deeper UIA-walk capability before writing assertion code.~~ — resolved: 09-02 added `UiaScope::Subtree{max_depth}`; 09-04's live gate proved it live against real 7-Zip (30 deeper elements matched, 130.2ms), see 09-04-SUMMARY.md

## Deferred Items

None outstanding for Phase 1. All ENV-01/02/03 requirements satisfied.
Phase 2 Plan 02: pre-existing rustdoc intra-doc-link warnings in config.rs (Wave 1) logged in `.planning/phases/02-rdp-session-framebuffer-core/deferred-items.md` — out of scope, cargo doc still exits 0.

### v1.0 Milestone Close — Acknowledged & Deferred (2026-07-10)

Pre-close artifact audit surfaced 3 open items. Reviewed and explicitly acknowledged as non-blocking for v1.0 close (see MILESTONES.md):

| Category | Item | Status |
|----------|------|--------|
| CONTEXT open questions | Phase 04 (04-CONTEXT.md): 3 open design questions — reply-channel design for `Session::ping()`, version-handshake wire format specifics, how the throwaway responder opens the server-side DVC channel | Non-blocking — DVC transport was live-proven in Phase 4 (165ms round trip); questions are implementation-detail notes, not unresolved capability gaps |
| Verification gap | Phase 08 (08-VERIFICATION.md): marked `[human_needed]` | Non-blocking — API-01/API-02 are Complete and the Phase 8 live gate passed |
| Unimplemented seed | SEED-001: "RDP view-only / session shadowing" | Out of v1 scope — already captured as Backlog Phase 999.4 (Remote Assistance / Shadowing) |

## Session Continuity

Last session: 2026-07-10T15:30:00.000Z
Stopped at: Phase 9 Plan 4 (09-04) complete — terminal v1 live gate PASSED, PROOF-01 retired, v1.0 milestone CLOSED. No further v1 work outstanding; any next work is v2 (see REQUIREMENTS.md "v2 Requirements (Deferred)") or backlog phases (see ROADMAP.md Backlog).
Resume file: .planning/phases/09-scripted-proof-harness/09-04-SUMMARY.md
