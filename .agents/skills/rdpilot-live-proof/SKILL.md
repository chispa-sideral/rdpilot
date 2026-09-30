---
name: rdpilot-live-proof
description: Run an rdpilot live proof (scripts/e2e/run-viewer-proof.py or run-cua-e2e.py) against a CrabBox Azure Windows lease, or write a new live proof harness. Covers lease sizing, test users, release builds, timing checks, ending sessions, evidence and cleanup.
---

# rdpilot live proof

A live proof drives real rdpilot binaries (CLI, daemon, MCP adapter) against a
disposable CrabBox Azure Windows lease and leaves an evidence directory. CrabBox
is the only test platform. This skill holds the rdpilot-specific lessons; the
user-level `crabbox-azure-windows` skill owns generic lease handling: acquiring,
warmup, `RDP_READY`/`CUA_READY` scripts, the SSH RDP tunnel, release and the
Azure absence check. Read it first and follow its failure gate.

The harnesses provision nothing. Run `--help` on each harness for its current
arguments; the lessons below are what `--help` does not tell you.

## Steps

1. **Lease.** Request `Standard_D2s_v3` (2 vCPU, 8 GB), the smallest size on
   which both harnesses have passed. Warmup takes about 6.5 minutes. Use a
   fixed `--lease-id`, no `--desktop`.
   Done when warmup exits 0, Verify prints `RDP_READY` and `CUA_READY`, and
   the host's RDP tunnel port gets an X.224 connection-confirm reply.

2. **Two throwaway Windows users.** Over the lease's SSH, create two local
   users (for example `rdpilotproofa`, `rdpilotproofb`) that can log on
   through RDP, with freshly generated passwords. Two users are required: a
   second RDP login as the same user takes over the first session, and the
   isolation checks require two distinct Windows session IDs.
   Keep the passwords in one mode-0600 file on tmpfs (`$XDG_RUNTIME_DIR`),
   never in arguments, logs, tickets or the repository. Pass them to the
   harness only through the environment (`run-viewer-proof.py`:
   `RDPILOT_VIEW_{A,B}_{HOST,PORT,USERNAME,PASSWORD}`) or 0600 credential JSON
   files on the same tmpfs (`run-cua-e2e.py`). Host and port are the
   loopback end of the SSH RDP tunnel; the harness puts its own TCP relay in
   front of it.
   Done when both users log on and the password file exists only on tmpfs.

3. **Build.** `cargo build --release --workspace`. The daemon downloads the Cua
   driver and the bridge itself into a fresh cache for each harness run. Only
   when the build's version has no published release yet, build
   `rdpilot-bridge.exe` for `x86_64-pc-windows-msvc` and pass a directory that
   holds it with `--bundle` (it becomes the daemon's `bundle_path`; Cua still
   downloads). Timing checks need release builds: a debug PNG encode of a
   1920x1080 frame takes about 1 s, release 8-14 ms. `run-viewer-proof.py`
   refuses a `debug` bin dir in live mode unless `--allow-debug`.
   Done when `target/release` holds `rdpilot`, `rdpilot-daemon` and
   `rdpilot-mcp`.

4. **Run the harness** on the tailnet host, with a new evidence directory
   outside the repository (the harness creates it owner-only; it holds guest
   screen content). Allow for the first connect: it downloads the Cua bundle
   and deploys it over RDPDR (default `--connect-timeout 600`).
   Done when the harness prints every check as passed, exits 0, and
   `summary.json` has `status: passed` and an `unverified` list you can
   explain.

5. **Clean up.** Close the tunnel, shred the password file, release the lease
   and check Azure as the crabbox skill describes. Match the leftover query on
   the lease slug, the VM name suffix and the lease ID, in the resource group
   and in the whole subscription; the result must be `[]`. The resource group
   also holds shared network resources and other leases' VMs: leave them.
   A `stop` warning about the GitHub Actions hydration marker is harmless.
   Done when `inspect` shows `released` with cleanup `complete` and the Azure
   query is empty.

6. **Secret scan.** The harness scan only knows the token and the passwords
   it was given. Scan the whole evidence directory yourself, including lease
   helper scripts and PNGs, for the generated passwords, `token=` or `Bearer`
   followed by 64 hex characters, and private key material. A 64-hex match may
   be a sha256 you recorded; confirm each hit.
   Done when every hit is explained and no secret value remains.

## Lessons

**Time a visible change, not a frame.** Measuring "first frame received after
the Cua call returned" gives a false pass: a long-poll frame from before the
change arrives within milliseconds. Keep the region pixels before the change
and time the first frame where enough of them differ (the viewer proof counts
channel deltas above 48 and needs 200 changed pixels in the text box; the
typed marker changes about 1400, a caret blink fewer than 100). Prove the check
with a negative control: an unreachable threshold must fail while frames keep
arriving. A canvas resize counts as fully changed, so keep the guest desktop
size fixed during timing.

**Fixture.** Type into a WinForms `TextBox` launched through Cua `launch_app`
with an encoded PowerShell script, then find it by an `AccessibleName` such as
`Proof input` and use `type_text` on its element token. The marker region in
the viewer proof is derived from the 900x300 form centred above a 40 px
taskbar; at 1920x1080 it is `(560, 436, 880, 466)`, or pass `--marker-region`.
Give `AccessibleName` only to controls you look up; a control whose text you
read back (a result label) must have no `AccessibleName`, because it masks the
dynamic `Text` in the accessibility tree.

**Ending a session.** A guest `logoff.exe` sent through Cua kills that
session's bridge before the call returns, so the MCP client sees `MCP EOF
during tools/call`. To show a server-ended session, stop that session's Cua
MCP client first (no call in flight), then drop its RDP connection at the
harness relay. The viewer proof waits up to 120 s for the "Disconnected"
banner; the list then shows the session as `Disconnected`. End the other
session with `rdpilot disconnect`.

**Fault injection** (Cua kill or stall). Target the exact Cua PID and Windows
session, suspend with `NtSuspendProcess` and check its status, and write a
success or error marker on every path. A missing marker is a harness failure,
not a product result; fix the injector before judging recovery.

**Tailnet evidence.** Run the viewer proof on the tailnet host so `rdpilot
view` binds the host's tailnet address; headless Chromium opens that tailnet
URL, and the rejection matrix runs on loopback and the tailnet address. Use
the printed IP URL; a MagicDNS name is refused. The cross-machine check is a
human opening the tailnet URL from another tailnet device.

**Daemon and pipes.** Every CLI verb except `view` auto-starts the daemon,
and on Unix the auto-started `rdpilot-daemon` inherits the starting command's
stdout and stderr. Piping or capturing a command that starts the daemon
therefore waits until the daemon exits:
`scripts/install-local.sh | tail` does not return, because the script ends
with `rdpilot list`. Redirect to a file instead (`> install.log 2>&1`). A
harness must start `rdpilot-daemon` itself with output to a log file before it
runs CLI commands with captured output, as both harnesses do.

**Isolated daemon.** The installed user daemon owns the default socket. Give
harnesses and `cargo test` a fresh `XDG_RUNTIME_DIR` (the harnesses also set
`XDG_CONFIG_HOME` and long idle and empty-grace timeouts), or tests that bind
the socket fail. After an IPC compatibility change, restart the installed
daemon.

**Local disk.** A full `cargo test --workspace` link can fill a small `/tmp`
and fail with linker bus errors; run it with `CARGO_INCREMENTAL=0
CARGO_BUILD_JOBS=2`.
