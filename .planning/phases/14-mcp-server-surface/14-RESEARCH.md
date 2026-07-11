# Phase 14: MCP Server Surface - Research

**Researched:** 2026-07-11
**Domain:** Rust MCP server (rmcp 2.x) over stdio, exposing an Anthropic computer-use-compatible mega-tool + rdpilot-native tools as a thin `rdpilot-ipc` client of the Phase 12 daemon.
**Confidence:** HIGH (rmcp API + concurrency model verified against live crates.io/docs.rs/GitHub source; Anthropic computer-use schema verified against the live current docs; daemon concurrency model verified by reading the actual Phase 12/13 source)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-14.1 — MCP transport: stdio, host-spawned per client (rmcp 2.x).**
  The MCP server speaks stdio and is spawned per-client by the MCP host (registered in the client's `mcpServers` block, e.g. Claude Desktop/Code config). It uses `rmcp` 2.x (the SDK pinned in D-26). Per-client stdio lifetime is acceptable precisely because the **daemon** (D-17) — not the MCP server — owns the long-lived RDP sessions and keepalive: sessions survive MCP-server restarts, so the MCP server is a stateless-per-process, restart-safe `rdpilot-ipc` client.
  **Rationale:** stdio is the conventional MCP host integration path and needs no port/DACL story of its own; the daemon already provides session durability across MCP-server process churn.
  **Rejected:** HTTP/SSE transport (adds a listening port + auth surface with no benefit for a locally-host-spawned server); the MCP server holding sessions itself (would duplicate the daemon and lose sessions on every client restart — violates D-17).

- **D-14.2 — `computer` tool spec: fixed 1280x800 (WXGA) advertised resolution + `computer_20250124` action set + prefixed native tools.**
  The `computer` mega-tool advertises a **FIXED 1280x800 (WXGA)** logical resolution as the downscale target that models see and click within — chosen for best model click accuracy (Anthropic guidance: keep the advertised resolution modest for coordinate precision). A single tested `scale_to_native(x, y)` bridges every model-supplied 1280x800 coordinate to the 96-DPI native remote desktop pixel space (satisfies MCP-04 / Success Criterion 3; the one-fixed-resolution + single-bridge shape is LOCKED). Implement the Anthropic **`computer_20250124`** action set — the schema-discriminated action superset that adds `scroll`, `triple_click`, `hold_key`, and `wait` on top of the base mouse/keyboard/screenshot actions — each action mapping onto the existing SDK input/capture verbs.
  rdpilot-native (non-`computer`) tools are **PREFIXED** with `rdpilot_` (e.g. `rdpilot_world_state`, `rdpilot_uia_tree`, `rdpilot_window_list`, `rdpilot_process_list`, `rdpilot_launch`, `rdpilot_foreground`, `rdpilot_connect`, `rdpilot_list`, `rdpilot_disconnect`, `rdpilot_put`, `rdpilot_get`) to avoid name collisions with the `computer` tool and with any other server the host has loaded.
  **Rationale:** matches Anthropic's own single-computer-tool design (D-21) and the `computer_20250124` tool version; a fixed advertised resolution keeps the coordinate bridge a single tested function rather than a per-session negotiation; a namespace prefix prevents tool-name collisions in multi-server MCP hosts.
  **Rejected:** dynamic/per-session advertised resolution; the older `computer_20241022` action set (lacks `scroll`/`triple_click`/`hold_key`/`wait`); unprefixed native tool names.

### Claude's Discretion

- Exact rmcp 2.x server scaffolding shape (handler struct, `#[tool]`/router macro usage vs. manual tool registration) — follow the current rmcp 2.x idiom; keep tool schemas serde-derived.
- Internal representation of the `computer` action discriminant (single serde-tagged enum over the `computer_20250124` actions vs. a dispatch table) — keep it a single schema-discriminated tool per D-21, one action enum.
- Precise per-call timeout values for the bounded-timeout requirement (MCP-06) — researcher/planner may choose concrete numbers per verb class (fast perception/input vs. slow file-transfer/launch-wait), so long as every tool call has an explicit bound and slow calls cannot starve fast ones.
- Where `scale_to_native` lives (dedicated module vs. inline in the computer-tool handler) — keep it ONE function with a direct unit test hitting edges/corners (MCP-04 is BLOCKING).

### Deferred Ideas (OUT OF SCOPE)

- MCP progress notifications (`notifications/progress`) for long file transfers — Future Requirements (deferred); MCP-06 is satisfied by per-call task isolation + bounded timeouts, not by streaming progress.
- Scripted MCP proof harness (PROOF-03) and live-LLM capstone (PROOF-04) — Phase 15; this phase builds the tools those harnesses exercise.
- HTTP/SSE MCP transport — rejected in D-14.1; stdio only for v1.1.

### Inherited / Cross-Cutting Decisions (do NOT re-open)

- **D-17:** CLI and MCP server are BOTH thin clients of the single long-lived daemon over local IPC.
- **D-21:** MCP presents ONE schema-discriminated `computer` tool PLUS a small rdpilot-native tool set — never 25+ one-per-action tools.
- **D-22 / MCP-05:** MCP file put/get operate on local disk paths and return `{path, size/bytes_transferred, checksum}` metadata — **never inline file bytes**.
- **D-26 (Stack):** rmcp 2.2.0, interprocess 2.4.2, clap 4.6.1, config 0.15.25.
- **D-27 (Config):** MCP-init parameter keys reuse the EXACT SAME key spellings as `config.toml`/env/CLI flags (host, port, username, password, domain, cert-bypass, …).
- **D-28 (Errors):** `WireError { code, message }` — the MCP server renders both fields as the tool error result.
- **D-29 (Session):** No implicit default session. **The MCP server requires a `session` parameter on EVERY tool call — per-call, NOT a one-session-per-server init binding.**
- **D-30 (Status vocabulary):** `Connecting` / `Live` / `Reconnecting` / `Disconnected` (+ `Orphaned`, Phase 12) rendered verbatim by `rdpilot_list`.

</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MCP-01 | An `rmcp`-based MCP server exposes rdpilot over MCP to any MCP client | §Standard Stack (rmcp 2.2.0 confirmed current), §Architecture Patterns (stdio server scaffold code shape) |
| MCP-02 | MCP exposes a single Anthropic computer-use-compatible `computer` tool (screenshot + action-discriminated mouse/keyboard/scroll) mapping onto the SDK input/capture verbs | §Computer-Use Action → IPC Verb Mapping (full action table verified against live Anthropic docs + reference implementation) |
| MCP-03 | MCP exposes rdpilot-native tools (world_state, UIA, window/process list, launch, foreground, session connect/list/disconnect, file put/get) as MCP Tools | §Architecture Patterns (native tool set + `Request`/`WireResponse` verb inventory already built by Phase 11-13) |
| MCP-04 [BLOCKING] | The computer-use surface bridges rdpilot's 96-DPI physical-pixel coordinates to the tool's expected scaled screenshot/coordinate space | §Coordinate Bridge: `scale_to_native` (pure function, exact math, edge/corner rounding discipline, native-dimension sourcing design) |
| MCP-05 | MCP file put/get operate on local disk paths and return path/size/checksum metadata (never inline file bytes) | §Don't Hand-Roll (confirms `WireResponse::Transfer(TransferOutcome)` already carries zero file bytes — direct passthrough, no new work) |
| MCP-06 [BLOCKING] | Slow RDP round-trips (file transfer, launch waits) do not block the MCP transport event loop | §Non-Blocking Isolation Design (verified: rmcp already spawns a task per inbound request; daemon already isolates per-session; remaining work is per-tool explicit timeout wrapping) |

</phase_requirements>

## Summary

Phase 14 composes three ALREADY-EXISTING pieces rather than inventing new concurrency or transport machinery: (1) `rmcp` 2.2.0's stdio server, which — verified directly from its `service.rs` source — spawns an independent tokio task per inbound JSON-RPC request before dispatch, so the transport read loop is never blocked by a slow tool handler; (2) the Phase 12 daemon's `Registry::call`, which — verified directly from `registry.rs` — clones a per-session `Arc<tokio::sync::Mutex<...>>` and drops the registry's outer lock before awaiting, so calls to different sessions never block each other (same-session calls intentionally serialize, matching a single RDP input stream); and (3) the exact same one-shot connect/round-trip/disconnect transport primitives (`rdpilot_ipc::transport::{connect_or_spawn, round_trip pattern, read_frame, write_frame, socket_path}`) the Phase 13 CLI already proved end-to-end. The MCP server therefore needs almost no novel plumbing for MCP-06 beyond wrapping each tool's daemon round-trip in an explicit `tokio::time::timeout` with a per-verb-class bound (fast perception/input vs. slow transfer/launch/connect) — the isolation itself is a byproduct of composing the two existing layers correctly, not something the MCP crate must build.

The Anthropic `computer_20250124` action set (locked by D-14.2) is fully documented and stable: 16 actions total (10 from the base `computer_20241022` set — `key`, `type`, `mouse_move`, `left_click`, `left_click_drag`, `right_click`, `middle_click`, `double_click`, `screenshot`, `cursor_position` — plus 6 additions — `left_mouse_down`, `left_mouse_up`, `scroll`, `hold_key`, `wait`, `triple_click`). Every action maps onto an existing `rdpilot-ipc` wire verb EXCEPT three genuine gaps that must be handled explicitly rather than silently dropped: `cursor_position` (no SDK pointer-position getter — must be client-side-tracked), `left_mouse_down`/`left_mouse_up` (no half-click primitive in `WireMouseAction` — must be rejected as unsupported, with `left_click_drag` as the documented alternative), and `scroll_direction: left/right` (rdpilot's `Scroll` wire verb only carries vertical `dy` — horizontal scroll is unsupported). `triple_click` has no atomic wire primitive either, but is fully achievable as three sequenced `Click` round trips at ~100ms spacing (matching the SDK's own internal `DoubleClick` gap).

MCP-04's coordinate bridge is a pure, five-argument function (`scale_to_native(x, y, native_w, native_h) -> (u16, u16)`) that the planner can unit-test directly against edges/corners without a live session. The one genuinely open design question this research surfaces (see Open Questions) is HOW the MCP server learns the session's current native desktop dimensions at call time, since neither `WireResponse` nor `SessionStatus` currently carries them — this research recommends a small `Request::DesktopSize`/`WireResponse::DesktopSize` wire addition (mirroring the exhaustive-match-forcing-function pattern already established in `rdpilot-ipc`) over the fragile alternative of sniffing PNG header dimensions from the last screenshot.

**Primary recommendation:** Build `rdpilot-mcp` as a fourth thin `rdpilot-ipc`/`rdpilot-config`-only crate (never `rdpilot`/`rdpilot-daemon`), reusing Phase 13's exact connect/round-trip pattern per tool call, wrapped in `tokio::time::timeout`; implement `computer` as one `#[tool]`-macro method taking a single serde-tagged action enum (`Parameters<ComputerAction>`); add one small `DesktopSize` wire verb to `rdpilot-ipc`/`rdpilot-daemon` to feed `scale_to_native`; explicitly reject the three unsupported computer-use actions with a clear `WireError`-shaped message rather than silently misbehaving.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| MCP protocol handshake / tool discovery / JSON-RPC framing | MCP Server (`rdpilot-mcp`, this phase) | — | rmcp owns the wire-level MCP protocol entirely; the crate only supplies tool implementations |
| `computer` action → coordinate scaling | MCP Server | — | Must be a pure function co-located with the tool handler, not pushed into the daemon (daemon has no concept of an "advertised resolution") |
| `computer` action → rdpilot input/capture verb dispatch | MCP Server | Daemon (executes the verb) | The MCP server translates one computer-use action into one-or-more `rdpilot-ipc::Request` values; the daemon is where the verb actually executes against the live `Session` |
| Session lifecycle (connect/list/disconnect) | Daemon (already built, Phase 12) | MCP Server (thin passthrough) | D-17: session state lives ONLY in the daemon; MCP just forwards |
| Per-call concurrency / non-blocking isolation | Both: rmcp (request dispatch) + Daemon (`Registry::call` per-session isolation) | MCP Server (explicit per-tool timeout) | Verified: isolation is a property of composing the two already-existing layers, not new MCP-layer machinery |
| File bytes transfer (local↔remote) | Daemon → SDK (RDPDR/sensor, already built Phase 10) | MCP Server (metadata-only passthrough) | MCP-05: the MCP server never sees file bytes; `WireResponse::Transfer(TransferOutcome)` already carries only `{bytes_transferred, checksum}` |
| Config resolution (host/user/pass/…) | `rdpilot-config` (already built Phase 11) | MCP Server (supplies the MCP-init-parameter override layer) | D-27: MCP-init params are just the third layer of the SAME `resolve()` pipeline the CLI already uses |

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `rmcp` | 2.2.0 `[VERIFIED: crates.io, published 2026-07-08]` | Official Rust MCP SDK — stdio transport, `#[tool_router]`/`#[tool]` macros, `ServerHandler` trait, JSON-RPC dispatch | Official `modelcontextprotocol/rust-sdk` crate; matches D-26's pin exactly; current `max_stable_version` on crates.io is 2.2.0 as of this research |
| `schemars` | `^1.0` (max stable 1.2.1) `[VERIFIED: crates.io]` | Derives JSON Schema for tool input structs (`#[derive(JsonSchema)]`) — required by rmcp's `server` feature | rmcp's own `Cargo.toml` pins `schemars ^1.0` as an optional dependency gated by the `schemars`/`server` features — not a separate stack decision, it is pulled in transitively and must simply be added as a direct dependency for `#[derive]` to resolve |
| `tokio` | `1.x` (workspace-pinned, matches existing crates) | Async runtime | Already the workspace standard (every other crate) |
| `serde` / `serde_json` | `1.x` | Tool input/output (de)serialization | Already the workspace standard |
| `thiserror` | `2` (workspace-pinned) | Owned error types on the MCP crate boundary, mirroring `rdpilot-cli`'s `CliError`/`rdpilot-ipc`'s `WireError` convention | Consistency with every other crate in the workspace |
| `rdpilot-ipc` | path dependency | Wire DTOs + transport (`connect_or_spawn`, `read_frame`, `write_frame`, `socket_path`) | Reused verbatim — the SAME crate the CLI already uses; zero new transport code needed |
| `rdpilot-config` | path dependency | Layered config resolution (`resolve`, `ResolvedConfig`) for MCP-init parameters | Reused verbatim — the SAME crate/function the CLI's `ConfigFlags::into_overrides` pattern already exercises |

### rmcp feature flags (required Cargo.toml shape)

```toml
[dependencies]
rmcp = { version = "2.2.0", features = ["server", "macros", "transport-io"] }
schemars = "1"
```

**Verified via crates.io's published feature manifest for `rmcp` 2.2.0** `[VERIFIED: crates.io API]`:
- `default = ["base64", "macros", "server"]` — `server` and `macros` are ALREADY on by default; only `transport-io` must be added explicitly (it is NOT in `default`).
- `transport-io = ["transport-async-rw", "tokio/io-std"]` — this is what makes `rmcp::transport::stdio()` available. Omitting it is a compile error at the `use rmcp::transport::stdio;` call site.
- `server = ["transport-async-rw", "dep:schemars", "dep:pastey"]` — confirms `schemars` is already pulled in by `server`; declaring it as a direct dependency is still required for the crate's own `#[derive(schemars::JsonSchema)]` usage on tool-input structs to resolve without a path-qualification workaround.

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `base64` | `0.22.1` (already an approved workspace pin, `rdpilot-cli`/`rdpilot-daemon` Cargo.toml) | If the MCP crate ever needs to decode/inspect PNG dimensions itself (see Open Questions on `DesktopSize`) | Reuse the exact existing approved pin — no new legitimacy checkpoint needed (already slopcheck `[OK]`-verified in Plan 13-04/13-06) |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| rmcp stdio + host-spawned-per-client | rmcp HTTP/SSE transport | Rejected by D-14.1 — adds a listening port/auth surface with zero benefit for a local, host-spawned server |
| rmcp `#[tool_router]`/`#[tool]` macros | Hand-rolled `ServerHandler::call_tool`/`list_tools` implementation | The macro path keeps schema derivation (`schemars`) and dispatch in sync automatically; a hand-rolled match avoids the macro but reintroduces exactly the kind of drift-prone hand-maintained dispatch table `rdpilot-ipc`'s exhaustive-match convention exists to prevent. Recommend the macro path (Claude's Discretion per CONTEXT.md, but macros are the current rmcp idiom and lower-risk) |
| One wire `DesktopSize` verb (recommended) | PNG-header-sniffing the last screenshot for native dimensions | See Open Questions — the wire-verb approach is simpler and has no "haven't screenshotted yet" edge case, at the cost of touching `rdpilot-ipc`/`rdpilot-daemon` in this phase |

**Installation:**
```bash
cargo new --lib crates/rdpilot-mcp   # workspace member, mirrors rdpilot-cli's layout
# Cargo.toml:
#   rmcp = { version = "2.2.0", features = ["server", "macros", "transport-io"] }
#   schemars = "1"
#   rdpilot-ipc = { path = "../rdpilot-ipc" }
#   rdpilot-config = { path = "../rdpilot-config" }
#   tokio = { version = "1", features = ["rt-multi-thread", "net", "io-util", "io-std", "macros", "time"] }
#   serde = "1"
#   serde_json = "1"
#   thiserror = "2"
```

**Version verification (2026-07-11):** `curl -s https://crates.io/api/v1/crates/rmcp` confirms `max_stable_version: "2.2.0"`, `created_at: 2026-07-08` for that version (i.e. one day old at research time — verify it is still the pin the workspace wants before locking; it exactly matches D-26's pre-existing pin, so no drift). `interprocess` max stable is `2.4.2`, exactly matching D-26 (this crate is NOT a direct `rdpilot-mcp` dependency — it is used, if at all, only inside `rdpilot-daemon`/`rdpilot-ipc`'s own transport, which the MCP crate reuses without adding the dependency itself). `schemars` max stable is `1.2.1`.

`[ASSUMED]` note: `rmcp`'s exact runtime request-concurrency behavior (per-request task spawn) was confirmed by fetching and reading `crates/rmcp/src/service.rs` from the `main` branch of `modelcontextprotocol/rust-sdk` on GitHub via an AI-summarized fetch, not by compiling and instrumenting the crate directly in this sandbox (no `cargo`/network-crate-download available in this research environment). Tag: `[CITED: github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/src/service.rs]` — MEDIUM-HIGH confidence (official source repo, quoted `tokio::select!`/`spawn_service_task` code), but the planner should confirm this against the exact 2.2.0 tag (not `main`) during Wave 0/implementation, since `main` can be ahead of the last published release.

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|--------------|
| `rmcp` | crates.io | created 2025-03-16, latest ver 2026-07-08 | 15.4M total / 8.0M recent | github.com/modelcontextprotocol/rust-sdk | `[OK]` | Approved (already D-26-pinned) |
| `schemars` | crates.io | long-established (pre-existing widely used crate) | high | github.com/GREsau/schemars | `[OK]` | Approved |
| `pastey` | crates.io | transitive dep of `rmcp`'s `macros`/`server` features, not a direct dependency this phase adds | — | — | `[OK]` | Approved (transitive only — no direct Cargo.toml line needed) |
| `interprocess` | crates.io | — | — | github.com/kotauskas/interprocess | `[OK]` | Not a direct `rdpilot-mcp` dependency this phase (reused only if `rdpilot-ipc`/`rdpilot-daemon` need it — already D-26-pinned at 2.4.2) |

**Packages removed due to slopcheck `[SLOP]` verdict:** none.
**Packages flagged as suspicious `[SUS]`:** none.

slopcheck was available in this research environment (`pip install slopcheck` succeeded) and ran successfully against `crates.io` for every candidate package (`slopcheck scan --pkg crates.io <name> --json`); every check returned `status: "OK"` with no flags. All four package identities above were additionally cross-checked against their `homepage`/`repository` fields via the crates.io API directly (not merely training-data recall), satisfying the `[VERIFIED]` provenance bar for `rmcp` (D-26-pinned + Context7/crates.io-confirmed) and `interprocess` (D-26-pinned + crates.io-confirmed). `schemars` and `pastey` are tagged `[VERIFIED: crates.io registry]` since their identity was read directly from `rmcp`'s own published dependency manifest (an authoritative source — the exact crate that requires them), not recalled from training data.

## Architecture Patterns

### System Architecture Diagram

```
                         MCP Host (Claude Desktop/Code, etc.)
                                     |
                          spawns per-client (stdio, D-14.1)
                                     v
                    +--------------------------------------+
                    |         rdpilot-mcp (this phase)      |
                    |  rmcp::ServerHandler (stdio transport) |
                    |                                        |
                    |  tools/list  -->  [computer,           |
                    |                    rdpilot_world_state, |
                    |                    rdpilot_uia_tree,    |
                    |                    rdpilot_window_list, |
                    |                    ... 11 native tools] |
                    |                                        |
                    |  tools/call(name, args)                |
                    |    |-- rmcp spawns an independent       |
                    |    |   tokio task per call (VERIFIED,   |
                    |    |   service.rs) -- MCP-06 layer 1    |
                    |    v                                    |
                    |  [per-tool handler]                     |
                    |    |-- computer: action enum dispatch   |
                    |    |     -> scale_to_native(x,y,W,H)    |
                    |    |     -> WireMouseAction/WireKeyAction|
                    |    |-- rdpilot_*: 1:1 Request mapping    |
                    |    v                                    |
                    |  tokio::time::timeout(bound, round_trip)|
                    |    -- explicit per-verb-class timeout    |
                    |    -- MCP-06 layer 3                     |
                    +-------------------|--------------------+
                                        | fresh UnixStream per call
                                        | (connect_or_spawn, same as CLI)
                                        v
                    +--------------------------------------+
                    |     rdpilot-daemon (Phase 12, built)   |
                    |  accept loop: spawn_local per           |
                    |  connection -- MCP-06 layer 2 (fan-out) |
                    |                                        |
                    |  dispatch(Request) -> Registry::call    |
                    |    |-- clone per-session Arc<TokioMutex>|
                    |    |-- drop outer lock BEFORE .await     |
                    |    |-- different sessions: parallel      |
                    |    |-- same session: serialize (by design|
                    |    |    -- one RDP input stream)          |
                    +-------------------|--------------------+
                                        v
                              rdpilot::Session (RDP/DVC/RDPDR)
                                        v
                                Remote Windows target
```

A reader can trace: MCP host -> stdio JSON-RPC -> rmcp per-call task -> tool handler -> bounded-timeout IPC round trip -> daemon per-connection task -> daemon per-session lock -> live `Session` -> remote desktop, and the response flows back the same path. Two arrows in this diagram (`spawn_service_task`/rmcp and `spawn_local`/daemon accept loop) are the two INDEPENDENT concurrency layers that jointly satisfy MCP-06 — no third concurrency mechanism needs to be invented inside `rdpilot-mcp` itself, only the explicit per-call `tokio::time::timeout` wrapper.

### Recommended Project Structure

```
crates/rdpilot-mcp/
├── Cargo.toml
├── src/
│   ├── main.rs           # entry point: build handler, .serve(stdio()).await, .waiting().await
│   ├── handler.rs         # the ServerHandler / #[tool_router] impl — one method per tool
│   ├── computer/
│   │   ├── mod.rs         # ComputerAction enum (serde-tagged, computer_20250124 shape)
│   │   ├── scale.rs        # scale_to_native — pure function + its own unit tests (MCP-04 BLOCKING)
│   │   └── dispatch.rs      # ComputerAction -> Request translation (the action/verb mapping table below)
│   ├── native_tools.rs      # the 11 rdpilot_* tool input structs + handlers
│   ├── connect.rs          # relocated-verbatim pattern from rdpilot-cli/src/connect.rs (round_trip + timeout wrapper)
│   ├── timeouts.rs          # per-verb-class Duration constants
│   └── error.rs            # McpError: thiserror wrapper translating WireError -> rmcp::ErrorData
└── tests/
    ├── tool_schema.rs       # tools/list shape, offline (no daemon needed)
    ├── scale_to_native.rs   # MCP-04 BLOCKING edge/corner precision test
    └── non_blocking.rs      # MCP-06 BLOCKING: slow fake call does not starve a concurrent fast one (offline, against the real compiled rdpilot-daemon binary + a slow fake connector, mirroring Plan 12's thread_leak_soak.rs / Plan 13's cli_lifecycle.rs pattern)
```

### Pattern 1: rmcp stdio server bootstrap

**What:** The top-level `main.rs` shape every rmcp stdio server uses.
**When to use:** Always — this is the whole of MCP-01's transport wiring.
**Example:**
```rust
// Source: github.com/modelcontextprotocol/rust-sdk,
// examples/servers/src/calculator_stdio.rs (verified via WebFetch of the raw file)
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr) // MUST be stderr: stdout is the JSON-RPC wire
        .init();

    let service = RdpilotMcpHandler::new().serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
```
**Pitfall guard baked into this pattern:** logging MUST go to stderr, never stdout — stdout is the raw JSON-RPC stream and any stray `println!` corrupts every subsequent frame the host tries to parse.

### Pattern 2: `#[tool_router]`/`#[tool]` macro tool declaration

**What:** Declaring one MCP tool as a plain async method with a serde+schemars-derived input struct.
**When to use:** For every one of the 11 `rdpilot_*` native tools (one method each).
**Example:**
```rust
// Source: github.com/modelcontextprotocol/rust-sdk,
// examples/servers/src/common/calculator.rs (verified via WebFetch)
use rmcp::{tool, tool_router, handler::server::tool::Parameters};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WorldStateArgs {
    /// The session to operate on (D-29: required on every call, no default).
    pub session: String,
    #[serde(default)]
    pub screenshot: bool,
    #[serde(default)]
    pub window_list: bool,
    // ... uia mode fields
}

#[tool_router(server_handler)]
impl RdpilotMcpHandler {
    #[tool(description = "Correlated desktop snapshot: screenshot + window list + optional UIA trees, one batch timestamp (D-8.1).")]
    async fn rdpilot_world_state(
        &self,
        Parameters(args): Parameters<WorldStateArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let session = parse_session(&args.session)?;
        let req = build_world_state_request(session, &args);
        let resp = self.round_trip_bounded(req, timeouts::FAST).await?;
        world_state_to_tool_result(resp)
    }
}
```

### Pattern 3: One-shot per-call daemon round trip (relocated from the CLI, D-14.1's stateless-per-process invariant)

**What:** Exactly the CLI's `connect.rs` `open_stream`/`round_trip` pattern — a fresh `UnixStream` per tool call, never a held-open connection.
**When to use:** Every tool handler, wrapped in an explicit timeout.
**Example:**
```rust
// Adapted verbatim from crates/rdpilot-cli/src/connect.rs (already-proven Phase 13 code)
use rdpilot_ipc::{Request, WireResponse, connect_or_spawn, read_frame, socket_path, write_frame};

pub async fn round_trip_bounded(req: Request, bound: std::time::Duration) -> Result<WireResponse, McpError> {
    tokio::time::timeout(bound, round_trip(req))
        .await
        .map_err(|_elapsed| McpError::Timeout(bound))? // MCP-06: explicit bound, never an indefinite hang
}

async fn round_trip(req: Request) -> Result<WireResponse, McpError> {
    let socket = socket_path().map_err(McpError::daemon_unreachable)?;
    let daemon_exe = std::env::current_exe()
        .map_err(McpError::daemon_unreachable)?
        .with_file_name(DAEMON_BINARY_NAME);
    let mut stream = connect_or_spawn(&socket, &daemon_exe).await.map_err(McpError::daemon_unreachable)?;
    write_frame(&mut stream, &req).await.map_err(McpError::transport)?;
    read_frame(&mut stream).await.map_err(McpError::transport)
}
```
**Why a fresh connection per call, not a shared/pooled connection:** (1) it is the proven Phase 13 pattern — zero new transport risk; (2) it trivially satisfies MCP-06's isolation at the transport layer too — each concurrent tool call gets its OWN socket, so there is no MCP-server-side connection-pool contention to reason about at all; (3) the daemon already `spawn_local`s one task per accepted connection, so N concurrent MCP tool calls become N independent daemon-side tasks for free.

### Anti-Patterns to Avoid

- **Holding one shared `UnixStream` across tool calls in the MCP server:** would reintroduce exactly the head-of-line-blocking MCP-06 forbids — a slow Put/Get would occupy the one shared stream and stall every other call's request/response framing on it. Use a fresh connection per call (Pattern 3).
- **Doing coordinate math inline in the tool handler instead of a standalone pure function:** MCP-04 is BLOCKING and its acceptance test is "a precision click test near screen edges/corners" — this requires `scale_to_native` to be independently unit-testable without a live session or a running MCP server.
- **Silently ignoring an unsupported computer-use action** (`left_mouse_down`/`left_mouse_up`, horizontal `scroll_direction`): return an explicit, clearly-worded tool error instead — a silently-no-op'd action is far worse for an LLM driving the loop than a legible rejection it can reason about and route around.

## Computer-Use Action → IPC Verb Mapping (MCP-01/MCP-02)

The Anthropic **`computer_20250124`** action set (locked, D-14.2) is the base `computer_20241022` set plus six additions `[CITED: platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool, fetched live 2026-07-11]` `[CITED: raw.githubusercontent.com/anthropics/anthropic-quickstarts/main/computer-use-demo/computer_use_demo/tools/computer.py]`:

| Action | Params | rdpilot-ipc Verb | Notes |
|--------|--------|-------------------|-------|
| `screenshot` | none | `Request::Screenshot { session }` | `WireResponse::Screenshot { png_base64 }` is a direct passthrough into the MCP `image` content block — no decode/re-encode needed (base64 PNG both ways) |
| `mouse_move` | `coordinate: [x,y]` | `Request::Mouse { session, action: WireMouseAction::Move { x, y } }` | `(x,y)` scaled via `scale_to_native` first |
| `left_click` | `coordinate`, optional `text` (modifier) | `Request::Mouse { ..., action: WireMouseAction::Click { x, y, button: WireButton::Left } }` | Modifier-key support (shift/ctrl/alt/super held during click) has NO direct `WireMouseAction` equivalent — see Open Questions |
| `right_click` | same | `WireMouseAction::Click { ..., button: WireButton::Right }` | |
| `middle_click` | same | `WireMouseAction::Click { ..., button: WireButton::Middle }` | |
| `double_click` | `coordinate`, optional `text` | `WireMouseAction::DoubleClick { x, y, button }` | SDK already applies `DOUBLE_CLICK_GAP` (100ms) internally — atomic, one round trip |
| `triple_click` | `coordinate`, optional `text` | **No atomic wire primitive** — synthesize as THREE sequential `Request::Mouse{Click}` round trips, ~100ms apart | Non-atomic approximation (3 round trips vs. 1); acceptable since most apps' triple-click detection is itself timer-based |
| `left_click_drag` | `start_coordinate`, `coordinate` | `Request::Mouse { ..., action: WireMouseAction::Drag { from_x, from_y, to_x, to_y, button: Left } }` | Both endpoints scaled independently |
| `left_mouse_down` | `coordinate` | **UNSUPPORTED — GAP** | `WireMouseAction` has no press-only variant. Return a clear tool error; document `left_click_drag` as the supported drag alternative |
| `left_mouse_up` | `coordinate` | **UNSUPPORTED — GAP** | Same as above |
| `key` | `text` (xdotool-style combo, e.g. `"ctrl+s"`, `"alt+Tab"`, `"Return"`) | `Request::Key { session, action: WireKeyAction::Combo(Vec<WireKey>) }` | Requires a `"+"`-split parser mapping xdotool/X11-keysym-style names onto `WireKey` — see Don't Hand-Roll |
| `type` | `text` (literal string) | `Request::Key { session, action: WireKeyAction::Type(text) }` | Direct 1:1 |
| `scroll` | `coordinate`, `scroll_direction` (up/down/left/right), `scroll_amount`, optional `text` | `Request::Mouse { ..., action: WireMouseAction::Scroll { x, y, dy } }` for `up`/`down` only | `left`/`right` is **UNSUPPORTED — GAP** (rdpilot's `Scroll` wire verb carries only vertical `dy`, "Horizontal scroll is deliberately absent, D-3.3"). `dy = scroll_amount * 120 * (direction == up ? 1 : -1)` (WHEEL_DELTA notch convention already used by the SDK) |
| `hold_key` | `text`, `duration` (seconds) | `Request::Key { session, action: WireKeyAction::Combo(...) }` sent, then held via `tokio::time::sleep(duration)`, then a release — **no direct SDK "hold for N seconds" primitive**; `WireKeyAction::Combo` already presses-then-releases atomically in one round trip, so a true multi-second HOLD requires either accepting the atomic press+release as a best-effort approximation, or (if genuinely needed) a small SDK-level extension — flag as scope question, likely acceptable to approximate for v1.1 (most `hold_key` uses in agent-driven UI work are actually satisfied by the atomic combo) |
| `wait` | `duration` (seconds) | No SDK verb at all — purely `tokio::time::sleep(duration)` inside the MCP handler, no daemon round trip | Cap `duration` to something bounded (e.g. 30s) so a malicious/confused caller cannot park an MCP-server task indefinitely — this interacts with MCP-06's own bounded-timeout discipline |
| `cursor_position` | none | **No SDK getter for last pointer position** — GAP, approximate client-side (see Open Questions) | rdpilot never tracks "current mouse position" as session state; only sends absolute-move/click PDUs |

**Gap summary (flag prominently for the planner):**
1. `cursor_position` — no SDK state to read; needs an MCP-server-local approximation (last coordinate this MCP server itself sent, session-scoped) or an explicit "not supported, always screenshot to see the pointer" rejection.
2. `left_mouse_down`/`left_mouse_up` — no SDK primitive; reject explicitly, document `left_click_drag` as the supported alternative.
3. `scroll_direction: left`/`right` — no SDK primitive (D-3.3 explicitly deferred horizontal scroll); reject explicitly.
4. `hold_key`'s multi-second HOLD semantics have no atomic SDK equivalent — approximate as an atomic combo press+release (functionally equivalent for the vast majority of UI-automation uses of a held key, e.g. accessibility "sticky" behaviors are rare) or flag as a documented limitation.

None of these four gaps are BLOCKING success criteria (MCP-04/MCP-06 are the two `[BLOCKING]` items and neither depends on these actions) — they are correctness/completeness gaps the planner should turn into explicit, tested rejection paths rather than silent no-ops, and should be called out in the tool's own MCP description string so a driving LLM understands the boundary.

## Coordinate Bridge: `scale_to_native` (MCP-04, BLOCKING)

### The pure function

```rust
/// The FIXED advertised resolution every model-supplied computer-use
/// coordinate is expressed in (D-14.2, LOCKED).
pub const ADVERTISED_WIDTH: u32 = 1280;
pub const ADVERTISED_HEIGHT: u32 = 800;

/// Pure, unit-testable bridge from the FIXED 1280x800 advertised space to
/// rdpilot's native 96-DPI physical-pixel desktop space (MCP-04, BLOCKING).
///
/// Rounds to the nearest native pixel (not truncation) for best fidelity,
/// then clamps into `[0, native_w-1] x [0, native_h-1]` — the same
/// half-open convention `Session::check_bounds` enforces server-side
/// (`x >= w || y >= h` is rejected) — so a boundary/edge coordinate a model
/// emits (e.g. exactly `x == 1280`, the exclusive right edge) can NEVER
/// produce a value `check_bounds` would reject, satisfying "precision click
/// test near screen edges/corners" without relying on the SDK's own
/// rejection path as a silent safety net.
pub fn scale_to_native(x: u32, y: u32, native_w: u32, native_h: u32) -> (u16, u16) {
    let scale_x = f64::from(native_w) / f64::from(ADVERTISED_WIDTH);
    let scale_y = f64::from(native_h) / f64::from(ADVERTISED_HEIGHT);

    let nx = (f64::from(x) * scale_x).round().clamp(0.0, f64::from(native_w.saturating_sub(1)));
    let ny = (f64::from(y) * scale_y).round().clamp(0.0, f64::from(native_h.saturating_sub(1)));

    // Safe: native_w/native_h are themselves u16 (rdpilot::config's
    // width/height fields are u16; RDP desktop dimensions never exceed
    // 65535), so both clamped values fit in u16.
    (nx as u16, ny as u16)
}
```

### Recommended unit test shape (the MCP-04 BLOCKING acceptance test)

```rust
#[test]
fn scale_to_native_lands_precisely_on_every_edge_and_corner() {
    // Default negotiated desktop: 1920x1080 (rdpilot::config::DEFAULT_WIDTH/HEIGHT).
    let (w, h) = (1920u32, 1080u32);

    // Corners: the four extreme points of the advertised space.
    assert_eq!(scale_to_native(0, 0, w, h), (0, 0));
    assert_eq!(scale_to_native(1279, 0, w, h), (1918, 0)); // top-right-ish, clamped inside bounds
    assert_eq!(scale_to_native(0, 799, w, h), (0, 1079));
    assert_eq!(scale_to_native(1279, 799, w, h), (1918, 1079));

    // Out-of-nominal-range boundary a model might still emit (exclusive edge):
    let (nx, ny) = scale_to_native(1280, 800, w, h);
    assert!(nx < w as u16 && ny < h as u16, "must clamp inside bounds, not equal to width/height");

    // Center sanity check.
    assert_eq!(scale_to_native(640, 400, w, h), (960, 540));
}
```

### Where do `native_w`/`native_h` come from at call time? (Open Question — see below)

`Session::desktop_size()` exists SDK-side (`crates/rdpilot/src/session.rs:391`, returns `(u32, u32)` in physical virtual-desktop pixels, captured once at connect) but **the MCP server never touches `rdpilot::Session` directly** (D-17 thin-client invariant) — it only sees `rdpilot-ipc` wire DTOs, and **neither `WireResponse` nor `SessionStatus` currently carries width/height** `[VERIFIED: read crates/rdpilot-ipc/src/response.rs in full]`. This research recommends adding a minimal wire verb (see Open Questions for the two options and the recommendation).

## Non-Blocking Isolation Design (MCP-06, BLOCKING)

Three independent, ALREADY-EXISTING mechanisms compose to satisfy MCP-06; the MCP crate itself needs to add only the third:

**Layer 1 — rmcp's own per-request task spawn** `[CITED: github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/src/service.rs, fetched 2026-07-11]`. The service's main event loop is a `tokio::select!` over several channels (`sink_proxy_rx`, `transport.receive()`, `peer_rx`, `send_task_set.join_next()`, a cancellation token); when it observes `Event::PeerMessage(JsonRpcMessage::Request(...))` it calls `spawn_service_task(async move { ... service.handle_request(request, context).await ... })` — a genuine `tokio::spawn`-equivalent, NOT an inline `.await`. The loop immediately returns to `select!`-ing on the NEXT event without waiting for that task to finish. **Consequence: this is already true for every `#[tool]`-macro-declared handler with zero extra code from this phase** — a slow `rdpilot_put`/`rdpilot_get`/`rdpilot_launch` call never blocks the stdio read loop from receiving and dispatching the NEXT incoming tool call.

**Layer 2 — the daemon's per-connection + per-session isolation** `[VERIFIED: read crates/rdpilot-daemon/src/server.rs and crates/rdpilot-daemon/src/registry.rs in full]`. `server.rs`'s accept loop calls `tokio::task::spawn_local(async move { crate::ipc::serve_connection(stream, &registry_for_conn).await; })` for EVERY accepted connection — since (per Pattern 3 above) the MCP server opens a fresh `UnixStream` per tool call, every concurrent tool call becomes an independent daemon-side task automatically. Within `dispatch`, `Registry::call` clones the target session's `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` under the registry's SYNCHRONOUS outer `Mutex`, drops that outer lock, THEN `.await`s the inner `tokio::sync::Mutex` — so two calls targeting DIFFERENT sessions run in true parallel (each has its own `Arc`), while two calls targeting the SAME session serialize on the inner per-session lock. This same-session serialization is intentional and correct (a single RDP session has one input stream — concurrent unordered mouse/keyboard PDUs to the same session would be a correctness bug, not a performance win), and it does not violate MCP-06: MCP-06's own success criterion is phrased as "a slow tool call does NOT block a concurrent **unrelated** tool call" — same-session calls are, by construction, related.

**Layer 3 (this phase must add) — explicit, bounded, per-verb-class `tokio::time::timeout`.** Neither the daemon's `dispatch` nor `Registry::call` has ANY built-in timeout `[VERIFIED: read crates/rdpilot-daemon/src/dispatch.rs and registry.rs in full — no timeout/deadline logic present]`. Without an explicit bound, a hung sensor round trip (DVC timeout, dead RDP session) would leave the MCP server's per-call tokio task (Layer 1) parked indefinitely — harmless to OTHER concurrent calls (Layer 1 already isolates them), but a real problem for the calling LLM's own agent loop (an indefinite hang looks identical to "still thinking," burning the loop's own wall-clock/iteration budget with no error to reason about). Recommended bounds, informed by the SDK's own documented worst-case latencies:

| Verb class | Verbs | Recommended timeout | Rationale |
|------------|-------|----------------------|-----------|
| Fast perception/input | `Ping`, `Screenshot`, `Mouse`, `Key`, `SetForeground`, `WindowList`, `ProcessList`, `Uia`, `WorldState` | **15s** | Sensor round trips are bounded internally at 500ms per `ping()` (session.rs); `WorldState` sequences several sensor calls but is still sub-second in the common case — 15s gives generous headroom for a slow/laggy remote without masking a genuinely stuck session for minutes |
| Session lifecycle | `Connect` | **120s** | Worst-case documented deploy-and-launch budget: `LAUNCH_ATTEMPTS` (3) × `PINGS_PER_LAUNCH_ATTEMPT` (20) × up to ~500ms/poll ≈ 30s per attempt × 3 ≈ 90s, plus `RUN_DIALOG_SETTLE`/settle delays — 120s comfortably covers the SDK's own documented worst case |
| Session lifecycle (cheap) | `List`, `Disconnect` | **10s** | No sensor round trip involved (`List` reads the in-memory registry; `Disconnect` awaits a close/join that is itself bounded by the session's own shutdown path) |
| Slow transfer/launch | `Put`, `Get`, `LaunchProcess` | **300s (5 min)** | File-transfer duration scales with file size and remote disk/network speed with no SDK-side cap; `LaunchProcess` itself is fire-and-forget (returns a PID immediately per `dispatch.rs`'s `Pid { pid }` response, not a wait-for-exit) so in practice this class only needs headroom for large `Put`/`Get` — 300s is a conservative starting bound the planner should treat as configurable, not hard-coded magic |

**On a timeout**, the MCP server's tool handler should return a distinct, clearly-worded tool error (NOT silently retry) — this is a genuinely NEW client-local error class (mirroring `CliError::DaemonUnreachable`'s precedent of a client-only class that never crosses the wire) rather than a `WireErrorCode` variant, since the daemon itself never observed a timeout — the MCP server gave up waiting.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|--------------|-----|
| Per-request concurrency isolation | A custom `tokio::spawn`-per-tool-call wrapper inside `rdpilot-mcp` | Nothing — rmcp's `service.rs` already spawns a task per inbound JSON-RPC request before dispatch | Verified directly from source; adding a second spawn layer would be redundant, not incorrect, but wasted complexity |
| JSON Schema for tool input structs | Hand-written JSON Schema literals | `#[derive(schemars::JsonSchema)]` on the input struct, consumed automatically by `#[tool]`/`#[tool_router]` | This is rmcp's own designed idiom (Pattern 2) — hand-written schemas drift from the actual deserialization struct exactly the way `rdpilot-ipc`'s doc comments repeatedly warn against for other hand-maintained mappings in this codebase |
| Key-combo string parsing (`"ctrl+s"` → `Vec<WireKey>`) | A THIRD hand-copied key-name table (the CLI already has one in `verbs/input.rs::parse_key_name`, and the MCP `key`/`hold_key`/native `rdpilot_key` tool would otherwise need a near-identical second copy) | **Recommend promoting `parse_key_name`'s table into `rdpilot-ipc` as a shared `pub fn parse_wire_key(name: &str) -> Result<WireKey, String>`**, then both `rdpilot-cli` and `rdpilot-mcp` call the ONE canonical table, and the MCP `key` action's `"+"`-split parsing becomes `text.split('+').map(parse_wire_key).collect()` | Avoids a second/third copy of a 67-entry match table silently drifting out of sync — exactly the kind of duplication `rdpilot-ipc`'s existing module docs repeatedly flag as a deliberate, tracked tradeoff (owned mirrors) that should NOT be extended further without reason. A key-name table is pure data/logic with zero `rdpilot`/`ironrdp` dependency, so moving it into `rdpilot-ipc` does not violate the thin-client invariant |
| Metadata-only file transfer enforcement (MCP-05) | Any new stripping/redaction logic in the MCP crate | Nothing — `WireResponse::Transfer(TransferOutcome { bytes_transferred, checksum })` `[VERIFIED: read crates/rdpilot-ipc/src/transfer.rs — the struct has exactly two fields, no byte-buffer field exists at all]` already structurally CANNOT carry file bytes; the MCP server's `rdpilot_put`/`rdpilot_get` handlers are a pure passthrough of this existing DTO into the tool result | MCP-05 is satisfied by composition, not new code — flag this to the planner explicitly so no plan wastes a task "implementing" something already true |
| PNG dimension decoding for the coordinate bridge (if the wire-verb option below is NOT taken) | Hand-parsing PNG IHDR bytes | The `image` crate's cheap header-only reader (`image::ImageReader::new(cursor).with_guessed_format()?.into_dimensions()`) | Only relevant if the planner picks the PNG-sniffing alternative in Open Questions instead of the recommended `DesktopSize` wire verb; hand-parsing an 8-byte-offset binary format for something a maintained crate already does correctly (including format-detection edge cases) is exactly the kind of "deceptively complex" problem this section exists to flag |

**Key insight:** Every piece of MCP-06's non-blocking-isolation requirement and MCP-05's metadata-only requirement is ALREADY satisfied by code that exists today in `rmcp` and the Phase 10-13 crates — this phase's actual net-new surface is small: tool schema declarations, the `computer` action dispatch table, the `scale_to_native` pure function, and a per-verb-class timeout wrapper. Resist the urge to re-derive concurrency guarantees that are already structurally true.

## Common Pitfalls

### Pitfall 1: stdout pollution corrupts the JSON-RPC stream
**What goes wrong:** Any `println!`/`eprintln!`-to-stdout call (a stray debug print, a panic message, a dependency's own logging default) interleaves plain text into the JSON-RPC frame stream stdio transport reads, and the MCP host's parser desyncs — usually surfacing as a cryptic "unexpected token" error on the HOST side, far from the actual bug.
**Why it happens:** stdio MCP servers use stdout as the ENTIRE wire protocol; there is no separate "log channel" unless the server explicitly routes logging to stderr.
**How to avoid:** `tracing_subscriber::fmt().with_writer(std::io::stderr)` from the very first line of `main`, before any other code runs (Pattern 1); `#[deny(clippy::print_stdout)]` or an equivalent lint at the crate root, mirroring this workspace's existing `#![deny(clippy::unwrap_used)]`/`#![deny(clippy::expect_used)]` per-crate convention.
**Warning signs:** The MCP host reports a generic parse/protocol error rather than a specific tool error; the server "hangs" from the host's perspective right after a specific log line would have fired.

### Pitfall 2: Coordinate rounding asymmetry near the advertised-resolution boundary
**What goes wrong:** A naive truncating (not rounding) scale, or a scale that does not clamp, can produce a native coordinate exactly `== native_w`/`== native_h` for a model-supplied `x == ADVERTISED_WIDTH - 1`/similar edge value, which `Session::check_bounds` (`x >= w || y >= h`) then REJECTS with `Error::CoordinateOutOfBounds` — turning a perfectly reasonable near-edge click into a hard tool-call failure, which is exactly the class of "drift" MCP-04 (BLOCKING) exists to catch.
**Why it happens:** Integer scale-factor math (`x * native_w / ADVERTISED_WIDTH` using integer division) truncates asymmetrically depending on rounding direction, and naive scaling has no clamp step at all.
**How to avoid:** Use floating-point scale factors, round-to-nearest (not truncate), and an explicit `.clamp(0.0, native_dim - 1.0)` as the LAST step before the `u16` cast (see the `scale_to_native` implementation above) — never rely on `Session::check_bounds`'s own rejection as the safety net; the bridge itself must never emit an out-of-bounds value.
**Warning signs:** Intermittent `CoordinateOutOfBounds` tool errors specifically on clicks near the edges of the advertised screenshot, never in the center.

### Pitfall 3: `computer` tool schema drift from the model's trained expectations
**What goes wrong:** Because the MCP server exposes `computer` as a NORMAL MCP tool (with an explicit JSON schema `rmcp`/`schemars` generates), NOT the Anthropic API's own built-in schema-less `computer_20250124` tool type, the exact field names the driving model emits depend entirely on what tool-list schema THIS server advertises — there is no guarantee a Claude model invoked through a generic MCP client will spontaneously use the EXACT field names (`coordinate`, `start_coordinate`, `scroll_direction`, `scroll_amount`, `text`) the native Anthropic computer-use tool uses, UNLESS the MCP tool's own schema/description explicitly documents them.
**Why it happens:** MCP tool schemas are model-agnostic JSON Schema, discovered fresh at `tools/list` time — the model has no special innate knowledge of "this is a computer-use tool," only what the schema + description string say.
**How to avoid:** Mirror the Anthropic field names EXACTLY in the schema (this maximizes the chance a computer-use-trained model's learned conventions transfer), and write an explicit, example-rich tool `description` string modeled on Anthropic's own action-by-action documentation (verified above) rather than a terse one-liner — this is the single highest-leverage lever available for click accuracy with a non-native-tool MCP integration.
**Warning signs:** PROOF-04 (Phase 15 live-LLM capstone) is the only test that can actually catch this — it cannot be caught offline. Document this explicitly as an offline/live testability split (see below).

### Pitfall 4: Session-parameter friction vs. D-29's no-implicit-default rule
**What goes wrong:** The computer-use `computer` tool, as Anthropic defines it, has NO concept of a session — every `left_click`/`screenshot` action in the native Anthropic schema takes only `coordinate`/`text`, never a session identifier. D-29 requires EVERY MCP tool call to carry an explicit `session` parameter with no implicit default. These two facts directly conflict for the `computer` tool specifically (the rdpilot-native tools have no such conflict — they can simply add `session` as a normal required field).
**Why it happens:** The `computer` tool's action schema is inherited/mirrored from a spec that assumes exactly one desktop; rdpilot supports N concurrent named sessions.
**How to avoid:** Add `session` as an EXTRA required top-level field alongside `action` on rdpilot's `computer` tool's OWN schema (this is fully legal — since the MCP server defines its own JSON schema for this tool per the resolution to Pitfall 3, it can add a field the Anthropic reference schema doesn't have without breaking anything). Document clearly in the tool description that `session` must be supplied on every call, matching every other `rdpilot_*` tool.
**Warning signs:** None at the schema level (the schema enforces it) — but confirm this is not silently reintroducing an implicit single-session assumption at the CALLING model's prompting/system-prompt layer (a genuinely single-session deployment is fine; a multi-session one needs the calling agent's own instructions to track which session it means).

### Pitfall 5: `wait`/`hold_key` durations as an unbounded resource-exhaustion vector
**What goes wrong:** Both `wait` (pure `tokio::time::sleep`) and `hold_key` (press, sleep, release) accept a caller-supplied `duration`. Because rmcp already isolates this into its own tokio task (Layer 1 above), a single huge `duration` does not block OTHER calls, but an unbounded value still lets a compromised/confused caller park an arbitrary number of long-lived tasks, and (per MCP-06's OWN spirit) a "slow tool call" with no upper bound at all somewhat undercuts the "bounded, explicit timeouts" requirement's intent.
**Why it happens:** The action schema's `duration` field has no inherent cap; Anthropic's reference implementation itself validates `hold_key`'s duration is `<= 100` seconds — precedent worth matching.
**How to avoid:** Cap both `wait` and `hold_key`'s `duration` at the schema/handler level (e.g. 30-100s) and reject anything larger with a clear tool error, mirroring Anthropic's own reference implementation's `duration <= 100` check.
**Warning signs:** None functional — this is a defense-in-depth/DoS-hardening pitfall, not a correctness one.

## Code Examples

### `computer` action enum (serde-tagged, `computer_20250124` shape)

```rust
// Field names mirror the Anthropic computer-use action schema verbatim
// (Pitfall 3) -- verified against platform.claude.com/docs live docs and
// the reference implementation's computer.py, 2026-07-11.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ComputerAction {
    Screenshot,
    MouseMove { coordinate: [u32; 2] },
    LeftClick { coordinate: [u32; 2], text: Option<String> },
    RightClick { coordinate: [u32; 2], text: Option<String> },
    MiddleClick { coordinate: [u32; 2], text: Option<String> },
    DoubleClick { coordinate: [u32; 2], text: Option<String> },
    TripleClick { coordinate: [u32; 2], text: Option<String> },
    LeftClickDrag { start_coordinate: [u32; 2], coordinate: [u32; 2] },
    LeftMouseDown { coordinate: [u32; 2] },  // rejected at dispatch time (GAP)
    LeftMouseUp { coordinate: [u32; 2] },    // rejected at dispatch time (GAP)
    Key { text: String },
    Type { text: String },
    Scroll { coordinate: Option<[u32; 2]>, scroll_direction: ScrollDirection, scroll_amount: u32, text: Option<String> },
    HoldKey { text: String, duration: f64 },
    Wait { duration: f64 },
    CursorPosition, // approximated at dispatch time (GAP)
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection { Up, Down, Left, Right }
```

### `screenshot` → MCP `image` content block (direct passthrough, MCP-05's sibling insight applied to screenshots)

```rust
// The daemon already base64-encodes the PNG (WireResponse::Screenshot).
// rmcp's CallToolResult content block for an image ALSO wants base64 PNG --
// no decode/re-encode round trip needed anywhere in this path.
fn screenshot_to_tool_result(png_base64: String) -> rmcp::model::CallToolResult {
    rmcp::model::CallToolResult::success(vec![
        rmcp::model::Content::image(png_base64, "image/png".to_owned()),
    ])
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|-------------------|----------------|--------|
| `computer_20241022` (base action set) | `computer_20250124` (adds scroll/triple_click/hold_key/wait/left_mouse_down/left_mouse_up) | Anthropic tool-version bump, 2025-01 | D-14.2 already locks `computer_20250124`, correctly ahead of the deprecated base set |
| `computer_20250124` | `computer_20251124` (adds `zoom`/`enable_zoom`, current recommended default for Claude Sonnet 5/Opus 4.8-4.5) | Anthropic tool-version bump, live docs as of 2026-07-11 | **Not a re-litigation of the locked D-14.2 decision** — flagged here purely as a state-of-the-art note: the CURRENT Anthropic docs recommend `computer_20251124` as the default for the newest model family, with `computer_20250124` remaining fully supported for older/other models. Since Phase 14 defines its OWN MCP tool schema (not Anthropic's built-in schema-less tool), this state-of-the-art shift has NO forcing effect on this phase — `computer_20250124`'s action set is a strict subset of `computer_20251124`'s, so nothing here is deprecated, only newly-superseded by an optional richer set outside this phase's locked scope |

**Deprecated/outdated:** None directly affecting this phase's locked scope.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|-----------------|
| A1 | rmcp's `service.rs` `main` branch source (fetched via WebFetch/AI-summarization, not compiled/instrumented directly) accurately reflects the published 2.2.0 crate's actual runtime behavior (per-request task spawn) | Non-Blocking Isolation Design, Layer 1 | If `main` has diverged from the 2.2.0 tag, the "rmcp already isolates concurrent calls" claim could be false for the pinned version, and the planner would need to add an explicit `tokio::spawn` wrapper per tool call as a defensive measure regardless — LOW practical risk since adding that wrapper is cheap insurance even if redundant |
| A2 | The Anthropic computer-use modifier-key field name for click/scroll actions is `text` (per the LIVE 2026-07 platform.claude.com docs) rather than `key` (as one AI-summarized fetch of the reference-implementation `computer.py` suggested) | Computer-Use Action → IPC Verb Mapping table, `left_click`/`scroll`'s optional modifier param | Since the MCP server defines its OWN schema (not Anthropic's built-in one), this only matters for maximizing transfer from a computer-use-trained model's learned conventions — worst case, a model uses the "wrong" field name once and the tool call fails validation, which is a recoverable, self-correcting failure mode for an agentic loop, not a silent-wrong-behavior risk |
| A3 | `hold_key`'s multi-second press-and-hold semantics can be acceptably approximated as an atomic press+release combo via `WireKeyAction::Combo` (no genuine SDK-level "hold for N seconds while other actions may also occur" primitive exists) | Action mapping table, `hold_key` row | If a real workflow genuinely needs a held-down modifier spanning multiple subsequent actions (not just this one call), the atomic-combo approximation silently under-delivers — flagged explicitly in the mapping table rather than hidden, and is not a BLOCKING criterion for this phase |
| A4 | `Registry::call`'s per-session `tokio::sync::Mutex` serialization of same-session calls is the CORRECT/intended behavior (not a bug to fix in this phase) and satisfies MCP-06's "unrelated" qualifier | Non-Blocking Isolation Design, Layer 2 | If the planner/human disagrees that same-session serialization is acceptable (e.g. wants a screenshot to be fetchable while a same-session file transfer is mid-flight), this is a Phase 12 daemon-level design decision this phase cannot unilaterally change — flagged as an Open Question below rather than silently assumed away |

**If this table is empty:** N/A — see entries above.

## Open Questions

1. **How does the MCP server learn a session's current native desktop dimensions for `scale_to_native`?**
   - What we know: `Session::desktop_size()` exists SDK-side and returns the negotiated `(width, height)` in physical pixels, captured once at connect (default 1920x1080 unless a future phase threads custom dimensions through `Request::Connect`, which it currently does NOT — `dispatch.rs`'s `Connect` arm builds `ConnectionConfig` without ever calling `.dimensions(...)`). Neither `WireResponse` nor `SessionStatus` currently exposes width/height on the wire.
   - What's unclear: Whether this phase should extend `rdpilot-ipc`/`rdpilot-daemon` with a small new `Request::DesktopSize { session }` → `WireResponse::DesktopSize { width: u16, height: u16 }` verb pair (mirrors `Session::desktop_size()` 1:1, trivial daemon-side one-line `Registry::call` forward), OR whether the MCP server should instead decode the PNG header dimensions of the most-recently-received screenshot for that session (no upstream-crate changes, but has a "haven't screenshotted yet in this session" cold-start edge case, and couples coordinate correctness to screenshot-call ordering).
   - Recommendation: Add the small `DesktopSize` wire verb. It is the more robust design (no ordering dependency, no PNG-parsing dependency), costs ~10 lines split across `rdpilot-ipc::request.rs`/`response.rs` and one `dispatch.rs` arm, and matches this codebase's own established precedent of later phases extending earlier-phase wire crates when a genuine new need arises (Phase 13 did exactly this to `rdpilot-ipc` within its own scope). This should be confirmed with the human/planner since it technically touches Phase 11/12 crates from within Phase 14 — small, mechanical, and low-risk, but worth an explicit go-ahead given CONTEXT.md frames this phase's boundary as "the MCP surface binary and its tool mappings."

2. **Local-path exfiltration risk unique to the MCP (LLM-facing) surface for `rdpilot_put`/`rdpilot_get` (MCP-05's local-path side).**
   - What we know: FILE-03 (Phase 10) rigorously validates the REMOTE-side path (canonicalization under the share root, rejecting `..`/absolute/mixed-separator traversal) — this protection is unaffected and still applies. The LOCAL-side path (`local_path` in `Request::Put`/`Get`) has NO analogous validation anywhere in the codebase today — the CLI already exposes this exact same unrestricted local-path capability (Phase 13, CLI-03, already shipped) under the reasonable assumption that a CLI's `--local` flag reflects a trusted HUMAN operator's own deliberate choice.
   - What's unclear: An MCP server exposes the identical capability to an LLM-DRIVEN agent, which is a materially different trust boundary — a prompt-injected or compromised model could invoke `rdpilot_put({session, local_path: "/home/user/.ssh/id_rsa", remote_name: "notes.txt"})` and exfiltrate an arbitrary local file to the remote Windows target (from which further exfiltration, e.g. via a remote web browser, becomes possible). This risk is NOT mentioned anywhere in CONTEXT.md's Decisions/Discretion/Deferred sections, and is NOT one of D-14's locked or discretionary items — it appears to be a genuinely novel consideration this research surfaces rather than a previously-considered-and-accepted tradeoff.
   - Recommendation: Raise this explicitly with the human before/during planning. Do NOT silently add local-path sandboxing as an unrequested feature (that would be scope creep past CONTEXT.md's boundary), but do NOT silently ship the gap unflagged either. A minimal, low-cost mitigation the planner could propose (pending human sign-off): document the risk prominently in the `rdpilot_put` tool's own MCP description string ("this tool reads an arbitrary local file path with the daemon's own OS permissions — do not expose this MCP server to an untrusted/adversarial prompt source without additional sandboxing"), which costs nothing architecturally and at minimum makes the risk visible to whoever configures the MCP host.

3. **Is same-session call serialization (`Registry::call`'s inner per-session `tokio::sync::Mutex`) acceptable for EVERY pairing of rdpilot-native tools, or does any pairing need reordering/preemption?**
   - What we know: Verified this serializes ALL same-session calls (screenshot, mouse, key, world_state, put, get, etc.) through one lock — a same-session `Put` genuinely blocks a same-session `Screenshot` issued concurrently until the `Put` completes.
   - What's unclear: Whether an LLM-driven agent loop would ever WANT to interleave, e.g., "keep taking screenshots to show transfer progress while a large `Put` is in flight" on the SAME session — the current daemon architecture (Phase 12, already built, out of THIS phase's boundary to change) makes that impossible without a Phase 12 redesign.
   - Recommendation: Treat as accepted/out-of-scope for Phase 14 (this is a Phase 12 architectural property, not something the MCP surface introduces or can fix), but the planner should ensure any PROOF-03/04 test that exercises a same-session concurrent screenshot+transfer scenario asserts the CORRECT (serialized, not deadlocked or racing) behavior rather than assuming parallelism that does not exist.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|--------------|-----------|---------|----------|
| `rustc`/`cargo` (Rust toolchain) | Building/testing `rdpilot-mcp` | Not probed directly in this research sandbox (no `cargo` binary on PATH here) | workspace pins `rust-version = "1.78"` (matches `rdpilot-cli`) | N/A — this is the standard project toolchain, already required and used by every prior phase; the RESEARCH environment's own lack of a `cargo` binary does not indicate anything about the actual execution/planning environment, which has built and tested every prior phase successfully |
| `rmcp` 2.2.0 + transitive deps (`schemars`, `pastey`) | MCP-01 (server itself) | Resolvable — confirmed live on crates.io, `[OK]` per slopcheck | 2.2.0 | None needed |
| A local Unix domain socket (`XDG_RUNTIME_DIR` or cache-dir fallback) | Every tool call's daemon round trip (Pattern 3) | Confirmed working (Phase 12/13 tests already exercise this path against the real compiled `rdpilot-daemon` binary) | — | None needed — same infra Phase 13 already proved |
| An MCP host (Claude Desktop/Code) for the offline-testable parts of this phase | Nothing — PROOF-03/04's LIVE-LLM exercising is Phase 15's concern, not this phase's | N/A this phase | — | This phase's own offline tests (tool schema shape, `scale_to_native` unit tests, non-blocking-isolation test against a fake slow connector) need NO live MCP host or live LLM at all |
| A live Windows RDP target | Real screenshot pixel content / real click-landing verification | Not available in this research sandbox; not needed for THIS phase's own offline-testable acceptance | — | Deferred to the Phase 15 batched live gate (PROOF-04), matching the exact precedent Phase 13's own traceability entry already documents ("real screenshot pixel content... deferred to the Phase 15 batched live gate") |

**Missing dependencies with no fallback:** None — every dependency this phase's OWN offline-testable success criteria need is either already resolvable (rmcp/crates.io) or already proven working infra from Phase 12/13 (the daemon + Unix socket transport).

**Missing dependencies with fallback:** Live Windows target / live LLM — both correctly deferred to Phase 15 per this phase's own CONTEXT.md scope boundary and the ROADMAP's Phase 15 description.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | `cargo test` (Rust's built-in `#[test]`/`#[tokio::test]`), matching every other crate in this workspace |
| Config file | none — Cargo's own test harness, no external config file (matches `rdpilot-cli`, `rdpilot-daemon`, `rdpilot-ipc`, `rdpilot-config`) |
| Quick run command | `cargo test -p rdpilot-mcp` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|---------------------|--------------|
| MCP-01 | rmcp server exposes tools over stdio | integration (offline — spin up the handler in-process, no real stdio subprocess needed for schema-shape assertions) | `cargo test -p rdpilot-mcp --test tool_schema` | ❌ Wave 0 |
| MCP-02 | `computer` tool action dispatch maps every locked action to the right `Request` (or the right explicit rejection for the 3 GAP actions) | unit | `cargo test -p rdpilot-mcp computer::dispatch` | ❌ Wave 0 |
| MCP-03 | Every `rdpilot_*` native tool round-trips a `Request`/`WireResponse` pair correctly | unit (fake `WireResponse` fixtures, no live daemon needed) | `cargo test -p rdpilot-mcp native_tools` | ❌ Wave 0 |
| MCP-04 [BLOCKING] | `scale_to_native` precision at edges/corners | unit, pure function, no session/daemon needed | `cargo test -p rdpilot-mcp --test scale_to_native` | ❌ Wave 0 |
| MCP-05 | `Put`/`Get` tool results never carry file bytes | unit (assert the `CallToolResult` JSON-serialized text never contains a bytes/base64-file-content field — mirrors `rdpilot-ipc`'s own `no_wire_response_variant_ever_carries_the_planted_secret`-style structural test) | `cargo test -p rdpilot-mcp native_tools::put_get_metadata_only` | ❌ Wave 0 |
| MCP-06 [BLOCKING] | A slow fake tool call does not block a concurrent fast one | integration, offline, against the REAL compiled `rdpilot-daemon` binary with a slow fake connector (mirrors `rdpilot-daemon/tests/thread_leak_soak.rs`'s and `rdpilot-cli/tests/cli_lifecycle.rs`'s existing "real binary + fake session/connector" pattern) | `cargo test -p rdpilot-mcp --test non_blocking` | ❌ Wave 0 |

### Sampling Rate

- **Per task commit:** `cargo test -p rdpilot-mcp`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full workspace suite green before `/gsd-verify-work`; MCP-04 and MCP-06's BLOCKING tests must be explicitly named in the wave/phase summary as having passed (mirrors how Phase 10/12/13's SUMMARY.md entries explicitly call out each BLOCKING success criterion by name)

### Wave 0 Gaps

- [ ] `crates/rdpilot-mcp/` — new workspace member crate does not exist yet; scaffold `Cargo.toml` + `src/main.rs` + module skeleton first
- [ ] `crates/rdpilot-mcp/tests/tool_schema.rs`, `scale_to_native.rs`, `non_blocking.rs` — none exist yet
- [ ] Workspace `Cargo.toml`'s `members` array needs `"crates/rdpilot-mcp"` added (currently lists `rdpilot`, `rdpilot-ipc`, `rdpilot-config`, `rdpilot-daemon`, `rdpilot-cli` — verified by reading the workspace root `Cargo.toml`)
- [ ] If the `DesktopSize` wire verb (Open Question 1) is adopted: `rdpilot-ipc::request.rs`/`response.rs` need one new `Request`/`WireResponse` variant pair each, plus one new `dispatch.rs` arm and its test — small, mechanical, but genuinely new test surface outside `rdpilot-mcp` itself

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|-----------------|---------|----------------------|
| V2 Authentication | No (delegated) | The MCP server itself does not authenticate the MCP host/client — MCP stdio transport trust is inherited from "who can spawn this process," which is the same trust boundary the MCP HOST config already establishes. Daemon-side IPC peer-uid authorization (Phase 12, DAEMON-02) is unaffected and unchanged |
| V3 Session Management | Partial | "Session" here means an RDP session, not an auth session — D-29's required-`session`-parameter-on-every-call rule IS the relevant control (no implicit default target), already locked and inherited |
| V4 Access Control | Yes | D-29 (no implicit session default) + the daemon's existing per-uid IPC authorization (Phase 12) are the controls; this phase adds no new access-control surface of its own beyond correctly threading `session` through every tool per D-29 |
| V5 Input Validation | Yes | `scale_to_native`'s clamp (never emits an out-of-bounds coordinate — Pitfall 2), the key-combo parser's explicit-rejection-of-unknown-tokens discipline (mirroring the CLI's existing `parse_key_name` convention, T-13-17: "no silent drop, no panic"), and the `wait`/`hold_key` duration cap (Pitfall 5) are the concrete V5 controls this phase must implement |
| V6 Cryptography | No | No new cryptographic material this phase — checksum verification (SHA-256) is already implemented upstream (Phase 10, FILE-02) and untouched here |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|-------------------------|
| Prompt-injection-driven local file exfiltration via `rdpilot_put`'s unrestricted `local_path` (see Open Question 2) | Information Disclosure | **NOT resolved by this research** — flagged as an explicit Open Question requiring human sign-off before the planner decides whether/how to mitigate within this phase's boundary |
| Coordinate-boundary drift causing mis-clicks near screen edges (MCP-04) | Tampering (of intent — a click landing on the wrong UI element than the model intended) | `scale_to_native`'s round+clamp discipline (Pitfall 2), verified by the BLOCKING edge/corner unit test |
| Resource exhaustion via unbounded `wait`/`hold_key` durations or an unbounded slow-tool-call timeout | Denial of Service | The per-verb-class `tokio::time::timeout` bounds (Non-Blocking Isolation Design, Layer 3) + the `duration` cap (Pitfall 5) |
| stdout channel corruption from stray logging (Pitfall 1) | Tampering (of the wire protocol itself) | `with_writer(std::io::stderr)` from the first line of `main`, plus a `#[deny(clippy::print_stdout)]`-style lint |
| A malicious/buggy MCP client supplying an unknown `WireKey`/malformed action JSON | Tampering / DoS | `serde`'s own strict deserialization (an unrecognized `action` tag or missing required field is a hard MCP-level schema-validation rejection, mirroring `rdpilot-ipc::Request`'s existing SESSION-02-style "hard rejection, not silent fallback" philosophy) |

## Sources

### Primary (HIGH confidence)
- `crates.io` API (`https://crates.io/api/v1/crates/rmcp`, `.../rmcp/2.2.0`, `.../schemars`, `.../interprocess`) — live-queried 2026-07-11, confirms `rmcp` 2.2.0 is `max_stable_version`/`newest_version`, published 2026-07-08; feature manifest for `default`/`server`/`transport-io`/`macros`; `schemars ^1.0` and `interprocess 2.4.2` current stable versions
- `slopcheck scan --pkg crates.io <name> --json` — live-run 2026-07-11 against `rmcp`, `schemars`, `pastey`, `interprocess`, all `status: "OK"`, no flags
- Direct repository reads (this session, via the Read/Bash tools, not summarized): `crates/rdpilot-ipc/src/{request,response,error,transfer,input,perception,transport,lib}.rs`, `crates/rdpilot-daemon/src/{registry,server,dispatch,error_map}.rs`, `crates/rdpilot-cli/src/{connect,main,exit_codes,verbs/input}.rs`, `crates/rdpilot/src/{session,connect,config,error,worldstate,input}.rs`, `crates/rdpilot-config/src/{resolve,resolved}.rs`, `.planning/{ROADMAP.md, REQUIREMENTS.md, DECISIONS-INDEX.md}`, `.planning/phases/14-mcp-server-surface/14-CONTEXT.md`
- `https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool` — fetched live 2026-07-11 (via redirect from `docs.claude.com`), the current official Anthropic computer-use tool documentation: full action list, `computer_20250124`/`computer_20251124` version distinction, tool parameters table, coordinate-scaling code samples, resolution guidance
- `https://raw.githubusercontent.com/anthropics/anthropic-quickstarts/main/computer-use-demo/computer_use_demo/tools/computer.py` — fetched live 2026-07-11, the official reference implementation's exact `Action_20241022`/`Action_20250124`/`Action_20251124` type definitions and per-action required/optional parameter table

### Secondary (MEDIUM confidence)
- `https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/crates/rmcp/src/service.rs` — fetched live 2026-07-11 via AI-summarized WebFetch (not a direct file read/grep, and reflects the `main` branch, not necessarily the exact 2.2.0 tag) — confirms the per-request `spawn_service_task`/`tokio::select!` concurrency model (see Assumptions Log A1)
- `https://github.com/modelcontextprotocol/rust-sdk/blob/main/examples/servers/src/{common/calculator.rs, calculator_stdio.rs}` — fetched live 2026-07-11 via AI-summarized WebFetch — the `#[tool_router]`/`#[tool]` macro shape and stdio-server `main()` bootstrap pattern
- `https://docs.rs/rmcp/2.2.0/rmcp/index.html` and `.../rmcp/model/struct.ErrorData.html` — fetched live 2026-07-11 via AI-summarized WebFetch — `ErrorData` fields/constructors, module layout confirmation

### Tertiary (LOW confidence)
- None used as load-bearing claims — every WebSearch-only finding in this research was cross-verified against at least one live-fetched authoritative source (crates.io API, official docs, or the actual repository source) before being stated as fact.

## Metadata

**Confidence breakdown:**
- Standard stack (rmcp/schemars/interprocess versions): HIGH — verified directly against crates.io's live API, not training-data recall
- Computer-use action schema: HIGH — verified against the LIVE current official docs (not a cached/stale training-data version) and the reference implementation source
- rmcp concurrency model (per-request task spawn): MEDIUM-HIGH — verified via AI-summarized fetch of the actual source file on the `main` branch (not the exact tagged release, and not independently compiled/instrumented in this sandbox — see Assumptions Log A1)
- Daemon-side concurrency model (`Registry::call`, `spawn_local` per connection): HIGH — read directly from the actual repository source in this session, not summarized or assumed
- `scale_to_native` math and design: HIGH — derived from first principles against the SDK's own documented coordinate/bounds-checking behavior (`Session::check_bounds`, `Error::CoordinateOutOfBounds`), which was read directly from source
- Pitfalls/gaps (unsupported computer-use actions): HIGH — each gap was confirmed by directly reading `crates/rdpilot/src/input.rs`'s `MouseAction`/`Key` enums and `crates/rdpilot-ipc/src/input.rs`'s wire mirrors, not inferred

**Research date:** 2026-07-11
**Valid until:** ~14 days for the rmcp/Anthropic-docs-dependent portions (both areas are fast-moving — `rmcp` published a new version one day before this research, and Anthropic's computer-use docs already show a version bump beyond this phase's locked `computer_20250124` pin) — the rdpilot-internal architecture findings (daemon concurrency, wire DTOs) remain valid until the underlying Phase 10-13 code changes, which is tracked by this repository's own git history, not a calendar date.
