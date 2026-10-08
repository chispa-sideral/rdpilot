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

Publisher tests use synthetic assets and stateful GitHub command failures;
they never create a production release.
