---
phase: 14-mcp-server-surface
verified: 2026-07-11T10:49:58Z
status: passed
score: 12/12 must-haves verified (offline scope)
overrides_applied: 0
deferred:
  - truth: "Real screenshot pixel content is correct through the `computer` screenshot action"
    addressed_in: "Phase 15"
    evidence: "ROADMAP.md Phase 14 note: 'Offline this phase; batched to Phase 15 live gate: real screenshot pixel content, real click landing near edges/corners (the live half of MCP-04...), and the live-LLM capstone (PROOF-04).'"
  - truth: "Real click landing precision near edges/corners against a live remote desktop (live half of MCP-04)"
    addressed_in: "Phase 15"
    evidence: "ROADMAP.md Phase 14 note (same as above); Phase 15 success criteria: 'A scripted harness proves the MCP surface end-to-end — tool calls exercised programmatically.'"
  - truth: "A live LLM drives a real read/inspect + file-transfer task through the MCP surface (PROOF-04 capstone)"
    addressed_in: "Phase 15"
    evidence: "Phase 15 goal: 'a live LLM drives a real read/inspect + file-transfer task through MCP — the milestone's dual finish line'; Phase 15 success criteria #3 (PROOF-04)."
---

# Phase 14: MCP Server Surface Verification Report

**Phase Goal:** An `rmcp`-based MCP server exposes rdpilot to any MCP client through a computer-use-compatible mega-tool plus rdpilot-native tools — without coordinate drift or event-loop stalls.
**Verified:** 2026-07-11T10:49:58Z
**Status:** passed (offline scope; live items explicitly deferred to Phase 15 by developer decision — see Deferred Items)
**Re-verification:** No — initial verification
**Scope note:** Per the developer's explicit instruction, live-Windows/live-LLM assertions are batched to Phase 15. This report verifies the offline-provable deliverables (Plans 14-01 through 14-05) against real code and real test runs, and lists (without attempting to verify) the deferred live items.
**Toolchain substitution:** All commands run via `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo ... --target x86_64-unknown-linux-gnu` — the repo-pinned `x86_64-pc-windows-gnu` toolchain is not installed on this verification host. This is the standing, previously-established substitution (Phases 06-13), safe because none of the phase-14 code has `cfg(windows)` branches.

## Goal Achievement

### Observable Truths / Criteria

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | MCP-01/02 — `computer` mega-tool (computer_20250124 schema) + tool registration; 12 tools, session-scoping split correct; action→ipc-verb dispatch | ✓ VERIFIED | `cargo test -p rdpilot-mcp --test tool_schema --target x86_64-unknown-linux-gnu` → 62/62 passed. Key tests read in full: `tools_list_advertises_exactly_computer_plus_the_eleven_native_tools` (asserts exact 12-name set), `every_session_scoped_tool_requires_session_in_its_input_schema` (asserts 10 tools require `session`, recursing through `allOf`-nested schemas), `the_two_structurally_session_less_tools_declare_no_session_property_at_all` (asserts `rdpilot_connect`/`rdpilot_list` declare NO `session` property at all, not merely non-required). Read `crates/rdpilot-mcp/src/computer/dispatch.rs::dispatch_computer` in full: an exhaustive `match` over every `ComputerAction` variant maps each to a `Request::Mouse`/`Request::Key`/`Request::Screenshot`/`Request::DesktopSize` round trip via `round_trip_bounded`, confirming the action→ipc-verb mapping is real code, not a stub. `handler.rs` confirms the SAME combined router (`Self::computer_tool_router() + Self::native_tool_router()`) is used both by the test's `all_tools()` helper and by the real `#[tool_handler]`-generated `ServerHandler` that `main.rs` serves via `.serve(stdio())` — no test/production divergence. |
| 2 | MCP-03 — 11 `rdpilot_*` native tools registered | ✓ VERIFIED | Same `tool_schema` run: the 12-tool set includes exactly `rdpilot_world_state`, `rdpilot_uia`, `rdpilot_window_list`, `rdpilot_process_list`, `rdpilot_launch`, `rdpilot_foreground`, `rdpilot_connect`, `rdpilot_list`, `rdpilot_disconnect`, `rdpilot_put`, `rdpilot_get` (11) plus `computer`. `crates/rdpilot-mcp/src/native_tools.rs` defines a second `#[tool_router]` block with all 11 `#[tool]` methods, each a thin adapter over one `rdpilot-ipc` wire verb. |
| 3 | [BLOCKING] MCP-04 — `scale_to_native` pure function, all required vectors including the corrected tie | ✓ VERIFIED | `cargo test -p rdpilot-mcp --test scale_to_native --target x86_64-unknown-linux-gnu` → 14/14 passed: `zero_corner_maps_to_zero_corner`, `near_corner_exact_tie_rounds_half_away_from_zero` (`scale_to_native(1279,799,1920,1080) == (1919,1079)` — asserted exactly, confirmed by direct source read), `bottom_left_near_corner`, `top_right_near_corner_shares_the_exact_tie`, `center_sanity`, `advertised_exclusive_edge_clamps_strictly_inside_native_bounds`, `exclusive_edge_clamps_across_multiple_native_resolutions` (spread of 4 native resolutions), `square_scale_native_equals_advertised_is_identity_for_interior_points`. Read `crates/rdpilot-mcp/src/computer/scale.rs` in full: `f64::round()` (round-half-away-from-zero, NOT banker's rounding) then `.clamp(0.0, native_dim-1.0)` as the LAST step before the `u16` cast — matches the required rounding/clamp discipline exactly. Advertised space is the hardcoded `ADVERTISED_WIDTH=1280`/`ADVERTISED_HEIGHT=800` constants; native dims are sourced via the real `Request::DesktopSize` wire verb in `dispatch.rs::native_dims` (never PNG-sniffed), confirmed by source read and by `Request::DesktopSize` living in `rdpilot-ipc/src/request.rs` as a genuine session-scoped wire verb (see Truth 6 below). |
| 4 | MCP-05 — put/get return metadata-only `{path, bytes_transferred, checksum}`, type-level guarantee + planted-bytes regression test | ✓ VERIFIED | `TransferOutcome` (`crates/rdpilot-ipc/src/transfer.rs`) has exactly two fields, `bytes_transferred: u64` and `checksum: String` — no byte-buffer field exists on the type at all (structural/type-level guarantee, not app-level stripping). `cargo test -p rdpilot-mcp --test tool_schema` includes `native_tools::tests::put_get_transfer_result_never_carries_the_planted_file_bytes_sentinel` — PASSED: plants a real file containing sentinel `RDPILOT-PLANTED-FILE-BYTES-SENTINEL` on disk, calls `render_transfer`, asserts the sentinel string never appears in the serialized tool result while `bytes_transferred`/`checksum` do. `render_transfer` (read in full) never opens/reads `path` as file content — `path` is only echoed back as addressing metadata. |
| 5 | [BLOCKING] MCP-06 — slow tool call does not block a fast one; genuine overlap against the real daemon; env-unset regression holds; slowness hook strictly test-connector-scoped | ✓ VERIFIED | `cargo test -p rdpilot-mcp --test non_blocking --target x86_64-unknown-linux-gnu` → 59/59 passed (2.06s wall time, consistent with a genuine 2000ms sleep in the critical path). Read `crates/rdpilot-mcp/tests/non_blocking.rs` in full: `slow_tool_call_does_not_block_a_concurrent_fast_tool_call_mcp06` pre-starts the REAL compiled `rdpilot-daemon` binary (via `CARGO_BIN_EXE_rdpilot-mcp`'s sibling path), sets `RDPILOT_DAEMON_TEST_SLOW_MS=2000` (a real `tokio::time::sleep` inside the fake connector's `upload_file`), spawns a slow `rdpilot_put` task, sleeps 100ms, asserts the slow task is `!is_finished()` (genuine in-flight sanity check), then issues a concurrent `rdpilot_list` and asserts it returns in <500ms AND the slow task is STILL `!is_finished()` at that exact instant — a real ordering/overlap proof, not merely "both eventually completed quickly". Finally awaits the slow task and asserts it took ≥2000ms (proving the delay was real, not raced past). Slowness hook scoping confirmed by source read of `crates/rdpilot-daemon/src/server.rs::run_inner`: the `slow_ms`-carrying `FakeTestConnector` is selected ONLY when `TEST_CONNECTOR_ENV` is set; the production `Arc::new(RealConnector)` branch has no `slow_ms` field or sleep logic at all — untouched. Env-unset regression: `fake_session_slow_class_methods_resolve_immediately_when_slow_ms_is_zero` (in `server.rs`'s own test module) asserts all three slow-class methods resolve in <50ms when `slow_ms: 0` (the default whenever the env var is absent), confirmed present and passing in the full workspace run. |
| 6 | The 3 computer-use gap rejections (cursor_position, left_mouse_down/up, horizontal scroll) are EXPLICIT tool errors, tested by name | ✓ VERIFIED | `computer::dispatch::tests`: `left_mouse_down_is_rejected_by_name`, `left_mouse_up_is_rejected_by_name`, `cursor_position_is_rejected_by_name`, `horizontal_scroll_directions_are_rejected_by_name` — all 4 PASSED (both `tool_schema` and `non_blocking` runs). Each asserts the error message names the specific action (`left_mouse_down`, `left_mouse_up`, `cursor_position`, `scroll`) and, for the mouse-down/up cases, names the supported alternative (`left_click_drag`). Source read of `dispatch.rs` confirms these are `Err(McpError::invalid_argument(...))` returns in the match arms, never a silent no-op, and never reach the daemon (no round trip issued). |
| 7 | SESSION-02 preservation: `DesktopSize` (14-01) is session-scoped, exhaustive `SessionScoped` match, missing-session rejection | ✓ VERIFIED | `crates/rdpilot-ipc/src/request.rs` read in full: `Request::DesktopSize { session: SessionId }` is a real variant; `impl SessionScoped for Request` exhaustively matches every variant including `Request::DesktopSize { session } => Some(session)` (a compile-time forcing function — an unmatched variant is a build error). `cargo test -p rdpilot-ipc --target x86_64-unknown-linux-gnu request::` → 7/7 passed, including `every_operational_request_verb_rejects_a_missing_session_field` whose test-case list includes `r#"{"op":"DesktopSize"}"#` and asserts deserialization is a hard `Err`. |
| 8 | THIN-CLIENT: `cargo tree -p rdpilot-mcp` shows NO ironrdp/rustls/rdpilot/rdpilot-daemon/interprocess | ✓ VERIFIED | `cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu \| grep -E 'ironrdp\|rustls\|^rdpilot v\|rdpilot-daemon\|interprocess\| rdpilot v'` — zero matches (grep exit 1). Full tree inspected: only `rdpilot-config`, `rdpilot-ipc`, `rmcp`, `schemars`, `tokio`, `serde`/`serde_json`, `tracing`/`tracing-subscriber` and their transitive deps. `interprocess` absent from `Cargo.toml`. |
| 9 | NON-REGRESSION: daemon's thread-leak-soak, registry-concurrency, autostart_lifecycle still pass after 14-01's DesktopSize seam + 14-05's slowness hook | ✓ VERIFIED | `cargo test -p rdpilot-daemon --test thread_leak_soak --target x86_64-unknown-linux-gnu -- --include-ignored` → 2/2 passed (`thread_count_returns_to_baseline_after_a_few_cycles`, `thread_and_rss_return_to_baseline_after_fifty_cycles`). `--test registry_concurrency -- --include-ignored` → 2/2 passed. `--test autostart_lifecycle -- --include-ignored` → 1/1 passed. |
| 10 | `ipc_security::cross_account_peer_is_rejected_end_to_end` gating confirmed (expected non-run, not a regression) | ✓ VERIFIED | Source read of `crates/rdpilot-daemon/tests/ipc_security.rs`: the test carries `#[ignore = "requires RDPILOT_SECOND_UID (a second local username, passwordless sudo -u) and --ignored"]`. Default `cargo test -p rdpilot-daemon --test ipc_security` → 3/4 passed, 1 ignored (this test) — expected. Forcing `--include-ignored` without `RDPILOT_SECOND_UID` set correctly panics with an explicit "must be set" message (not a silent pass/skip) — confirms the gate is genuine, not decorative. Other 3 `ipc_security` tests (`authorize_uid_accepts_a_matching_uid`, `authorize_uid_rejects_a_different_uid`, `the_socket_directory_is_mode_0700`) pass unconditionally. |
| 11 | Full workspace test suite (native-Linux substitute) | ✓ VERIFIED | `cargo test --workspace --target x86_64-unknown-linux-gnu`: **461 passed, 0 failed, 32 ignored** across all crates (`rdpilot`, `rdpilot-ipc`, `rdpilot-config`, `rdpilot-daemon` lib + 8 integration test binaries, `rdpilot-cli`, `rdpilot-mcp` lib + 3 integration test binaries). All 32 ignored are the documented gated/heavy tests (28 `rdpilot/tests/live_session.rs` live-RDP-gated, 1 `cli_verbs` Phase-15-deferred placeholder, 1 `autostart_lifecycle` heavy-spawn variant not run by default, 1 `thread_leak_soak` N=50 heavy soak not run by default, 1 `ipc_security::cross_account_peer_is_rejected_end_to_end`). Only compiler warning: pre-existing, unrelated `unused import: crate::error::Error` in `crates/rdpilot/src/input.rs` — not touched by Phase 14. `cargo clippy -p rdpilot-mcp --target x86_64-unknown-linux-gnu --all-targets` — clean, 0 warnings. |
| 12 | TRUST MODEL: rdpilot-mcp README documents no-server-sandbox trust model + rdpilot_put risk note + backlog reference | ✓ VERIFIED | `crates/rdpilot-mcp/README.md` "Trust model" section (read in full) states "**This server implements NO server-side local-path sandbox**", names the concrete exfiltration risk (`/home/user/.ssh/id_rsa` example), references threat-register item **T-14-12**, and has a "Backlog (not a Phase 14 deliverable)" paragraph pointing at "a server-side transfer sandbox... a named, deferred hardening item — see the Phase 14 research document's 'Open Question 2'". `rdpilot_put`'s own `#[tool(description = ...)]` string (in `native_tools.rs`) also carries the same RISK note verbatim, confirmed present. |

**Score:** 12/12 offline-scope criteria verified.

### Deferred Items

Items not verified this phase because the developer explicitly batched them to Phase 15's live gate. Not gaps — deliberately scoped out per `ROADMAP.md`'s own Phase 14 note.

| # | Item | Addressed In | Evidence |
|---|------|-------------|----------|
| 1 | Real screenshot pixel content correctness | Phase 15 | ROADMAP.md Phase 14: "Offline this phase; batched to Phase 15 live gate: real screenshot pixel content..." |
| 2 | Real click landing near edges/corners against a live remote desktop (live half of MCP-04) | Phase 15 | ROADMAP.md Phase 14: "...real click landing near edges/corners (the live half of MCP-04 — the pure scale_to_native is offline-proven here)..." |
| 3 | PROOF-04 live-LLM capstone (read/inspect + file-transfer through MCP against a real remote-only Windows program) | Phase 15 | Phase 15 goal + success criterion #3 (PROOF-04) in ROADMAP.md. |

Note: MCP-06's isolation is architecture-level (per-call task spawn + per-connection daemon isolation + fresh-socket-per-call), proven fully offline against the real compiled daemon binary — it does not require re-exercising at the live gate, per the developer's own scoping note.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rdpilot-mcp/src/computer/scale.rs` | Pure `scale_to_native` bridge | ✓ VERIFIED | Real implementation, 14/14 tests pass, matches rounding/clamp spec exactly |
| `crates/rdpilot-mcp/src/computer/mod.rs` | `computer_20250124` action schema | ✓ VERIFIED | Full Anthropic-parity action enum + `ComputerArgs` with required `session` |
| `crates/rdpilot-mcp/src/computer/dispatch.rs` | Action→ipc-verb dispatch + 3 gap rejections | ✓ VERIFIED | Exhaustive match, real `Request::*` round trips, explicit rejections tested by name |
| `crates/rdpilot-mcp/src/native_tools.rs` | 11 `rdpilot_*` tools + put/get metadata-only | ✓ VERIFIED | All 11 `#[tool]` methods present, `render_transfer` type-level metadata-only |
| `crates/rdpilot-mcp/src/handler.rs` | Combined tool router, `ServerHandler` impl | ✓ VERIFIED | Same router used by tests and by `main.rs`'s real `.serve(stdio())` |
| `crates/rdpilot-mcp/src/connect.rs` | Bounded daemon round trip | ✓ VERIFIED | `round_trip_bounded`/`timeout_wrap`, unit-tested against a genuinely pending future |
| `crates/rdpilot-mcp/README.md` | Trust model documentation | ✓ VERIFIED | Full trust-model section, T-14-12 reference, backlog note |
| `crates/rdpilot-ipc/src/request.rs` (`DesktopSize`) | Session-scoped native-dim source | ✓ VERIFIED | Exhaustive `SessionScoped` match, rejection test present |
| `crates/rdpilot-daemon/src/server.rs` (`FakeTestConnector`/`TEST_SLOW_MS_ENV`) | Test-connector-scoped slowness hook | ✓ VERIFIED | `RealConnector` branch has no slow-path code at all |

### Key Link Verification

| From | To | Via | Status | Details |
|------|-----|-----|--------|---------|
| `computer` tool call | `Request::Mouse`/`Key`/`Screenshot`/`DesktopSize` | `dispatch_computer` → `round_trip_bounded` | WIRED | Confirmed by source read of every match arm |
| `native_tools::*` tool calls | corresponding `Request::*` wire verbs | 1:1 per-tool round trip | WIRED | 11 tools, each backed by a real `round_trip_bounded` call |
| `handler.rs` combined router | `main.rs`'s live `ServerHandler` | `#[tool_handler(router = ...)]` on `impl ServerHandler for RdpilotMcpHandler` | WIRED | Same expression construct that `tool_schema.rs`'s `all_tools()` test helper uses |
| `rdpilot-mcp` | `rdpilot-daemon` (production) | Unix socket via `rdpilot-ipc::connect_or_spawn` | WIRED, thin-client-only | `cargo tree` confirms no direct crate dependency; IPC is the only channel |
| `FakeTestConnector` slowness hook | `RealConnector` (production path) | env-gated selection in `run_inner` | ISOLATED (by design) | `RealConnector` branch carries no `slow_ms`/sleep code |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| `computer` + 11 native tools, correct session-scoping | `cargo test -p rdpilot-mcp --test tool_schema --target x86_64-unknown-linux-gnu` | 62 passed, 0 failed | ✓ PASS |
| `scale_to_native` all required vectors incl. corrected tie | `cargo test -p rdpilot-mcp --test scale_to_native --target x86_64-unknown-linux-gnu` | 14 passed, 0 failed | ✓ PASS |
| Slow tool call does not block fast one (real daemon) | `cargo test -p rdpilot-mcp --test non_blocking --target x86_64-unknown-linux-gnu` | 59 passed, 0 failed (2.06s) | ✓ PASS |
| Full workspace regression | `cargo test --workspace --target x86_64-unknown-linux-gnu` | 461 passed, 0 failed, 32 ignored | ✓ PASS |
| Thin-client dependency gate | `cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu \| grep -E '...'` | 0 matches (clean) | ✓ PASS |
| Clippy clean | `cargo clippy -p rdpilot-mcp --target x86_64-unknown-linux-gnu --all-targets` | 0 warnings | ✓ PASS |

### Probe Execution

No `scripts/*/tests/probe-*.sh` probes declared for this phase; none found under `scripts/`. Verification relied on the `cargo test` integration test binaries listed above, which are the phase's own declared verification mechanism (PLAN `<verification>` blocks). N/A.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|--------------|--------|----------|
| MCP-01 | 14-02/14-04 | `rmcp` stdio server, 12-tool `tools/list` surface | ✓ SATISFIED | `tool_schema.rs` 62/62; `main.rs` real stdio server |
| MCP-02 | 14-01/14-03 | `computer` mega-tool mapped onto SDK input/capture verbs | ✓ SATISFIED | `dispatch.rs` exhaustive action→`Request::*` mapping |
| MCP-03 | 14-04 | 11 `rdpilot_*` native tools exposed | ✓ SATISFIED | `native_tools.rs`, confirmed in tool set |
| MCP-04 | 14-01/14-03 | [BLOCKING] `scale_to_native` coordinate bridge, edge/corner-tested | ✓ SATISFIED (offline half; live click-precision deferred to Phase 15 per developer scoping) | `scale_to_native.rs` 14/14, corrected tie vector confirmed |
| MCP-05 | 14-04 | put/get metadata-only `{path, bytes_transferred, checksum}` | ✓ SATISFIED | `TransferOutcome` type has no byte field; planted-sentinel regression passes |
| MCP-06 | 14-05 | [BLOCKING] slow call does not block fast call, per-call isolation | ✓ SATISFIED | `non_blocking.rs` genuine overlap proof against real daemon; slowness hook test-connector-scoped |

No orphaned requirements: REQUIREMENTS.md's Phase-14 rows (MCP-01 through MCP-06) all appear in at least one plan's declared `requirements` field, and all six are marked `Complete` in REQUIREMENTS.md, consistent with the evidence above.

### Anti-Patterns Found

None. `grep -rn -E "TBD|FIXME|XXX|TODO|HACK|PLACEHOLDER"` across `crates/rdpilot-mcp/src`, `crates/rdpilot-mcp/tests`, `crates/rdpilot-mcp/README.md`, and the phase-14-touched `rdpilot-ipc`/`rdpilot-daemon` files returned zero matches. No "not yet implemented"/"coming soon" strings found. No debt markers requiring the debt-marker gate.

### Human Verification Required

None for the offline scope verified in this report. The three items requiring a live remote Windows target and/or a live LLM (real screenshot pixel content, real click-precision near edges/corners, PROOF-04 capstone) are explicitly deferred to Phase 15 by developer decision and listed under "Deferred Items" above — they are not treated as gaps or as pending human-verification items for Phase 14's own gate.

### Gaps Summary

No gaps found in the offline-provable scope. All 6 requirements (MCP-01 through MCP-06), all 3 computer-use gap rejections, SESSION-02 preservation on `DesktopSize`, the thin-client invariant, the daemon non-regression suite (thread-leak-soak, registry-concurrency, autostart_lifecycle), the `ipc_security` cross-account gate's own gating, the full 461-test workspace run, and the trust-model README documentation are all independently verified against real, freshly-executed test runs and direct source reads — not SUMMARY.md claims.

**One planning-doc drift item (non-blocking, informational):** `.planning/STATE.md`'s `stopped_at`/`last_activity`/`Current Position` narrative is stale — it still reads "Phase 14 Plan 03 complete (3/5 plans)... 14-04 and 14-05 remain," last updated `2026-07-11T10:41:31.293Z`. This contradicts the authoritative sources: `ROADMAP.md` shows Phase 14 as `5/5 plans complete` with every plan checkbox ticked, `REQUIREMENTS.md` marks MCP-01 through MCP-06 all `Complete`, and `git log` confirms all 5 plans' commits landed through `fe1c9b9 docs(14-05): complete MCP-06 non-blocking isolation plan`. This is the same class of narrative drift noted in prior-phase VERIFICATION.md files (STATE.md has drifted before) and does not affect the codebase-level verdict — ROADMAP.md and REQUIREMENTS.md are the authoritative phase-completion sources per this project's convention, and both correctly show Phase 14 complete. Recommend a STATE.md refresh pass, but this is not a phase-14 blocker.

---

## Overall Verdict: VERIFIED (offline)

The MCP surface achieves the Phase 14 goal on every offline-provable dimension: the `computer` mega-tool (computer_20250124-compatible schema) and the 11 `rdpilot_*` native tools are registered, correctly session-scoped, and dispatch onto real `rdpilot-ipc` wire verbs; `scale_to_native` is pure, fully tested at corners/near-corners/center/exclusive-edge including the corrected exact-tie vector; put/get responses are metadata-only by type-level construction with a passing planted-bytes regression guard; and the MCP-06 non-blocking isolation proof demonstrates genuine overlap (a still-pending slow call while a fast call already returned) against the real compiled daemon, with the slowness hook strictly confined to the test-only connector. SESSION-02 is preserved on the new `DesktopSize` verb, the thin-client dependency invariant holds, and the daemon's pre-existing non-regression suites (thread-leak-soak, registry-concurrency, autostart_lifecycle) plus the full 461-test workspace run are all green. The `rdpilot-mcp` README documents the no-server-sandbox trust model with the required risk note and backlog reference.

Phase 14 is ready for Phase 15's proof harnesses and live-LLM capstone. The three explicitly-deferred live items (real screenshot pixel content, real click-precision near edges/corners, and the PROOF-04 capstone) remain to be exercised at that batched live gate, per the developer's own scoping decision recorded in ROADMAP.md.

---

_Verified: 2026-07-11T10:49:58Z_
_Verifier: Claude (gsd-verifier)_
