# rdpilot

rdpilot connects to Windows over RDP and exposes **Cua Driver's native MCP** on a
connection bound to one named target. Cua owns desktop discovery, accessibility,
screenshots, app launch and actions. rdpilot owns RDP, offline deployment, target
identity, process lifetime, recovery and file transfer.

```text
MCP caller ⇄ rdpilot-mcp --session NAME ⇄ local daemon ⇄ RDP dynamic channel
                                                     ⇄ bridge ⇄ Cua MCP
```

No guest network port, Python, Node installation or internet access is required.
The guest runs the pinned upstream Windows x64 bundle and our small Rust bridge.
The agent and its reasoning remain on the caller's machine.

## Build and use

Build local binaries with `cargo build --workspace`. Build the guest bridge for
`x86_64-pc-windows-msvc` on Windows or with cargo-xwin. Package the original Cua
0.28.2 archive (downloaded only on the build machine):

```sh
python3 scripts/package-cua.py \
  --bridge path/to/rdpilot-bridge.exe \
  --bin-dir target/debug --output dist
export RDPILOT_BUNDLE_PATH="$PWD/dist/bundle"
rdpilot session connect --help
# Connect using configured credentials, then bind one MCP process to the name:
rdpilot-mcp --session NAME
```

`scripts/install-local.sh` installs the three local binaries together, preserving
the daemon's sibling lookup. Set `RDPILOT_BUNDLE_SRC` to stage an existing bundle.
Use `--archive ZIP` to package an already downloaded archive without build-time
network access. `--bundle-only` builds a guest payload for development probes.

The MCP endpoint forwards upstream initialize, tools, results, images, requests
and notifications unchanged. It requires an existing named RDP connection. One
active Cua attachment per target is permitted; a second receives a busy error.
A Cua session label is application data, not RDP target selection.

Use `rdpilot --help` for connection management, native framebuffer/input recovery,
ping and file put/get. Cua paths refer to **guest files**. File transfer uses an
explicit local path and a relative guest name under `%TEMP%\rdpilot-transfer-root`;
both sides verify SHA256 and reject traversal and destination overwrites.

## Runtime and packaging

The package contains the original signed upstream ZIP, bridge, manifest, bootstrap
script and notices. Deployment verifies the ZIP, bridge and extracted file hashes,
publishes a versioned user-local directory, and launches adjacent
`cua-driver.exe mcp --direct` with telemetry disabled. Upstream signatures stay
unchanged. Hash verification is separate from Windows Authenticode trust.

Development bridges are **unsigned**. Release signing is a separate publisher
step: sign the built bridge with your Authenticode signing service before passing
it to the packager, then verify with `Get-AuthenticodeSignature` on Windows. This
repository does not provide signing credentials or claim a fully signed release.
The packager hashes final signed bytes. See `packaging/notices` for licenses and
versioned upstream source/build references.

Frames are bounded at 16 MiB, with bounded queues and pipe/write/request deadlines.
A crash, stalled tool, transport loss or slow caller closes its attachment and
terminates the contained child tree. Completion may be unknown; requests are
never automatically replayed. Explicit reattachment starts fresh MCP state.
Existing handles cannot route to a new connection that reuses the same name.
Native RDP framebuffer/input remain independent of Cua. UAC/secure-desktop
computer use is unsupported; signed binaries do not imply elevated automation.

## Live viewer

`rdpilot view` shows the daemon's live sessions in a browser. The viewer is
read-only: it sends no input, Cua call, file transfer, connect or disconnect,
and it does not change session activity, idle reaping or daemon self-shutdown.

```sh
rdpilot connect --name work ...   # the viewer needs a running daemon
rdpilot view                      # prints one URL per bound address
```

The command prints a URL such as `http://127.0.0.1:PORT/?token=TOKEN` for each
bound address and runs until Ctrl-C. Open a URL, then select **View** for one or
more sessions (at most 4 per tab). The page shows each session's current RDP
framebuffer at up to 4 frames per second, its size changes, and a banner when
the server ends the session or the session is closed. Frames do not show the
mouse cursor. Frames are not written to disk.

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
- The page removes the token from the address bar. A reload or a new tab needs
  the printed URL again. Browsers allow about 6 connections per origin across
  all tabs; use one or two tabs.

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

Upgrade note: this release changes the IPC compatibility version to 3. After
you upgrade, restart `rdpilot-daemon` (this ends its live sessions). Until then,
the CLI and MCP adapter report the daemon compatibility mismatch message.

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
small CrabBox Azure Windows lease with two Windows users and release builds.

Run `python3 scripts/e2e/run-cua-e2e.py --help` for the focused live suite.
It uses the shipped CLI, daemon and MCP adapter with two independent Windows
users, checks native desktop operations and transfer, then exercises Cua crash,
stall and RDP reconnect isolation. It provisions nothing and keeps credentials
out of evidence. `--windows-job-test EXE` also runs the bridge's Windows test
executable through the same RDP-only path.

`cargo run -p rdpilot --example cua_probe -- BUNDLE REQUESTS.jsonl OUTPUT.jsonl`
connects, deploys via RDPDR, initializes native MCP, runs supplied JSON-RPC requests,
and captures a native recovery screenshot. It reads `RDPILOT_HOST`,
`RDPILOT_PORT`, `RDPILOT_USERNAME`, `RDPILOT_PASSWORD` and optional
`RDPILOT_ACCEPT_INVALID_CERTS=1`. Live verification also needs two independently
bound Windows sessions, transfer roundtrip, Cua crash/stall and reconnect checks.
The probe never provisions machines. See the ticket's evidence for actual results.
