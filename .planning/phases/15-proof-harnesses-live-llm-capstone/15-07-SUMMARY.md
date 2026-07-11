---
phase: 15-proof-harnesses-live-llm-capstone
plan: 07
subsystem: proof-harnesses-cli-mcp
tags: [live-gate, azure, proof-02, proof-03, sensor-deployment, rmcp, cli-subprocess]

# Dependency graph
requires:
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 05
    provides: "Single live Azure VM (rdpilot-vm) held UP, byte-verified C# sensor exe at .secrets/sensor-build/rdpilot-sensor.exe, fresh .secrets/connection.json"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 06
    provides: "Daemon's real Connect path fixed to source sensor_binary_path and call deploy_and_launch (1156c9a, 2ae729f)"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 02
    provides: "PROOF-02 CLI end-to-end harness (crates/rdpilot-cli/tests/live_proof.rs), offline-authored"
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 03
    provides: "PROOF-03 MCP end-to-end harness (crates/rdpilot-mcp/tests/live_proof.rs), offline-authored"
provides:
  - "PROOF-02 LIVE-VERIFIED: the real rdpilot CLI + auto-started daemon (fake connector unset) exercised end-to-end -- connect, screenshot, launch 7-Zip::FM, window list, put/get checksum round trip, disconnect, D-28 distinct exit code -- against the live Azure VM"
  - "PROOF-03 LIVE-VERIFIED: the real rdpilot-mcp binary driven programmatically via an rmcp client subprocess (no LLM) -- tools/list, rdpilot_connect, rdpilot_list (D-30 status), rdpilot_world_state, rdpilot_put/rdpilot_get (MCP-05 metadata round trip), rdpilot_disconnect -- against the live Azure VM"
  - "Both PROOF-02 and PROOF-03 harnesses are now self-sufficient (set RDPILOT_SENSOR_BINARY_PATH themselves), matching the pattern already fixed into live_cli_verbs.rs in 15-06"
affects: [15-08]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Every gated live harness that spawns a real Connect against a real target (CLI subprocess OR MCP-client-driven subprocess) MUST set RDPILOT_SENSOR_BINARY_PATH itself to be self-sufficiently re-runnable -- this is now the THIRD file this recurring finding has required fixing (live_cli_verbs.rs in 15-06; live_proof.rs in both rdpilot-cli and rdpilot-mcp here). Any future live harness authored against a real Connect path should set this env var from the start rather than rediscovering the gap."

key-files:
  created: []
  modified:
    - crates/rdpilot-cli/tests/live_proof.rs
    - crates/rdpilot-mcp/tests/live_proof.rs

key-decisions:
  - "Fixed both harnesses to be self-sufficient (set RDPILOT_SENSOR_BINARY_PATH on their own spawned subprocess env) rather than relying on an externally-exported shell variable, even though the latter would have made the tests pass just as well in this one session -- the harnesses' own doc comments declare them 'PERMANENT, re-runnable artifacts', which requires the fix to live in source, not in an ephemeral invocation convention."

requirements-completed: [PROOF-02, PROOF-03]

# Metrics
duration: ~45min (diagnosis was fast -- the exact root cause and fix pattern were already documented in 15-06's SUMMARY)
completed: 2026-07-11
---

# Phase 15 Plan 07: Live-Run — PROOF-02 (CLI) + PROOF-03 (MCP) Scripted Proofs Summary

**Ran the phase's two scripted, no-live-LLM end-to-end proof harnesses against the live Azure VM; both initially failed identically (DVC timeouts / "no framebuffer captured yet") because neither harness set `RDPILOT_SENSOR_BINARY_PATH` on its spawned real-Connect-path subprocess -- the exact daemon-side gap 15-06 fixed in production but whose harness-side companion fix (already applied to `live_cli_verbs.rs`) had not yet been mirrored into either PROOF harness. Fixed both, re-ran clean (twice each, reproducibly), left the VM UP.**

## Results — both PROOF harnesses PASS with measured evidence

### PROOF-02: `RDPILOT_LIVE=1 cargo test -p rdpilot-cli --test live_proof -- --ignored`

First run (before the fix) FAILED at every sensor-backed step:
```
[PASS] connect: connected as session "proof-cli"
[FAIL] perceive screenshot: no framebuffer captured yet (awaiting the first graphics update)
[FAIL] input launch 7zFM.exe: DVC transport error: request timed out after 500ms
[FAIL] perceive window list (7-Zip::FM): never observed a 7-Zip::FM window after polling
[FAIL] put: DVC transport error: request timed out after 30000ms
[FAIL] get (checksum round trip): checksum mismatch or get failed
[PASS] disconnect
[PASS] D-28 distinct exit code
PROOF: FAIL
```

After the fix (`crates/rdpilot-cli/tests/live_proof.rs` now sets `RDPILOT_SENSOR_BINARY_PATH` on the CLI subprocess's env, mirroring `live_cli_verbs.rs`), re-run twice, both genuinely PASS:
```
=== rdpilot PROOF-02 CLI end-to-end proof harness ===
  [PASS] connect: connected as session "proof-cli"
  [PASS] perceive screenshot: wrote 70070 bytes to ".../proof-screenshot.png"
  [PASS] input launch 7zFM.exe: launched 7-Zip File Manager
  [PASS] perceive window list (7-Zip::FM): found a window with class_name "7-Zip::FM"
  [PASS] put: uploaded rdpilot-proof-02.bin, checksum=ff30ea145b85fdf5f66f623dc004b5f905d044415336fb7213a09d7766a3140a
  [PASS] get (checksum round trip): checksum matched put's: ff30ea145b85fdf5f66f623dc004b5f905d044415336fb7213a09d7766a3140a
  [PASS] disconnect: disconnected session "proof-cli"
  [PASS] D-28 distinct exit code (get against an unknown session): exit code Some(2) (expected Some(2), the session-not-found class, never a generic 1); stderr=error: SessionNotFound: no such session "does-not-exist"
PROOF: PASS
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
Exit code 0 both times; the daemon used was the real one (fake connector unset throughout -- `RDPILOT_DAEMON_TEST_CONNECTOR` was never set in this session).

### PROOF-03: `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_proof -- --ignored`

Same failure signature before the fix (`rdpilot_world_state`/`rdpilot_put`/`rdpilot_get` all failed with the identical DVC-timeout / no-framebuffer errors; `tools/list`, `rdpilot_connect`, `rdpilot_list`, `rdpilot_disconnect` passed since they never touch the sensor).

After the fix (`crates/rdpilot-mcp/tests/live_proof.rs` now sets `RDPILOT_SENSOR_BINARY_PATH` directly on the `Command` handed to `TokioChildProcess::new` before spawning the real `rdpilot-mcp` binary), re-run twice, both genuinely PASS:
```
=== rdpilot PROOF-03 MCP end-to-end proof harness ===
  [PASS] tools/list discovery: 12 tools advertised, all expected rdpilot_* tools present
  [PASS] rdpilot_connect: connected as session "proof-mcp"
  [PASS] rdpilot_list (D-30 status): session "proof-mcp" present, status=Some(String("Live"))
  [PASS] rdpilot_world_state (perception read): received 95670 bytes of world-state JSON (nonempty screenshot field present=true)
  [PASS] rdpilot_put: uploaded rdpilot-proof-03.bin, bytes_transferred=25, checksum=76d0a83d4907cab0f6279977dd9debddb3febe06bf746132d6bc3935628c282e
  [PASS] rdpilot_get (MCP-05 metadata round trip): checksum and bytes_transferred matched put's: checksum=76d0a83d4907cab0f6279977dd9debddb3febe06bf746132d6bc3935628c282e bytes_transferred=25
  [PASS] rdpilot_disconnect: disconnected session "proof-mcp"
PROOF: PASS
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
Every `CallToolResult.is_error` was `false`; every session-scoped call carried an explicit `session` argument; the MCP-05 metadata match compared both `checksum` AND `bytes_transferred`, never inline file bytes.

## Live-diagnosed fix (the root cause, one finding, two files)

**[Rule 1 — Bug] Neither PROOF-02 nor PROOF-03 set `RDPILOT_SENSOR_BINARY_PATH` on their spawned real-Connect-path subprocess.**
- **Found during:** Task 1's first run — `perceive screenshot` failed with "no framebuffer captured yet", `input launch` and `put` failed with "DVC transport error: request timed out".
- **Root cause:** identical to 15-06's two production bug fixes (`sensor_binary_path` wiring + `deploy_and_launch` call) — the daemon/MCP server's real `Connect` path only deploys a sensor and starts the RDPDR channel when `RDPILOT_SENSOR_BINARY_PATH` is set. 15-06 fixed the daemon production code AND fixed the harness-side companion in `live_cli_verbs.rs` (commit `a84e6af`), but that companion fix was never mirrored into `crates/rdpilot-cli/tests/live_proof.rs` (PROOF-02, authored earlier in 15-02) or `crates/rdpilot-mcp/tests/live_proof.rs` (PROOF-03, authored in 15-03) — both predate 15-06's discovery.
- **Confirmation before fixing:** ran PROOF-02 with `RDPILOT_SENSOR_BINARY_PATH` exported externally in the shell (not committed) — every step passed, confirming the root cause before touching source.
- **Fix (PROOF-02):** `crates/rdpilot-cli/tests/live_proof.rs`'s `run_cli` now sets `.env("RDPILOT_SENSOR_BINARY_PATH", sensor_binary_path())` on the spawned `rdpilot` CLI subprocess, with a `sensor_binary_path()` helper resolving `.secrets/sensor-build/rdpilot-sensor.exe` the same two-levels-up way as `connection_file()`.
- **Fix (PROOF-03):** `crates/rdpilot-mcp/tests/live_proof.rs` now builds the `Command` for the `rdpilot-mcp` subprocess explicitly (rather than inline in `TokioChildProcess::new(Command::new(&mcp_bin))`) and calls `.env("RDPILOT_SENSOR_BINARY_PATH", sensor_binary_path())` on it before spawning.
- **Verification:** both harnesses re-run twice, back-to-back, with NO externally-exported `RDPILOT_SENSOR_BINARY_PATH` in the shell — both genuinely PASS both times, confirming the fix is self-sufficient (the harness itself, not an invocation convention, now supplies the path). Full offline workspace suite (`cargo test --workspace --target x86_64-unknown-linux-gnu`) still exits 0 after both fixes — unaffected, since these are `#[ignore]`-gated live-only tests.
- **Committed:** `1e1912d` (PROOF-02), `e7774c2` (PROOF-03).

## Package Legitimacy

No new crate/package added or version-changed this plan.

## Task Commits

1. `1e1912d` — `fix(15-07): PROOF-02 harness self-sufficiently sets RDPILOT_SENSOR_BINARY_PATH`
2. `e7774c2` — `fix(15-07): PROOF-03 harness sets RDPILOT_SENSOR_BINARY_PATH on the spawned rdpilot-mcp subprocess`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] PROOF-02/PROOF-03 harnesses missing `RDPILOT_SENSOR_BINARY_PATH`** — see "Live-diagnosed fix" above. One root cause, two files, two commits (each independently verifiable/revertible).

No Rule 2/3/4 deviations. No architectural changes. No package-legitimacy checkpoints triggered (T-15-SC's `accept` disposition held — no new packages).

## Threat Model Compliance

- **T-15-16 (password disclosure in traces):** verified — no `println!`/`format!`/`panic!` in either harness formats the `LiveTarget` struct (no `Debug`/`Display` impl exists on it); all captured stdout/stderr/trace output reviewed above contains no password.
- **T-15-17 (masked regression):** verified — PROOF-02's D-28 assertion checks the exact exit code (`Some(2)`, never a generic `1`); PROOF-03's `call()` helper treats both a JSON-RPC `Err` AND `is_error: Some(true)` as failure, and the MCP-05 step asserts BOTH `checksum` and `bytes_transferred` match, not prose.

## Issues Encountered

None beyond the one root-cause finding above (already resolved). The VM's zombie `7zFM.exe` processes noted in 15-06 as a non-blocking issue were cleaned up (`taskkill /IM 7zFM.exe /F` via `az vm run-command invoke`) before running these proofs, to avoid a stale-window false match; none of PROOF-02/03's own assertions depend on window uniqueness (PROOF-02 polls for ANY `7-Zip::FM` class match, tolerant per 15-06's noted caveat).

## User Setup Required

None. VM, sensor binary, and Azure CLI auth were all already in place from Plans 15-05/15-06.

## Next Phase Readiness

- **PROOF-02 and PROOF-03 are both Complete** — two of the milestone's three finish-line deliverables now genuinely proven against the same live VM.
- **VM confirmed UP** (`az vm get-instance-view` → `VM running`) and left running for Plan 15-08 (the live-LLM capstone + authorized teardown). Not torn down here, per binding constraint.
- The `RDPILOT_SENSOR_BINARY_PATH`-self-sufficiency pattern is now consistent across all three live-Connect-driving harnesses (`live_cli_verbs.rs`, `live_proof.rs` ×2) — any future live harness (e.g. 15-08's capstone, which reuses this file's `TokioChildProcess` pattern per its own plan doc) should follow the same convention from the start.
- No blockers for 15-08.

---
*Phase: 15-proof-harnesses-live-llm-capstone*
*Completed: 2026-07-11*

## Self-Check: PASSED

Both claimed files exist on disk with the described changes (`crates/rdpilot-cli/tests/live_proof.rs`, `crates/rdpilot-mcp/tests/live_proof.rs`), and both task commit hashes (`1e1912d`, `e7774c2`) are present in `git log --oneline --all`.
