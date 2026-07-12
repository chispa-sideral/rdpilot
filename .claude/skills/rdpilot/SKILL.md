---
name: rdpilot
description: Use when you need to inspect, read, or control a remote Windows desktop that is reachable only over RDP — take a screenshot or read the UI Automation tree of a remote machine, list its windows/processes, or drive mouse/keyboard/launch input against a Windows box you cannot SSH into. Not for local/non-RDP GUI automation.
---

# rdpilot

## What rdpilot is

`rdpilot` is a thin local CLI (`rdpilot`) backed by a long-lived local session
daemon (`rdpilot-daemon`) that opens and drives an RDP connection to a remote
Windows desktop. The agent — you — always stays on the **local** machine; you
never run code, an MCP server, or anything else on the remote target. The CLI
talks to the daemon over a local Unix-domain socket; the daemon holds the
actual RDP session and forwards perception reads (screenshots, UI Automation
tree, window/process lists) and input writes (click/type/key/launch) to the
remote machine over the RDP connection.

## Quickstart

```bash
# One-time setup (installs rdpilot + rdpilot-daemon, seeds config, installs
# this skill to both discovery surfaces). Safe to re-run.
scripts/install-local.sh

# Edit the seeded config with YOUR OWN target host + credentials
# (or export RDPILOT_HOST / RDPILOT_USERNAME / RDPILOT_PASSWORD instead):
$EDITOR ~/.config/rdpilot/config.toml

# Check for a session-name collision before connecting (see Safety below)
rdpilot list

# Connect, using a self-owned, descriptive session name
rdpilot connect --name <agent>-<purpose>

# ... perceive and act (see the workflows and reference below) ...

# ALWAYS disconnect explicitly when finished
rdpilot disconnect --session <agent>-<purpose>
```

## Common read/inspect workflows

Perception is read-only and safe to run repeatedly. Lead with perception
before ever touching input.

**Screenshot the desktop:**
```bash
rdpilot perceive screenshot --session <id> --output /tmp/desktop.png
```

**Read structured world-state in one call** (screenshot + window list + UIA,
combined):
```bash
rdpilot perceive world-state --session <id> --screenshot --window-list \
  --uia-mode foreground --output /tmp/world.png
```

**Walk a specific window's UI Automation subtree:**
```bash
rdpilot perceive window list --session <id>          # find the --hwnd
rdpilot perceive uia --session <id> --hwnd <hwnd> --scope subtree --max-depth 3
```

**List windows or processes:**
```bash
rdpilot perceive window list --session <id>
rdpilot perceive process list --session <id>
```

Only after you have perceived and understand what's on screen should you move
to input verbs (below) — they mutate remote state.

## Safety & etiquette

- **Read/inspect-only by default.** The input verbs (`click`, `scroll`,
  `drag`, `type`, `key`, `launch`, `foreground`) MUTATE remote state — a
  click can close a dialog, a keystroke can submit a form. Always `perceive`
  first (screenshot and/or UIA) to confirm what you're about to act on, and
  act deliberately, one verb at a time.
- **Credentials are plaintext.** `password` in `config.toml` and
  `RDPILOT_PASSWORD` are stored/passed in plaintext (an accepted project
  risk posture, not something to work around). Never pass `--password` on
  the command line — it is visible in `ps` output and shell history. Prefer
  the config file (mode 0600) or the `RDPILOT_PASSWORD` env var.
- **Session naming — you own your session.** There is no shared or default
  session. Pick a stable, self-owned name of the form `<agent>-<purpose>`
  (e.g. `claude-inspect-billing-app`), and run `rdpilot list` **before**
  `connect` to check for a name collision. Connecting with a name that's
  already in use returns `DuplicateSession` (exit code 7) — treat the name
  as your ownership handle, not a random string.
- **Do not manage the daemon manually.** `rdpilot-daemon` auto-starts on
  first `connect` (a sibling-binary lookup, not a `$PATH` search — see the
  installer). You never need to start, stop, or restart it yourself.
- **Always `disconnect` explicitly when finished:**
  `rdpilot disconnect --session <id>`. The daemon's idle-reap (~30 minutes)
  and empty-registry self-shutdown are safety-net backstops, not your
  cleanup path — a session you forget to close keeps a live RDP connection
  and your plaintext credentials resident in the daemon for up to half an
  hour. Disconnecting promptly is the correct behavior, not merely
  defensive.

## Config reference

Location: `~/.config/rdpilot/config.toml` on Linux (`%APPDATA%\rdpilot\config.toml`
on Windows). Every field is optional at the file layer.

| Field | Type | Notes |
|-------|------|-------|
| `host` | string | RDP target hostname/IP; required (via file/env/flag) before `connect` |
| `port` | u16 | TCP port; defaults downstream if unset |
| `username` | string | required before `connect` |
| `password` | string | required before `connect`; **plaintext on disk**, never committed |
| `domain` | string | optional Windows domain |
| `accept_invalid_certs` | bool | skip TLS cert validation; default `false`, only for a self-signed lab target you control |
| `share_root` | string | daemon-local file-transfer staging root for `put`/`get`; defaults to `<data_dir>/rdpilot/transfer-staging` |
| `sensor_binary_path` | string | daemon-local path to the deployed Windows sensor executable; **no safe default** — without it the daemon connects session-management-only and `perceive`/`input`/`put`/`get` will not work against a real target |

**Precedence (lowest to highest, later layer wins):**
`config.toml` file -> `RDPILOT_*` env vars (e.g. `RDPILOT_HOST`,
`RDPILOT_PASSWORD`, `RDPILOT_SENSOR_BINARY_PATH`) -> CLI flags
(`--host`, `--port`, `--username`, `--password`, `--domain`,
`--accept-invalid-certs`) passed to `rdpilot connect`. An absent/unset layer
never overrides a lower one that IS set.

## CLI verb reference

Global flag: `--json` (works on every verb) — machine-readable JSON instead
of a human table; errors still emit `{"error":{"code":...,"message":...}}`
to stdout under `--json`.

**Lifecycle:**
| Verb | Required args |
|------|----------------|
| `rdpilot connect [--name <str>] [--host] [--port] [--username] [--password] [--domain] [--accept-invalid-certs]` | none (`--name` auto-generated if omitted) |
| `rdpilot list` | none |
| `rdpilot disconnect --session <id>` | `--session` |

**Perceive** (read-only):
| Verb | Required args |
|------|----------------|
| `rdpilot perceive screenshot --session <id> --output <path>` | `--output` |
| `rdpilot perceive world-state --session <id> [--screenshot] [--window-list] [--uia-mode none\|foreground\|all\|hwnd] [--hwnd <n>]...` | — |
| `rdpilot perceive uia --session <id> --hwnd <n> --scope children\|subtree [--max-depth <n>]` | `--hwnd`, `--scope` |
| `rdpilot perceive window list --session <id>` | — |
| `rdpilot perceive process list --session <id>` | — |

**Input** (mutating — perceive first):
| Verb | Required args |
|------|----------------|
| `rdpilot input click --session <id> --x <n> --y <n> [--button left\|right\|middle] [--double]` | `--x`, `--y` |
| `rdpilot input scroll --session <id> --x <n> --y <n> --dy <n>` | `--x`, `--y`, `--dy` |
| `rdpilot input drag --session <id> --from-x <n> --from-y <n> --to-x <n> --to-y <n>` | all four coords |
| `rdpilot input type --session <id> --text <str>` | `--text` |
| `rdpilot input key --session <id> --combo <str>` | `--combo` (comma-separated, e.g. `ctrl,a`) |
| `rdpilot input launch --session <id> --exe <str> [--args <str>] [--cwd <str>]` | `--exe` |
| `rdpilot input foreground --session <id> --hwnd <n>` | `--hwnd` |

**File transfer:**
| Verb | Required args |
|------|----------------|
| `rdpilot put --session <id> --local <path> --remote-name <str> [--force]` | `--local`, `--remote-name` |
| `rdpilot get --session <id> --remote-name <str> --local <path> [--force]` | `--remote-name`, `--local`; refuses client-side before sending if `--local` exists and `--force` absent |

**Exit-code taxonomy:**
| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Internal / MissingConfig / Transport |
| 2 | SessionNotFound |
| 3 | DaemonUnreachable |
| 4 | TransferFailed |
| 5 | PathTraversal |
| 6 | ChecksumMismatch |
| 7 | DuplicateSession |
| 8 | NoClobber |
