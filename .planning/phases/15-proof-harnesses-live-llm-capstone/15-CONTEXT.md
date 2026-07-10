# Phase 15: Proof Harnesses & Live-LLM Capstone - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Both consumer surfaces (CLI and MCP) are proven end-to-end by scripted, no-live-LLM harnesses, and a committed live-LLM capstone drives a real read/inspect + file-transfer task through the MCP surface against a real remote-only Windows program — the v1.1 milestone's dual finish line. This phase writes no new SDK/daemon/CLI/MCP capability; it exercises and proves the full stack that Phases 10-14 built.

**Depends on:** Phase 14 (the MCP server surface) — and transitively every prior v1.1 phase (11 shared wire protocol + config, 12 session daemon, 13 CLI, 14 MCP). This phase validates the combination of every prior phase functioning end-to-end.
**Requirements:** PROOF-02, PROOF-03, PROOF-04.

**Success Criteria** (from ROADMAP.md, verbatim):
1. A scripted harness proves the CLI surface end-to-end against a real remote-only Windows program, with no live LLM (PROOF-02).
2. A scripted harness proves the MCP surface end-to-end — tool calls exercised programmatically — with no live LLM (PROOF-03).
3. A capstone live-LLM demo drives a read/inspect + file-transfer task through the MCP surface against a real remote-only Windows program (PROOF-04).

**Out of scope:** any new daemon/CLI/MCP feature (those are Phases 12/13/14); MCP progress notifications for long transfers (deferred, REQUIREMENTS.md Future); durable session reattach; PyO3/NAPI bindings. This phase only adds proof harnesses and the capstone agent loop over the already-shipped surfaces.

</domain>

<decisions>
## Implementation Decisions

Phase-local decisions use the repo's `D-<phase>.<n>` convention (matching Phase 10's `D-10.x`); cross-cutting decisions this phase inherits live in DECISIONS-INDEX.md and are listed under `<inherited_decisions>` below.

- **D-15.1:** Proof + capstone run at FULL FIDELITY, no mocks of the surfaces under test. The CLI proof harness (PROOF-02) spawns the REAL `rdpilot` CLI binary as a subprocess, and the MCP proof harness (PROOF-03) spawns the REAL `rmcp` stdio server as a subprocess and speaks the actual MCP protocol over its stdin/stdout — both driving the live session daemon over its real local IPC transport against a live Azure VM. Both harnesses reuse the Phase 9 live-test scaffolding verbatim: the `RDPILOT_LIVE` env gate, `.secrets/connection.json` loading (`crates/rdpilot/examples/proof_harness.rs`), and the plain-stdout step-by-step `PASS`/`FAIL` trace with no JSON schema (D-9.4). The capstone (PROOF-04) is a COMMITTED, `RDPILOT_LIVE` + Anthropic-API-key-gated agent loop that calls the real Anthropic API and drives a read/inspect + file-transfer task purely through the MCP surface (no direct SDK/daemon shortcuts). For continuity with the Phase 9 proof, all three target the v1.0 7-Zip File Manager window (`class_name` = `"7-Zip::FM"`) as the real remote-only program.
  **Rationale:** the whole point of the milestone finish line is that the surfaces are proven as real consumers see them; spawning the actual CLI/MCP binaries over live IPC + a live VM is the only thing that proves the end-to-end composition. Reusing the Phase 9 gate/secrets/trace conventions avoids inventing a second live-test idiom and keeps the capstone a permanent, re-runnable artifact rather than a throwaway demo.

## Claude's Discretion

- Exact task/prompt the capstone LLM is given (as long as it exercises both read/inspect via perception tools AND a file put/get), the Anthropic model id, and the max-turns/timeout budget of the agent loop — researcher/planner may refine within the gated, committed constraint.
- Whether the CLI and MCP proof harnesses live as `cargo test` gated integration tests, `--example` binaries mirroring `examples/proof_harness.rs`, or a mix — follow the existing Phase 9 harness placement (`crates/rdpilot/examples/proof_harness.rs` + `crates/rdpilot/tests/support/proof_harness.rs`) unless a better structure emerges.
- Exact set of MCP tool calls the PROOF-03 harness exercises programmatically (must at minimum cover session connect/list/disconnect, one perception read, and file put/get), and how CLI distinct non-zero exit codes (D-28) are asserted in PROOF-02.

</decisions>

<inherited_decisions>
## Inherited / Cross-Cutting Decisions

This phase inherits the WHOLE v1.1 stack — it validates every prior phase end-to-end — so effectively every cross-cutting decision (D-16 through D-30) is exercised here. The ones this phase directly owns or asserts against:

- **D-25 (DECISIONS-INDEX.md, Proof):** Dual finish line — a scripted proof harness per surface (no live LLM) AND a capstone live-LLM demo driving a read/inspect + file-transfer task through the MCP surface. Already LOCKED: THAT the live-LLM capstone runs through the MCP server (not the CLI, not the raw SDK). This phase implements that decision; D-15.1 above only fixes the fidelity/continuity details (real binaries, `RDPILOT_LIVE` gate, 7-Zip::FM target).
- **D-27 (CC1 — Config, phases 11/13/14):** The `config.toml` platform-config-dir file layered file → env → flag/MCP-init with the `RDPILOT_` env prefix and identical key spellings across surfaces. The proof harnesses consume this config path to point the CLI/MCP surfaces at the live VM; the capstone relies on it for credential resolution. Inherited, not re-opened.
- **D-28 (CC2 — Errors, phases 11/13/14):** Wire error taxonomy `WireError { code, message }` with the fixed code set (`session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`) mapped from the Phase 10 SDK `Error` variants; the CLI emits a DISTINCT non-zero exit code per class. PROOF-02's scriptability depends directly on these distinct exit codes — the CLI harness asserts them. Inherited, not re-opened.
- **D-29 (CC3 — Session, phases 12/13/14):** No implicit default target — the CLI requires `--session` on every command and the MCP server requires a `session` parameter on every tool call; auto-ids are short adjective-noun word-pairs. Both proof harnesses and the capstone must pass an explicit session on every call. Inherited, not re-opened.
- **D-30 (CC4 — Session, phases 12/13/14):** The `list` lifecycle status vocabulary (`Connecting` / `Live` / `Reconnecting` / `Disconnected`) derived from the SDK keepalive signal. The proof harnesses observe this vocabulary when asserting session state. Inherited, not re-opened.

</inherited_decisions>

<canonical_refs>
## Canonical References

**Downstream agents (researcher/planner/executor) MUST read these before planning or implementing.**

### Roadmap / Requirements / Decisions
- `.planning/ROADMAP.md` §"Phase 15: Proof Harnesses & Live-LLM Capstone" (lines 116-127) — goal, success criteria, requirement IDs
- `.planning/REQUIREMENTS.md` PROOF-02, PROOF-03, PROOF-04 (lines 54-56) — and the Future/Out-of-scope sections (deferred MCP progress notifications, etc.)
- `.planning/DECISIONS-INDEX.md` — D-25 (proof dual finish line), D-27/D-28/D-29/D-30 (config/errors/session/list cross-cutting decisions this phase asserts against), plus the full D-16..D-30 stack it validates end-to-end

### Phase 9 proof harness precedent (the pattern to reuse)
- `crates/rdpilot/examples/proof_harness.rs` — the `RDPILOT_LIVE`-gated, `.secrets/connection.json`-loading, plain-stdout `PASS`/`FAIL` trace (D-9.4); the fidelity/continuity template D-15.1 extends
- `crates/rdpilot/tests/support/proof_harness.rs` — shared harness helper logic
- `crates/rdpilot/tests/common/mod.rs` — `RDPILOT_LIVE` gating helper for gated live tests
- `crates/rdpilot/tests/live_session.rs` — existing gated live-session integration test structure

### Surfaces under test (built in Phases 11-14, read once they exist)
- The `rdpilot` CLI crate + binary (Phase 13) — spawned as a subprocess by the PROOF-02 harness; distinct exit codes per D-28
- The `rmcp` MCP server crate + stdio binary (Phase 14) — spawned as a subprocess by the PROOF-03 harness and driven by the PROOF-04 capstone
- The session daemon + `rdpilot-ipc` wire crate (Phases 11/12) — the live IPC both surfaces drive

### SDK source touchpoints
- `crates/rdpilot/src/error.rs` — SDK `Error` variants the D-28 `WireError` codes map from (`PathTraversal`, `ChecksumMismatch`, `SensorRejected`, `Dvc`, etc.)
- `crates/rdpilot/src/config.rs` — `ConnectionConfig` builder consumed by the harnesses to point surfaces at the live VM (and the D-27 config layering it feeds)
- `crates/rdpilot/src/keepalive.rs` — the keepalive signal D-30's `list` status vocabulary derives from
- `crates/rdpilot/src/session.rs` — the SDK `Session` (`upload_file`/`download_file`, perception verbs) that the daemon/CLI/MCP surfaces ultimately drive

### Live-test infrastructure
- `.secrets/connection.json` (gitignored) — live VM connection details consumed by the harnesses via the Phase 9 loader
- `infra/` + `manage-env.ps1` — Azure WS2022 live-test VM provisioning (`up`/`down`); the 7-Zip File Manager (`class_name` `"7-Zip::FM"`) is pre-installed by `Configure-Target.ps1`

</canonical_refs>

<deferred>
## Deferred Ideas

No new scope-creep ideas surfaced for this phase. Items intentionally NOT in Phase 15 (owned elsewhere or explicitly deferred in REQUIREMENTS.md):

- MCP progress notifications (`notifications/progress`) for long transfers — deferred to a future release (REQUIREMENTS.md Future Requirements); the capstone tolerates long transfers via the D-30 status vocabulary / bounded timeouts instead.
- Any new daemon/CLI/MCP feature — Phases 12/13/14; this phase only proves what they ship.
- Durable session reattach across daemon restart, PyO3/NAPI bindings, CLIPRDR clipboard — all explicitly deferred (D-16 / REQUIREMENTS.md Future).

</deferred>

---

*Phase: 15 - Proof Harnesses & Live-LLM Capstone*
*Context gathered: 2026-07-10*
