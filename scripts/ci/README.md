# Reproduce CI checks

Linux checks use the committed Cargo lockfile:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
python3 -m unittest discover -s scripts/ci -p 'test_*.py'
```

When testing locally with an existing daemon, give the Cargo test process a
new private `XDG_RUNTIME_DIR`. The Unix authorization test binds the resolved
daemon socket; using the active daemon's runtime directory causes `AddrInUse`.
The offline proof harnesses already create their own private runtime directories.

Windows checks retain config/CLI tests and the bridge containment tests. Use an
MSVC developer environment for the Windows lint and DACL checks:

```powershell
cargo clippy -p rdpilot-daemon -p rdpilot-ipc -p rdpilot-bridge --all-targets --locked -- -D warnings
cargo test -p rdpilot-config --locked
cargo test -p rdpilot-cli --bins --locked
./scripts/ci/run-windows-dacl.ps1
cargo test -p rdpilot-bridge --lib --tests --locked
python -m unittest discover -s scripts/ci -p 'test_*.py'
```

The DACL script needs permission to create and remove a temporary local user.

Offline proofs exercise the real daemon, CLI, MCP and browsers against fake
sessions. They require Playwright Chromium and Firefox, plus `ffmpeg` and
`ffprobe`. Install browser OS dependencies as appropriate for your platform:

```sh
python3 -m pip install playwright==1.63.0
python3 -m playwright install --with-deps chromium firefox
cargo build --locked -p rdpilot-cli -p rdpilot-daemon -p rdpilot-mcp
python3 scripts/ci/run-offline-proof.py viewer --bin-dir target/debug --output proof-viewer
python3 scripts/ci/run-offline-proof.py recording --bin-dir target/debug --output proof-recording
python3 scripts/ci/run-offline-proof.py takeover --bin-dir target/debug --output proof-takeover
```

Output directories must be new. Only `artifact-summary.json` is suitable for CI
upload: the wrapper selects trusted check labels and static failure stages,
excluding raw exceptions, tool results, credentials and viewer/lease tokens.
Raw evidence stays local. A failed harness or missing successful evidence scan
still fails the wrapper. These proofs do not establish live Windows RDP or Cua
Driver behavior.

`ci.yml` also exposes `workflow_call`. Release checks use that same definition
and check out the immutable caller SHA; publication waits for every required
job. Publishing credentials are available only to the final release job.

Release retries resume a matching draft and verify both downloaded assets
before publication. Rerun the same tagged workflow after an interrupted upload
or verification. A published release is refused without changing its assets.
Draft identity is checked before each replacement and publication; per-tag
Actions concurrency serializes workflow attempts, but these reads are not an
atomic lock against another publisher. Default-branch manual recovery for old
tags is not supported by this workflow.
The publisher finds a draft in the authenticated release listing, because
the by-tag endpoint returns only published releases.

Publisher tests use synthetic assets and stateful GitHub command failures;
they never create a production release.

## Live Windows desktop proofs

The `desktop` job builds the client, daemon, MCP server and bridge in release
mode and runs the live Cua, viewer and takeover harnesses against the runner's
own RDP listener. `run-hosted-desktop.ps1 -Proof cua|viewer|takeover` creates
disposable local standard users (Users and Remote Desktop Users, not
administrators) with random masked passwords, enables RDP with NLA on
`127.0.0.1:3389`, opens a loopback-only firewall rule and gives the harness a
bundle directory that holds only the bridge built from the checked-out source.
It stops the harness before the step timeout, logs the users off, removes their
profiles and accounts and restores every setting it changed. The script
changes machine settings: use it only on a disposable Windows machine.

The Cua proof checks that the running guest bridge is the source build and
records the resolved upstream Cua version and hashes. Cua resolves from the
product's default channel, so an upstream Cua change can fail this job.
The evidence artifact has, per proof, `environment.json` (commit, tree, image,
authentication and graphics settings), `result.json` (stage, harness exit code,
summary status), `cleanup.json` (one readback per restored item) and the
harness `proof/summary.json`, screenshots and logs.

To run the same harnesses against another prepared Windows host, use
`run-cua-e2e.py` with two credential files, or `run-viewer-proof.py` and
`run-takeover-proof.py` with `RDPILOT_VIEW_{A,B}_*` or `RDPILOT_TAKE_*`
variables, plus `--bundle` with a directory that holds a source-built
`rdpilot-bridge.exe` and `--allow-no-tailnet` when no tailnet is available.

Not executed live: the recording proof (three concurrent users and POSIX
client checks; offline only), tailnet and cross-machine viewing, UAC and the
secure desktop, Windows Job containment over RDP, download of a published
bridge release, and domain or production authentication.
