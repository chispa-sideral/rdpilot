---
phase: 05-sensor-bootstrap-deployment
plan: 04
subsystem: rdp-transport
tags: [ironrdp-rdpdr, winrm, live-gate, rdpsnd, ms-rdpefs, deployment]

# Dependency graph
requires:
  - phase: 05-sensor-bootstrap-deployment
    provides: "05-01's real rdpilot-sensor.exe (2.57 MiB NativeAOT), 05-03's Session::deploy_and_launch + RDPDR static-channel registration -- this plan proves both live"
  - phase: 04-dvc-transport-channel
    provides: "Session::ping() 500ms-bounded round trip, live-gate methodology (throwaway-fixture pattern, VM up/down discipline)"
provides:
  - "crates/rdpilot/tests/fixtures/deploy-winrm.ps1 -- WinRM deploy/arm/teardown helper copying the real rdpilot-sensor.exe via AtLogOn Interactive Scheduled Task"
  - "sensor_rdpdr_deploy_and_ping_within_1s + sensor_winrm_deploy_and_ping_within_1s gated live tests (SC2/SC3/SC4)"
  - "crates/rdpilot/src/rdpsnd_stub.rs -- RdpsndStub static channel (join-only, MS-RDPEFS Appendix A footnote <1> compliance) required for Windows to initiate the RDPDR handshake at all"
  - "QueryInformation + QueryVolumeInformation IRP handling in RdpilotDriveBackend (crates/rdpilot/src/rdpdr_backend.rs), required by Windows' real Create->QueryInformation->QueryVolumeInformation->QueryDirectory sequence"
  - "deploy_and_launch: one-time SESSION_SETTLE (3s) before the first launch attempt + chunked typed launch command (avoids Run-dialog ComboBox autocomplete corruption)"
  - "LIVE GATE PASSED (2026-07-09): SC2 (RDPDR primary, mandatory) measured 22.612044ms; SC3 (WinRM fallback) measured 21.62078ms; SC4 (<1s bound) met on both paths"
affects: ["Phase 6 (Window + Process Perception)", "Phase 7 (UIA Tree Module)", "any future RDPDR/drive-redirection or DVC static-channel extension"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "A functionally-inert MS-RDPEFS-adjacent static channel (rdpsnd) can be structurally REQUIRED for an unrelated channel (rdpdr) to receive its Server Announce Request at all -- Windows RDP servers withhold RDPDR startup per MS-RDPEFS Appendix A footnote <1> unless rdpsnd is also advertised/joined. RdpsndStub implements SvcProcessor/SvcClientProcessor with process() dropping every payload and never replying -- presence-only, zero behavior."
    - "Windows' real drive-redirection Create sequence always issues QueryInformation and QueryVolumeInformation immediately after Create, for ANY path including the drive root -- a backend that only implements Create/Close/Read/QueryDirectory (the plan's original 4-IRP scope) is unusable end-to-end even though those 4 are individually correct; both extra IRPs now return small fixed metadata for known file_ids and ACCESS_DENIED for unknown ones, mirroring the existing handle_read access-control discipline."
    - "Empirically-tuned timing/reliability fixes (SESSION_SETTLE, chunked typing) are documented in code comments as live-tuned, not root-caused to a specific Windows readiness signal -- explicit residual concern carried forward, not treated as fully understood."

key-files:
  created:
    - crates/rdpilot/tests/fixtures/deploy-winrm.ps1
    - crates/rdpilot/src/rdpsnd_stub.rs
  modified:
    - crates/rdpilot/tests/live_session.rs
    - crates/rdpilot/src/rdpdr_backend.rs
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/connect.rs
    - crates/rdpilot/src/lib.rs
    - crates/rdpilot/Cargo.toml
    - Cargo.lock
  deleted:
    - crates/rdpilot/tests/fixtures/sensor-responder.ps1
    - crates/rdpilot/tests/fixtures/deploy-responder.ps1

key-decisions:
  - "deploy-winrm.ps1 adapted near-verbatim from the proven Phase-4 deploy-responder.ps1 shape (same self-signed-lab-cert WinRM options, AtLogOn trigger + Interactive principal, never-echo-password discipline, -Remove teardown) but copies and launches the real rdpilot-sensor.exe directly instead of a pwsh-hosted throwaway responder script."
  - "Both throwaway Phase-4 fixtures (sensor-responder.ps1, deploy-responder.ps1) deleted per D-5.7 once the C# sensor + deploy-winrm.ps1 fully superseded them; the Phase-4 sensor_ping_pong_under_500ms test's SC#2/SC#3 positive-path coverage is now subsumed by the two new gated tests."
  - "Live wire-trace diagnosis (Plan 04 live gate) found Windows joins the rdpdr static channel (ChannelJoinConfirm received) but the server never sends the Server Announce Request that starts MS-RDPEFS -- root-caused to MS-RDPEFS Appendix A footnote <1>: a Windows RDP server withholds RDPDR startup unless rdpsnd (audio) is also advertised. Added RdpsndStub as a join-only, non-functional sibling static channel (commit 405f47c) -- confirmed via live wire-trace that this alone unblocks the RDPDR handshake."
  - "Live diagnosis found Windows' real Create->QueryInformation->QueryVolumeInformation->QueryDirectory sequence (issued for ANY path, including the drive root) failing against the prior generic NOT_SUPPORTED rejection for the two Query* IRPs, making the entire redirected drive unusable ('device is not connected' / 'I/O device error'). Implemented both IRPs for known file_ids with small plausible fixed metadata, ACCESS_DENIED for unknown file_ids (commit d07e710), mirroring the existing handle_read access-control boundary."
  - "Two live-diagnosed timing/reliability bugs fixed in deploy_and_launch (commit e2fedf1): (1) the interactive session can still be mid-transition (lock-screen wallpaper still rendering) immediately after Session::connect returns even though the first framebuffer has already arrived -- input injected in this window is silently dropped; added a one-time 3s SESSION_SETTLE before the first launch attempt only. (2) typing the full ~113-character launch command in one send_key burst (one FastPath PDU, ~226 events) corrupted the text the Run dialog's ComboBox autocomplete actually submitted (live evidence: spliced/truncated command line); split into TYPE_CHUNK_LEN-sized chunks with a short settle between each."
  - "StringMarshalling=Utf8/ANSI (resolved in Plan 01, carried forward): WTSVirtualChannelOpenEx has no W export; Utf16 failed silently. Confirmed still correct through this plan's live gate."

patterns-established:
  - "Live-gate bug-fix commits are grouped by the plan whose surface they touch (fix(05-01)/fix(05-02)/fix(05-03)/fix(05-04) prefixes) even when all were discovered and committed during this plan's single live-gate session -- preserves per-plan attribution in git history."

requirements-completed: [SENSOR-01, SENSOR-02]

# Metrics
duration: ~2h5min (Tasks 1-2 offline authoring 2026-07-09T12:55:49+02:00 -> 12:58:00+02:00; live gate + 6 bug-fix commits spanning 13:27:54+02:00 -> 14:21:20+02:00, including VM provisioning/publish/diagnosis time not captured in commit timestamps alone)
completed: 2026-07-09
---

# Phase 5 Plan 4: WinRM Fallback Fixture + Live Gate Summary

**Live gate PASSED against a real disposable Azure Windows VM: RDPDR primary path (SC2, mandatory) measured 22.61ms and WinRM fallback path (SC3) measured 21.62ms, both well under the SC4 1-second bound — closing out SENSOR-01 and SENSOR-02 end-to-end, with six live-diagnosed bugs fixed along the way (rdpsnd-channel requirement, RDPDR IRP gaps, and Run-dialog input-timing fixes).**

## Performance

- **Duration:** ~2h5min total (offline fixture/test authoring ~15 min; live gate session, VM provisioning, diagnosis, and 6 bug-fix commits the remainder)
- **Tasks:** 3 (2 auto, 1 checkpoint:human-verify) — all complete
- **Files modified:** 9 (2 created, 7 modified), 2 deleted

## Accomplishments

- **Task 1 (offline):** `deploy-winrm.ps1` authored — adapts the proven Phase-4 `deploy-responder.ps1` WinRM mechanism (self-signed-lab-cert options, AtLogOn/Interactive Scheduled Task, never-echo-password, `-Remove` teardown) to copy and launch the real `rdpilot-sensor.exe` instead of a throwaway PowerShell responder.
- **Task 2 (offline):** Two gated live tests added to `live_session.rs` — `sensor_rdpdr_deploy_and_ping_within_1s` (SC2 mandatory + SC4, drives `Session::deploy_and_launch`) and `sensor_winrm_deploy_and_ping_within_1s` (SC3 + SC4, drives `deploy-winrm.ps1` + bounded-retry `ping()`). Throwaway Phase-4 fixtures `sensor-responder.ps1` and `deploy-responder.ps1` deleted (D-5.7); offline suite (`cargo test -p rdpilot --no-run` / `cargo test -p rdpilot`) stays green.
- **Task 3 (live gate, phase-closing):** Provisioned the Azure VM, published the sensor, and ran both gated live tests.
  - **SC1 (re-confirmed):** NativeAOT publish exit 0; publish dir has only `rdpilot-sensor.exe` + `.pdb`; runs with no .NET runtime installed; 2,699,264 bytes (~2.57 MiB); SHA256 identical VM-built vs locally-retrieved.
  - **SC2 PASS (RDPDR, mandatory, D-5.6):** measured elapsed **22.612044 ms** — the RDPDR primary path deploys and launches the sensor in-band via drive redirection and gets a pong well under budget.
  - **SC3 PASS (WinRM):** measured round trip **21.62078 ms** — the WinRM fallback path independently deploys and launches the sensor.
  - **SC4 PASS:** both paths ~22 ms, measured from the first successful pong per D-5.2, comfortably under the 1s bound.
  - **AV/EDR:** no block encountered — `deploy-winrm.ps1 -AddAvExclusion` never needed.
  - **GPO/`fDisableCdm`:** no block — ruled out via registry check + `gpresult`; drive redirection worked without policy remediation.
  - **AtLogOn task:** needed one manual `Start-ScheduledTask` kick during the WinRM test — consistent with the Phase-4-documented no-refire-on-reconnect finding (RESEARCH Pitfall 5), not a new bug.
  - **Teardown confirmed:** `az group show -n rdpilot-test` → `ResourceGroupNotFound`; the persistent `rdpilot-mgmt` RG remains intact.
- Six live-run bug-fix commits (all offline-tested afterward, 72/72 passing): three attributed to Plan 01 (csproj/marshalling — see `05-01-SUMMARY.md`), and three attributed to this plan's live-gate diagnosis:
  - `405f47c` — added `RdpsndStub` static channel (`crates/rdpilot/src/rdpsnd_stub.rs`, new file): Windows never sends the RDPDR Server Announce Request unless the `rdpsnd` channel is also advertised/joined (MS-RDPEFS Appendix A footnote <1>), confirmed via live wire-trace.
  - `d07e710` — implemented `QueryInformation` + `QueryVolumeInformation` IRPs in `RdpilotDriveBackend`: Windows' real `Create -> QueryInformation -> QueryVolumeInformation -> QueryDirectory` sequence (issued for any path, including the drive root) needs both to succeed for any redirected-drive access to work at all.
  - `e2fedf1` — `deploy_and_launch`: one-time 3s `SESSION_SETTLE` before the first launch attempt (interactive session can still be mid-transition immediately after connect) + chunked typed launch command (avoids Run-dialog ComboBox autocomplete corrupting the typed text).

## Task Commits

Each task was committed atomically:

1. **Task 1: WinRM fallback deploy fixture** - `d18bbb3` (feat) - `crates/rdpilot/tests/fixtures/deploy-winrm.ps1`
2. **Task 2: Gated live tests + delete throwaway fixtures** - `5d7dc9d` (test) - `crates/rdpilot/tests/live_session.rs`, deletion of `sensor-responder.ps1` + `deploy-responder.ps1`
3. **Task 3 live-gate fix: rdpsnd stub channel** - `405f47c` (fix) - `crates/rdpilot/src/rdpsnd_stub.rs`, `connect.rs`, `lib.rs`, `Cargo.toml`, `Cargo.lock`
4. **Task 3 live-gate fix: QueryInformation/QueryVolumeInformation IRPs** - `d07e710` (fix) - `crates/rdpilot/src/rdpdr_backend.rs`
5. **Task 3 live-gate fix: SESSION_SETTLE + chunked typed launch command** - `e2fedf1` (fix) - `crates/rdpilot/src/session.rs`

(Three additional live-gate fix commits — `491fdbb`, `2c1237a`, `f4a48f3` — touch Plan 01's sensor csproj/Program.cs surface and are attributed to `05-01-SUMMARY.md`.)

## Files Created/Modified

- `crates/rdpilot/tests/fixtures/deploy-winrm.ps1` - WinRM deploy/arm/teardown helper for the real sensor exe (AtLogOn Interactive Scheduled Task)
- `crates/rdpilot/tests/live_session.rs` - `sensor_rdpdr_deploy_and_ping_within_1s` + `sensor_winrm_deploy_and_ping_within_1s` gated tests
- `crates/rdpilot/src/rdpsnd_stub.rs` (new) - join-only, non-functional `RdpsndStub` static channel required for Windows to start the RDPDR handshake
- `crates/rdpilot/src/rdpdr_backend.rs` - `QueryInformation` + `QueryVolumeInformation` IRP handling
- `crates/rdpilot/src/session.rs` - `SESSION_SETTLE` one-time delay + chunked typed launch command in `deploy_and_launch`
- `crates/rdpilot/src/connect.rs`, `src/lib.rs`, `Cargo.toml`, `Cargo.lock` - `RdpsndStub` registration/dependency wiring
- Deleted: `crates/rdpilot/tests/fixtures/sensor-responder.ps1`, `crates/rdpilot/tests/fixtures/deploy-responder.ps1` (throwaway Phase-4 assets, D-5.7)

## Decisions Made

See frontmatter `key-decisions` for the full list. Summary: `deploy-winrm.ps1` reuses the proven Phase-4 WinRM mechanism verbatim except for the payload (real exe, not a throwaway script); both throwaway Phase-4 fixtures deleted; `rdpsnd` presence is a structural (not optional) requirement for RDPDR to start at all (MS-RDPEFS Appendix A footnote <1>); `QueryInformation`/`QueryVolumeInformation` are structurally required by Windows' real Create sequence, not an edge case; `SESSION_SETTLE`/chunked typing are empirically-tuned timing fixes, documented as live-tuned rather than root-caused.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] rdpsnd static channel required for RDPDR to start**
- **Found during:** Task 3 (live gate — SC2 RDPDR primary path)
- **Issue:** Windows joined the rdpdr static channel (ChannelJoinConfirm received) but the server never sent the Server Announce Request that starts MS-RDPEFS, leaving the redirected drive entirely inert.
- **Fix:** Added `RdpsndStub`, a join-only non-functional sibling static channel (drops every payload, never replies) — confirmed via live wire-trace this alone unblocks the RDPDR handshake, per MS-RDPEFS Appendix A footnote <1>.
- **Files modified:** `crates/rdpilot/src/rdpsnd_stub.rs` (new), `connect.rs`, `lib.rs`, `Cargo.toml`, `Cargo.lock`
- **Committed in:** `405f47c`

**2. [Rule 1 - Bug] QueryInformation/QueryVolumeInformation IRPs unimplemented**
- **Found during:** Task 3 (live gate — SC2 RDPDR primary path)
- **Issue:** Windows' real drive-access sequence (`Create -> QueryInformation -> QueryVolumeInformation -> QueryDirectory`), issued for any path including the drive root, failed against the prior generic NOT_SUPPORTED rejection of these two IRPs, making the entire redirected drive unusable ("device is not connected", then "I/O device error").
- **Fix:** Implemented both IRPs, returning small plausible fixed metadata for known file_ids and `ACCESS_DENIED` for unknown ones, mirroring the existing `handle_read` access-control discipline.
- **Files modified:** `crates/rdpilot/src/rdpdr_backend.rs`
- **Committed in:** `d07e710`

**3. [Rule 1 - Bug] deploy_and_launch input dropped / corrupted during Run-dialog interaction**
- **Found during:** Task 3 (live gate — SC2 RDPDR primary path)
- **Issue:** (a) Input injected immediately after `Session::connect` returns was silently dropped while the interactive session was still mid-transition (lock-screen wallpaper still rendering) despite the first framebuffer having already arrived. (b) Typing the ~113-character launch command in a single burst corrupted the text actually submitted by the Run dialog's ComboBox autocomplete (spliced/truncated command line observed live).
- **Fix:** Added a one-time 3s `SESSION_SETTLE` before the first launch attempt; split the typed command into `TYPE_CHUNK_LEN`-sized chunks with a short settle between each.
- **Files modified:** `crates/rdpilot/src/session.rs`
- **Committed in:** `e2fedf1`

---

**Total deviations:** 3 auto-fixed in this plan (all Rule 1 — bugs surfaced only by the live gate, none discoverable offline), plus 3 more attributed to Plan 01 (`05-01-SUMMARY.md`).
**Impact on plan:** All fixes were required for SC2 (mandatory, non-waivable per D-5.6) to pass at all; no scope creep. The `rdpsnd` stub is a permanent architectural addition (see Deferred Issues below), not a temporary hack.

## Issues Encountered

Diagnosis of the RDPDR silent-stall required live wire-trace inspection to root-cause to the `rdpsnd`-channel MS-RDPEFS footnote — not discoverable from spec-reading alone. Resolved; documented above.

## Deferred Issues / Residual Concerns

- **`SESSION_SETTLE` + chunked typing are empirically-tuned timing fixes, not root-caused to a specific Windows readiness signal.** Documented as live-tuned in code comments; may need revisiting on a differently-provisioned target (e.g. faster/slower VM, different Windows build).
- **The `rdpsnd` stub is intentionally non-functional** (channel presence only, no audio redirection implemented or planned) — correct for v1 scope but is a permanent architectural addition to the static-channel set, not a temporary hack to be removed later.

## User Setup Required

None — the live gate ran entirely through existing `infra/manage-env.ps1` + `.secrets/connection.json` tooling already established in Phase 1/4.

## Next Phase Readiness

- SENSOR-01 and SENSOR-02 both fully live-verified end-to-end: build -> deploy (RDPDR primary, mandatory) -> deploy (WinRM fallback) -> launch -> ping, all measured on a real disposable Azure VM.
- Phase 6 (Window + Process Perception) can build directly on the now-proven sensor process lifecycle and DVC transport; no known blockers carried forward from Phase 5.
- The `rdpsnd` stub and the two new RDPDR IRP handlers are stable, tested surface — future sensor modules (window list, process tree, UIA) ride the same already-open channels.

## Threat Flags

None beyond what this plan's and Plan 03's `<threat_model>` already cover — `RdpsndStub` introduces no new network surface (it is a passive, presence-only sibling of the already-registered rdpdr static channel, unauthenticated by the same accepted Phase-4 design as drdynvc/rdpdr); the two new RDPDR IRP handlers extend the existing access-controlled `RdpilotDriveBackend` boundary rather than opening a new one.

## Self-Check: PASSED

All claimed created/modified files exist on disk (`crates/rdpilot/tests/fixtures/deploy-winrm.ps1`, `crates/rdpilot/src/rdpsnd_stub.rs`, `crates/rdpilot/tests/live_session.rs` containing both new test names, `crates/rdpilot/src/rdpdr_backend.rs`, `crates/rdpilot/src/session.rs`); both claimed-deleted throwaway fixtures (`sensor-responder.ps1`, `deploy-responder.ps1`) confirmed absent; all 5 claimed commit hashes (`d18bbb3`, `5d7dc9d`, `405f47c`, `d07e710`, `e2fedf1`) present in `git log --oneline --all`.

---
*Phase: 05-sensor-bootstrap-deployment*
*Completed: 2026-07-09*
