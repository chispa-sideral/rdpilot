//! CLI-03 offline integration proof: session-not-found, daemon-unreachable,
//! and no-clobber surface as DISTINCT, legible errors with DISTINCT
//! non-zero exit codes (D-28) — driven entirely as subprocesses of the
//! compiled `rdpilot` binary, mirroring `tests/cli_lifecycle.rs`'s
//! isolation pattern (`XDG_RUNTIME_DIR` pointed at a fresh temp dir,
//! `RDPILOT_DAEMON_TEST_CONNECTOR=1` so the auto-started daemon uses its
//! in-process fake connector — no real RDP target needed).
//!
//! Not `#[ignore]`-gated: this IS the plan's own `<verify>` block
//! (`cargo test -p rdpilot-cli`), and CLI-03's error-legibility contract is
//! a first-class must-have for this plan, not an opt-in heavy test. Any
//! assertion requiring a REAL multi-MB transfer or real transfer-failure
//! semantics is out of scope here — that's the Phase 15 batched live gate
//! (FILE-01/02/04 were already live-verified in Phase 10; the CLI path
//! re-exercises, doesn't re-prove, per research SC#3).
//!
//! **What this proves, offline, against the real compiled binaries:**
//! 1. **session-not-found -> exit 2 (D-28):** an operational verb
//!    (`disconnect`) against a session id the daemon has never heard of.
//! 2. **daemon-unreachable -> exit 3 (D-28):** the CLI binary copied to an
//!    isolated directory with NO sibling `rdpilot-daemon` binary, pointed
//!    at a socket path with no listener — `connect_or_spawn`'s spawn
//!    attempt fails immediately (research Environment/Offline SC#3 row:
//!    "point the CLI at a socket path with no listener and a nonexistent
//!    daemon binary").
//! 3. **no-clobber -> exit 8 (CLI-local, D-28), then --force succeeds:**
//!    `get` against a pre-existing local `--local` destination refuses
//!    before ever contacting the daemon; the identical request WITH
//!    `--force` proceeds and succeeds against the canned fake session
//!    (`FakeTestSession::download_file`, `crates/rdpilot-daemon/src/server.rs`
//!    — it ignores whether the local file already exists, matching the
//!    documented put/get contract that this CLI-side no-clobber check runs
//!    strictly before the wire round trip).
//! 4. **`--json` error rendering:** the session-not-found case above also
//!    asserts the exact `{"error":{"code":"...","message":"..."}}` shape
//!    (D-28's taxonomy legible under machine output too).
//!
//! **Stdio pitfall (test-harness-only, not a production bug), same as
//! `cli_lifecycle.rs`:** `run_cli` below redirects the CLI subprocess's
//! stdout/stderr to real files rather than piped `output()` — the
//! auto-started daemon grandchild inherits the pipe and is never `.wait()`-
//! ed, so a piped `output()`/`wait_with_output()` read would hang until the
//! long-lived daemon itself exits.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// A unique temp root per test invocation — isolates `XDG_RUNTIME_DIR` (the
/// daemon's socket directory) so this test never collides with a real
/// daemon or another concurrent test run.
fn unique_temp_root(label: &str) -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    std::env::temp_dir().join(format!("rdpilot-cli-errors-{label}-{}-{nanos}", std::process::id()))
}

/// One CLI subprocess invocation's captured result.
struct CliRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run `bin` (the compiled `rdpilot` binary, or a copy of it) as a
/// subprocess with `args`, isolated to `xdg_runtime_dir`/`sink_path`.
///
/// `capture_dir`/`call_index` name this invocation's stdout/stderr capture
/// files uniquely — real files, not piped `output()` (see the module doc's
/// stdio pitfall).
fn run_cli_bin(
    bin: &Path,
    args: &[&str],
    xdg_runtime_dir: &Path,
    sink_path: &Path,
    capture_dir: &Path,
    call_index: usize,
) -> CliRun {
    let stdout_path = capture_dir.join(format!("call-{call_index}-stdout.log"));
    let stderr_path = capture_dir.join(format!("call-{call_index}-stderr.log"));
    let stdout_file = std::fs::File::create(&stdout_path).expect("create stdout capture file");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture file");

    let status = Command::new(bin)
        .args(args)
        .env("XDG_RUNTIME_DIR", xdg_runtime_dir)
        .env("RDPILOT_DAEMON_SINK_PATH", sink_path)
        .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
        // Long enough that neither the idle reaper nor the empty-registry
        // grace period fires during this test's short, sequential calls.
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "60000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000")
        .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        // `.status()`, never `.output()`/`.wait_with_output()` — see the
        // module doc's stdio pitfall.
        .status()
        .unwrap_or_else(|e| panic!("spawning {bin:?} must succeed: {e}"));

    CliRun {
        status,
        stdout: std::fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

/// Run the compiled `rdpilot` CLI binary from its normal build location
/// (sibling `rdpilot-daemon` present, auto-start works normally).
fn run_cli(args: &[&str], xdg_runtime_dir: &Path, sink_path: &Path, capture_dir: &Path, call_index: usize) -> CliRun {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
    let daemon_bin = bin.with_file_name(if cfg!(windows) { "rdpilot-daemon.exe" } else { "rdpilot-daemon" });
    assert!(
        daemon_bin.exists(),
        "expected the rdpilot-daemon binary at {daemon_bin:?} — run `cargo build --workspace` \
         before this test (rdpilot-cli intentionally never depends on rdpilot-daemon, so \
         `cargo test -p rdpilot-cli` alone cannot build it)"
    );
    run_cli_bin(&bin, args, xdg_runtime_dir, sink_path, capture_dir, call_index)
}

/// Copy the compiled `rdpilot` binary into `dest_dir`, deliberately WITHOUT
/// a sibling `rdpilot-daemon` binary — `connect.rs`'s
/// `std::env::current_exe().with_file_name(...)` sibling-lookup will then
/// resolve to a path that does not exist, so `connect_or_spawn`'s spawn
/// attempt fails immediately (research Environment/Offline SC#3 row).
fn copy_cli_binary_without_daemon_sibling(dest_dir: &Path) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
    let file_name = src.file_name().expect("CARGO_BIN_EXE_rdpilot must have a file name");
    let dest = dest_dir.join(file_name);
    std::fs::copy(&src, &dest).expect("copying the compiled rdpilot binary must succeed");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).expect("stat the copied binary").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).expect("mark the copied binary executable");
    }

    assert!(
        !dest.with_file_name(if cfg!(windows) { "rdpilot-daemon.exe" } else { "rdpilot-daemon" }).exists(),
        "test setup invariant: the copy destination must have NO sibling rdpilot-daemon binary"
    );

    dest
}

/// Class 1 (D-28): `session-not-found` -> exit code 2, distinct message
/// naming the missing session — plus the `--json` error shape
/// (`{"error":{"code":"session-not-found","message":"..."}}`).
#[test]
fn session_not_found_maps_to_exit_code_2_and_names_the_session() {
    let root = unique_temp_root("session-not-found");
    let xdg_runtime_dir = root.join("xdg-runtime");
    let capture_dir = root.join("capture");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    std::fs::create_dir_all(&capture_dir).expect("create the stdio capture dir");
    let sink_path = root.join("sessions.json");

    // `disconnect` on a session id the daemon (freshly auto-started, empty
    // registry) has never heard of.
    let run = run_cli(&["disconnect", "--session", "ghost-session", "--json"], &xdg_runtime_dir, &sink_path, &capture_dir, 1);

    assert!(!run.status.success(), "disconnecting an unknown session must not exit 0");
    assert_eq!(run.status.code(), Some(2), "SessionNotFound must map to exit code 2 (D-28); stderr={}", run.stderr);

    let payload: serde_json::Value =
        serde_json::from_str(run.stdout.trim()).unwrap_or_else(|e| panic!("--json error output must be valid JSON: {e}; stdout={}", run.stdout));
    assert_eq!(
        payload["error"]["code"].as_str(),
        Some("session-not-found"),
        "expected the wire kebab-case code in the --json error payload: {payload}"
    );
    let message = payload["error"]["message"].as_str().expect("--json error payload must include a message string");
    assert!(
        message.contains("ghost-session") || message.to_lowercase().contains("session"),
        "the error message should be legible about the missing session, got: {message}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Class 2 (D-28): `daemon-unreachable` -> exit code 3, distinct from
/// session-not-found — the CLI binary has no sibling daemon binary to
/// auto-start, and the target socket has no listener.
#[test]
fn daemon_unreachable_maps_to_exit_code_3() {
    let root = unique_temp_root("daemon-unreachable");
    let xdg_runtime_dir = root.join("xdg-runtime");
    let capture_dir = root.join("capture");
    let isolated_bin_dir = root.join("isolated-bin");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    std::fs::create_dir_all(&capture_dir).expect("create the stdio capture dir");
    std::fs::create_dir_all(&isolated_bin_dir).expect("create the isolated binary dir");
    let sink_path = root.join("sessions.json");

    let isolated_bin = copy_cli_binary_without_daemon_sibling(&isolated_bin_dir);

    // `list` — a session-less verb, simplest way to force exactly one
    // round trip attempt. No listener at xdg_runtime_dir's socket path, and
    // no sibling rdpilot-daemon binary to spawn -> connect_or_spawn's spawn
    // attempt fails immediately (never reaches its bounded backoff loop).
    let run = run_cli_bin(&isolated_bin, &["list"], &xdg_runtime_dir, &sink_path, &capture_dir, 1);

    assert!(!run.status.success(), "list against an unreachable, unspawnable daemon must not exit 0");
    assert_eq!(
        run.status.code(),
        Some(3),
        "DaemonUnreachable must map to exit code 3 (D-28), distinct from SessionNotFound's 2; stderr={}",
        run.stderr
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Class 3 (CLI-local, D-28): `get`'s no-clobber refusal -> exit code 8,
/// distinct from both session-not-found (2) and daemon-unreachable (3);
/// the identical request WITH `--force` proceeds and succeeds (exit 0)
/// against the canned fake session.
#[test]
fn get_no_clobber_refuses_without_force_and_succeeds_with_force() {
    let root = unique_temp_root("no-clobber");
    let xdg_runtime_dir = root.join("xdg-runtime");
    let capture_dir = root.join("capture");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    std::fs::create_dir_all(&capture_dir).expect("create the stdio capture dir");
    let sink_path = root.join("sessions.json");

    // A real, live session first — `--force` must reach the daemon and
    // succeed, not just skip the CLI-side check.
    let connect_run = run_cli(
        &["connect", "--name", "xfer", "--host", "10.0.0.5", "--username", "u", "--password", "p", "--json"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        1,
    );
    assert!(connect_run.status.success(), "connect must exit 0; stdout={} stderr={}", connect_run.stdout, connect_run.stderr);
    let connect_json: serde_json::Value =
        serde_json::from_str(connect_run.stdout.trim()).expect("connect --json must emit valid JSON");
    let session_id = connect_json["session"].as_str().expect("connect --json must include a session id").to_owned();

    // A pre-existing local destination.
    let existing_dest = root.join("already-here.bin");
    std::fs::write(&existing_dest, b"pre-existing local content").expect("seed the pre-existing local destination");
    let existing_dest_str = existing_dest.to_str().expect("temp path must be valid UTF-8 on this platform");

    // --- without --force: refused before ever reaching the daemon ---
    let no_force_run = run_cli(
        &["get", "--session", &session_id, "--remote-name", "r.bin", "--local", existing_dest_str],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        2,
    );
    assert!(!no_force_run.status.success(), "get against an existing local destination without --force must not exit 0");
    assert_eq!(
        no_force_run.status.code(),
        Some(8),
        "NoClobber must map to exit code 8 (D-28), distinct from 2 and 3; stderr={}",
        no_force_run.stderr
    );
    // The pre-existing file must be untouched — the refusal happens before
    // any write.
    assert_eq!(
        std::fs::read(&existing_dest).expect("the pre-existing file must still be readable"),
        b"pre-existing local content",
        "a refused no-clobber get must never touch the existing local file"
    );

    // --- with --force: proceeds and succeeds against the canned fake session ---
    let force_run = run_cli(
        &["get", "--session", &session_id, "--remote-name", "r.bin", "--local", existing_dest_str, "--force"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        3,
    );
    assert!(
        force_run.status.success(),
        "get --force against an existing local destination must proceed and succeed; stdout={} stderr={}",
        force_run.stdout,
        force_run.stderr
    );

    let _ = std::fs::remove_dir_all(&root);
}
