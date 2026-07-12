---
phase: 15-proof-harnesses-live-llm-capstone
verified: 2026-07-11T23:30:00Z
status: passed
score: 3/3 must-haves verified (plus 12 batched live-gate requirements, all confirmed closed)
overrides_applied: 0
---

# Phase 15: Proof Harnesses & Live-LLM Capstone — Final Milestone Verification Report

**Phase Goal:** Both consumer surfaces are proven end-to-end by scripted harnesses, and a live LLM drives a real read/inspect + file-transfer task through MCP.
**Verified:** 2026-07-11 (post-teardown; VM `rdpilot-vm` / RG `rdpilot-test` confirmed destroyed by 15-08)
**Status:** VERIFIED — Phase 15 delivers its goal; the v1.1 milestone (Phases 10-15) is fully live-verified and ready to close.
**Method:** Goal-backward verification against delivered code, git history, SUMMARY evidence, and REQUIREMENTS.md/ROADMAP.md state. The live Azure VM is torn down; live re-execution was not possible and was not attempted — all live claims below are cross-checked against code that is still in HEAD (the two production-bug fixes, the DACL module, the harness files) rather than trusted from SUMMARY prose alone.

## Goal Achievement — Core Phase 15 Criteria

| # | Truth (PROOF requirement) | Status | Evidence |
|---|------|--------|----------|
| 1 | PROOF-02: scripted CLI harness proves the CLI surface end-to-end against a real remote-only Windows program, no live LLM | VERIFIED | `crates/rdpilot-cli/tests/live_proof.rs` (21,256 bytes) exists, `#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM..."]`-gated, targets `class_name "7-Zip::FM"` (7-Zip File Manager, continuity with the Phase 9 proof), drives connect→screenshot→launch→window-list→put→get(checksum)→disconnect→D-28 distinct-exit-code. `15-07-SUMMARY.md` records two clean back-to-back re-runs with measured output (`checksum=ff30ea14...`, exit code `Some(2)` for the unknown-session case), not a single lucky pass. REQUIREMENTS.md: `[x] PROOF-02`. |
| 2 | PROOF-03: scripted MCP harness (rmcp client + transport-child-process) drives rdpilot-mcp programmatically, no LLM | VERIFIED | `crates/rdpilot-mcp/tests/live_proof.rs` (18,605 bytes), `#[ignore]`+`RDPILOT_LIVE`-gated, uses `rmcp::transport::TokioChildProcess` + `ServiceExt` to spawn the REAL compiled `rdpilot-mcp` binary as an rmcp client subprocess (same `rmcp` crate as the production server — wire-compatible by construction). `Cargo.toml` confirms `rmcp` client+`transport-child-process` features are `[dev-dependencies]` only (never merged into `[dependencies]`). `15-07-SUMMARY.md` records two clean re-runs (`12 tools advertised`, checksum+bytes_transferred match on both `put`/`get`). REQUIREMENTS.md: `[x] PROOF-03`. |
| 3 | PROOF-04: capstone live-LLM (claude -p) drove read/inspect + file-transfer through MCP, verified by transcript + independent side-effect | VERIFIED | `crates/rdpilot-mcp/tests/live_capstone.rs` (32,654 bytes), `#[ignore]`+`RDPILOT_LIVE`-gated. `15-08-SUMMARY.md` records a genuine PASS: transcript shows real `rdpilot_connect`/`rdpilot_launch`/`rdpilot_uia`(read/inspect)/`rdpilot_put`/`rdpilot_get` tool-use blocks against 7-Zip File Manager (menu bar enumerated: File, Edit, View, Favorites, Tools, Help), AND a SECOND, independent `rmcp` client subprocess downloaded the uploaded file fresh and matched it byte-for-byte against the known local seed (`side_effect_ok = true`) — never model self-report. `PROOF: PASS`, 62.09s, exit 0. REQUIREMENTS.md: `[x] PROOF-04`. |

**Score:** 3/3 truths verified.

## Batched Live-Gate Closure (this phase closed all remaining v1.1 live gates)

| Requirement | Status | Evidence |
|---|---|---|
| DAEMON-02 (Unix half) | VERIFIED (pre-existing, Phase 12) | `authorize_uid` peer-uid check, `0700` runtime dir — unaffected by this phase. |
| DAEMON-02 (Windows DACL half) | VERIFIED | `crates/rdpilot-daemon/src/ipc/windows.rs` (19,762 bytes) present: `OwnerOnlyDacl::build()`, raw Win32 chain (`OpenProcessToken`→`GetTokenInformation`→`InitializeAcl`→`AddAccessAllowedAce`→`SetSecurityDescriptorDacl`), `first_pipe_instance` anti-squatting, `ERROR_ACCESS_DENIED`→`AddrInUse` mapping. `15-05-SUMMARY.md`: first real Windows compile surfaced 2 genuine windows-sys 0.61.2 API bugs (`OpenProcessToken`/`SECURITY_DESCRIPTOR_REVISION` module paths, `HANDLE` type), both fixed (commit `534bec4`). Cross-account rejection independently re-verified (not trusted on first "PASS" — a `runas`-based false-positive was live-diagnosed and replaced with a Scheduled-Task probe, manually confirmed via `whoami` before trusting the Rust test result). All 3 DACL tests genuinely PASS. REQUIREMENTS.md: `[x] DAEMON-02` (both halves). |
| DAEMON-04 (live orphan-liveness) | VERIFIED | `15-06-SUMMARY.md`: `kill -9` mid-session + restart against the real VM surfaces the session as `Orphaned` (never forgotten/auto-killed); explicit disconnect reconciles it. REQUIREMENTS.md: `[x]`. |
| SESSION-01/03/04 (e2e) | VERIFIED | `15-06-SUMMARY.md`: explicit-name connect, `list` (correct id/name/host/status), disconnect all pass against the real VM. REQUIREMENTS.md: `[x]` all three. |
| CLI-02/03 (live) | VERIFIED | `15-06-SUMMARY.md`: real before/after screenshot pixel diff (PNG-magic confirmed), real click-landing (UIA-verified focus), real UIA tree shape, real 8 MiB put/get checksum round trip. REQUIREMENTS.md: `[x]` both. |
| MCP-04 (live half) | VERIFIED | `15-06-SUMMARY.md`: real `computer` `left_click` near advertised-space corners lands on intended elements (File menu focus, Minimize state transition), verified via UIA/window-state, never via pre-click coordinate computation. REQUIREMENTS.md: `[x]`. |

## The Two Production Bugs (verified in HEAD, not just claimed)

The task explicitly calls these out as the most important integration fixes of the milestone. Both were read directly from current source, not taken from SUMMARY prose.

**Bug 1 — `sensor_binary_path` never sourced on the real Connect path.**
`crates/rdpilot-daemon/src/dispatch.rs:90-98` (current HEAD): the `Connect` handler calls `resolve_sensor_binary_path()` and, when configured, calls `cfg.sensor_binary_path(...)` and records `sensor_configured = true`. `crates/rdpilot-config/src/resolved.rs` defines `ResolvedConfig::sensor_binary_path` (mirrors `share_root`, `RDPILOT_SENSOR_BINARY_PATH` env, never a wire field). Confirmed present via `git show 1156c9a` and a direct grep of `dispatch.rs` at HEAD (lines 90, 232-234, 254).

**Bug 2 — nothing ever called `Session::deploy_and_launch`.**
`crates/rdpilot-daemon/src/dispatch.rs:114-118` (current HEAD): `if sensor_configured { registry.call(&session, |s| s.deploy_and_launch()).await ... }`, closing the session and surfacing a Connect failure if deployment fails (never a silent partial success). `ManagedSession::deploy_and_launch` was added to the trait (`seams.rs`) and to all 9 fake implementors (`lifecycle.rs`, `registry.rs`, `server.rs`'s `FakeTestSession`, `dispatch.rs`'s 3 inline fakes, 3 `tests/*.rs` integration fakes) — confirmed via `git show 2ae729f --stat` (8 files changed) and a direct grep confirming 3 `deploy_and_launch` implementations remain in `dispatch.rs` at HEAD (test fakes) plus the real dispatch call.

Both fixes are load-bearing: without them, every sensor-backed verb (launch/perceive/put/get) against a real target times out — this is exactly what CLI-02/03, MCP-04, and PROOF-02/03/04 all depend on, and why 15-06's discovery unblocked every subsequent live plan (15-07, 15-08) in this phase.

## 12-07 Supersession — Confirmed, No Scope Dropped

`.planning/phases/12-session-daemon/12-07-PLAN.md`'s `must_haves.truths` requires exactly: (1) owner-only DACL never null/default, (2) `first_pipe_instance(true)` fails loudly against a squatted pipe, (3) a genuinely different Windows account rejected at the pipe boundary, (4) kill-9 + restart surfaces the session as `Orphaned` and reconciles it, (5) connect/list/disconnect e2e against a real target.

All five are independently confirmed above, closed by 15-01 (authored the DACL code + split live test files, explicitly noting "closing 12-07's Task 2 scope") and live-verified by 15-05 (DACL half) / 15-06 (orphan-liveness + e2e half). `12-07-PLAN.md` remains the one un-executed plan (34 plans total, 33 SUMMARYs) BY DESIGN — this is documented supersession, not an incomplete plan. ROADMAP.md's Phase 12 entry (line 98) explicitly states: "Superseded by Phase 15's file split (15-RESEARCH.md)."

## Milestone-Level Requirements Coverage (v1.1, Phases 10-15)

All 27 v1.1 requirements read directly from `.planning/REQUIREMENTS.md`:

| Requirement | Status |
|---|---|
| FILE-01 | [x] Complete |
| FILE-02 | [x] Complete |
| FILE-03 | [x] Complete |
| FILE-04 | [x] Complete |
| SESSION-01 | [x] Complete |
| SESSION-02 | [x] Complete |
| SESSION-03 | [x] Complete |
| SESSION-04 | [x] Complete |
| CONFIG-01 | [x] Complete |
| CONFIG-02 | [x] Complete |
| CONFIG-03 | [x] Complete |
| DAEMON-01 | [ ] **NOT marked [x]** — see note below |
| DAEMON-02 | [x] Complete |
| DAEMON-03 | [x] Complete |
| DAEMON-04 | [x] Complete |
| CLI-01 | [x] Complete |
| CLI-02 | [x] Complete |
| CLI-03 | [x] Complete |
| MCP-01 | [x] Complete |
| MCP-02 | [x] Complete |
| MCP-03 | [x] Complete |
| MCP-04 | [x] Complete |
| MCP-05 | [x] Complete |
| MCP-06 | [x] Complete |
| PROOF-02 | [x] Complete |
| PROOF-03 | [x] Complete |
| PROOF-04 | [x] Complete |

**DAEMON-01 is the one requirement NOT checked `[x]` in REQUIREMENTS.md's checklist section.** However, the Traceability table (bottom of REQUIREMENTS.md) clarifies: DAEMON-01's core substance — "a long-lived daemon holds N live RDP sessions with keepalive, decoupled from any CLI process lifetime" — decomposes into (a) leak-free registry holding (proven: SC#3 BLOCKING thread+RSS soak across N=50 real connect/disconnect cycles, offline, Phase 12), and (b) the daemon *process* auto-start/idle-shutdown lifecycle, which is DAEMON-03's scope and IS marked `[x] Complete`. The checkbox appears to be residual drift from before 12-04/12-06 landed (the registry-level and process-level halves), rather than an actual functional gap — no phase 13-15 test exercises a distinct "DAEMON-01-only" behavior that isn't already covered by the registry soak test (12-03) and the auto-start/idle-reap test (12-06, `autostart_lifecycle.rs`). This is flagged here for the developer's attention as a REQUIREMENTS.md bookkeeping item, not a functional blocker — Phase 15's own scope (PROOF-02/03/04) does not depend on it, and DAEMON-01's underlying mechanics are independently offline-proven.

**Coverage: 26/27 checked `[x]`, 1 (DAEMON-01) unchecked but functionally covered by other checked requirements — this is bookkeeping drift, not a missing capability.**

## Offline Test Suite (Full Workspace, Native-Linux Substitute)

The repo's pinned toolchain (`rust-toolchain.toml`) targets `stable-x86_64-pc-windows-gnu`, which is not installed in this environment (confirmed via `rustup toolchain list` → only `stable-x86_64-unknown-linux-gnu` present). Per the verification task's standing instruction, ran against the native-Linux substitute:

```
RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --workspace --target x86_64-unknown-linux-gnu
```

**Result: 467 passed, 0 failed, 40 ignored** (aggregated across all 30 test-binary result lines in the full run — every `test result: ok.` line, zero `FAILED` occurrences anywhere in the log). The 40 ignored tests are all `#[ignore]`+`RDPILOT_LIVE`-gated live-only tests (correctly skipped offline, including the three Phase 15 harnesses themselves and `live_daemon_windows_dacl.rs`, which additionally compiles to 0 tests on Linux via `#![cfg(windows)]`). No regressions.

## Thin-Client Invariant (D-17)

```
cargo tree -p rdpilot-cli --target x86_64-unknown-linux-gnu -e normal | grep -iE "ironrdp|rustls|rdpilot-daemon|rdpilot "
cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu -e normal | grep -iE "ironrdp|rustls|rdpilot-daemon|rdpilot "
```

Both produced zero matches (confirmed the tree command itself works normally by inspecting its unfiltered output — `rdpilot-cli`'s tree correctly lists `clap`, `rdpilot-config`, `rdpilot-ipc`, etc.). Neither `rdpilot-cli` nor `rdpilot-mcp`'s shipped (non-dev) dependency graph pulls in `ironrdp`, `rustls`, `rdpilot`, or `rdpilot-daemon`. Thin-client invariant holds.

## Known Cosmetic Debt (noted, not fixed — for the cleanup pass)

| Item | Classification | Confirmation |
|---|---|---|
| STATE.md narrative-field drift | Cosmetic, informational only | `STATE.md`'s frontmatter reports `total_phases: 6, completed_phases: 5` — reflecting Phase 12's 6/7-plan count (12-07 superseded/never executed) rather than its functional completion. `REQUIREMENTS.md` (the authoritative source) shows every Phase-12-mapped requirement `[x] Complete`. Does not affect correctness — a bookkeeping lag, not a functional gap. |
| CLI-02 traceability prose staleness | Cosmetic | `REQUIREMENTS.md`'s CLI-02 traceability row is itself fully current (includes 15-06's live-verification detail); any staleness is confined to intermediate SUMMARY/PLAN prose in Phase 13, which is historical record, not a live artifact consumers read. Does not affect correctness. |
| `FakeTestSession` embedded-newline typo | Cosmetic, test-only fake data | Confirmed at `crates/rdpilot-daemon/src/server.rs:286-287`: `path: r"C:\Windows\notepad.exe".to_owned()` has an actual embedded newline character between `C:\Windows` and `notepad.exe` (a raw string literal spanning two source lines instead of one continuous path). This is canned fake data returned only by the offline `FakeTestSession` connector (never real production data), compiles and runs fine (Rust raw strings permit embedded newlines), and does not affect any test assertion's pass/fail outcome. Purely cosmetic. |
| test-only `#[allow(clippy::expect_used)]` | Cosmetic, justified | All occurrences (`rdpilot-ipc/src/transport.rs`, `rdpilot-daemon/src/server.rs`, `rdpilot-daemon/src/registry.rs` ×9, `rdpilot-daemon/src/lifecycle.rs`, `rdpilot-mcp/src/computer/dispatch.rs`) carry an inline justification comment (test-only fail-fast assertions, or "a poisoned registry mutex is unrecoverable"). Each crate's `#![deny(clippy::expect_used)]` remains in force outside these explicitly-scoped allows. Does not affect correctness. |

None of these affect the milestone verdict.

### Anti-Patterns Found

No `TBD`/`FIXME`/`XXX` debt markers found in any Phase 15-touched file (`live_proof.rs` ×2, `live_capstone.rs`, `dispatch.rs`, `ipc/windows.rs`, `server.rs`). No unreferenced debt markers — the debt-marker gate (Step 7) has nothing to flag.

### Human Verification Required

None. All Phase 15 must-haves are independently confirmed against committed code, git history, and the SUMMARY-recorded measured live-run evidence (checksums, transcript tool-call names, exit codes) rather than narrative claims alone. No visual/UX/real-time judgment call remains open — the VM is torn down and cannot be re-probed, but the code-level evidence (harness gating, production-bug fixes in HEAD, DACL module) is independently verifiable without the VM.

## Overall Verdict

**VERIFIED.** Phase 15 delivers its stated goal: both consumer surfaces (CLI, MCP) are proven end-to-end by scripted, no-live-LLM harnesses (PROOF-02, PROOF-03), and a live LLM (`claude -p`) genuinely drove a read/inspect + file-transfer task through the MCP surface against a real remote-only Windows program, verified by both transcript evidence and an independent side-effect check (PROOF-04). The batched live gates this phase closed (DAEMON-02 Windows half, DAEMON-04, SESSION-01/03/04, CLI-02/03, MCP-04) are all confirmed `[x]` in REQUIREMENTS.md with measured live evidence in their respective SUMMARYs. The two production bugs the live gate caught (`sensor_binary_path` wiring, `deploy_and_launch` call) are confirmed present in the current daemon source, not just claimed. 12-07's full must_haves scope was fulfilled by Phase 15 without being independently re-executed — a genuine, traceable supersession, not a dropped plan.

**The v1.1 milestone (Phases 10-15) is fully live-verified and ready to close**, with one minor bookkeeping item flagged for the developer: DAEMON-01's checkbox in REQUIREMENTS.md remains unchecked despite its underlying mechanics (registry soak test, auto-start/idle-reap) being independently proven — recommend closing this as part of the cleanup pass alongside the STATE.md narrative drift.

---
*Verified: 2026-07-11*
*Verifier: Claude (gsd-verifier)*
