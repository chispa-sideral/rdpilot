# Stack Research

**Domain:** Rust consumer-surface additions to an existing IronRDP-based RDP perception SDK — persistent session daemon, CLI, dual-surface MCP server, bidirectional file transfer
**Researched:** 2026-07-10
**Confidence:** HIGH (all core version claims verified live against crates.io API and Context7/official docs on research date)

This file covers ONLY the NEW v1.1 additions. The existing SDK core (IronRDP, tokio, rustls, `windows`, `uiautomation`, serde) is fixed and already validated in CLAUDE.md — it is referenced here only where a new v1.1 crate must interoperate with it.

## Recommended Stack

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `rmcp` | **2.2.0** (2026-07-08) | Official Rust MCP SDK — MCP server tool surface | The only actively maintained Rust MCP SDK; lives at `github.com/modelcontextprotocol/rust-sdk`, 86 releases, 3.6k stars, tokio-native. Verified via Context7 (`/websites/rs_rmcp_rmcp`) and crates.io API directly. Ships both `#[tool_router]`/`#[tool]` macros (fast to wire two parallel tool surfaces — Anthropic computer-use-compatible + rdpilot-native — as two `tool_router` impls or a dispatch layer) and hand-rolled `ServerHandler` for cases the macros don't cover. |
| `clap` | **4.6.1** | CLI argument parsing (derive API) | De facto standard Rust CLI framework; derive API keeps session-targeting flags (`--session <name>`) and per-verb subcommands (`connect`, `list`, `disconnect`, `screenshot`, `input`, `launch`, `upload`, `download`) declarative and typed. Verified via Context7 (`/clap-rs/clap`) and crates.io API. |
| `interprocess` | **2.4.2** | Cross-platform local IPC transport (daemon ↔ CLI/MCP-server) | Unifies Unix domain sockets and Windows named pipes behind one `local_socket` API (`GenericNamespaced`/`GenericFilePath`), with a first-class `tokio` feature (async local socket server/client, plus raw `named_pipe::tokio` for advanced Windows control). This directly satisfies the "local workstation may be Windows or Linux" cross-platform IPC requirement without hand-rolled `cfg(unix)`/`cfg(windows)` branching. Verified via Context7 (`/kotauskas/interprocess`) and crates.io API. |
| `tokio` | 1.52.3 (already in tree) | Async runtime | Already the SDK's runtime (per CLAUDE.md). New surfaces reuse it — no second runtime. Tokio also ships *native* `tokio::net::UnixListener`/`UnixStream` (unix, `net` feature) and `tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient}` (windows) — see Alternatives for when to prefer these over `interprocess`. |
| `serde` + `serde_json` | 1.0.228 / 1.0.150 (already in tree) | Wire format for the daemon's internal IPC protocol and for MCP tool schemas | Already the SDK's serialization stack. Reused for both the custom daemon⇄client protocol (see Supporting Libraries) and `rmcp`'s `schemars`-derived tool-parameter JSON Schemas. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tokio-util` (codec) | 0.7.18 | Message framing over the raw IPC byte stream | `LengthDelimitedCodec` (or a small custom codec) frames JSON request/response messages over the `interprocess`/tokio duplex stream so the daemon protocol doesn't need to invent its own delimiter/escaping scheme. Pairs with `serde_json` for a minimal internal RPC — see "What NOT to Use" for why this beats pulling in a full JSON-RPC crate for this internal, single-writer-per-connection channel. |
| `schemars` | 1.2.1 | JSON Schema derivation for MCP tool parameters | Required by `rmcp`'s `#[tool]` macro (`schemars` feature) to auto-generate the `inputSchema` each tool advertises to an MCP client. Also directly reusable for hand-describing the Anthropic computer-use tool schema (`action`, `coordinate`, `text` fields) since that tool's JSON Schema is fixed by Anthropic's spec and just needs a matching Rust struct. |
| `directories` | 6.0.0 | Cross-platform standard paths (config dir, runtime/socket dir, data dir) | Locates the daemon's config file, the IPC socket/named-pipe path, log files, and any session-registry persistence in the OS-idiomatic location (XDG dirs on Linux, Known Folders on Windows) rather than hardcoding paths. Small, stable, single-purpose — no runtime dependencies beyond `cfg`. |
| `config` | 0.15.25 | Layered configuration (file → env → CLI flags) | Matches the "Layered connection config: config file + env + flags" requirement almost verbatim — it is explicitly a "layered configuration system," supporting TOML/YAML/JSON file sources merged with environment-variable overrides, with the final CLI-flag layer applied on top by `clap` parsing into the same struct (`serde::Deserialize`). ~3x the crates.io downloads of the closest alternative (`figment`, see Alternatives) and simpler for this project's needs (no builder DSL beyond source layering). |
| `thiserror` | 2.0.18 | Structured error types for the new daemon/CLI/MCP crates | Keeps error variants explicit and matches whatever error-handling convention the existing `crates/rdpilot` library already uses (CLAUDE.md notes strict lint gates — no `unwrap`/`expect` in library code); use in any new library-shaped crate (daemon core, IPC protocol crate). |
| `anyhow` | 1.0.103 | Ergonomic error propagation in binary-shaped code | Use only in the CLI binary and MCP server binary entry points (`main.rs`) where errors terminate the process — not inside library code, consistent with the existing project's `thiserror`-for-libraries / `anyhow`-for-binaries split convention common in Rust CLI tooling. |
| `tracing` + `tracing-subscriber` | 0.1.44 / 0.3.23 | Structured logging for the long-lived daemon process | The daemon is a background process with no attached terminal by default — structured, leveled logs (to a file via `directories`-located log path, or stderr when run in foreground/debug mode) are the only visibility into it. `tracing` also composes cleanly with `rmcp`'s own internal tracing spans (rmcp is tracing-instrumented). |
| `indicatif` | 0.18.6 | CLI progress bars for file transfer | Human-facing file upload/download progress in the CLI surface (bytes transferred / ETA). Not needed on the MCP server side (no TTY) — gate behind the CLI binary only. |
| `windows-sys` | 0.61.2 | Low-level Windows API bindings, if daemon detach needs manual process-creation flags | Only needed if the daemon-start path is implemented via manual `CREATE_NO_WINDOW`/`DETACHED_PROCESS` flags on `std::process::Command` (see Stack Patterns by Variant) rather than an OS service. Thin, code-genned from Windows metadata (same family as the SDK's existing `windows` crate dependency) — no heavyweight abstraction added. |
| `nix` | 0.31.3 | Low-level Unix syscalls, if daemon detach needs manual `setsid`/double-fork | Unix-side mirror of the above — only pull in if hand-rolling detach instead of using a supervisor. Keep both `windows-sys` and `nix` additions minimal and behind `cfg(target_os)` — do not adopt a crate that bundles both platforms' logic with hidden defaults (see "What NOT to Use": `daemon-base`). |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| `clap_complete` (4.6.7) | Shell-completion generation for the CLI | Optional polish; low cost since already part of the `clap` family and versioned in lockstep. |
| `cargo-nextest` | Test runner for the growing multi-crate workspace (daemon, CLI, MCP server, IPC protocol) | Not a dependency, a dev-workflow tool; faster parallel test execution as the workspace grows past the single `crates/rdpilot` crate. |

## Installation

```bash
# MCP server crate
cargo add rmcp --features server,macros,schemars,transport-io

# CLI crate
cargo add clap --features derive
cargo add clap_complete   # optional, shell completions
cargo add indicatif

# Daemon IPC transport (shared by daemon + CLI + MCP-server-as-client)
cargo add interprocess --features tokio

# Wire framing + config + paths (shared workspace crate, e.g. `rdpilot-daemon-protocol`)
cargo add tokio-util --features codec
cargo add serde_json
cargo add schemars
cargo add directories
cargo add config
cargo add thiserror
cargo add tracing tracing-subscriber

# Binary entry points only (daemon main.rs, cli main.rs, mcp-server main.rs)
cargo add anyhow

# Platform-specific detach (only if hand-rolling daemon backgrounding — see Stack Patterns)
cargo add windows-sys --target 'cfg(windows)'
cargo add nix --target 'cfg(unix)' --features process
```

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|--------------------------|
| `rmcp` (official SDK) | Hand-rolled JSON-RPC 2.0 over stdio | Never for this project — `rmcp` already implements the full MCP handshake (`initialize`, capability negotiation, `tools/list`, `tools/call`), schema generation, and both stdio and streamable-HTTP transports; reimplementing it buys nothing and risks protocol drift as MCP itself evolves. |
| `interprocess` (`local_socket`, unified API) | Raw `tokio::net::UnixListener` (unix) + `tokio::net::windows::named_pipe` (windows) behind manual `cfg` | Use tokio-native primitives directly if the daemon protocol needs Windows-named-pipe-specific features `interprocess` doesn't expose cleanly (e.g. per-client pipe security descriptors/ACLs, exact `PIPE_TYPE_MESSAGE` framing semantics) — `interprocess` wraps these but the abstraction leaks less friction if you go straight to `tokio`'s own module when you need OS-specific control. For this project's needs (one daemon, local-only clients, simple request/response), the unified API is worth the small abstraction cost. |
| `config` (layered config) | `figment` | `figment` has a cleaner `Provider`-trait-based merge model and nicer error messages, and is a fine choice if the config surface grows complex (e.g. profiles, nested provider composition). At v1.1's scope (host, credentials, socket path — three flat layers: file/env/flags) `config`'s simplicity and 3x larger install base make it the safer default. |
| Custom framed JSON protocol (`tokio-util` codec + `serde_json`) for daemon⇄client IPC | `jsonrpsee` (0.26.0) | Use `jsonrpsee` only if the daemon protocol needs to be exposed as a *standardized, externally-documented* JSON-RPC 2.0 API (e.g. multiple independent client implementations, or a future network-exposed daemon). It is pre-1.0 (working toward a stable v1.0, per its own changelog — expect breaking changes), pulls in a large transitive dependency tree (hyper/tower-adjacent tooling for its HTTP/WS transports even when unused), and is overkill for a single first-party CLI/MCP-server client talking to a local daemon over one IPC channel. |
| Manual `Command`-based daemon start/detach (`std::process::Command` + `windows-sys`/`nix` flags) | `daemon-base`, `cross-platform-service` | `daemon-base` is a very new, low-adoption crate (found via web search only, not independently verified in Context7 or by download volume) claiming unified cross-platform daemon lifecycle — too immature to trust for a solo-author personal-tooling project; `cross-platform-service` pulls in D-Bus/systemd-unit-file machinery on Linux, which is real OS-service integration overkill for a background process the same user starts and stops via the CLI (`rdpilot daemon start`/`stop`). Prefer the "CLI spawns a detached child process of itself" pattern (see Stack Patterns by Variant) — it's simpler, has zero exotic dependencies, and matches the project's "personal tooling first" stance in CLAUDE.md. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `jsonrpsee` for the daemon⇄CLI/MCP-server internal protocol | Pre-1.0 with acknowledged upcoming breaking changes toward v1.0; designed for network-facing multi-client JSON-RPC (HTTP/WS/TCP) — heavyweight for one local, first-party client talking to one local daemon | `tokio-util::codec::LengthDelimitedCodec` + `serde_json` structs for a minimal internal request/response protocol |
| `daemonize` crate for cross-platform daemon start | Unix-only (fork/`setsid`/PID-file), has no Windows story at all — directly contradicts the "local workstation may be Windows or Linux" requirement | Manual detached-spawn: `std::process::Command::new(current_exe()).spawn()` with `DETACHED_PROCESS`/`CREATE_NO_WINDOW` flags (windows-sys) on Windows, and `setsid`-style detach (nix) on Unix, gated behind `cfg(target_os)` |
| `axum`/`warp`/`actix-web` for the daemon's local IPC surface | Full HTTP frameworks — daemon⇄client is a local, single-purpose duplex stream, not an HTTP service; pulling in a web framework (routing, middleware, TLS stack) for this is pure overhead and a large unnecessary dependency surface | `interprocess` local socket + `tokio-util` codec, as above |
| `tonic`/gRPC for the daemon protocol | Same overkill problem as HTTP frameworks, plus adds a `protoc`/build-time codegen dependency the project doesn't otherwise need | Same as above — plain framed JSON over the local socket is sufficient for a single local client type |
| Rolling a bespoke MCP JSON-RPC implementation | MCP's wire protocol has subtle handshake/capability-negotiation and content-block semantics (per the official spec); rmcp already tracks spec revisions (evidenced by its `1.8.0`→`2.x` deprecation of `enable_sampling_tools` per SEP-2577) | `rmcp` (official SDK) |
| `ironrdp-cliprdr`/CLIPRDR as the PRIMARY bidirectional file-transfer channel for this milestone | CLIPRDR (MS-RDPECLIP) file transfer is modeled on clipboard copy/paste semantics with delayed rendering and format-list negotiation — a UX-shaped, single-shot "paste" primitive, not a general-purpose scripted read/write API; awkward to drive deterministically for arbitrary upload/download verbs from CLI/MCP tool calls | RDPDR (drive/device redirection, MS-RDPEFS) — see File Transfer Transport section below; reserve `ironrdp-cliprdr` for the *deferred* clipboard-text-sync feature already tracked separately in PROJECT.md |

## Stack Patterns by Variant

**If the daemon needs to survive the CLI process exiting (the actual v1.1 requirement — "long-lived background service"):**
- Use a two-step CLI command: `rdpilot daemon start` spawns `std::process::Command::new(current_exe()).arg("daemon").arg("run").spawn()` with output redirected to a log file and platform detach flags (`DETACHED_PROCESS | CREATE_NO_WINDOW` on Windows via `windows-sys`; `setsid` via `nix` on Unix), then exits immediately; `daemon run` is the actual long-lived process.
- Because this needs zero new "service manager" abstraction crates, matches "personal tooling first," and composes naturally with "is the daemon already running?" liveness checks done simply by attempting to connect to the IPC socket/pipe (no separate PID-file or single-instance crate needed — connection failure IS the "not running" signal).

**If the CLI, MCP server, AND daemon are three separate binaries (matches "MCP server surface ... a daemon client" and "CLI ... thin client over the daemon"):**
- Put the IPC wire protocol (request/response enums, `serde` derives, the `tokio-util` codec setup) in a small shared library crate (e.g. `rdpilot-daemon-protocol`) that both the CLI and MCP-server binaries depend on alongside `interprocess`.
- Because duplicating the protocol enum and framing logic across two client binaries is a correctness risk (protocol drift) for negligible savings — this is exactly the kind of internal-only crate boundary that costs nothing and prevents a real bug class.

**If dual MCP tool surfaces (Anthropic computer-use-compatible + rdpilot-native) need to share one daemon-client core:**
- Register both tool sets on the same `rmcp` `ServerHandler`/`tool_router` (two `#[tool_router]` impls composed, or one router with both tool groups registered) rather than running two separate MCP server processes.
- Because a single MCP server process is what a client (e.g. Claude Desktop, or a custom harness) configures once; splitting into two processes doubles daemon-connection bookkeeping and process lifecycle for no isolation benefit — the tool *names/schemas* are what differentiate the two surfaces, not the transport.

## File Transfer Transport — Options, Not a Premature Decision

Bidirectional file transfer needs a channel from the local workstation into the remote Windows target's filesystem. Three RDP-family options exist; do not commit to one without validating against the already-built sensor-deployment path (SENSOR-02: "RDPDR primary, WinRM fallback," both already live-verified in v1.0):

| Option | Crate | What it is | Fit for scripted bidirectional file transfer |
|--------|-------|-------------|----------------------------------------------|
| **RDPDR device/drive redirection (MS-RDPEFS)** | `ironrdp-rdpdr` (0.7.0 current; verify exact version already vendored in `crates/rdpilot`, per CLAUDE.md's note that IronRDP crate versions are non-uniform across the workspace) | Presents the local machine (or a virtual store) as a redirected drive/device the remote OS's own file system driver reads/writes through IRP-style requests | **Best fit.** Already integrated in v1.0 to push the sensor executable onto the target (SENSOR-02) — this is the same mechanism, generalized from "push one known binary" to "arbitrary named-path upload/download." Gives real file-system read/write semantics (chunked, arbitrary size) with no clipboard-format negotiation in the way. Recommended primary path. |
| **CLIPRDR clipboard file transfer (MS-RDPECLIP)** | `ironrdp-cliprdr` (0.7.0 current) | Clipboard copy/paste of files via delayed rendering and format-list negotiation | Poor fit as the *primary* mechanism for this milestone: modeled on a human copy/paste gesture, not a scripted request/response verb; format negotiation and delayed-render callbacks add complexity for no benefit when the caller (CLI/MCP tool) already knows exact source/destination paths. Keep reserved for the separately deferred clipboard-text-sync feature noted in PROJECT.md's Out of Scope list. |
| **WinRM/PowerShell (base64-chunked file copy)** | existing WinRM plumbing from SENSOR-02 (out-of-band fallback) | `Invoke-Command`/`Copy-Item` style transfer, or manual base64 chunking through PowerShell remoting | Fallback-only, mirroring the sensor deployment pattern: works when RDPDR isn't available/negotiated, but base64 encoding adds ~33% size inflation and per-invocation overhead unsuitable as the primary path for larger files. Reuse the same "RDPDR primary, WinRM fallback" shape already validated for sensor bootstrap rather than inventing a third pattern. |

**Recommendation for planners:** default to extending the already-built RDPDR integration (generalize from "deploy sensor.exe" to "upload/download arbitrary named paths"), with WinRM as the fallback transport mirroring the existing SENSOR-02 pattern. Do not add `ironrdp-cliprdr` for file transfer in this milestone — evaluate it only if/when the deferred clipboard-sync feature is scoped.

## Version Compatibility

| Package A | Compatible With | Notes |
|-----------|-----------------|-------|
| `rmcp` 2.2.0 | `tokio` 1.x (already in tree), `schemars` 1.x | `rmcp`'s `schemars` feature expects the `schemars` 1.x derive API (not 0.8.x) — verify the version resolved into the workspace matches `schemars` 1.2.1, since `schemars` had a breaking 0.8→1.0 API change. |
| `interprocess` 2.4.2 | `tokio` 1.x (already in tree) via its `tokio` feature | No conflicting async-runtime assumptions — `interprocess` treats tokio as opt-in, so enabling only the `tokio` feature avoids pulling in its (also-supported) blocking API surface unless explicitly used. |
| `ironrdp-rdpdr` / `ironrdp-cliprdr` (0.7.0 latest as of this research) | The workspace's already-pinned `ironrdp*` crate versions | CLAUDE.md flags that IronRDP crate versions are already non-uniform across the workspace (a v1.0 Phase 2 correction) and that the top-level `ironrdp` meta-crate has moved to 0.16.0 upstream since the SDK was built. Do not blanket-bump; confirm the exact `ironrdp-rdpdr`/`ironrdp-cliprdr` versions compatible with whatever `ironrdp-session`/`ironrdp-connector` versions are currently pinned before adding either crate, the same way Phase 2 Plan 01 had to reconcile versions originally. |
| `config` 0.15.25 | `serde` 1.x (already in tree) | Deserializes into a plain `#[derive(Deserialize)]` struct shared with `clap`'s parsed args struct — no special glue crate needed to merge the final CLI-flag layer on top of file/env layers; do it by hand (file/env parse → struct, then apply `Some(cli_value)` overrides field by field, or use `clap`'s `default_value_t` sourced from the merged config). |

## Sources

- Context7 `/websites/rs_rmcp_rmcp` — rmcp tool macros, `tool_handler`, transport feature flags
- Context7 `/clap-rs/clap` — derive API, subcommand patterns
- Context7 `/kotauskas/interprocess` — `local_socket` unified API, Windows named-pipe Tokio server/client examples
- crates.io API (`https://crates.io/api/v1/crates/<name>`) — live version verification for: `rmcp` 2.2.0, `clap` 4.6.1, `interprocess` 2.4.2, `tokio` 1.52.3, `tokio-util` 0.7.18, `schemars` 1.2.1, `directories` 6.0.0, `config` 0.15.25, `figment` 0.10.19, `thiserror` 2.0.18, `anyhow` 1.0.103, `tracing` 0.1.44, `tracing-subscriber` 0.3.23, `indicatif` 0.18.6, `windows-sys` 0.61.2, `nix` 0.31.3, `jsonrpsee` (via web, 0.26.0 pre-1.0), `ironrdp` 0.16.0, `ironrdp-rdpdr` 0.7.0, `ironrdp-cliprdr` 0.7.0, `ironrdp-dvc` 0.8.0 — HIGH confidence, checked 2026-07-10
- [modelcontextprotocol/rust-sdk GitHub](https://github.com/modelcontextprotocol/rust-sdk) — 86 releases, 3.6k stars, `rmcp-v2.2.0` tag (2026-07-08) — MEDIUM-HIGH confidence (WebFetch-summarized, cross-checked against crates.io version)
- [rmcp README](https://github.com/modelcontextprotocol/rust-sdk/blob/main/crates/rmcp/README.md) — feature-flag inventory (server/client/macros/transport-io/transport-child-process/transport-streamable-http-*), TLS backend defaults (rustls via reqwest) — MEDIUM confidence (WebFetch summary of README, not directly machine-verified)
- [tokio docs: `tokio::net::windows::named_pipe`](https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/index.html) — confirms tokio-native Windows named-pipe support exists as an `interprocess` alternative — HIGH confidence
- WebSearch: `jsonrpsee` maturity/version (0.26.0, pre-1.0, per its own changelog) — MEDIUM confidence, single-source but corroborated by the crate's own README framing ("working towards v1.0")
- WebSearch: `ironrdp-rdpdr`/`ironrdp-cliprdr` capability descriptions (MS-RDPEFS drive redirection vs MS-RDPECLIP clipboard file transfer with delayed rendering) — MEDIUM confidence, cross-checked against crates.io descriptions and the IronRDP docs site
- WebSearch: `daemon-base`/`cross-platform-service` — LOW confidence (single-source, low adoption signal), used only to justify the "avoid" recommendation, not as a positive recommendation
- CLAUDE.md (project file) — existing pinned stack (IronRDP 0.14.0-era, tokio, rustls, `windows` 0.58.x, `uiautomation` 0.22.0) and the Phase 2 non-uniform-versioning note that informs the Version Compatibility caveat above

---
*Stack research for: rdpilot v1.1 consumer surfaces (session daemon, CLI, dual-surface MCP server, bidirectional file transfer)*
*Researched: 2026-07-10*
