//! Native RDP screenshot/input CLI integration against the daemon fixture.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// A unique temp root per test invocation — isolates `XDG_RUNTIME_DIR` (the
/// daemon's socket directory) so this test never collides with a real
/// daemon or another concurrent test run.
fn unique_temp_root() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("rdpilot-cli-verbs-{}-{nanos}", std::process::id()))
}

/// One CLI subprocess invocation's captured result.
struct CliRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run the compiled `rdpilot` CLI binary as a subprocess with `args`,
/// isolated to `xdg_runtime_dir`/`sink_path` — the auto-started daemon uses
/// the canned in-process fake connector, never a real RDP target.
///
/// `capture_dir`/`call_index` name this invocation's stdout/stderr capture
/// files uniquely — real files, not piped `output()` (see the module doc's
/// `cli_lifecycle.rs` cross-reference for why).
fn run_cli(
    args: &[&str],
    xdg_runtime_dir: &Path,
    sink_path: &Path,
    capture_dir: &Path,
    call_index: usize,
) -> CliRun {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
    let daemon_bin = bin.with_file_name(if cfg!(windows) {
        "rdpilot-daemon.exe"
    } else {
        "rdpilot-daemon"
    });
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
        // grace period fires during this test's sequential verb-by-verb run.
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "60000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000")
        .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        // `.status()`, never `.output()`/`.wait_with_output()` — see
        // `cli_lifecycle.rs`'s module doc for the pipe-hang pitfall this
        // avoids (the auto-started daemon grandchild inherits a piped fd
        // and is never `.wait()`-ed).
        .status()
        .unwrap_or_else(|e| panic!("spawning the compiled rdpilot binary must succeed: {e}"));

    CliRun {
        status,
        stdout: std::fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

#[test]
fn every_cli_02_verb_round_trips_against_the_canned_fake_session() {
    let root = unique_temp_root();
    let xdg_runtime_dir = root.join("xdg-runtime");
    let capture_dir = root.join("capture");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    std::fs::create_dir_all(&capture_dir).expect("create the stdio capture dir");
    let sink_path = root.join("sessions.json");
    let screenshot_path = root.join("shot.png");

    let mut call_index = 0_usize;
    let mut next_call = || {
        call_index += 1;
        call_index
    };

    // --- connect (auto-starts the daemon) -> Connected, reserving session id "cli02" ---
    let connect_run = run_cli(
        &[
            "connect",
            "--name",
            "cli02",
            "--host",
            "10.0.0.5",
            "--username",
            "u",
            "--password",
            "p",
            "--json",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        connect_run.status.success(),
        "connect must exit 0; stdout={} stderr={}",
        connect_run.stdout,
        connect_run.stderr
    );
    let session = "cli02";

    // --- perceive screenshot --output <file> -> a non-empty PNG-magic-byte file, nothing on stdout ---
    let screenshot_run = run_cli(
        &[
            "perceive",
            "screenshot",
            "--session",
            session,
            "--output",
            screenshot_path.to_str().expect("utf8 path"),
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        screenshot_run.status.success(),
        "perceive screenshot must exit 0; stdout={} stderr={}",
        screenshot_run.stdout,
        screenshot_run.stderr
    );
    let png_bytes =
        std::fs::read(&screenshot_path).expect("screenshot --output must have written a file");
    assert!(
        !png_bytes.is_empty(),
        "the written screenshot file must be non-empty"
    );
    assert_eq!(
        &png_bytes[0..4],
        &[0x89, b'P', b'N', b'G'],
        "the written bytes must start with the PNG magic number"
    );
    assert!(
        !screenshot_run
            .stdout
            .as_bytes()
            .windows(4)
            .any(|w| w == [0x89, b'P', b'N', b'G']),
        "raw PNG magic bytes must never appear on stdout (D-13.1)"
    );

    // --- input click -> Ack ---
    let click_run = run_cli(
        &[
            "input",
            "click",
            "--session",
            session,
            "--x",
            "10",
            "--y",
            "10",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        click_run.status.success(),
        "input click must exit 0; stdout={} stderr={}",
        click_run.stdout,
        click_run.stderr
    );

    // --- input scroll -> Ack ---
    let scroll_run = run_cli(
        &[
            "input",
            "scroll",
            "--session",
            session,
            "--x",
            "10",
            "--y",
            "10",
            "--dy",
            "-120",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        scroll_run.status.success(),
        "input scroll must exit 0; stdout={} stderr={}",
        scroll_run.stdout,
        scroll_run.stderr
    );

    // --- input drag -> Ack ---
    let drag_run = run_cli(
        &[
            "input",
            "drag",
            "--session",
            session,
            "--from-x",
            "0",
            "--from-y",
            "0",
            "--to-x",
            "10",
            "--to-y",
            "10",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        drag_run.status.success(),
        "input drag must exit 0; stdout={} stderr={}",
        drag_run.stdout,
        drag_run.stderr
    );

    // --- input type -> Ack ---
    let type_run = run_cli(
        &["input", "type", "--session", session, "--text", "hello"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        type_run.status.success(),
        "input type must exit 0; stdout={} stderr={}",
        type_run.stdout,
        type_run.stderr
    );

    // --- input key --combo ctrl,a -> Ack ---
    let key_run = run_cli(
        &["input", "key", "--session", session, "--combo", "ctrl,a"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        key_run.status.success(),
        "input key must exit 0; stdout={} stderr={}",
        key_run.stdout,
        key_run.stderr
    );

    // --- input key --combo with an unknown key name -> a legible non-zero exit, not a panic (T-13-17) ---
    let bad_key_run = run_cli(
        &[
            "input",
            "key",
            "--session",
            session,
            "--combo",
            "not-a-real-key",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        !bad_key_run.status.success(),
        "an unknown --combo key name must not exit 0"
    );
    assert!(
        bad_key_run.stderr.contains("unknown key name"),
        "expected a legible 'unknown key name' error on stderr, got: {}",
        bad_key_run.stderr
    );

    // --- disconnect -> success (registry empties; the daemon's own self-shutdown-on-empty
    // path is proven separately by `rdpilot-daemon/tests/autostart_lifecycle.rs`, not re-tested here) ---
    let disconnect_run = run_cli(
        &["disconnect", "--session", session],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        disconnect_run.status.success(),
        "disconnect must exit 0; stdout={} stderr={}",
        disconnect_run.stdout,
        disconnect_run.stderr
    );

    let _ = std::fs::remove_dir_all(&root);
}
