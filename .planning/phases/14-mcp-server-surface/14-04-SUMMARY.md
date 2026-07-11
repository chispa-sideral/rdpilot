---
phase: 14-mcp-server-surface
plan: 04
subsystem: mcp
tags: [rmcp, mcp, tool-router, file-transfer, session-targeting, trust-model]

# Dependency graph
requires:
  - phase: 14-02
    provides: rdpilot-mcp crate scaffold, round_trip_bounded, timeouts, McpError, empty RdpilotMcpHandler
  - phase: 14-03
    provides: the computer mega-tool, scale_to_native, the first #[tool_router(server_handler)] block on handler.rs
provides:
  - The eleven rdpilot_* native MCP tools (MCP-03), each a 1:1 wrapper over one rdpilot-ipc wire verb
  - A second #[tool_router] block (native_tool_router) summed with computer_tool_router via ToolRouter's Add impl
  - McpConnectParams (D-27 key vocabulary) -> ResolvedConfig override layer for rdpilot_connect
  - Metadata-only put/get rendering ({path, bytes_transferred, checksum}), MCP-05, with a planted-sentinel regression test
  - crates/rdpilot-mcp/README.md documenting the trust model, session-targeting, and metadata-only transfer
  - tests/tool_schema.rs: MCP-01 offline tools/list schema assertion (12 tools, session-required except 2 exceptions)
affects: [14-05, phase-15-proof]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Two #[tool_router] blocks (custom router names) summed via ToolRouter<Self>'s Add impl, wired through an explicit #[tool_handler(router = (A + B))] block -- required because #[tool_router(server_handler)] only sums a single block's own router"
    - "Pure render_* functions (WireResponse -> Result<CallToolResult, McpError>) factored out of each tool's daemon round-trip, directly unit-testable against hand-built WireResponse fixtures with no daemon"

key-files:
  created:
    - crates/rdpilot-mcp/src/native_tools.rs
    - crates/rdpilot-mcp/src/config_params.rs
    - crates/rdpilot-mcp/tests/tool_schema.rs
    - crates/rdpilot-mcp/README.md
  modified:
    - crates/rdpilot-mcp/src/handler.rs
    - crates/rdpilot-mcp/src/main.rs

key-decisions:
  - "rdpilot_connect and rdpilot_list carry NO session parameter (deviation from the plan's literal blanket 'every tool requires session, including computer' wording) -- they map onto Request::Connect/Request::List, the two wire verbs rdpilot-ipc's own SessionScoped::session() returns None for by design (SESSION-01/03). Adding an unused session field to either schema would not map onto anything on the wire."
  - "The router-summation attribute expression must be parenthesized: #[tool_handler(router = (Self::a() + Self::b()))] -- without parens, Rust's postfix-binds-tighter-than-+ precedence attaches the macro's trailing .call()/.list_all()/.get() only to the right operand, producing 6 cascading E0308 errors."
  - "put/get's 'path' field in the {path, bytes_transferred, checksum} tool result is the caller-supplied local_path, echoed back as addressing metadata -- never read as file content."
  - "No BACKLOG.md convention exists in this repo; the required 'named backlog item for a future server-side transfer sandbox' is documented directly in the README's Trust Model section, referencing research Open Question 2."

patterns-established:
  - "Multi-file #[tool_router] composition: crate::handler owns the combining #[tool_handler(router = ...)] block; crate::native_tools owns a second, independently-named router (native_tool_router, vis = pub(crate)) so a later plan can add a third file's worth of tools the same way."

requirements-completed: [MCP-01, MCP-03, MCP-05]

# Metrics
duration: 45min
completed: 2026-07-11
---

# Phase 14 Plan 04: Native MCP Tool Surface Summary

**Eleven `rdpilot_*` native MCP tools wired 1:1 onto rdpilot-ipc wire verbs via a second summed `#[tool_router]` block, with structurally metadata-only file transfer and an offline `tools/list` schema test proving the full dual (computer + native) surface.**

## Performance

- **Duration:** ~45 min
- **Completed:** 2026-07-11
- **Tasks:** 3/3 completed
- **Files modified:** 6 (4 created, 2 modified)

## Accomplishments

- All eleven `rdpilot_*` tools (`rdpilot_world_state`, `rdpilot_uia`, `rdpilot_window_list`, `rdpilot_process_list`, `rdpilot_launch`, `rdpilot_foreground`, `rdpilot_connect`, `rdpilot_list`, `rdpilot_disconnect`, `rdpilot_put`, `rdpilot_get`) registered and routed through `round_trip_bounded` with the correct timeout class (FAST for perception/input, CONNECT for connect, LIFECYCLE for list/disconnect, TRANSFER for put/get).
- MCP-05 proven at both the type level (`TransferOutcome` structurally has no byte field) and via a planted-sentinel regression test that plants a real file on disk and asserts the sentinel never appears in the rendered tool result.
- MCP-01 proven offline: `tests/tool_schema.rs` constructs the handler's combined tool router in-process (no daemon, no live MCP host) and asserts the advertised set is exactly `computer` + the 11 native tools, with `session` required on every tool except the two structurally session-less ones.
- `crates/rdpilot-mcp/README.md` documents the no-sandbox trust model (developer decision, T-14-12), the session-targeting model (D-29 plus its two exceptions), and the metadata-only transfer guarantee, plus a backlog reference for a future server-side transfer sandbox.
- `cargo tree -p rdpilot-mcp` remains free of `ironrdp`/`rustls`/`rdpilot`/`rdpilot-daemon`/`interprocess` — thin-client invariant preserved.

## Task Commits

1. **Task 1+2: Native tools (perception/input + session lifecycle/file transfer)** - `6793f62` (feat)
2. **Task 2 (trust-model doc)** - `16196f0` (docs)
3. **Task 3: tools/list schema test (MCP-01)** - `358b87e` (test)

**Plan metadata:** (this commit, docs: complete plan)

_Note: Tasks 1 and 2 were authored together (both touch `native_tools.rs`/`handler.rs`) and committed as one `feat` commit, since the two files' content interleaves rather than cleanly separating by task boundary; the trust-model README addition from Task 2 was split into its own `docs` commit._

## Files Created/Modified

- `crates/rdpilot-mcp/src/native_tools.rs` - 11 `rdpilot_*` tool input structs + `#[tool]` handlers, pure `render_*`/`build_*` helpers (directly unit-tested against hand-built `WireResponse` fixtures), the MCP-05 planted-sentinel test
- `crates/rdpilot-mcp/src/config_params.rs` - `McpConnectParams` (D-27 keys verbatim) → `ResolvedConfig` override layer, mirrors `rdpilot-cli::config_flags::ConfigFlags::into_overrides`
- `crates/rdpilot-mcp/src/handler.rs` - split the single `#[tool_router(server_handler)]` block into two named routers (`computer_tool_router`, `native_tool_router`) summed via an explicit `#[tool_handler(router = (... + ...))]` block
- `crates/rdpilot-mcp/src/main.rs` - added `mod config_params;` / `mod native_tools;`
- `crates/rdpilot-mcp/README.md` - trust model, session-targeting, metadata-only transfer documentation (new file)
- `crates/rdpilot-mcp/tests/tool_schema.rs` - MCP-01 offline `tools/list` schema test (new file)

## Decisions Made

- **`rdpilot_connect`/`rdpilot_list` carry no `session` parameter.** The plan's `<must_haves>` and `<threat_model>` sections state "every tool ... including computer" requires session (D-29), but Task 1/2's own `<action>` text never lists a `session` field for connect/list, and `Request::Connect`/`Request::List` are exactly the two wire verbs `rdpilot-ipc`'s `SessionScoped::session()` itself returns `None` for (SESSION-01/03) — a compile-time-enforced fact in a crate this plan is not permitted to modify. Forcing a `session` field onto either tool's schema would be a field with no wire destination, contradicting the plan's own "1:1 onto a wire Request" framing. Resolved by following the more specific, operational Task 1/2 action text and the underlying locked wire model over the summary-level blanket wording; `tests/tool_schema.rs` asserts this two-exception model explicitly rather than a blanket "every tool" rule.
- **`put`/`get`'s `path` field is the caller-supplied `local_path`**, echoed back as addressing metadata (never read as file content) — `TransferOutcome` itself carries no path field, only `bytes_transferred`/`checksum`.
- **Router-summation parenthesization.** `#[tool_handler(router = Self::a() + Self::b())]` (no parens) produced 6 cascading `E0308` errors, because the macro splices `#router` directly before `.call(tcc).await`/`.list_all()`/`.get(name)` — without parens, method-call precedence binds those postfix calls only to the RIGHT operand of `+`. Fixed with explicit parens: `router = (Self::computer_tool_router() + Self::native_tool_router())`.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Toolchain substitution (offline environment)**

- **Found during:** Setup, before Task 1
- **Issue:** The workspace's `rust-toolchain.toml` pins `stable-x86_64-pc-windows-gnu` (the project's real target: an ARM64 Windows dev machine cross-compiling via MinGW). This execution environment is Linux with no Windows toolchain/linker.
- **Fix:** Built/tested against the native Linux substitute target throughout, per the prompt's explicit instruction: `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --target x86_64-unknown-linux-gnu`.
- **Files modified:** None (build invocation only).
- **Verification:** `cargo build --workspace` and `cargo test -p rdpilot-mcp` both green under the substitute target; `cargo tree -p rdpilot-mcp` also run under it.

**2. [Rule 1 - Bug] Let-chains not available on this crate's edition**

- **Found during:** Task 3, first compile of `tests/tool_schema.rs`
- **Issue:** `if let Some(...) = ... && ...` (a let-chain) is a Rust 2024 feature; `rdpilot-mcp` is edition 2021.
- **Fix:** Rewrote both recursive schema-walking helpers (`schema_requires_field`, `schema_declares_property`) to use a `match` + `||` instead of a let-chain.
- **Files modified:** `crates/rdpilot-mcp/tests/tool_schema.rs`
- **Verification:** `cargo test -p rdpilot-mcp --test tool_schema` compiles and all 4 schema-shape tests pass.

**3. [Rule 1 - Bug] Duplicate import in a hand-authored test module**

- **Found during:** Task 1/2, first compile of `native_tools.rs`
- **Issue:** A copy-paste artifact left `use rdpilot_ipc::{WireError, WireErrorCode, WireRect, WireRect as _};` (duplicate `WireRect` import) and a redundant `use std::str::FromStr;` inside the `#[cfg(test)] mod tests` block (already brought in via `use super::*;`).
- **Fix:** Removed both duplicates before the first build.
- **Files modified:** `crates/rdpilot-mcp/src/native_tools.rs`
- **Verification:** Compiles clean; `cargo clippy -p rdpilot-mcp --all-targets` reports zero warnings.

**No architectural deviations (Rule 4) were needed.**

## Known Stubs

None. Every tool's `_impl`/`render_*` function is fully wired to a real `Request`/`WireResponse` pair; no hardcoded empty/placeholder data flows to any tool result.

## Threat Flags

None — every threat surface this plan introduces (`rdpilot_put`'s local-path read, `rdpilot_connect`'s credential passthrough) is already named and dispositioned in the plan's own `<threat_model>` (T-14-11 through T-14-14), and the mitigations described there (planted-sentinel test, D-29 session-required schema, README trust-model doc, put-description risk note) are all implemented as specified.

## Verification Results

- `cargo build --workspace --target x86_64-unknown-linux-gnu` (via `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu`): green (one pre-existing unrelated warning in `rdpilot::input`, out of this plan's scope).
- `cargo test -p rdpilot-mcp --target x86_64-unknown-linux-gnu`: 58 unit tests + 14 (`scale_to_native.rs`) + 62 (`tests/tool_schema.rs`, includes the 58 unit tests re-pulled in via `#[path]` plus 4 new schema tests) all pass, 0 failed.
- `cargo clippy -p rdpilot-mcp --all-targets --target x86_64-unknown-linux-gnu`: zero warnings.
- `cargo tree -p rdpilot-mcp`: confirmed no `ironrdp`/`rustls`/`rdpilot`/`rdpilot-daemon`/`interprocess` in the dependency graph.
- `cargo fmt -p rdpilot-mcp -- --check`: reports diffs, but identically against files this plan did NOT touch (`computer/dispatch.rs`, `computer/mod.rs`, `connect.rs`, `error.rs` — all pre-existing from Plans 14-02/14-03). This environment's local `rustfmt` disagrees with the project's actual formatting baseline (likely a version/config mismatch versus the Windows dev machine this project is normally built on, per `rust-toolchain.toml`), not a regression introduced by this plan. Not auto-fixed (out of scope per the deviation rules' scope boundary — would have reformatted files outside this task's `<files>` list).

## MCP-01 / MCP-03 / MCP-05 State

- **MCP-01** (tools/list surface): Complete — proven offline by `tests/tool_schema.rs`; marked complete in REQUIREMENTS.md.
- **MCP-03** (native tool set): Complete — all 11 tools registered and unit-tested; marked complete in REQUIREMENTS.md.
- **MCP-05** (metadata-only transfer): Complete — type-level guarantee (`TransferOutcome` has no byte field) plus a passing planted-bytes regression test; marked complete in REQUIREMENTS.md.
- MCP-06 (non-blocking isolation) remains Pending — out of this plan's scope (`round_trip_bounded`'s timeout wrapper is already built in 14-02; the dedicated `non_blocking.rs` BLOCKING soak test is a different plan's deliverable per the research's Recommended Project Structure).

## Self-Check: PASSED

- FOUND: `crates/rdpilot-mcp/src/native_tools.rs`
- FOUND: `crates/rdpilot-mcp/src/config_params.rs`
- FOUND: `crates/rdpilot-mcp/tests/tool_schema.rs`
- FOUND: `crates/rdpilot-mcp/README.md`
- FOUND: commit `6793f62` (feat: 11 native tools + combined router)
- FOUND: commit `16196f0` (docs: trust-model README)
- FOUND: commit `358b87e` (test: tools/list schema test)
