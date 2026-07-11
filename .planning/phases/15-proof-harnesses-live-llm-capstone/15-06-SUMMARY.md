---
phase: 15-proof-harnesses-live-llm-capstone
plan: 06
subsystem: daemon-cli-mcp-live-verification
tags: [live-gate, azure, daemon-connect, sensor-deployment, uia, computer-tool, deploy-and-launch]

# Dependency graph
requires:
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 05
    provides: "Single live Azure VM (rdpilot-vm) held UP, byte-verified C# sensor exe at .secrets/sensor-build/rdpilot-sensor.exe, fresh .secrets/connection.json"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 01
    provides: "live_daemon_e2e.rs (Linux-hostable orphan-liveness + e2e gate)"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 02
    provides: "live_cli_verbs.rs (CLI-02/03 live-deferred gate)"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 03
    provides: "live_mcp_computer.rs (MCP-04 live half gate)"
provides:
  - "DAEMON-04, SESSION-01/03/04 LIVE-VERIFIED: orphan-liveness (kill-9 + restart) and connect/list/disconnect e2e against a real Azure VM"
  - "CLI-02/03 LIVE-VERIFIED: real screenshot pixels, real click-landing (UIA-verified), real UIA tree shape, real 8 MiB put/get checksum round trip through the CLI surface"
  - "MCP-04 LIVE-VERIFIED: real computer-tool click landing near advertised-space corners on a real Windows target"
  - "The daemon's real (non-fake-connector) Connect path now genuinely deploys and starts the remote sensor -- a production gap that had never been exercised by any prior offline test suite"
affects: [15-07, 15-08]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "rdpilot-config's ResolvedConfig gains a new daemon-local operational field (sensor_binary_path) the SAME way share_root already worked: file->env CONFIG-01 layering, RDPILOT_<FIELD> env var, no wire/flag exposure. Any future daemon-local-only config value should follow this exact shape."
    - "ManagedSession trait extension checklist (now 3rd time this has recurred: desktop_size in 14-01, deploy_and_launch here): the trait's SOLE canonical definition is seams.rs; every fake (lifecycle.rs, registry.rs, server.rs's FakeTestSession, dispatch.rs's 3 inline fakes, and the 3 tests/*.rs integration-test fakes -- 9 total non-canonical implementors) needs the new method added with a trivial canned value. tests/*.rs fakes use their own locally-declared OpFuture<'a,T> type alias, NOT the crate-internal seams::BoxFuture -- easy to typo."
    - "Live-diagnosing a sensor-backed daemon verb: when a real (fake-connector-unset) Connect succeeds but a subsequent sensor round trip times out, check TWO things in order: (1) was ConnectionConfig::sensor_binary_path ever actually set (share_root's own .rdpilot-staging creation is gated on this being Some -- a silent, easy-to-miss coupling), and (2) was Session::deploy_and_launch ever actually called after connect (every rdpilot-crate live test calls it explicitly; nothing in the daemon's Connect path did before this plan)."
    - "UIA verification after a click should default to --scope subtree --max-depth 3 (the Phase 9 D-9.1 live-tuned depth), never --scope children -- children-only scope only reaches Window/ToolBar/Pane/TitleBar/MenuBar (depth 1), never the actual menu items/toolbar buttons/listview rows a click is meant to land on. This has now recurred in CLI-02 (13-06/15-02 authoring) and MCP-04 (15-03 authoring) independently."
    - "7-Zip File Manager persists its OWN remembered window placement (including a stale 'maximized' GetWindowPlacement flag) across process launches, independent of the CURRENT RDP session's real negotiated desktop size. A freshly launched window can report state:\"maximized\" while its real UIA-measurable chrome is narrower than the actual screen. Sending the maximize hotkey to an already-\"maximized\" window is a Windows no-op. The fix: send a restore/minimize hotkey FIRST (unconditionally), THEN maximize -- but a minimized window loses OS foreground focus entirely, so an explicit re-foreground between the two key presses is required, or the second hotkey silently targets the wrong window."

key-files:
  created: []
  modified:
    - crates/rdpilot-daemon/tests/live_daemon_e2e.rs
    - crates/rdpilot-config/src/resolved.rs
    - crates/rdpilot-config/src/resolve.rs
    - crates/rdpilot-daemon/src/dispatch.rs
    - crates/rdpilot-daemon/src/seams.rs
    - crates/rdpilot-daemon/src/lifecycle.rs
    - crates/rdpilot-daemon/src/registry.rs
    - crates/rdpilot-daemon/src/server.rs
    - crates/rdpilot-daemon/tests/crash_restart_reconcile.rs
    - crates/rdpilot-daemon/tests/registry_concurrency.rs
    - crates/rdpilot-daemon/tests/thread_leak_soak.rs
    - crates/rdpilot-cli/src/config_flags.rs
    - crates/rdpilot-cli/tests/live_cli_verbs.rs
    - crates/rdpilot-mcp/src/config_params.rs
    - crates/rdpilot-mcp/src/native_tools.rs
    - crates/rdpilot-mcp/tests/live_mcp_computer.rs

key-decisions:
  - "ConnectionConfig::sensor_binary_path is preserved as unconfigured=legitimate (session-management-only mode), never a hard Connect error -- a caller that never needs perception/input/transfer can still connect without a sensor path configured; only a Some-configured-but-deploy_and_launch-fails path surfaces as a Connect failure (closes the just-opened session first, never a silent partial success)."
  - "The MCP-04 corner-click coordinates are hardcoded, live-tuned literals (not computed from a live UIA probe at test time) -- preserves the test's own binding design constraint (coordinate reasoning never leaves the advertised 1280x800 space; UIA is used only to VERIFY landing, never to CHOOSE the target, Pitfall 3). Recalibrating them required two full rounds of live measurement (round 1 against 7-Zip's stale pre-restore-fix maximize bounds was wrong; round 2 against the true full-screen bounds after the restore-then-maximize fix is correct and reproducibly verified across multiple fresh runs)."

requirements-completed: [DAEMON-04, SESSION-01, SESSION-03, SESSION-04, CLI-02, CLI-03, MCP-04]

# Metrics
duration: ~2.5h (live diagnosis dominated: 2 genuine daemon-side production bugs found+fixed, 2 rounds of CLI-02 UIA/foreground fixes, 3 rounds of MCP-04 maximize/UIA/foreground fixes)
completed: 2026-07-11
---

# Phase 15 Plan 06: Live-Run — Session/CLI/MCP Batched Gate Summary

**Ran the batched cross-platform live gates (DAEMON-04 live + SESSION-01/03/04, CLI-02/03, MCP-04) from the Linux host against the Plan 15-05 Azure VM — surfaced and fixed two genuine daemon-side production bugs (the real Connect path never deployed a sensor at all) plus several live-diagnosed test-authoring fixes (shallow UIA scope, missing foreground-before-click, and 7-Zip FM's own stale-maximize-placement quirk), landing all seven requirements genuinely green.**

## Results — every criterion PASS with measured evidence

### Task 1: DAEMON-04 live + SESSION-01/03/04 (orphan-liveness + e2e)

```
test connect_list_disconnect_e2e_against_a_real_target ... ok
test kill_minus_9_mid_session_then_restart_surfaces_the_orphan_which_is_then_explicitly_reconciled ... ok
test result: ok. 2 passed; 0 failed; finished in 1.02s
```

- **SESSION-01/03/04 e2e:** connect (explicit name `live-e2e`) succeeded; `list` showed exactly one `Live` session with correct id/name/host/`connected_since`; `disconnect` acknowledged.
- **DAEMON-04 orphan-liveness (the critical assertion):** daemon A connected to the real target, was `kill -9`'d mid-session (no graceful teardown), daemon B restarted on the SAME reconciliation-state sink. `list` surfaced the prior session as `Orphaned` (never silently forgotten, host preserved) — never auto-reconnected or auto-killed (D-31). An explicit `Disconnect` reconciled it; a final `list` confirmed it was gone.
- **Fix required:** `unique_temp_root`'s socket path (`<root>/xdg-runtime/rdpilot/daemon.sock`) exceeded Linux's 108-byte `sockaddr_un.sun_path` limit for this file's longer test label (`connect-list-disconnect`) — genuine `io error: path must be shorter than SUN_LEN` bind failure. Shortened the generated path (6-digit nanosecond suffix instead of full precision, shorter fixed prefix).

### Task 2: CLI-02/03 live re-exercise

```
test cli_02_screenshot_and_click_landing_against_a_real_target ... ok
test cli_03_multi_mb_put_get_round_trip_against_a_real_target ... ok
test result: ok. 2 passed; 0 failed; finished in 32.43s
```

- **CLI-02:** connect → launch 7-Zip FM → `perceive window list` finds it → `perceive screenshot` (before, 542796 bytes, PNG-magic confirmed) → `input foreground` + settle → `input click` on a live-selected `MenuItem`/`Button` leaf (id `42-197388` at native (1029,596)) → `perceive uia` confirms `focused: true` → `perceive screenshot` (after, byte-differs from before) → disconnect.
- **CLI-03:** connect → an 8 MiB deterministic payload `put` (reported `bytes_transferred=8388608`, checksum `0ff4d6c0...5b`) → `get` (same `bytes_transferred`/checksum) → on-disk downloaded file size matches → disconnect.
- **Fixes required (in addition to the two shared daemon-side sensor-deployment fixes below):**
  1. `--scope children` only reaches UIA depth-1 (Phase 9 D-9.1 finding recurring) — switched to `--scope subtree --max-depth 3`.
  2. Target-element selection always picked the root `Window` element itself (depth 0, always `focusable: true`, always first in BFS order) — the click landed on the window's raw center, which never moves UIA focus. Now prefers an actual interactive `MenuItem`/`Button` leaf at depth > 0.
  3. A freshly `launch_process`'d window is not guaranteed OS foreground focus (Phase 6/9 finding recurring) — added `input foreground` + a 300ms settle immediately before the click.

### Task 3: MCP-04 live half

```
test computer_click_lands_near_screen_edges_and_corners_mcp04_live ... ok
test result: ok. 1 passed; 0 failed; finished in 14.17s (and again in 26.36s on a second independent run)
```

- connect → `rdpilot_launch` 7-Zip FM → `rdpilot_window_list` finds it → `rdpilot_foreground` → `computer key "win+down"` (forces a real restore/minimize) → re-foreground → `computer key "win+up"` (genuine fresh full-screen maximize, confirmed `state: "maximized"`) → `computer screenshot` (advertised 1280x800, image content block received) → `computer left_click` near `[11, 24]` (advertised) → `rdpilot_uia` confirms an element named `"File"` is `focused: true` → `computer left_click` near `[1202, 8]` (advertised) → `rdpilot_window_list` confirms `state: "minimized"` → disconnect.
- Confirmed reproducible on two independent fresh runs (zombie-window cleanup between runs).

## Live-diagnosed production bugs (the critical findings)

**1. [Rule 1 — critical production bug] The daemon's real `Connect` handler never sourced a sensor binary path at all.**
- **Found during:** Task 2, first attempt — `input launch` timed out with `DVC transport error: request timed out after 500ms`.
- **Root cause:** `dispatch.rs`'s `Connect` arm built a `ConnectionConfig` and set `share_root`, but NEVER called `.sensor_binary_path(...)`. `connect.rs`'s RDPDR static-channel registration (which also pre-creates `share_root`'s own `.rdpilot-staging/` subdirectory) is gated on `cfg.get_sensor_binary_path()` being `Some` — so on every real (non-fake-connector) daemon session, the RDPDR channel was never registered AND `share_root` was never actually created on disk, even though a `share_root` value had been set on the config.
- **Why this was never caught before:** every prior CLI-02/CLI-03/MCP-04 offline test used the fake connector (`RDPILOT_DAEMON_TEST_CONNECTOR`), which never exercises `connect.rs`'s real RDPDR-registration gate at all.
- **Fix:** added `ResolvedConfig::sensor_binary_path` (mirrors `share_root` exactly — daemon-local operational config, `RDPILOT_SENSOR_BINARY_PATH` env var, never a wire field), and `dispatch.rs`'s `Connect` handler now resolves and wires it onto `cfg` when configured.
- **Verification:** full offline workspace suite green; live re-run still hit a DIFFERENT timeout (see bug 2) — confirming this fix was necessary but not sufficient on its own.
- **Committed:** `1156c9a`

**2. [Rule 1 — critical production bug] Nothing in the daemon ever called `Session::deploy_and_launch`.**
- **Found during:** Task 2, second attempt (after bug 1's fix) — `input launch` STILL timed out identically.
- **Root cause:** registering the RDPDR channel only sets up drive redirection; it does not START the remote sensor process. Every single `rdpilot`-crate live test (`tests/live_session.rs`) calls `Session::deploy_and_launch()` explicitly right after connect, before any sensor-mediated request — but nothing in the daemon (`registry.open`/`dispatch.rs`'s `Connect` handler) ever called it.
- **Fix:** added `ManagedSession::deploy_and_launch` to the trait (mirrors the existing `ping`/`desktop_size` seam pattern), the real `Session` impl, and all 9 fake implementors across the crate. `dispatch.rs`'s `Connect` handler now calls it once, only when a sensor path was actually configured (bug 1's `sensor_configured` flag) — a `deploy_and_launch` failure closes the just-opened session and surfaces as a `Connect` failure rather than a silent partial success.
- **Verification:** full offline workspace suite green; live re-run of Task 1 (unaffected, no sensor configured) still passed both tests; Task 2's `input launch` then genuinely succeeded.
- **Committed:** `2ae729f`

These two fixes are load-bearing prerequisites for CLI-02, CLI-03, and MCP-04 all — none of Phase 15's three prior plans (13/14's offline suites, 15-02/15-03's authoring) could have caught this gap, since none of them exercised a real `Connect` against a real remote target with a real sensor. This is exactly the class of finding this batched live-run plan exists to surface.

## Other live-diagnosed fixes (test-authoring, not production bugs)

**3. [Rule 1] `unique_temp_root`'s generated socket path exceeded Linux's SUN_LEN.** See Task 1 above. Committed `02845ec`.

**4. [Rule 1] `live_cli_verbs.rs` never set `RDPILOT_SENSOR_BINARY_PATH` for the auto-started daemon.** Companion fix to bug 1 — `run_cli` now points at the byte-verified `.secrets/sensor-build/rdpilot-sensor.exe`. Committed `a84e6af`.

**5. [Rule 1] CLI-02's UIA scope + target-element selection.** See Task 2 above (fixes 1-3 of that section). Committed `ecc8b1d`.

**6. [Rule 1] MCP-04's UIA scope + genuine-fresh-maximize + foreground-before-second-keypress, three compounding root causes:**
- `verify_focus_via_uia` used `--scope children` (same shallow-scope issue as CLI-02) — switched to `subtree`/`max_depth: 3`.
- 7-Zip FM persists its own remembered "maximized" placement across launches, independent of the session's real negotiated desktop size (confirmed via a direct `Request::DesktopSize` probe showing 1920x1080 while a freshly-launched "maximized" window's own UIA-measured chrome spanned only ~1408px). Sending `"win+up"` to an already-"maximized" window is a genuine Windows no-op. Fix: send `"win+down"` unconditionally first.
- `"win+down"` genuinely MINIMIZES this window (not merely restores it to "normal"), and a minimized window loses OS foreground focus entirely — the follow-up `"win+up"` (a GLOBAL hotkey) was silently landing on the wrong window/context. Fix: an explicit `rdpilot_foreground` between the two key presses.
- Corner coordinates re-tuned against the TRUE full-screen maximize bounds (two full rounds of live measurement; round 1's coordinates, calibrated against the stale pre-restore-fix maximize state, were themselves invalidated by fix #2 above and had to be re-measured).
- Committed: `66d56c5`

**Total: 6 live-diagnosed fixes across 6 commits, all pushed to `develop` on top of `d227e46` (15-05's completion commit).**

## Package Legitimacy

No new package/crate added or version-changed this plan.

## Task Commits

1. `02845ec` — `fix(15-06): shorten unique_temp_root socket path to fit SUN_LEN`
2. `1156c9a` — `fix(15-06): wire sensor_binary_path into the daemon's real Connect path`
3. `2ae729f` — `fix(15-06): daemon Connect now calls deploy_and_launch to start the sensor`
4. `a84e6af` — `fix(15-06): point live_cli_verbs.rs at the byte-verified sensor exe`
5. `ecc8b1d` — `fix(15-06): CLI-02 click-landing -- deeper UIA scope + real-focus target`
6. `66d56c5` — `fix(15-06): MCP-04 live half -- deeper UIA scope + genuine fresh maximize`

## Deviations from Plan

### Auto-fixed Issues

All 6 fixes above are documented in full in the "Live-diagnosed production bugs" and "Other live-diagnosed fixes" sections. Summarized by Rule:

- **[Rule 1 - Bug] x6** (all): genuine bugs directly blocking this plan's stated live-verification tasks from passing — auto-fixed inline, verified, committed per this workflow's Rule 1/3 discipline. Two of the six (sensor_binary_path wiring, deploy_and_launch call) are load-bearing PRODUCTION bugs in `rdpilot-daemon` itself, not test-authoring gaps — flagged prominently above since they affect every future sensor-backed daemon session, not just this plan's own gated tests.

No Rule 2/3/4 deviations beyond the above. No architectural changes. No package-legitimacy checkpoints triggered.

## Issues Encountered

- **VM state accumulation across the day's many test runs:** dozens of leftover `7zFM.exe` processes accumulated on the VM across this and prior plans' live runs (each `launch_process` spawns a fresh instance; nothing tears them down on disconnect). Cleaned up via `taskkill /IM 7zFM.exe /F` (through the daemon) and, for out-of-band diagnosis, `az vm run-command invoke` directly — non-blocking, but worth noting for 15-07/15-08: a stale window can occasionally be grabbed by a naive "first `class_name` match" poll instead of the freshly-launched one. Not fixed in this plan (out of scope — the gated tests themselves tolerate it via UIA-verified assertions rather than trusting window identity), but future live-gate authors should be aware.
- **One transient DVC channel wedge** was observed during ad-hoc manual diagnosis (not during an actual gated test run) after many rapid manual connect/disconnect/launch cycles in a short window — resolved by a fresh connect after a brief pause. Never observed during any of the actual automated `cargo test` runs recorded above; not investigated further (manual diagnostic artifact, not a reproducible test failure).

## User Setup Required

None. VM connectivity, sensor binary, and Azure CLI auth were all already in place from Plan 15-05.

## Next Phase Readiness

- **DAEMON-04, SESSION-01, SESSION-03, SESSION-04, CLI-02, CLI-03, MCP-04 are all Complete** in `REQUIREMENTS.md`.
- **VM is confirmed UP** (`az vm get-instance-view` → `VM running`) and cleaned of zombie `7zFM.exe` processes, held for Plans 15-07/15-08 per the binding constraint (teardown is 15-08's authorized checkpoint only).
- The two production sensor-deployment fixes (`sensor_binary_path` wiring, `deploy_and_launch` call) are now permanent daemon behavior — any future live gate against a real target (15-07's PROOF-02/03, 15-08's PROOF-04 capstone) benefits from a genuinely-functional real `Connect` path without needing to rediscover this gap.
- No blockers for 15-07.

---
*Phase: 15-proof-harnesses-live-llm-capstone*
*Completed: 2026-07-11*

## Self-Check: PASSED

All 16 claimed key-files exist on disk, and all 6 task commit hashes (`02845ec`, `1156c9a`, `2ae729f`, `a84e6af`, `ecc8b1d`, `66d56c5`) are present in `git log --oneline --all`.
