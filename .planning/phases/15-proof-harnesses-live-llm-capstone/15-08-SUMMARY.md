---
phase: 15-proof-harnesses-live-llm-capstone
plan: 08
subsystem: proof-harnesses-live-llm-capstone
tags: [proof-04, live-gate, azure, claude-p, mcp-config, capstone, sensor-binary-path, teardown-pending]

# Dependency graph
requires:
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 04
    provides: "PROOF-04 capstone harness (crates/rdpilot-mcp/tests/live_capstone.rs) + secret-free --mcp-config fixture template, offline-authored, PERMISSION_FLAGS constant with documented fallback"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 07
    provides: "Single live Azure VM (rdpilot-vm) held UP; PROOF-02/PROOF-03 both live-verified; the RDPILOT_SENSOR_BINARY_PATH-on-spawned-subprocess pattern established for live_proof.rs (CLI + MCP)"
provides:
  - "PROOF-04 LIVE-VERIFIED: claude -p (local, authenticated Claude Code CLI, no ANTHROPIC_API_KEY) drives rdpilot_connect -> rdpilot_launch -> rdpilot_window_list/foreground -> rdpilot_uia (read/inspect) -> rdpilot_put/rdpilot_get (file-transfer) entirely through the real compiled rdpilot-mcp binary against the live Azure VM's 7-Zip File Manager"
  - "PERMISSION_FLAGS confirmed working as authored: --permission-mode bypassPermissions --strict-mcp-config auto-permits every rdpilot MCP tool call headlessly -- no fallback flag needed"
  - "live_capstone.rs + its fixture template now self-sufficiently set RDPILOT_SENSOR_BINARY_PATH on the rendered --mcp-config, matching the pattern already established in live_cli_verbs.rs and both live_proof.rs harnesses (15-06/15-07)"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Fourth and final confirmation of the recurring live-gate finding: any harness that spawns a real Connect path (CLI subprocess env, MCP TokioChildProcess env, OR a claude -p --mcp-config env block) must set RDPILOT_SENSOR_BINARY_PATH itself to be self-sufficiently re-runnable. This is now proven across all four live-Connect-driving harness shapes in the codebase (live_cli_verbs.rs, rdpilot-cli's live_proof.rs, rdpilot-mcp's live_proof.rs, and now live_capstone.rs's rendered --mcp-config)."

key-files:
  created: []
  modified:
    - crates/rdpilot-mcp/tests/live_capstone.rs
    - crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json

key-decisions:
  - "Live-diagnosed the exact same RDPILOT_SENSOR_BINARY_PATH gap 15-06/15-07 already found and fixed in three sibling harnesses, but never mirrored into live_capstone.rs (authored in 15-04, before 15-07's discovery). Fixed by adding a sensor_binary_path() helper (mirrors live_proof.rs verbatim) and setting it on render_mcp_config's rendered env block, plus documenting the new __RDPILOT_SENSOR_BINARY_PATH__ substitution token in the committed fixture template's _comment block and env object -- consistent with how every other token is documented there."
  - "PERMISSION_FLAGS's authored default (--permission-mode bypassPermissions --strict-mcp-config) worked on the first genuine attempt once the sensor-path fix was applied -- no fallback to --dangerously-skip-permissions was needed. The TODO-for-15-08 comment in live_capstone.rs is now resolved; the constant's doc comment could be updated to note this in a future pass but was left as-is since it already documents the fallback correctly."
  - "STOPPED before the Task 3 teardown checkpoint per explicit orchestrator instruction, pending developer authorization -- the VM remained UP through that pause. Once the developer replied 'approved: teardown', Task 3 was executed: `pwsh infra/manage-env.ps1 -Action down` (the script's default BLOCKING mode) ran to completion, confirmed by both the script's own internal poll ('Resource group '\''rdpilot-test'\'' is fully deleted.', exit 0) and an independent `az group exists -n rdpilot-test` => `false` check. `rdpilot-mgmt` (the persistent management RG) was left untouched and confirmed still `Succeeded`."

requirements-completed: [PROOF-04]

# Metrics
duration: ~45min (Tasks 1-2) + ~4min (Task 3 teardown, developer-authorized in a follow-up turn)
completed: 2026-07-11
---

# Phase 15 Plan 08: PROOF-04 Live-LLM Capstone (claude -p) + Teardown — Live Run Summary

**`claude -p` (local, already-authenticated Claude Code CLI, headless print mode) drove a real read/inspect + file-transfer task through the actual `rdpilot-mcp` server against the live Azure VM's 7-Zip File Manager — connect, launch, window discovery, UIA menu-bar enumeration, and a `put`/`get` round trip — with PROOF-04 asserted PASS by both required signals (transcript tool-call evidence AND an independent second-client byte-for-byte side-effect check), after one live-diagnosed fix identical in shape to three prior sibling-harness fixes this phase. Following explicit developer authorization at the Task 3 blocking-human checkpoint, the Azure test environment was torn down (`manage-env.ps1 down`) and confirmed destroyed, closing the v1.1 milestone's single-VM live session and Phase 15 itself.**

## Performance

- **Duration:** ~45 min (Tasks 1-2) + ~4 min (Task 3, in a developer-authorized follow-up turn)
- **Completed:** 2026-07-11
- **Tasks:** 3 of 3 — Complete
- **Files modified:** 2 (Tasks 1-2 only; Task 3 was an infrastructure-only teardown, no source files)

## Accomplishments

- **Task 1 — claude-auth fail-fast probe:** Confirmed `claude` resolves on `PATH` (`claude --version` → `2.1.207 (Claude Code)`) and a trivial `claude -p "reply OK"` returned `OK` — the local CLI is authenticated and reachable before spending capstone tokens.
- **Task 2 — PROOF-04 capstone live run:** First attempt genuinely FAILED (connect succeeded, then every sensor-backed call — `rdpilot_launch`, `rdpilot_window_list`, `rdpilot_world_state`, `rdpilot_put` — timed out with `DVC transport error`). Live-diagnosed the root cause (identical to 15-06/15-07's finding: the spawned `rdpilot-mcp` subprocess never had `RDPILOT_SENSOR_BINARY_PATH` set, so the real `Connect` path never deployed a sensor). Fixed `render_mcp_config` to set the env var on the rendered `--mcp-config`, rebuilt (clean, zero clippy warnings), re-ran: **`PROOF: PASS`** in 62.09s.

## PROOF-04 Result — PASS

**Exact invocation:**
```
claude -p "<deterministic task prompt>" \
  --mcp-config <rendered temp path> \
  --permission-mode bypassPermissions --strict-mcp-config \
  --output-format stream-json --verbose
```
`PERMISSION_FLAGS` used exactly as authored in 15-04 (`--permission-mode bypassPermissions --strict-mcp-config`) — **no fallback flag (`--dangerously-skip-permissions`) was needed.** Every `rdpilot_*` MCP tool call auto-permitted headlessly on the first attempt after the sensor-path fix.

**Transcript evidence (both required tool-call kinds observed):**
- `mcp__rdpilot__rdpilot_connect` → `{"session":"capstone-04"}`
- `mcp__rdpilot__rdpilot_launch` → `{"pid":7680}` (7-Zip File Manager)
- `mcp__rdpilot__rdpilot_window_list` → found `class_name: "7-Zip::FM"`, `hwnd: 1704844`
- `mcp__rdpilot__rdpilot_foreground` → `ok`
- `mcp__rdpilot__rdpilot_uia` (read/inspect, two calls — `children` then `subtree max_depth:2` after the model noticed the first scope was too shallow) → menu bar enumerated: **File, Edit, View, Favorites, Tools, Help**
- `mcp__rdpilot__rdpilot_put` (file-transfer) → `{"bytes_transferred":60,"checksum":"1dcd191b52e6591e861e901db7ab5cf709f583c908057f7b855979f402aba050"}`
- `mcp__rdpilot__rdpilot_get` (file-transfer round trip) → `{"bytes_transferred":60,"checksum":"1dcd191b52e6591e861e901db7ab5cf709f583c908057f7b855979f402aba050"}` — matches `put`'s checksum

`tool_use_names_from_transcript`'s suffix-matching (`rdpilot_uia`/`computer` for read/inspect, `rdpilot_put`/`rdpilot_get` for file-transfer) confirmed both `[PASS]` in the harness's own D-9.4 trace.

**Independent side-effect verification (never model self-report):** A SECOND, wholly separate `rmcp` client subprocess connected fresh, called `rdpilot_get` for `rdpilot-capstone-04-upload.bin` on session `capstone-04` to a fresh local path, and the harness compared the downloaded bytes byte-for-byte against the test-authored seed content (`rdpilot PROOF-04 capstone payload -- independently verified\n`, 60 bytes): **`side_effect_ok = true`**. `[PASS] independent side-effect: downloaded bytes match the known local seed file`.

**Final harness verdict:**
```
PROOF: PASS
test capstone_llm_drives_read_inspect_and_file_transfer_through_mcp ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 62.09s
```

## Task 3 — Teardown authorization + confirmed destroy

**Developer authorized teardown** ("TEARDOWN AUTHORIZED by the developer... approved") in a follow-up turn after reviewing the PROOF-04 PASS evidence and the phase's full live-gate track record (DAEMON-02 Windows DACL, DAEMON-04/SESSION-01/03/04, CLI-02/03, MCP-04, PROOF-02/03/04 — all PASS).

**Teardown executed:**
```
pwsh -NoProfile -Command "infra/manage-env.ps1 -Action down"
```
Ran in the script's default BLOCKING mode (no `-NoWait`): `az group delete -n rdpilot-test --yes` (server-side blocking delete) followed by the script's own confirmation poll.

**Result:**
```
Deleting TEST resource group 'rdpilot-test' (the management RG 'rdpilot-mgmt' is left in place)...
Waiting for deletion to complete (this can take several minutes)...
Resource group 'rdpilot-test' is fully deleted.
EXIT=0
```

**Independent confirmation (not just the script's own claim):**
- `az group exists -n rdpilot-test` → **`false`**
- `az group exists -n rdpilot-mgmt` → **`true`** (persistent management RG correctly left in place)
- `az group show -n rdpilot-mgmt --query "properties.provisioningState"` → `Succeeded` (healthy, untouched)
- Mid-deletion progress was observed directly: `az resource list -g rdpilot-test` showed the VM/disk gone first, then `rdpilot-pip`/`rdpilot-nsg`/`rdpilot-vnet` draining, then all three individually confirmed `ResourceNotFound` before the RG shell itself finished deleting.

**Local process hygiene, final confirmation:**
- `ps aux | grep -E "rdpilot-daemon|rdpilot-mcp|rdpilot-sensor|claude -p"` → no matches (no stray local daemon/MCP-server/sensor/claude subprocess left running).
- `/run/user/1000/rdpilot/` → no `daemon.sock` present (no stale local IPC socket).

## Task Commits

1. **Task 2 fix — RDPILOT_SENSOR_BINARY_PATH live-diagnosed fix** — `d405805` (`fix(15-08): PROOF-04 capstone harness sets RDPILOT_SENSOR_BINARY_PATH on the rendered --mcp-config`)
2. **Task 3 — teardown** — infrastructure-only (Azure resource deletion via `manage-env.ps1 down`); no source file changes, no code commit. Documented here plus the closing docs commit.

(Task 1 required no code change — a pure verification probe.)

## Files Created/Modified

- `crates/rdpilot-mcp/tests/live_capstone.rs` — added a `sensor_binary_path()` helper (mirrors `tests/live_proof.rs` verbatim) and threaded it into `render_mcp_config`'s rendered `env` block as `RDPILOT_SENSOR_BINARY_PATH`.
- `crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json` — documented the new `__RDPILOT_SENSOR_BINARY_PATH__` substitution token in the `_comment` array and added the corresponding placeholder key to the template's `env` object, matching the existing convention for every other substituted token.

## Decisions Made

- Live-diagnosed and fixed the `RDPILOT_SENSOR_BINARY_PATH` gap in-place (Rule 1 — Bug) rather than treating it as an architectural question: this is the fourth occurrence of an already-established, already-documented pattern (`live_cli_verbs.rs`, both `live_proof.rs` harnesses), not a new design decision.
- Did not need to fall back to `--dangerously-skip-permissions` — the authored `PERMISSION_FLAGS` default worked correctly once the sensor path was fixed, confirming 15-04's flag research was sound.
- Cleaned up local leftover `rdpilot-daemon` processes (spawned detached by the live capstone run, holding the local Unix IPC socket) before running the full offline workspace suite — this is local process hygiene, not VM teardown, and is unrelated to the Task 3 checkpoint.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] PROOF-04 capstone harness's rendered `--mcp-config` was missing `RDPILOT_SENSOR_BINARY_PATH`**
- **Found during:** Task 2, first live run — `claude -p` transcript showed `rdpilot_connect` succeeding but every subsequent sensor-backed call (`rdpilot_launch`, `rdpilot_window_list`, `rdpilot_world_state`, `rdpilot_put`) failing with `DVC transport error: request timed out`; the model itself diagnosed "Every DVC/sensor-backed call is timing out" and retried multiple times (including reconnecting) before giving up. The harness's own `PROOF: FAIL` trace and the independent verification client's `rdpilot_get` (also failing with the identical 30s DVC timeout) confirmed the pattern end-to-end.
- **Root cause:** Identical to 15-06's original production finding and 15-07's harness-side mirror fix for both `live_proof.rs` files: the real `Connect` path only deploys a sensor and starts the RDPDR channel when `RDPILOT_SENSOR_BINARY_PATH` is set on the `rdpilot-mcp` server subprocess's own environment. `live_capstone.rs`'s `render_mcp_config` (authored in 15-04, before 15-07's discovery) never set it in the rendered `--mcp-config`'s `env` block.
- **Fix:** Added `sensor_binary_path()` (mirrors `tests/live_proof.rs`'s identical helper) and set `env["RDPILOT_SENSOR_BINARY_PATH"]` in `render_mcp_config`; documented the new token in the fixture template.
- **Files modified:** `crates/rdpilot-mcp/tests/live_capstone.rs`, `crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json`
- **Verification:** Rebuilt clean (`cargo build -p rdpilot-mcp --tests`, zero warnings), zero clippy warnings, re-ran the live capstone: `PROOF: PASS`, exit 0, 62.09s. Full offline `cargo test --workspace` green afterward (after cleaning up the leftover live-run daemon process holding the local IPC socket — a pre-existing test-isolation artifact of running any live gate, not caused by this fix).
- **Committed in:** `d405805`

---

**Total deviations:** 1 auto-fixed (Rule 1 — Bug)
**Impact on plan:** Necessary for correctness; without it PROOF-04 could never genuinely pass. No scope creep — the fix is scoped exactly to the one file/pattern already established by 15-06/15-07's identical prior fixes.

## Issues Encountered

None beyond the one root-cause finding above (already resolved and re-verified green).

## User Setup Required

None. VM, sensor binary, `.secrets/connection.json`, and local `claude` CLI authentication were all already in place from Plans 15-05/15-06/15-07.

## Next Phase Readiness

- **PROOF-04 is Complete** — the milestone's third and final finish-line deliverable (alongside PROOF-02/PROOF-03, both already live-verified in 15-07) is now genuinely proven against the same live VM. The v1.1 milestone's dual/triple finish line (PROOF-02/03/04) is complete.
- **Task 3 (teardown authorization + confirmed destroy) is Complete** — developer-authorized, executed, and independently confirmed: `az group exists -n rdpilot-test` → `false`, `rdpilot-mgmt` persists (`Succeeded`). No stray local processes or sockets left behind.
- **Plan 15-08 is now fully Complete (3/3 tasks)** — this closes Phase 15 (8/8 plans) and the v1.1 milestone's live-gate/proof-harness track.
- No blockers. The v1.1 milestone's Consumer Surfaces & File Transfer scope (Phases 10-15) is now fully live-verified end to end.

---
*Phase: 15-proof-harnesses-live-llm-capstone*
*Completed: 2026-07-11 (3/3 tasks — capstone PASS + teardown confirmed)*
