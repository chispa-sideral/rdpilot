# Phase 15: Proof Harnesses & Live-LLM Capstone - Research

**Researched:** 2026-07-11
**Domain:** Scripted end-to-end proof harnesses for a CLI + an MCP stdio server (subprocess-driven, no live LLM), a live-LLM MCP-client agent loop against the real Anthropic Messages API, and the FULL batched live-gate inventory (all deferred live items from Phases 12/13/14) run in one Azure VM session.
**Confidence:** MEDIUM-HIGH (harness mechanics, batched-gate inventory, and Anthropic tool-use wire shape are HIGH — all directly verified against official docs/source/crates.io this session; the exact live-diagnosed timing/behavior of the batched run itself is inherently LOW until executed, same as every prior live-gate phase in this project)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions (Phase-Local)

- **D-15.1:** Proof + capstone run at FULL FIDELITY, no mocks of the surfaces under test. PROOF-02 spawns the REAL `rdpilot` CLI binary as a subprocess; PROOF-03 spawns the REAL `rmcp` stdio server (`rdpilot-mcp`) as a subprocess and speaks the actual MCP protocol over its stdin/stdout — both driving the live session daemon over its real local IPC transport against a live Azure VM. Both harnesses reuse the Phase 9 live-test scaffolding verbatim: the `RDPILOT_LIVE` env gate, `.secrets/connection.json` loading, and the plain-stdout step-by-step `PASS`/`FAIL` trace with no JSON schema (D-9.4). The capstone (PROOF-04) is a COMMITTED, `RDPILOT_LIVE` + Anthropic-API-key-gated agent loop that calls the real Anthropic API and drives a read/inspect + file-transfer task purely through the MCP surface (no direct SDK/daemon shortcuts). All three target the v1.0 7-Zip File Manager window (`class_name` = `"7-Zip::FM"`) as the real remote-only program, for continuity with the Phase 9 proof.

### Claude's Discretion

- Exact task/prompt the capstone LLM is given (as long as it exercises both read/inspect via perception tools AND a file put/get), the Anthropic model id, and the max-turns/timeout budget of the agent loop.
- Whether the CLI and MCP proof harnesses live as `cargo test` gated integration tests, `--example` binaries mirroring `examples/proof_harness.rs`, or a mix — follow the existing Phase 9 harness placement unless a better structure emerges. **See Common Pitfalls #1 below — this research recommends gated `tests/*.rs`, not `--example`, and explains the mechanical reason why.**
- Exact set of MCP tool calls the PROOF-03 harness exercises programmatically (must at minimum cover session connect/list/disconnect, one perception read, and file put/get), and how CLI distinct non-zero exit codes (D-28) are asserted in PROOF-02.

### Deferred Ideas (OUT OF SCOPE)

- MCP progress notifications (`notifications/progress`) for long transfers — future release.
- Any new daemon/CLI/MCP feature — Phases 12/13/14 already shipped these; Phase 15 only proves them.
- Durable session reattach across daemon restart, PyO3/NAPI bindings, CLIPRDR clipboard — all explicitly deferred.

</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PROOF-02 | A scripted harness proves the CLI surface end-to-end against a real remote-only Windows program (no live LLM) | §Code Examples "PROOF-02 CLI harness", §Common Pitfalls #1 (CARGO_BIN_EXE_ placement), §Batched Live-Gate Procedure Wave 4 |
| PROOF-03 | A scripted harness proves the MCP surface end-to-end (tool calls exercised programmatically, no live LLM) | §Standard Stack (`rmcp` client feature), §Code Examples "PROOF-03 MCP harness", §Batched Live-Gate Procedure Wave 5 |
| PROOF-04 | A capstone live-LLM demo drives a read/inspect + file-transfer task through the MCP surface against a real remote-only Windows program | §Standard Stack (reqwest + Anthropic Messages API), §Code Examples "PROOF-04 capstone agent loop", §Human Checkpoints (Anthropic API key), §Batched Live-Gate Procedure Wave 6 |

**Also carried by this phase (batched live gates, not new requirements but must close before Phase 15's own harnesses can be considered the "final" proof of the milestone):** DAEMON-02 (Windows half), DAEMON-04 (live half), SESSION-01/03/04 (live e2e), the CLI-02/03 live-deferred items, and the MCP-04 live-deferred half. See §Batched Live-Gate Procedure.

</phase_requirements>

## Summary

Phase 15 has two structurally different jobs that the developer explicitly chose to fuse into one Azure VM session: (1) author three NEW proof artifacts (PROOF-02/03/04) that did not exist before, and (2) **execute** a backlog of ALREADY-WRITTEN-BUT-NOT-YET-RUN live verification work that Phases 12/13/14 deliberately deferred. The single most consequential finding for the NEW work is a Rust mechanics detail that directly resolves one of CONTEXT.md's open discretion items: `env!("CARGO_BIN_EXE_<name>")` — the mechanism `rdpilot-cli`'s own `tests/cli_lifecycle.rs` already uses to locate the compiled `rdpilot`/`rdpilot-daemon` binaries — is **only populated for `tests/`/`benches/` targets, never for `--example` binaries** (confirmed against the Cargo Book). Since D-15.1 requires PROOF-02/03 to spawn the REAL compiled CLI/MCP binaries as subprocesses (not link the SDK as a library, unlike Phase 9's `examples/proof_harness.rs`), this research recommends PROOF-02/03 live as gated `tests/*.rs` integration tests (mirroring `cli_lifecycle.rs`'s existing, already-proven pattern) rather than `--example` binaries — an `--example` binary would need a hand-rolled sibling-binary path-resolution workaround (and even that is fragile: example binaries build into `target/debug/examples/`, one directory level away from `target/debug/`, so a naive `current_exe().with_file_name(...)` lookup would resolve to the wrong directory).

The second load-bearing finding is the **inventory itself**: Phase 15 is not just PROOF-02/03/04. Reading STATE.md's Accumulated Context, ROADMAP.md's per-phase "Deferred to Phase 15" notes, and the still-unexecuted `12-07-PLAN.md` in full reveals SEVEN distinct live obligations that must all run against the SAME provisioned VM before it is torn down: (a) Phase 12's Windows-DACL/anti-squatting pipe security + windows-permissions legitimacy checkpoint, (b) Phase 12's live orphan-liveness (disconnect-vs-logoff) reconciliation, (c) Phase 12's end-to-end connect/list/disconnect against a real target, (d) Phase 13's CLI-02 live screenshot/click/UIA re-exercise, (e) Phase 13's CLI-03 live multi-MB put/get re-exercise, (f) Phase 14's MCP-04 live click-near-edges/corners re-exercise, and only then (g) the three new PROOF-02/03/04 harnesses. Critically, item (a) is the ONLY one of the seven that structurally cannot run on this project's established Linux-orchestration-host-driving-a-remote-Azure-VM pattern — `tokio::net::windows::named_pipe` is Windows-only code (`#[cfg(windows)]`), so proving real Windows DACL/squatting/cross-account-rejection semantics requires compiling and running on an ACTUAL Windows machine. This project already has one: PROJECT.md records the pinned ARM64 Windows host (Phase 2's `x86_64-pc-windows-gnu`/MinGW build decision) used throughout the whole milestone. Items (b)-(g) do NOT need that Windows host — they exercise cross-platform Rust (IronRDP + the Unix IPC path) against the remote Windows VM target exactly like every prior live gate in this project, and can run from this Linux agent environment via `az vm run-command` + the established sensor-build/SAS-relay pattern.

The third finding: PROOF-04's live-LLM capstone needs an HTTP client for the real Anthropic Messages API and an MCP client to drive `rdpilot-mcp` as a subprocess — NEITHER requires an unfamiliar or newly-legitimacy-gated dependency. `rmcp` 2.2.0 (already D-26-locked, already in the manifest) ships a `client` + `transport-child-process` feature pair (`TokioChildProcess`, `serve_client`) that is the idiomatic way to drive an MCP stdio server programmatically — this is the SAME mechanism PROOF-03's harness needs, so PROOF-03 and PROOF-04 share one client-transport pattern. `reqwest` 0.12 is already resolved in `Cargo.lock` (a transitive dependency of `ironrdp-tokio`'s `reqwest` feature) — adding it as a DIRECT dependency of the capstone target for calling `https://api.anthropic.com/v1/messages` needs no new crate, just a `[dev-dependencies]` (or crate-scoped) entry with `rustls-tls` selected to stay consistent with the project's existing TLS-backend choice (D-15/v1.0: rustls, never OpenSSL).

**Primary recommendation:** Author PROOF-02/03 as gated `tests/*.rs` files (CARGO_BIN_EXE_ pattern, mirroring `cli_lifecycle.rs`), author PROOF-04 as a gated `tests/*.rs` file too (for the same CARGO_BIN_EXE_ reason) living in `crates/rdpilot-mcp/tests/`, and sequence the batched VM session as: provision once -> build sensor on VM -> run the Windows-DACL portion of 12-07 SEPARATELY on the pinned Windows dev machine (parallel or before/after the VM session, developer-executed) -> run the remaining live_daemon.rs (orphan-liveness + e2e) from the Linux host against the VM -> CLI-02/03 live re-exercise -> MCP-04 live half -> PROOF-02 -> PROOF-03 -> PROOF-04 -> developer-authorized teardown.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| PROOF-02 CLI proof harness | Backend / Test process (Linux orchestration host) | Client surface under test (`rdpilot` CLI binary, spawned) | The harness IS a test process; it spawns and observes the real CLI as a black box, exactly as a human operator would |
| PROOF-03 MCP proof harness | Backend / Test process (Linux orchestration host) | Client surface under test (`rdpilot-mcp` binary, spawned via rmcp `TokioChildProcess`) | Same shape as PROOF-02, but the "black box" speaks MCP JSON-RPC over stdio instead of CLI args/exit codes |
| PROOF-04 capstone agent loop | Backend / Test process (Linux orchestration host) | External service (Anthropic Messages API) + Client surface under test (`rdpilot-mcp`) | The loop is simultaneously an Anthropic API HTTP client AND an MCP client of the real `rdpilot-mcp` subprocess — never touches the SDK/daemon directly (D-15.1) |
| Windows named-pipe DACL + anti-squatting (12-07 Task 2/3a) | Windows build/runtime host (the pinned ARM64 dev machine) | — | `tokio::net::windows::named_pipe` is `#[cfg(windows)]`; this code cannot compile or run on the Linux orchestration host at all — genuinely different from every other live-gate item in this milestone |
| Orphan-liveness reconciliation + e2e session verify (12-07 Task 3b/3c) | Backend / Daemon process (Linux orchestration host, driving the remote Azure VM) | Remote Windows target (VM) | Cross-platform Rust (IronRDP + the already-offline-proven Unix IPC path); no Windows build host needed — same topology as every Phase 4-10 live gate |
| Azure VM provisioning/teardown | Infra (Bicep + `manage-env.ps1`, PowerShell) | Automation Account (auto-destroy backstop) | Unchanged from Phase 1; Phase 15 is a CONSUMER of this infra, not a modifier |
| Remote Windows target program (7-Zip File Manager) | Remote Windows target (VM) | — | Pre-installed via `Configure-Target.ps1`'s SHA-256-pinned 7-Zip 26.01 install; reused verbatim, no re-verification needed |

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `rmcp` | 2.2.0 `[VERIFIED: crates.io, repo github.com/modelcontextprotocol/rust-sdk]` | Already D-26-locked server dependency of `rdpilot-mcp`; PROOF-03/PROOF-04 additionally need its **client** side | Confirmed on crates.io: max/stable version 2.2.0, 15.4M downloads, official `modelcontextprotocol` org repo. Client-side transport confirmed via docs.rs (this session): `transport::child_process::TokioChildProcess` + `service::serve_client()` under the `client` + `transport-child-process` feature flags |
| `tokio` | already workspace-pinned (1.x, `full`/subset features per crate) | Async runtime for the harness/capstone test binaries | Already the workspace's async runtime everywhere |
| `serde_json` | already workspace-pinned | Building/parsing Anthropic Messages API JSON bodies and rmcp tool-call arguments | Already established throughout `rdpilot-ipc`/`rdpilot-mcp` |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `reqwest` | 0.12 (already resolved in `Cargo.lock` via `ironrdp-tokio`'s `reqwest` feature) `[VERIFIED: crates.io]` | HTTP client for the real Anthropic Messages API (`POST https://api.anthropic.com/v1/messages`) in the PROOF-04 capstone | Add as a direct, capstone-scoped dependency with `default-features = false, features = ["json", "rustls-tls"]` — matches the project's existing rustls-over-OpenSSL stance (D-15/v1.0) and reuses the already-resolved lockfile entry rather than pulling a second HTTP-client crate |
| `base64` | 0.22.1 (already pinned twice in this workspace, Phase 13) | Encoding a screenshot's PNG bytes into an Anthropic `image` content block (`data:` base64 payload) inside a `tool_result`, if the capstone task includes a `computer` `screenshot` action | Reuse the SAME pin already legitimacy-approved in Phase 13 — no new checkpoint |

**No new crate needs a fresh legitimacy checkpoint this phase** — `rmcp` was already approved (D-26), `reqwest` is already resolved in the lockfile and is one of the most widely-used, longest-lived HTTP crates in the ecosystem, and `base64` reuses Phase 13's existing pin. See Package Legitimacy Audit for the formal record.

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-rolled `reqwest` calls to the Anthropic Messages API | A third-party Anthropic Rust SDK crate (`async-anthropic`, `anthropic-sdk`, `anthropic-rs`, `anthropic_rust`, `anthropic-api`) | All five candidates checked on crates.io this session have LOW download counts (1.5K-73K) relative to `reqwest`'s 572M, none is Anthropic's own official crate (Anthropic does not publish an official Rust SDK), and the Messages API tool-use wire shape is simple/stable/well-documented enough that a raw `reqwest` call avoids taking on an under-vetted dependency for a security/cost-sensitive capstone that literally spends real API credits. If a future phase wants richer agent-loop ergonomics, evaluate then — not now, not for a ~5-tool-call capstone loop. |
| `--example` binaries for PROOF-02/03/04 | Gated `tests/*.rs` | `env!("CARGO_BIN_EXE_<name>")` (the mechanism needed to locate the sibling compiled binaries) is only set for test/bench targets, not examples (Cargo Book, confirmed this session) — see Common Pitfalls #1 |
| A single monolithic `crates/rdpilot-proof` new workspace crate | Per-surface `tests/*.rs` inside `rdpilot-cli`/`rdpilot-mcp` | Each proof harness needs `CARGO_BIN_EXE_` resolution scoped to the crate whose binary it is proving; a separate crate would need its own `[[bin]]`/build-order dance to guarantee the CLI/daemon/MCP binaries are already built, duplicating what `cli_lifecycle.rs` already solved. Living inside the surface's own crate (like `cli_lifecycle.rs` already does) is simpler and precedented. |

**Installation (capstone, added to `crates/rdpilot-mcp/Cargo.toml`):**
```toml
[dev-dependencies]
rmcp = { version = "2.2.0", features = ["client", "transport-child-process"] }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
base64 = "0.22.1"
```
(`rmcp`'s existing `[dependencies]` entry keeps its `server`/`macros`/`transport-io` features for the production binary; the `client`/`transport-child-process` features are ADDITIVE and only needed by the `tests/` target, so `[dev-dependencies]` is the correct scope — this does NOT violate the crate's thin-client `cargo tree` gate, since dev-dependencies are excluded from a production `cargo tree` invocation.)

**Version verification:** `rmcp` reconfirmed 2.2.0 on crates.io during this research session (2026-07-11), matching the already-locked D-26 pin — no drift since Phase 14. `reqwest` reconfirmed already resolved at 0.12.28 in the committed `Cargo.lock` (transitively, via `ironrdp-tokio`); crates.io's current max_stable is 0.13.4, but this research recommends staying on the 0.12 line already resolved in the lockfile rather than introducing a second major version into the dependency graph for zero functional benefit.

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|-------------|
| `rmcp` (client feature addition) | crates.io | created 2025-03-16 | 15.4M | github.com/modelcontextprotocol/rust-sdk | `[OK]` | Approved — already D-26-locked; this phase only enables additional feature flags on the same already-approved crate/version, no new legitimacy surface |
| `reqwest` (new direct dev-dependency) | crates.io | created 2016-10-16 (~10 yrs) | 572.7M | github.com/seanmonstar/reqwest | `[OK]` | Approved — already resolved in `Cargo.lock` transitively; one of the most widely used Rust crates in existence |
| `base64` | crates.io | pinned 0.22.1, Phase 13 legitimacy-approved | — | github.com/marshallpierce/rust-base64 | `[OK]` (Phase 13 record) | Approved — verbatim reuse of the existing pin, no second checkpoint |

**Packages removed due to slopcheck `[SLOP]` verdict:** none
**Packages flagged as suspicious `[SUS]`:** none

`slopcheck` 0.6.x (already installed on this research host) confirmed `[OK]` for both `rmcp` and `reqwest` this session (`slopcheck install rmcp reqwest --ecosystem crates.io`). Five third-party Anthropic-API Rust SDK crates were checked as alternatives and explicitly REJECTED as the primary path (see Alternatives Considered) — not because slopcheck flagged them, but because their download counts (1.5K-73K) are far below the bar this project's own legitimacy protocol treats as comfortable, and a raw `reqwest` call is simpler and fully auditable for the capstone's narrow, well-documented API surface.

## Architecture Patterns

### System Architecture Diagram — the batched live-gate session

```
                     ┌─────────────────────────────────────────────────────────┐
                     │   Linux orchestration host (this agent's environment)     │
                     │                                                            │
  az CLI ──────────▶ │  infra/manage-env.ps1 up  →  Azure VM (WS2022, 7-Zip)     │
  (already            │       │                                                   │
   authenticated)     │       ▼                                                   │
                     │  az vm run-command invoke (build C# sensor ON the VM)     │
                     │       │  (.NET 8 SDK + VS Build Tools install FIRST —      │
                     │       │   fresh WS2022 has neither preinstalled, 10-05)    │
                     │       ▼                                                   │
                     │  Storage blob SAS relay (sensor exe: VM → host)           │
                     │       │                                                   │
                     │       ▼                                                   │
                     │  ┌─────────────────────────────────────────────────┐      │
                     │  │ rdpilot-daemon (real compiled binary)             │      │
                     │  │  - Unix IPC (0700 dir + peer_cred, ALREADY PROVEN)│      │
                     │  │  - drives rdpilot::Session over RDP/DVC to the VM │─────▶│─── RDP (TLS/NLA) ───▶ Azure VM
                     │  └─────────────────────────────────────────────────┘      │      (7-Zip::FM window,
                     │       ▲                    ▲                    ▲          │       sensor over DVC)
                     │       │                    │                    │          │
                     │  ┌────┴─────┐        ┌─────┴──────┐      ┌──────┴──────┐   │
                     │  │ rdpilot  │        │ rdpilot-mcp│      │ PROOF-04    │   │
                     │  │ CLI      │        │ (real      │      │ capstone    │   │
                     │  │ (real    │        │  binary)   │◀─────│ (rmcp CLIENT│   │
                     │  │  binary, │        │            │ MCP  │  TokioChild-│   │
                     │  │  spawned │        │            │ JSON │  Process +  │   │
                     │  │  by      │        │            │ -RPC │  reqwest    │───┼──▶ https://api.anthropic.com/v1/messages
                     │  │  PROOF-02│        │            │ stdio│  Anthropic  │   │      (REAL API call, REAL $ cost)
                     │  └──────────┘        └────────────┘      │  client)    │   │
                     │       ▲                    ▲             └─────────────┘   │
                     │  PROOF-02              PROOF-03                            │
                     │  (tests/*.rs,          (tests/*.rs, rmcp CLIENT             │
                     │   CARGO_BIN_EXE_)       TokioChildProcess, CARGO_BIN_EXE_)  │
                     └─────────────────────────────────────────────────────────┘

                     ┌───────────────────────────────────────────┐
                     │  Pinned Windows dev machine (ARM64,         │
                     │  separate from the Linux orchestration host)│
                     │                                             │
                     │  cargo build --target x86_64-pc-windows-gnu │
                     │  RDPILOT_LIVE=1 cargo test --test live_daemon│
                     │  -- --ignored  (Windows-DACL/squatting/     │
                     │   cross-account-rejection ONLY — the        │
                     │   #[cfg(windows)] portion of 12-07)          │
                     └───────────────────────────────────────────┘
```

A reader tracing PROOF-04 end to end: the capstone process starts on the Linux host → spawns `rdpilot-mcp` as a child process via `TokioChildProcess` → calls `tools/list` to discover the real tool schemas → calls `rdpilot_connect` to open a real session against the VM → sends the task prompt + translated tool schemas to the Anthropic Messages API over `reqwest` → the API returns `tool_use` blocks → the capstone dispatches each one as a real `call_tool` to the `rdpilot-mcp` subprocess (which round-trips to the real daemon, which drives the real RDP session) → the tool result (including, for `computer` `screenshot`, a real base64 PNG) is sent back to the API as a `tool_result` → loop until `stop_reason != "tool_use"` or a turn budget is exhausted → the capstone asserts a file landed (via `rdpilot_get`/`rdpilot_put`'s returned checksum) and prints the D-9.4-style PASS/FAIL trace.

### Pattern 1: `env!("CARGO_BIN_EXE_<name>")` sibling-binary resolution (PROOF-02/03/04)

**What:** Locate the compiled `rdpilot`/`rdpilot-mcp` binaries at compile time via the Cargo-injected `CARGO_BIN_EXE_<name>` environment variable, exactly as `crates/rdpilot-cli/tests/cli_lifecycle.rs` already does for `rdpilot`/`rdpilot-daemon`.

**When to use:** Any `tests/*.rs` integration test that must spawn a REAL sibling binary from the same workspace as a subprocess.

**Example:**
```rust
// Source: crates/rdpilot-cli/tests/cli_lifecycle.rs (already in this codebase,
// verbatim pattern — PROOF-02/03/04 should copy this, not reinvent it).
use std::path::PathBuf;

let cli_bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
let daemon_bin = cli_bin.with_file_name(if cfg!(windows) { "rdpilot-daemon.exe" } else { "rdpilot-daemon" });
assert!(daemon_bin.exists(), "run `cargo build --workspace` first — {daemon_bin:?} missing");

// For PROOF-03/04, the MCP binary's own env var:
let mcp_bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-mcp"));
```

Note this ONLY works inside a `tests/`/`benches/` target — see Common Pitfalls #1 for why `--example` binaries cannot use this mechanism.

### Pattern 2: rmcp client-side subprocess transport (PROOF-03/04)

**What:** Spawn the REAL `rdpilot-mcp` binary as a child process and speak MCP over its stdio, using `rmcp`'s own client transport rather than hand-rolling JSON-RPC framing.

**When to use:** Any programmatic driver of the real `rdpilot-mcp` binary — both PROOF-03 (no LLM, scripted tool calls) and PROOF-04 (the capstone's tool-dispatch side) use this identical pattern.

**Example:**
```rust
// Source: rmcp 2.2.0 docs.rs (confirmed this session) — client feature +
// transport-child-process feature. `().serve(...)` uses the unit type as a
// trivial ClientHandler (no client-side capabilities needed for this harness).
use rmcp::{ServiceExt, transport::{TokioChildProcess, ConfigureCommandExt}};
use tokio::process::Command;

let mcp_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-mcp"));
let client = ()
    .serve(TokioChildProcess::new(Command::new(&mcp_bin).configure(|cmd| {
        // Inherit RDPILOT_* env (host/creds) from .secrets/connection.json
        // loading, mirroring the CLI/MCP surfaces' own config resolution.
    }))?)
    .await?;

let tools = client.list_tools(Default::default()).await?;
// tools.tools: Vec<rmcp::model::Tool> — each has `name`, `description`,
// `input_schema` (a JSON Schema object) — this is what PROOF-04 translates
// 1:1 into Anthropic's `tools` array `input_schema` field (Pattern 3).

let result = client
    .call_tool(rmcp::model::CallToolRequestParam {
        name: "rdpilot_connect".into(),
        arguments: Some(serde_json::json!({ "host": host, "username": user, "password": pass }).as_object().unwrap().clone()),
    })
    .await?;
```

### Pattern 3: Translating MCP tool schemas into Anthropic `tools` array entries (PROOF-04)

**What:** The Anthropic Messages API's `tools` array entries (`{name, description, input_schema}`) are structurally the SAME shape as an MCP `Tool` (`{name, description, input_schema}`, per the MCP spec's JSON-Schema-based tool definitions) — a straight 1:1 field mapping, no schema-dialect translation needed.

**When to use:** Building the capstone's `tools` request body from the `tools/list` response obtained via Pattern 2, once per agent-loop run (schemas don't change mid-session).

**Example:**
```rust
// Source: rmcp::model::Tool fields (name/description/input_schema) mapped
// directly onto Anthropic's documented tool-definition shape
// (platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools,
// confirmed this session).
let anthropic_tools: Vec<serde_json::Value> = tools.tools.iter().map(|t| {
    serde_json::json!({
        "name": t.name,
        "description": t.description,
        "input_schema": t.input_schema,
    })
}).collect();
```

### Pattern 4: The Anthropic tool-use loop over `reqwest` (PROOF-04)

**What:** A plain HTTP POST loop: send `messages` + `tools`, inspect `stop_reason`, if `"tool_use"` dispatch each `tool_use` block via Pattern 2's MCP client and append a `tool_result` user message (tool_result blocks MUST come first in that message's content array — confirmed this session against the official docs), repeat; stop on any other `stop_reason` or a hard turn-count ceiling.

**When to use:** PROOF-04's capstone main loop.

**Example:**
```rust
// Source: platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls
// (confirmed this session) — request/response shape, header requirements,
// and the "tool_result blocks first" ordering rule.
let resp = http_client
    .post("https://api.anthropic.com/v1/messages")
    .header("x-api-key", &api_key)
    .header("anthropic-version", "2023-06-01")
    .json(&serde_json::json!({
        "model": model_id, // see Open Questions — verify/override via env at run time
        "max_tokens": 4096,
        "tools": anthropic_tools,
        "messages": messages,
    }))
    .send()
    .await?
    .json::<serde_json::Value>()
    .await?;

let stop_reason = resp["stop_reason"].as_str().unwrap_or_default();
if stop_reason != "tool_use" {
    break; // natural completion, refusal, or max_tokens hit
}
// else: extract each `{"type": "tool_use", "id", "name", "input"}` block,
// call the matching rdpilot_* / computer MCP tool (Pattern 2), append a
// `{"role": "user", "content": [{"type": "tool_result", "tool_use_id": id,
// "content": [...]}]}` message, loop.
```

### Anti-Patterns to Avoid

- **Using `--example` for PROOF-02/03/04 and hoping to locate sibling binaries via `std::env::current_exe()`:** works but is fragile (examples build one directory deeper than the main binaries: `target/debug/examples/` vs `target/debug/`) and reinvents what `CARGO_BIN_EXE_` already solves cleanly for `tests/`. See Common Pitfalls #1.
- **Sending `tool_result` blocks with text before them in the same message:** the Anthropic API returns a 400 error; `tool_result` blocks must be first, any accompanying text after (confirmed in the official docs this session).
- **Hardcoding the Anthropic model id without an override mechanism:** model ids and naming conventions have changed multiple times in this project's training-vs-live-docs gap already (see State of the Art) — make it env-overridable so a stale hardcoded id doesn't silently break the capstone months later.
- **Running the FULL `12-07` live_daemon.rs suite exclusively on the Windows dev machine "because it's easiest":** the orphan-liveness and e2e-session portions (Task 3b/3c) do NOT need Windows and can run through the same Linux-orchestrated Azure-VM flow as everything else in this phase — needlessly routing them through the Windows host adds a second machine dependency to the critical path for no reason. Split as described in §Batched Live-Gate Procedure.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| MCP client wire protocol (framing, JSON-RPC method dispatch, capability negotiation) | A hand-rolled stdio JSON-RPC client speaking the MCP spec directly | `rmcp`'s `client` + `transport-child-process` features (`TokioChildProcess`, `serve_client`) | Already a workspace dependency (D-26); the SAME crate the production `rdpilot-mcp` server uses, so the client and server are guaranteed wire-compatible by construction — a hand-rolled client risks a subtly wrong framing/handshake that a real MCP host (Claude Desktop, etc.) would never hit |
| Anthropic Messages API tool-use agent loop | A full agentic-framework crate (Tool Runner-style SDK wrapper) | A direct, ~40-line `reqwest` loop (Pattern 4) | The capstone's loop is narrow (read/inspect + one file transfer, single agent, no parallelism, no memory/retrieval) — pulling in a framework crate adds a dependency surface with LOW download counts (see Alternatives Considered) for a problem this small; the official "Handle tool calls" doc is written explicitly for exactly this manual-loop case |
| Sibling-binary path resolution across `tests/`/`examples/`/`target/debug/` layout quirks | A custom "find the workspace target dir" helper walking `CARGO_MANIFEST_DIR` | `env!("CARGO_BIN_EXE_<name>")` (Pattern 1) | Already solved, already proven in this exact codebase (`cli_lifecycle.rs`) — Cargo itself guarantees the referenced binary is built before the test binary that references it runs |

**Key insight:** every "Don't Hand-Roll" item above already has a proven, in-repo precedent (either literally reused code from `cli_lifecycle.rs`, or the same crate/feature the production binary already depends on) — Phase 15's novelty is composition and sequencing, not new low-level mechanics.

## Runtime State Inventory

Not applicable — Phase 15 adds no renamed identifiers, no new persistent stores, and touches no OS-registered state. It is a proof/verification phase over already-shipped code.

## Batched Live-Gate Procedure

This is the ordered, single-VM-session inventory the developer explicitly requested — the FULL set of deferred live work, not just PROOF-02/03/04. Each wave lists its human checkpoint(s) explicitly.

### Wave 0 — Pre-flight (before spending any Azure minute)

1. **[HUMAN CHECKPOINT — blocking, pre-existing in `12-07-PLAN.md` Task 1]** Verify `windows-permissions` 0.2.4 (or the documented `windows-sys` fallback) crate legitimacy per `12-07-PLAN.md`'s already-authored Task 1. This gate exists independently of Phase 15 and should run FIRST, before the manifest change it gates, and ideally before VM provisioning (it costs nothing and blocks Wave 3).
2. **[HUMAN CHECKPOINT — new, this phase]** Confirm Anthropic API key availability. `env | grep -i anthropic` on this research session found NO `ANTHROPIC_API_KEY` set — the developer must supply one (with billing enabled) before PROOF-04 can run at all. Recommend the planner add an explicit `checkpoint:human-verify` gate for this, distinct from the VM-cost checkpoint, since it is a different budget/credential.
3. **[HUMAN CHECKPOINT — new, this phase]** Confirm a second local Windows account exists (or can be created) on the pinned Windows dev machine, for the DAEMON-02 cross-account-rejection assertion — mirrors the already-established `RDPILOT_SECOND_UID` pattern used for the Unix equivalent in `crates/rdpilot-daemon/tests/ipc_security.rs` (12-04). Without it, the DACL live test can prove "explicit DACL was set" and "squatting fails" but NOT "a genuinely different account is rejected" (one of 12-07's own `must_haves`).
4. **[HUMAN CHECKPOINT — recurring, pre-existing pattern]** Azure cost awareness / provisioning authorization. Every prior live gate in this project has run VM up→work→down within a single plan; THIS session is explicitly longer (7 waves of live work instead of 1), so flag the extended VM-uptime cost to the developer BEFORE `manage-env.ps1 up`, not only before teardown.

### Wave 1 — Provision (Linux host, `az` already authenticated in this environment)

`infra/manage-env.ps1 up -VmSize Standard_B2s_v2 -Location westeurope` (the `Standard_B2ms` default is `SkuNotAvailable` in westeurope per this project's own STATE.md — always override). Produces `.secrets/connection.json`. Then build the sensor ON the VM via `az vm run-command invoke` (embed the source tarball base64 directly in the script body — the `--parameters` flag has an undocumented size limit that silently fails, per the Phase 8 finding), FIRST installing `.NET 8 SDK` + `VS Build Tools` (VCTools workload) — a fresh WS2022 Datacenter image has neither preinstalled (10-05 finding). Relay the built exe back via a short-lived Storage blob SAS. Confirm SHA-256 byte-identity between the VM-built and locally-relayed copies (mandatory pattern from every prior phase's live gate).

### Wave 2 — Windows-DACL live gate (pinned Windows dev machine — SEPARATE from Wave 1's Linux orchestration)

Run ONLY the `#[cfg(windows)]`-dependent assertions from `12-07-PLAN.md` Task 2/3: explicit owner-only DACL creation, `first_pipe_instance(true)` anti-squatting, and the different-Windows-account rejection. **Recommendation for the planner:** split `live_daemon.rs` (as currently scoped by `12-07-PLAN.md`) into two files — `live_daemon_windows_dacl.rs` (Windows-host-only, run here) and the orphan-liveness/e2e portion (Wave 3, below, runs from the Linux host). This is a MODIFICATION to `12-07-PLAN.md`'s stated file scope; flag as an open question for the planner/developer to confirm before executing 12-07. Commands (on the pinned Windows machine, against the SAME `.secrets/connection.json` the VM's `up` step just wrote — copy/sync it over):
```
cargo build -p rdpilot-daemon --target x86_64-pc-windows-gnu
set RDPILOT_LIVE=1 && cargo test -p rdpilot-daemon --test live_daemon_windows_dacl -- --ignored
```
This wave can run IN PARALLEL with Wave 1's sensor build, since it doesn't depend on the sensor at all (pure IPC-transport-layer testing) — only the DACL/squatting/cross-account assertions, no RDP session needed. Sequencing it in parallel minimizes total VM billable time.

### Wave 3 — Orphan-liveness + e2e session verify (Linux host, against the VM — no Windows host needed)

The remaining `12-07-PLAN.md` Task 3 assertions: kill-9-mid-session + restart surfaces the possibly-live remote session as `Orphaned` (never silently forgotten), and connect/list/disconnect work end-to-end against the real target. These exercise cross-platform Rust (the Unix IPC path, already offline-proven) and the real `rdpilot::Session`/daemon registry — same topology as every Phase 4-10 live gate. `cargo test -p rdpilot-daemon --test live_daemon_e2e -- --ignored` (or whatever the split file is named).

### Wave 4 — CLI-02/03 live re-exercise (Linux host, against the VM)

Real screenshot pixel content, real click-coordinate landing, real UIA tree shape through the `rdpilot` CLI (CLI-02); a real multi-MB `put`/`get` transfer's bytes-transferred/checksum correctness through the CLI (CLI-03 — re-exercises FILE-01/02/04, already live-verified in Phase 10, through the NEW client surface; does not re-prove the underlying transfer semantics). Structure as a gated `tests/*.rs` in `rdpilot-cli` (or reuse/extend `cli_verbs.rs`/`cli_errors.rs` with a `--ignored` live variant) driving the REAL CLI binary against the REAL daemon (no `RDPILOT_DAEMON_TEST_CONNECTOR` fake — that env var must be UNSET for this wave, the opposite of `cli_lifecycle.rs`'s offline mode).

### Wave 5 — MCP-04 live half (Linux host, against the VM)

Real screenshot pixels, real click landing near screen edges/corners through the `computer` MCP tool (the pure `scale_to_native` math is already offline-proven — this wave proves the coordinates it produces actually land correctly on real Windows). Structure similarly: a gated live test in `rdpilot-mcp` driving the real daemon (no fake connector).

### Wave 6 — PROOF-02 (Linux host, against the VM)

The NEW scripted CLI end-to-end proof (this phase's own deliverable). See §Code Examples.

### Wave 7 — PROOF-03 (Linux host, against the VM)

The NEW scripted MCP end-to-end proof (no live LLM). See §Code Examples.

### Wave 8 — PROOF-04 capstone (Linux host, against the VM, real Anthropic API)

The NEW live-LLM capstone. See §Code Examples. Requires the Wave-0 Anthropic API key checkpoint.

### Wave 9 — Teardown

**[HUMAN CHECKPOINT — blocking, pre-existing pattern from EVERY prior live gate in this project]** Explicit developer authorization before `infra/manage-env.ps1 down`. STATE.md records this exact pattern for Phase 10 ("Teardown pending developer authorization (blocking checkpoint)") — Phase 15, being the longest live session yet, should hold to the same discipline, then confirm via `az group exists -n rdpilot-test` => `false` post-teardown (the verification step every prior gate has used).

### Open sequencing question for the planner

Does Phase 15's own plan set literally EXECUTE `12-07-PLAN.md` (marking Phase 12 complete as a side effect of Phase 15's Wave 1-3), or does Phase 15 author its OWN wave that duplicates/supersedes 12-07's already-written tasks? This research recommends the FORMER — `12-07-PLAN.md` is already fully authored (including its Task 1 legitimacy checkpoint and Task 2/3 acceptance criteria) and re-authoring it inside a Phase 15 plan would be pure duplication. The one modification this research recommends is the Windows/non-Windows FILE SPLIT described in Wave 2/3 above (a scope change to 12-07's stated `files_modified`, not a full rewrite).

## Common Pitfalls

### Pitfall 1: `--example` binaries cannot resolve sibling compiled binaries via `CARGO_BIN_EXE_`

**What goes wrong:** A PROOF-02/03/04 harness authored as `crates/rdpilot-cli/examples/proof_cli.rs` (mirroring Phase 9's `examples/proof_harness.rs` placement) tries `env!("CARGO_BIN_EXE_rdpilot-daemon")` and gets a compile error — the env var simply isn't set for example targets.

**Why it happens:** Cargo only populates `CARGO_BIN_EXE_<name>` when compiling `tests/`/`benches/` targets (confirmed against the Cargo Book this session); it was never extended to `examples/`, and Phase 9's own `examples/proof_harness.rs` never needed it because it links the SDK as a library directly — it has no sibling BINARY to locate at all (only the C# sensor .exe, resolved via a plain env-var-with-default convention, `RDPILOT_SENSOR_EXE`).

**How to avoid:** Use `tests/*.rs` for PROOF-02/03/04 (Pattern 1), or — if `--example` placement is preferred for some other reason — hand-roll a `RDPILOT_<SURFACE>_EXE` override env var with a computed default of `current_exe().parent().unwrap().parent().unwrap().join(name)` (walking OUT of the `examples/` subdirectory), mirroring `examples/proof_harness.rs`'s own `SENSOR_EXE_ENV` pattern for the ONE binary dependency it does have (the sensor).

**Warning signs:** A compile error naming `CARGO_BIN_EXE_rdpilot` (or similar) as an unset environment variable inside an `examples/*.rs` file.

### Pitfall 2: `tool_result` ordering and same-message-turn rules (PROOF-04)

**What goes wrong:** The capstone sends a `tool_result` message with explanatory text BEFORE the `tool_result` block (e.g. `[{"type":"text",...}, {"type":"tool_result",...}]`), or interleaves an extra assistant/user turn between the `tool_use` response and its `tool_result` reply — the API returns a 400 error ("tool_use ids were found without tool_result blocks immediately after").

**Why it happens:** Anthropic's tool-use protocol integrates tool calls directly into the `user`/`assistant` message array (no separate `tool` role, unlike some other vendors' APIs) — the ordering constraint is a documented but easy-to-miss requirement (confirmed against the official docs this session).

**How to avoid:** Always build the `tool_result` content array with EVERY `tool_result` block first, any trailing text second; never insert an intermediate message between a `tool_use` response and its corresponding `tool_result` reply.

**Warning signs:** A 400 HTTP response from the Messages API mid-loop; the capstone's PASS/FAIL trace should treat any non-2xx API response as an immediate FAIL step, not retry silently (mirrors D-9.4's "logical failure vs. hard transport error" distinction).

### Pitfall 3: The `computer` tool's fixed 1280x800 advertised space vs. the capstone's screenshot interpretation

**What goes wrong:** If the capstone's task prompt asks the LLM to click something based on a screenshot, the LLM reasons in the FIXED 1280x800 space the `computer` tool advertises (per MCP-04's `scale_to_native` design) — this is already correctly bridged server-side, so no NEW bug is expected here, but the capstone's task/prompt design must not accidentally tell the model to reason in native pixel coordinates (e.g. by also exposing `rdpilot_window_list`'s native-pixel `rect` values in the same turn without clarifying which space is which) — mixing the two coordinate spaces in the model's context risks confusing rather than helping it.

**Why it happens:** MCP-03's native tools (`rdpilot_window_list`, `rdpilot_uia`) report NATIVE-pixel bounding boxes, while the `computer` tool's `screenshot`/click actions operate in the FIXED 1280x800 advertised space — two different, both-correct, coordinate systems coexist by design (D-21).

**How to avoid:** If the capstone task uses BOTH the `computer` tool (for visual clicking) and native tools (for structured UIA/window queries) in the same run, keep the task narrow enough that the model doesn't need to reconcile the two coordinate spaces itself — e.g. prefer `rdpilot_uia`'s reported element name/role for VERIFYING what was clicked, not for computing a click target meant for the `computer` tool.

**Warning signs:** A capstone run where the model's `left_click` action lands on the wrong element even though the coordinate bridge unit tests (Phase 14) all pass — check whether the task's own prompt/tool sequencing conflated the two spaces before assuming a code regression.

### Pitfall 4: Azure `az vm run-command --parameters` size limit (already known, re-flagged for this phase's heavier build load)

**What goes wrong:** Passing the sensor source as a `--parameters` value (rather than embedding it in the script body) fails near-instantly with no useful error.

**Why it happens:** An undocumented CLI parameter-size limit (Phase 8 finding, STATE.md).

**How to avoid:** Always embed the base64 payload directly in the script body (the Phase 6/7/8/9/10 pattern) — this phase's Wave 1 provisioning step must follow the SAME convention, not rediscover the bug.

**Warning signs:** `az vm run-command invoke` returning near-instantly with a generic failure and no build log content.

### Pitfall 5: The `windows-permissions` legitimacy checkpoint blocks Wave 2, not Wave 1

**What goes wrong:** An executor tries to add `windows-permissions` to `Cargo.toml` before the Task-1 human-verify checkpoint from `12-07-PLAN.md` has been explicitly approved, treating it as "already fine because RESEARCH said `[OK]` on slopcheck."

**Why it happens:** slopcheck `[OK]` and an `[ASSUMED]` package-name-provenance tag are DIFFERENT signals — 12-RESEARCH.md correctly tagged `windows-permissions` `[ASSUMED]` (provenance from WebSearch, not Context7/official docs) even though slopcheck returned `[OK]`. Per this project's own package-legitimacy protocol, `[ASSUMED]` mandates human verification regardless of slopcheck's verdict.

**How to avoid:** Sequence Wave 0's checkpoint 1 BEFORE Wave 2 (already the plan's own ordering) and do not let Wave 1's VM-provisioning urgency pressure skipping it — it costs nothing and can run before, during, or independent of VM uptime.

## Code Examples

### PROOF-02 CLI harness (skeleton)

```rust
// crates/rdpilot-cli/tests/live_proof.rs — #[ignore], RDPILOT_LIVE=1 gated,
// mirrors cli_lifecycle.rs's CARGO_BIN_EXE_ resolution but WITHOUT the fake
// connector (real daemon, real VM target from .secrets/connection.json).
#[test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]
fn cli_end_to_end_against_a_real_target() {
    if std::env::var("RDPILOT_LIVE").is_err() {
        eprintln!("skipping: RDPILOT_LIVE not set");
        return;
    }
    // 1. connect --name proof-cli   (reads .secrets/connection.json via the
    //    D-27 config layer, e.g. --host/--username/--password flags or
    //    RDPILOT_* env vars set from the secrets file)
    // 2. perceive screenshot --output <tmp>.png   -> assert file nonzero size
    // 3. input launch "C:\Program Files\7-Zip\7zFM.exe" --args "C:\Program Files"
    // 4. perceive window list --json   -> assert a "7-Zip::FM" class_name entry
    // 5. put <local_file> <remote_name>   -> assert exit 0, parse --json output
    // 6. get <remote_name> <local_dest>   -> assert checksum matches step 5's
    // 7. disconnect proof-cli
    // Print PASS/FAIL per step (D-9.4 style); assert distinct nonzero exit
    // codes for at least one deliberately-broken call (e.g. get with a
    // wrong --session) to prove D-28's exit-code taxonomy end-to-end.
}
```

### PROOF-03 MCP harness (skeleton)

```rust
// crates/rdpilot-mcp/tests/live_proof.rs — same gating, but drives the real
// rdpilot-mcp binary as an MCP client (Pattern 2), not CLI args/exit codes.
#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM"]
async fn mcp_end_to_end_against_a_real_target() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("RDPILOT_LIVE").is_err() { return Ok(()); }
    let client = /* Pattern 2's client setup */;
    client.call_tool("rdpilot_connect", /* host/user/pass */).await?;
    client.call_tool("rdpilot_world_state", /* session, screenshot: true */).await?;
    client.call_tool("rdpilot_put", /* session, local_path, remote_name */).await?;
    client.call_tool("rdpilot_get", /* session, remote_name, local_path */).await?;
    client.call_tool("rdpilot_disconnect", /* session */).await?;
    // Assert each CallToolResult.is_error is false, and that put/get's
    // rendered {bytes_transferred, checksum} match (MCP-05 metadata-only
    // contract already unit-tested offline — this proves it live).
    Ok(())
}
```

### PROOF-04 capstone (skeleton)

```rust
// crates/rdpilot-mcp/tests/live_capstone.rs — RDPILOT_LIVE=1 AND
// ANTHROPIC_API_KEY gated (both must be present; fail fast with a clear
// message naming which is missing, never a silent skip that looks like a
// pass in CI-style output).
#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1, ANTHROPIC_API_KEY, and a live Azure VM"]
async fn capstone_reads_and_transfers_a_file_via_a_real_llm() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return Ok(());
    };
    if std::env::var("RDPILOT_LIVE").is_err() { return Ok(()); }

    let mcp_client = /* Pattern 2 */;
    let tools = mcp_client.list_tools(Default::default()).await?;
    let anthropic_tools = /* Pattern 3 */;

    let mut messages = vec![serde_json::json!({
        "role": "user",
        "content": "Connect to the configured rdpilot session, open the 7-Zip \
                     File Manager, report what you see in its menu bar via the \
                     UIA tree, then upload the file at <known local path> to \
                     the session and confirm its checksum via rdpilot_get."
    })];

    for _turn in 0..MAX_TURNS {
        let resp = /* Pattern 4 */;
        // dispatch tool_use blocks via mcp_client.call_tool, append tool_result
        // messages, break on stop_reason != "tool_use"
    }
    // Assert: at least one rdpilot_uia or computer/screenshot call happened
    // (the "read/inspect" half), and at least one rdpilot_put/rdpilot_get
    // call succeeded with a matching checksum (the "file-transfer" half) —
    // parse these from the transcript, don't just trust the model's own
    // closing summary text.
    Ok(())
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| Anthropic model ids with explicit date suffixes (e.g. `claude-3-5-sonnet-20241022`) | Short family-generation ids (e.g. `claude-opus-4-8`, per the official docs fetched this session) | Observed in current (2026-07) official docs snapshot | The capstone must NOT hardcode a training-data-vintage model id string without an env-override — verify the exact current id against the live API/docs at execution time, not against this document's training knowledge |
| N/A — no prior Anthropic-API integration in this codebase | First direct Anthropic API integration in `rdpilot` | This phase | No precedent to deviate from; follow the official Messages API docs verbatim |

**Deprecated/outdated:** None specific to this phase's stack — `rmcp`/`reqwest` are both current and actively maintained.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The exact Anthropic model id to hardcode (or default) for the capstone — this research deliberately did NOT pin one, per CONTEXT.md's "Claude's Discretion" on this exact point, and because model-id strings are one of the fastest-moving facts in Anthropic's ecosystem | §Code Examples, §State of the Art | If a stale/retired model id is hardcoded, the capstone fails outright with a 404/model-not-found at execution time — mitigated by making it env-overridable (`ANTHROPIC_MODEL`, defaulting to a value verified at PLANNING or EXECUTION time via a live models-list check, not this research session) |
| A2 | The Windows/non-Windows SPLIT of `12-07-PLAN.md`'s `live_daemon.rs` into two files (§Batched Live-Gate Procedure Wave 2/3) is a RECOMMENDATION, not something confirmed against the developer's actual intent for how 12-07 should be modified | §Batched Live-Gate Procedure | If the developer prefers running the WHOLE 12-07 suite on the Windows machine as originally scoped (simpler, one less moving part), the planner should keep it as a single file and accept the slightly slower/less-parallel sequencing — low risk either way, purely a scheduling optimization, not a correctness concern |
| A3 | `windows-permissions` 0.2.4's exact SDDL-string-to-`SECURITY_ATTRIBUTES` API surface — this was ALREADY flagged `[ASSUMED]` by 12-RESEARCH.md and gated by 12-07-PLAN.md's own Task 1; not re-verified in this session (out of this phase's scope — 12-07 is a dependency this research inventories, not re-researches) | §Batched Live-Gate Procedure Wave 0/2 | Already mitigated by the pre-existing Task 1 blocking-human checkpoint; no NEW risk introduced by this phase |

**If this table were empty:** it is not — three items above need explicit confirmation before/during execution, all already flagged as human-facing checkpoints or open questions.

## Open Questions

1. **Does the planner treat `12-07-PLAN.md` as directly executed inside Phase 15's wave sequence, or re-author its scope?**
   - What we know: `12-07-PLAN.md` is fully authored, including its Task 1 legitimacy checkpoint and acceptance criteria; Phase 12 shows 6/7 plans complete in ROADMAP.md.
   - What's unclear: Whether Phase 15's plan set should literally invoke/reuse `12-07-PLAN.md`'s tasks (closing out Phase 12 as a side effect) or duplicate equivalent tasks inside Phase 15's own plan files.
   - Recommendation: Execute `12-07-PLAN.md` as-authored (with the Wave 2/3 file-split recommendation applied), let it close out Phase 12, and have Phase 15's own plans pick up from there — avoids duplicating already-reviewed task content.

2. **Exact capstone task prompt wording and success-assertion mechanics.**
   - What we know: CONTEXT.md leaves this to discretion; must exercise both read/inspect AND file put/get through MCP.
   - What's unclear: Whether success should be judged by parsing the tool-call transcript (recommended — see Code Examples' closing assertion note) or by asking the model to self-report success in its final text response (weaker — models can claim success without having actually called the right tools).
   - Recommendation: Assert on the TRANSCRIPT (which tools were actually called, with what results), never solely on the model's closing prose.

3. **Does the second-Windows-account setup for the DACL cross-account test need to be provisioned by the agent (e.g. via a PowerShell script on the pinned Windows machine) or is it assumed to already exist?**
   - What we know: The Unix equivalent (`RDPILOT_SECOND_UID`) is opt-in/manually-configured, not agent-provisioned.
   - What's unclear: Whether the pinned Windows dev machine already has a second local account from prior project setup.
   - Recommendation: Flag as a Wave-0 human checkpoint (already done above) rather than assume either way.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `az` CLI (authenticated) | Wave 1 VM provisioning | ✓ (confirmed this session, `az account show` succeeded) | — | — |
| `ANTHROPIC_API_KEY` env var | Wave 8 / PROOF-04 | ✗ (confirmed absent this session) | — | None — PROOF-04 cannot run without it; must be supplied by the developer before Wave 8 |
| `slopcheck` | Package legitimacy checks | ✓ (installed on this research host) | 0.6.x | — |
| Rust toolchain / `cargo` | Building/running any of this phase's tests | ✗ in THIS specific research session's shell (no `rustup`/`cargo` on PATH) | — | Not a blocker for research — prior phases' SUMMARY files confirm `cargo` IS available in the actual execution environment (every phase since Phase 2 has run `cargo build`/`cargo test` successfully); this research session's shell profile simply didn't have it on PATH. Flag for the planner/executor to confirm at execution time, not treat as a genuine gap. |
| Pinned Windows dev machine (ARM64, `x86_64-pc-windows-gnu` toolchain, MinGW gcc) | Wave 2 (Windows-DACL live gate only) | Assumed ✓ (established throughout the whole milestone per PROJECT.md; not independently re-verified this session — outside this research's reach, it's the developer's own machine) | — | None documented for the DACL-specific assertions — this is inherently Windows-only code; no cross-platform substitute exists for proving real named-pipe DACL semantics |
| `.secrets/connection.json` | Every live wave | Present but STALE (last written 2026-07-10, prior VM already torn down per convention) | — | Regenerated fresh by Wave 1's `manage-env.ps1 up` — the stale file is expected and harmless, just needs overwriting |

**Missing dependencies with no fallback:**
- `ANTHROPIC_API_KEY` — blocks Wave 8/PROOF-04 entirely until supplied.
- A genuine second Windows Windows-account for the DACL cross-account-rejection assertion — blocks proving that specific `must_have` in `12-07-PLAN.md` Task 3(a) until provisioned.

**Missing dependencies with fallback:**
- None else identified — every other dependency this phase needs is either already present or already has an established provisioning path (Azure VM via `manage-env.ps1`, sensor build via `az vm run-command`).

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust's built-in `#[test]`/`#[tokio::test]` harness (via `cargo test`), exactly as every prior phase in this workspace uses — no separate test framework |
| Config file | none — plain `cargo test`, gated tests use `#[ignore]` + env-var checks (established convention, `tests/live_session.rs` et al.) |
| Quick run command | `cargo test --workspace` (excludes `#[ignore]`d live tests by default — fast, no Azure cost) |
| Full suite command | `RDPILOT_LIVE=1 ANTHROPIC_API_KEY=<key> cargo test --workspace -- --include-ignored` (requires the live VM + API key — this IS the batched live-gate run) |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| PROOF-02 | CLI end-to-end against a real target | live integration | `RDPILOT_LIVE=1 cargo test -p rdpilot-cli --test live_proof -- --ignored` | ❌ Wave 0 |
| PROOF-03 | MCP end-to-end against a real target, no live LLM | live integration | `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_proof -- --ignored` | ❌ Wave 0 |
| PROOF-04 | Live-LLM capstone read/inspect + file-transfer | live integration | `RDPILOT_LIVE=1 ANTHROPIC_API_KEY=... cargo test -p rdpilot-mcp --test live_capstone -- --ignored` | ❌ Wave 0 |
| DAEMON-02 (Windows half) | Explicit DACL + anti-squatting + cross-account rejection | live integration, Windows-host-only | `cargo test -p rdpilot-daemon --test live_daemon_windows_dacl -- --ignored` (run ON the pinned Windows machine) | ❌ Wave 0 (recommended split from existing 12-07 scope) |
| DAEMON-04 (live half) / SESSION-01/03/04 | Orphan-liveness + e2e session verify | live integration | `RDPILOT_LIVE=1 cargo test -p rdpilot-daemon --test live_daemon_e2e -- --ignored` | ❌ Wave 0 (recommended split; or reuse 12-07's single `live_daemon.rs` as originally scoped) |
| CLI-02/03 (live-deferred) | Real pixels/click/UIA/transfer through the CLI | live integration | new gated test in `rdpilot-cli/tests/` | ❌ Wave 0 |
| MCP-04 (live half) | Real click landing near edges/corners through `computer` | live integration | new gated test in `rdpilot-mcp/tests/` | ❌ Wave 0 |

### Sampling Rate

- **Per task commit (offline portions only):** `cargo test --workspace` — the default, no Azure cost, run continuously during authoring.
- **Per wave merge (the live batched session itself):** the full `--include-ignored` command above, run ONCE across the whole VM session per §Batched Live-Gate Procedure's wave ordering — re-running individual waves mid-session is fine, but the intent is one VM lifecycle covering all waves.
- **Phase gate:** all seven waves' assertions PASS, teardown authorized and confirmed, before `/gsd-verify-work`.

### Wave 0 Gaps

- [ ] `crates/rdpilot-cli/tests/live_proof.rs` — PROOF-02
- [ ] `crates/rdpilot-mcp/tests/live_proof.rs` — PROOF-03
- [ ] `crates/rdpilot-mcp/tests/live_capstone.rs` — PROOF-04
- [ ] A CLI-02/03 live-deferred gated test (new file or extension of `cli_verbs.rs`/`cli_errors.rs`)
- [ ] An MCP-04 live-deferred gated test (new file, `rdpilot-mcp/tests/`)
- [ ] Confirm/execute `12-07-PLAN.md`'s pre-existing `crates/rdpilot-daemon/tests/live_daemon.rs` (with or without the Windows-host file split recommended above)
- [ ] `crates/rdpilot-mcp/Cargo.toml` — add `[dev-dependencies]` for `rmcp` (client + transport-child-process features), `reqwest` (rustls-tls), `base64`

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes (indirect) | The capstone's `ANTHROPIC_API_KEY` is a bearer credential — never logged, never printed, read from env only (mirrors `ConnectionConfig`'s existing password-redaction discipline, D-14/D-24) |
| V3 Session Management | no (new) | Already covered by Phases 11/12's D-29 session-identity work; Phase 15 only CONSUMES it |
| V4 Access Control | no (new) | Already covered by DAEMON-02 (this phase's Wave 2/3 EXERCISE it, not define it) |
| V5 Input Validation | yes (indirect) | The capstone's task prompt is developer-authored (trusted, not untrusted user input) — but the MODEL'S tool-call `input` arguments are effectively externally-supplied data flowing into `rdpilot_put`/`rdpilot_get`'s existing path-canonicalization validators (FILE-03, already `[BLOCKING]`-proven); no NEW validation surface, but worth noting the capstone is itself a live end-to-end proof that an LLM-originated tool call still passes through the SAME validators as a human-originated one |
| V6 Cryptography | no (new) | The Anthropic API call is a plain HTTPS `reqwest` request; TLS is handled by `rustls-tls` (project-standard backend, never hand-rolled) |

### Known Threat Patterns for this phase's stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| `ANTHROPIC_API_KEY` leaking into logs/stdout/committed fixtures | Information Disclosure | Read from env only, never printed; the capstone's PASS/FAIL trace (D-9.4 style) must be written to explicitly exclude the key, mirroring `ConnectionConfig`'s `Debug` redaction precedent — grep the capstone's own test output for the key value as a regression guard, same style as `render_transfer`'s MCP-05 planted-sentinel test in `native_tools.rs` |
| Prompt injection via a remote UI element's text (e.g. a malicious window title or UIA element `name` fed back into the model's context) leading the model to call `rdpilot_put`/`rdpilot_get` against an unintended path | Tampering / Elevation of Privilege | Already flagged by `rdpilot_put`'s own tool description (T-14-12, "trusted-operator model") — the capstone's target program (7-Zip File Manager against a pre-provisioned, disposable, credential-free VM) has no attacker-controlled content in this specific proof, so this is a LOW-severity residual risk for THIS phase's narrow demo, not a new mitigation this phase must build; note it explicitly in the capstone's own doc comments so a future phase reusing this harness against untrusted content doesn't assume it's already hardened |
| A malformed/adversarial Anthropic API response (missing `stop_reason`, malformed `tool_use` block) crashing the capstone loop | Denial of Service (local, to the test run only) | Treat every field extraction as fallible (`.get()`/`.as_str()` with explicit error branches, never `.unwrap()`/array-index panics) — mirrors this crate's existing `#![deny(clippy::unwrap_used)]` discipline already enforced crate-wide in `rdpilot-mcp`/`rdpilot-cli` |

## Sources

### Primary (HIGH confidence)
- `crates/rdpilot-cli/tests/cli_lifecycle.rs`, `crates/rdpilot-mcp/src/{handler,native_tools,connect,timeouts}.rs`, `crates/rdpilot/examples/proof_harness.rs`, `crates/rdpilot/tests/support/proof_harness.rs` — read directly this session, the precedents this research builds on
- `.planning/phases/12-session-daemon/12-07-PLAN.md`, `12-RESEARCH.md` — read in full this session, the pending live-gate scope this phase must inventory
- `.planning/ROADMAP.md`, `.planning/REQUIREMENTS.md`, `.planning/DECISIONS-INDEX.md`, `.planning/STATE.md` — read in full this session
- crates.io API (`curl` this session) — `rmcp` 2.2.0, `reqwest` 0.12.28 (Cargo.lock-resolved)/0.13.4 (max), `base64` 0.22.1, five Anthropic-SDK candidates — all version/download/repo facts confirmed live
- `docs.rs/rmcp/2.2.0` (WebFetch this session) — confirmed `TokioChildProcess`, `serve_client`, `client`/`transport-child-process` feature flags
- `platform.claude.com/docs/en/agents-and-tools/tool-use/{overview,handle-tool-calls}` (WebFetch this session) — confirmed Messages API request/response shape, header requirements, tool_result ordering rules, current model-id naming convention
- Cargo Book "Environment Variables" (WebSearch this session) — confirmed `CARGO_BIN_EXE_<name>` scoping to tests/benches only

### Secondary (MEDIUM confidence)
- `slopcheck install rmcp reqwest --ecosystem crates.io` (run this session, host lacks `cargo` so the install step itself errored AFTER both packages were already scanned and returned `[OK]`) — the scan result itself is trustworthy; the tool's own install-attempt failure is an environment quirk, not a finding about the packages

### Tertiary (LOW confidence)
- None — every claim in this document traces to a primary or secondary source above; no unverified WebSearch-only claim was retained without cross-checking against crates.io/docs.rs/official docs

## Metadata

**Confidence breakdown:**
- Standard stack (rmcp client, reqwest, base64): HIGH — all versions/features verified live against crates.io and docs.rs this session
- Architecture / batched live-gate inventory: HIGH — directly enumerated from STATE.md/ROADMAP.md/12-07-PLAN.md source text, cross-checked against each phase's own "deferred to Phase 15" notes
- Pitfalls (CARGO_BIN_EXE_, tool_result ordering, Azure run-command size limit): HIGH — the first two independently verified against the Cargo Book and official Anthropic docs this session; the third is a directly-cited prior finding from this project's own STATE.md
- Live-execution outcomes (actual timings, actual DACL API call shape, actual model behavior in the capstone): LOW by nature — cannot be verified until the batched session actually runs, same as every prior live-gate phase in this project

**Research date:** 2026-07-11
**Valid until:** ~14 days for the Anthropic model-id/API specifics (fast-moving); ~30 days for the Rust/Cargo mechanics and the batched live-gate inventory itself (stable, but re-check against STATE.md if Phase 12/13/14 gain any further plans before Phase 15 executes)
