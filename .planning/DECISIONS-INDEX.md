# Decisions Index

Cross-cutting decisions that span multiple phases or plans. Phase/plan-scoped
decisions (e.g. `D-7.4`) live in their originating SUMMARY.md and are cross-referenced
from PROJECT.md's Key Decisions table; this index is for milestone-level and
cross-cutting calls that don't belong to a single plan.

<decisions>

### Scope

- **D-16:** v1.1 ships exactly two consumer surfaces — MCP server + CLI. PyO3/NAPI-RS language bindings and clipboard/CLIPRDR are explicitly deferred. Rationale: focus the milestone on the first non-harness consumers; bindings are a separate concern.

### Architecture

- **D-17:** CLI and MCP server are BOTH thin clients of a single long-lived session daemon over local IPC — not embedded, not duplicated. Rationale: the CLI is invoke-and-exit and cannot hold an RDP session/keepalive; the daemon owns session state so both surfaces share one registry.
- **D-18:** Session identity: every command explicitly targets a session by name (or auto-id); NO implicit default target, enforced as a required wire-schema field. Rationale: an AI agent acting on the wrong session is dangerous; a required field prevents convenience shortcuts from reintroducing a default.
- **D-19:** Daemon uses an in-memory session registry with orphan-cleanup on restart (detect + tear down orphaned Windows-side sessions); durable reattach across daemon restart is deferred. Rationale: balanced cost/safety for v1.1; avoids RDP-reconnect-semantics complexity while preventing zombie-session accumulation.

### Transport

- **D-20:** Bidirectional file transfer generalizes the existing RDPDR path (extend RdpilotDriveBackend to accept writes + new sensor upload/download commands), NOT CLIPRDR. Rationale: RDPDR already deploys the sensor; CLIPRDR is clipboard-shaped, reserved for the deferred clipboard feature.

### Surface

- **D-21:** MCP presents a dual surface: a single Anthropic computer-use-compatible schema-discriminated `computer` tool PLUS a small rdpilot-native tool set — not 25+ one-per-action tools. Rationale: matches Anthropic's own single-tool design and avoids degrading model tool-selection.
- **D-22:** MCP file put/get operate on local disk paths and return path/size/checksum metadata; never inline file bytes. Rationale: real MCP clients corrupt base64 binary content above ~12KB (multiple open upstream bugs).

### Config

- **D-23:** Consumers use a conventionally-named, discoverable, gitignored config file (platform config dir / clearly-named `.rdpilot.*`), layered with env vars + CLI flags / MCP init params — never a cryptic `.secrets/connection.json`. Rationale: follow common CLI-tool conventions so the file is self-explanatory.
- **D-27:** Config concrete form (phases 11, 13, 14): a TOML file `config.toml` at the platform config dir (`~/.config/rdpilot/config.toml` on Unix; `%APPDATA%\rdpilot\config.toml` on Windows), gitignored; layered file -> env -> flag/MCP-init per CONFIG-01. Env overrides use the `RDPILOT_` prefix with the config crate's `__` nesting separator (`RDPILOT_<KEY>`); CLI flags (`--<key>`) and MCP-init parameter keys reuse the EXACT SAME key spellings verbatim as the config keys so all three surfaces are consistent. Keys cover at least host, port, username, password, domain, cert-bypass. Rationale: one canonical key vocabulary across file/env/flag/MCP-init prevents drift and surprise.

### Security

- **D-24:** The IPC/MCP wire and logs must redact credentials on Serialize paths (v1.0's D-14 redaction covered only Debug). Rationale: derive(Serialize) does not inherit Debug redaction; a naive list-sessions response would leak the password.

### Proof

- **D-25:** Dual finish line — scripted proof harness per surface (no live LLM) AND a capstone live-LLM demo driving a read/inspect + file-transfer task through the MCP surface. Rationale: keep v1's provable-without-LLM rigor while proving the namesake AI-consumer path once end-to-end.

### Stack

- **D-26:** Rust stack additions: rmcp 2.2.0 (MCP SDK), interprocess 2.4.2 / native named-pipe (IPC), clap 4.6.1 (CLI), config 0.15.25 (layered config). Rationale: current, well-maintained, cross-platform; anti-recommend jsonrpsee/daemonize/axum/tonic for local IPC.

### Errors

- **D-28:** Wire error taxonomy (phases 11, 13, 14): failures are modeled on the wire as a typed `WireError { code: <enum>, message: String }` with a FIXED code set — `session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch` — mapped 1:1 from the Phase 10 SDK Error variants (`Error::PathTraversal`, `Error::ChecksumMismatch`, etc. in `crates/rdpilot/src/error.rs`). The CLI emits a DISTINCT non-zero exit code per error class (scriptable for PROOF-02); the MCP server renders code+message as the tool error result. Rationale: CLI-03 error legibility (distinct session-not-found / daemon-unreachable / transfer-failure) is designed INTO the taxonomy, not bolted on downstream.

### Session

- **D-29:** Session identity & targeting (phases 12, 13, 14): auto-generated session ids are short, human-legible adjective-noun word-pairs (e.g. `brave-otter`). There is NO implicit/silent default target (reinforces D-18): the CLI requires a global `--session` flag on EVERY command, and the MCP server requires a `session` parameter on EVERY tool call (per-call, not a one-session-per-server init binding). Rationale: favors anti-wrong-session safety over LLM ergonomics.
- **D-30:** `list` status vocabulary (phases 12, 13, 14): the daemon's `list` reports a lifecycle status enum — `Connecting` / `Live` / `Reconnecting` / `Disconnected` — derived from the SDK keepalive signal (`crates/rdpilot/src/keepalive.rs`; no public health enum exists yet, so the daemon defines it). Both CLI and MCP render this vocabulary. Rationale: a single shared status vocabulary keeps both surfaces consistent and legible.

</decisions>
