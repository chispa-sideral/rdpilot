---
phase: 06-window-process-perception
plan: 05
subsystem: perception
tags: [ironrdp, dvc, nativeaot, win32, live-gate, azure, foreground-lock]

# Dependency graph
requires:
  - phase: 06-window-process-perception
    provides: "06-01's generalized DVC request/response plumbing + owned perception types, 06-02's four sensor-backed Session methods, 06-03's WindowList handler, 06-04's ProcessTree/SetForegroundWindow/LaunchProcess handlers"
provides:
  - "Four gated live integration tests (window_list_returns_visible_windows, process_tree_returns_pid_parent_name_path, set_foreground_window_confirmed_by_followup_query, launch_process_appears_in_followup_process_tree) — LIVE-VERIFIED PASSING against a real disposable Azure Windows VM"
  - "PERC-01/PERC-02/PERC-04/PROC-01/CAP-02 fully closed out end-to-end (Rust SDK -> DVC -> C# NativeAOT sensor -> real Win32)"
  - "deploy_and_launch idempotency fix for RDP session reuse across reconnects"
  - "SetForegroundWindow AttachThreadInput fix defeating Windows' foreground-lock restriction"
  - "Azure VM Run Command established as the working win-x64 NativeAOT build channel when WinRM Negotiate/Basic auth is unavailable from a Linux client"
affects: [07-uia-tree-module]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "az vm run-command invoke as a WinRM-independent remote PowerShell execution channel, authenticated via the operator's own az session instead of guest credentials — used both for build orchestration and for post-hoc diagnosis (with the caveat that Run Command executes in Session 0 and cannot see into the interactive RDP session's window station)"
    - "A short-lived Azure Storage blob (SAS PUT/GET) as a file-transfer relay between the VM and the local build orchestrator when neither WinRM file copy nor a shared filesystem is available"
    - "launch_command() is now idempotent under RDP session reuse: unconditional taskkill before copy+start, since Windows reconnects a disconnected interactive session rather than creating a fresh one"
    - "Win32 SetForegroundWindow from a windowless background process requires AttachThreadInput to the current foreground thread to bypass the foreground-lock-timeout restriction — a bare call can return TRUE with no Z-order effect"
    - "Z-order confirmation for a focus change must compare against the minimum z_order among TITLED (real, user-facing) windows, not the global minimum across all enumerated windows — always-on-top shell chrome (the taskbar) legitimately outranks every normal application window regardless of focus"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/tests/live_session.rs
    - sensor/WindowControl.cs

key-decisions:
  - "win-x64 NativeAOT builds run ON the disposable Azure VM itself (a real Windows x64 box), driven from this Linux host via `az vm run-command invoke` — NOT WinRM. WinRM Negotiate/NTLM auth failed from this host (missing gssntlmssp GSS mechanism plugin; PSWSMan's patched libmi still needs a system NTLM provider) and Basic auth was rejected server-side. Rather than modify system packages, Azure Run Command (already authenticated via the operator's az session) was used instead — a durable, reusable pattern for any future Linux-hosted live gate against this same VM image."
  - "Sensor source is embedded as base64 directly in the run-command script body (well under Azure's per-script size limit) rather than transferred via blob, since it is small (~80KB total); the built exe (2.5-2.9MB) is retrieved via a short-lived Azure Storage blob SAS PUT/GET relay in the same disposable resource group (torn down automatically with `manage-env.ps1 down`)."

requirements-completed: [PERC-01, PERC-02, PERC-04, PROC-01, CAP-02]

# Metrics
duration: ~58min (live-gate session: VM provisioning through teardown; Task 1's four gated tests were authored in a prior session, commit 1b69c0f)
completed: 2026-07-09
---

# Phase 6 Plan 5: Live Gate — Window + Process Perception Summary

**LIVE GATE PASSED against a real disposable Azure Windows VM: all four Phase 6 success criteria (window list, process tree, foreground focus, remote process launch) verified end-to-end through the real NativeAOT sensor, closing out PERC-01/PERC-02/PERC-04/PROC-01/CAP-02 — with three live-diagnosed bugs fixed along the way (RDP session-reuse hang, Win32 foreground-lock, and a wrong-metric test assertion).**

## Performance

- **Duration:** ~58 min (VM `up` through VM `down`, including two full sensor rebuild/redeploy cycles and iterative live diagnosis)
- **Tasks:** 1 (`checkpoint:human-verify`, executed with explicit user authorization) — Task 1 (four gated tests) was completed in a prior session (commit `1b69c0f`)
- **Files modified:** 3 (`crates/rdpilot/src/session.rs`, `crates/rdpilot/tests/live_session.rs`, `sensor/WindowControl.cs`)

## Feasibility Pre-Check

`pwsh` present, `az` authenticated to subscription "Chispa Sideral" (`az account show` succeeded), `rdpilot-test` RG clear (no leftover environment), `rdpilot-mgmt` persistent RG present as expected. Pre-check PASSED — proceeded to provision.

## Accomplishments — Per-Criterion Results (all PASS)

- **SC#1 / PERC-02 (`window_list_returns_visible_windows`):** PASS. `get_window_list()` returned a non-empty window list with valid HWND/title/rect/z-order/state/pid; the selected candidate's rect landed fully within `session.desktop_size()` (framebuffer pixel space). Soft CAP-02 smoke (`screenshot_window` crop-dimension match) also passed silently.
- **SC#2 / PERC-01 (`process_tree_returns_pid_parent_name_path`):** PASS. `get_process_tree()` returned a non-empty list with at least one record carrying a nonzero pid + non-empty name, and at least one with a non-empty best-effort-resolved path.
- **SC#3 / PERC-04 (`set_foreground_window_confirmed_by_followup_query`):** PASS (confirmed on the very first poll attempt after both live-run fixes below). `set_foreground_window` brought the launched notepad window forward; confirmed via a SECOND `get_window_list()` query showing it topmost among titled windows.
- **SC#4 / PROC-01 (`launch_process_appears_in_followup_process_tree`):** PASS. `launch_process("notepad.exe")` returned a PID that appeared in a follow-up `get_process_tree()` query with a notepad-like name.

## win-x64 NativeAOT Publish Result

Built ON the Azure VM itself (real Windows x64, not a surrogate) via `az vm run-command invoke`, since WinRM Negotiate/NTLM authentication failed from this Linux host (missing `gssntlmssp` GSS mechanism plugin under GSSAPI; PSWSMan's patched libmi still requires a system NTLM provider) and Basic auth was rejected by the WinRM service. Azure Run Command — already authenticated via the operator's own `az` session — was used as a durable substitute, with a short-lived Azure Storage blob (SAS PUT/GET) relaying the sensor source in and the built exe back out.

- `dotnet` was present on the VM but was a runtime-only stub with **zero SDKs installed** — the presence-check silently skipped a real SDK install on the first attempt. Fixed by force-installing .NET 8 SDK 8.0.422 to a clean `C:\dotnet8` directory via `dotnet-install.ps1 -Channel 8.0`.
- VC++ Build Tools (`Microsoft.VisualStudio.Workload.VCTools`, required for the NativeAOT linker) were not present on the fresh VM image; installed via the `vs_buildtools.exe` bootstrapper (exit code 0, ~10-20 min).
- `dotnet publish -c Release -r win-x64 -p:PublishAot=true --self-contained` succeeded twice (once before, once after the `WindowControl.cs` fix), **zero source-gen/trimming warnings** both times.
- Publish dir contains only `rdpilot-sensor.exe` + `.pdb` (no hostfxr/hostpolicy companion, self-contained NativeAOT signature intact).
- First build: 2,864,640 bytes, SHA256 `eb8395975f95112b3093f666e05b833b4c0fea661d8a37a3204cd89c87df4834` — verified identical between VM-built and locally-retrieved (via blob relay).
- Rebuilt after the `AttachThreadInput` fix: 2,866,176 bytes, SHA256 `a9e530798e1e707ba38759b223b72104f4ae5e78b8dabaea57a117da1a962e0d` — again verified identical.

## Task Commits

Task 1 (four gated tests) was committed in a prior session:

1. **Task 1: Add the four gated live tests** — `1b69c0f` (test) — `crates/rdpilot/tests/live_session.rs`

Task 2's live-gate run surfaced three bugs, each fixed and committed atomically:

2. **Live-gate fix 1: idempotent deploy_and_launch under RDP session reuse** — `2982994` (fix) — `crates/rdpilot/src/session.rs`
3. **Live-gate fix 2: AttachThreadInput to defeat SetForegroundWindow lock** — `84c3986` (fix) — `sensor/WindowControl.cs`
4. **Live-gate fix 3: SC#3 confirms topmost-among-titled-windows, not global min** — `b516b6f` (fix) — `crates/rdpilot/tests/live_session.rs`

## Files Created/Modified

- `crates/rdpilot/src/session.rs` — `launch_command()` now unconditionally `taskkill`s any already-running sensor instance before copy+start
- `crates/rdpilot/tests/live_session.rs` — SC#3 test: bounded poll-retry (10 attempts) + comparison against minimum z_order among titled windows only + a failure-path diagnostic window-list dump
- `sensor/WindowControl.cs` — `Focus()` now wraps `SetForegroundWindow` with `AttachThreadInput`/`BringWindowToTop`/`ShowWindow(SW_RESTORE)`

## Decisions Made

See frontmatter `key-decisions`. Summary: win-x64 NativeAOT builds run on the Azure VM itself via `az vm run-command invoke` (not WinRM, which failed to authenticate from this Linux host); sensor source embedded as base64 in the run-command script body; built exe retrieved via a short-lived Storage blob SAS relay in the same disposable RG (torn down automatically with the environment).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `deploy_and_launch` hangs when a prior sensor process is still running in a reused RDP session**
- **Found during:** first armed live-gate run — tests 2-4 (all but the first) failed with "sensor did not respond after 3 launch attempts"
- **Issue:** Windows reconnects a disconnected interactive RDP session (confirmed live via `quser`/`qwinsta`) rather than creating a fresh one for each new client connection to the same account. A prior test's still-running sensor process locked the destination exe, silently failing the plain `copy` and short-circuiting `start` via `&&` — while the old process could no longer answer the new connection's dynamic virtual channel, exhausting the full retry budget with no pong ever arriving.
- **Fix:** `launch_command()` now unconditionally `taskkill`s any already-running instance first (`>nul 2>&1` — "not found" is expected/harmless on the first launch) before copying and starting, making every call idempotent regardless of prior launches in the same reused session.
- **Files modified:** `crates/rdpilot/src/session.rs`
- **Verification:** re-armed run progressed from 1/4 to 3/4 passing (the remaining failure was the separate SC#3 issue below)
- **Committed in:** `2982994`

**2. [Rule 1 - Bug] `SetForegroundWindow` returns TRUE without actually raising Z-order**
- **Found during:** live-gate runs 2-4 — `set_foreground_window_confirmed_by_followup_query` consistently failed with the target window's z_order never moving, even across 10 poll-retry attempts and re-settling
- **Issue:** A bare `SetForegroundWindow` call from this windowless background sensor process — which has received no recent user input of its own — is subject to Windows' foreground-lock-timeout restriction. The Win32 call can return TRUE (call-success) while the target window's actual Z-order position never changes (visual-outcome failure), exactly the risk Pitfall 5 had documented but the handler had not yet mitigated.
- **Fix:** `Focus()` now temporarily attaches this thread's input queue to the current foreground window's input queue via `AttachThreadInput` before calling `SetForegroundWindow` — input-queue attachment is one of the Windows-documented conditions that permits a thread to change the foreground window. `BringWindowToTop` + `ShowWindow(SW_RESTORE)` added alongside for a minimized target. Threads are always detached in a `finally`.
- **Files modified:** `sensor/WindowControl.cs`
- **Verification:** required rebuilding and redeploying the sensor on the VM (SHA256 `a9e530798e1e707ba38759b223b72104f4ae5e78b8dabaea57a117da1a962e0d`); combined with fix 3 below, the live test passed on the first poll attempt
- **Committed in:** `84c3986`

**3. [Rule 1 - Bug] SC#3 test assertion compared against the wrong z_order baseline**
- **Found during:** live-gate diagnosis after fix 2 — the target window's z_order was still not matching the test's expected global minimum, even though a diagnostic window-list dump showed the window visibly frontmost
- **Issue:** The original assertion compared the focused window's z_order against the GLOBAL minimum across ALL enumerated windows, which includes always-on-top OS shell chrome (the taskbar — confirmed live: empty title, rect `y=1040, h=40`, exactly a taskbar strip on a 1920x1080 desktop) that structurally sits above every normal application window regardless of focus. No amount of `SetForegroundWindow` correctness can or should out-rank always-on-top system chrome.
- **Fix:** The follow-up confirmation now compares against the minimum z_order among windows with a non-empty title only (real, user-facing top-level windows) — which is what SC#3's "brings a window forward" means in practice. A bounded 10-attempt poll-retry (settling between attempts) was kept to absorb genuine one-off foreground-lock timing flakiness; a diagnostic window-list dump on failure was added for future live debugging.
- **Files modified:** `crates/rdpilot/tests/live_session.rs`
- **Verification:** full armed 4-test suite passed, SC#3 confirmed on poll attempt 1/10
- **Committed in:** `b516b6f`

---

**Total deviations:** 3 auto-fixed (all Rule 1 — bugs surfaced only by the live gate, none discoverable in the offline Linux sandbox).
**Impact on plan:** All three fixes were required for the live gate to pass at all; no scope creep. The `AttachThreadInput` fix and the test's titled-window z_order metric are both durable, architecturally sound corrections (not workarounds) that will hold for any future live target with a visible taskbar.

## Issues Encountered

WinRM Negotiate/NTLM authentication from this Linux host failed with `SPNEGO cannot find mechanisms to negotiate` (missing `gssntlmssp` GSS mechanism plugin) and Basic auth was rejected server-side. Rather than modify system packages on the host, `az vm run-command invoke` was used instead as a WinRM-independent remote execution channel — already authenticated via the operator's own `az` session. This required a short-lived Azure Storage blob SAS relay for file transfer (Run Command has no native file-copy primitive), and a discovery that Run Command executes in Session 0 (non-interactive), so it could not be used to inspect the interactive RDP session's window state directly during diagnosis — the Rust test's own diagnostic dump (added temporarily, kept for future debugging) was the actual source of the SC#3 root-cause data.

## User Setup Required

None beyond the pre-authorized live-gate execution itself. The user's Azure session (`az account show` — subscription "Chispa Sideral") was already active; no new credentials or manual dashboard steps were required.

## Next Phase Readiness

- Phase 6 (Window + Process Perception) is fully complete and live-verified: PERC-01, PERC-02, PERC-04, PROC-01, and CAP-02 all closed out end-to-end against a real remote Windows desktop.
- Phase 7 (UIA Tree Module) can build directly on the now-proven sensor request/response protocol, the window list's HWND/rect data (needed for UIA tree scoping per-window), and the established live-gate methodology (Azure Run Command as the WinRM-independent remote build/diagnosis channel for this environment).
- No known stubs. The per-window screenshot crop (D-6.1) remains a documented, intentional limitation for occluded/minimized windows (deferred to backlog, not a Phase 6 gap).

## Threat Flags

None beyond what 06-05-PLAN's `<threat_model>` already covers. `AttachThreadInput` in `WindowControl.cs` only attaches/detaches this process's own thread to windows already visible in the same interactive session it is running in (no new trust boundary — mirrors the existing accepted `launch_process("notepad.exe")` capability, T-06-03).

## Self-Check: PASSED

All 4 claimed commit hashes (`1b69c0f`, `2982994`, `84c3986`, `b516b6f`) present in `git log --oneline --all`; all 3 claimed-modified files (`crates/rdpilot/src/session.rs`, `crates/rdpilot/tests/live_session.rs`, `sensor/WindowControl.cs`) show the expected diffs; `az group exists -n rdpilot-test` returns `false` (VM torn down) and `az group exists -n rdpilot-mgmt` returns `true` (persistent management RG intact, as designed).

---
*Phase: 06-window-process-perception*
*Completed: 2026-07-09*
