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

The required `desktop` job uses a Windows Server 2025 hosted runner for real
loopback RDP with NLA/CredSSP, temporary standard users and product-default
graphics. It builds the candidate client, daemon, MCP and bridge, delivers the
bridge through normal RDPDR, and verifies the running guest bridge/Cua against
the installed bundle manifest and source hash. Cua 0.34.0, Playwright 1.63.0 and
Pillow 12.3.0 are pinned. The existing full two-user Cua, two-user browser viewer
and single-user takeover proofs run serially. Fault isolation, recovery, file
transfer, browser latency/authentication, takeover/read-only behavior and actual
Ctrl-C shutdown assertions remain required.

On a fresh Windows host, from an administrator MSVC shell:

```powershell
cargo build --release --locked -p rdpilot-cli -p rdpilot-daemon -p rdpilot-mcp -p rdpilot-bridge --target x86_64-pc-windows-msvc
python -m pip install playwright==1.63.0 Pillow==12.3.0
python -m playwright install chromium
$proofOutput = Join-Path $env:TEMP ('desktop-proof-' + [guid]::NewGuid())
python scripts/ci/hosted_desktop.py --initialize --output $proofOutput
./scripts/ci/run-hosted-desktop.ps1 -BinDir target/x86_64-pc-windows-msvc/release -Output $proofOutput -ExpectedCommit (git rev-parse HEAD)
```

The gate refuses an existing rdpilot daemon, pipe or native cache. It provisions
only disposable local users; it must not be used while another rdpilot session
uses that host. Password files, child temporary files and raw diagnostics live
under an owner-only Windows ACL. Only `artifacts/` may be uploaded: fixed-schema
check summaries, validated provenance, cleanup readbacks and successfully
scanned screenshots. Build/setup/proof/scan/cleanup failures remain red with
static failure stages. Raw exceptions and transcripts are never uploaded.
Cleanup revalidates held process identities and user SIDs, logs off only owned
sessions, removes unloaded nonspecial owned profiles and guest files, removes
owned accounts/cache/private files and restores registry/service/firewall state.
Each cleanup action is bounded and attempted independently.

The manual `Desktop host diagnostics` workflow accepts only `setup` or `cua`.
Setup checks host preparation and cleanup without RDP. Cua builds the same four
source executables and runs the complete two-user Cua proof, including faults,
provenance, recovery, scanning and live profile cleanup, within a 50-minute job.
It produces `mode: cua_diagnostic` and `status: cua_diagnostic_passed` on success;
setup produces `setup_diagnostic`/`setup_passed`. Both labels remain diagnostic
at build-stage initialization. Neither qualifies the required all-three gate,
release verification or delivery. To run focused Cua locally after the build:

```powershell
$diagnosticOutput = Join-Path $env:TEMP ('cua-diagnostic-' + [guid]::NewGuid())
python scripts/ci/hosted_desktop.py --cua-diagnostic --initialize --output $diagnosticOutput
python scripts/ci/hosted_desktop.py --cua-diagnostic --bin-dir target/x86_64-pc-windows-msvc/release --output $diagnosticOutput --expected-commit (git rev-parse HEAD)
```

Failure artifacts retain the primary fixed operation, exception category and
numeric CLI exit before cleanup, with secondary cleanup/scan/summary failures
recorded separately. Exception messages stay private. Attempted suites become
completed only after the full proof and owned cleanup pass. A failed diagnostic
does not establish RDP incompatibility or successful live guest cleanup.

Failed Cua connects also retain a closed CLI error code and source producer,
PasswordCommand category or negotiation/finalize substage when the existing
JSON envelope matches. Only the exact protocol-2 no-start timeout can report
its recorded bootstrap stages; missing or malformed stages are unavailable.
The primary snapshot includes each relay's accepted connections, successful
upstream connections and bytes whose write/drain completed in each direction,
plus the held daemon's alive/exited state. These describe activity, not
authenticated RDP or guest execution. Raw messages and packet contents stay private.

Before failed-Cua cleanup removes the initially absent owned cache,
`local-acquisition.json` records absent, invalid, observation_failed or
verified_source_bundle. Verification reads only the contained bridge, ZIP and
manifest, checks the built bridge hash, archive hash, recomputed bundle identity,
flat archive entries and their actual hashes against the manifest, including
`cua-driver.exe`. It never extracts or removes files, and local assembly is not
running guest provenance. Observer failures are supplemental; the failed proof
stays red and every cleanup action is still attempted.

Diagnostic parsing is limited to 64 KiB of CLI JSON and 16 KiB of message,
nine unique bootstrap stages and integer counters through 2^63-1. Native
assembly observation reads at most 16 bundle entries, a 1 MiB manifest,
64 MiB bridge, 512 MiB archive, 1 MiB central directory, 4,096 archive entries
and 2 GiB of expanded content. ZIP central metadata is checked before parsing;
ZIP64 is outside this observer. Exceeding a diagnostic budget yields unavailable/observation_failed;
these limits do not change product or live-proof acceptance. Unknown labels,
unsafe/reparse paths and ambiguous assemblies never qualify as verified.

The Python harnesses remain usable against supplied remote Windows credentials;
they provision nothing. Their existing documented environment variables and
credential-file interfaces are unchanged. To require source delivery, add
`--bundle <bridge-directory> --source-bridge-sha256 <sha256> --cua-version 0.34.0`.
The proof harnesses write that semantic version as the explicit
`CuaVersion cua-driver-rs-v0.34.0` release tag in their hosts files, while the
guest manifest and provenance checks must still report version `0.34.0`.
For loopback viewer/takeover use `--bind loopback --allow-no-tailnet`.

Coverage exclusions are explicit: tailnet/cross-machine access, UAC/secure
desktop, live recording's Linux metrics/third user and the optional guest Job
executable. Windows containment tests remain separate required checks. Pillow
records image differences without introducing a pixel threshold. Missing
required checks, unknown exclusions, fake execution, skipped faults or absent
provenance cannot pass the live gate. Release invokes the same required CI.

For local setup diagnosis, use `--setup-diagnostic` during both initialization
and execution of `hosted_desktop.py`. Host errors expose only fixed subaction
and category labels and numeric native/HRESULT codes; raw host stderr stays private.
