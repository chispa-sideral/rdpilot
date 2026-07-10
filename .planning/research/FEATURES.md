# Feature Research

**Domain:** Consumer surfaces for an RDP-perception SDK — session daemon, CLI, dual-mode MCP server (Anthropic computer-use-compatible + rdpilot-native tools), bidirectional file transfer
**Researched:** 2026-07-10
**Confidence:** HIGH (computer-use schema, MCP tool/resource conventions verified against current official docs and reference implementation) / MEDIUM (CLI daemon UX, file-transfer semantics — verified against well-known analogous tools, not an rdpilot-specific spec)

This research covers ONLY the v1.1 consumer-surface features. The RDP core (IronRDP session, screenshot capture, input injection, DVC transport) and the C# sensor (window list, process tree, UIA tree, focus, launch) are v1.0-complete and are treated here strictly as dependencies, not subjects of research.

## Feature Landscape

### Table Stakes (Users Expect These)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Full Anthropic computer-use action vocabulary as MCP tool(s): `screenshot`, `cursor_position`, `mouse_move`, `left_click`, `right_click`, `middle_click`, `double_click`, `triple_click`, `left_click_drag`, `left_mouse_down`, `left_mouse_up`, `key`, `hold_key`, `type`, `scroll`, `wait` | Claude's computer-use system prompt hard-codes this exact vocabulary; if the MCP surface doesn't reproduce it faithfully, "computer-use-compatible" is false advertising and drop-in agent loops (e.g. the Anthropic quickstart loop) won't work unmodified | MEDIUM | Anthropic's `computer` tool is **schema-less on the Anthropic API side** (Claude's model weights encode the schema) — but MCP has no schema-less tool concept; every MCP tool needs an explicit JSON `inputSchema`. rdpilot's MCP server must **author its own JSON schema** that mirrors the `computer_20250124`/`computer_20251124` action/parameter shape (`action`, `coordinate`, `text`, `duration`, `scroll_direction`, `scroll_amount`, `start_coordinate`, `region`). This is new authoring work, not a re-export. |
| Session lifecycle CLI verbs: `connect`, `list`, `disconnect` | Every daemon-backed CLI (tmux, docker, ssh-agent, adb) has this exact triad; a session-oriented CLI without discoverable "what's running" and explicit teardown feels broken | LOW–MEDIUM | Depends on: session daemon (new), session registry (new). No SDK-perception dependency — this is pure daemon/IPC plumbing. |
| Explicit session targeting on every command (no implicit default/last-used session) | Already an explicit PROJECT.md decision; matches the safety-first posture used elsewhere (no ambient state a stray command can hit) | LOW | Pure CLI/MCP argument-passing discipline; zero SDK dependency. |
| MCP daemon client (MCP server is a thin, mostly stateless process; the daemon is the actual state holder) | Matches current (2026) MCP community guidance: server processes should mint/accept an explicit handle (session name) passed back as an ordinary tool argument rather than binding RDP state to the MCP transport-level session id, because MCP server processes restart independently of long-lived RDP sessions | MEDIUM | Depends on: session daemon + local IPC (new). This is the correct pattern for rdpilot specifically because the daemon already exists as the durable state owner — the MCP server would be actively wrong to *also* try to hold RDP session state per MCP connection. |
| rdpilot-native perception/control tools exposed as MCP tools (not resources) | The model must actively decide *when* to re-query window list / UIA tree / process tree as part of an autonomous loop; MCP Resources are host-attached/subscribed context, not something the calling model reliably invokes on demand across all clients | LOW | Thin adapters over v1.0 SDK APIs (`PERC-01..04`, `API-02`/`WorldState`, `PROC-01`, `SENSOR-01..03`). No new RDP-layer engineering — pure MCP-schema authoring + daemon dispatch. |
| File transfer `put` (local→remote) / `get` (remote→local) on both CLI and MCP | Bidirectional file transfer is explicitly named as a v1.1 target feature; "put/get" is the universal naming convention (scp, sftp, docker cp variants, rsync) | MEDIUM–HIGH | New protocol work. RDPDR (`MS-RDPEFS` drive redirection) is *already* partially wired for sensor deployment (SENSOR-02, "RDPDR primary, WinRM fallback") — reusing/extending that channel for general-purpose file put/get is the natural extension rather than inventing a second custom transport. |
| Overwrite semantics on file transfer (explicit, not silent) | Every file-copy tool must define default clobber behavior; silent overwrite of a remote/local file is a classic footgun especially for a tool one agent-loop step away from LLM control | LOW | Convention split: `scp`/`docker cp` overwrite silently by default; `cp -n`/`rsync --ignore-existing` don't. Given rdpilot's stated "v1 is read/inspect, low blast radius" posture, **default to no-clobber with an explicit `--force`/`overwrite: true` flag** is the safer convention fit, even though put/get is itself the first "write" capability shipped. |
| Layered connection config (file + env + flags) | Universal CLI convention (docker, kubectl, aws-cli, git all layer config file → env → flag, flags always win) | LOW | Already an explicit v1.1 target feature; no ambiguity in ordering — flags override env override config file is the near-universal precedent. |
| Daemon auto-discovery / auto-start on first client command | Tools like `adb`, `ssh-agent`, `gpg-agent`, `docker` (via systemd socket activation) avoid forcing users to manually start a background service before every session; a CLI that errors with "daemon not running, please start it" on first use is a common UX complaint | LOW–MEDIUM | Recommend: CLI attempts IPC connect, and if the daemon isn't reachable, spawns it and retries (adb pattern) — while still supporting an explicit `daemon start/stop/status` escape hatch for scripting/systemd use. |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| **Dual tool surface in one MCP server** (Anthropic computer-use-compatible tools *and* rdpilot-native structured tools, side by side) | No known prior art (agent-rdp exposes UIA via a PowerShell/DVC agent but is a CLI, not an MCP dual surface; generic computer-use MCP shims only wrap pixels/input) does both in one server. This is the actual differentiator named in PROJECT.md's v1.1 goal. | MEDIUM | Composition risk: exposing ~25+ tools (16 computer-use actions + 8+ native tools) in one MCP `tools/list` risks context/tool-selection bloat for the calling model. Consider grouping (e.g. a single schema-less-*style* `computer` mega-tool with an `action` enum param, mirroring Anthropic's own single-tool design, rather than 16 separate MCP tools) to keep the tool list compact — Anthropic's own reference tool is exactly one tool with an `action` discriminator, not sixteen. |
| **`world_state` as a single correlated MCP tool** (screenshot + window list + UIA tree in one coordinate space, already built as `API-02`) | A generic computer-use loop needs a *separate* `screenshot` call plus manual structured queries; rdpilot can offer one round-trip that gives richer grounding (pixels + accessibility tree correlated) than any pixels-only computer-use implementation, directly serving the project's stated pixels+structured core value | LOW | Pure adaptation of an already-shipped, already-latency-profiled (`23-73ms`) SDK API — the cheapest differentiator to ship. |
| **UIA tree as first-class native MCP tool (`get_uia_tree`)** | Most computer-use tooling in the wild is vision-only; text/accessibility-tree grounding lets the agent reason about exact control names/roles/bounding rects instead of guessing from pixels — directly reduces the click-precision problems Anthropic's own docs describe (small UI elements, ambiguous targets) | LOW | Already shipped as `PERC-03` with a tuned bounded-depth walk (130ms at depth 3). Because rdpilot has this, the Anthropic `zoom` action (for reading small illegible text) is **less critical** here than for pure-vision computer-use agents — worth noting as a reason it can be deprioritized relative to the rest of the vocabulary. |
| **Per-window cropped screenshot (`CAP-02`) exposed as a native tool distinct from full-desktop `screenshot`** | Anthropic's computer-use `screenshot` action is desktop-wide only; rdpilot can additionally offer a scoped, per-window capture that reduces image size/token cost and improves click precision for a known target window | LOW | Already shipped in v1.0; this is pure MCP-exposure work, not new capture logic. |
| **Session naming as durable identity** (user-chosen name survives daemon restarts, distinct from an ephemeral tmux-style auto-id) | Lets a human operator and an AI agent refer to "the same" remote target across CLI and MCP surfaces and across process restarts without re-discovering a session id each time | LOW–MEDIUM | Requires the daemon's session registry to persist name→connection-config mapping (not necessarily the live RDP socket) across daemon restarts — a small but real scope decision for the daemon's persistence design. |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|------------------|-------------|
| Binding RDP/session state to the MCP transport-level session id (relying on `mcp-session-id` / a stateful `ClientSession` to remember "which remote desktop") | Feels natural — "the MCP session IS my desktop session" | MCP server processes are expected to be restartable/horizontally-scalable independent of any single long-lived RDP connection; conflating the two means an MCP server restart silently orphans a live remote desktop session that should have outlived it | Session name/id passed as an explicit ordinary tool argument on every call (already the PROJECT.md decision); MCP server stays a thin, mostly-stateless daemon client |
| Returning raw file bytes (base64) inline in an MCP `tool_result` for `get`/`put` | Simplest possible implementation — "just base64 the file into the response" | Verified real-world limits: base64 inflates payload ~33%, and several current MCP clients/hosts truncate or reject tool results in the ~10–12KB range well before hitting any protocol-level limit — this silently corrupts or truncates anything beyond a tiny file | File transfer tools should write directly to/read directly from the local filesystem using host-side paths and return only `{path, bytes_transferred, checksum}` metadata in the tool result — never the file content itself, except optionally for genuinely tiny files (sub-few-KB) |
| One MCP tool per computer-use action (16 separate tools: `mcp_screenshot`, `mcp_left_click`, `mcp_key`, ...) | Feels like "clean" MCP tool design — one tool, one job | Bloats `tools/list` (16 CU tools + 8+ native tools ≈ 25+), increasing model tool-selection overhead/latency and context cost every turn; also diverges from Anthropic's own reference design, which is exactly *one* tool with an `action` discriminator | A single `computer` MCP tool with an `action` enum parameter (mirroring Anthropic's schema shape), plus a small number of native tools |
| Real-time bidirectional file *sync*/watch-folder mirroring | "Since we're doing file transfer anyway, why not keep folders in sync automatically" | Explicit scope creep beyond the stated "upload/download" feature; introduces conflict resolution, watch-loop resource cost, and partial-write races that have nothing to do with the read/inspect-plus-file-transfer goal of v1.1 | Discrete, explicit `put`/`get` invocations only; sync is a v2+ concern if ever justified |
| Implicit "last session" or "default session" fallback when no session is specified | Reduces typing for the common single-session case | Already explicitly rejected in PROJECT.md ("no implicit default target"); silent fallback to the wrong remote desktop is exactly the kind of ambient-state footgun a computer-use tool must avoid, since a misdirected click/type is a real-world side effect on someone's Windows box | Every command/tool call requires an explicit session name; CLI can offer a *documented* shell/env convenience (e.g. `$RDPILOT_SESSION`) but the underlying protocol call never has an implicit default |
| MCP Prompts primitive (reusable "inspect and report" macro prompt templates) | MCP formally supports a third primitive (Prompts) alongside Tools/Resources, and it maps naturally to canned task templates | Out of scope for v1.1's stated goal (surfaces + file transfer); adds a third protocol surface to build/test for no immediate proof-harness requirement | Defer; if wanted later, ship as a thin layer on top of the already-working Tools surface, not as a v1.1 feature |
| Auto-approving destructive/consequential actions (e.g. `launch_process`, file overwrite) without any confirmation gate | Speeds up the agent loop, avoids interrupting automation | Anthropic's own computer-use guidance explicitly recommends human confirmation for consequential actions; v1.1 is adding the *first* write-capable verbs (`launch_process`, file `put`) to a project whose stated v1 posture is deliberately "low blast radius, read/inspect" | No blanket auto-approve; at minimum, default-no-clobber on file `put`, and treat `launch_process`/write verbs as explicitly opt-in per the requirements phase, not silently expanded scope |

## Feature Dependencies

```
Session daemon (new: IPC transport + session registry)
    └──requires──> none from v1.0 SDK directly (new subsystem)

CLI surface
    └──requires──> Session daemon (IPC client)
    └──requires──> Layered connection config (host/creds resolution before connect)

MCP server — native tools (world_state, get_uia_tree, get_window_list,
get_process_tree, launch_process, set_foreground_window, session connect/list/disconnect)
    └──requires──> Session daemon (IPC client)
    └──requires──> v1.0 SDK APIs: API-02 WorldState, PERC-01..04, PROC-01, SENSOR-01..03
                       (all already shipped — pure adapter work)

MCP server — computer-use-compatible tools (screenshot, mouse_move, click
variants, type, key, scroll, cursor_position, wait, ...)
    └──requires──> Session daemon (IPC client)
    └──requires──> v1.0 SDK APIs: CAP-01/CAP-02 (screenshot), INPUT-01/INPUT-02 (input)
    └──requires──> new: MCP JSON-schema authoring (Anthropic's schema-less tool has
                       no independent JSON schema to import — must be hand-authored)
    └──requires──> new (likely): cursor_position tracking (RDP protocol does not
                       report server-side cursor position back to the client; must
                       be tracked client-side from last-commanded mouse_move/click)

Bidirectional file transfer (put/get)
    └──requires──> RDPDR/MS-RDPEFS drive-redirection channel (partially wired,
                       SENSOR-02 "RDPDR primary" — extend, don't reinvent)
    └──enhances──> CLI surface (put/get verbs)
    └──enhances──> MCP server, both tool surfaces (put/get tools)
    └──requires──> Session daemon (a live session to attach the transfer to)

Proof harnesses (per-surface scripted + capstone live-LLM MCP demo)
    └──requires──> all of the above, functioning end-to-end
```

### Dependency Notes

- **CLI and MCP server both require the session daemon:** both are described in PROJECT.md as "thin clients" over the daemon; the daemon must exist and expose a stable local IPC contract before either consumer surface can be built. This makes the daemon the correct first phase of this milestone.
- **MCP native tools enhance but do not require the computer-use tools:** they wrap entirely separate, already-shipped SDK surfaces (perception/process/sensor APIs) and could ship independently of the computer-use-compatible tool set if sequencing required it.
- **`cursor_position` and MCP schema-authoring are the two "hidden" pieces of new engineering inside the computer-use vocabulary** that aren't simply 1:1 wrappers over existing SDK verbs — everything else in the vocabulary maps directly onto `CAP-01/02` and `INPUT-01/02`.
- **File transfer enhances (does not block) the rest of v1.1:** CLI/MCP/daemon can all be built and proven for perception+input+launch before file transfer is layered in, but file transfer's *value* is realized only once both surfaces exist to expose it through.
- **File transfer conflicts with naive MCP content-inlining:** as documented in Anti-Features, returning file bytes through the MCP protocol layer directly conflicts with real MCP client size limits — this is a hard architectural constraint, not a style preference.

## MVP Definition

### Launch With (v1.1 core)

- [ ] Session daemon: connect/keepalive/registry over local IPC — foundation everything else sits on
- [ ] CLI: `connect` / `list` / `disconnect` + perception/input/launch verbs, explicit session targeting
- [ ] MCP server: single `computer` tool (action-discriminated, full Anthropic vocabulary) + native tools (`world_state`, `get_uia_tree`, `get_window_list`, `get_process_tree`, `launch_process`, `set_foreground_window`, session connect/list/disconnect)
- [ ] File transfer `put`/`get` on both surfaces, default no-clobber, metadata-only MCP responses (no inline base64 for non-trivial files)
- [ ] Layered connection config (file + env + flags)
- [ ] Per-surface scripted proof harness + capstone live-LLM MCP demo (already the stated v1.1 finish line)

### Add After Validation (v1.1.x, if scope allows)

- [ ] Transfer progress reporting via MCP's `notifications/progress` mechanism (tied to a `progressToken`) rather than polling — natural fit once `put`/`get` exist and are observed to be slow enough to need it
- [ ] Daemon auto-start-on-first-use (adb-style) if manual `daemon start` friction proves real in practice

### Future Consideration (v2+)

- [ ] `zoom` action support (lower priority here specifically *because* the UIA tree already gives exact-text grounding that pure-vision computer-use setups rely on `zoom` for)
- [ ] MCP Prompts primitive (canned task templates)
- [ ] File sync/watch-folder mirroring — explicitly rejected as anti-feature above, revisit only with strong justification
- [ ] Multi-session concurrency orchestration (already out of scope per PROJECT.md)

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|----------------------|----------|
| Session daemon (IPC + registry) | HIGH | MEDIUM | P1 |
| CLI session lifecycle (connect/list/disconnect) | HIGH | LOW | P1 |
| CLI perception/input/launch verbs | HIGH | LOW (thin wrap over v1.0 SDK) | P1 |
| MCP `computer` tool (full CU vocabulary) | HIGH | MEDIUM (schema authoring + cursor_position tracking) | P1 |
| MCP native tools (world_state, UIA, windows, processes, launch, focus) | HIGH | LOW (thin wrap over v1.0 SDK) | P1 |
| File transfer put/get (both surfaces) | HIGH | MEDIUM–HIGH (new RDPDR-based protocol work) | P1 |
| Layered connection config | MEDIUM | LOW | P1 |
| Metadata-only MCP file-transfer responses (no inline base64) | HIGH (correctness, not just polish) | LOW | P1 |
| Default no-clobber + explicit overwrite flag | MEDIUM | LOW | P1 |
| Daemon auto-start-on-first-use | MEDIUM | LOW–MEDIUM | P2 |
| Transfer progress via MCP `notifications/progress` | MEDIUM | MEDIUM | P2 |
| `zoom` action | LOW (mitigated by UIA tree) | LOW (image-crop only, no new SDK primitive) | P2 |
| MCP Prompts primitive | LOW | MEDIUM | P3 |
| File sync/watch mirroring | LOW | HIGH | P3 (anti-feature — do not build) |

## Competitor / Reference-Implementation Analysis

No direct competitor ships this exact combination (RDP transport + dual computer-use/native MCP surface + CLI daemon); the closest and most relevant references are the Anthropic reference implementation (for the computer-use half) and `agent-rdp` (for the "RDP + structured perception over a virtual channel" half). Neither combines both, which is exactly rdpilot's differentiator.

| Feature | Anthropic reference implementation | agent-rdp (thisnick) | rdpilot's approach |
|---------|-------------------------------------|-----------------------|---------------------|
| Action vocabulary | Single schema-less `computer` tool, `action`-discriminated, Xvfb/Linux sandbox target | Not computer-use-schema-shaped; CLI tool over IronRDP | Reuse Anthropic's exact vocabulary/shape but target a real Windows RDP desktop instead of a local Xvfb sandbox |
| Structured perception | None (pixels only) | UIA tree over DVC via a PowerShell agent (closest prior art) | Native MCP tools for UIA tree/window list/process tree, already shipped in v1.0, newly exposed via MCP/CLI |
| Consumer surface | Web UI + Docker container + example agent loop (not packaged as MCP) | CLI only, not an SDK, not MCP | Daemon + CLI + dual-mode MCP server, explicitly the SDK's first non-scripted consumer layer |
| File transfer | Not part of computer-use tool itself (would go through `bash`/`text_editor` companion tools) | Not documented | RDPDR/MS-RDPEFS-based put/get, exposed on both CLI and MCP |
| Session model | Single implicit session per API conversation (no session id concept) | Not documented as multi-session | Named, explicit, no-implicit-default session identity across both surfaces |

## Sources

- [Claude Platform Docs — Computer use tool](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool) — HIGH confidence; official, current (references Claude Sonnet 5/Opus 4.8-era models), verified action vocabulary, tool parameters, agent-loop shape, screenshot scaling/downscale guidance, error handling patterns
- [claude-quickstarts computer_use_demo/tools/computer.py](https://github.com/anthropics/claude-quickstarts/blob/main/computer-use-demo/computer_use_demo/tools/computer.py) — HIGH confidence; canonical reference implementation, confirms per-version action availability (`computer_20241022` base set, `computer_20250124` additions, `computer_20251124` zoom) and exact parameter names (`coordinate`, `start_coordinate`, `text`, `duration`, `scroll_direction`, `scroll_amount`, `region`) plus `cursor_position` response shape (`X=..,Y=..` text)
- MCP session-state discussion (community/vendor sources: LangChain MCP docs, CodeSignal "Managing Stateful MCP Server Sessions", MCP GitHub Discussion #102 "State, and long-lived vs. short-lived connections", MCPcat multi-connection guide) — MEDIUM confidence, multiple independent sources agree on the "explicit handle passed as ordinary tool argument" pattern as the modern recommended approach over binding state to transport-level session ids
- MCP file/binary-content size-limit issues (anthropics/claude-code#50358, #54137, #15722; github/copilot-cli#1732; modelcontextprotocol/modelcontextprotocol Discussion #1197; HackerNoon "Multi-Modal MCP Servers") — MEDIUM confidence; consistent, independently-reported real-world truncation/corruption behavior around base64 binary content in MCP tool results across multiple different MCP clients, supporting the "never inline file bytes" anti-feature finding
- tmux session-management conventions (tmux.app, man7.org tmux(1), terminal.guide) — HIGH confidence for the daemon/client + named-session CLI convention this milestone's CLI/daemon design should follow
- PROJECT.md (`.planning/PROJECT.md`) — authoritative source for v1.0-shipped SDK surface (`CAP-01/02`, `INPUT-01/02`, `SENSOR-01..03`, `PERC-01..04`, `PROC-01`, `API-01/02`) that every v1.1 feature here wraps or extends

---
*Feature research for: rdpilot v1.1 consumer surfaces (session daemon, CLI, dual-mode MCP server, bidirectional file transfer)*
*Researched: 2026-07-10*
