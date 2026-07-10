---
phase: 09-scripted-proof-harness
plan: 04
subsystem: testing
tags: [uia, rdp, 7-zip, proof-harness, live-gate, terminal-gate, milestone]

# Dependency graph
requires:
  - phase: 09-scripted-proof-harness (09-01)
    provides: "HUMAN-APPROVED window predicate (exact class_name == \"7-Zip::FM\"), D-9.6 seeding form, and the recorded depth-1-only TreeScope_Children dump"
  - phase: 09-scripted-proof-harness (09-02)
    provides: "UiaScope::Subtree { max_depth } deeper-walk capability, sensor-side UIA_MAX_WALK_DEPTH = 4 safety cap"
  - phase: 09-scripted-proof-harness (09-03)
    provides: "Shared run_proof_harness/ProofReport, examples/proof_harness.rs, gated proof_harness_end_to_end test"
provides:
  - "PROOF-01 empirically CLOSED: all four SC#1-4 proven live against a real disposable Azure VM running a freshly AOT-rebuilt (not Phase-8-cached) sensor and the real 7-Zip File Manager"
  - "Live-diagnosed and fixed: proof_harness.rs's navigate step now calls set_foreground_window before clicking (the freshly-launched 7-Zip window was not guaranteed OS focus, so the first click on the File menu was consumed as a pure activation click, not a menu-open)"
  - "Live-tuned SC2_MAX_DEPTH 4 -> 3 (555.2ms -> 130.2ms deeper-walk latency, both under/over the Phase 7 SC#3 500ms budget respectively) with zero loss of SC#2 element coverage (same 30 deeper elements matched at both depths)"
  - "v1.0 milestone CLOSED — the full read/inspect loop (connect -> screenshot -> UIA -> navigate -> report) is proven end-to-end against a real remote-only Windows program with no live LLM"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "set_foreground_window + settle before any click on a freshly-launched (fire-and-forget launch_process) window — launch does not imply OS focus; this is now the second live-diagnosed occurrence of the same class of bug (first at Phase 6 SC#3), and should be treated as a standing convention for any future navigation code in this codebase, not a one-off fix"
    - "Deeper-walk latency scales sharply with UiaScope::Subtree max_depth on a real, densely-populated window (179 total elements at depth=4 vs 50 at depth=3 for the same 7-Zip window) — request the shallowest max_depth that still reaches the target elements, not the sensor's full safety cap by default"

key-files:
  created:
    - .planning/phases/09-scripted-proof-harness/09-04-SUMMARY.md
  modified:
    - crates/rdpilot/tests/support/proof_harness.rs

key-decisions:
  - "SC2_MAX_DEPTH live-tuned from 4 (the 09-03-authored default, matching the sensor's full UIA_MAX_WALK_DEPTH cap) down to 3. At depth=4 the measured get_uia_tree round-trip was 555.2ms against real 7-Zip (179 total elements) — over the Phase 7 SC#3 500ms sensor-side budget (T-09-12). All SC#2-recorded deeper elements (menu items, toolbar buttons) were observed at depth=2 in both runs, so depth=3 (one level of headroom, in case a listview row sits deeper under Pane) is sufficient; re-measured at 130.2ms (50 total elements), same 30 deeper elements matched. The wire-level caller value never exceeds the sensor's own UIA_MAX_WALK_DEPTH=4 hard cap regardless."
  - "[Rule 1 fix] The SC#3 navigate step now calls Session::set_foreground_window(window.hwnd) + a 300ms settle before clicking the deeper element. Root cause of the first live failure: launch_process (D-6.2) is fire-and-forget and does not guarantee the newly-launched window holds OS foreground focus; Phase 6's own SC#3 live diagnosis (set_foreground_window_confirmed_by_followup_query) already established this, and every other live navigation test in the crate already focuses+settles before clicking — the 09-03-authored harness was the one place this convention was missed."
  - "[Rule 2] Added inline latency instrumentation (std::time::Instant) around the get_uia_tree deeper-walk call specifically (isolated from the surrounding launch-poll/verify-poll timing) so this plan's required SUMMARY measurement (T-09-12 mitigation) could be captured and is now a permanent, visible part of the uia_deeper_walk step's detail string for any future run."
  - "VM REUSED, not reprovisioned: the existing rdpilot-test/rdpilot-vm from an earlier stalled session was found healthy (VM running, CSE Succeeded, RDP:3389 reachable) and reused directly, saving the ~6min CSE provisioning wait. It also already carried .NET 8 SDK + VC++ Build Tools 2022 from prior sessions, saving the toolchain-install step of the mandatory sensor rebuild."

requirements-completed: [PROOF-01]

# Metrics
duration: ~2h (VM triage ~5min, sensor rebuild+relay+verify ~15min, live gate iterations incl. one Rule-1 fix + one Rule-3 live-tune + one self-resolved DVC transient ~45min, teardown wait ~5min, checkpoint relay + human-approval round-trip + closeout ~work remainder)
completed: 2026-07-10
---

# Phase 9 Plan 4: Terminal v1 Live Gate (PROOF-01) Summary

**Ran the terminal v1 live gate against a real disposable Azure VM: AOT-rebuilt the sensor ON the VM from current 09-02 source (SHA256 `41a35f8c...`, confirmed byte-identical VM-built vs relayed, confirmed different from the stale Phase 8 cache `7a775b99...`), then proved all four PROOF-01 success criteria end-to-end against the real 7-Zip File Manager — both the gated `proof_harness_end_to_end` test and the human-facing `examples/proof_harness` binary PASS. Live-diagnosed and fixed a missing `set_foreground_window` call before the SC#3 navigation click, and live-tuned the SC#2 deeper-walk depth from 4 to 3 to bring latency (555.2ms -> 130.2ms) under the Phase 7 500ms budget with zero loss of element coverage. PROOF-01 retired; the v1.0 milestone is closed.**

## Performance

- **Duration:** ~2h (rescue session — a prior stalled executor had already provisioned the VM and gone idle; this session triaged, reused it, and completed the gate)
- **Completed:** 2026-07-10
- **Tasks:** 1/1 (single blocking `checkpoint:human-verify` task — all automation performed by the executor per the checkpoint-automation-first protocol, results relayed to the human for the final sign-off, which was received: "APPROVED")
- **Files modified:** 1 (`crates/rdpilot/tests/support/proof_harness.rs`)

## Accomplishments

- **VM triage (rescue-specific):** confirmed the pre-existing `rdpilot-test` resource group / `rdpilot-vm` (52.157.207.20) left by a stalled prior session was HEALTHY — VM running, `Configure-Target` CSE `Succeeded`, RDP:3389 reachable via direct TCP probe — and **reused it** rather than reprovisioning, saving the ~6min CSE wait. The VM also already had .NET 8 SDK (`C:\dotnet8`, 8.0.422) and VC++ Build Tools 2022 installed from earlier sessions, saving the toolchain-install step of the mandatory sensor rebuild.
- **Mandatory sensor rebuild (not cache-reused):** AOT-rebuilt `rdpilot-sensor.exe` ON the VM via `az vm run-command invoke`, embedding a base64 tarball of the current `sensor/` source directly in the script body (via `az ... --scripts @file.ps1`, not `--parameters`, per the Phase 8 CLI size-limit finding). Result: 3,045,376 bytes, SHA256 `41a35f8cc60e0a1ab38c62b8736246518c84c0f7dc8220c25940f1ab9c313f6e`. Relayed back to the local host via a throwaway blob-SAS container in the VM's own resource-group storage account (`Invoke-WebRequest -UseBasicParsing` PUT from the VM side — the default `Invoke-WebRequest` on this Windows Server image fails with a `WebCmdletIEDomNotSupportedException` without that flag); the container was deleted immediately after retrieval. Local download SHA256 confirmed **byte-identical** to the VM-reported hash, and confirmed **different** from the Phase 8 cache hash `7a775b990cca3b40e72d4cd8ce910ebfc8e14262dd660089a4e5c62355ec34c2` recorded in `08-03-SUMMARY.md` — proving the deployed binary is the new 09-02 deeper-walk build, not the stale children-only cache.
- **Live gate run 1 (diagnosis):** SC#1/SC#2/navigate-click all passed, but `verify_navigation` failed after 15x300ms polls — no visible screenshot-diff change around the clicked "File" MenuItem. Root-caused live: the harness's navigate step clicked the deeper element without first calling `set_foreground_window` on the freshly-launched 7-Zip window. `launch_process` is fire-and-forget (D-6.2) and does not guarantee OS foreground focus — this is the identical class of issue Phase 6's own `set_foreground_window_confirmed_by_followup_query` live-diagnosed, and every other live navigation test in this crate already focuses+settles before clicking; the 09-03-authored harness omitted it. **Fixed (Rule 1):** added `session.set_foreground_window(window.hwnd)` + a 300ms settle immediately before the click.
- **Live gate run 2 (post-fix, PASS):** `proof_harness_end_to_end` PASSed (13.84s); `examples/proof_harness` printed `PROOF: PASS` and exited 0 with all four steps `[PASS]`.
- **Latency instrumentation + live-tuning:** added `std::time::Instant`-based timing around the `get_uia_tree(hwnd, UiaScope::Subtree{max_depth})` call (required by this plan's SUMMARY deliverable, T-09-12). At `SC2_MAX_DEPTH=4` (the 09-03 default) the measured latency was **555.2ms** — over the Phase 7 SC#3 500ms budget. Since all SC#2-matched deeper elements were observed at depth=2 in that same run, live-tuned `SC2_MAX_DEPTH` down to 3; re-measured at **130.2ms**, with the identical 30 deeper elements still matched (same `Button "Add"` example, same category coverage) — confirming zero loss of SC#2 coverage from the tighter depth.
- **Final verification runs (both PASS, tuned code):** one transient `Dvc("request timed out after 2000ms")` hit mid-session on a `cargo test` retry — the same class of first-RDP-login `deploy_and_launch` transient documented in Phases 6-8; self-resolved on immediate retry with no code change. Final confirmed measurements below are from the last clean run of each artifact.
- **Offline regression check:** `cargo test -p rdpilot --lib` — 103/103 pass (unchanged); `cargo clippy -p rdpilot --lib` — only the 2 pre-existing, out-of-scope warnings (`input.rs` unused import, `rdpdr_backend.rs` `unnecessary_get_then_check`); `cargo test --test live_session -- --list` — 23 tests, unchanged.
- **Teardown:** `infra/manage-env.ps1 down` run to completion (blocking); confirmed `az group exists -n rdpilot-test` -> `false` and `az resource list -g rdpilot-test` -> `ResourceGroupNotFound`; `rdpilot-mgmt` persists as designed.
- **Checkpoint discipline:** all live-run automation (VM triage, sensor rebuild, both test/example runs, offline regression, teardown) was performed autonomously per the checkpoint-automation-first protocol; the executor did NOT self-approve the blocking `checkpoint:human-verify` — results were relayed up and an explicit human "APPROVED" decision was received before this closeout (SUMMARY write, PROOF-01 retirement, ROADMAP/STATE updates, final commit) proceeded.

## Task Commits

1. **Task 1 (checkpoint:human-verify): rebuild sensor, run terminal live gate, tear down** - `6e58b90` (fix — the live-diagnosed `set_foreground_window` fix + `SC2_MAX_DEPTH` live-tune + latency instrumentation, committed once the live gate PASSed post-fix)

**Plan metadata:** (this SUMMARY + STATE/ROADMAP/REQUIREMENTS update, committed separately per protocol)

## Files Created/Modified

- `crates/rdpilot/tests/support/proof_harness.rs` - Added `set_foreground_window` + settle before the SC#3 navigate click (Rule 1 bug fix); added `Instant`-based latency instrumentation around the SC#2 `get_uia_tree` deeper-walk call and surfaced the measured latency in the `uia_deeper_walk` step's detail string; live-tuned `SC2_MAX_DEPTH` from `4` to `3` with the before/after measurements documented inline.

## Live Gate Result (2026-07-10) — PROOF-01 CLOSED

Ran against `rdpilot-vm` (52.157.207.20, reused, healthy) with the freshly VM-rebuilt sensor (`RDPILOT_SENSOR_EXE=.secrets/sensor-build/rdpilot-sensor.exe`, SHA256 `41a35f8cc60e0a1ab38c62b8736246518c84c0f7dc8220c25940f1ab9c313f6e`):

| SC | Criterion | Result |
|----|-----------|--------|
| Sensor rebuild | AOT-rebuilt on the VM from 09-02 source, hash differs from Phase 8 cache | **PASS** — `41a35f8c...` (new) vs `7a775b99...` (Phase 8 cache); byte-identical VM-built vs relayed |
| SC#1 | Connect, authenticate, screenshot the real 7-Zip File Manager | **PASS** — `1920x1080 pixels` |
| SC#2 | Deeper `UiaScope::Subtree` UIA walk asserts named deeper elements + valid bboxes | **PASS** — 30 deeper elements found (menu items + toolbar buttons, e.g. `Button "Add"` depth=2 bbox `Rect{x:164,y:209,w:43,h:46}`) out of 50 total at `max_depth=3`; latency **130.2ms** (well under the Phase 7 SC#3 500ms budget; the pre-tune `max_depth=4` measurement was 555.2ms, over budget) |
| SC#3 | Navigation action injected + verified via follow-up screenshot | **PASS** — clicked deeper `MenuItem "File"`; `verify_navigation` confirmed a visible screenshot-diff change in the region around the target after 1 poll attempt (post-focus-fix; pre-fix this step failed after 15 attempts) |
| SC#4 | Full loop completes, stdout PASS/FAIL report, correct exit code | **PASS** — `examples/proof_harness` printed a per-step `[PASS]` trace, a final `PROOF: PASS` line, and exited with process code **0** (verified via captured `$?`, not a piped exit code) |

Gated test: `RDPILOT_LIVE=1 cargo test -p rdpilot --test live_session -- --ignored --test-threads=1 proof_harness_end_to_end` — **ok, 1 passed; 0 failed** (13.24s, final tuned run).

No plaintext password appeared in any captured stdout (`grep`-verified against the example's full output; T-09-08 confirmed).

### Deviations from Plan

**1. [Rule 1 - Bug] Missing `set_foreground_window` before the SC#3 navigate click**
- **Found during:** first live run of `proof_harness_end_to_end` (post-sensor-rebuild)
- **Issue:** `verify_navigation` failed — no visible screenshot-diff change after 15x300ms polls around the clicked "File" MenuItem, despite the click round-tripping successfully.
- **Fix:** Added `session.set_foreground_window(window.hwnd)` + a 300ms settle immediately before the click, matching the established convention in every other live navigation test in this crate and Phase 6's own live diagnosis of the identical root cause.
- **Files modified:** `crates/rdpilot/tests/support/proof_harness.rs`
- **Verification:** Re-ran live — `verify_navigation` now confirms a visible change after 1 poll attempt.
- **Committed in:** `6e58b90`

**2. [Rule 3 - live-tune, plan-sanctioned] `SC2_MAX_DEPTH` 4 -> 3**
- **Found during:** the same live run — `uia_deeper_walk` passed, but the newly-added latency instrumentation measured 555.2ms at `max_depth=4`, over the Phase 7 SC#3 500ms budget (T-09-12, explicitly anticipated by this plan's `<threat_model>` and `<how-to-verify>` step 5).
- **Fix:** Tuned `SC2_MAX_DEPTH` down to `3` (all SC#2-matched elements were observed at depth=2; depth=3 keeps one level of headroom). Re-measured live: 130.2ms, identical 30 deeper elements matched.
- **Files modified:** `crates/rdpilot/tests/support/proof_harness.rs`
- **Verification:** Re-ran both the gated test and the example live post-tune — both PASS, latency consistently ~130ms.
- **Committed in:** `6e58b90`

**3. Documented transient (not a deviation): one self-resolved `Dvc` timeout**
- **Found during:** a `cargo test` re-run after the latency instrumentation was added.
- **Detail:** `Dvc("request timed out after 2000ms")` on `deploy_and_launch` — the same class of first-RDP-login transient documented in Phases 6-8 (`08-03-SUMMARY.md`, `05-04-SUMMARY.md`).
- **Outcome:** Self-resolved on immediate retry (test PASSed). No code change made, per the plan's explicit guidance to retry this documented condition before treating it as a real failure.

## Auth Gates

None encountered — Azure CLI session was already authenticated from prior sessions; no interactive credential prompt was reached.

## User Setup Required

None for this plan's own scope. The disposable Azure VM was already provisioned by an earlier (stalled) session and was reused after a health check; teardown was performed and confirmed by this session.

## Known Stubs

None. All four PROOF-01 success criteria are proven against real, live-measured behavior — no hardcoded/mocked assertion paths.

## Threat Flags

None beyond what this plan's own `<threat_model>` (T-09-07 cost/teardown, T-09-08 credential leak, T-09-09 sensor byte-integrity, T-09-12 deeper-walk latency, T-09-SC supply chain) already anticipated and mitigated: teardown confirmed absent before this closeout proceeded (T-09-07); no plaintext password in captured output (T-09-08); VM-built vs relayed sensor confirmed byte-identical and confirmed different from the stale cache (T-09-09); deeper-walk latency measured and live-tuned under budget (T-09-12); zero new dependencies (T-09-SC).

## Self-Check: PASSED

- FOUND: commit `6e58b90` in `git log --oneline`.
- CONFIRMED: `crates/rdpilot/tests/support/proof_harness.rs` contains `set_foreground_window` in the navigate step and `SC2_MAX_DEPTH: u32 = 3`.
- CONFIRMED: `cargo test -p rdpilot --test live_session -- --ignored --test-threads=1 proof_harness_end_to_end` under `RDPILOT_LIVE=1` reports `ok. 1 passed; 0 failed`.
- CONFIRMED: `cargo run -p rdpilot --example proof_harness` under `RDPILOT_LIVE=1` prints `PROOF: PASS` and the captured process exit code is `0`.
- CONFIRMED: `sha256sum .secrets/sensor-build/rdpilot-sensor.exe` = `41a35f8cc60e0a1ab38c62b8736246518c84c0f7dc8220c25940f1ab9c313f6e`, differing from the Phase 8 cache hash `7a775b990cca3b40e72d4cd8ce910ebfc8e14262dd660089a4e5c62355ec34c2`.
- CONFIRMED: `az group exists -n rdpilot-test` -> `false`; `az resource list -g rdpilot-test` -> `ResourceGroupNotFound`; `az group exists -n rdpilot-mgmt` -> `true`.
- CONFIRMED: `cargo test -p rdpilot --lib` -> `103 passed; 0 failed`.

---
*Phase: 09-scripted-proof-harness*
*Completed: 2026-07-10*
