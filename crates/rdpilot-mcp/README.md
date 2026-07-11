# rdpilot-mcp

An MCP (Model Context Protocol) server exposing a running `rdpilot` daemon
over stdio (D-14.1). Bin-only, thin client: this crate links only
`rdpilot-ipc`/`rdpilot-config`/`rmcp`/`schemars`/`tokio` — never
`rdpilot`/`rdpilot-daemon` directly, and never IronRDP/rustls (D-17). Every
tool call is a fresh, one-shot round trip to the daemon over the local IPC
socket (auto-starting the daemon on first use); this binary itself holds no
live RDP connection state.

## Tool surface

Twelve tools are advertised (MCP-01):

- `computer` — the Anthropic `computer_20250124`-compatible mega-tool
  (screenshot/mouse/keyboard/scroll), coordinates in a fixed 1280x800 space
  bridged to the target session's native resolution (MCP-02/MCP-04, D-14.2).
- Eleven `rdpilot_`-prefixed native tools (MCP-03), each a 1:1 wrapper over
  one `rdpilot-ipc` wire verb: `rdpilot_world_state`, `rdpilot_uia`,
  `rdpilot_window_list`, `rdpilot_process_list`, `rdpilot_launch`,
  `rdpilot_foreground`, `rdpilot_connect`, `rdpilot_list`,
  `rdpilot_disconnect`, `rdpilot_put`, `rdpilot_get`. The `rdpilot_` prefix
  avoids name collisions with `computer` and with any other MCP server the
  host has loaded (D-14.2).

## Session targeting (D-29)

There is no implicit "current session" and no one-session-per-server init
binding. Every tool call that operates on a live remote desktop carries an
explicit, required `session` parameter — an unrecognized/missing session is
a schema-validation rejection (never a silent default), asserted for every
advertised tool by `tests/tool_schema.rs`.

**Exactly two tools are the deliberate exception**, and only because the
wire verb they map onto is itself session-less:

- `rdpilot_connect` maps onto `Request::Connect` — it *creates* a session,
  so there is nothing yet to target.
- `rdpilot_list` maps onto `Request::List` — it enumerates every session
  the daemon knows about, so it targets no single one.

This mirrors `rdpilot-ipc`'s own compile-time-enforced model exactly
(`SessionScoped::session()` returns `None` for precisely these two `Request`
variants, `Some(_)` for every other one, SESSION-01/03) — adding an unused
`session` field to either tool's schema would not correspond to anything on
the wire, so deliberately it does not exist there.

## File transfer is metadata-only (MCP-05)

`rdpilot_put`/`rdpilot_get` return `{path, bytes_transferred, checksum}`
and nothing else. This is a structural guarantee, not application-level
stripping logic: the wire response type they render
(`rdpilot_ipc::TransferOutcome`) has exactly two fields
(`bytes_transferred`, `checksum`) and *no byte-buffer field at all* — file
bytes cannot appear in a tool result because there is nowhere on the DTO for
them to live. `path` is the caller-supplied local path, echoed back as
addressing metadata only; it is never read as file content by the rendering
code. A planted-sentinel regression test
(`native_tools::tests::put_get_transfer_result_never_carries_the_planted_file_bytes_sentinel`)
guards this: it plants a real file on disk containing a known byte sequence
and asserts the sentinel never appears in the serialized tool result.

Both the daemon and this MCP server (like the CLI, Phase 13) run on the same
machine — no file bytes ever cross the local IPC socket at all, in either
direction; only paths and `TransferOutcome` metadata do.

## Trust model (developer decision — read before exposing this server to an untrusted prompt source)

**This server implements NO server-side local-path sandbox.** `rdpilot_put`
reads an *arbitrary* local file path with the daemon's own OS permissions
and uploads it to the remote target; `rdpilot_get` writes to an arbitrary
local destination path. Local paths behave exactly as they do for the
`rdpilot` CLI's trusted-operator model (CLI-03) — this MCP server adds no
additional restriction on top of that.

**Why this matters specifically for the MCP surface:** unlike the CLI
(driven directly by a human at a terminal), an MCP server exposes the
identical capability to an LLM-driven agent. A prompt-injected or
compromised caller could invoke `rdpilot_put({session, local_path:
"/home/user/.ssh/id_rsa", remote_name: "notes.txt"})` and exfiltrate any
locally-readable file to the remote Windows target, from which further
exfiltration becomes possible (e.g. via a remote web browser). This is
tracked in the phase's threat register as **T-14-12** (Information
Disclosure, high severity, disposition: **accept**, mitigated only by the
minimal measures below).

**The MCP host is the tool-approval/sandboxing boundary.** Responsibility
for deciding whether — and under what constraints — to expose the
`rdpilot_put` tool to a given prompt source belongs to whoever configures
the MCP host (approval prompts, path allow-lists, running the server under a
restricted OS user, etc.). `rdpilot-mcp` itself does not implement any of
those controls.

**Minimal mitigations shipped this phase (cost: documentation only):**

- This README section.
- A prominent risk note directly in the `rdpilot_put` tool's own MCP
  `description` string, visible to the calling model at tool-selection time
  without needing to read this file.

**Backlog (not a Phase 14 deliverable):** a server-side transfer sandbox
(e.g. a configurable local-path allow-list/root-confinement for
`rdpilot_put`/`rdpilot_get`) is a named, deferred hardening item — see the
Phase 14 research document's "Open Question 2" (local-path exfiltration
risk unique to the MCP surface). No phase currently owns this work; a
future phase should pick it up before this server is recommended for use
with untrusted/adversarial prompt sources.

## Config resolution (D-27)

`rdpilot_connect` resolves `host`/`port`/`username`/`password`/`domain`/
`accept_invalid_certs` through the exact same file → env → override layered
pipeline the CLI uses (`rdpilot_config::resolve`, CONFIG-01). The MCP-init
parameter layer (`McpConnectParams`, this crate's `config_params` module)
reuses the D-27 key vocabulary verbatim — the CLI's `--host` flag and this
tool's `host` parameter are the same key, just arriving through two
different typed front-ends onto the one shared resolution pipeline. As with
the CLI, prefer the config file or `RDPILOT_PASSWORD` over passing
`password` directly through a tool call where avoidable.
