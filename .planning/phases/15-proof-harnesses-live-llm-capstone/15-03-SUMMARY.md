---
phase: 15-proof-harnesses-live-llm-capstone
plan: 03
subsystem: rdpilot-mcp (gated live MCP proof/computer-tool integration tests)
tags: [proof-03, mcp-04, offline-author, rmcp-client, cargo-bin-exe, gated-live-test, tokio-child-process]
dependency_graph:
  requires: ["crates/rdpilot-cli/tests/live_proof.rs (PROOF-02's PASS/FAIL trace + CARGO_BIN_EXE_/live-target-loader convention this harness mirrors)", "crates/rdpilot-cli/tests/live_cli_verbs.rs (CLI-02's UIA-focus click-landing verification pattern, reused for the MCP-04 live half)", "crates/rdpilot-mcp/src/native_tools.rs + handler.rs (the 12 rdpilot_*/computer tools this harness drives)", "crates/rdpilot-mcp/src/computer/scale.rs (ADVERTISED_WIDTH/HEIGHT pulled in via #[path], mirroring tests/scale_to_native.rs's precedent)", "rmcp 2.2.0 client + transport-child-process APIs (TokioChildProcess, ().serve(...), Peer<RoleClient>::call_tool/list_all_tools)"]
  provides: ["crates/rdpilot-mcp/Cargo.toml [dev-dependencies] rmcp client+transport-child-process", "crates/rdpilot-mcp/tests/live_proof.rs (PROOF-03 gated harness)", "crates/rdpilot-mcp/tests/live_mcp_computer.rs (MCP-04 live-half gated test)"]
  affects: ["Plan 15-06 (runs live_mcp_computer.rs against the VM)", "Plan 15-07 (runs live_proof.rs against the VM)"]
tech_stack:
  added: ["rmcp 2.2.0 [dev-dependencies] client + transport-child-process features (additive on the already-D-26-approved production version; transitively pulls process-wrap/nix/tokio-stream/cfg_aliases as NEW dev-only crates -- confirmed absent from the shipped, non-dev cargo tree)"]
  patterns: ["rmcp client-side subprocess transport: TokioChildProcess::new(Command::new(bin)) + ().serve(transport).await -> RunningService<RoleClient, ()> derefs to Peer<RoleClient> (call_tool/list_all_tools/cancel)", "credentials passed as MCP tool-call JSON arguments over the child's stdin, never via child env/argv (stronger than the CLI harness's necessarily argv-based approach)", "D-9.4 per-step PASS/FAIL trace + a final PROOF:/MCP-04 LIVE HALF: PASS/FAIL line, mirroring live_proof.rs (PROOF-02)", "hardcoded advertised-space (1280x800) corner coordinates targeting well-known Windows UI landmarks (menu-bar File item, title-bar Minimize button) -- never a coordinate computed from a pre-click UIA query (Pitfall 3)", "#[path] import of computer/scale.rs's pure ADVERTISED_WIDTH/HEIGHT constants into the test binary, mirroring tests/scale_to_native.rs's established precedent"]
key_files:
  created:
    - crates/rdpilot-mcp/tests/live_proof.rs
    - crates/rdpilot-mcp/tests/live_mcp_computer.rs
  modified:
    - crates/rdpilot-mcp/Cargo.toml
decisions:
  - "rmcp's client + transport-child-process features live in [dev-dependencies] only, never merged into the production [dependencies] rmcp entry (server/macros/transport-io) -- cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu -e normal (the shipped, non-dev graph) is confirmed byte-for-byte IDENTICAL before and after this plan's Cargo.toml edit, so the thin-client invariant (D-17) holds exactly."
  - "Deliberately NO reqwest/base64 dev-dependency added, per the plan's binding constraint -- the capstone (Plan 15-04) drives claude -p, not the Anthropic HTTP API, so those research-recommended deps are not needed by anything this plan authors."
  - "The MCP-04 live-half harness targets TWO well-known, hardcoded Windows UI landmarks in the advertised 1280x800 space (never derived from a pre-click UIA query, Pitfall 3): the maximized 7-Zip FM window's top-left menu-bar 'File' item (verified via a focused UIA element after the click) and its top-right title-bar 'Minimize' button (verified via the window state transitioning to 'minimized', a more decisive signal than UIA focus). Minimize is deliberately the LAST corner tested since the window is not needed afterward."
  - "Exact pixel offsets from the true corners are a documented ESTIMATE (small margins into the menu bar/title bar) -- explicitly expected to need calibration once this test actually runs against the live VM (Plan 15-06), consistent with this plan's OFFLINE-AUTHOR scope; a landing mismatch there is a finding for 15-06 to surface and fix, not a defect in this authoring pass."
  - "PROOF-03/MCP-04 traceability rows in REQUIREMENTS.md were updated in place (PROOF-03: Pending -> In Progress; MCP-04: appended a 15-03 live-half-authored note to its existing Phase-14-Complete row) rather than running requirements mark-complete for PROOF-03 -- PROOF-03's own requirement text ('proves ... end-to-end') is not yet satisfied until the live run (Plan 15-07) actually executes, mirroring the identical precedent 15-02 set for PROOF-02 (top-level checkbox stayed unchecked, only the traceability row's descriptive text was updated)."
metrics:
  duration: "~65 min"
  completed: 2026-07-11
---

# Phase 15 Plan 03: PROOF-03 MCP Harness + MCP-04 Live Half Summary

Added rmcp's client-side dev-dependencies and authored two gated `tests/*.rs` integration tests in `rdpilot-mcp` -- `live_proof.rs` (PROOF-03's scripted MCP end-to-end harness, driving the real compiled `rdpilot-mcp` binary as an rmcp client subprocess with no live LLM) and `live_mcp_computer.rs` (the MCP-04 live half: real click landing near screen edges/corners through the `computer` tool) -- both compiled and verified offline on the Linux substitute target with no live run performed.

## What Was Built

**Task 1 -- `crates/rdpilot-mcp/Cargo.toml`.** Added a `[dev-dependencies]` block: `rmcp = { version = "2.2.0", features = ["client", "transport-child-process"] }` -- the SAME already-D-26-approved crate/version as the production `[dependencies] rmcp` entry (server/macros/transport-io), only additive feature flags, so no new package-legitimacy checkpoint was needed or triggered. `cargo build -p rdpilot-mcp --tests` resolves cleanly, pulling in 4 new dev-only transitive crates (`process-wrap`, `nix`, `tokio-stream`, `cfg_aliases`). `cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu -e normal` (the shipped, non-dev graph) was diffed byte-for-byte against the pre-change tree via `git stash`/`git stash pop` and confirmed IDENTICAL -- the thin-client invariant (D-17) is unaffected; `base64` appearing in the full (dev-inclusive) tree is a PRE-EXISTING transitive dependency of the production `rmcp` entry itself, not something this plan introduced.

**Task 2 -- `crates/rdpilot-mcp/tests/live_proof.rs` (PROOF-03).** A single `#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM"]` async test, `mcp_end_to_end_against_a_real_target`, that:

- Locally re-implements the `.secrets/connection.json` load/gate convention (`LIVE_ENV = "RDPILOT_LIVE"`), mirroring `crates/rdpilot-cli/tests/live_proof.rs`'s identical duplication (`rdpilot-mcp` never depends on `rdpilot`, D-17).
- Spawns the REAL compiled `rdpilot-mcp` binary via `rmcp`'s client transport: `TokioChildProcess::new(Command::new(env!("CARGO_BIN_EXE_rdpilot-mcp")))` + `().serve(transport).await` (the unit type as a trivial `ClientHandler`), completing the MCP `initialize` handshake against the real server.
- Drives the full sequence programmatically (no LLM): `tools/list` discovery (asserts the expected `rdpilot_*` tools are advertised) -> `rdpilot_connect` (host/username/password/port passed as MCP tool-call JSON *arguments*, never child env/argv) -> `rdpilot_list` (asserts the session appears with a D-30 status) -> `rdpilot_world_state` with `screenshot: true` (one perception read) -> `rdpilot_put` then `rdpilot_get` (asserts the MCP-05 `{bytes_transferred, checksum}` metadata matches between the two calls) -> `rdpilot_disconnect`, each session-scoped call carrying an explicit `session` (D-29).
- A `call()` helper classifies outcomes correctly for this codebase's actual error-surfacing shape: every `rdpilot_*`/`computer` tool maps its own `McpError` into a JSON-RPC PROTOCOL-level error (`Result<CallToolResult, rmcp::ErrorData>`), never a successful `CallToolResult{is_error: Some(true)}` -- so `call()` treats an `Err` from `call_tool` as the primary failure signal, with `is_error` also checked defensively.
- Prints a D-9.4-style per-step `[PASS]`/`[FAIL]` trace and a final `PROOF: PASS`/`PROOF: FAIL` line, mirroring `live_proof.rs` (PROOF-02)'s established style. Shuts the client/subprocess down cleanly via `RunningService::cancel()`.

**Task 3 -- `crates/rdpilot-mcp/tests/live_mcp_computer.rs` (MCP-04 live half).** A second `#[ignore]`-gated async test, `computer_click_lands_near_screen_edges_and_corners_mcp04_live`, closing what Phase 14 deferred (the pure `scale_to_native` math is already offline-proven; this proves the scaled coordinates actually LAND on real Windows):

- Connects, launches + foregrounds 7-Zip File Manager (continuity with the Phase 9/PROOF-02/03 target), then maximizes it via `computer` `key` `"win+up"` (no coordinate needed) so the window's corners coincide with the SCREEN's corners -- polled via `rdpilot_window_list` until `state == "maximized"`.
- Takes a `computer` `screenshot` (asserts an image content block is present, in the fixed advertised 1280x800 space) then issues two `computer` `left_click` actions near hardcoded, well-known landmarks rather than a computed target (Pitfall 3): near the **top-left corner** (`[10, 10]`, the menu bar's `"File"` item -- verified by a follow-up `rdpilot_uia` children query reporting a focused element, matched by name with an "any focused" fallback mirroring `live_cli_verbs.rs`'s CLI-02 precedent) and near the **top-right corner** (`[ADVERTISED_WIDTH - 30, 10]`, the title-bar `"Minimize"` button -- verified via `rdpilot_window_list` polling for `state == "minimized"`, a more decisive signal than UIA focus; deliberately the LAST corner clicked since the window is not needed afterward).
- `ADVERTISED_WIDTH` is imported directly from `computer/scale.rs` via `#[path]` (mirroring `tests/scale_to_native.rs`'s established precedent for pulling in that pure, zero-dependency module) rather than a hand-duplicated literal, so the coordinate reasoning can never silently drift from the D-14.2-LOCKED 1280x800 constant the production binary actually links.
- Same D-9.4 trace/gating/security conventions as `live_proof.rs`. Ends with `rdpilot_disconnect` and `RunningService::cancel()`.

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build -p rdpilot-mcp --tests --target x86_64-unknown-linux-gnu` -- green, both new files compiled clean on the FIRST attempt (no fix-up iterations needed).
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build --workspace --target x86_64-unknown-linux-gnu` -- green (one pre-existing, unrelated `unused_imports` warning in `crates/rdpilot/src/input.rs`, not touched by this plan, already noted by 15-02).
- `cargo clippy -p rdpilot-mcp --tests --target x86_64-unknown-linux-gnu` -- zero warnings.
- `cargo test -p rdpilot-mcp --target x86_64-unknown-linux-gnu --test live_proof -- --list` -- lists `mcp_end_to_end_against_a_real_target: test` (1 test).
- `cargo test -p rdpilot-mcp --target x86_64-unknown-linux-gnu --test live_mcp_computer -- --list` -- lists `computer_click_lands_near_screen_edges_and_corners_mcp04_live: test` plus the 6 pure `scale.rs` unit tests pulled in via `#[path]` (7 tests total; identical precedent to `tests/scale_to_native.rs`).
- `cargo test -p rdpilot-mcp --target x86_64-unknown-linux-gnu` (full offline suite, no `RDPILOT_LIVE`) -- unit tests (58) + `live_mcp_computer.rs` (6 pass, 1 ignored) + `live_proof.rs` (0 pass, 1 ignored) + `non_blocking.rs` (59) + `scale_to_native.rs` (14) + `tool_schema.rs` (62) all pass unchanged; both new gated tests report `ignored, requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)` -- confirmed nothing new executes offline.
- `cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu -e normal` -- diffed byte-for-byte against the pre-change tree (`git stash`/`git stash pop`) and confirmed IDENTICAL; grep for `ironrdp`/`rustls`/`rdpilot-daemon`/a bare `rdpilot` package found nothing -- thin-client invariant (D-17) intact for the SHIPPED binary. The full (dev-inclusive) `cargo tree` does show the new `rmcp (dev)` entry plus its 4 new transitive dev-only crates, exactly as expected for a `[dev-dependencies]`-only addition.

**Toolchain substitution note:** built/tested via the native Linux substitute target (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu`, `--target x86_64-unknown-linux-gnu`) since the pinned `stable-x86_64-pc-windows-gnu` toolchain is not installed in this environment -- identical substitution to 15-01/15-02.

## Deviations from Plan

None -- both files were authored following the plan's action text and 15-RESEARCH.md's PROOF-03 skeleton/Pattern 2 closely, compiled clean on the first attempt with zero clippy warnings, and needed no auto-fixes (Rules 1-3 did not trigger).

## Known Stubs

None -- both files are complete, gated, compile-checked test harnesses. No hardcoded empty values or placeholder rendering paths were introduced.

## Threat Flags

None beyond what the plan's own `<threat_model>` already names (T-15-06 password-never-printed/never-in-child-env-or-argv, T-15-07 coordinate-space confusion). Both mitigations are implemented exactly as specified: no `Debug`/`Display` impl on `LiveTarget`, credentials passed only as MCP tool-call JSON arguments over stdin (never the child's env or command line -- a STRONGER guarantee than the CLI harness's necessarily argv-based approach, since the MCP transport carries them over a private pipe rather than a process-list-visible argv), and all click-coordinate reasoning stays in the advertised 1280x800 space (UIA read only after each click, only to verify). No new network endpoints, auth paths, or schema changes were introduced; this plan only adds test code exercising existing, already-threat-modeled surfaces.

## What Remains (Live Runs)

- **Plan 15-06:** `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_mcp_computer -- --ignored` against the provisioned Azure VM -- exercises the MCP-04 live half (real click landing near the menu-bar/title-bar corner landmarks, verified via UIA focus / window-state transition). The hardcoded pixel offsets are expected to need calibration against the real 7-Zip FM window layout; this is the anticipated purpose of that live run, not a defect here.
- **Plan 15-07:** `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_proof -- --ignored` against the same VM -- exercises PROOF-03's full `tools/list` -> connect -> list -> world_state -> put -> get -> disconnect sequence over real MCP JSON-RPC.
- Neither test file has been executed against a live target yet; both were authored and compile-checked offline only, per this plan's OFFLINE-AUTHOR scope.

## Self-Check: PASSED

- FOUND: `crates/rdpilot-mcp/Cargo.toml` (dev-dependencies rmcp client+transport-child-process)
- FOUND: `crates/rdpilot-mcp/tests/live_proof.rs`
- FOUND: `crates/rdpilot-mcp/tests/live_mcp_computer.rs`
- FOUND commit `ed8797f` (chore(15-03): add rmcp client dev-deps for the PROOF-03/MCP-04 live harnesses)
- FOUND commit `7c582cf` (test(15-03): author PROOF-03 gated MCP end-to-end proof harness)
- FOUND commit `5a1a7f3` (test(15-03): author MCP-04 live-half gated click-landing test)
