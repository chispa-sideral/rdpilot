---
phase: 13-cli-surface
plan: 05
subsystem: cli
tags: [clap, rust-cli, unix-socket, tokio, session-lifecycle, rdpilot-ipc, rdpilot-config]

# Dependency graph
requires:
  - phase: 13-cli-surface (13-01)
    provides: "rdpilot_ipc::transport (connect_or_spawn/socket_path/read_frame/write_frame) relocated out of rdpilot-daemon, dependency-free"
  - phase: 13-cli-surface (13-04)
    provides: "daemon dispatch wired to the live Registry for every wire verb (Connect/List/Disconnect already routed since Phase 12)"
provides:
  - "The rdpilot-cli binary crate (bin `rdpilot`), the workspace's first thin daemon client"
  - "connect [--name] / list / disconnect session-lifecycle verbs, transparently auto-starting the daemon on first use (CLI-01)"
  - "Shared clap command tree (flat + grouped `session`), config-flag layer, transport client, exit-code mapping, and hand-rolled table renderer that Plans 13-06/13-07 build on"
affects: [13-06-file-transfer-verbs, 13-07-perception-input-verbs, 15-proof-harnesses]

# Tech tracking
tech-stack:
  added: ["clap 4.6.1 (derive)", "rdpilot-cli crate (bin: rdpilot)"]
  patterns:
    - "clap Subcommand enum mixing flat leaf variants with a nested grouped subcommand that shares the SAME arg structs (or-pattern dispatch in main.rs collapses both spellings to one handler call)"
    - "CliError as a plain hand-written enum (Display + Error, no thiserror) wrapping either a typed WireError or a client-local failure class, with a private code_for(&CliError)->u8 unit-tested independently of process::ExitCode"
    - "Sibling-binary resolution via std::env::current_exe().with_file_name(\"rdpilot-daemon\") — same convention the cli_lifecycle test uses to locate the daemon binary it never depends on"

key-files:
  created:
    - crates/rdpilot-cli/Cargo.toml
    - crates/rdpilot-cli/src/main.rs
    - crates/rdpilot-cli/src/cli.rs
    - crates/rdpilot-cli/src/config_flags.rs
    - crates/rdpilot-cli/src/connect.rs
    - crates/rdpilot-cli/src/exit_codes.rs
    - crates/rdpilot-cli/src/render/mod.rs
    - crates/rdpilot-cli/src/render/table.rs
    - crates/rdpilot-cli/src/verbs/mod.rs
    - crates/rdpilot-cli/src/verbs/session.rs
    - crates/rdpilot-cli/tests/cli_lifecycle.rs
  modified:
    - Cargo.toml

key-decisions:
  - "Added serde (no derive feature) as a direct rdpilot-cli dependency, not in the plan's dependency list — required so `print_json<T: serde::Serialize>`'s trait bound resolves the `serde` path directly (Rust requires a direct crate dependency to reference a crate path even when a transitive dependency, e.g. serde_json, already re-exports the trait's impls); a blocking compile issue, Rule 3."
  - "Split main.rs's dispatch into two commits matching the plan's two tasks: Task 1 lands a scaffolding stub dispatch (module-level #[allow(dead_code, unused_imports)], mirroring rdpilot-daemon::registry's established interface-first convention) that compiles and passes the thin-client cargo-tree check standalone; Task 2 replaces it with the real verbs::session wiring."
  - "cli_lifecycle.rs redirects the CLI subprocess's stdout/stderr to real files (Stdio::from(File) + Command::status()) instead of Command::output() — see Deviations."

requirements-completed: [CLI-01]

# Metrics
duration: ~55min
completed: 2026-07-11
---

# Phase 13 Plan 05: rdpilot-cli scaffold + connect/list/disconnect Summary

**A new thin `rdpilot-cli` binary crate (`rdpilot`) implementing `connect [--name] / list / disconnect` over the daemon with transparent auto-start, proven end-to-end offline against the real compiled `rdpilot-daemon` binary via a subprocess-driven integration test.**

## Performance

- **Duration:** ~55 min
- **Started:** 2026-07-11T07:36:00Z (approx, first context read)
- **Completed:** 2026-07-11T08:31:45Z
- **Tasks:** 2/2
- **Files modified:** 11 (10 created + 1 modified: root `Cargo.toml`)

## Accomplishments

- New `rdpilot-cli` workspace crate producing the `rdpilot` binary, depending ONLY on `clap`/`rdpilot-ipc`/`rdpilot-config`/`tokio`/`serde`/`serde_json` — `cargo tree -p rdpilot-cli` is verified free of `ironrdp`/`rustls`/`rdpilot`/`rdpilot-daemon` (thin-client invariant, D-17, T-13-14).
- `connect [--name] [--host --port --username --password --domain --accept-invalid-certs]` resolves the file→env→flag layered config (`rdpilot_config::resolve`), sends `Request::Connect`, and prints/renders the (possibly caller-reserved) session id.
- `list` renders `id/name/host/status/connected-since/last-activity` using the D-30 lifecycle vocabulary verbatim (`Connecting/Live/Reconnecting/Disconnected/Orphaned`), human table by default, `--json` for machine consumption.
- `disconnect --session <id>` sends `Request::Disconnect`; a `SessionNotFound` wire error maps to exit code 2 (D-28).
- Every session-scoped verb accepts BOTH the flat spelling (`rdpilot connect ...`) and the grouped spelling (`rdpilot session connect ...`) via one shared `ConnectArgs`/`SessionArg` parser and an or-pattern dispatch in `main.rs`.
- `exit_codes.rs` maps the full D-28 taxonomy to 8 distinct non-zero exit codes (`SessionNotFound`→2, `DaemonUnreachable`→3, `TransferFailed`→4, `PathTraversal`→5, `ChecksumMismatch`→6, `DuplicateSession`→7, `NoClobber`→8 (declared now, wired in 13-07), everything else→1).
- `tests/cli_lifecycle.rs` (not `#[ignore]`-gated — part of the default `cargo test -p rdpilot-cli` run) drives the REAL compiled `rdpilot-daemon` binary as a subprocess of the CLI, isolated via a fresh `XDG_RUNTIME_DIR` + `RDPILOT_DAEMON_TEST_CONNECTOR=1` fake connector: proves no daemon is listening before the first `connect`, auto-start on that first invocation, `Live` status + name/host round-trip through `list --json`, a clean `disconnect`, and a second `disconnect` on the now-gone session exiting 2 (`SessionNotFound`).

## Task Commits

Each task was committed atomically:

1. **Task 1: Scaffold the rdpilot-cli crate, clap command tree, config-flag layer, transport client, and exit-code mapping** - `a9dc486` (feat)
2. **Task 2: Implement connect/list/disconnect verbs and prove them end-to-end against the real daemon (fake connector)** - `0b37d42` (feat)

**Plan metadata:** (this commit, see below)

## Files Created/Modified

- `Cargo.toml` — added `crates/rdpilot-cli` to the workspace `members` array
- `crates/rdpilot-cli/Cargo.toml` — new crate manifest (`clap`/`serde`/`rdpilot-ipc`/`rdpilot-config`/`tokio`/`serde_json`; no `rdpilot`/`rdpilot-daemon`)
- `crates/rdpilot-cli/src/main.rs` — `#[tokio::main(flavor = "current_thread")]` entry point, `Cli::parse()` → `dispatch` → `exit_code_for` translation
- `crates/rdpilot-cli/src/cli.rs` — `Cli`/`Command`/`SessionCmd`/`ConnectArgs`/`SessionArg` clap derive tree
- `crates/rdpilot-cli/src/config_flags.rs` — `ConfigFlags` (D-27 verbatim flag names) + `into_overrides()`
- `crates/rdpilot-cli/src/connect.rs` — `open_stream`/`round_trip`, wrapping `rdpilot_ipc::transport::connect_or_spawn`
- `crates/rdpilot-cli/src/exit_codes.rs` — `CliError` + `exit_code_for` (D-28 distinct-code mapping), unit-tested
- `crates/rdpilot-cli/src/render/mod.rs` + `render/table.rs` — hand-rolled table printer + `print_json`
- `crates/rdpilot-cli/src/verbs/mod.rs` + `verbs/session.rs` — `connect`/`list`/`disconnect` handlers
- `crates/rdpilot-cli/tests/cli_lifecycle.rs` — CLI-01 offline integration proof against the real daemon binary

## Decisions Made

- **Added `serde` as a direct dependency** (not listed in the plan's Task 1 dependency set) so `render::print_json<T: serde::Serialize>`'s generic bound resolves — Rust requires a direct crate dependency to name a crate's path even when a transitive dependency (here `serde_json`, which itself depends on `serde`) already uses its traits. Rule 3 (blocking compile issue), documented.
- **Split the single-shot implementation into two commits matching the plan's task boundaries** by authoring an interim scaffolding `main.rs` for the Task 1 commit (module-level `#[allow(dead_code, unused_imports)]`, mirroring `rdpilot-daemon::registry`'s already-established interface-first convention) that compiles standalone and passes the Task 1 `cargo build -p rdpilot-cli && cargo tree` verify, then replacing it with the real `verbs::session`-wired `main.rs` for the Task 2 commit.
- **`CliError` is a hand-written enum** (manual `Display`/`Error` impls) rather than `thiserror`-derived, keeping the crate's dependency list to exactly what the plan specified (no `thiserror` added).
- **`#[tokio::main(flavor = "current_thread")]`** rather than the default multi-thread flavor — this is a single-shot invoke-and-exit process, not a persistent server; keeps the `tokio` feature list minimal (`rt`, not `rt-multi-thread`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added `serde` as a direct dependency for a trait-bound path reference**
- **Found during:** Task 1 (`render/mod.rs`'s `print_json`)
- **Issue:** `fn print_json<T: serde::Serialize>(...)` fails to compile — `serde` is only a *transitive* dependency (via `serde_json`/`rdpilot-ipc`/`rdpilot-config`), and Rust 2021 requires a direct `Cargo.toml` dependency to reference a crate's path, even when its trait impls are already in scope through another dependency's re-exports.
- **Fix:** Added `serde = "1"` (no `derive` feature — only the trait bound is needed) to `crates/rdpilot-cli/Cargo.toml`.
- **Files modified:** `crates/rdpilot-cli/Cargo.toml`
- **Verification:** `cargo build -p rdpilot-cli` succeeds; `cargo tree -p rdpilot-cli` still free of `ironrdp`/`rustls`/`rdpilot`/`rdpilot-daemon` (adding `serde` does not affect the thin-client invariant — it was already present transitively).
- **Committed in:** `a9dc486` (Task 1 commit)

**2. [Rule 3 - Blocking, test-only] Redirected the CLI subprocess's stdio to real files instead of a piped `Command::output()` in `cli_lifecycle.rs`**
- **Found during:** Task 2 (`tests/cli_lifecycle.rs` initial run)
- **Issue:** `connect_or_spawn` (relocated verbatim into `rdpilot-ipc`, consumed read-only per this plan's binding constraints) spawns the daemon via a plain `std::process::Command::new(daemon_exe).spawn()` with no stdio redirection — harmless in a real interactive terminal, but `std::process::Command::output()`/`wait_with_output()` captures a child's stdout/stderr through an OS pipe and blocks reading until EVERY holder of the pipe's write end closes it. The long-lived, detached daemon grandchild inherits that same pipe (by design it is never `.wait()`-ed by `connect_or_spawn`) and never closes it, so the first test run hung for ~120 seconds before failing a downstream assertion.
- **Fix:** `run_cli` in the test now redirects the CLI subprocess's stdout/stderr to per-invocation temp files (`Stdio::from(File)`) and uses `Command::status()` (never `.output()`), reading the files back after the immediate child exits — a file read does not wait on any other process's open descriptor the way a pipe read does. No production code (`rdpilot-ipc`, `rdpilot-daemon`, `rdpilot-cli`'s own `connect.rs`) was modified.
- **Files modified:** `crates/rdpilot-cli/tests/cli_lifecycle.rs`
- **Verification:** `cargo test -p rdpilot-cli --test cli_lifecycle` now completes in 0.06s and passes; the workspace-wide `cargo test --workspace` run also completed cleanly with the same fast time.
- **Committed in:** `0b37d42` (Task 2 commit)

---

**Total deviations:** 2 auto-fixed (2 Rule 3 — blocking issues)
**Impact on plan:** Both were required for the crate/test to compile and pass at all; no scope creep, no production-crate modification (rdpilot-ipc/rdpilot-daemon untouched, per the plan's binding constraint).

## Issues Encountered

- Leftover `rdpilot-daemon` processes were left running after each test invocation during manual verification (expected: the fake-connector daemon self-shuts-down only after its configured empty-registry grace period, which the test/manual runs set generously long — 60s — to avoid interfering with the sequential connect→list→disconnect flow). Killed manually after each verification pass; not a code defect — this mirrors the daemon's documented DAEMON-03 self-shutdown-on-empty behavior, already proven by `rdpilot-daemon/tests/autostart_lifecycle.rs` in Phase 12.
- `cargo`/`rustup` were not on `PATH` in the execution shell by default (`~/.cargo/bin` had to be prepended); the Windows-GNU default `rustup` toolchain channel errors on plain `rustup toolchain list` on this host, confirming the plan's instruction to substitute `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` + `--target x86_64-unknown-linux-gnu` for all build/test/tree/clippy invocations, which is what every command in this plan's execution used.
- No generic `Write`/`Edit` tool was available in this execution environment for non-`.planning` files (only `Read`/`Bash`/the `Perficio*` planning-scoped tools and Context7 were exposed) — all `crates/rdpilot-cli/**` source files were authored via `Bash` heredocs instead of a native file-write tool. Documented for transparency; does not affect the shipped code.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- **CLI-01 is fully satisfied and offline-proven**: `rdpilot connect/list/disconnect` manage session lifecycle over the daemon with transparent auto-start, verified end-to-end against the real `rdpilot-daemon` binary (fake connector, no live RDP target needed).
- **Shared CLI infrastructure is in place for Plans 13-06/13-07**: the clap command tree is structured so adding the `Perceive`/`Input`/`File` grouped families and the flat `Put`/`Get` leaves is purely additive (no rework); `exit_codes.rs` already declares the full D-28 taxonomy including the `NoClobber` class 13-07 will construct; `render/` is ready for additional response-variant rendering; `connect.rs`'s `round_trip` is verb-agnostic.
- **13-06 (file put/get + base64)** can proceed directly: it must add `base64` (developer-approved legitimacy checkpoint per research, NOT auto-added here per the plan's explicit exclusion) and wire the `File`/flat `Put`/`Get` verbs using the same `round_trip`/`render`/`exit_codes` scaffolding.
- **13-07 (perception/input verbs + full error taxonomy)** can wire the `Perceive`/`Input` grouped families and complete the `NoClobber`/`--force` check this plan declared but did not implement.
- No blockers identified.

---
*Phase: 13-cli-surface*
*Completed: 2026-07-11*

## Self-Check: PASSED

All 13 created/modified files confirmed present on disk (`crates/rdpilot-cli/Cargo.toml`, `src/main.rs`, `src/cli.rs`, `src/config_flags.rs`, `src/connect.rs`, `src/exit_codes.rs`, `src/render/mod.rs`, `src/render/table.rs`, `src/verbs/mod.rs`, `src/verbs/session.rs`, `tests/cli_lifecycle.rs`, root `Cargo.toml`, this SUMMARY). Both task commits (`a9dc486`, `0b37d42`) confirmed present in `git log --oneline --all`.
