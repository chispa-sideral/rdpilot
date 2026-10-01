# rdpilot

rdpilot connects to Windows over RDP and exposes **Cua Driver's native MCP** on a
connection bound to one named target. Cua owns desktop discovery, accessibility,
screenshots, app launch and actions. rdpilot owns RDP, offline deployment, target
identity, process lifetime, recovery and file transfer.

```text
MCP caller ⇄ rdpilot-mcp --session NAME ⇄ local daemon ⇄ RDP dynamic channel
                                                     ⇄ bridge ⇄ Cua MCP
```

No guest network port, Python, Node installation, administrator right or
internet access is required. The rdpilot host downloads the upstream Windows x64
Cua driver and our small Rust bridge, verifies them and copies them to the guest
over RDP. The agent and its reasoning remain on the caller's machine.

## Build and use

Build local binaries with `cargo build --workspace` (`scripts/install-local.sh`
installs the three binaries together, preserving the daemon's sibling lookup).

```sh
rdpilot session connect --help
# Connect to a host from your hosts file (or an rdp:// URL), then bind one MCP
# process to the name:
rdpilot connect my-host --name NAME
rdpilot-mcp --session NAME
```

A connect with `CuaEnabled yes` (the default) gets the Cua bundle automatically
(see [Cua bundle](#cua-bundle)). There is nothing to package or configure.

The MCP endpoint forwards upstream initialize, tools, results, images, requests
and notifications unchanged. It requires an existing named RDP connection. One
active Cua attachment per target is permitted; a second receives a busy error.
A Cua session label is application data, not RDP target selection.

Use `rdpilot --help` for connection management, native framebuffer/input recovery,
ping and file put/get. Cua paths refer to **guest files**. File transfer uses an
explicit local path and a relative guest name under `%TEMP%\rdpilot-transfer-root`;
both sides verify SHA256 and reject traversal and destination overwrites.

## Hosts and targets

`rdpilot connect TARGET` takes a **host alias** from your hosts file or an
`rdp://` URL. Connection settings (address, port, user, credentials) live in the
hosts file, which uses ssh_config syntax. The old `--host`, `--port`,
`--username`, `--password`, `--domain` and `--accept-invalid-certs` flags, the
matching `config.toml` keys and the `RDPILOT_HOST`-style variables are gone;
leftovers are ignored. `config.toml` keeps only daemon-local settings
(`share_root`, `bundle_path`, `[viewer]`).

The hosts file is `~/.config/rdpilot/hosts` on Linux (`$XDG_CONFIG_HOME/rdpilot/hosts`),
`~/Library/Application Support/rdpilot/hosts` on macOS and `%APPDATA%\rdpilot\hosts`
on Windows. rdpilot never creates it. A missing file is fine; `-F FILE` reads a
different file instead (which must exist).

```
# ~/.config/rdpilot/hosts
Host lab
  HostName 10.0.0.5
  User alice
  Domain CORP
  Port 3390
  AcceptInvalidCerts yes
  PasswordCommand "pass show rdp/lab"

Host *
  CuaEnabled yes
```

**Matching and order.** `Host` takes patterns (`*`, `?`, `!negation`); a line of
only negations never matches. As in ssh, the target host is lowercased and
patterns are matched as written, so write patterns in lower case: `Host DevBox`
never matches. The **first value obtained for each setting wins**: rdpilot reads
the URL, then `-o` options in order, then the hosts file top to bottom, then the
built-in default (`Host *` with `CuaEnabled yes`, `CuaVersion latest-dev`,
`CuaAutoDownload yes`). Lines before the first `Host`
apply to every host. `Include PATH` (globs allowed, sorted, `~` and paths
relative to the hosts file's directory) reads more files; a pattern that matches
nothing is skipped, cycles and nesting deeper than 16 are errors, and an
`Include` inside a non-matching `Host` block is not read. `Match` is not
supported. Keywords are case-insensitive; `Keyword value` and `Keyword=value` both
work; quote values that contain spaces. Booleans accept only `yes` and `no`.

| Keyword | Meaning |
| --- | --- |
| `HostName` | Address to connect to (`%h` = the target name, `%%` = `%`). Default: the target. |
| `Port` | TCP port. Default 3389. |
| `User`, `Domain` | Account. |
| `Password` | Literal password. Prefer `PasswordCommand`. |
| `PasswordCommand` | Shell command whose stdout is the password (see below). |
| `AcceptInvalidCerts` | Accept any server certificate (`yes`/`no`, default `no`). |
| `CuaEnabled` | `no` = native RDP only: no bridge is deployed and `put`/`get` and Cua are unavailable. Default `yes`. |
| `CuaVersion` | Cua driver release: `latest-dev` (newest, nightlies included), `latest` (newest non-nightly) or an upstream tag. Default `latest-dev`. |
| `CuaAutoDownload` | `no` = use only cached or `bundle_path` files, never download. Default `yes`. |
| `Include` | Read more files. |

`Password` and `PasswordCommand` share one slot, so whichever is obtained first
wins. On Unix a hosts file that holds a literal `Password` and is readable by
group or others is refused (`chmod 600` it).

**URLs.** `rdp://[DOMAIN\]USER[:PASSWORD]@HOST[:PORT]`; `rdps://` is identical.
Percent-encode special characters (`CORP%5Calice`). IPv6 hosts go in brackets.
The URL has **no path, query or fragment**: every one is an error, including a
lone trailing `/`. URL parts beat everything else. A password in a URL is visible
in the process list and shell history; prefer `PasswordCommand`.

**`-o KEYWORD=VALUE`** overrides one setting (`-o Port=3390 -o CuaEnabled=no`);
`Host`, `Match` and `Include` cannot be used with `-o`.

**`rdpilot config resolve TARGET [-o ...] [-F FILE] [--json]`** shows every
resolved setting and where it came from (`url`, `-o #N`, `file:line`, built-in
default). Passwords print as `<redacted>`; a `PasswordCommand` is shown as
written and is never run.

**PasswordCommand.** It runs through `sh -c` (`cmd /C` on Windows) only when a
connection needs the password, with a 30 s timeout, stdin closed and stderr
passed through. One trailing newline of stdout is removed. A failure, empty
output or timeout is an error that never shows the output. A timed-out command
is killed, but processes it started may keep running. `%h` (address), `%n` (the
target name; for a URL just the hostname, never the URL), `%r` (user), `%p`
(port, default 3389) and `%%` are substituted, then `${VAR}` reads your
environment. Because the command goes to a shell, a value substituted for `%h`,
`%n`, `%r` or `%p` must be non-empty, must not start with `-` and must not
contain whitespace, control characters or any of
`` ' " ` \ $ ; & | < > ( ) { } [ ] * ? ! # ~ % ^ = ``. This applies whatever the
value's source. If a value legitimately needs one of these, write it literally in
the command instead of using the token. `${VAR}` values are not checked and end
up in the command text, so do not put a secret in `${VAR}`; have the command read
it (`PasswordCommand "printenv MY_SECRET"`). `%%` yields one literal `%`:
`PasswordCommand "printf %%s hunter"` runs `printf %s hunter`, and on Windows `PasswordCommand "cmd /c echo 100%%"`
runs `cmd /c echo 100%`.

## Cua bundle

A Cua-enabled connect needs two guest files. The daemon gets both before the RDP
logon:

- **rdpilot-bridge.exe** comes from this project's GitHub release whose tag is
  `v<rdpilot version>`, checked against that release's `SHA256SUMS`. A
  development build (a version with no published release) uses
  `rdpilot-bridge.exe` from `bundle_path` when there is one, else the newest
  published release; the connect then warns that the bridge version differs.
- **The Cua driver** is the upstream `cua-driver-rs-<version>-windows-x86_64-binary.zip`
  from the trycua/cua GitHub releases, checked against that release's
  `checksums.txt`. `CuaVersion` selects it: `latest-dev` (default: the newest
  release, nightlies included), `latest` (the newest non-nightly release) or an
  upstream tag such as `cua-driver-rs-v<version>` or
  `nightly-cua-driver-rs-v<version>`. Releases are ordered by version number,
  not by GitHub's latest or pre-release flags.

Only the rdpilot host needs network access. Set `-o CuaVersion=<tag>` to pin a
driver release, for example when a new upstream release breaks.

**Cache.** Verified files stay in `$XDG_CACHE_HOME/rdpilot` (else
`~/.cache/rdpilot`) on Linux, `~/Library/Caches/rdpilot` on macOS and
`%LOCALAPPDATA%\rdpilot\cache` on Windows, one directory per component, version
and architecture. Every file is hashed again before use. The answer for
`latest-dev` and `latest` is reused for one hour. You can delete the cache at any
time.

**Offline.** When GitHub cannot be reached, a verified cached copy of the
requested version is used. For `latest-dev` and `latest` the newest cached
version of that channel is used, with a warning. `CuaAutoDownload no` never
downloads: it uses only the cache and `bundle_path`. The daemon honours
`HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`, read when it starts.

**bundle_path.** For offline and development use, set `bundle_path` in the
daemon's `config.toml` to a local directory that holds `rdpilot-bridge.exe`, a Cua archive
(`cua-driver-rs-<version>-windows-x86_64-binary.zip`), or both. A file found there
is used instead of downloading and is trusted without a checksum; a missing one
is downloaded. An archive there fixes the Cua version and overrides
`CuaVersion` (with a warning when they differ). `bundle_path` cannot be set per
connect. To test a bridge built from source, build it for
`x86_64-pc-windows-msvc` (on Windows, or with cargo-xwin) and put the exe in that
directory.

**Fail closed.** If Cua is enabled and a component cannot be obtained or
verified, `rdpilot connect` fails before the RDP logon. The error names the
component, version, architecture and cause (offline, checksum failure, no
matching release, unsupported archive layout, missing path) and the fixes:
`-o CuaEnabled=no` for a native-only session, `-o CuaVersion=<tag>`, a
`bundle_path`, or network access.

**Trust.** Each file is checked against a checksum file from the same GitHub
release, so the check detects damaged or mixed-up downloads, not a compromised
release. rdpilot compiles in no hash table and no Cua version. The Cua driver is
downloaded from upstream under its own licence; rdpilot does not redistribute
it. Development bridges and the released bridge are not Authenticode-signed.

**Guest footprint.** The daemon serves the bridge, the archive and a manifest on
the RDP drive `\\tsclient\RDPILOT\bundle`. Windows asks for confirmation before
it starts a program from that drive, so the daemon types a `cmd /d /c` line
into the Run dialog that copies the served bridge to
`%LOCALAPPDATA%\rdpilot\l<generation in base 36>.exe`, starts that copy with
`install --generation <generation>`, and deletes the copy when it exits. The
bridge checks its own image and every served file against the manifest,
extracts the archive, installs into
`%LOCALAPPDATA%\rdpilot\<bundle id>`, starts `cua-driver.exe mcp --direct`
there with telemetry disabled, and contains it in a kill-on-close job. It writes
nothing else: no registry, service, scheduled task, PATH or firewall change
(Windows itself records the Run dialog history). File transfer uses
`%TEMP%\rdpilot-transfer-root`. To remove everything, run
`"%LOCALAPPDATA%\rdpilot\<bundle id>\rdpilot-bridge.exe" cleanup` in the guest
session; it stops this user's rdpilot processes in that session and removes
`%LOCALAPPDATA%\rdpilot` and `%TEMP%\rdpilot-transfer-root`. The manual
equivalent, after the rdpilot sessions are closed, is
`rmdir /s /q "%LOCALAPPDATA%\rdpilot"` and
`rmdir /s /q "%TEMP%\rdpilot-transfer-root"`.

**Releases.** Set `[workspace.package] version` in `Cargo.toml`, merge, then
push the tag `v<version>` on that develop commit. The release workflow builds
`rdpilot-bridge.exe`, checks that the tag matches the version, and publishes the
exe with `SHA256SUMS`.

## Runtime

Frames are bounded at 16 MiB, with bounded queues and pipe/write/request deadlines.
A crash, stalled tool, transport loss or slow caller closes its attachment and
terminates the contained child tree. Completion may be unknown; requests are
never automatically replayed. Explicit reattachment starts fresh MCP state.
Existing handles cannot route to a new connection that reuses the same name.
Native RDP framebuffer/input remain independent of Cua. UAC/secure-desktop
computer use is unsupported; signed binaries do not imply elevated automation.

## Live viewer

`rdpilot view` shows the daemon's live sessions in a browser. The viewer sends
no input, Cua call, file transfer, connect or disconnect, and it does not change
session activity, idle reaping or daemon self-shutdown. Its only actions are the
recording controls: start and stop recording, annotate, and keep recordings
(see [Session recording](#session-recording)).

```sh
rdpilot connect my-host --name work   # the viewer needs a running daemon
rdpilot view                      # prints one URL per bound address
```

The command prints a URL such as `http://127.0.0.1:PORT/?token=TOKEN` for each
bound address and runs until Ctrl-C. Open a URL, then select **View** for one or
more sessions (at most 4 per tab). The page shows each session's current RDP
framebuffer at up to 4 frames per second, its size changes, and a banner when
the server ends the session or the session is closed. Frames do not show the
mouse cursor. The viewer writes nothing to disk; only a recording, when it is
on for a session, writes that session's frames.

Activity strip. Under each viewed session, a strip lists what was done in that
session, newest first. Each Cua tool call and each native verb (`screenshot`,
`mouse`, `key`, `desktop_size`, `put`, `get`) gets one row with:

- the local time it started,
- the tool or verb name,
- the source: `cua` (a tool call through `rdpilot-mcp`) or `cli` (a native
  verb),
- the outcome: `ok`, `error`, `no_reply` (the attachment closed or the daemon
  gave up before an answer) or `running` (not finished yet),
- the duration, which includes any time spent waiting for the session.

Marker rows show when a Cua attachment starts and ends (with the reason), when
the server ends the session, and when older events were dropped. The strip never
shows argument values, typed text, file paths, results, images or error text.

The daemon keeps the newest 200 events of each session in memory, from connect
until the session is closed. It records them whether or not a viewer runs, never
writes them to disk, and drops them when the session closes. A panel opened
later shows the retained events. The page asks for new events about twice a
second; many open tabs can slow updates.

Bind set. Nothing listens until you run `rdpilot view`, and the listener stops
when the command exits. The viewer binds `127.0.0.1` and, by default, this
host's Tailscale IPv4 address. It never binds a wildcard, LAN or public address.
Set the bind set in the `[viewer]` table of `config.toml`, with
`RDPILOT_VIEWER__BIND`, or with `--bind`:

- `loopback+tailnet` (default): `127.0.0.1` plus the Tailscale address.
- `loopback`: `127.0.0.1` only.

The viewer uses an address in `100.64.0.0/10` only on a Tailscale interface,
because that range is also carrier-grade NAT space. If it finds no such address,
or several, it serves on loopback only and prints a notice. Set
`tailnet_address` (`RDPILOT_VIEWER__TAILNET_ADDRESS`, `--tailnet-address`) to
choose one; it must be on a local interface.

Access boundary:

- Each start makes a new random token. Every request needs it: the page takes it
  from the printed URL, and the page's API calls send it as a bearer token.
- Requests with a foreign `Origin` or an unexpected `Host` are refused. Use the
  printed IP URL; a MagicDNS name is refused by the Host check.
- There is no TLS. On the tailnet, WireGuard encrypts the traffic. Anyone with a
  URL and network access to its address can view all sessions of this daemon.
- The token stays in the address bar, so a reload or a bookmark keeps working
  until the viewer restarts. Treat the URL as a secret. Browsers allow about 6
  connections per origin across all tabs; use one or two tabs.

Residual risk: the viewer runs inside the daemon, which holds the RDP passwords
of its sessions in memory. The HTTP server parses requests from any peer that
can reach a bound address (for example, any tailnet peer that the tailnet ACL
allows) before it checks the token, and heavy viewer traffic uses daemon
resources. The token gives access to screen contents, which can show secrets
typed or displayed in the guest. Use `--bind loopback` when you do not need
remote viewing.

Lifetime: `rdpilot view` never starts a daemon. When the daemon exits, the
viewer stops and the command exits. The daemon exits about 30 s after its last
session closes, so a viewer started with no sessions stops after that time.

Remote viewing: from another device on the tailnet, open the printed tailnet
URL. When the tailnet bind is off, forward the same port over SSH and open the
printed loopback URL on your machine (the Host check needs the same port):

```sh
ssh -L PORT:127.0.0.1:PORT user@daemon-host
```

Upgrade note: this release changes the IPC compatibility version to 6. After
you upgrade, restart `rdpilot-daemon` (this ends its live sessions). Until then,
the CLI and MCP adapter report the daemon compatibility mismatch message.

## Session recording

The daemon can record a session: its video and its session events (the
activity strip's events, recording lifecycle and annotations) go to an
owner-only directory. Recording is off unless you switch it on.

Switching. For each session the most specific setting wins:

1. `rdpilot connect TARGET --record` or `--no-record`.
2. The first `[[recording.hosts]]` entry in `config.toml` whose `host` is the
   target as you give it to `connect`: a hosts-file alias as typed, or the host
   of an `rdp://` URL. The match ignores ASCII case.
3. `[recording] enabled` (`RDPILOT_RECORDING__ENABLED`). Default: `false`.

```toml
[recording]
enabled = false
[[recording.hosts]]
host = "lab-vm"
enabled = true
```

On a connected session, `rdpilot record start --session NAME` starts a new
recording from that moment and `rdpilot record stop --session NAME` stops it;
the session continues. A session can have several recordings. A start on a
recording session, or a stop on a session that is not recording, changes
nothing and says so. The viewer's live panel has the same Start/Stop control.
These actions do not change `config.toml`. `rdpilot list` and the viewer show
the active recording of each session.

The CLI resolves the switch at `connect`. The daemon reads `dir`, `max_fps` and
`budget_mib` when it starts and when a recording starts. The manifest and
`rdpilot recording list` show the address the daemon connected to.

Annotations. `rdpilot annotate --session NAME TEXT`, or the annotation input
in the viewer's live panel, adds a note with its time and source to the active
recording. Text is at most 4 KiB. When the session is not recording, the
command and the viewer refuse and write nothing.

Location. Recordings go to `<local data dir>/rdpilot/recordings/`
(`.local/share/rdpilot/recordings` in the home directory on Linux,
`%LOCALAPPDATA%\rdpilot\recordings` on Windows, so that roaming profiles do
not copy them). Set `[recording] dir` (`RDPILOT_RECORDING__DIR`) to change it.
Directories are `0700` and files `0600` on Unix; on Windows each recording
directory has a protected DACL that allows only your user.

Format 1. Each recording is one directory `<id>/`, where `<id>` is
`YYYYMMDDTHHMMSSZ-` plus 8 hex digits (UTC start):

- `manifest.json`: format version, session id and name, host, UTC start and
  end, end reason, rdpilot version, how the recording started, settings,
  codec (`av1`) and container (`webm`), the event log's id, the closed
  segments (file, start and end on the recording timeline, frames, bytes,
  size) and, after stop, the recorder's figures (`stats`). Written atomically.
- `events.jsonl`: one JSON event per line, append-only. Each event has
  `seq` (from 1), `at` (UTC, milliseconds), `offset_ms` (milliseconds on the
  recording timeline, which is the video's clock), `frame_seq`, `source`
  (`cua`, `cli`, `viewer` or `daemon`) and `kind` with its fields. Kinds: the
  activity strip's `call_started`, `call_finished`, `cua_attached`,
  `cua_detached`, `session_ended`, and `recording_started`,
  `recording_stopped`, `session_closed`, `desktop_resized`, `segment_started`,
  `segment_closed`, `frames_dropped`, `events_lost`, `video_stopped`,
  `annotation`. Readers ignore unknown fields and kinds, and a partial last
  line.
- `keep`: an empty file when the recording is kept.
- `segments/NNNNNN.webm`: AV1 video in WebM, each segment self-contained and
  closed. A segment starts at the first display change after the previous one
  closed and closes 60 s after its first frame, at a desktop resize, at stop
  and at the size cap. The segment being written is `NNNNNN.webm.part`; it is
  not playable until it closes, so the last minute of an active recording is
  not yet in the replay.

Play a segment outside rdpilot with a browser, `mpv` or `ffplay`. There is no
single-file export. A segment's timestamps start at its `start_offset_ms` on
the recording timeline.

Video. The recorder encodes a frame only when the display changed, at most
`max_fps` (`RDPILOT_RECORDING__MAX_FPS`, default 4, 0.5 to 8) frames per
second. A still display adds no video. Each frame keeps the time it was
captured. The encoder (rav1e, AV1, constant quantizer 130, speed 10) runs in
the daemon on a thread of its own with two encoder threads; on Linux these
threads run at nice 10, so RDP, IPC and viewer work comes first. No external
program runs.

Cost. On a 1920x1080 desktop (8-core host, measured on an idle and a loaded
host), one changed frame costs about 0.3 s of encoder time (0.5 CPU s), and
text-only changes cost about 0.16 s. The encoder returns a frame's data only
after four more changes or when the segment closes, so a single call can take
up to about 0.9 s. When the screen changes faster than the encoder keeps up,
for example on a busy or small host, the recorder encodes fewer than `max_fps`
frames per second: the newest captured display state replaces an older one
that was not encoded yet, and the log records the skipped states as
`frames_dropped`. Producers never wait for the recorder. The manifest's
`stats` shows the encoder time per frame, the time frames waited for the
encoder, the frames held until a segment closed and dropped frames.

Retention. At daemon start and when a recording starts, the daemon deletes
recordings that are not kept and not active: first those older than 7 days,
then the oldest while the total of unkept recordings (active ones included)
is more than `budget_mib` (`RDPILOT_RECORDING__BUDGET_MIB`, default 2048 MiB).
Kept recordings are never deleted and do not count toward the budget. When
kept recordings alone use more than the budget, the daemon logs a warning, and
`rdpilot recording list` and the viewer show it; nothing is deleted. One
recording that reaches the budget by itself stops its video (`video_stopped`)
and continues to record events.

Keep. `rdpilot recording keep ID` and `rdpilot recording unkeep ID`, or the
Keep toggle in the viewer's recordings list and replay view, set or clear the
mark. `rdpilot recording list` shows each recording's id, session, host, UTC
start, duration, size, whether it is active and kept, and the kept and unkept
totals. An unmarked recording is subject to the next pruning pass. To delete a
recording by hand, remove its directory when it is not active.

Replay. In `rdpilot view`, select **Recordings**, then **Open**. The replay
view has the live panel's layout: the video, the activity strip up to the
playback position, and a logs pane with every recorded event and its UTC time.
Play, pause, seek on the timeline, choose the speed, and read the UTC time of
the shown frame. Select an event in the strip or the logs to seek to it.
Between segments the last frame stays on screen. Replay needs Chrome or
Firefox; Safari plays AV1 only on some hardware.

What is recorded: the screen as the RDP session shows it, without the mouse
cursor, and the events above. What is not recorded: audio, tool arguments
(names and values), typed text, key sequences, coordinates, file paths, tool
results, images, error text, daemon log lines and credentials. Screen contents
are recorded as they are and can show secrets typed or displayed in the guest.
Anyone with a viewer URL can start and stop recordings, annotate and keep
recordings, and replay them.

## Migration and tests

The custom C# sensor and desktop SDK/CLI/MCP APIs are removed. Replace window,
process, UIA, world-state, launch and custom computer-tool calls with Cua's native
MCP tools. There is no legacy schema compatibility layer or process-tree parity
promise. Native RDP screenshot/input and file transfer remain rdpilot APIs.

Run `cargo check --workspace --all-targets` and `cargo test --workspace`.
The remaining tests cover transport framing, child lifetime, stale generations,
isolation, IPC authorization, lifecycle and transfer. Tests of our former UIA,
window/process trees and coordinate tool adapters are replaced by a focused live
Cua integration harness; upstream tool schema behavior is Cua's responsibility.

Run `python3 scripts/e2e/run-viewer-proof.py --help` for the live viewer proof.
Its `--fake` mode runs offline against the fake connector; its live mode needs a
Windows host with two independent Windows users and release builds.
`python3 scripts/e2e/run-recording-proof.py --help` describes the session
recording proof: `--fake` runs offline (it needs `ffmpeg`, `ffprobe` and
Playwright Chromium and Firefox); live mode needs three Windows users.

Run `python3 scripts/e2e/run-cua-e2e.py --help` for the focused live suite.
It uses the shipped CLI, daemon and MCP adapter with two independent Windows
users, checks native desktop operations and transfer, then exercises Cua crash,
stall and RDP reconnect isolation. It provisions nothing and keeps credentials
out of evidence. `--windows-job-test EXE` also runs the bridge's Windows test
executable through the same RDP-only path.

`cargo run -p rdpilot --example cua_probe -- BUNDLE_DIR REQUESTS.jsonl OUTPUT.jsonl`
takes a prepared bundle directory (for example one under the cache's `bundles`
directory), connects, deploys via RDPDR, initializes native MCP, runs supplied JSON-RPC requests,
and captures a native recovery screenshot. It reads `PROBE_HOST`,
`PROBE_PORT`, `PROBE_USERNAME`, `PROBE_PASSWORD` and optional
`PROBE_ACCEPT_INVALID_CERTS=1` (example-only variables, not rdpilot configuration). Live verification also needs two independently
bound Windows sessions, transfer roundtrip, Cua crash/stall and reconnect checks.
The probe never provisions machines. See the ticket's evidence for actual results.
