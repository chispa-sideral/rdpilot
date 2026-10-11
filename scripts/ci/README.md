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
own RDP listener. `run-hosted-desktop.ps1 -Proof cua|viewer|takeover|perf` creates
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

### Performance workload

`run-hosted-desktop.ps1 -Proof perf` runs `scripts/e2e/run-perf-proof.py`
for one test user. The workflow input `perf_workload` of the manual
`workflow_dispatch` trigger starts it: on such a run the `desktop` job builds
the binaries, installs the browser and runs the performance step, and skips the
live Cua, viewer and takeover proofs. A push, a pull request and a release
call never run the step, and the per-push job is unchanged.

The wrapper copies the painter (`scripts/e2e/perf/painter.ps1` and
`PixelClock.cs`) to a directory that the test user can write and starts it
from a logon entry of that user. The entry and the directory are removed in
cleanup; `cleanup.json` has a `painter_removed` readback. The session connects
with Cua disabled.

The step uploads the `perf-workload-result` artifact (90 days) with
`perf-result.json` and the raw samples. The offline job runs the same script
with `--fake` against synthetic frames of a known age
(`run-offline-proof.py perf`). The docstring of `run-perf-proof.py` has the
workload definitions and the measurement method. `scripts/e2e/perf/baseline.toml`
has the targets and the baseline. Each run that measures W1 also writes
`perf-comparison.html` and `perf-comparison.md` to the `perf-workload-result`
artifact, and a live run adds the table to the job summary. The comparison is
report only.

Not executed live: the recording proof (three concurrent users and POSIX
client checks; offline only), tailnet and cross-machine viewing, UAC and the
secure desktop, Windows Job containment over RDP, download of a published
bridge release, and domain or production authentication.

## Evidence matrix

Every `ci.yml` run publishes the artifact `evidence-matrix`, which holds
`evidence-matrix.toml` and `evidence-matrix.html`. The report-only job
`evidence-matrix` makes it with `scripts/ci/evidence-matrix.py render`. The job
is not in `success.needs`, and a failure of the job does not fail the run or a
release that calls `ci.yml`.

Each row is one assertion selector of the tree and has the ID
`<kind>/<selector>`:

- `offline/<proof>/<label>`: a check of `run-offline-proof.py`.
- `e2e/<script>/<site>`: a `check()` site of `scripts/e2e/*.py`. A generated
  part of the label is `{}`. The labels that the run produced are in the row.
- `cargo/<package>/lib`, `lib/<module>`, `bin/<name>` or `test/<name>`: a Cargo
  test target. The libraries of `split_packages` have one selector per module.
- `ignored/<repo path>::<fn>`: an `#[ignore]` test.
- `file/<repo path>`: a `scripts/ci/test_*.py` or `scripts/e2e/*.sh` file.

`evidence-rules.toml` gives each selector its tier, mode, cadence and the
`ci.yml` steps that run it. A rule has a `kind`, a glob in `match` and the
steps in `runs`. The first rule of a kind whose glob fits a selector wins, so a
rule for one OS goes before a general rule. The same file holds the definitions
of the tiers, modes and cadences, the placement rules and the duration limits
that the HTML shows. It holds no run data.

The unit tests (`test_evidence_matrix.py`, in the step "CI and publisher
failure-injection tests") run `evidence-matrix.py check`. The check fails when
a selector has no rule, a rule fits no selector, or a rule names a `ci.yml` job
or step that does not exist. A new test target, `#[ignore]` test or `check()`
label in a new script therefore needs a rule in the same change. A new label in
a script that a rule already covers needs none. The check does not run cargo.

A row result is `passed`, `failed`, `skipped` or `not run`:

- A proof row (`offline`, `e2e`) is `passed` when a listed proof reports the
  label as passed. The summary of a proof lists passed checks only, so a missing
  label in the summary of a failed proof is `failed` with the note "failed or
  not reached". A missing label in the summary of a passed proof is `not run`:
  the proof did not reach the check. A label is matched by name within the
  summaries of the listed proofs.
- Other rows take the conclusion of every listed step. A row without a step is
  `not run` and shows why.
- The job reads the steps and the durations of the run from the jobs API of the
  repository, with the job token and then without a token. The API is not read
  when `release.yml` calls `ci.yml`, and a failed read does not fail the job.
  The row results then come from the job results, and the artifact states this
  in `jobs_api`. Durations are reported against the limits in the rules file
  and are never enforced.
