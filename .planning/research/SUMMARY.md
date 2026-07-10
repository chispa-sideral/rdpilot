# Project Research Summary

**Project:** rdpilot — v1.1: Consumer Surfaces & File Transfer
**Domain:** Rust CLI/daemon/MCP consumer surfaces over an existing IronRDP-based RDP-perception SDK
**Researched:** 2026-07-10
**Confidence:** HIGH

## Executive Summary

v1.1 turns the v1.0 SDK (`Session`, DVC sensor, RDPDR drive backend — all DONE and treated as fixed ground truth) into three new consumer-facing binaries — a long-lived session daemon, a thin CLI, and a dual-tool-surface MCP server — plus bidirectional file transfer. None of this requires touching `Session`'s core threading model: `Session` is already `Send + Sync` with plain `async fn(&self, ...)` methods (a deliberate v1.0 design choice, confirmed directly from `session.rs`), so the daemon can hold N `Arc<Session>` in an ordinary multi-thread Tokio runtime registry and call them exactly as any other async code would. The stack additions are all mature, current, and low-risk: `rmcp` 2.2.0 (official Rust MCP SDK) for the dual tool surface, `interprocess`/tokio for cross-platform local IPC (with an explicit carve-out to bypass it on the Windows leg for DACL control), `clap` for the CLI, and `config` for layered file/env/flag configuration. File transfer is not a new transport — it is a generalization of the already-proven RDPDR drive-redirection channel (used in v1.0 to deploy the sensor exe) from "one hardcoded read-only file" to an allow-listed, bidirectional set of named transfers, paired with two new sensor `MsgType`s (`UploadFile`/`DownloadFile`) that trigger the actual remote-side copy — deliberately avoiding both CLIPRDR (wrong semantic model, clipboard-shaped) and the fragile Win+R keystroke-injection hack used once in v1.0 for bootstrapping.

The single biggest engineering lift disguised as "just add file transfer" is generalizing `RdpilotDriveBackend` to support `DeviceWriteRequest`, which the current backend explicitly rejects — this is precisely the code shape where FreeRDP shipped a real, disclosed path-traversal CVE (`contains_dotdot()` off-by-one on a trailing `..` with no separator), so canonicalized-path validation with a specific adversarial test suite is a hard, non-negotiable gate on this milestone, not generic "sanitize inputs" advice. The second major risk cluster is entirely new to this milestone because nothing in v1.0 was long-lived: a persistent daemon introduces session-registry correctness (atomic insert to avoid orphaned duplicate connections), thread-per-session lifecycle cleanup (soak-testable leaks), credential-redaction gaps at the IPC/serialization boundary (distinct from the already-solved `Debug`-redaction problem), local-IPC authentication (unauthenticated sockets/pipes let any local user hijack a live remote session), and daemon-crash/restart reconciliation against Windows' own reconnect-to-disconnected-session semantics (already empirically observed once in v1.0's sensor deployment).

The recommended roadmap therefore sequences by hard dependency, not by feature prominence: `rdpilot` SDK extensions (RDPDR write support + sensor upload/download commands) must land first since everything else needs a working transfer primitive to build on top of; `rdpilot-ipc`/`rdpilot-config` (shared wire protocol, session-identity schema, layered config) come next because both the daemon and its two clients need identical types; the daemon itself (registry, lifecycle, auth) is the true foundation both consumer surfaces sit on; CLI and MCP server can then be built roughly in parallel as thin daemon clients; and per-surface proof harnesses plus the live-LLM capstone demo close the milestone. Each phase has an explicit, named pitfall it must resolve as a success criterion — not a "nice to have" — most critically the RDPDR path-traversal test suite (file-transfer phase) and the connect/disconnect soak test plus `kill -9`-and-restart reconciliation test (daemon phase).

## Key Findings

### Recommended Stack

Every new v1.1 crate reuses the existing async/serialization backbone (`tokio`, `serde`/`serde_json`, already in tree) rather than introducing a second runtime or wire format. The daemon/CLI/MCP split needs exactly one new shared-protocol crate and one new config crate; everything else is per-binary tooling.

**Core technologies:**
- `rmcp` 2.2.0 — official Rust MCP SDK (tokio-native, `#[tool_router]`/`#[tool]` macros) — the only actively maintained Rust MCP implementation; avoids hand-rolling MCP's handshake/schema/content-block semantics
- `interprocess` 2.4.2 (`tokio` feature) — unified Unix-socket/Windows-named-pipe local IPC API for daemon↔CLI/MCP — **except** on the Windows leg for the daemon's actual listener, where `tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw` must be used directly instead, because `interprocess` does not appear to expose DACL configuration and default Windows named-pipe security descriptors grant Everyone/anonymous read access
- `clap` 4.6.1 (derive) — CLI argument parsing; de facto standard, keeps `--session <name>` and per-verb subcommands typed and declarative
- `config` 0.15.25 — layered file→env→flags configuration; ~3x the install base of `figment` and matches the milestone's flat 3-layer scope (host/creds/socket path) without a builder DSL
- `tokio-util` (codec, `LengthDelimitedCodec`) + `serde_json` — the daemon's own hand-rolled internal wire protocol; explicitly **not** `jsonrpsee` (pre-1.0, network-multi-client-shaped, unnecessary weight) or any HTTP/gRPC framework (`axum`/`tonic` — wrong shape entirely for a local single-client-type duplex stream)
- RDPDR (`ironrdp-rdpdr`, already partially wired via `RdpilotDriveBackend`) — the file-transfer transport, generalized from one hardcoded file to a `Write`-IRP-capable allow-listed set; **not** `ironrdp-cliprdr`/CLIPRDR, which is clipboard-paste-shaped and reserved for the separately deferred clipboard-sync feature

**Supporting:** `schemars` 1.2.1 (MCP tool JSON-schema derivation), `directories` 6.0.0 (standard config/socket/log paths), `thiserror`/`anyhow` (library/binary error-handling split, matching existing convention), `tracing`/`tracing-subscriber` (daemon has no attached terminal by default), `indicatif` (CLI-only transfer progress bars).

**Explicit anti-recommendations:** `daemonize` (Unix-only, contradicts cross-platform requirement), `daemon-base`/`cross-platform-service` (unverified/immature or D-Bus/systemd overkill), `axum`/`tonic`/`jsonrpsee` for internal daemon IPC (all wrong-shaped for one local client type), reusing the Win+R keystroke-injection hack for file transfer (fragile by construction, unnecessary once the sensor has a typed request/reply channel).

### Expected Features

**Must have (table stakes):**
- Full Anthropic computer-use action vocabulary, exposed as MCP tool(s) — but as **one schema-discriminated `computer` mega-tool** (an `action` enum parameter), not 16+ separate MCP tools, mirroring Anthropic's own single-tool reference design and avoiding `tools/list` bloat when combined with rdpilot-native tools (would otherwise be 25+ tools total)
- Session lifecycle CLI verbs (`connect`/`list`/`disconnect`) and explicit session targeting on every command — **no implicit default/last-used session**, already a hard PROJECT.md decision
- rdpilot-native perception/control tools (`world_state`, `get_uia_tree`, `get_window_list`, `get_process_tree`, `launch_process`, `set_foreground_window`) exposed as MCP **Tools, not Resources** — the model must actively decide when to re-query, which Resources (host-attached/subscribed) don't reliably support across MCP clients
- File `put`/`get` on both CLI and MCP, returning **only `{path, bytes_transferred, checksum}` metadata — never inline base64** for non-trivial files (verified real-world MCP client truncation/corruption in the ~10–12KB range)
- Default no-clobber file-transfer semantics with an explicit overwrite flag; layered connection config; daemon auto-discovery/auto-start on first client command

**Should have (competitive):**
- Dual tool surface (computer-use-compatible + rdpilot-native) in one MCP server process — no known prior art combines both
- `world_state` as one correlated MCP tool (screenshot + windows + UIA in one coordinate space) — cheapest differentiator to ship, already latency-profiled at 23-73ms in v1.0
- Durable, user-chosen session naming that survives daemon restarts (registry persistence of name→config, not necessarily the live socket)

**Defer (v2+):**
- MCP `notifications/progress` for transfer progress (add only once put/get are observed slow enough to need it)
- `zoom` action (lower priority here specifically because the UIA tree already gives exact-text grounding pure-vision computer-use setups rely on `zoom` for)
- MCP Prompts primitive; file sync/watch-folder mirroring (explicit anti-feature — scope creep, conflict-resolution/race complexity with no stated justification)

### Architecture Approach

The daemon owns exactly one new state concept — a `SessionId → Arc<Session>` registry inside an ordinary multi-thread Tokio runtime — while every `Session`'s existing dedicated-OS-thread-plus-current-thread-runtime design (a Phase 2 v1.0 constraint, not up for revisiting) stays completely untouched and opaque to the daemon. `rdpilot-ipc` is deliberately its own crate (not folded into the daemon binary) so both server and client binaries share one protocol definition without the CLI/MCP depending on daemon-only code; `rdpilot-config` is separate again because config layering is a CLI/MCP-only concern (the daemon only ever receives an already-resolved connect payload).

**Major components:**
1. `rdpilot-ipc` (new) — shared wire protocol (Request/Response enums, `SessionId` newtype, length-delimited JSON framing), transport bootstrap (connect-or-autostart), Windows-DACL-aware pipe construction
2. `rdpilot-daemon` (new bin) — session registry (atomic-insert, idle reaper, disk-persisted minimal state for crash reconciliation), IPC accept loop, per-connection dispatch to `Session` methods directly (no adapter layer needed — `Session` is already `Send + Sync`)
3. `rdpilot-cli` / `rdpilot-mcp` (new bins) — thin invoke-and-exit / long-lived stdio `rmcp` clients respectively, both over the identical `rdpilot-ipc` protocol; MCP maps tool calls (including the `computer` mega-tool and native tools) onto IPC requests over one persistent connection per process lifetime
4. `crates/rdpilot` (extended, not rebuilt) — `RdpilotDriveBackend` generalized to a registered allow-list with `Write` IRP support; sensor gains `UploadFile`/`DownloadFile` `MsgType`s mirroring the existing `LaunchProcess` request/reply shape

File-transfer data flow in both directions: CLI/MCP → daemon → `Session::upload_file`/`download_file` → register a per-transfer entry in `RdpilotDriveBackend` (client-side, in-process) → `sensor_request(UploadFile/DownloadFile)` triggers the remote-side `File.Copy`-equivalent through the RDPDR-redirected share → sensor replies success → SDK deregisters/relocates the staged file. Both directions reuse the identical passive-RDPDR-transport-plus-active-sensor-trigger split; no surface-specific transfer logic exists outside `Session`.

### Critical Pitfalls

1. **RDPDR write path-traversal (CVE-shaped)** — extending `RdpilotDriveBackend` to accept `DeviceWriteRequest` reopens exactly the bug class disclosed in FreeRDP (`contains_dotdot()` off-by-one on trailing `..` with no separator) and in Windows' own RDP client (CVE-2025-48817). Avoid by canonicalizing the full resolved path and verifying ancestry under the share root — never substring-matching `..` — with unit tests specifically covering trailing-no-separator, mixed-separator, and absolute-path-as-relative inputs.
2. **`Serialize` does not inherit `Debug` redaction** — v1.0's `ConnectionConfig` password redaction is a hand-written `Debug` impl (D-14); a naive IPC/MCP DTO reusing that struct's shape with `#[derive(Serialize)]` puts the plaintext password on the wire and potentially into MCP tool-result content, even though `{:?}` still looks correctly redacted everywhere else. Avoid with a distinct, credential-free status DTO for every IPC/MCP response and a boundary test that serializes every response type and greps for a planted secret.
3. **Computer-use screenshot/coordinate scaling bridge** — Anthropic's computer-use convention expects XGA/WXGA-downscaled screenshots, but rdpilot's native 96-DPI physical-pixel contract is a *second, independent* coordinate space; failing to bridge them produces "looks fine, clicks slightly off" bugs that erode trust in the live-LLM demo. Fix with one fixed advertised resolution, a single tested `scale_to_native(x, y)` function, and a round-trip precision test near screen edges/corners (not just center).
4. **Daemon-crash session orphaning** — Windows treats an RDP connection drop as a disconnect, not a logoff (already empirically observed in v1.0's sensor deployment); an in-memory-only registry means a `kill -9`'d and restarted daemon has zero knowledge that a durable Windows-side session (and possibly a live sensor) still exists. Fix with minimal disk-persisted registry state and startup reconciliation ("possibly still live, reconnect to confirm") rather than silent amnesia.
5. **MCP tool call blocks the event loop on slow RDP round trips** — v1.1 introduces genuinely long operations (large file transfer, slow `launch_process`, a silently-dying connection retrying) that v1.0's sub-second-everything profile never exercised; an inline-await tool-call handler makes one slow call block all others, including unrelated fast ones. Fix with per-call task isolation, bounded explicit timeouts, and MCP progress notifications for file transfer.
6. **No-implicit-default session as a required schema field, not a CLI convention** — the realistic violation path is convenience creep ("default to the only session") during CLI ergonomics work, which is exactly the wrong failure mode for an AI agent operating multiple sessions. Fix by making session id a required, non-optional field at the wire-protocol/MCP-schema level (a hard validation error, not a fallback), verified by a test that omits it on every verb and asserts rejection.

## Implications for Roadmap

Based on research, suggested phase structure:

### Phase 1: SDK File-Transfer Extension
**Rationale:** Every consumer surface (daemon, CLI, MCP) and the milestone's proof harnesses depend on a working upload/download primitive existing in `crates/rdpilot` first; building the daemon before this exists means building on a mocked capability that has to be retrofitted later.
**Delivers:** `RdpilotDriveBackend` generalized to an allow-listed, per-transfer registry with `DeviceWriteRequest` support; sensor `MsgType::UploadFile`/`DownloadFile` mirroring the existing `LaunchProcess` request/reply shape; `Session::upload_file`/`download_file` public methods.
**Addresses:** Bidirectional file transfer (FEATURES.md table stakes); reuses the already-proven RDPDR channel rather than CLIPRDR or keystroke injection (ARCHITECTURE.md Pattern 4, Anti-Patterns 2/3).
**Avoids:** Pitfall 8 (RDPDR write path-traversal) — this phase's success criterion must include the adversarial path-traversal test suite (trailing `..` no separator, mixed separators, absolute-as-relative), not just "normal files transfer correctly."

### Phase 2: Shared Wire Protocol & Config (`rdpilot-ipc`, `rdpilot-config`)
**Rationale:** Both the daemon and its two clients (CLI, MCP) need identical Request/Response types, session-identity schema, and framing before either consumer binary can be written — defining this once, in its own crate, avoids protocol drift between two independently-built client binaries.
**Delivers:** `rdpilot-ipc` (Request/Response enums, `SessionId` newtype, `LengthDelimitedCodec` + `serde_json` framing, `WireError` adapter for `rdpilot::Error`, Windows-DACL-aware pipe construction helper, `ensure_daemon_running()` bootstrap); `rdpilot-config` (layered file/env/flag resolution into a connect payload).
**Uses:** `interprocess`/tokio (with the Windows-DACL carve-out via raw `tokio::net::windows::named_pipe`), `tokio-util` codec, `config`, `schemars`/`serde_json`.
**Implements:** ARCHITECTURE.md Pattern 3 (IPC as thin IDL over `Session`'s existing async methods).
**Avoids:** Pitfall 2 (implicit default session) by making `session: SessionId` a required, non-optional field in every session-scoped variant at the schema level; Pitfall 4 (Serialize-doesn't-inherit-Debug-redaction) by defining credential-free DTOs from the start, with a grep-for-secret boundary test as an explicit success criterion; Pitfall 5 (unauthenticated local IPC) via peer-uid checks (Unix) / explicit DACL construction (Windows), verified with a different-uid-client-rejected test.

### Phase 3: Session Daemon
**Rationale:** The daemon is the true foundation both CLI and MCP sit on ("thin clients over the daemon" per PROJECT.md); it must exist with a correct, tested registry and lifecycle before either consumer surface is built against it, since retrofitting registry correctness after two clients depend on it is expensive.
**Delivers:** `rdpilot-daemon` binary — atomic-insert session registry (`HashMap::entry()` or single-writer actor, not check-then-insert), per-session shutdown-signal-plus-join teardown (respecting `Session::close(self)`'s consuming signature via `Arc::try_unwrap`-or-drop), idle-session reaper + idle-daemon self-shutdown (sccache-style auto-start/backoff), disk-persisted minimal registry state for crash reconciliation.
**Addresses:** Session daemon + session identity (FEATURES.md/PROJECT.md table stakes).
**Avoids:** Pitfall 1 (session-per-thread leak) — soak test (N connect/disconnect cycles, thread/memory returns to baseline) is a named success criterion, not just "disconnect works once"; Pitfall 3 (registry check-then-insert race) — concurrency test firing N simultaneous same-name connects, asserting exactly one success; Pitfall 9 (daemon-crash orphaning) — explicit `kill -9`-mid-session-then-restart test confirming reconciliation reporting, not silent amnesia.

### Phase 4: CLI Surface
**Rationale:** Once the daemon and shared protocol exist, the CLI is a straightforward thin client — low implementation cost, high user value, and it can validate the daemon/IPC layer end-to-end before the more complex MCP surface is built on the same foundation.
**Delivers:** `rdpilot-cli` — `connect`/`list`/`disconnect` plus perception/input/launch/file verbs, explicit `--session` targeting, layered config resolution, `daemon start/stop/status` escape hatch, no-clobber-by-default file transfer with `--force`.
**Uses:** `clap` (derive), `indicatif` (transfer progress), `rdpilot-ipc`/`rdpilot-config`.

### Phase 5: MCP Server Surface
**Rationale:** Built after the CLI proves the daemon/IPC foundation works end-to-end; the MCP surface is the more complex consumer (dual tool routers, schema authoring, long-lived stdio connection, event-loop isolation) and benefits from a battle-tested daemon underneath it.
**Delivers:** `rdpilot-mcp` — single schema-discriminated `computer` tool (full Anthropic action vocabulary, hand-authored JSON schema, client-side `cursor_position` tracking since RDP doesn't report it), rdpilot-native tool set (`world_state`, `get_uia_tree`, `get_window_list`, `get_process_tree`, `launch_process`, `set_foreground_window`, session connect/list/disconnect, file put/get returning metadata only), per-tool-call task isolation with bounded timeouts.
**Uses:** `rmcp` 2.2.0 (`tool_router`/`tool` macros, `transport-io`), `schemars`.
**Avoids:** Pitfall 6 (computer-use screenshot/coordinate mismatch) — fixed advertised resolution + tested `scale_to_native` round-trip as a named success criterion; Pitfall 7 (MCP event-loop blocking) — per-call task isolation and bounded timeouts required before file-transfer tools are wired in, verified by a test asserting a slow tool call doesn't block a concurrent fast one.

### Phase 6: Proof Harnesses (Per-Surface + Live-LLM Capstone)
**Rationale:** This is the milestone's explicitly stated finish line and depends on every prior phase functioning end-to-end; sequencing it last matches PROJECT.md's "proven both by scripted per-surface harnesses and a live-LLM MCP demo."
**Delivers:** Scripted proof harness per surface (CLI, MCP) plus a capstone live-LLM demo driving a read/inspect + file-transfer task through the MCP surface.
**Addresses:** PROOF requirement (PROJECT.md Active requirements).

### Phase Ordering Rationale

- SDK file-transfer work must precede the daemon/CLI/MCP work because file transfer is a hard dependency of both consumer surfaces' full verb sets, and it is the one component that touches the deepest, riskiest existing code (`RdpilotDriveBackend`) — doing it first isolates that risk from the newer, less-proven daemon/IPC code.
- Shared protocol/config before the daemon (not alongside or after) because the daemon's registry and dispatch code is written *against* the protocol's types — defining session-identity-as-required-field and credential-free DTOs at the schema layer is far cheaper than retrofitting after the daemon and two clients already exist and would need coordinated changes.
- Daemon before CLI/MCP because both are explicitly "thin clients over the daemon" (PROJECT.md) — there is no scenario in which either consumer surface should be built or tested against a stub/mock daemon when the real one is next in the dependency chain anyway.
- CLI before MCP because it is the lower-complexity consumer (invoke-and-exit, no dual tool router, no schema authoring, no long-lived-connection concerns) and serves as a cheaper end-to-end validation of the daemon/IPC layer before the more complex MCP surface is layered on.
- Proof harnesses last because PROJECT.md defines them as validating the *combination* of everything else — building them earlier would only validate partial slices already covered by each phase's own success criteria.

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 1 (SDK File-Transfer Extension):** RDPDR `DeviceWriteRequest`/path-canonicalization is genuinely novel implementation work with a disclosed-CVE-shaped risk profile — worth a focused research/design pass on the exact IRP semantics and MS-RDPEFS chunk-size limits before coding.
- **Phase 3 (Session Daemon):** Windows named-pipe DACL construction (`create_with_security_attributes_raw` + a SID-scoped `SECURITY_ATTRIBUTES`) is flagged MEDIUM confidence in ARCHITECTURE.md (single security-research source corroborating the default-DACL risk) — verify against current Windows documentation at implementation time, not just this research pass.
- **Phase 5 (MCP Server Surface):** Hand-authoring the Anthropic computer-use JSON schema (no independent schema to import — Claude's model weights encode it) plus client-side `cursor_position` tracking are both "hidden" pieces of new engineering not covered by existing SDK wrappers; worth validating the exact parameter shape (`coordinate`, `start_coordinate`, `duration`, `scroll_direction`/`scroll_amount`, `region`) against the live claude-quickstarts reference during planning.

Phases with standard patterns (skip research-phase):
- **Phase 2 (Shared Wire Protocol & Config):** Length-delimited JSON framing over a local socket/pipe and layered file/env/flag config are both extremely well-established patterns (FEATURES.md/STACK.md cite `docker`/`kubectl`/`aws-cli` precedent) with HIGH-confidence library support already verified.
- **Phase 4 (CLI Surface):** `clap` derive-based subcommand CLIs are a well-worn Rust pattern; this phase is pure thin-adapter work over an already-designed protocol.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | All core version claims (`rmcp` 2.2.0, `clap` 4.6.1, `interprocess` 2.4.2, `config` 0.15.25, etc.) verified live against crates.io API and Context7 on research date |
| Features | HIGH (computer-use schema, MCP tool/resource conventions) / MEDIUM (CLI daemon UX, file-transfer semantics — verified against analogous tools, not an rdpilot-specific spec) | Split confidence explicitly carried over from FEATURES.md |
| Architecture | HIGH for integration points grounded directly in v1.0 source (`Session`, `session_loop`, `connect.rs`, `rdpdr_backend.rs`); MEDIUM for external library choices (`rmcp`, `interprocess`) verified via Context7/official docs | Windows named-pipe DACL claim specifically flagged MEDIUM (single security-research source) |
| Pitfalls | MEDIUM-HIGH | Grounded in this codebase's own v1.0 live-gate findings (thread-per-session design, Debug redaction precedent, Windows reconnect-to-disconnected-session behavior) plus verified external sources (disclosed FreeRDP/Windows CVEs, MCP community timeout discussions) |

**Overall confidence:** HIGH

### Gaps to Address

- Exact `ironrdp-rdpdr`/`ironrdp-cliprdr` version compatibility with the workspace's already-pinned (non-uniform, per Phase 2 v1.0 correction) `ironrdp-session`/`ironrdp-connector` versions must be reconciled before adding `Write` IRP support — do not blanket-bump (STACK.md Version Compatibility).
- `interprocess`'s actual (non-)support for Windows named-pipe ACL configuration is asserted from available documentation, not exhaustively confirmed — the recommended mitigation (bypass `interprocess` on the Windows listener leg, use raw `tokio::net::windows::named_pipe` directly) should be validated at Phase 3 implementation time, not assumed correct from this research pass alone.
- MS-RDPEFS per-IRP max chunk size (for streaming large-file writes without loading the whole file into memory, per PITFALLS.md's Performance Traps) needs a concrete number pulled from the spec during Phase 1 implementation — not resolved definitively in this research pass.
- The precise JSON schema Anthropic's computer-use tool expects (parameter names/types per action, since Claude's schema is encoded in model weights, not published as an importable artifact) should be re-verified against the live claude-quickstarts reference implementation at Phase 5 implementation time, since it is API-version-dependent (`computer_20241022`/`computer_20250124`/`computer_20251124`).

## Sources

### Primary (HIGH confidence)
- Context7 `/websites/rs_rmcp_rmcp` — rmcp tool macros, transport feature flags
- Context7 `/clap-rs/clap` — derive API, subcommand patterns
- Context7 `/kotauskas/interprocess` — `local_socket` unified API, Windows named-pipe examples
- crates.io API — live version verification for `rmcp`, `clap`, `interprocess`, `tokio`, `tokio-util`, `schemars`, `directories`, `config`, `thiserror`, `anyhow`, `tracing`, `indicatif`, `windows-sys`, `nix`, `ironrdp*` crates
- `crates/rdpilot/src/session.rs`, `session_loop.rs`, `connect.rs`, `rdpdr_backend.rs` (project source, read directly)
- [Claude Platform Docs — Computer use tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool); [claude-quickstarts computer_use_demo/tools/computer.py](https://github.com/anthropics/claude-quickstarts/blob/main/computer-use-demo/computer_use_demo/tools/computer.py)
- [FreeRDP GHSA-3xpj-m4hx-8vmx](https://github.com/FreeRDP/FreeRDP/security/advisories/GHSA-3xpj-m4hx-8vmx); [ZeroPath CVE-2025-48817](https://zeropath.com/blog/cve-2025-48817-windows-rdp-path-traversal)
- [tokio::net::windows::named_pipe::ServerOptions docs.rs](https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ServerOptions.html)
- `.planning/PROJECT.md`, `.planning/STATE.md` — v1.0 accumulated decisions and requirements

### Secondary (MEDIUM confidence)
- [modelcontextprotocol/rust-sdk GitHub](https://github.com/modelcontextprotocol/rust-sdk) — release cadence, feature-flag inventory
- MCP session-state and long-running-tool-call community discussions (LangChain MCP docs, CodeSignal, MCP GitHub Discussion #102, ClickHouse mcp-clickhouse #128)
- MCP binary-content size-limit real-world reports (anthropics/claude-code issues, modelcontextprotocol Discussion #1197)
- [sccache GitHub](https://github.com/mozilla/sccache) — auto-start/idle-timeout daemon lifecycle model
- [csandker.io Offensive Windows IPC — Named Pipes](https://csandker.io/2021/01/10/Offensive-Windows-IPC-1-NamedPipes.html) — default named-pipe DACL risk

### Tertiary (LOW confidence)
- `daemon-base`/`cross-platform-service` crates — single-source, low-adoption signal, used only to justify avoiding them
- `jsonrpsee` maturity assessment — single-source (own changelog), corroborated only by self-description

---
*Research completed: 2026-07-10*
*Ready for roadmap: yes*
