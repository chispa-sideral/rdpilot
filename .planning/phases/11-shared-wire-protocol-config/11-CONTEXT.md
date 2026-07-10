# Phase 11: Shared Wire Protocol & Config - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 11 delivers the two shared foundation crates every downstream consumer surface (daemon, CLI, MCP) is built on, BEFORE any consumer binary exists:

- **`rdpilot-ipc`** — the daemon↔client wire protocol: request/response DTOs (including the Phase 10 file-transfer put/get variants), with **session identity as a required, non-optional schema field** and a **typed wire error taxonomy**, and with **credential-free** status/response types by construction.
- **`rdpilot-config`** — layered configuration resolution: a conventionally-named, gitignored config file overridden by env vars, then CLI flags / MCP-init params, with the highest-precedence layer deterministically winning.

The unifying constraint: session-identity-as-required-field and credential-redaction are enforced **at the schema level** — structurally, before any consumer can accidentally reintroduce a default target or leak a secret.

**Depends on:** Phase 10 (the wire protocol must cover the file-transfer request/response variants — `upload_file`/`download_file`, `TransferOutcome { bytes_transferred, checksum }`).
**Requirements:** SESSION-02, CONFIG-01, CONFIG-02, CONFIG-03.

**Success Criteria** (from ROADMAP.md, verbatim):
1. **[BLOCKING]** Every session-scoped request carries a required, non-optional `session: SessionId` field; omitting it is a hard schema-validation rejection at the wire boundary (not a fallback), verified by a test that omits the field on every verb (SESSION-02; Pitfall 2).
2. Target host + credentials resolve through a layered file → env → flag/MCP-init precedence, with the highest-precedence layer deterministically winning (CONFIG-01).
3. The config file follows common CLI-tool convention (platform config dir / clearly-named `.rdpilot.*`), is gitignored, and is discoverable and self-explanatory (CONFIG-02).
4. **[BLOCKING]** Serializing every wire response type and grepping the output for a planted secret sentinel finds nothing — credential-free status DTOs, no plaintext password on the wire or in logs (CONFIG-03; Pitfall 4, closing the v1.0 D-14 Debug-only redaction gap).

**Out of scope:** the daemon itself and its session registry (Phase 12); the CLI verbs/binary (Phase 13); the MCP server/tools (Phase 14); the IPC transport wiring / socket / DACL enforcement (Phase 12, DAEMON-02). Phase 11 defines the **types and config resolution** those phases consume — not the daemon runtime or transport plumbing.

</domain>

<decisions>
## Decisions

Phase-local decisions continue the shared cross-cutting namespace (max in DECISIONS-INDEX.md is D-30; this phase adds D-31).

- **D-31:** **Credentials NEVER appear on the wire — daemon-only, structural not string-scrubbed.** Credentials (password, and the full connect secret set) are supplied ONCE to the daemon on connect and held daemon-side only; there is NO password/credential field on ANY list/status/response DTO in `rdpilot-ipc`. This satisfies CONFIG-03's BLOCKING planted-sentinel grep test **structurally** — there is no secret in the serializable type to leak — rather than by post-hoc `Serialize` string-scrubbing or custom redacting serializers. Wire status/list DTOs carry only non-secret fields (name/id, target host, status, timestamps). **v1.0 baseline note:** `crates/rdpilot/src/config.rs` already has manual `Debug` redaction (`impl fmt::Debug for ConnectionConfig`, ~line 221, `password: String` at line 42) but NO `Serialize` impl yet — so the leak surface is created only when serializable wire types are introduced. The new `rdpilot-ipc` + `rdpilot-config` crates must therefore never place credentials into any type that derives/implements `Serialize` for the wire. This closes the D-14 / D-24 gap by prevention, not by scrubbing.

</decisions>

<inherited_decisions>
## Inherited Decisions

Cross-cutting decisions this phase OWNS and implements (full text in `.planning/DECISIONS-INDEX.md`). **Phase 11 is where CC1 (config surface) and CC2 (wire error taxonomy) are built.**

- **D-27 (CC1 — Config surface — OWNED/BUILT HERE):** `rdpilot-config` produces a TOML `config.toml` at the platform config dir (`~/.config/rdpilot/config.toml` on Unix; `%APPDATA%\rdpilot\config.toml` on Windows), gitignored; layered **file → env → flag/MCP-init** precedence (CONFIG-01). Env overrides use the `RDPILOT_` prefix with the `config` crate's `__` nesting separator (`RDPILOT_<KEY>`); CLI flags (`--<key>`) and MCP-init param keys reuse the **EXACT SAME key spellings verbatim** as the config keys so all three surfaces share one canonical key vocabulary. Keys cover at least host, port, username, password, domain, cert-bypass. Phase 11 defines this crate + key vocabulary; Phases 13/14 consume it.

- **D-28 (CC2 — Wire error taxonomy — OWNED/BUILT HERE):** `rdpilot-ipc` models failures on the wire as a typed `WireError { code: <enum>, message: String }` with a FIXED code set — `session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch` — mapped 1:1 from the Phase 10 SDK `Error` variants (`Error::PathTraversal`, `Error::ChecksumMismatch`, etc. in `crates/rdpilot/src/error.rs`). Phase 11 defines the enum + the SDK-`Error`→`WireError` mapping. Downstream: the CLI (Phase 13) emits a DISTINCT non-zero exit code per class (CLI-03 / PROOF-02 scriptability); the MCP server (Phase 14) renders code+message as the tool error result.

Cross-cutting decisions this phase must stay consistent with (defined elsewhere, referenced here because Phase 11's types encode them):

- **D-18 / D-29 (session identity):** No implicit/silent default target; session identity is a **required wire-schema field** (SESSION-02 success criterion #1). Phase 11's `SessionId` field on every session-scoped request is the schema-level enforcement point; D-29's `--session`/`session` per-call requirement and short adjective-noun auto-ids are consumed by Phases 12-14.
- **D-30 (list status vocabulary):** `list` reports a lifecycle status enum — `Connecting` / `Live` / `Reconnecting` / `Disconnected` — derived from the SDK keepalive signal (`crates/rdpilot/src/keepalive.rs`; no public health enum exists yet). Phase 11's list/status DTO carries this status field (credential-free, per D-31); Phase 12 populates it from the keepalive signal.
- **D-24 (Serialize-path redaction):** the root motivation for CONFIG-03; D-31 above discharges it structurally.
- **D-26 (stack):** `config` 0.15.25 for layered config; `serde`/`serde_json` for wire DTO serialization. `interprocess`/`rmcp`/`clap` belong to the consumer phases, not Phase 11.

</inherited_decisions>

<canonical_refs>
## Canonical References

**Downstream researcher/planner MUST read these before planning or implementing.**

### Roadmap / Requirements / Decisions
- `.planning/ROADMAP.md` §"Phase 11: Shared Wire Protocol & Config" (lines 60-72) — goal, success criteria, requirement IDs, Phase 10 dependency note
- `.planning/REQUIREMENTS.md` — SESSION-02 (line 20), CONFIG-01 (line 48), CONFIG-02 (line 49), CONFIG-03 (line 50); FILE-01..04 (lines 41-44) for the file-transfer wire variants the protocol must cover
- `.planning/DECISIONS-INDEX.md` — D-18, D-24, D-26, D-27, D-28, D-29, D-30 (the inherited/consistency set), plus this phase's D-31

### Phase 10 wire contract (what the protocol must cover)
- `.planning/phases/10-sdk-file-transfer-extension/10-CONTEXT.md` §D-10.4 (public API shape: `Session::upload_file`/`download_file`, `TransferOutcome { bytes_transferred, checksum }`) — the file-transfer request/response the `rdpilot-ipc` put/get variants wrap
- `.planning/phases/10-sdk-file-transfer-extension/10-VERIFICATION.md` — live-verified outcomes / observed field shapes

### Source files (SDK layer the new crates map onto)
- `crates/rdpilot/src/error.rs` — the SDK `Error` enum, including `Error::PathTraversal` and `Error::ChecksumMismatch` (Phase 10) plus `SensorRejected`/`Dvc`; the 1:1 source for the D-28 `WireError` code mapping
- `crates/rdpilot/src/config.rs` — `ConnectionConfig` (`password: String` line 42; manual `Debug` redaction `impl fmt::Debug` ~line 221; NO `Serialize` impl); the v1.0 baseline the new config crate + credential-free DTOs must not regress (D-31/CONFIG-03)
- `crates/rdpilot/src/keepalive.rs` — the keepalive signal D-30's `Connecting`/`Live`/`Reconnecting`/`Disconnected` status vocabulary is derived from
- `crates/rdpilot/src/session.rs` — the public `Session` surface (upload/download + perception/input/launch verbs) the wire protocol exposes remotely
- `crates/rdpilot/examples/proof_harness.rs` — existing scripted-harness usage pattern; informs how the wire verbs will be exercised by the Phase 15 CLI/MCP proof harnesses
- `Cargo.toml` (workspace root, `members = ["crates/rdpilot"]`) — must gain `crates/rdpilot-ipc` and `crates/rdpilot-config` members

</canonical_refs>

<deferred>
## Deferred Ideas

None surfaced. The IPC transport (socket/named-pipe + DACL), the daemon registry, the CLI/MCP binaries, and the proof harnesses are not deferrals — they are explicitly scoped to Phases 12-15 and are out of Phase 11's boundary by design.

</deferred>

---

*Phase: 11-Shared Wire Protocol & Config*
*Context gathered: 2026-07-10*
