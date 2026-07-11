---
phase: 14-mcp-server-surface
plan: 02
subsystem: mcp-server
tags: [rmcp, mcp, stdio, thin-client, timeout, error-taxonomy]
dependency_graph:
  requires: [rdpilot-ipc::connect_or_spawn/read_frame/write_frame/socket_path, rdpilot-ipc::WireError/WireErrorCode, rdpilot-cli::connect.rs pattern]
  provides: [rdpilot-mcp crate, RdpilotMcpHandler, connect::round_trip_bounded, timeouts::{FAST,CONNECT,LIFECYCLE,TRANSFER}, error::McpError]
  affects: [Cargo.toml workspace members]
tech_stack:
  added: ["rmcp 2.2.0 (server/macros/transport-io)", "schemars ^1.0"]
  patterns: ["thin-client crate (rdpilot-ipc/rdpilot-config only, D-17)", "fresh-UnixStream-per-call transport (relocated from rdpilot-cli)", "tokio::time::timeout bounded round trip (MCP-06 Layer 3)"]
key_files:
  created:
    - crates/rdpilot-mcp/Cargo.toml
    - crates/rdpilot-mcp/src/main.rs
    - crates/rdpilot-mcp/src/handler.rs
    - crates/rdpilot-mcp/src/connect.rs
    - crates/rdpilot-mcp/src/timeouts.rs
    - crates/rdpilot-mcp/src/error.rs
  modified:
    - Cargo.toml
    - Cargo.lock
decisions:
  - "rmcp 2.2.0 + schemars ^1.0 pinned exactly per D-26/research; no legitimacy checkpoint needed (both pre-verified [OK]/[VERIFIED])"
  - "round_trip_bounded's tokio::time::timeout wrapping factored into a private, directly-testable timeout_wrap helper so the Duration::ZERO -> McpError::Timeout mapping is unit-tested against a genuinely pending future, no live daemon socket needed"
  - "McpError -> rmcp::ErrorData renders via ErrorData::internal_error(message, Some(json!({\"code\": discriminant})))-- both WireError's code (kebab-case discriminant in `data.code`) and message text surface in the tool error (D-28)"
  - "Deferred REQUIREMENTS.md MCP-01/MCP-06 checkbox updates: this plan delivers the mechanism (server bootstrap + bounded-timeout transport) but not the full requirement (MCP-01 needs tools, Plan 14-04; MCP-06's non-blocking proof is Plan 14-05) -- marking those requirements complete now would be premature"
metrics:
  duration: "~45 min"
  completed: 2026-07-11
---

# Phase 14 Plan 02: rdpilot-mcp thin-client scaffold Summary

Scaffolded the `rdpilot-mcp` crate as the fourth thin `rdpilot-ipc`/`rdpilot-config`-only daemon client (D-17): an rmcp 2.2.0 stdio MCP server binary with zero tools registered, a `round_trip_bounded` transport reusing the CLI's fresh-connection `connect_or_spawn` pattern wrapped in an explicit per-verb-class `tokio::time::timeout` (MCP-06 Layer 3), and an `McpError` taxonomy that renders `WireError { code, message }` as an rmcp tool error (D-28).

## What Was Built

**Task 1 — Crate scaffold + workspace member + stdio bootstrap** (commit `45c4406`):
- Added `"crates/rdpilot-mcp"` to the root `Cargo.toml` workspace `members` array.
- `crates/rdpilot-mcp/Cargo.toml`: a binary crate with EXACTLY the thin-client dependency set — `rmcp = { version = "2.2.0", features = ["server", "macros", "transport-io"] }`, `schemars = "1"`, `rdpilot-ipc`, `rdpilot-config`, `tokio`, `serde`, `serde_json`, `thiserror`, `tracing`, `tracing-subscriber`. Never `rdpilot`/`rdpilot-daemon`/`interprocess`.
- `src/main.rs`: crate-root deny lints (`unsafe_code`, `clippy::unwrap_used`, `clippy::expect_used`) plus `#![deny(clippy::print_stdout)]` (T-14-04 — stdout is the JSON-RPC wire). `#[tokio::main]` entry initializes `tracing_subscriber::fmt().with_writer(std::io::stderr)` as the FIRST statement, then `RdpilotMcpHandler::new().serve(stdio()).await` + `.waiting().await`.
- `src/handler.rs`: `RdpilotMcpHandler` (zero-sized, stateless) with an empty `#[tool_router(server_handler)]` impl. Confirmed by reading the published `rmcp-macros` 2.2.0 crate source directly (not `main`-branch docs) that a zero-`#[tool]`-method impl produces a valid `ToolRouter::new()` with no routes and a working macro-generated `ServerHandler`/`get_info`/`call_tool`/`list_tools`/`get_tool`.

**Task 2 — Bounded-timeout transport + timeout constants + error taxonomy** (commit `d8d74db`):
- `src/connect.rs`: `open_stream()`/`round_trip()` relocated verbatim in shape from `rdpilot-cli::connect` (resolve `socket_path()`, resolve the sibling `rdpilot-daemon` binary via `current_exe().with_file_name(DAEMON_BINARY_NAME)`, `connect_or_spawn` a FRESH `UnixStream` per call). `round_trip_bounded(req, bound)` wraps the round trip via a private `timeout_wrap` helper (`tokio::time::timeout(bound, fut)`, mapping `Elapsed` to `McpError::Timeout(bound)`).
- `src/timeouts.rs`: `FAST` (15s — perception/input verbs), `CONNECT` (120s — RDP+TLS handshake), `LIFECYCLE` (10s — daemon-local bookkeeping), `TRANSFER` (300s — file transfer), each documented with its rationale.
- `src/error.rs`: `McpError` (`Wire(WireError)`, `Timeout(Duration)`, `DaemonUnreachable(String)`, `Transport(String)`) + `impl From<McpError> for rmcp::ErrorData` using `ErrorData::internal_error(message, Some(json!({"code": discriminant})))` — the discriminant mirrors `rdpilot-cli::exit_codes::code_str_for`'s kebab-case vocabulary exactly.

## Verification

- `cargo build -p rdpilot-mcp` — clean, zero warnings.
- `cargo test -p rdpilot-mcp` — 7/7 passed:
  - `connect::tests::timeout_wrap_maps_a_pending_future_to_mcp_error_timeout` — proves `Duration::ZERO` against a genuinely pending future maps to `McpError::Timeout` (the plan's `<behavior>` spec, tested directly against the exact wrapper `round_trip_bounded` uses).
  - `connect::tests::timeout_wrap_passes_through_a_fast_ready_future` — a ready future is passed through unchanged.
  - `error::tests::wire_error_renders_both_code_and_message_in_the_rmcp_error` — proves `McpError::from(WireError{SessionNotFound,...})` renders an `rmcp::ErrorData` whose `message` contains the text and whose `data.code` is `"session-not-found"` (D-28).
  - `error::tests::timeout_renders_a_clearly_worded_message_naming_the_elapsed_bound`, `error::tests::client_local_classes_render_legibly`.
  - `timeouts::tests::the_four_verb_class_bounds_are_strictly_ordered_lifecycle_fast_connect_transfer`.
  - `handler::tests::handler_reports_tools_capability_even_with_zero_registered_tools` — the macro-generated `get_info()` reports the `tools` capability even with zero routes.
- `cargo clippy -p rdpilot-mcp --target x86_64-unknown-linux-gnu --all-targets` — clean (0 warnings). Every intentionally-unused-for-now `pub` item (this plan's transport/timeout API has no `#[tool]` caller yet) carries an explicit `#[allow(dead_code)]` with a comment pointing at Plans 14-03/14-04 as the consumer — mirrors `rdpilot-cli::exit_codes::CliError::NoClobber`'s "declared now, consumed later" precedent, so the build stays warning-free instead of accumulating unexplained dead code.
- **Thin-client invariant gate (D-17, BLOCKING):** `cargo tree -p rdpilot-mcp --target x86_64-unknown-linux-gnu | grep -E 'ironrdp|rustls|rdpilot-daemon|^rdpilot v| rdpilot v'` — matched nothing (grep exit 1 = clean, as the plan's `<verification>` specifies). `grep -i interprocess crates/rdpilot-mcp/Cargo.toml` — confirmed absent as a direct dependency.
- `cargo build --workspace --target x86_64-unknown-linux-gnu` — green (rdpilot-mcp, rdpilot-cli, rdpilot-daemon, rdpilot, rdpilot-ipc, rdpilot-config all compiled). Run AFTER both this plan's commits landed; by that point Plan 14-01 (concurrent) had already completed and committed (`208ae88 docs(14-01): complete upstream wire/daemon extensions plan`), so no transient cross-plan race was actually observed — the workspace build's only warning (`unused import: crate::error::Error` in `crates/rdpilot/src/input.rs`) predates/belongs to a different crate this plan does not touch.

## Toolchain Note

Built and tested via `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> --target x86_64-unknown-linux-gnu` (the repo-pinned `x86_64-pc-windows-gnu` cross toolchain is not installed on this host). This is the standard substitute-target convention already established by Plan 14-01 and prior phases.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 — blocking, compile fix] `main.rs`'s error return type**
- **Found during:** Task 1 (writing `main.rs` per the research's `calculator_stdio.rs`-derived Pattern 1, which used `anyhow::Result<()>`).
- **Issue:** `anyhow` is not in the plan's approved dependency list (rmcp/schemars/rdpilot-ipc/rdpilot-config/tokio/serde/serde_json/thiserror/tracing/tracing-subscriber only) and adding it would be an unapproved dependency.
- **Fix:** Used `Result<(), Box<dyn std::error::Error>>` instead — both `.serve()`'s `ServerInitializeError` and `.waiting()`'s `tokio::task::JoinError` implement `std::error::Error`, so `?` converts cleanly without adding a dependency.
- **Files modified:** `crates/rdpilot-mcp/src/main.rs`
- **Commit:** `45c4406`

**2. [Rule 2 — quality] `#[allow(dead_code)]` annotations on the not-yet-consumed transport/timeout API**
- **Found during:** Task 2 (`cargo build -p rdpilot-mcp` produced 9 `never used` warnings for every `pub` item in `connect.rs`/`timeouts.rs` — expected, since this plan registers zero tools).
- **Issue:** An unannotated warning-emitting build doesn't match this codebase's existing convention for "declared now, consumed by a later plan" code (see `rdpilot-cli::exit_codes::CliError::NoClobber`).
- **Fix:** Added `#[allow(dead_code)]` with an explanatory comment on each currently-unreferenced item (`DAEMON_BINARY_NAME`, `open_stream`, `round_trip`, `timeout_wrap`, `round_trip_bounded`, and the four `timeouts` constants), pointing at Plans 14-03/14-04 as the consumer.
- **Files modified:** `crates/rdpilot-mcp/src/connect.rs`, `crates/rdpilot-mcp/src/timeouts.rs`
- **Commit:** `d8d74db`

**3. [Rule 1 — bug fix] `clippy::expect_used` violation in a test**
- **Found during:** Task 2 (`cargo clippy --all-targets` after adding the `error.rs` unit tests).
- **Issue:** `rmcp_err.data.expect(...)` in `wire_error_renders_both_code_and_message_in_the_rmcp_error` violated the crate-root `#![deny(clippy::expect_used)]`, which applies to test code too.
- **Fix:** Replaced with a `match ... { Some(data) => assert_eq!(...), None => panic!(...) }` pattern (mirrors `rdpilot-ipc`'s own test convention of avoiding `.unwrap()`/`.expect()`).
- **Files modified:** `crates/rdpilot-mcp/src/error.rs`
- **Commit:** `d8d74db`

No other deviations. Plan executed as written otherwise.

## Known Stubs

None — this is an intentional zero-tool scaffold per the plan's own `<objective>` ("Output: a compiling, startable MCP server with zero tools yet"), not a stub. Plans 14-03/14-04 add the `computer` mega-tool and the 11 `rdpilot_*` native tools respectively.

## Threat Flags

None — every threat register entry in the plan's `<threat_model>` (T-14-04 stdout corruption, T-14-05 hung round-trip DoS, T-14-06 thin-client info-disclosure, T-14-SC package legitimacy) is directly mitigated by this plan's own deliverables (stderr-only logging + `deny(print_stdout)`; `round_trip_bounded`'s explicit timeout; the verified thin dependency set + `cargo tree` gate; pre-verified package pins) and verified above. No new unmitigated surface introduced.

## Self-Check: PASSED

Files verified present:
- `crates/rdpilot-mcp/Cargo.toml` — FOUND
- `crates/rdpilot-mcp/src/main.rs` — FOUND
- `crates/rdpilot-mcp/src/handler.rs` — FOUND
- `crates/rdpilot-mcp/src/connect.rs` — FOUND
- `crates/rdpilot-mcp/src/timeouts.rs` — FOUND
- `crates/rdpilot-mcp/src/error.rs` — FOUND

Commits verified present in `git log --oneline`:
- `45c4406 feat(14-02): scaffold rdpilot-mcp crate + rmcp stdio server bootstrap` — FOUND
- `d8d74db feat(14-02): bounded-timeout daemon transport + timeout constants + McpError` — FOUND
