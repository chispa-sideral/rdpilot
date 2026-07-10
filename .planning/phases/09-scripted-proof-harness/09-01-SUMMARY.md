---
phase: 09-scripted-proof-harness
plan: 01
subsystem: testing
tags: [uia, rdp, 7-zip, spike, risk-gate, live-test]

# Dependency graph
requires:
  - phase: 07-uia-tree-module
    provides: "get_uia_tree(hwnd) returning a flat UiaElement[] at TreeScope_Children scope (D-7.4)"
  - phase: 08-public-sdk-api-worldstate
    provides: "full public Session API (connect/deploy_and_launch/launch_process/get_window_list/get_uia_tree/screenshot/close)"
provides:
  - "D-9.1 fidelity decision, HUMAN-APPROVED: 7-Zip File Manager stays the SC#2/SC#3 target; TreeScope_Children alone is NOT sufficient — Plan 09-02 MUST add a scoped, caller-configurable deeper UIA-walk capability before any assertion code is written"
  - "Confirmed real window match predicate: exact class_name == \"7-Zip::FM\""
  - "Confirmed D-9.6 seeding form: launch_process with the full quoted exe path and a quoted seed-path argument — 7zFM.exe \"<path>\" — window title verbatim reflects the seeded path"
  - "RESEARCH Assumption A1 resolved: the sensor's CreateProcessW(lpApplicationName=null) call requires exe to be the FULL, quoted path (C:\\Program Files\\7-Zip\\7zFM.exe); a bare \"7zFM.exe\" fails with LastError=2 (ERROR_FILE_NOT_FOUND)"
  - "Captured live TreeScope_Children dump (5 elements) proving the gap: only the root Window (name=seeded path) and MenuBar (name=\"Application\") carry a distinguishing name; ToolBar/Pane/TitleBar are name-empty; the File/Edit/View/... menu items, toolbar buttons (Add/Extract/Test/Copy/Move/Delete/Info), and listview rows visible on screen are ALL one level deeper than TreeScope_Children reaches"
affects: [09-02-harness-plan, 09-03-live-gate-plan]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Reused the Phase 7 D-7.5 risk-gate pattern verbatim: throwaway dump-only example + blocking checkpoint:human-verify, human reviews a durably captured artifact (not the live session) before any assertion code is written"
    - "Sensor's CreateProcessW(lpApplicationName=null) requires launch_process(exe=<full quoted path>, args=<quoted args>, cwd=None) for any target not on PATH — the exe field is NOT PATH-searched relative to a bare filename"

key-files:
  created: []
  modified: []

key-decisions:
  - "HUMAN-APPROVED (checkpoint): 7-Zip stays the SC#2/SC#3 target — do NOT fall back to Notepad. TreeScope_Children's insufficiency is addressed by flagging a scoped, caller-configurable deeper UIA-walk capability as in-scope for Plan 09-02, not by changing the target program."
  - "HUMAN-APPROVED: real 7-Zip window match predicate is an EXACT match on class_name == \"7-Zip::FM\" (not a title substring match — title reflects the currently-navigated folder and is not stable)."
  - "HUMAN-APPROVED: D-9.6 seeding form confirmed as launch_process(exe=\"\\\"C:\\\\Program Files\\\\7-Zip\\\\7zFM.exe\\\"\", args=Some(\"\\\"C:\\\\Program Files\\\"\"), cwd=None) — both the exe and the seed-path argument must be individually quoted because the sensor concatenates them as a single command line before calling CreateProcessW."
  - "REQUIREMENTS.md deviation (deliberate, documented): PROOF-01 is NOT marked complete by this plan. This plan is a throwaway risk-gate spike (no assertions, no harness, the spike file was deleted per Pitfall 2) — PROOF-01's actual success criteria (connect->screenshot->UIA->navigate->report) are only satisfied once Plans 09-02/09-03 land. PROOF-01 stays 'Pending' in REQUIREMENTS.md's traceability table until the terminal plan of this phase."

requirements-completed: []

# Metrics
duration: ~70min (this session: VM health-check/poll ~7min, live spike run + Rule-3 fix ~10min, VM teardown ~4min, checkpoint digest + SUMMARY/state finalization ~work remainder; excludes an earlier stalled session that authored the spike file and provisioned the VM)
completed: 2026-07-10
---

# Phase 9 Plan 1: D-9.1 7-Zip UIA Fidelity Spike Summary

**Live-ran a throwaway 7-Zip UIA dump spike against a real disposable Azure VM; human-approved decision: 7-Zip stays the SC#2/SC#3 target, but `TreeScope_Children` alone is insufficient — Plan 09-02 must add a scoped, caller-configurable deeper UIA-walk capability, using a human-confirmed exact `class_name == "7-Zip::FM"` window predicate and a human-confirmed `7zFM.exe "<path>"` (full quoted path) seeding form.**

## Performance

- **Duration:** ~70 min (this session, resuming a stalled prior session that had already authored the spike file and provisioned the VM)
- **Completed:** 2026-07-10
- **Tasks:** 2/2 completed (Task 1 auto + Task 2 blocking checkpoint, human-approved)
- **Files modified:** 1 (created then deleted per Pitfall 2 — net zero surviving files)

## Accomplishments

- Resumed a stalled prior session cleanly: reused the already-committed spike file (`b5520ed`) and the already-running `rdpilot-test` VM (waited out its still-`Updating` `Configure-Target` CSE extension, confirmed 7-Zip installed and RDP reachable) instead of re-provisioning.
- Reused the cached, hash-verified sensor build from the Phase 8 live gate (`.secrets/sensor-build/rdpilot-sensor.exe`, SHA256 `7a775b990c...` — byte-identical to `08-03-SUMMARY.md`'s recorded hash; sensor source unchanged since Phase 7-04) instead of rebuilding on the VM — no `az vm run-command invoke` dotnet-publish cycle was needed this run.
- First live run failed (`CreateProcessW failed (LastError=2)`); diagnosed and fixed live (Rule 3 blocking-fix, `a431852`): the sensor's `CreateProcessW(lpApplicationName=null, ...)` call does not PATH-search a bare `"7zFM.exe"` — switched to the full, individually-quoted path. Second live run succeeded.
- Captured the full live dump to a durable artifact (stdout tee'd to a scratch file) and the first-launch screenshot to a PNG, independent of the VM's lifetime, satisfying the plan's cost-control directive to review artifacts rather than the live session.
- Tore the VM down immediately after capture (`infra/manage-env.ps1 down`) and confirmed `az group exists -n rdpilot-test` -> `false`.
- Relayed the checkpoint digest to the orchestrator (no self-approval) and received an explicit human decision approving 7-Zip as the target, the deeper-walk-capability flag for Plan 09-02, the window predicate, and the seeding form.
- Deleted the throwaway spike file (`8632837`) per the checkpoint's Pitfall 2 mandate.

## Task Commits

Each task was committed atomically:

1. **Task 1: Write throwaway spike, provision VM, run it live** - `b5520ed` (feat, prior session) + `a431852` (fix, this session — Rule 3 live-diagnosed blocking-issue fix)
2. **Task 2: Inspect the dump, record findings, remove the spike, tear the VM down** - `8632837` (chore — spike removal, post-human-approval)

**Plan metadata:** (this SUMMARY + STATE/ROADMAP update, committed separately per protocol)

## Files Created/Modified

- `crates/rdpilot/examples/spike_7zip_uia_dump.rs` - Created (`b5520ed`), live-fixed (`a431852`), then deleted (`8632837`) per Pitfall 2 — throwaway by design, does not survive into Wave 2/09-02.

## The captured live dump (findings — the explicit input to Plan 09-02)

**Real observed window (HUMAN-CONFIRMED as the Plan 09-02 match predicate):**
`class_name = "7-Zip::FM"` (use an EXACT match — the title reflects the currently-navigated folder and is NOT stable, e.g. it read `"C:\Program Files\"` here purely because of the D-9.6 seed path).

**Flat `UiaElement[]` at `TreeScope_Children` of the window (5 elements total):**

| # | role | name | bbox (x,y,w,h) | depth |
|---|------|------|----------------|-------|
| 0 | Window | `"C:\Program Files\"` (= seeded path) | (130,130,1440,759) | 0 |
| 1 | ToolBar | *(empty)* | (138,181,1424,52) | 1 |
| 2 | Pane | *(empty)* | (138,233,1424,648) | 1 |
| 3 | TitleBar | *(empty)* | (154,133,1408,28) | 1 |
| 4 | MenuBar | `"Application"` | (138,161,1424,19) | 1 |

**The gap (HUMAN-CONFIRMED as the driver for Plan 09-02's scope change):** the captured screenshot shows a completely normal 7zFM window with a fully populated, visible menu bar (File/Edit/View/Favorites/Tools/Help), toolbar (Add/Extract/Test/Copy/Move/Delete/Info), and a 17-row listing of `C:\Program Files\` — but **none of those individually-nameable elements appear in the flat dump**. Only their four depth-1 CONTAINER elements (ToolBar/Pane/TitleBar/MenuBar) are reachable via `get_uia_tree(hwnd)`'s current `TreeScope_Children`-only scope (D-7.4). The actual menu items, toolbar buttons, and listview rows are one tree level deeper.

**D-9.1 decision (HUMAN-APPROVED):** 7-Zip's UIA tree is fidelity-ADEQUATE for the Core Value narrative and stays the SC#2/SC#3 target — the Notepad fallback is explicitly NOT invoked. The inadequacy is scoped, not fundamental: it is entirely explained by `TreeScope_Children`'s locked depth (D-7.4), not by 7-Zip exposing opaque/unlabeled controls. **Plan 09-02 MUST add a scoped, caller-configurable deeper UIA-walk capability** (e.g. an explicit depth parameter, or a walk-from-element-id entry point) as a flagged, scoped capability addition (not a silent absorption, not a redesign) before any SC#2/SC#3 assertion code is written — this directly supersedes RESEARCH's illustrative `launch_7zip_and_find_window` code example's implicit assumption that `TreeScope_Children` alone would suffice.

**D-9.6 seeding (HUMAN-CONFIRMED, RESEARCH A1 resolved):** `launch_process` invoked with the FULL, individually-quoted exe path and a quoted seed-path argument — `exe = "\"C:\\Program Files\\7-Zip\\7zFM.exe\""`, `args = Some("\"C:\\Program Files\"")`, `cwd = None` — reliably launches 7zFM.exe pre-navigated to the seeded folder (the window title verbatim reflects it). A bare, unqualified `"7zFM.exe"` fails: the sensor's `CreateProcessW` call passes `lpApplicationName = null` (`sensor/ProcessLaunch.cs`), so Win32's standard command-line search algorithm applies to `exe`, which does not include 7-Zip's install directory. No first-run dialog or other unexpected first-launch UI was observed (Open Q#2 closed).

## Decisions Made

- 7-Zip File Manager remains the Phase 9 SC#2/SC#3 target (no fallback to Notepad) — see "D-9.1 decision" above.
- Plan 09-02's scope explicitly grows to include a scoped, caller-configurable deeper-than-`TreeScope_Children` UIA-walk capability, flagged (not silently absorbed) per the CONTEXT's "pure composition... UNLESS the D-9.1 spike proves a needed capability" carve-out.
- The window match predicate for Plan 09-02/09-03 is `class_name == "7-Zip::FM"` (exact match), never a title substring.
- The D-9.6 seeding form is `launch_process` with the full quoted exe path + quoted seed-path argument, as captured above.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `launch_process("7zFM.exe", ...)` failed live with `CreateProcessW LastError=2`**
- **Found during:** Task 1 (live spike run against the disposable VM)
- **Issue:** The bare filename `"7zFM.exe"` is not resolvable by the sensor's `CreateProcessW(lpApplicationName=null, ...)` call — it is not on the remote `PATH` and the sensor's own working directory has no relation to 7-Zip's install location.
- **Fix:** Changed the spike's `exe` argument to the full, quoted install path (`"C:\Program Files\7-Zip\7zFM.exe"`) and quoted the D-9.6 seed-path argument (both required, since the sensor concatenates `exe` + `args` into one command line before calling `CreateProcessW`).
- **Files modified:** `crates/rdpilot/examples/spike_7zip_uia_dump.rs`
- **Verification:** Second live run succeeded — window launched, seeded correctly, dump + screenshot captured.
- **Committed in:** `a431852`

**2. Non-code deviation: PROOF-01 intentionally NOT marked complete**
- **Found during:** State-update step (post-checkpoint finalization)
- **Issue:** This plan's frontmatter carries `requirements: [PROOF-01]` (inherited phase-level tagging), but this plan is a throwaway risk-gate spike only — no assertions, no harness, the spike file was deleted. PROOF-01's actual success criteria require a real connect->screenshot->UIA->navigate->report loop, which does not exist until Plans 09-02/09-03 land.
- **Fix:** `requirements-completed: []` in this SUMMARY's frontmatter; `requirements.mark-complete` was deliberately NOT invoked for PROOF-01. REQUIREMENTS.md's traceability table stays "Pending" for PROOF-01 until the phase's terminal plan.
- **Files modified:** none (a decision not to write, not a code change)
- **Verification:** N/A (documentation-accuracy decision, not a correctness/security fix)

---

**Total deviations:** 2 (1 auto-fixed Rule 3 blocking-issue, 1 documented protocol deviation for requirements-tracking accuracy)
**Impact on plan:** The Rule 3 fix was necessary to complete the live run at all; the requirements-tracking deviation prevents REQUIREMENTS.md from claiming PROOF-01 is done when the actual harness doesn't exist yet. No scope creep — both keep the plan's deliverable accurate.

## Issues Encountered

- The `rdpilot-test` resource group's `Configure-Target` CSE extension was still reporting `provisioningState: Updating` for several minutes into this session (VM had been created ~9 min prior by the stalled session) — this blocked `az vm run-command invoke` with a `Conflict` error ("Run command extension execution is in progress"). Resolved by polling until the CSE settled to `Succeeded` (~6 min total) before proceeding; no code change needed, this was VM-provisioning-time latency (consistent with the CSE's own multi-minute 7-Zip-install-plus-registry-mutation work), not a defect.

## User Setup Required

None - no external service configuration required (Azure CLI session was already authenticated from the prior stalled session's context).

## Next Phase Readiness

- **Ready for Plan 09-02:** the exact, human-approved inputs are now recorded above — window predicate, seeding form, and (critically) the requirement to add a scoped deeper-than-`TreeScope_Children` UIA-walk capability before writing any SC#2/SC#3 assertion. The planner for 09-02 should design this capability's shape (depth parameter vs. walk-from-element-id, etc.) as its own explicit, flagged addition per the CONTEXT's carve-out — not silently absorbed into existing `get_uia_tree`.
- **No blockers.** VM torn down and confirmed absent; no live target is currently provisioned (09-02's authoring wave can proceed offline; a fresh VM provisioning will be needed for 09-02/09-03's own live gates).
- **PROOF-01 remains "Pending"** in REQUIREMENTS.md — expected to close at the phase's terminal live-gate plan.

## Known Stubs

None. The throwaway spike was deleted; no stub/placeholder code was introduced or left behind.

## Threat Flags

None. This plan introduced no new network endpoint, auth path, file-access pattern, or schema change beyond what the threat model's T-09-01/T-09-02/T-09-03 register already anticipated (credential-redaction discipline held — no password appeared in the captured dump/screenshot; VM was torn down per T-09-02's mitigation; the spike was deleted per T-09-03's mitigation).

## Self-Check: PASSED

- FOUND: commit `b5520ed` in `git log --oneline`.
- FOUND: commit `a431852` in `git log --oneline`.
- FOUND: commit `8632837` in `git log --oneline`.
- CONFIRMED: `crates/rdpilot/examples/spike_7zip_uia_dump.rs` does not exist on disk (deleted, `8632837`).
- CONFIRMED: `az group exists -n rdpilot-test` -> `false`.

---
*Phase: 09-scripted-proof-harness*
*Completed: 2026-07-10*
