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

requirements-completed: []

# Metrics
duration: ~30min
completed: 2026-07-09
---

# Phase 4 Plan 3: DVC Transport Channel — Live Gate Artifacts Summary

**Authored the throwaway server-side WTS PowerShell responder, its WinRM deploy/launch/teardown helper, and the gated `sensor_ping_pong_under_500ms` live test — all offline-buildable and offline-verified (57 unit tests green, 11 live tests correctly `#[ignore]`d); the actual live ping/pong run against the Phase 1 Azure VM is PENDING (blocked on `pwsh` + a live target, unavailable in this sandbox).**

## Performance

- **Duration:** ~30 min
- **Tasks:** 2 completed (code/script artifacts only — the plan's embedded live human-check was explicitly NOT run this session)
- **Files modified:** 3 (2 created, 1 modified)

## Accomplishments

- `tests/fixtures/sensor-responder.ps1`: throwaway server-side `RDPILOT_SENSOR` DVC responder. `Add-Type`-declared P/Invoke of `wtsapi32.dll`'s `WTSVirtualChannelOpenEx`/`Read`/`Write`/`Close`, `WTS_CURRENT_SESSION = 0xFFFFFFFF` + `WTS_CHANNEL_OPTION_DYNAMIC = 0x1`; a 20-attempt × 500ms retry loop around the open call to survive the documented `ERROR_GEN_FAILURE`/0x31 timing race; reads the SDK's Version handshake first and echoes its own version in the identical envelope shape; then loops answering every `Ping` with a same-`req_id` `Pong`. No `WTSVirtualChannelQuery` conversion, no hand-rolled re-framing (both explicitly avoided per RESEARCH Q3's anti-patterns). Header marks it THROWAWAY/Phase-4-only so it does not prejudice the Phase 5 sensor-language decision (D-4.2).
- `tests/fixtures/deploy-responder.ps1`: WinRM helper that reads `.secrets/connection.json` (identical schema/fields to `tests/common/mod.rs`), opens a `New-PSSession` using Phase 1's exact self-signed-lab-cert convention (`New-PSSessionOption -SkipCACheck -SkipCNCheck`, `-UseSSL`, `-Authentication Negotiate` — copied from `infra/tests/Validate-Target.ps1`), copies the responder via `Copy-Item -ToSession`, and registers a Scheduled Task (`AtLogOn` trigger, `Interactive` principal for the target user) so the responder launches inside the interactive RDP session rather than WinRM's own Session 0 (T-04-08). Includes a `-Remove` teardown switch. The password is read once into a `PSCredential` and never appears in any `Write-Host`/log line (T-04-06, verified by grep).
- `tests/live_session.rs::sensor_ping_pong_under_500ms`: gated (`#[ignore]` + `RDPILOT_LIVE`) test mirroring the existing `require_target!`/`block_on` harness. Connects, then retries `session.ping()` in a bounded ~15s window (500ms between attempts) tolerating the transient `Error::Dvc` while the responder is still launching, and asserts the first successful ping's `elapsed < 500ms` (SC#2). Because `Session::ping()` fast-fails on a mismatched handshake before ever sending, a successful ping is only reachable after a matching Version handshake completed — proving SC#3's positive path in the same assertion. On exhausting the retry budget it panics naming the exact live-verify risk (responder not in the interactive session) plus the last `Error::Dvc`. Public API only (`rdpilot::Session`/`Error`) — no `ironrdp`/`image` import.
- `cargo build -p rdpilot` and `cargo test -p rdpilot` both exit 0 (offline, `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu --target x86_64-unknown-linux-gnu`): 57 unit/integration tests pass unchanged, 11 live tests (10 existing + `sensor_ping_pong_under_500ms`) correctly `#[ignore]`d; `cargo test -p rdpilot -- --include-ignored --list` lists exactly one `sensor_ping_pong_under_500ms`; `cargo clippy --tests` clean (one pre-existing, out-of-scope warning in `input.rs` unrelated to this plan).
- `pwsh` was not available in this sandbox, so both `.ps1` fixtures were verified structurally (grep for `WTSVirtualChannelOpenEx`, `RDPILOT_SENSOR`, `WTS_CHANNEL_OPTION_DYNAMIC`, `New-PSSession`, `Register-ScheduledTask`/`ScheduledTask`, the Session>0 NOTE, and the `-Remove` teardown path) rather than parsed with PowerShell's own parser, per this dispatch's explicit scope.

## Task Commits

1. **Task 1: Throwaway WTS responder script + WinRM deploy/launch helper (D-4.1, D-4.2, D-4.5, RESEARCH Q3)** - `eef7533` (feat)
2. **Task 2: Gated sensor_ping_pong_under_500ms live test (SC#2, SC#3 positive)** - `858f0ae` (feat)

## Files Created/Modified

- `crates/rdpilot/tests/fixtures/sensor-responder.ps1` - throwaway server-side WTS P/Invoke DVC responder (new)
- `crates/rdpilot/tests/fixtures/deploy-responder.ps1` - WinRM deploy/arm/teardown helper targeting the interactive RDP session (new)
- `crates/rdpilot/tests/live_session.rs` - added `sensor_ping_pong_under_500ms` (gated live test)

## Decisions Made

- **Session-targeting mechanism = Scheduled Task, AtLogOn trigger, Interactive principal** (Claude's discretion, D-4.2/D-4.5): directly registerable over the same WinRM session already open for the copy step, no extra event-log plumbing. The script also does an immediate `Start-ScheduledTask` kick so a responder is running even if the target user's RDP session is already active (not a fresh logon). The header NOTE documents the alternative (RDP session-connect event, IDs 4778/25) as the empirical adjustment path if the live run shows Session 0 landing instead — this is exactly the live-verify risk RESEARCH and CONTEXT flagged, deliberately left open for empirical resolution rather than guessed at.
- **Retry loop lives in the test, not in `Session::ping()`:** `Session::ping()`'s own 500ms timeout (locked in Plan 02) stays a hard per-attempt bound so the SC#2 measurement is a genuine single round-trip, not inflated by setup retries. The live test's outer ~15s/500ms retry loop only absorbs the one-time responder-launch-and-channel-open latency before the first attempt that actually reaches a running responder.
- **Toolchain override repeated from Plans 01/02:** `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test,clippy} -p rdpilot --target x86_64-unknown-linux-gnu` — this sandbox is a native Fedora Linux VM, not the ARM64-Windows/scoop host `rust-toolchain.toml` targets. Session-local env override only, no committed file touched.

## Deviations from Plan

None — plan executed exactly as written for the offline-buildable scope. No Rule 1-4 auto-fixes were needed; the only "deviation" is the deliberate, explicitly-authorized scope boundary of this dispatch: the plan's embedded end-of-phase `<human-check>` (the actual live VM run) was intentionally NOT attempted, per this session's explicit instructions (no `manage-env.ps1 up`, no live RDPILOT_LIVE run, no Azure provisioning). This is not a plan deviation in the Rule 1-4 sense — it is the plan's own documented human-check step, correctly left for a workstation that has `pwsh` and reaches the live Azure VM.

## Issues Encountered

None. `pwsh` is not installed in this sandbox, so the `<verify><automated>` block's optional `pwsh -NoProfile -Command ... ParseFile ...` syntax-check branch could not run; the plan's own verify block anticipates this ("pwsh not present — structural grep only" fallback), which was used instead and passed.

## User Setup Required

**The live ping/pong gate is PENDING and requires a workstation outside this sandbox:**

1. On a machine with `pwsh` and network access to the Phase 1 Azure VM (and, for a fresh build, the scoop rustup env + MinGW gcc on PATH per Plans 01/02's carried-forward toolchain note):
   - `pwsh infra/manage-env.ps1 up -VmSize Standard_B2s_v2` (provisions the target, writes `.secrets/connection.json` — do not open it)
   - `pwsh crates/rdpilot/tests/fixtures/deploy-responder.ps1` (copies + arms `sensor-responder.ps1` in the interactive RDP session)
   - `RDPILOT_LIVE=1 cargo test -p rdpilot sensor_ping_pong_under_500ms -- --ignored --test-threads=1` and confirm it **PASSES** (not skipped) — record the measured ping latency
   - If it fails with a channel-not-open/timeout error, the responder is not in the interactive session (check `(Get-Process -Id <pid>).SessionId` on the target); adjust `deploy-responder.ps1`'s launch trigger (the header NOTE names the RDP session-connect event as the fallback) and re-run
   - `pwsh crates/rdpilot/tests/fixtures/deploy-responder.ps1 -Remove` then `pwsh infra/manage-env.ps1 down` to tear down and leave no residue
2. Only after that live pass should SENSOR-03 be marked complete and Phase 4 considered live-verified — **this dispatch deliberately does NOT do either.**

## Next Phase Readiness

All Phase 4 code and scripts are written and offline-verified across all 3 plans. Phase 4 cannot be closed out (SENSOR-03 stays "In Progress" in REQUIREMENTS.md, ROADMAP.md's Phase 4 entry stays unchecked, STATE.md's Blockers/Concerns records the pending live gate) until the live ping/pong run above executes successfully on a workstation with `pwsh` and reach to the Azure VM. Phase 5 planning should NOT begin until that live proof lands, per the phase's own success-criteria definition (SC#2/SC#3 require the real live target, D-4.1 — local loopback was explicitly rejected).

---
*Phase: 04-dvc-transport-channel*
*Completed: 2026-07-09 (offline code artifacts only — live gate PENDING)*

## Self-Check: PASSED

- FOUND: `crates/rdpilot/tests/fixtures/sensor-responder.ps1`
- FOUND: `crates/rdpilot/tests/fixtures/deploy-responder.ps1`
- FOUND: `crates/rdpilot/tests/live_session.rs` (`sensor_ping_pong_under_500ms`)
- FOUND: commit `eef7533`
- FOUND: commit `858f0ae`
