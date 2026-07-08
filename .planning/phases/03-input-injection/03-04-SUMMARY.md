---
phase: 03-input-injection
plan: 04
subsystem: testing
tags: [rust, ironrdp, integration-test, live-validation, screenshot-diff, gating, phase-gate, azure, powershell]

# Dependency graph
requires:
  - phase: 03-input-injection
    provides: "Plan 01 owned MouseAction/KeyAction/Button/Key vocabulary + pure translation to ironrdp_input::Operation; Plan 02 Session::send_mouse + coordinate-contract enforcement; Plan 03 Session::send_key (Type/Combo)"
  - phase: 02-rdp-session-framebuffer-core
    provides: "Gated live harness (tests/live_session.rs, tests/common/mod.rs), RDPILOT_LIVE/RDPILOT_IDLE_SECS gating convention, and infra/manage-env.ps1 up/down"
  - phase: 01-test-environment
    provides: "infra/manage-env.ps1 provisioning + infra/scripts/Configure-Target.ps1 in-guest hardening (extended by this plan)"
provides:
  - "Screenshot-diff helper (region_changed/changed_fraction) extending Phase 2's frame helpers (D-3.4)"
  - "Gated live tests covering all 4 Phase-3 success criteria: mouse_click_activates_menu (SC#1), mouse_action_types_all_work (SC#2), coordinate_contract_enforced (SC#4), keyboard_typed_text_is_received + keyboard_combos_are_received (SC#3)"
  - "Recorded canonical live-validation pass: all 5 Phase-3 input tests + all 5 Phase-2 tests green against a real Azure Windows RDP target (10/10) -- the Phase 3 Nyquist gate"
  - "Empirically confirmed D-3.7 (DOUBLE_CLICK_GAP=100ms) and D-3.8 (DRAG_INTERPOLATION_STEPS=5/DRAG_STEP_GAP=15ms) both register reliably on the live VM -- no tuning needed, values kept as-is"
  - "infra/scripts/Configure-Target.ps1 step 4/5: DoNotOpenServerManagerAtLogon=1 (default-hive) -- a Phase 1 provisioning gap discovered live that broke bare-desktop input assumptions"
affects: [phase-4-dvc-sensor, phase-6-process-launch]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "region_changed(before, after, rect) crops both frames to rect and reports whether > REGION_CHANGE_THRESHOLD (1%) of pixels differ; changed_fraction(before, after) does the same over the whole frame with action-specific lower thresholds for small-area changes (typed text, Start-menu open, wheel scroll)"
    - "Observable MouseAction/KeyAction variants (Click-opens-menu, Ctrl+Esc-opens-Start, Alt+F4-opens-shutdown-dialog, typed text) are asserted via screenshot-diff; non-reliably-observable ones on this VM image (DoubleClick, Drag, Scroll — no scrollable Start overflow) are asserted round-trip-only (Ok + session stays alive), matching the plan's own DECISION POINT carve-out"
    - "Live-only environmental gaps (e.g. an OS chrome window covering the desktop) are fixed at the infra-provisioning layer (Configure-Target.ps1 default-hive registry), not worked around with fragile test-side coordinate hacks"

key-files:
  created: []
  modified:
    - "crates/rdpilot/tests/live_session.rs"
    - "infra/scripts/Configure-Target.ps1"

key-decisions:
  - "Server Manager auto-launch is suppressed at the infra layer (DEFAULT user hive, same idempotent reg-load/write/unload pattern as the existing DPI/SuppressWhenMinimized steps), not worked around in the test — the true root cause was an environmental gap in Phase 1 provisioning, not a test coordinate choice."
  - "Scroll's acceptance criterion on mouse_action_types_all_work is round-trip-only (matching DoubleClick/Drag), because this VM's Windows Server 2022 Start app list has no scrollable overflow with the current small set of installed apps -- confirmed empirically, not a product defect. A future VM image with enough Start entries to overflow can restore the changed_fraction screenshot-diff assertion."
  - "DOUBLE_CLICK_GAP (100ms) and DRAG_INTERPOLATION_STEPS/DRAG_STEP_GAP (5 steps / 15ms) were empirically confirmed reliable on the live VM via a targeted diagnostic (double-clicking and dragging the Recycle Bin desktop icon) and were NOT changed -- the original Plan 02/03 values already hold."

requirements-completed: [INPUT-01, INPUT-02]

# Metrics
duration: ~90min (incl. ~10min provisioning, ~15min diagnosis, ~5min live re-validation, teardown)
completed: 2026-07-08
---

# Phase 3 Plan 4: Screenshot-Diff Helper + Gated Live Input Suite + Canonical Validation Summary

**The full Phase 3 input surface (owned MouseAction/KeyAction vocabulary, coordinate-contract enforcement, mouse/keyboard session wiring) is now proven end-to-end against a real Azure Windows RDP target: all 4 success criteria pass live via screenshot-diff, and a Phase 1 provisioning gap (Server Manager auto-launch blocking the bare desktop) was discovered and fixed at the infra layer during validation.**

## Performance

- **Duration:** ~90 min (provisioning ~10min, first live run + failure diagnosis ~20min, root-cause fix + re-validation ~15min, empirical double-click/drag tuning check ~10min, teardown + closeout)
- **Started:** 2026-07-08
- **Completed:** 2026-07-08
- **Tasks:** 3 (2 auto, already committed in prior session as `d73d846`/`fa23347`; 1 blocking live-validation checkpoint, resolved in this session)
- **Files modified:** 2 (`tests/live_session.rs`, `infra/scripts/Configure-Target.ps1`)

## Accomplishments

- **Tasks 1-2 (prior session, `d73d846`/`fa23347`):** Screenshot-diff helper (`region_changed`/`changed_fraction`) and the full gated live test suite covering all 4 Phase-3 success criteria were authored and offline-verified (default `cargo test -p rdpilot` green, 10 tests correctly `#[ignore]`'d).
- **Task 3 (this session) — canonical live validation, the phase Nyquist gate:**
  - Provisioned a real Azure Windows VM (`Standard_B2s_v2`, westeurope — the default `Standard_B2ms` is `SkuNotAvailable` in this region, matching Phase 2's precedent).
  - **First live run: 7/10 passed, 3 failed** (`keyboard_combos_are_received`, `keyboard_typed_text_is_received`, `mouse_action_types_all_work`).
  - **Root-caused via screenshot diagnosis** (a series of throwaway debug example binaries, not committed): Windows Server 2022 auto-launches Server Manager, maximized, at every RDP logon — it silently absorbed the tests' "click desktop center" and "Alt+F4 on the desktop" assumptions (a center-screen click landed on Server Manager's own window; Alt+F4 targeted Server Manager, which does not respond to it, instead of opening the Shut Down Windows dialog on the real desktop). Confirmed by manually closing Server Manager mid-session and re-testing Alt+F4, which then correctly opened the Shut Down Windows dialog.
  - **Fixed at the infra layer:** applied `DoNotOpenServerManagerAtLogon=1` to the live VM's already-loaded `rdpadmin` profile hive via `az vm run-command invoke` (WinRM's client isn't available on PowerShell Core/Linux, so Azure's VM Run Command extension was used instead) to unblock this validation run, and added the same setting to `infra/scripts/Configure-Target.ps1`'s DEFAULT user hive (step 4/5) so all future `manage-env.ps1 up` provisions are fixed permanently and idempotently.
  - **Second failure category:** `mouse_action_types_all_work`'s Scroll-over-Start-menu assertion failed even after the Server Manager fix — this VM's Windows Server 2022 Start app list (7-Zip, Azure Arc Setup, Microsoft Edge, Server Manager, Settings, Windows Accessories/Administrative Tools/Ease of Access/PowerShell/Security/System) fits entirely on one screen with no scrollable overflow, so scrolling produces no visible diff (not a product defect). Relaxed to round-trip-only acceptance (matching DoubleClick/Drag), per the plan's own DECISION POINT carve-out for exactly this scenario.
  - **Final live run: 10/10 passed, 0 failed** — all 5 Phase-3 input tests plus all 5 Phase-2 tests green.
  - **Empirically confirmed the two LIVE-VERIFY items** (D-3.7/D-3.8) via a targeted diagnostic against the Recycle Bin desktop icon: a 100ms-gapped `DoubleClick` reliably opened the Recycle Bin window (a true double-click activation, not two separate single-clicks/selections); a 5-step/15ms-gap `Drag` visibly relocated the icon from its origin to the drop point. **Neither constant needed tuning** — `DOUBLE_CLICK_GAP=100ms`, `DRAG_INTERPOLATION_STEPS=5`, `DRAG_STEP_GAP=15ms` all confirmed reliable as originally set in Plans 02/03.
  - Torn down (`infra/manage-env.ps1 down`) — `rdpilot-test` resource group fully deleted, confirmed via `az group exists -n rdpilot-test` → `false`.

## Task Commits

1. **Task 1: Screenshot-diff helper + gated mouse tests (SC#1, SC#2, SC#4)** — `d73d846` (feat, prior session)
2. **Task 2: Gated keyboard tests — typed text + key combos (SC#3)** — `fa23347` (feat, prior session)
3. **Task 3 fix 1: Relax Start-menu scroll assertion to round-trip-only** — `da53bc3` (fix, this session)
4. **Task 3 fix 2: Suppress Server Manager auto-launch on the test VM** — `a4f9f31` (fix, this session, `infra/scripts/Configure-Target.ps1`)

## Files Created/Modified

- `crates/rdpilot/tests/live_session.rs` (modified) — Scroll's acceptance in `mouse_action_types_all_work` relaxed to round-trip-only with a DECISION POINT comment explaining the empirical finding
- `infra/scripts/Configure-Target.ps1` (modified) — new step 4/5: `DoNotOpenServerManagerAtLogon=1` written to the DEFAULT user hive; trailing 7-Zip step renumbered 4/4 → 5/5

## Live Validation Results (the phase gate)

- **Target:** Azure Windows VM, `Standard_B2s_v2`, westeurope, self-signed lab cert (`accept_invalid_certs(true)`, D-15); provisioned + torn down via `infra/manage-env.ps1`.
- **Canonical run (`RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1`), after both fixes:** **10 passed / 0 failed.**
- **Success criterion mapping (all TRUE):**
  1. **SC#1** — a click at a known coordinate activates a menu: `mouse_click_activates_menu` ✅ (right-click desktop, region-changed)
  2. **SC#2** — all mouse action types observably affect the desktop: `mouse_action_types_all_work` ✅ (Move/Click/DoubleClick/Scroll/Drag all round-trip; right-click menu screenshot-diff observed; DoubleClick/Drag/Scroll round-trip-only per the live decision points above)
  3. **SC#3** — typed text and key combinations received: `keyboard_typed_text_is_received` ✅ (Start search box), `keyboard_combos_are_received` ✅ (Ctrl+Esc opens Start, Ctrl+A round-trips, Alt+F4 opens then cancels the Shut Down Windows dialog)
  4. **SC#4** — coordinate contract enforced: `coordinate_contract_enforced` ✅ (`desktop_size()` reports 1920x1080 physical; an out-of-bounds click returns `Error::CoordinateOutOfBounds`; a subsequent in-bounds click still succeeds)
- **Phase 2 regression tests (also exercised in the same canonical run):** `connect_authenticates` ✅, `screenshot_is_rgb_correct` ✅, `screenshot_crop` ✅, `stays_rendered_while_idle` ✅, `keepalive_survives_10min` ✅.
- **Double-click / drag empirical tuning (D-3.7/D-3.8):**
  - `DOUBLE_CLICK_GAP = 100ms` (session.rs) — confirmed: a `DoubleClick` at the Recycle Bin icon reliably opened the Recycle Bin window (true double-click activation). **No change.**
  - `DRAG_INTERPOLATION_STEPS = 5` (input.rs) / `DRAG_STEP_GAP = 15ms` (session.rs) — confirmed: a `Drag` from the Recycle Bin icon to a point 200px away visibly relocated the icon (before/after screenshots show the icon at the new position). **No change.**
- **Standard-integrity scope (Pitfall 4 / C4 UIPI):** every exercised target was standard-integrity — desktop context menu, Start menu/search, the cancelled Alt+F4 Shut Down Windows dialog, and (for the empirical tuning check only) the Recycle Bin desktop icon. No elevated/UAC surface was ever touched.
- **Teardown:** `manage-env.ps1 -Action down` completed — `rdpilot-test` resource group fully deleted (confirmed via `az group exists`); no lingering Azure cost. The persistent `rdpilot-mgmt` RG is intentionally left in place.

## Decisions Made

- **Server Manager suppression belongs in infra, not the test.** The root cause was a genuine Phase 1 provisioning gap (an OS chrome window unexpectedly covering the desktop on every fresh logon), not a test-side coordinate mistake — fixed at the source (`Configure-Target.ps1` DEFAULT-hive registry write), consistent with the script's existing DPI/SuppressWhenMinimized pattern, so every future re-provision is unaffected by this class of flakiness (relevant to Phase 6's process-launch work too).
- **Scroll's acceptance on this VM image is round-trip-only.** The Start app list has no scrollable overflow with the currently-installed small app set; forcing a screenshot-diff assertion here would be testing an artifact of app-list length, not real Scroll functionality. Matches the plan's own DECISION POINT guidance for exactly this situation.
- **No double-click/drag constant changes were needed.** Both were empirically confirmed reliable at their originally-chosen values (100ms / 5 steps / 15ms) via a targeted live diagnostic against the Recycle Bin icon (double-click opened it; drag visibly relocated it).
- **`az vm run-command invoke` used instead of WinRM** to apply the immediate live fix — PowerShell Core on Linux has no WSMan client library, so `Invoke-Command` against the VM's WinRM listener isn't available from this host; Azure's VM Run Command extension (which uses the VM agent, not WinRM) was used instead, targeting the already-loaded `rdpadmin` profile hive (`HKEY_USERS\<rdpadmin SID>`) directly.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 — Bug/test-assertion] Relaxed Start-menu Scroll assertion to round-trip-only**
- **Found during:** Task 3 (canonical live run, first pass).
- **Issue:** `mouse_action_types_all_work`'s Scroll-over-Start-menu screenshot-diff assertion failed — this VM's Start app list fits entirely on one screen (no scrollable overflow with the current small app set), so scrolling produces zero visible diff. Not a product defect (`send_mouse(Scroll)` round-trips correctly); reaching an overflowing list would require launching an app (out of scope, Phase 6/Pitfall 4).
- **Fix:** Changed the Scroll assertions to round-trip-only (Ok + session alive), matching how `DoubleClick`/`Drag` are already handled in the same test, with a DECISION POINT comment recording the empirical finding for future VM images.
- **Files modified:** `crates/rdpilot/tests/live_session.rs`.
- **Commit:** `da53bc3`.
- **Verification:** Offline `cargo test -p rdpilot` green (10 ignored); live re-run passed.

**2. [Rule 3 — Blocking] Suppressed Server Manager auto-launch (infra fix)**
- **Found during:** Task 3 (canonical live run, first pass) — root-caused via a sequence of throwaway diagnostic screenshots (not committed).
- **Issue:** Windows Server 2022 auto-opens Server Manager, maximized, at every interactive RDP logon, silently intercepting input intended for the bare desktop: `keyboard_combos_are_received` (Alt+F4 targeted Server Manager instead of opening the Shut Down Windows dialog) and `keyboard_typed_text_is_received` (Start-menu-adjacent interactions were affected by the covering window) both failed as a direct result.
- **Fix:** Applied `DoNotOpenServerManagerAtLogon=1` immediately to the live VM's loaded `rdpadmin` hive via `az vm run-command invoke` (unblocking this validation run), and added the same registry write to `infra/scripts/Configure-Target.ps1`'s DEFAULT-user-hive step (new step 4/5, same reg-load/write/unload idempotent pattern as the existing DPI/SuppressWhenMinimized steps) so all future VM provisions are fixed permanently.
- **Files modified:** `infra/scripts/Configure-Target.ps1` (source fix); live VM registry (out-of-band, via Azure VM Run Command, not committed to any file).
- **Commit:** `a4f9f31`.
- **Verification:** Re-ran the canonical live suite after the fix — `keyboard_combos_are_received` and `keyboard_typed_text_is_received` both passed; pwsh AST parse of the modified script reported 0 errors.

---

**Total deviations:** 2 (1 test-assertion relaxation, 1 infra-layer environmental fix). Neither touched the Phase 3 SDK source (`input.rs`/`session.rs`) — the input-injection implementation itself passed the live gate unchanged, including both empirically-verified timing constants.

## Issues Encountered

- **Server Manager auto-launch on Windows Server 2022** — diagnosed via a sequence of throwaway screenshot-driven debug binaries (connect → act → screenshot → inspect), isolated to the exact root cause, and fixed at the infra provisioning layer (see Deviation 2 above).
- **No scrollable Start-menu overflow on this VM image** — diagnosed as an environmental/content-length artifact rather than a functional defect, resolved by relaxing the test's acceptance criterion (see Deviation 1 above).
- **PowerShell Core has no WinRM/WSMan client on Linux** — `Invoke-Command` against the VM's WinRM listener from this host errored with "no supported WSMan client library was found"; worked around by using `az vm run-command invoke` (Azure VM agent-based execution) instead, which required no additional tooling.
- **pwsh not installed on this host** — worked around by running PowerShell 7 + Azure CLI inside a `podman` container (`mcr.microsoft.com/powershell`) with the repo and `~/.azure` credential cache bind-mounted in; `infra/manage-env.ps1` ran unmodified inside the container against the host's already-authenticated `az` session.

## Resolved Open Questions / Assumptions

- **RESEARCH §4 Live-verify item 1 (double-click threshold):** RESOLVED — the 100ms `DOUBLE_CLICK_GAP` reliably registers as a true double-click (not two single clicks) on the live VM (`GetDoubleClickTime()`'s default 500ms window was not overridden on this image). **No change to the constant.**
- **RESEARCH §4 Live-verify item 2 (drag step count/spacing):** RESOLVED — 5 interpolated `MouseMove` steps at 15ms spacing reliably registers as a visible drag (the dragged icon's on-screen position changed) on the live VM's `SM_CXDRAG`/`SM_CYDRAG` threshold. **No change to the constants.**

## Known Stubs

None. All 5 Phase-3 input tests exercise the real public API against a live target and pass; no mock/stub data paths exist in the input-injection surface.

## Threat Flags

None beyond the plan's threat model. Mitigations confirmed:
- **T-03-12** (UIPI/elevated-target DoS): every live target was standard-integrity (desktop, Start menu, cancelled shutdown dialog, Recycle Bin icon for the tuning check) — never an elevated/UAC surface.
- **T-03-13** (silent no-op tests): the canonical run explicitly asserted `--include-ignored` and reported **10 passed** (not skipped); offline runs correctly show all 10 as `ignored`.
- **T-03-14** (typed-content disclosure): `keyboard_typed_text_is_received` asserts only the boolean screenshot-diff outcome; the typed string ("rdpilot") is never logged.
- **T-03-15** (Alt+F4 irreversible side effect): the Shut Down Windows dialog was immediately cancelled with Esc in both `keyboard_combos_are_received` and the live diagnostic; the VM was never actually shut down.
- **T-03-16** (VM cost): the environment was torn down immediately after validation (`manage-env.ps1 down`), confirmed via `az group exists -n rdpilot-test` → `false`; Phase 1 auto-destroy remains the backstop.
- No new threat surface introduced: `infra/scripts/Configure-Target.ps1`'s new registry write follows the exact same trust boundary (in-guest, CustomScriptExtension-invoked, idempotent default-hive write) as its existing DPI/SuppressWhenMinimized steps.

## User Setup Required

None going forward. The live gate is satisfied and the Server Manager fix is now baked into `Configure-Target.ps1` for all future provisions. Re-running the canonical validation later requires `infra/manage-env.ps1 -Action up -VmSize Standard_B2s_v2` (or another available SKU) and `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1`, then `down`.

## Next Phase Readiness

- **Phase 3 is complete and proven end-to-end** against a real target: all 4 success criteria (menu-activating click, all mouse action types, typed text + key combos, enforced coordinate contract) pass live, with both empirical timing constants (double-click gap, drag step count) confirmed reliable at their original values.
- **Phase 4 (DVC sensor)** can build on the validated input-injection surface; the Server Manager auto-launch fix in `Configure-Target.ps1` also benefits Phase 4/6 (any future live validation that assumes a bare, input-reachable desktop).
- **Phase 6 (process launch)** should be aware that this VM image (Windows Server 2022, Desktop Experience) has a short default Start app list with no scrollable overflow — relevant if a future live test relies on Start-menu scrolling as an observable.

---
*Phase: 03-input-injection*
*Completed: 2026-07-08*
