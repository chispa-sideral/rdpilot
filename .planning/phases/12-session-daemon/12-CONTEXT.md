# Phase 12: Session Daemon - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 12 delivers the **long-lived local session daemon**: a single background process that holds N named RDP `Session`s behind a correct, leak-free, in-memory registry, exposes them over a local-user-only IPC transport, auto-starts on first client connect, reaps idle sessions / self-shuts-down when the registry empties, and — after its own crash — surfaces possibly-still-live orphaned remote Windows sessions for explicit reconciliation rather than silently forgetting or blindly auto-killing them.

This is the shared foundation both v1.1 consumer surfaces (Phase 13 CLI, Phase 14 MCP) become thin clients of (D-17). This phase builds the daemon + registry + IPC server + lifecycle/orphan machinery ONLY — it does not build the CLI, the MCP server, or the wire-protocol/config crates.

**Depends on:** Phase 11 (`rdpilot-ipc` shared wire protocol + `rdpilot-config` layered config). The registry and request dispatch are written against Phase 11's shared protocol types (required-`session: SessionId` schema, credential-free DTOs, `WireError` taxonomy).
**Requirements:** DAEMON-01, DAEMON-02, DAEMON-03, DAEMON-04, SESSION-01, SESSION-03, SESSION-04.

**Success Criteria** (from ROADMAP.md, verbatim):
1. A session opens under a caller-supplied name (or a short, human-legible auto-id when unnamed) and stays addressable on later commands; duplicate names are rejected, and a concurrency test firing N simultaneous same-name connects yields exactly one live session and N-1 clean rejections (SESSION-01, SESSION-04; Pitfall 3 atomic insert).
2. `list` reports active sessions with name/id, target, status, and connected-since / last-activity (SESSION-03).
3. **[BLOCKING]** A soak test of N connect/disconnect cycles returns thread count and RSS to baseline — no session-per-OS-thread leak (DAEMON-01; Pitfall 1).
4. **[BLOCKING]** The IPC transport is restricted to the local user (Unix `0700` dir + peer-uid check / Windows explicit DACL via `create_with_security_attributes_raw`), verified by a different-uid/other-account client being rejected (DAEMON-02; Pitfall 5).
5. **[BLOCKING]** The daemon auto-starts on first client connect and self-shuts-down when the registry empties; after a `kill -9` mid-session and restart it reports the possibly-still-live remote session and tears down / reconciles orphans rather than silently forgetting them (DAEMON-03, DAEMON-04; Pitfall 9, in-memory registry + minimal disk-persisted reconciliation state).

**Out of scope:** the shared wire-protocol + config crates (Phase 11); the CLI verbs (Phase 13); the MCP server + tools (Phase 14); durable session reattach across daemon restart (deferred per D-19 — orphans are reconciled/torn-down, not re-adopted as live sessions); remote-assist / session shadowing (Backlog 999.4).

</domain>

<decisions>
## Phase-Local Decisions

- **D-31:** Daemon lifecycle policy (owned by Phase 12). Idle sessions ARE reaped after a config-overridable inactivity timeout (satisfying DAEMON-03's "reaps idle sessions"), and when the registry empties the daemon self-exits — Success Criterion 5's immediate self-shutdown-on-empty is honored, softened only by a short config-overridable anti-thrash **grace period** before the empty daemon actually exits (so a rapid disconnect→reconnect does not pay a full daemon cold-start). On crash/restart the daemon SURFACES possibly-live orphaned remote Windows sessions (from the minimal disk-persisted reconciliation state) for EXPLICIT reclaim/teardown rather than auto-killing them — it never silently destroys a session a human may be actively using, and never silently forgets it (DAEMON-04). All durations (idle-session timeout, empty-daemon grace period) are config-overridable via the Phase 11 config layers. Rationale: balances Success Criterion 5's crisp self-shutdown-on-empty against thrash avoidance, and prioritizes anti-destroy safety for orphaned remote sessions over convenience auto-cleanup.

</decisions>

<inherited_decisions>
## Inherited / Already-Locked Decisions (carried forward, do NOT re-open)

### Owned by this phase (Phase 12 is the defining phase)

- **D-29 (DECISIONS-INDEX.md) — Session identity & targeting.** Auto-generated session ids are short, human-legible adjective-noun word-pairs (e.g. `brave-otter`). There is NO implicit/silent default target (reinforces D-18). **Phase 12 owns the id-generation logic** (the daemon mints the auto-id when a caller opens a session unnamed, and enforces uniqueness / collision rejection per SESSION-04). The downstream CLI (`--session` on every command) and MCP server (`session` parameter on every tool call) consume these ids; those surfaces are built in Phases 13/14.
- **D-30 (DECISIONS-INDEX.md) — `list` status vocabulary.** The daemon's `list` reports a lifecycle status enum — `Connecting` / `Live` / `Reconnecting` / `Disconnected` — derived from the SDK keepalive signal. **Phase 12 defines this status enum** (no public health enum exists yet in the SDK; the daemon defines it, deriving it from `crates/rdpilot/src/keepalive.rs`). Both the Phase 13 CLI and Phase 14 MCP render this same vocabulary.

### Locked upstream (do NOT re-decide in this phase)

- **D-17 (DECISIONS-INDEX.md) — Thin-client daemon architecture.** CLI and MCP are BOTH thin clients of this single long-lived daemon over local IPC; the daemon owns session state and the shared registry. Phase 12 builds that daemon.
- **D-18 (DECISIONS-INDEX.md) — Explicit per-command session targeting.** No implicit default target; session identity is a required wire-schema field (enforced in Phase 11's `rdpilot-ipc`). Phase 12's dispatch resolves every request against its required `session` field.
- **D-19 (DECISIONS-INDEX.md) — In-memory registry + orphan cleanup on restart, reattach deferred.** The registry is in-memory; on restart the daemon detects and reconciles/tears-down orphaned Windows-side sessions. Durable reattach is explicitly deferred. **This registry architecture is LOCKED** — Phase 12 implements it (in-memory registry + minimal disk-persisted reconciliation state), it does not re-decide it. D-31 above only refines the lifecycle *policy* (idle reap + grace + explicit orphan reclaim) on top of this locked architecture.
- **D-26 (DECISIONS-INDEX.md) — Stack.** IPC transport is `interprocess` 2.4.2 / native named-pipe; jsonrpsee/daemonize/axum/tonic are anti-recommended for local IPC. The DAEMON-02 local-user scoping uses `interprocess`'s `create_with_security_attributes_raw` on Windows (explicit DACL) and a `0700` socket-dir + peer-uid check on Unix. **This local-user IPC-scoping mechanism is LOCKED** (Success Criterion 4) — Phase 12 implements it, it does not re-decide the transport or the scoping primitive.
- **D-24 (DECISIONS-INDEX.md) — Serialize-path credential redaction.** Enforced at the Phase 11 schema level; the daemon's `list`/status responses ride credential-free DTOs and must not leak the password. Phase 12 consumes those DTOs; it does not re-implement redaction.
- **D-27 / D-28 (DECISIONS-INDEX.md) — Config form / Wire error taxonomy.** The daemon reads config via the Phase 11 config crate (D-27: `config.toml` at the platform config dir, `RDPILOT_`-prefixed env overrides) — this is where D-31's config-overridable idle-timeout / grace-period durations live. The daemon's dispatch maps failures onto the fixed `WireError` code set (D-28: `session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`), so a duplicate-name/collision rejection and a session-not-found lookup surface as their designated codes.

</inherited_decisions>

<canonical_refs>
## Canonical References

**Downstream researcher/planner MUST read these before planning or implementing.**

### Roadmap / Requirements / Decisions
- `.planning/ROADMAP.md` §"Phase 12: Session Daemon" (lines 74-87) — goal, success criteria, requirement IDs
- `.planning/REQUIREMENTS.md` DAEMON-01..04 (lines 12-15), SESSION-01/03/04 (lines 19, 21-22) — requirement text
- `.planning/DECISIONS-INDEX.md` — inherited cross-cutting decisions: D-17/D-18/D-19 (architecture), D-24 (redaction), D-26 (stack/IPC), D-27 (config form), D-28 (wire error taxonomy), D-29 (session identity — owned here), D-30 (`list` status vocab — owned here)

### Phase 11 dependency (shared crates — must exist before Phase 12 planning finalizes)
- `rdpilot-ipc` — wire protocol DTOs, required `session: SessionId` field, `WireError { code, message }` taxonomy (D-28); the daemon's IPC server dispatches these types
- `rdpilot-config` — layered file→env→flag config resolution (D-27); source of D-31's config-overridable idle-timeout / grace-period durations
- (If Phase 11 emits its own CONTEXT/SUMMARY, read it for the concrete DTO + `SessionId` shape.)

### SDK source the daemon builds on (`crates/rdpilot/src/`)
- `crates/rdpilot/src/session.rs` — `Session` type the registry holds N of; the UNTOUCHABLE threading model (dedicated OS thread + current-thread Tokio runtime, ~line 206-210 — never `tokio::spawn`). DAEMON-01's "no session-per-OS-thread leak" soak test lives against exactly this: each `Session` owns a thread, so the registry must join/drop threads on disconnect or RSS/thread-count will not return to baseline.
- `crates/rdpilot/src/keepalive.rs` — the keepalive signal (`KEEPALIVE_INTERVAL` = 60 s, zero-delta null input); no public health/status enum exists yet, so the daemon defines the D-30 `Connecting`/`Live`/`Reconnecting`/`Disconnected` enum derived from this keepalive/liveness signal.
- `crates/rdpilot/src/connect.rs` — connection entry point (`RDPILOT_SENSOR` / `SENSOR_EXE_NAME` consts, sensor-gated RDPDR registration); how a daemon-held session is established per target.
- `crates/rdpilot/src/config.rs` — `ConnectionConfig` builder the daemon populates from `rdpilot-config` to open each session (host/creds/sensor path).
- `crates/rdpilot/src/error.rs` — SDK `Error` enum; the daemon maps these to Phase 11 `WireError` codes (D-28 mapping is 1:1 from these variants).
- `crates/rdpilot/examples/proof_harness.rs` — end-to-end Session-driving harness; the pattern the daemon's per-session lifecycle and the Phase 15 proof harnesses build on.

</canonical_refs>

<deferred>
## Deferred Ideas

- Durable session reattach across daemon restart — explicitly deferred per D-19; v1.1 uses an in-memory registry and reconciles orphans rather than re-adopting them as live sessions.
- Remote-assist / session shadowing — Backlog Phase 999.4 / SEED-001, out of scope for v1.1.
- MCP progress notifications for long operations — Future Requirements (Phase 14+ concern, not the daemon).

No scope-creep ideas surfaced during this phase's discussion.

</deferred>

---

*Phase: 12-Session Daemon*
*Context gathered: 2026-07-10*
