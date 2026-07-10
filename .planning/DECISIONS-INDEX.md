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

### Security

- **D-24:** The IPC/MCP wire and logs must redact credentials on Serialize paths (v1.0's D-14 redaction covered only Debug). Rationale: derive(Serialize) does not inherit Debug redaction; a naive list-sessions response would leak the password.

### Proof

- **D-25:** Dual finish line — scripted proof harness per surface (no live LLM) AND a capstone live-LLM demo driving a read/inspect + file-transfer task through the MCP surface. Rationale: keep v1's provable-without-LLM rigor while proving the namesake AI-consumer path once end-to-end.

### Stack

- **D-26:** Rust stack additions: rmcp 2.2.0 (MCP SDK), interprocess 2.4.2 / native named-pipe (IPC), clap 4.6.1 (CLI), config 0.15.25 (layered config). Rationale: current, well-maintained, cross-platform; anti-recommend jsonrpsee/daemonize/axum/tonic for local IPC.

</decisions>
