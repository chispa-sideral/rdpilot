---
phase: 15-proof-harnesses-live-llm-capstone
plan: 04
subsystem: rdpilot-mcp (gated live-LLM capstone via claude -p, PROOF-04)
tags: [proof-04, offline-author, claude-p, mcp-config, transcript-assertion, independent-side-effect, gated-live-test]
dependency_graph:
  requires: ["crates/rdpilot-mcp/tests/live_proof.rs (15-03's rmcp client-subprocess Pattern 2, reused for the independent verification client)", "crates/rdpilot-mcp/src/native_tools.rs + handler.rs (the 12 rdpilot_*/computer tools the capstone drives)", "crates/rdpilot-mcp/src/config_params.rs + connect_impl (D-27 file->env->MCP-init layering -- confirms rdpilot_connect resolves credentials from the server subprocess's own env, so the prompt never needs them)", "crates/rdpilot/tests/support/proof_harness.rs (SEVEN_ZIP_EXE/SEVEN_ZIP_SEED_ARGS/SEVEN_ZIP_CLASS constants mirrored verbatim for target continuity)", "crates/rdpilot-cli/tests/cli_lifecycle.rs (file-capture stdio discipline -- Stdio::from(File) + .status()/.wait(), never output()/wait_with_output())", "the locally installed `claude` CLI (v2.1.207) -- --help inspected live this session to confirm --mcp-config/--strict-mcp-config/--permission-mode/-p/--output-format flag names"]
  provides: ["crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json (secret-free --mcp-config template)", "crates/rdpilot-mcp/tests/live_capstone.rs (PROOF-04 gated capstone harness)"]
  affects: ["Plan 15-08 (runs live_capstone.rs against the VM + a live claude auth session; the ONE place it may need to adjust is PERMISSION_FLAGS)"]
tech_stack:
  added: []
  patterns: ["claude -p headless print mode + --mcp-config pointed at env!(\"CARGO_BIN_EXE_rdpilot-mcp\") -- no ANTHROPIC_API_KEY/reqwest/SDK/hardcoded model id anywhere", "connection secrets reach the MCP server subprocess ONLY via its own process environment (D-27 RDPILOT_HOST/PORT/USERNAME/PASSWORD), never the LLM's prompt or transcript", "two-part success assertion: schema-tolerant recursive tool_use-name transcript walk (never depends on an exact stream-json envelope) AND a SECOND, independent rmcp client subprocess downloading the file and comparing bytes verbatim against a locally-authored seed (no re-derived trust from the model's self-report or even its own reported checksum)", "fixed/deterministic session name + remote_name + local seed file baked into the prompt so the independent verification step never needs to discover the model's own choices", "file-capture stdio discipline (Stdio::from(File) + child.wait() via tokio::time::timeout, never output()/wait_with_output()) for the claude -p subprocess, mirroring cli_lifecycle.rs's documented pitfall"]
key_files:
  created:
    - crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json
    - crates/rdpilot-mcp/tests/live_capstone.rs
  modified: []
decisions:
  - "The LLM driving PROOF-04 is the LOCAL, already-authenticated claude CLI (`claude -p --mcp-config <rendered fixture>`) per the plan's binding direction 1 -- explicitly supersedes 15-RESEARCH.md's earlier reqwest/Anthropic-API sketch (that research predates this pivot; 15-03's Cargo.toml already carried a forward-note anticipating it). No ANTHROPIC_API_KEY, no reqwest, no Anthropic SDK, no hardcoded model id anywhere in this plan's files."
  - "Independent side-effect verification uses a direct byte-for-byte file comparison (original seed bytes vs. a freshly, independently rdpilot_get-downloaded copy) rather than reimplementing SHA-256 locally to compare against the daemon-reported checksum -- strictly stronger than a checksum match (no collision possibility) and needs zero new dependency. Confirmed via `git diff --stat -- Cargo.toml Cargo.lock` showing no change at all (not even a dev-dependency)."
  - "Connection secrets (host/port/username/password) are threaded through the rendered --mcp-config's `env` block only -- picked up by rdpilot_connect's existing rdpilot_config::resolve(...) file->env->MCP-init layering (D-27) when the model calls rdpilot_connect with no host/username/password params. The task prompt explicitly tells the model NOT to pass or ask for credentials. This means the captured transcript can never contain the password by construction, independent of any redaction logic."
  - "The exact `claude -p` non-interactive MCP tool-approval flag is unverified (this plan is OFFLINE-AUTHOR; nothing live ran). Isolated in one named constant (`PERMISSION_FLAGS = [\"--permission-mode\", \"bypassPermissions\", \"--strict-mcp-config\"]`), chosen from the locally installed CLI's own `--help` (v2.1.207, inspected live this authoring session) as a sane default, with a documented `--dangerously-skip-permissions` fallback in the constant's doc comment. Plan 15-08 changes ONLY this constant if it proves insufficient."
  - "Transcript tool-call detection walks the ENTIRE parsed stream-json JSON tree recursively looking for `{\"type\": \"tool_use\", \"name\": ...}` shapes and matches tool names by SUFFIX (e.g. ends_with(\"rdpilot_uia\")) rather than hardcoding Claude Code's `mcp__<server>__<tool>` prefix convention verbatim -- the exact CLI JSONL envelope for this installed version could not be verified without actually running `claude -p` against live MCP tools (out of scope, OFFLINE-AUTHOR), so the parser is deliberately schema-tolerant rather than schema-brittle."
  - "PROOF-04's REQUIREMENTS.md traceability row was updated in place (Pending -> In Progress, describing what's authored + what's deferred to 15-08) rather than running `requirements mark-complete` -- mirrors the identical precedent 15-02/15-03 set for PROOF-02/PROOF-03 (the requirement's own text, 'A capstone live-LLM demo drives...', is not satisfied until the live run actually executes)."
metrics:
  duration: "~50 min"
  completed: 2026-07-11
---

# Phase 15 Plan 04: PROOF-04 Live-LLM Capstone (claude -p) Summary

Authored the PROOF-04 capstone as a gated `rdpilot-mcp` integration test that spawns the local, already-authenticated `claude` CLI in headless print mode against the real compiled `rdpilot-mcp` binary via `--mcp-config`, asserting success from transcript tool-call evidence AND an independent, second-client byte-for-byte file comparison — never from the model's self-report. No Anthropic API key, `reqwest`, SDK, or hardcoded model id anywhere; no new Cargo dependency of any kind.

## What Was Built

**Task 1 — `crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json`.** A committed, secret-free JSON template in the standard Claude Code `--mcp-config` shape (`{"mcpServers": {"rdpilot": {"command", "args", "env"}}}`), with a top-level `_comment` array documenting the five substitution tokens (`__RDPILOT_MCP_BIN__`, `__RDPILOT_HOST__`, `__RDPILOT_PORT__`, `__RDPILOT_USERNAME__`, `__RDPILOT_PASSWORD__`) and the runtime rendering contract. `RDPILOT_ACCEPT_INVALID_CERTS` is pre-set to `"true"` (the disposable lab VM's self-signed cert, mirroring `live_proof.rs`'s identical risk-named passthrough). Validated with `python3 -c "import json; json.load(open(...))"` per the plan's `<verify>` block.

**Task 2 — `crates/rdpilot-mcp/tests/live_capstone.rs`.** A single `#[ignore]`-gated `#[tokio::test]`, `capstone_llm_drives_read_inspect_and_file_transfer_through_mcp`:

- **Gating (D-18):** Loads `.secrets/connection.json` under `RDPILOT_LIVE` exactly like `live_proof.rs` (a legitimate, quiet skip when unarmed). Once armed, a missing `claude` binary on `PATH` (probed via a short `claude --version` call) is a HARD `assert!` failure with a descriptive message — never a silent `eprintln!`+`return`, satisfying the plan's explicit "fail fast, never a silent skip that reads like a pass" requirement for this specific gate half. Deliberately never checks `ANTHROPIC_API_KEY`.
- **Config rendering:** `render_mcp_config` reads the Task-1 template via `include_str!`, substitutes `env!("CARGO_BIN_EXE_rdpilot-mcp")` and the loaded `LiveTarget`'s host/port/username/password into the parsed `serde_json::Value` (never raw string substitution, so no escaping bugs), strips the `_comment` key, and writes the rendered JSON to a per-run temp directory — removed at the end of the test, never logged.
- **Deterministic task prompt (`capstone_prompt`):** Instructs the model, step by step, to `rdpilot_connect` with a FIXED session name (`capstone-04`, no credentials passed/requested), launch 7-Zip File Manager (`SEVEN_ZIP_EXE`/`SEVEN_ZIP_SEED_ARGS`/`SEVEN_ZIP_CLASS` mirrored verbatim from `crates/rdpilot/tests/support/proof_harness.rs` for target continuity with Phase 9/PROOF-02/03), find+foreground its window, `rdpilot_uia` its menu bar (read/inspect half), then `rdpilot_put`/`rdpilot_get` a FIXED local seed file (`CAPSTONE_SEED_CONTENT`, authored by the test itself before spawning `claude`) under a FIXED `remote_name` (`rdpilot-capstone-04-upload.bin`). The fixed names let the independent verification step address the exact session/file without discovery.
- **`claude -p` subprocess (`run_claude_p`):** `claude -p "<prompt>" --mcp-config <rendered> --permission-mode bypassPermissions --strict-mcp-config --output-format stream-json --verbose`, stdout/stderr redirected to REAL FILES (`Stdio::from(File)` + `child.wait()`, never `output()`/`wait_with_output()`) — mirrors `cli_lifecycle.rs`'s documented pitfall: the `rdpilot-mcp` server subprocess `claude` spawns auto-starts a detached `rdpilot-daemon` grandchild that would otherwise hold a piped stdout open forever. Bounded by a 300s `tokio::time::timeout` (T-15-10); on timeout the child is killed and the (possibly partial) captured transcript is still evaluated, never discarded.
- **Transcript assertion:** `tool_use_names_from_transcript` parses the captured stdout as JSONL, and `collect_tool_use_names` recursively walks EVERY parsed value looking for `{"type": "tool_use", "name": ...}` shapes (schema-tolerant by design — the exact stream-json envelope for the installed CLI version was not runnable offline this plan). `any_tool_called` matches by name SUFFIX (`rdpilot_uia`/`computer` for read/inspect, `rdpilot_put`/`rdpilot_get` for file-transfer) since Claude Code's `mcp__<server>__<tool>` prefix convention could not be verified live.
- **Independent side-effect verification:** A SECOND, wholly separate `TokioChildProcess` + `().serve(...)` client (Pattern 2, `live_proof.rs`) spawns its OWN `rdpilot-mcp` subprocess and calls `rdpilot_get(session="capstone-04", remote_name="rdpilot-capstone-04-upload.bin", ...)` to a fresh local path, then compares the downloaded bytes BYTE-FOR-BYTE against `CAPSTONE_SEED_CONTENT` — no dependency on the model's own reported checksum, no new hashing dependency.
- **D-9.4 trace:** `record()` prints/collects `[PASS]`/`[FAIL]` per step and a final `PROOF: PASS`/`PROOF: FAIL` line, mirroring `live_proof.rs`. Only three of the four recorded steps decide the outcome (transcript read/inspect, transcript file-transfer, independent side-effect) — the raw subprocess exit-code step is informational only, since a model that completed every needed tool call but then hit an unrelated late-turn CLI hiccup should still be judged on what it demonstrably did.
- Cleans up: disconnects the capstone session via the independent client, removes the seed/download/rendered-config files and the temp capture directory.

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-mcp --target x86_64-unknown-linux-gnu --test live_capstone -- --list` — lists `capstone_llm_drives_read_inspect_and_file_transfer_through_mcp: test` (1 test).
- `cargo test -p rdpilot-mcp --target x86_64-unknown-linux-gnu` (full offline suite, no `RDPILOT_LIVE`) — `live_capstone.rs` reports `0 passed; 0 failed; 1 ignored`; every other file in the crate (unit tests 58, `live_mcp_computer.rs` 6/1-ignored, `live_proof.rs` 0/1-ignored, `non_blocking.rs` 59, `scale_to_native.rs` 14, `tool_schema.rs` 62) passes unchanged.
- `cargo build --workspace --target x86_64-unknown-linux-gnu` and `cargo test --workspace --target x86_64-unknown-linux-gnu` — both green across every crate in the workspace (one pre-existing, unrelated `unused_imports` warning in `crates/rdpilot/src/input.rs`, already noted by 15-02/15-03, not touched here).
- `cargo clippy -p rdpilot-mcp --target x86_64-unknown-linux-gnu --tests` — zero warnings.
- `python3 -c "import json; json.load(open('crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json'))"` — valid JSON.
- `git diff --stat -- crates/rdpilot-mcp/Cargo.toml Cargo.lock` — empty (no dependency of any kind added, dev or otherwise).
- `cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu -e no-dev` — no `reqwest`/`anthropic`/`sha2` in the production tree (unchanged from before this plan; the shipped, non-dev graph was already clean per 15-03's own gate).

**Toolchain substitution note:** built/tested via the native Linux substitute target (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu`, `--target x86_64-unknown-linux-gnu`), identical substitution to 15-01/15-02/15-03; `cargo`/`rustup` were resolved via `$HOME/.cargo/bin` (not on the default `PATH` in this shell). The local `claude` CLI itself IS installed in this environment (v2.1.207, confirmed via `claude --version`/`claude --help`) and its `--help` output was inspected live to ground `PERMISSION_FLAGS`'s flag names in fact rather than guesswork — but `claude -p` was never actually invoked/run this plan (OFFLINE-AUTHOR scope; the live run is Plan 15-08).

## Deviations from Plan

None — both files were authored following the plan's action text, `15-RESEARCH.md`'s prior-art patterns (Pattern 2, D-9.4 trace), and `live_proof.rs`/`cli_lifecycle.rs`'s established conventions closely; compiled clean on the first attempt with zero clippy warnings; no auto-fixes needed (Rules 1-3 did not trigger). One discretionary design choice beyond the plan's literal text: independent side-effect verification uses direct byte comparison instead of a locally-recomputed SHA-256 checksum, to avoid a new dependency — documented above as a decision, not a deviation from any explicit plan instruction (the plan said "checksum matching the known local file," which a byte-for-byte match satisfies at least as strongly).

## Known Stubs

None — both files are complete, gated, compile-checked test harnesses; no hardcoded empty values or placeholder rendering paths.

## Threat Flags

None beyond what the plan's own `<threat_model>` already names (T-15-08 secrets-in-rendered-config, T-15-09 prompt-injection-accepted-risk, T-15-10 bounded-timeout, T-15-SC no-new-packages). All four are implemented exactly as specified: the rendered config lives in a gitignored temp path removed at test end and is never logged; the prompt-injection risk is documented in this file's own doc comments per T-15-09's mitigation plan; the 300s `tokio::time::timeout` bounds the whole `claude -p` run; and no new package was added at all (not even the anticipated `sha2` dev-dependency).

## What Remains (Live Run)

- **Plan 15-08:** `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_capstone -- --ignored` against the provisioned Azure VM with an authenticated local `claude` CLI session. This is the one place `PERMISSION_FLAGS` may need adjusting if `bypassPermissions --strict-mcp-config` still prompts interactively for MCP tool approval — the documented fallback is `--dangerously-skip-permissions` (paired with `--allow-dangerously-skip-permissions` first if the installed CLI gates it behind that companion flag). 15-08 should also sanity-check the `stream-json` transcript's actual tool-name shape against `any_tool_called`'s suffix-matching assumption, and confirm the 300s `CAPSTONE_TIMEOUT` is adequate for a real RDP-driven multi-tool-call run (widen if needed — it is a single named constant).
- Neither the fixture nor the test file has been executed against a live target yet; both were authored and compile-checked offline only, per this plan's OFFLINE-AUTHOR scope. `claude -p` itself was never invoked.

## Self-Check: PASSED

- FOUND: `crates/rdpilot-mcp/tests/fixtures/capstone-mcp-config.json`
- FOUND: `crates/rdpilot-mcp/tests/live_capstone.rs`
- FOUND commit `b7f86eb` (test(15-04): author PROOF-04 mcp-config template fixture)
- FOUND commit `eb0fab0` (test(15-04): author PROOF-04 live-LLM capstone (claude -p + independent side-effect verification))
