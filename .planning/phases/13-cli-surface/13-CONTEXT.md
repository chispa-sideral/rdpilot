# Phase 13: CLI Surface - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 13 delivers a thin `rdpilot` command-line binary that drives the full session / perception / input / file verb set over the Phase 12 daemon via the Phase 11 `rdpilot-ipc` transport — with every command explicitly targeting a named session. The CLI holds NO RDP session state itself: it is an invoke-and-exit `rdpilot-ipc` client that transparently auto-starts the daemon on first use, sends one request, renders the reply, and exits.

**Depends on:** Phase 12 (Session Daemon) — the CLI is a thin IPC client of the daemon; Phase 11 (`rdpilot-ipc` wire protocol + `rdpilot-config`) — request/response DTOs and layered config resolution.
**Requirements:** CLI-01, CLI-02, CLI-03.

**Success Criteria** (from ROADMAP.md, verbatim):
1. `rdpilot connect [--name] / list / disconnect` manages session lifecycle over the daemon, transparently auto-starting it on first use (CLI-01).
2. The full perception + input + launch verb set (screenshot, world_state, uia, window/process list, click/type/key/scroll/drag, launch, foreground) runs against an explicit `--session` (CLI-02).
3. `put`/`get` transfer files (no-clobber by default with an explicit `--force`), and failures — session-not-found, daemon-unreachable, transfer failure — surface as distinct, legible errors (CLI-03).

**Out of scope:** the daemon itself and the session registry (Phase 12); the wire protocol / config crates (Phase 11); the MCP server surface (Phase 14); the scripted CLI proof harness — PROOF-02 (Phase 15, which consumes this CLI via `--json`). This phase only builds the CLI binary on top of the already-shipped daemon + IPC + config foundation.

</domain>

<decisions>
## Implementation Decisions

### Phase-Local (this phase owns these)

- **D-13.1 — CLI shape: GROUPED noun-verb subcommands with flat lifecycle/file verbs; human tables by default, `--json` opt-in; clap 4.x.**
  The CLI is organized as grouped noun-verb subcommand families — `session`, `perceive`, `input`, `file` — for the perception/input/file verb sets, while the session-lifecycle verbs (`connect` / `list` / `disconnect`) and the file transfer verbs (`put` / `get`) are ALSO exposed FLAT at the top level for ergonomics (e.g. `rdpilot connect`, `rdpilot put`, `rdpilot get`). Default output is HUMAN-READABLE TABLES; a `--json` flag opts into machine-readable output for scripting (PROOF-02 in Phase 15 consumes the CLI exclusively via `--json`). The `screenshot` verb writes its image to a REQUIRED `--output <path>` argument (binary image bytes are never dumped to stdout). Built on `clap` 4.x (per D-26). Perception/input verb naming (e.g. `screenshot`, `world-state`, `uia`, `window list` / `process list`, `click` / `type` / `key` / `scroll` / `drag`, `launch`, `foreground`) is to be FINALIZED in planning — the grouped-vs-flat structure and the table/`--json`/`--output` contract are locked here.
  **Rationale:** grouped subcommands keep a large verb set discoverable while flat lifecycle/file verbs stay ergonomic for the common path; tables-by-default serve the human operator and `--json` serves the scripted proof harness without a second code path; a required `--output` avoids binary-on-stdout corruption.

### Locked (carried into planning, do not re-open)

- **Lifecycle + file verb set is fixed:** `connect` / `list` / `disconnect` (CLI-01) and `put` / `get` (CLI-03) are the committed lifecycle and file verbs.
- **File transfer default is no-clobber:** `put`/`get` refuse to overwrite an existing destination by default; an explicit `--force` flag is the only override.

### Claude's Discretion (planner/researcher may refine)

- Exact final spelling of each perception/input verb and whether they live only under their noun group, only flat, or both (D-13.1 fixes the grouping model, not every leaf name).
- Table-rendering approach (hand-rolled vs a formatting crate) — keep dependency footprint minimal and consistent with D-26's stack.
- How `--session` is surfaced in clap (global arg vs per-subcommand) so long as D-29's "required on every command" contract holds.

</decisions>

<inherited_decisions>
## Inherited / Cross-Cutting Decisions

This phase inherits and is a primary CONSUMER of the following cross-cutting decisions recorded in `.planning/DECISIONS-INDEX.md`. It does not re-open them.

- **D-27 (CC1 — Config):** The CLI resolves target host + credentials through the layered `config.toml` (platform config dir) -> `RDPILOT_`-prefixed env vars -> CLI flags precedence chain. CLI flag spellings MUST reuse the EXACT config-key vocabulary verbatim (host, port, username, password, domain, cert-bypass) so file/env/flag stay consistent. The `rdpilot-config` crate (Phase 11) provides the resolver; the CLI supplies the flag layer.
- **D-28 (CC2 — Error taxonomy -> distinct CLI exit codes):** The CLI maps the wire `WireError { code, message }` fixed code set (`session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`) to a DISTINCT non-zero process exit code per error class, so PROOF-02 can script against exit codes. This directly satisfies CLI-03's "distinct, legible errors" for session-not-found / daemon-unreachable / transfer-failure.
- **D-29 (CC3 — `--session` on every command):** There is NO implicit/default session target. The CLI requires a global `--session <name-or-id>` on EVERY session-scoped command (reinforces D-18). Auto-generated ids are short adjective-noun word-pairs (e.g. `brave-otter`) minted daemon-side.
- **D-30 (CC4 — lifecycle status vocabulary):** `rdpilot list` renders the daemon's shared lifecycle status enum — `Connecting` / `Live` / `Reconnecting` / `Disconnected` — alongside name/id, target, and activity timestamps, using the same vocabulary the MCP surface (Phase 14) renders.

Additional milestone-level decisions this phase operates under:

- **D-16:** v1.1 ships exactly two consumer surfaces (CLI + MCP); no language bindings, no clipboard.
- **D-17:** The CLI is a THIN client of the single long-lived session daemon over local IPC — it does not embed or duplicate session/keepalive logic.
- **D-18:** Every command explicitly targets a session by name/id (enforced as a required field; see D-29).
- **D-26:** Stack: `clap` 4.6.1 for the CLI; `interprocess` 2.4.2 / native named-pipe for IPC to the daemon.

</inherited_decisions>

<canonical_refs>
## Canonical References

**Downstream researcher / planner MUST read these before planning or implementing.**

### Roadmap / Requirements / Decisions
- `.planning/ROADMAP.md` §"Phase 13: CLI Surface" (lines 89-100) — goal, success criteria, requirement IDs, dependency on Phase 12.
- `.planning/REQUIREMENTS.md` CLI-01, CLI-02, CLI-03 (lines 26-28) — the CLI requirement statements.
- `.planning/DECISIONS-INDEX.md` — inherited cross-cutting decisions D-16, D-17, D-18, D-26, D-27 (CC1), D-28 (CC2), D-29 (CC3), D-30 (CC4).

### Upstream phase context (dependencies)
- `.planning/phases/11-*/11-CONTEXT.md` (when written) — `rdpilot-ipc` wire DTOs and `rdpilot-config` resolution the CLI is a client of.
- `.planning/phases/12-*/12-CONTEXT.md` (when written) — daemon IPC transport, registry, auto-start / list semantics the CLI drives.
- `.planning/phases/10-sdk-file-transfer-extension/10-CONTEXT.md` — file-transfer wire contract (`Session::upload_file`/`download_file`, `TransferOutcome { bytes_transferred, checksum }`) that the `put`/`get` verbs ultimately trigger through the daemon.

### Source files (phase-relevant)
- `crates/rdpilot/src/error.rs` — SDK `Error` enum whose variants (`PathTraversal`, `ChecksumMismatch`, `SensorRejected`, `Dvc`, ...) are mapped 1:1 into the wire `WireError` code set that the CLI translates to exit codes (D-28).
- `crates/rdpilot/src/config.rs` — `ConnectionConfig` builder and existing key vocabulary (host/port/username/password/domain/cert-bypass) that D-27's config keys and CLI flag spellings must match verbatim.
- `crates/rdpilot/src/keepalive.rs` — the keepalive signal from which the daemon derives the `Connecting`/`Live`/`Reconnecting`/`Disconnected` status vocabulary D-30's `list` renders.
- `crates/rdpilot/src/session.rs` — the public perception/input/launch/file methods (`screenshot`, `world_state`, `get_uia_tree`, window/process list, click/type/key/scroll/drag, `launch_process`, `set_foreground_window`, `upload_file`/`download_file`) the CLI verb set maps onto through the daemon.
- `crates/rdpilot/examples/proof_harness.rs` — existing scripted-proof precedent; PROOF-02 (Phase 15) will drive this CLI via `--json` the way the harness drives the SDK today.

</canonical_refs>

<deferred>
## Deferred Ideas

No scope-creep ideas surfaced during this phase's context gathering. The MCP `put`/`get` tool, the scripted CLI proof harness (PROOF-02), and the daemon/IPC/config crates all belong to other phases (14, 15, 12, 11 respectively) and are already tracked there.

</deferred>

---

*Phase: 13-CLI Surface*
*Context gathered: 2026-07-10*
