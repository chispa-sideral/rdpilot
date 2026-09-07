//! CLI-02 offline integration proof: the full perception + input + launch
//! verb set — `screenshot`, `world-state`, `uia`, `window list`,
//! `process list` (perception); `click`, `type`, `key`, `scroll`, `drag`
//! (input); `launch`, `foreground` — round-trips against the REAL
//! `rdpilot-daemon` binary's canned `FakeTestSession` (`RDPILOT_DAEMON_TEST_CONNECTOR=1`),
//! driven entirely as subprocesses of the compiled `rdpilot` binary.
//!
//! Mirrors `tests/cli_lifecycle.rs`'s isolation pattern (`XDG_RUNTIME_DIR`
//! pointed at a fresh temp dir per test, dedicated stdio capture files
//! rather than piped `output()` — see that module's doc comment for the
//! stdio pitfall this avoids) and its own `<verify>` block
//! (`cargo test -p rdpilot-cli`), never `#[ignore]`-gated: CLI-02 is a
//! first-class must-have for this plan.
//!
//! **What is proven offline, against the real daemon binary's canned
//! `FakeTestSession` (`crates/rdpilot-daemon/src/server.rs`):**
//! - every verb parses its required `--session` and round-trips a
//!   renderable/ack'd result;
//! - `screenshot --output <file>` writes real, non-empty, base64-decoded
//!   PNG-magic-byte file contents (the fake's `to_png()` output, not a
//!   stub);
//! - `--json` emits machine-readable output containing the fake's canned
//!   window/process rows.
//!
//! **What is explicitly deferred to the Phase 15 batched live gate
//! (research Offline-vs-Batched-Live SC#2 row) — REAL Windows semantics,
//! never provable against a canned in-process fake:** actual screenshot
//! pixel content, real click coordinate landing, real UIA tree shape. Any
//! assertion needing those is `#[ignore]`-gated below with a comment
//! pointing here.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// A unique temp root per test invocation — isolates `XDG_RUNTIME_DIR` (the
/// daemon's socket directory) so this test never collides with a real
/// daemon or another concurrent test run.
fn unique_temp_root() -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
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
    let world_state_screenshot_path = root.join("world-state-shot.png");

    let mut call_index = 0_usize;
    let mut next_call = || {
        call_index += 1;
        call_index
    };

    // --- connect (auto-starts the daemon) -> Connected, reserving session id "cli02" ---
    let connect_run = run_cli(
        &["connect", "--name", "cli02", "--host", "10.0.0.5", "--username", "u", "--password", "p", "--json"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(connect_run.status.success(), "connect must exit 0; stdout={} stderr={}", connect_run.stdout, connect_run.stderr);
    let session = "cli02";

    // --- perceive screenshot --output <file> -> a non-empty PNG-magic-byte file, nothing on stdout ---
    let screenshot_run = run_cli(
        &["perceive", "screenshot", "--session", session, "--output", screenshot_path.to_str().expect("utf8 path")],
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
    let png_bytes = std::fs::read(&screenshot_path).expect("screenshot --output must have written a file");
    assert!(!png_bytes.is_empty(), "the written screenshot file must be non-empty");
    assert_eq!(&png_bytes[0..4], &[0x89, b'P', b'N', b'G'], "the written bytes must start with the PNG magic number");
    assert!(
        !screenshot_run.stdout.as_bytes().windows(4).any(|w| w == [0x89, b'P', b'N', b'G']),
        "raw PNG magic bytes must never appear on stdout (D-13.1)"
    );

    // --- perceive window list --json -> the fake's canned window (hwnd 1, "Notepad", pid 1234) ---
    let window_list_run =
        run_cli(&["--json", "perceive", "window", "list", "--session", session], &xdg_runtime_dir, &sink_path, &capture_dir, next_call());
    assert!(
        window_list_run.status.success(),
        "perceive window list must exit 0; stdout={} stderr={}",
        window_list_run.stdout,
        window_list_run.stderr
    );
    let windows: serde_json::Value = serde_json::from_str(window_list_run.stdout.trim()).expect("window list --json must be valid JSON");
    let windows = windows.as_array().expect("window list --json must be a JSON array");
    assert!(
        windows.iter().any(|w| w["hwnd"].as_u64() == Some(1) && w["title"].as_str() == Some("Notepad") && w["pid"].as_u64() == Some(1234)),
        "expected the fake's canned window (hwnd=1, title=Notepad, pid=1234) in {windows:?}"
    );

    // --- perceive process list --json -> the fake's canned process (pid 1234, notepad.exe) ---
    let process_list_run = run_cli(
        &["--json", "perceive", "process", "list", "--session", session],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        process_list_run.status.success(),
        "perceive process list must exit 0; stdout={} stderr={}",
        process_list_run.stdout,
        process_list_run.stderr
    );
    let process_list_response: serde_json::Value =
        serde_json::from_str(process_list_run.stdout.trim()).expect("process list --json must be valid JSON");
    assert!(
        process_list_response["elevation_active"].is_boolean(),
        "process list --json must surface elevation_active: {process_list_response:?}"
    );
    let processes = process_list_response["processes"]
        .as_array()
        .expect("process list --json must carry a processes array");
    assert!(
        processes.iter().any(|p| p["pid"].as_u64() == Some(1234) && p["name"].as_str() == Some("notepad.exe")),
        "expected the fake's canned process (pid=1234, name=notepad.exe) in {processes:?}"
    );

    // --- perceive uia --json -> the fake's canned UIA element (id "42", role Button) ---
    let uia_run = run_cli(
        &["--json", "perceive", "uia", "--session", session, "--hwnd", "1", "--scope", "children"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(uia_run.status.success(), "perceive uia must exit 0; stdout={} stderr={}", uia_run.stdout, uia_run.stderr);
    let elements: serde_json::Value = serde_json::from_str(uia_run.stdout.trim()).expect("uia --json must be valid JSON");
    let elements = elements.as_array().expect("uia --json must be a JSON array");
    assert!(
        elements.iter().any(|e| e["id"].as_str() == Some("42") && e["role"].as_str() == Some("Button")),
        "expected the fake's canned UIA element (id=42, role=Button) in {elements:?}"
    );

    // --- perceive world-state --screenshot --window-list --uia-mode all --output <file> -> all three components present ---
    let world_state_run = run_cli(
        &[
            "--json",
            "perceive",
            "world-state",
            "--session",
            session,
            "--screenshot",
            "--window-list",
            "--uia-mode",
            "all",
            "--output",
            world_state_screenshot_path.to_str().expect("utf8 path"),
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        world_state_run.status.success(),
        "perceive world-state must exit 0; stdout={} stderr={}",
        world_state_run.stdout,
        world_state_run.stderr
    );
    let world_state_bytes = std::fs::read(&world_state_screenshot_path).expect("world-state --output must have written a file");
    assert_eq!(&world_state_bytes[0..4], &[0x89, b'P', b'N', b'G'], "world-state's --output file must start with the PNG magic number");
    let world_state_json: serde_json::Value =
        serde_json::from_str(world_state_run.stdout.trim()).expect("world-state --json must be valid JSON");
    assert!(
        world_state_json["window_list"].as_array().is_some_and(|ws| !ws.is_empty()),
        "world-state --json must include the requested window_list: {world_state_json:?}"
    );

    // --- input click -> Ack ---
    let click_run = run_cli(
        &["input", "click", "--session", session, "--x", "10", "--y", "10"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(click_run.status.success(), "input click must exit 0; stdout={} stderr={}", click_run.stdout, click_run.stderr);

    // --- input scroll -> Ack ---
    let scroll_run = run_cli(
        &["input", "scroll", "--session", session, "--x", "10", "--y", "10", "--dy", "-120"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(scroll_run.status.success(), "input scroll must exit 0; stdout={} stderr={}", scroll_run.stdout, scroll_run.stderr);

    // --- input drag -> Ack ---
    let drag_run = run_cli(
        &[
            "input", "drag", "--session", session, "--from-x", "0", "--from-y", "0", "--to-x", "10", "--to-y", "10",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(drag_run.status.success(), "input drag must exit 0; stdout={} stderr={}", drag_run.stdout, drag_run.stderr);

    // --- input type -> Ack ---
    let type_run = run_cli(
        &["input", "type", "--session", session, "--text", "hello"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(type_run.status.success(), "input type must exit 0; stdout={} stderr={}", type_run.stdout, type_run.stderr);

    // --- input key --combo ctrl,a -> Ack ---
    let key_run = run_cli(
        &["input", "key", "--session", session, "--combo", "ctrl,a"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(key_run.status.success(), "input key must exit 0; stdout={} stderr={}", key_run.stdout, key_run.stderr);

    // --- input key --combo with an unknown key name -> a legible non-zero exit, not a panic (T-13-17) ---
    let bad_key_run = run_cli(
        &["input", "key", "--session", session, "--combo", "not-a-real-key"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(!bad_key_run.status.success(), "an unknown --combo key name must not exit 0");
    assert!(
        bad_key_run.stderr.contains("unknown key name"),
        "expected a legible 'unknown key name' error on stderr, got: {}",
        bad_key_run.stderr
    );

    // --- input launch -> the fake's canned pid (4242) ---
    let launch_run = run_cli(
        &["--json", "input", "launch", "--session", session, "--exe", "notepad.exe"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(launch_run.status.success(), "input launch must exit 0; stdout={} stderr={}", launch_run.stdout, launch_run.stderr);
    let launch_json: serde_json::Value = serde_json::from_str(launch_run.stdout.trim()).expect("launch --json must be valid JSON");
    assert_eq!(launch_json["pid"].as_u64(), Some(4242), "expected the fake's canned launch pid (4242): {launch_json:?}");

    // --- input foreground -> Ack ---
    let foreground_run = run_cli(
        &["input", "foreground", "--session", session, "--hwnd", "1"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    assert!(
        foreground_run.status.success(),
        "input foreground must exit 0; stdout={} stderr={}",
        foreground_run.stdout,
        foreground_run.stderr
    );

    // --- disconnect -> success (registry empties; the daemon's own self-shutdown-on-empty
    // path is proven separately by `rdpilot-daemon/tests/autostart_lifecycle.rs`, not re-tested here) ---
    let disconnect_run = run_cli(&["disconnect", "--session", session], &xdg_runtime_dir, &sink_path, &capture_dir, next_call());
    assert!(
        disconnect_run.status.success(),
        "disconnect must exit 0; stdout={} stderr={}",
        disconnect_run.stdout,
        disconnect_run.stderr
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// REAL Windows semantics — actual pixel content decoded from the screenshot,
/// genuine click-coordinate landing on a live remote desktop, real UIA tree
/// shape from an actual Windows accessibility tree — cannot be proven
/// against the canned in-process `FakeTestSession` (it returns fixed,
/// synthetic data regardless of input). Deferred to the Phase 15 batched
/// live gate (research Offline-vs-Batched-Live SC#2 row), where a real
/// Windows target is available.
#[test]
#[ignore = "requires a real Windows RDP target -- deferred to the Phase 15 batched live gate (see module doc)"]
fn real_windows_semantics_are_proven_in_the_phase_15_live_gate() {
    unreachable!(
        "placeholder for the Phase 15 batched live gate: real screenshot pixel content, real click \
         landing, and real UIA tree shape assertions belong there, never against this crate's offline \
         canned fake session"
    );
}
