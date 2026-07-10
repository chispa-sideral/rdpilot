---
phase: 04-dvc-transport-channel
plan: 03
subsystem: transport
tags: [ironrdp-dvc, wtsapi32, winrm, powershell, live-test, ping-pong]

# Dependency graph
requires:
  - phase: 04-dvc-transport-channel
    plan: 02
    provides: "RdpilotSensorProcessor registered before connect_begin; Session::ping() -> Result<Duration> with handshake fast-fail + 500ms timeout"
provides:
  - "tests/fixtures/sensor-responder.ps1 — throwaway server-side RDPILOT_SENSOR DVC responder via wtsapi32.dll P/Invoke (WTSVirtualChannelOpenEx/Read/Write/Close), retry loop, Version echo, req_id-correlated Ping->Pong"
  - "tests/fixtures/deploy-responder.ps1 — WinRM deploy/arm/teardown helper that launches the responder inside the interactive RDP session (Session > 0) via a logon-triggered Scheduled Task, never logs the password"
  - "tests/live_session.rs::sensor_ping_pong_under_500ms — gated (#[ignore] + RDPILOT_LIVE) live test asserting a successful ping under 500ms, proving SC#2 and SC#3's positive path together"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Bounded client-side retry loop (~15s budget, 500ms interval) around Session::ping() in the live test, tolerating the transient Error::Dvc while the server-side responder is still launching/opening its own DVC handle — mirrors the responder's own WTS-open retry loop symmetrically on the client side"
    - "Scheduled-task-at-logon (interactive principal, LogonType=Interactive) as the WinRM-to-interactive-session bridge, reusing Phase 1's New-PSSessionOption -SkipCACheck -SkipCNCheck / -UseSSL / -Authentication Negotiate WinRM credential convention verbatim from infra/tests/Validate-Target.ps1"

key-files:
  created:
    - crates/rdpilot/tests/fixtures/sensor-responder.ps1
    - crates/rdpilot/tests/fixtures/deploy-responder.ps1
  modified:
    - crates/rdpilot/tests/live_session.rs

key-decisions:
  - "Scheduled Task (AtLogOn trigger, Interactive principal) chosen as the session-targeting mechanism (Claude's discretion per D-4.2/D-4.5) over other candidates (e.g. an RDP session-connect event trigger) because it is directly WinRM-registerable in one Invoke-Command call with no extra event-log plumbing; the script's header NOTE documents the alternative (session-connect event, IDs 4778/25) as the empirical fallback if the live run shows the responder still landing in Session 0."
  - "The live test's retry loop lives in the TEST, not inside Session::ping() itself — Session::ping()'s own 500ms timeout (Plan 02) stays a hard per-call bound for SC#2's measurement; the test's outer ~15s retry loop only absorbs one-time responder-launch/channel-open latency, so the SC#2 assertion measures a single successful round trip, not a retry-inflated one."
  - "This sandbox (native Fedora Linux VM, no pwsh, no route to the Phase 1 Azure VM) cannot execute the live gate itself — same constraint documented in the 04-01/04-02 SUMMARYs. Both fixtures and the gated test are therefore offline-authored and offline-verified only in this session; the actual live ping/pong pass (end-of-phase human-check) remains an open item for a workstation with pwsh + the live VM."

requirements-completed: [SENSOR-03]

# Metrics
duration: ~30min
completed: 2026-07-09
---

# Phase 4 Plan 3: DVC Transport Channel — Live Gate Artifacts Summary

**Authored the throwaway server-side WTS PowerShell responder, its WinRM deploy/launch/teardown helper, and the gated `sensor_ping_pong_under_500ms` live test, then RAN the end-of-phase live gate against a real disposable Azure Windows VM: PASSED, measured round trip 165ms (SC#2), Version handshake proven first (SC#3 positive).**

## LIVE GATE RESULT (2026-07-09) — PASSED

Ran the full live human-check in a later session against a real, disposable Azure Windows VM
(`Standard_B2s_v2`, westeurope, subscription "Chispa Sideral"):

- `pwsh infra/manage-env.ps1 up -VmSize Standard_B2s_v2` — provisioned successfully, `.secrets/connection.json` written.
- Deployed + armed the responder. **Deviation:** this execution environment's `pwsh` lacks the WSMan/PSWSMan client libraries `deploy-responder.ps1`'s `New-PSSession` requires on Linux; `az vm run-command invoke` (already-authenticated Azure CLI, no new package installs) was used instead to achieve the identical functional outcome — copy the responder script and register the same AtLogOn-triggered Scheduled Task. `deploy-responder.ps1` itself is unchanged and remains the primary documented path for a Windows/pwsh-with-WSMan workstation.
- `RDPILOT_LIVE=1 cargo test -p rdpilot sensor_ping_pong_under_500ms -- --ignored --test-threads=1`: **PASSED** — `sensor_ping_pong_under_500ms: measured round trip = 165.136465ms` (well under the 500ms bound, SC#2). A successful ping is only reachable after the Version handshake completes (`Session::ping()` fast-fails on `Mismatched`), proving SC#3's positive path in the same run.
- Teardown: responder unarmed (scheduled task unregistered, copied script + log removed), `pwsh infra/manage-env.ps1 -Action down` — `rdpilot-test` RG fully deleted and confirmed absent (`az group show` returns `ResourceGroupNotFound`); persistent `rdpilot-mgmt` RG left in place as designed.

**Two genuine bugs were found and fixed during the live run (see Deviations below) before the pass was achieved.**

## Performance

- **Duration:** ~30 min (offline authoring) + ~90 min (live gate run + debugging + teardown, later session)
- **Tasks:** 2 completed (code/script artifacts) + live human-check executed and PASSED
- **Files modified:** 5 total (2 created in this plan's original session; `session_loop.rs`, `sensor-responder.ps1`, `live_session.rs` further modified during the live-gate debugging session)

## Accomplishments

- `tests/fixtures/sensor-responder.ps1`: throwaway server-side `RDPILOT_SENSOR` DVC responder. `Add-Type`-declared P/Invoke of `wtsapi32.dll`'s `WTSVirtualChannelOpenEx`/`Read`/`Write`/`Close`, `WTS_CURRENT_SESSION = 0xFFFFFFFF` + `WTS_CHANNEL_OPTION_DYNAMIC = 0x1`; a 20-attempt × 500ms retry loop around the open call to survive the documented `ERROR_GEN_FAILURE`/0x31 timing race; reads the SDK's Version handshake first and echoes its own version in the identical envelope shape; then loops answering every `Ping` with a same-`req_id` `Pong`. No `WTSVirtualChannelQuery` conversion, no hand-rolled re-framing (both explicitly avoided per RESEARCH Q3's anti-patterns). Header marks it THROWAWAY/Phase-4-only so it does not prejudice the Phase 5 sensor-language decision (D-4.2).
- `tests/fixtures/deploy-responder.ps1`: WinRM helper that reads `.secrets/connection.json` (identical schema/fields to `tests/common/mod.rs`), opens a `New-PSSession` using Phase 1's exact self-signed-lab-cert convention (`New-PSSessionOption -SkipCACheck -SkipCNCheck`, `-UseSSL`, `-Authentication Negotiate` — copied from `infra/tests/Validate-Target.ps1`), copies the responder via `Copy-Item -ToSession`, and registers a Scheduled Task (`AtLogOn` trigger, `Interactive` principal for the target user) so the responder launches inside the interactive RDP session rather than WinRM's own Session 0 (T-04-08). Includes a `-Remove` teardown switch. The password is read once into a `PSCredential` and never appears in any `Write-Host`/log line (T-04-06, verified by grep).
- `tests/live_session.rs::sensor_ping_pong_under_500ms`: gated (`#[ignore]` + `RDPILOT_LIVE`) test mirroring the existing `require_target!`/`block_on` harness. Connects, then retries `session.ping()` in a bounded ~15s window (500ms between attempts) tolerating the transient `Error::Dvc` while the responder is still launching, and asserts the first successful ping's `elapsed < 500ms` (SC#2). Because `Session::ping()` fast-fails on a mismatched handshake before ever sending, a successful ping is only reachable after a matching Version handshake completed — proving SC#3's positive path in the same assertion. On exhausting the retry budget it panics naming the exact live-verify risk (responder not in the interactive session) plus the last `Error::Dvc`. Public API only (`rdpilot::Session`/`Error`) — no `ironrdp`/`image` import.
- `cargo build -p rdpilot` and `cargo test -p rdpilot` both exit 0 (offline, `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu --target x86_64-unknown-linux-gnu`): 57 unit/integration tests pass unchanged, 11 live tests (10 existing + `sensor_ping_pong_under_500ms`) correctly `#[ignore]`d; `cargo test -p rdpilot -- --include-ignored --list` lists exactly one `sensor_ping_pong_under_500ms`; `cargo clippy --tests` clean (one pre-existing, out-of-scope warning in `input.rs` unrelated to this plan).
- `pwsh` was not available in this sandbox, so both `.ps1` fixtures were verified structurally (grep for `WTSVirtualChannelOpenEx`, `RDPILOT_SENSOR`, `WTS_CHANNEL_OPTION_DYNAMIC`, `New-PSSession`, `Register-ScheduledTask`/`ScheduledTask`, the Session>0 NOTE, and the `-Remove` teardown path) rather than parsed with PowerShell's own parser, per this dispatch's explicit scope.

## Task Commits

1. **Task 1: Throwaway WTS responder script + WinRM deploy/launch helper (D-4.1, D-4.2, D-4.5, RESEARCH Q3)** - `eef7533` (feat)
2. **Task 2: Gated sensor_ping_pong_under_500ms live test (SC#2, SC#3 positive)** - `858f0ae` (feat)
3. **Live-gate fix: stop a transient DVC-not-ready Ping from killing the whole session** - `40038b2` (fix)
4. **Live-gate fix: sensor-responder.ps1 JSON-start scan + live_session.rs retry budget/measurement** - `eeeea51` (fix)

## Files Created/Modified

- `crates/rdpilot/tests/fixtures/sensor-responder.ps1` - throwaway server-side WTS P/Invoke DVC responder (new)
- `crates/rdpilot/tests/fixtures/deploy-responder.ps1` - WinRM deploy/arm/teardown helper targeting the interactive RDP session (new)
- `crates/rdpilot/tests/live_session.rs` - added `sensor_ping_pong_under_500ms` (gated live test)

## Decisions Made

- **Session-targeting mechanism = Scheduled Task, AtLogOn trigger, Interactive principal** (Claude's discretion, D-4.2/D-4.5): directly registerable over the same WinRM session already open for the copy step, no extra event-log plumbing. The script also does an immediate `Start-ScheduledTask` kick so a responder is running even if the target user's RDP session is already active (not a fresh logon). The header NOTE documents the alternative (RDP session-connect event, IDs 4778/25) as the empirical adjustment path if the live run shows Session 0 landing instead — this is exactly the live-verify risk RESEARCH and CONTEXT flagged, deliberately left open for empirical resolution rather than guessed at.
- **Retry loop lives in the test, not in `Session::ping()`:** `Session::ping()`'s own 500ms timeout (locked in Plan 02) stays a hard per-attempt bound so the SC#2 measurement is a genuine single round-trip, not inflated by setup retries. The live test's outer ~15s/500ms retry loop only absorbs the one-time responder-launch-and-channel-open latency before the first attempt that actually reaches a running responder.
- **Toolchain override repeated from Plans 01/02:** `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test,clippy} -p rdpilot --target x86_64-unknown-linux-gnu` — this sandbox is a native Fedora Linux VM, not the ARM64-Windows/scoop host `rust-toolchain.toml` targets. Session-local env override only, no committed file touched.

## Deviations from Plan

The original offline-authoring session had no deviations (plan executed exactly as written for the
offline-buildable scope). The subsequent live-gate execution session found and auto-fixed the following:

**1. [Rule 1 - Bug] Transient DVC-not-ready Ping fatally killed the entire session**
- **Found during:** first live test attempt — panicked at `session.close().await.expect("close")` with `Dvc("sensor channel not registered")`.
- **Issue:** `session_loop.rs`'s `RdpInputEvent::Ping` handler used `?` to propagate `get_dvc()`/`channel_id()` lookup failures. This condition is EXPECTED and transient (the responder opens the DVC channel asynchronously, after the interactive session exists) — but propagating it via `?` terminated the entire session-loop background thread on the very first Ping attempt, breaking every subsequent ping and any other in-flight operation for the rest of the session's lifetime. This contradicted `Session::ping()`'s own documented per-call-retryable contract and the live test's designed retry loop.
- **Fix:** extracted the frame-build logic into `build_ping_frame(&mut ActiveStage, req_id) -> Result<Vec<u8>>`; the `Ping` arm now matches on its result and returns `vec![]` (dropping the request, session stays alive) instead of propagating the error, letting the caller's existing 500ms client-side timeout surface a normal, retryable `Error::Dvc`.
- **Files modified:** `crates/rdpilot/src/session_loop.rs`
- **Commit:** `40038b2`

**2. [Rule 1 - Bug] Responder's `Read-Envelope` didn't strip a DVC framing prefix**
- **Found during:** second live test attempt, after fix 1 — session now survived and retried correctly, but no pong ever arrived; diagnostic log capture showed `WTSVirtualChannelRead` consistently returning a small fixed-size binary prefix (observed 6 bytes, e.g. `00 00 03 00 00 00`) ahead of the JSON envelope, on every message including the first Version handshake.
- **Issue:** `sensor-responder.ps1`'s `Read-Envelope` assumed the JSON envelope started at byte 0 of the WTS read buffer (per RESEARCH's documented WTS behavior of stripping DRDYNVC framing) — but empirically, a small binary prefix survived the OS read. All messages were dropped as malformed JSON.
- **Fix:** `Read-Envelope` now scans forward for the first `{` byte in the read buffer and parses JSON from there, discarding whatever precedes it — robust regardless of the exact header width. This is a fixture-only fix (the disposable responder script, explicitly adjustable per D-4.2), not an SDK/library protocol change.
- **Files modified:** `crates/rdpilot/tests/fixtures/sensor-responder.ps1`
- **Commit:** `eeeea51`

**3. [Rule 1 - Bug/timing] Live test's outer setup-retry budget was empirically too short**
- **Found during:** live diagnostics — confirmed via direct VM process/session inspection that the WinRM-registered AtLogOn scheduled task takes ~20-30s from interactive-session creation to actually launching the responder (Task Scheduler's own trigger-evaluation latency), on top of the responder's own up-to-10s `WTSVirtualChannelOpenEx` retry window (RESEARCH Pitfall 1) — the original 15s outer budget in `sensor_ping_pong_under_500ms` was insufficient headroom for a cold connect + fresh AtLogOn firing.
- **Fix:** widened `RETRY_BUDGET` from 15s to 60s; added a `println!` of the measured elapsed time on success for the human-check record. `Session::ping()`'s own hard 500ms per-call timeout (Plan 02, unchanged) remains what SC#2 actually measures — only the outer setup-tolerance loop widened.
- **Files modified:** `crates/rdpilot/tests/live_session.rs`
- **Commit:** `eeeea51`

**Deployment mechanism substitution (not a code deviation, but worth recording):** this execution environment's `pwsh` lacks the WSMan/PSWSMan client libraries needed for `New-PSSession` (WinRM) from Linux, so `deploy-responder.ps1` itself could not run directly in this session. `az vm run-command invoke` (already-authenticated Azure CLI, zero new package installs) was used instead to achieve the identical functional outcome (copy the responder script + register the same AtLogOn-triggered Scheduled Task, using `powershell.exe` since PowerShell 7 is not installed on the WindowsServer2022 base image either). `deploy-responder.ps1` itself is unmodified and remains correct for a Windows/pwsh-with-WSMan workstation per the plan's `<human-check>` instructions.

Also empirically found (informational, no code change needed): Task Scheduler's `AtLogOn` trigger does not refire on an RDP session *reconnect* to an existing disconnected session — only on a genuinely fresh logon. Repeat test runs against the same already-connected VM require a manual `Start-ScheduledTask` kick before each run; this is a live-gate operational note, not a bug in the shipped fixture (a fresh `manage-env.ps1 up` + first connect always produces a fresh logon and fires the trigger normally).

## Issues Encountered

None outstanding. `pwsh` was located at `/usr/bin/pwsh` (PowerShell 7.6.3) in the live-gate execution environment; `az` was pre-authenticated to subscription "Chispa Sideral". All three deviations above were found, fixed, and verified live before this SUMMARY was finalized.

## User Setup Required

None — the live gate has been run and PASSED. No further user action is required for Phase 4.

## Next Phase Readiness

Phase 4 is COMPLETE: all 3 plans are code-complete and the end-of-phase live human-check PASSED against a real disposable Azure Windows VM (`sensor_ping_pong_under_500ms`, measured round trip 165ms, SC#2; Version handshake proven first, SC#3 positive). SENSOR-03 is marked complete in REQUIREMENTS.md; ROADMAP.md's Phase 4 entry is checked off. The Azure VM was torn down after the run (`rdpilot-test` RG deleted and confirmed absent; persistent `rdpilot-mgmt` RG untouched). Phase 5 (Sensor Bootstrap + Deployment) planning may now begin.

---
*Phase: 04-dvc-transport-channel*
*Completed: 2026-07-09 (live gate PASSED — measured round trip 165ms)*

## Self-Check: PASSED

- FOUND: `crates/rdpilot/tests/fixtures/sensor-responder.ps1`
- FOUND: `crates/rdpilot/tests/fixtures/deploy-responder.ps1`
- FOUND: `crates/rdpilot/tests/live_session.rs` (`sensor_ping_pong_under_500ms`)
- FOUND: commit `eef7533`
- FOUND: commit `858f0ae`
