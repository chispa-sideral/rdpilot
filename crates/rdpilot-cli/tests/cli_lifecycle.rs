//! CLI-01 offline integration proof: `rdpilot connect` (with no daemon
//! pre-started) auto-starts the REAL `rdpilot-daemon` binary via
//! `rdpilot_ipc::transport::connect_or_spawn`, then `list`/`disconnect`
//! round-trip against it end to end — driven entirely as subprocesses of
//! the compiled `rdpilot` binary, mirroring
//! `rdpilot-daemon/tests/autostart_lifecycle.rs`'s isolation pattern
//! (`XDG_RUNTIME_DIR` pointed at a fresh temp dir,
//! `RDPILOT_DAEMON_TEST_CONNECTOR=1` so the auto-started daemon uses its
//! in-process fake connector — no real RDP target needed).
//!
//! Not `#[ignore]`-gated (unlike `autostart_lifecycle.rs`'s heavier
//! soak-adjacent daemon-process test): this IS the plan's own `<verify>`
//! block (`cargo test -p rdpilot-cli`), and CLI-01 is a first-class
//! must-have for this plan, not an opt-in heavy test.
//!
//! Requires `cargo build --workspace` (or an equivalent prior build) to
//! have produced BOTH the `rdpilot` and `rdpilot-daemon` binaries in the
//! same target directory before this test runs — `rdpilot-cli` never
//! depends on `rdpilot-daemon` (thin-client invariant, D-17), so
//! `cargo test -p rdpilot-cli` alone cannot build the daemon binary itself;
//! the CLI subprocess locates it at runtime purely by sibling-directory
//! convention (`std::env::current_exe().with_file_name("rdpilot-daemon")`,
//! `connect.rs`), exactly like the real installed-alongside-each-other
//! production layout.
//!
//! **Stdio pitfall (test-harness-only, not a production bug):**
//! `connect_or_spawn` (relocated verbatim from `rdpilot-daemon`, consumed
//! read-only here — see the plan's binding constraints) spawns the daemon
//! via a plain `std::process::Command::new(daemon_exe).spawn()`, with no
//! stdio redirection. In a real interactive terminal that's harmless (the
//! daemon just inherits the terminal's stdout/stderr, same as the shell
//! that launched the CLI). But `std::process::Command::output()`/
//! `wait_with_output()` captures a CHILD's stdout/stderr via an OS pipe and
//! blocks reading until EVERY process holding the pipe's write end closes
//! it — and the long-lived, detached daemon grandchild inherits that same
//! pipe (it is never `.wait()`-ed by `connect_or_spawn`, by design). Using
//! `output()` here would therefore hang the CLI-invoking test until the
//! auto-started daemon itself exits. `run_cli` below sidesteps this by
//! redirecting the CLI subprocess's stdout/stderr to real files instead of
//! pipes (`Stdio::from(File)` + `Command::status()`, never `output()`) —
//! reading a file back after the immediate child exits does not wait on
//! any other process's open file descriptor the way a pipe read does.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// A unique temp root per test invocation — isolates `XDG_RUNTIME_DIR` (the
/// daemon's socket directory) so this test never collides with a real
/// daemon or another concurrent test run.
fn unique_temp_root() -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    std::env::temp_dir().join(format!("rdpilot-cli-lifecycle-{}-{nanos}", std::process::id()))
}

/// One CLI subprocess invocation's captured result.
struct CliRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run the compiled `rdpilot` CLI binary as a subprocess with `args`,
/// isolated to `xdg_runtime_dir`/`sink_path` — the auto-started daemon uses
/// the fake in-process connector, never a real RDP target.
///
/// `capture_dir`/`call_index` name this invocation's stdout/stderr capture
/// files uniquely — see the module doc's stdio pitfall for why real files
/// (not piped `output()`) are used.
fn run_cli(args: &[&str], xdg_runtime_dir: &Path, sink_path: &Path, capture_dir: &Path, call_index: usize) -> CliRun {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
    let daemon_bin = bin.with_file_name(if cfg!(windows) { "rdpilot-daemon.exe" } else { "rdpilot-daemon" });
    assert!(
        daemon_bin.exists(),
        "expected the rdpilot-daemon binary at {daemon_bin:?} — run `cargo build --workspace` \
         before this test (rdpilot-cli intentionally never depends on rdpilot-daemon, so \
         `cargo test -p rdpilot-cli` alone cannot build it)"
    );

    let stdout_path = capture_dir.join(format!("call-{call_index}-stdout.log"));
    let stderr_path = capture_dir.join(format!("call-{call_index}-stderr.log"));
    let stdout_file = std::fs::File::create(&stdout_path).expect("create stdout capture file");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr capture file");

    let status = Command::new(&bin)
        .args(args)
        .env("XDG_RUNTIME_DIR", xdg_runtime_dir)
        .env("RDPILOT_DAEMON_SINK_PATH", sink_path)
        .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
        // Long enough that neither the idle reaper nor the empty-registry
        // grace period fires during this test's short, sequential
        // connect->list->disconnect->disconnect round trip.
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "60000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000")
        .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        // `.status()`, never `.output()`/`.wait_with_output()` — see the
        // module doc's stdio pitfall.
        .status()
        .unwrap_or_else(|e| panic!("spawning the compiled rdpilot binary must succeed: {e}"));

    CliRun {
        status,
        stdout: std::fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

#[test]
fn connect_list_disconnect_lifecycle_auto_starts_the_real_daemon() {
    let root = unique_temp_root();
    let xdg_runtime_dir = root.join("xdg-runtime");
    let capture_dir = root.join("capture");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    std::fs::create_dir_all(&capture_dir).expect("create the stdio capture dir");
    let sink_path = root.join("sessions.json");
    let socket_path = xdg_runtime_dir.join("rdpilot").join("daemon.sock");

    // Precondition: nothing is running yet — the first `connect` call must
    // be what auto-starts the daemon (CLI-01/DAEMON-03).
    assert!(!socket_path.exists(), "no daemon should be listening before the CLI's first invocation");

    // --- connect (auto-starts the daemon) -> Connected ---
    let connect_run = run_cli(
        &["connect", "--name", "web", "--host", "10.0.0.5", "--username", "u", "--password", "p", "--json"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        1,
    );
    assert!(
        connect_run.status.success(),
        "connect must exit 0; stdout={} stderr={}",
        connect_run.stdout,
        connect_run.stderr
    );
    let connect_json: serde_json::Value =
        serde_json::from_str(connect_run.stdout.trim()).expect("connect --json must emit valid JSON");
    let session_id = connect_json["session"].as_str().expect("connect --json must include a session id").to_owned();
    assert_eq!(session_id, "web", "a caller-supplied name reserves that exact session id (D-29)");
    assert!(socket_path.exists(), "the daemon must be listening after connect auto-starts it");

    // --- list --json -> the session, status Live (D-30) ---
    let list_run = run_cli(&["list", "--json"], &xdg_runtime_dir, &sink_path, &capture_dir, 2);
    assert!(list_run.status.success(), "list must exit 0; stderr={}", list_run.stderr);
    let sessions_json: serde_json::Value =
        serde_json::from_str(list_run.stdout.trim()).expect("list --json must emit valid JSON");
    let sessions = sessions_json.as_array().expect("list --json must emit a JSON array");
    let entry = sessions
        .iter()
        .find(|s| s["id"].as_str() == Some(session_id.as_str()))
        .unwrap_or_else(|| panic!("expected session {session_id} in list output: {sessions:?}"));
    assert_eq!(entry["status"].as_str(), Some("Live"), "a fake-connector session must be Live immediately (D-30)");
    assert_eq!(entry["name"].as_str(), Some("web"), "the caller-supplied name must round-trip");
    assert_eq!(entry["host"].as_str(), Some("10.0.0.5"));

    // --- disconnect -> success ---
    let disconnect_run = run_cli(&["disconnect", "--session", &session_id], &xdg_runtime_dir, &sink_path, &capture_dir, 3);
    assert!(
        disconnect_run.status.success(),
        "disconnect must exit 0; stdout={} stderr={}",
        disconnect_run.stdout,
        disconnect_run.stderr
    );

    // --- a second disconnect on the now-gone session -> SessionNotFound, exit 2 (D-28) ---
    let second_disconnect =
        run_cli(&["disconnect", "--session", &session_id], &xdg_runtime_dir, &sink_path, &capture_dir, 4);
    assert!(!second_disconnect.status.success(), "a disconnect on an unknown session must not exit 0");
    assert_eq!(
        second_disconnect.status.code(),
        Some(2),
        "SessionNotFound must map to exit code 2 (D-28); stderr={}",
        second_disconnect.stderr
    );

    let _ = std::fs::remove_dir_all(&root);
}
