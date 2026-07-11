//! PROOF-02 -- the scripted CLI end-to-end proof harness (gated, live-only).
//!
//! Drives the REAL compiled `rdpilot` CLI binary as a black-box subprocess
//! against a REAL remote Windows target (`.secrets/connection.json`), with
//! the auto-started `rdpilot-daemon`'s Phase 13 fake connector
//! (`RDPILOT_DAEMON_TEST_CONNECTOR`) deliberately UNSET so the daemon opens
//! a genuine RDP session (D-15.1 full fidelity) -- the OPPOSITE of
//! `cli_lifecycle.rs`/`cli_verbs.rs`/`cli_errors.rs`'s offline canned-fake
//! mode. This is a PERMANENT, re-runnable artifact proving the CLI surface
//! exactly as a real consumer sees it: args in, exit codes + stdout out,
//! over live IPC to a live target.
//!
//! Exercises the full lifecycle end to end -- connect -> screenshot ->
//! launch the v1.0 target program (7-Zip File Manager, `class_name`
//! `"7-Zip::FM"`, matching the Phase 9 proof for continuity) -> window-list
//! assertion -> put -> get (checksum round trip) -> disconnect -- then
//! asserts D-28's distinct-non-zero-exit-code taxonomy end to end via one
//! deliberately-broken call.
//!
//! Prints a step-by-step `PASS`/`FAIL` trace to stdout, D-9.4 style
//! (mirroring `crates/rdpilot/examples/proof_harness.rs` and its shared
//! `tests/support/proof_harness.rs` module), and a final `PROOF: PASS`/
//! `PROOF: FAIL` line.
//!
//! # Gating (D-18)
//!
//! `#[ignore]`-gated, and additionally early-returns cleanly (never a hard
//! failure) unless BOTH:
//! 1. `RDPILOT_LIVE` is set (the explicit opt-in), and
//! 2. `.secrets/connection.json` exists and parses.
//!
//! `RDPILOT_LIVE=1 cargo test -p rdpilot-cli --test live_proof -- --ignored`
//! runs it against a provisioned VM. Plain `cargo test -p rdpilot-cli`
//! lists it (`--list`) but never executes it -- nothing here runs offline
//! or in CI.
//!
//! # Why `tests/*.rs`, not `--example` (research binding direction 3b)
//!
//! `env!("CARGO_BIN_EXE_rdpilot")` -- the mechanism used below to locate the
//! compiled `rdpilot` binary -- is only populated by Cargo for `tests/`/
//! `benches/` targets, never for `--example` binaries (confirmed against the
//! Cargo Book, 15-RESEARCH.md Pitfall 1). Since PROOF-02 must spawn the REAL
//! compiled CLI binary as a subprocess (not link the SDK as a library, unlike
//! `examples/proof_harness.rs`'s PROOF-01), it lives here as a gated
//! integration test, mirroring `tests/cli_lifecycle.rs`'s already-proven
//! pattern.
//!
//! # Thin-client invariant (D-17)
//!
//! `rdpilot-cli` never depends on `rdpilot`/`rdpilot-daemon` -- so the
//! `.secrets/connection.json` loader below is a small, deliberate local
//! re-implementation of `crates/rdpilot/tests/common/mod.rs::load_config`'s
//! schema/gating convention, not a shared import.
//!
//! # Security
//!
//! Reads the gitignored `.secrets/connection.json` and hands the credentials
//! to the CLI subprocess as `--host`/`--username`/`--password`/`--port`
//! flags (same layered-config override mechanism `cli_lifecycle.rs` already
//! exercises with its fake-target credentials, D-27). The password is NEVER
//! printed by this harness -- it is read into a local, handed straight to
//! `Command::args`, and never included in any `println!`/`format!`/`panic!`
//! message here (T-15-04). Passing it on the child process's command line is
//! an existing, already-documented risk of the CLI's own design
//! (`config_flags.rs`'s `ConfigFlags::password` doc, T-13-13), not something
//! this harness introduces.
//!
//! **Stdio pitfall (test-harness-only, not a production bug), same as
//! `cli_lifecycle.rs`:** `run_cli` below redirects the CLI subprocess's
//! stdout/stderr to real files rather than piped `output()` -- the
//! auto-started daemon grandchild inherits the pipe and is never `.wait()`-
//! ed, so a piped `output()`/`wait_with_output()` read would hang until the
//! long-lived daemon itself exits.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

/// Name of the opt-in env var that arms this live suite (D-18), mirroring
/// `crates/rdpilot/tests/common/mod.rs::LIVE_ENV` -- that module is not
/// reachable from here (`rdpilot-cli` never depends on `rdpilot`, D-17), so
/// the name/convention is duplicated, not imported.
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// A live connection target, loaded from `.secrets/connection.json` at the
/// workspace root. Deliberately has NO `Debug`/`Display` impl -- nothing in
/// this file is allowed to accidentally format the whole struct (and thus
/// the password) into a message.
struct LiveTarget {
    host: String,
    user: String,
    password: String,
    port: u16,
}

/// Locate `.secrets/connection.json` relative to the workspace root.
/// `CARGO_MANIFEST_DIR` points at `crates/rdpilot-cli`; the secrets file
/// lives two levels up, exactly as `crates/rdpilot/tests/common/mod.rs`
/// resolves it from `crates/rdpilot`.
fn connection_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("connection.json")
}

/// Locate the byte-verified sensor executable relayed by Plan 15-05
/// (`.secrets/sensor-build/rdpilot-sensor.exe`), same two-levels-up
/// resolution as [`connection_file`]. `run_cli` points the auto-started
/// daemon's `RDPILOT_SENSOR_BINARY_PATH` at this path -- **live-diagnosed
/// (Plan 15-06, mirrored here Plan 15-07):** without it, the daemon's real
/// `Connect` never deploys a sensor at all (see `dispatch.rs`'s
/// `resolve_sensor_binary_path` fix from 15-06), so every sensor-backed step
/// this file exercises (screenshot, launch, window list, put/get) would
/// otherwise fail.
fn sensor_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("sensor-build").join("rdpilot-sensor.exe")
}

/// Load the live target, or `None` if the suite is not armed (D-18 gate:
/// `RDPILOT_LIVE` unset, or the secrets file absent). A present-but-
/// malformed file panics with a descriptive message that never includes the
/// password (mirrors `crates/rdpilot/tests/common/mod.rs::load_config`).
fn load_live_target() -> Option<LiveTarget> {
    std::env::var_os(LIVE_ENV)?;

    let path = connection_file();
    if !path.exists() {
        return None;
    }

    let raw = std::fs::read_to_string(&path)
        .expect("RDPILOT_LIVE is set and .secrets/connection.json exists but could not be read");
    let json: serde_json::Value =
        serde_json::from_str(&raw).expect(".secrets/connection.json is not valid JSON");

    let host = json
        .get("host")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `host`")
        .to_owned();
    let user = json
        .get("user")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `user`")
        .to_owned();
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `password`")
        .to_owned();
    let port: u16 = json
        .get("rdpPort")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(3389);

    Some(LiveTarget { host, user, password, port })
}

/// A unique temp root per test invocation -- isolates `XDG_RUNTIME_DIR` (the
/// daemon's socket directory) so this test never collides with a real
/// daemon or another concurrent test run.
fn unique_temp_root() -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    std::env::temp_dir().join(format!("rdpilot-cli-live-proof-{}-{nanos}", std::process::id()))
}

/// One CLI subprocess invocation's captured result.
struct CliRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run the compiled `rdpilot` CLI binary as a subprocess with `args`,
/// isolated to `xdg_runtime_dir`/`sink_path`. `RDPILOT_DAEMON_TEST_CONNECTOR`
/// is deliberately UNSET -- the opposite of the offline suites -- so the
/// auto-started daemon drives a REAL RDP session against whatever target the
/// caller's `connect --host/--username/--password` args name.
///
/// `capture_dir`/`call_index` name this invocation's stdout/stderr capture
/// files uniquely -- real files, not piped `output()` (see the module doc's
/// stdio pitfall).
fn run_cli(args: &[&str], xdg_runtime_dir: &Path, sink_path: &Path, capture_dir: &Path, call_index: usize) -> CliRun {
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
    let daemon_bin = bin.with_file_name(if cfg!(windows) { "rdpilot-daemon.exe" } else { "rdpilot-daemon" });
    assert!(
        daemon_bin.exists(),
        "expected the rdpilot-daemon binary at {daemon_bin:?} -- run `cargo build --workspace` \
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
        // Live-diagnosed (Plan 15-06, mirrored here): the daemon's real
        // Connect handler only deploys a sensor when this is set -- required
        // for screenshot/launch/window-list/put/get to work against a real
        // target.
        .env("RDPILOT_SENSOR_BINARY_PATH", sensor_binary_path())
        // Long enough for a real RDP handshake/session-establishment
        // round trip plus a full CLI-driven proof sequence to complete
        // without the idle reaper or empty-registry grace period firing.
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "180000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000")
        .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "250")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        // `.status()`, never `.output()`/`.wait_with_output()` -- see the
        // module doc's stdio pitfall.
        .status()
        .unwrap_or_else(|e| panic!("spawning the compiled rdpilot binary must succeed: {e}"));

    CliRun {
        status,
        stdout: std::fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

/// Record one D-9.4-style `PASS`/`FAIL` step: print it immediately (so a
/// long live run streams progress) and append it to `steps` for the final
/// summary/assert.
fn record(steps: &mut Vec<(String, bool, String)>, name: &str, passed: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("  [{}] {name}: {detail}", if passed { "PASS" } else { "FAIL" });
    steps.push((name.to_owned(), passed, detail));
}

#[test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]
fn cli_end_to_end_against_a_real_target() {
    let Some(target) = load_live_target() else {
        eprintln!("skipping PROOF-02: RDPILOT_LIVE unset or .secrets/connection.json absent");
        return;
    };

    let root = unique_temp_root();
    let xdg_runtime_dir = root.join("xdg-runtime");
    let capture_dir = root.join("capture");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    std::fs::create_dir_all(&capture_dir).expect("create the stdio capture dir");
    let sink_path = root.join("sessions.json");

    let mut call_index = 0_usize;
    let mut next_call = || {
        call_index += 1;
        call_index
    };
    let mut steps: Vec<(String, bool, String)> = Vec::new();
    let session = "proof-cli";

    println!("=== rdpilot PROOF-02 CLI end-to-end proof harness ===");

    // --- Step 1: connect --name proof-cli (D-29 explicit caller-supplied name) ---
    let port_arg = target.port.to_string();
    let connect_run = run_cli(
        &[
            "connect",
            "--name",
            session,
            "--host",
            &target.host,
            "--port",
            &port_arg,
            "--username",
            &target.user,
            "--password",
            &target.password,
            // The disposable lab VM uses a self-signed cert (D-15,
            // test-only risk-named passthrough -- mirrors
            // `crates/rdpilot/tests/common/mod.rs::load_config`).
            "--accept-invalid-certs",
            "--json",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let connect_ok = connect_run.status.success();
    record(
        &mut steps,
        "connect",
        connect_ok,
        if connect_ok {
            format!("connected as session {session:?}")
        } else {
            format!("connect failed (exit {:?}); stderr={}", connect_run.status.code(), connect_run.stderr)
        },
    );
    assert!(
        connect_ok,
        "PROOF-02 cannot continue past a failed connect; stderr={}",
        connect_run.stderr
    );

    // --- Step 2: perceive screenshot -> a nonzero-size PNG file ---
    let screenshot_path = root.join("proof-screenshot.png");
    let screenshot_run = run_cli(
        &["perceive", "screenshot", "--session", session, "--output", screenshot_path.to_str().expect("utf8 path")],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let screenshot_len = std::fs::metadata(&screenshot_path).map(|m| m.len()).unwrap_or(0);
    let screenshot_ok = screenshot_run.status.success() && screenshot_len > 0;
    record(
        &mut steps,
        "perceive screenshot",
        screenshot_ok,
        if screenshot_ok {
            format!("wrote {screenshot_len} bytes to {screenshot_path:?}")
        } else {
            format!("screenshot failed or was empty; stderr={}", screenshot_run.stderr)
        },
    );

    // --- Step 3: input launch the v1.0 target program (7-Zip File Manager) ---
    let launch_run = run_cli(
        &["input", "launch", "--session", session, "--exe", "C:\\Program Files\\7-Zip\\7zFM.exe"],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let launch_ok = launch_run.status.success();
    record(
        &mut steps,
        "input launch 7zFM.exe",
        launch_ok,
        if launch_ok {
            "launched 7-Zip File Manager".to_owned()
        } else {
            format!("launch failed; stderr={}", launch_run.stderr)
        },
    );

    // --- Step 4: perceive window list -> a "7-Zip::FM" class_name entry ---
    // Polled: the launched process needs a moment to create its window.
    let mut found_7zip_fm = false;
    let mut last_window_list_detail = String::new();
    for attempt in 0..20 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        let window_list_run = run_cli(
            &["--json", "perceive", "window", "list", "--session", session],
            &xdg_runtime_dir,
            &sink_path,
            &capture_dir,
            next_call(),
        );
        if !window_list_run.status.success() {
            last_window_list_detail = format!("window list failed; stderr={}", window_list_run.stderr);
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(window_list_run.stdout.trim()) {
            Ok(windows) => {
                let classes: Vec<String> = windows
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|w| w["class_name"].as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                if classes.iter().any(|c| c == "7-Zip::FM") {
                    found_7zip_fm = true;
                    break;
                }
                last_window_list_detail = format!("no 7-Zip::FM window yet; observed classes={classes:?}");
            }
            Err(e) => {
                last_window_list_detail = format!("window list --json did not parse: {e}");
            }
        }
    }
    record(
        &mut steps,
        "perceive window list (7-Zip::FM)",
        found_7zip_fm,
        if found_7zip_fm {
            "found a window with class_name \"7-Zip::FM\"".to_owned()
        } else {
            format!("never observed a 7-Zip::FM window after polling; {last_window_list_detail}")
        },
    );

    // --- Step 5: put a local file to the remote transfer root ---
    let local_upload = root.join("proof-upload.bin");
    std::fs::write(&local_upload, b"rdpilot PROOF-02 payload\n").expect("seed the local upload file");
    let put_run = run_cli(
        &[
            "put",
            "--session",
            session,
            "--local",
            local_upload.to_str().expect("utf8 path"),
            "--remote-name",
            "rdpilot-proof-02.bin",
            "--json",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let put_ok = put_run.status.success();
    let put_checksum = if put_ok {
        serde_json::from_str::<serde_json::Value>(put_run.stdout.trim())
            .ok()
            .and_then(|v| v["checksum"].as_str().map(str::to_owned))
    } else {
        None
    };
    record(
        &mut steps,
        "put",
        put_ok && put_checksum.is_some(),
        match &put_checksum {
            Some(cs) => format!("uploaded rdpilot-proof-02.bin, checksum={cs}"),
            None => format!("put failed or emitted no --json checksum; stdout={} stderr={}", put_run.stdout, put_run.stderr),
        },
    );

    // --- Step 6: get it back -> the checksum round-trips exactly ---
    let local_download = root.join("proof-download.bin");
    let get_run = run_cli(
        &[
            "get",
            "--session",
            session,
            "--remote-name",
            "rdpilot-proof-02.bin",
            "--local",
            local_download.to_str().expect("utf8 path"),
            "--json",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let get_ok = get_run.status.success();
    let get_checksum = if get_ok {
        serde_json::from_str::<serde_json::Value>(get_run.stdout.trim())
            .ok()
            .and_then(|v| v["checksum"].as_str().map(str::to_owned))
    } else {
        None
    };
    let checksums_match = put_checksum.is_some() && put_checksum == get_checksum;
    record(
        &mut steps,
        "get (checksum round trip)",
        get_ok && checksums_match,
        if checksums_match {
            format!("checksum matched put's: {}", get_checksum.as_deref().unwrap_or(""))
        } else {
            format!(
                "checksum mismatch or get failed: put={put_checksum:?} get={get_checksum:?}; stderr={}",
                get_run.stderr
            )
        },
    );

    // --- Step 7: disconnect ---
    let disconnect_run = run_cli(&["disconnect", "--session", session], &xdg_runtime_dir, &sink_path, &capture_dir, next_call());
    let disconnect_ok = disconnect_run.status.success();
    record(
        &mut steps,
        "disconnect",
        disconnect_ok,
        if disconnect_ok {
            format!("disconnected session {session:?}")
        } else {
            format!("disconnect failed; stderr={}", disconnect_run.stderr)
        },
    );

    // --- D-28: a deliberately-broken call must exit with the DISTINCT
    // non-zero code mapped from session-not-found (2), never a generic 1
    // (per cli_errors.rs's code map / exit_codes.rs's code_for). Points at
    // a local destination that does NOT exist so the CLI-side no-clobber
    // check (exit 8) never short-circuits this assertion.
    let broken_dest = root.join("proof-broken-get.bin");
    let broken_run = run_cli(
        &[
            "get",
            "--session",
            "does-not-exist",
            "--remote-name",
            "rdpilot-proof-02.bin",
            "--local",
            broken_dest.to_str().expect("utf8 path"),
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let distinct_exit_ok = !broken_run.status.success() && broken_run.status.code() == Some(2);
    record(
        &mut steps,
        "D-28 distinct exit code (get against an unknown session)",
        distinct_exit_ok,
        format!(
            "exit code {:?} (expected Some(2), the session-not-found class, never a generic 1); stderr={}",
            broken_run.status.code(),
            broken_run.stderr
        ),
    );

    let all_passed = steps.iter().all(|(_, passed, _)| *passed);
    println!("PROOF: {}", if all_passed { "PASS" } else { "FAIL" });

    let _ = std::fs::remove_dir_all(&root);

    assert!(all_passed, "one or more PROOF-02 steps failed -- see the PASS/FAIL trace above");
}
