# Phase 14: MCP Server Surface - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 14 delivers an `rmcp`-based MCP server binary that exposes rdpilot to any MCP client (Claude Desktop/Code, etc.) as a **thin client of the session daemon** (D-17), presenting a dual tool surface: a single Anthropic computer-use-compatible schema-discriminated `computer` mega-tool PLUS a small set of rdpilot-native tools — with a tested coordinate-scaling bridge and non-blocking per-call task isolation.

**Depends on:** Phase 13 (built on the daemon/IPC + shared-wire foundation the CLI already validated end-to-end; the MCP server is another `rdpilot-ipc` client, NOT a re-implementation).
**Requirements:** MCP-01, MCP-02, MCP-03, MCP-04, MCP-05, MCP-06.

**Success Criteria** (from ROADMAP.md, verbatim):
1. An MCP client (e.g. Claude) can call a single Anthropic computer-use-compatible `computer` tool (screenshot + action-discriminated mouse/keyboard/scroll) that maps onto the SDK input/capture verbs (MCP-01, MCP-02).
2. rdpilot-native tools (world_state, uia, window/process list, launch, foreground, session connect/list/disconnect, file put/get returning `{path, bytes_transferred, checksum}` metadata only — never inline file bytes) are exposed as MCP Tools (MCP-03, MCP-05).
3. **[BLOCKING]** Coordinates round-trip correctly: one fixed advertised resolution + a single tested `scale_to_native(x, y)` bridges rdpilot's 96-DPI physical-pixel space to the computer-use scaled screenshot/coordinate space, verified by a precision click test near screen edges/corners (not just center) (MCP-04; Pitfall 6).
4. **[BLOCKING]** A slow tool call (large file transfer, `launch_process` wait) does not block a concurrent unrelated fast tool call — per-call task isolation with bounded, explicit timeouts (MCP-06; Pitfall 7).

**Out of scope:** MCP progress notifications (`notifications/progress`) for long transfers (deferred, Future Requirements); the scripted MCP proof harness (PROOF-03) and the live-LLM capstone (PROOF-04) — both Phase 15; PyO3/NAPI-RS bindings (deferred). This phase builds the MCP surface binary and its tool mappings only.

</domain>

<decisions>
## Implementation Decisions

Phase-local decisions use the `D-14.N` sub-numbered convention (matching Phase 10's `D-10.N` style). Cross-cutting decisions this phase inherits are enumerated separately in `<inherited_decisions>`.

- **D-14.1 — MCP transport: stdio, host-spawned per client (rmcp 2.x).**
  The MCP server speaks stdio and is spawned per-client by the MCP host (registered in the client's `mcpServers` block, e.g. Claude Desktop/Code config). It uses `rmcp` 2.x (the SDK pinned in D-26). Per-client stdio lifetime is acceptable precisely because the **daemon** (D-17) — not the MCP server — owns the long-lived RDP sessions and keepalive: sessions survive MCP-server restarts, so the MCP server is a stateless-per-process, restart-safe `rdpilot-ipc` client.
  **Rationale:** stdio is the conventional MCP host integration path and needs no port/DACL story of its own; the daemon already provides session durability across MCP-server process churn.
  **Rejected:** HTTP/SSE transport (adds a listening port + auth surface with no benefit for a locally-host-spawned server); the MCP server holding sessions itself (would duplicate the daemon and lose sessions on every client restart — violates D-17).

- **D-14.2 — `computer` tool spec: fixed 1280x800 (WXGA) advertised resolution + `computer_20250124` action set + prefixed native tools.**
  The `computer` mega-tool advertises a **FIXED 1280x800 (WXGA)** logical resolution as the downscale target that models see and click within — chosen for best model click accuracy (Anthropic guidance: keep the advertised resolution modest for coordinate precision). A single tested `scale_to_native(x, y)` bridges every model-supplied 1280x800 coordinate to the 96-DPI native remote desktop pixel space (satisfies MCP-04 / Success Criterion 3; the one-fixed-resolution + single-bridge shape is LOCKED). Implement the Anthropic **`computer_20250124`** action set — the schema-discriminated action superset that adds `scroll`, `triple_click`, `hold_key`, and `wait` on top of the base mouse/keyboard/screenshot actions — each action mapping onto the existing SDK input/capture verbs.
  rdpilot-native (non-`computer`) tools are **PREFIXED** with `rdpilot_` (e.g. `rdpilot_world_state`, `rdpilot_uia_tree`, `rdpilot_window_list`, `rdpilot_process_list`, `rdpilot_launch`, `rdpilot_foreground`, `rdpilot_connect`, `rdpilot_list`, `rdpilot_disconnect`, `rdpilot_put`, `rdpilot_get`) to avoid name collisions with the `computer` tool and with any other server the host has loaded.
  **Rationale:** matches Anthropic's own single-computer-tool design (D-21) and the `computer_20250124` tool version; a fixed advertised resolution keeps the coordinate bridge a single tested function rather than a per-session negotiation; a namespace prefix prevents tool-name collisions in multi-server MCP hosts.
  **Rejected:** dynamic/per-session advertised resolution (would multiply the coordinate-bridge test matrix and reintroduce drift the fixed-resolution lock exists to prevent); the older `computer_20241022` action set (lacks `scroll`/`triple_click`/`hold_key`/`wait`); unprefixed native tool names (collision risk across MCP servers).

### Claude's Discretion

- Exact rmcp 2.x server scaffolding shape (handler struct, `#[tool]`/router macro usage vs. manual tool registration) — follow the current rmcp 2.x idiom; keep tool schemas serde-derived.
- Internal representation of the `computer` action discriminant (single serde-tagged enum over the `computer_20250124` actions vs. a dispatch table) — keep it a single schema-discriminated tool per D-21, one action enum.
- Precise per-call timeout values for the bounded-timeout requirement (MCP-06) — planner/researcher may choose concrete numbers per verb class (fast perception/input vs. slow file-transfer/launch-wait), so long as every tool call has an explicit bound and slow calls cannot starve fast ones.
- Where `scale_to_native` lives (dedicated module vs. inline in the computer-tool handler) — keep it ONE function with a direct unit test hitting edges/corners (MCP-04 is BLOCKING).

</decisions>

<inherited_decisions>
## Inherited / Cross-Cutting Decisions (carried forward, do NOT re-open)

These are owned in `.planning/DECISIONS-INDEX.md`; this phase consumes them.

- **D-27 (Config — CC1):** Layered config `config.toml` at the platform config dir, gitignored, resolved file → env (`RDPILOT_` prefix, `__` nesting separator) → flag/**MCP-init** parameter. **This phase's MCP-init parameter keys reuse the EXACT SAME key spellings verbatim as the config keys and the CLI flags** (host, port, username, password, domain, cert-bypass, …) so file/env/flag/MCP-init share one canonical vocabulary. (CONFIG-01)
- **D-28 (Errors — CC2):** Wire failures are a typed `WireError { code: <fixed enum>, message: String }` (`session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`) mapped 1:1 from the Phase 10 SDK `Error` variants. **The MCP server renders `code` + `message` as the tool error result** (the CLI's distinct-exit-code rendering is the CLI's concern; the MCP server surfaces both fields in its tool-call error output).
- **D-29 (Session — CC3):** No implicit/silent default target (reinforces D-18). **The MCP server requires a `session` parameter on EVERY tool call — per-call, NOT a one-session-per-server init binding.** Auto-generated ids are short adjective-noun word-pairs (e.g. `brave-otter`).
- **D-30 (Session list status — CC4):** `list` reports the shared lifecycle status enum — `Connecting` / `Live` / `Reconnecting` / `Disconnected` — derived from the SDK keepalive signal. **The MCP server renders this same vocabulary** in its `rdpilot_list` tool output (identical to the CLI's rendering).

### Already-Locked Surface Decisions (do NOT re-litigate)

- **D-21 (DECISIONS-INDEX):** MCP presents ONE schema-discriminated `computer` tool PLUS a small rdpilot-native tool set — never 25+ one-per-action tools.
- **D-22 (DECISIONS-INDEX):** MCP file put/get operate on local disk paths and return `{path, size/bytes_transferred, checksum}` metadata — **never inline file bytes** (real MCP clients corrupt base64 binary above ~12KB). (MCP-05)
- **Success Criterion 3 (LOCKED):** ONE fixed advertised resolution + a single tested `scale_to_native` bridging 96-DPI physical pixels — not per-session negotiation. (concretized as 1280x800 in D-14.2)
- **D-17 (DECISIONS-INDEX):** CLI and MCP server are BOTH thin clients of the single long-lived daemon over local IPC — the MCP server does NOT embed or duplicate session/keepalive logic.

</inherited_decisions>

<canonical_refs>
## Canonical References

**Downstream researcher/planner MUST read these before planning or implementing.**

### Roadmap / Requirements / Decisions
- `.planning/ROADMAP.md` §"Phase 14: MCP Server Surface" (lines 102-114) — goal, success criteria, requirement IDs
- `.planning/REQUIREMENTS.md` — MCP-01, MCP-02, MCP-03, MCP-04, MCP-05, MCP-06 (lines 32-37); CONFIG-01/02/03 (lines 48-50) for the shared config vocabulary; MCP-05 metadata-only rule
- `.planning/DECISIONS-INDEX.md` — inherited cross-cutting decisions D-17, D-21, D-22, D-26 (stack: rmcp 2.2.0), D-27, D-28, D-29, D-30

### Upstream phase context (this phase builds ON these — read for the wire/daemon contract)
- `.planning/phases/10-sdk-file-transfer-extension/10-CONTEXT.md` — SDK file-transfer `Session::upload_file`/`download_file` + `TransferOutcome { bytes_transferred, checksum }` shape that `rdpilot_put`/`rdpilot_get` surface as metadata (MCP-05)
- Phase 11 `rdpilot-ipc` wire schema (required `session: SessionId` field, credential-free DTOs, `WireError`) and `rdpilot-config` — the MCP server is a client of these; read whatever Phase 11 produces before planning
- Phase 12 daemon IPC contract (the transport the MCP server dials) and Phase 13 CLI verb set (the MCP tools mirror this verb set over the same daemon)

### SDK source (verb mappings + coordinate/keepalive/error primitives)
- `crates/rdpilot/src/session.rs` — the public perception/input/launch/file verbs the `computer` actions and `rdpilot_*` native tools map onto; threading model context
- `crates/rdpilot/src/error.rs` — `Error` enum variants (`PathTraversal`, `ChecksumMismatch`, `SensorRejected`, `Dvc`, `CoordinateOutOfBounds`, …) that map 1:1 into `WireError.code` (D-28); Display/category pattern
- `crates/rdpilot/src/config.rs` — `ConnectionConfig` builder + the config-key vocabulary the MCP-init parameter keys must reuse verbatim (D-27)
- `crates/rdpilot/src/keepalive.rs` — the keepalive signal the `Connecting`/`Live`/`Reconnecting`/`Disconnected` list-status enum is derived from (D-30); no public health enum exists yet — the daemon defines it
- `crates/rdpilot/examples/proof_harness.rs` — the v1.0 scripted-harness pattern; precedent for the Phase 15 MCP proof harness (PROOF-03) that will drive this phase's tools programmatically

### External spec (coordinate bridge + action set)
- Anthropic computer-use tool `computer_20250124` action schema (screenshot / mouse / keyboard + `scroll` / `triple_click` / `hold_key` / `wait`) and its advertised-resolution / coordinate-scaling guidance (Pitfall 6) — researcher should confirm the exact action discriminant field names against current Anthropic docs
- rmcp 2.x server/tool API (D-26 pins rmcp 2.2.0) — stdio transport + tool registration idiom

</canonical_refs>

<deferred>
## Deferred Ideas

No scope-creep ideas surfaced during context gathering. For completeness, the following are explicitly NOT this phase (owned elsewhere):

- MCP progress notifications (`notifications/progress`) for long file transfers — Future Requirements (deferred); MCP-06 is satisfied by per-call task isolation + bounded timeouts, not by streaming progress.
- Scripted MCP proof harness (PROOF-03) and live-LLM capstone (PROOF-04) — Phase 15; this phase builds the tools those harnesses exercise.
- HTTP/SSE MCP transport — rejected in D-14.1; stdio only for v1.1.

</deferred>

---

*Phase: 14-MCP Server Surface*
*Context gathered: 2026-07-10*
