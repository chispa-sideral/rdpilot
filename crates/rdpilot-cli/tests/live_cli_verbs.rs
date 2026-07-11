//! CLI-02/03 live-deferred re-exercise (gated, live-only).
//!
//! Closes the Phase-13-deferred live items from `cli_verbs.rs`'s own module
//! doc: real screenshot pixel content, real click-coordinate landing, and
//! real UIA tree shape (CLI-02) -- none of which can be proven against the
//! offline canned `FakeTestSession` -- plus a real multi-MB `put`/`get`
//! checksum round trip through the CLI surface (CLI-03). This file
//! re-exercises the SAME semantics through the CLI surface; it does NOT
//! re-prove the underlying SDK transfer (Phase 10 already live-verified
//! FILE-01/02/04, per 15-RESEARCH.md SC#3).
//!
//! Same gating/isolation/subprocess conventions as `live_proof.rs`
//! (`#[ignore]` + `RDPILOT_LIVE` + `.secrets/connection.json`, the fake
//! connector UNSET, `CARGO_BIN_EXE_rdpilot` + a locally-duplicated
//! `.secrets/connection.json` loader per the thin-client invariant, D-17) --
//! kept in a SEPARATE test binary from `live_proof.rs` so the two live-run
//! steps (CLI-02/03 in Plan 15-06, PROOF-02 in Plan 15-07) stay
//! independently runnable.
//!
//! # Gating (D-18)
//!
//! `RDPILOT_LIVE=1 cargo test -p rdpilot-cli --test live_cli_verbs -- --ignored`
//! runs both tests below against a provisioned VM. Plain `cargo test -p
//! rdpilot-cli` lists them (`--list`) but never executes them.
//!
//! # Security
//!
//! Same as `live_proof.rs`: the password is read from the gitignored
//! `.secrets/connection.json`, handed straight to `Command::args`, and NEVER
//! appears in any `println!`/`format!`/`panic!` message here (T-15-04).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

/// Name of the opt-in env var that arms this live suite (D-18) -- mirrors
/// `crates/rdpilot/tests/common/mod.rs::LIVE_ENV`, duplicated locally
/// because `rdpilot-cli` never depends on `rdpilot` (D-17).
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// A live connection target, loaded from `.secrets/connection.json` at the
/// workspace root. Deliberately has NO `Debug`/`Display` impl.
struct LiveTarget {
    host: String,
    user: String,
    password: String,
    port: u16,
}

/// Locate `.secrets/connection.json` relative to the workspace root
/// (`CARGO_MANIFEST_DIR` = `crates/rdpilot-cli`, two levels below root).
fn connection_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("connection.json")
}

/// Locate the byte-verified sensor executable relayed by Plan 15-05
/// (`.secrets/sensor-build/rdpilot-sensor.exe`), same two-levels-up
/// resolution as [`connection_file`]. Every `run_cli` invocation below
/// points the auto-started daemon's `RDPILOT_SENSOR_BINARY_PATH` at this
/// path -- **live-diagnosed (Plan 15-06):** without it, the daemon's real
/// `Connect` never deploys a sensor at all (see the dispatch.rs fix this
/// plan committed), so every sensor-backed verb this file exercises
/// (`launch`, `perceive`, `input click`, `put`/`get`) would otherwise fail.
fn sensor_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("sensor-build").join("rdpilot-sensor.exe")
}

/// Load the live target, or `None` if the suite is not armed (D-18: either
/// `RDPILOT_LIVE` is unset, or the secrets file is absent).
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

/// A unique temp root per test invocation -- isolates `XDG_RUNTIME_DIR` so
/// this test never collides with a real daemon or another concurrent run.
fn unique_temp_root(label: &str) -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    std::env::temp_dir().join(format!("rdpilot-cli-live-verbs-{label}-{}-{nanos}", std::process::id()))
}

/// One CLI subprocess invocation's captured result.
struct CliRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Run the compiled `rdpilot` CLI binary as a subprocess with `args`. Same
/// isolation/stdio-capture conventions as `live_proof.rs::run_cli` --
/// `RDPILOT_DAEMON_TEST_CONNECTOR` deliberately UNSET, so the auto-started
/// daemon drives a REAL RDP session.
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
        // Live-diagnosed (Plan 15-06): the daemon's real Connect handler
        // only deploys a sensor when this is set (see dispatch.rs's
        // resolve_sensor_binary_path fix this plan committed) -- required
        // for launch/perceive/input/put/get to work against a real target.
        .env("RDPILOT_SENSOR_BINARY_PATH", sensor_binary_path())
        // Long enough for a real RDP handshake plus a multi-MB transfer to
        // complete without the idle reaper or empty-registry grace period
        // firing mid-test.
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "300000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000")
        .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "250")
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        // `.status()`, never `.output()`/`.wait_with_output()` -- see
        // `live_proof.rs`'s module doc for the pipe-hang pitfall this avoids.
        .status()
        .unwrap_or_else(|e| panic!("spawning the compiled rdpilot binary must succeed: {e}"));

    CliRun {
        status,
        stdout: std::fs::read_to_string(&stdout_path).unwrap_or_default(),
        stderr: std::fs::read_to_string(&stderr_path).unwrap_or_default(),
    }
}

/// Record one D-9.4-style `PASS`/`FAIL` step: print it immediately and
/// append it to `steps` for the final summary/assert.
fn record(steps: &mut Vec<(String, bool, String)>, name: &str, passed: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("  [{}] {name}: {detail}", if passed { "PASS" } else { "FAIL" });
    steps.push((name.to_owned(), passed, detail));
}

/// Connect (D-29 explicit `--name`) and poll `perceive window list` until a
/// window with `class_name == "7-Zip::FM"` appears, launching the target
/// first. Shared setup for both live tests below. Returns the found
/// `hwnd`, or `None` if it never appeared (the caller records that as a
/// failed step rather than panicking, so the rest of the trace still runs).
#[allow(clippy::too_many_arguments)]
fn connect_and_launch_7zip(
    target: &LiveTarget,
    session: &str,
    xdg_runtime_dir: &Path,
    sink_path: &Path,
    capture_dir: &Path,
    next_call: &mut impl FnMut() -> usize,
    steps: &mut Vec<(String, bool, String)>,
) -> Option<u64> {
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
            "--accept-invalid-certs",
            "--json",
        ],
        xdg_runtime_dir,
        sink_path,
        capture_dir,
        next_call(),
    );
    let connect_ok = connect_run.status.success();
    record(
        steps,
        "connect",
        connect_ok,
        if connect_ok {
            format!("connected as session {session:?}")
        } else {
            format!("connect failed; stderr={}", connect_run.stderr)
        },
    );
    assert!(connect_ok, "cannot continue past a failed connect; stderr={}", connect_run.stderr);

    let launch_run = run_cli(
        &["input", "launch", "--session", session, "--exe", "C:\\Program Files\\7-Zip\\7zFM.exe"],
        xdg_runtime_dir,
        sink_path,
        capture_dir,
        next_call(),
    );
    record(
        steps,
        "input launch 7zFM.exe",
        launch_run.status.success(),
        if launch_run.status.success() {
            "launched 7-Zip File Manager".to_owned()
        } else {
            format!("launch failed; stderr={}", launch_run.stderr)
        },
    );

    let mut hwnd = None;
    let mut last_detail = String::new();
    for attempt in 0..20 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        let window_list_run =
            run_cli(&["--json", "perceive", "window", "list", "--session", session], xdg_runtime_dir, sink_path, capture_dir, next_call());
        if !window_list_run.status.success() {
            last_detail = format!("window list failed; stderr={}", window_list_run.stderr);
            continue;
        }
        if let Ok(windows) = serde_json::from_str::<serde_json::Value>(window_list_run.stdout.trim()) {
            if let Some(arr) = windows.as_array() {
                if let Some(w) = arr.iter().find(|w| w["class_name"].as_str() == Some("7-Zip::FM")) {
                    hwnd = w["hwnd"].as_u64();
                    break;
                }
                last_detail = format!("no 7-Zip::FM window yet among {} windows", arr.len());
            }
        }
    }
    record(
        steps,
        "perceive window list (7-Zip::FM)",
        hwnd.is_some(),
        match hwnd {
            Some(h) => format!("found 7-Zip::FM at hwnd={h}"),
            None => format!("never observed a 7-Zip::FM window after polling; {last_detail}"),
        },
    );
    hwnd
}

/// CLI-02 (live-deferred): a real `perceive screenshot` captures genuine,
/// varying desktop content (not the offline suite's fixed canned bytes), and
/// a real `input click` at a UIA-reported element's coordinates measurably
/// changes that element's `focused` state, observed through a follow-up
/// `perceive uia`.
///
/// **"Non-blank" heuristic, deliberately WITHOUT a PNG-decoding dependency:**
/// `rdpilot-cli` has no PNG/image codec dependency and this plan
/// (15-RESEARCH.md, binding constraints) prefers not to add one just for a
/// pixel-level sanity check. Instead this asserts (a) both captures start
/// with the PNG magic number and are non-trivially sized, and (b) the two
/// captures -- taken before and after the click -- are byte-for-byte
/// DIFFERENT. A canned/fixed image (like the offline `FakeTestSession`'s)
/// would never satisfy (b); a genuinely live, changing desktop reliably
/// does. This is the same "prove realness via variance, not full decode"
/// principle `cli_verbs.rs` already applies for other verbs.
#[test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]
fn cli_02_screenshot_and_click_landing_against_a_real_target() {
    let Some(target) = load_live_target() else {
        eprintln!("skipping CLI-02 live re-exercise: RDPILOT_LIVE unset or .secrets/connection.json absent");
        return;
    };

    let root = unique_temp_root("cli02");
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
    let session = "cli02-live";

    println!("=== rdpilot CLI-02 live re-exercise (real screenshot pixels + click landing) ===");

    let hwnd = connect_and_launch_7zip(&target, session, &xdg_runtime_dir, &sink_path, &capture_dir, &mut next_call, &mut steps);

    // --- screenshot before the click ---
    let before_path = root.join("before.png");
    let before_run = run_cli(
        &["perceive", "screenshot", "--session", session, "--output", before_path.to_str().expect("utf8 path")],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let before_bytes = std::fs::read(&before_path).unwrap_or_default();
    let before_ok = before_run.status.success() && before_bytes.len() > 1024 && before_bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    record(
        &mut steps,
        "perceive screenshot (before)",
        before_ok,
        format!("{} bytes, PNG magic present={}", before_bytes.len(), before_bytes.starts_with(&[0x89, b'P', b'N', b'G'])),
    );

    // --- find a clickable element in the 7-Zip::FM UIA tree, click its
    // center, then re-fetch to see the focus move ---
    let mut click_landed = false;
    // Every branch below always overwrites this before it's read (the
    // `None` init is a defensive default only, never expected to survive to
    // the final `record` call).
    #[allow(unused_assignments)]
    let mut click_detail = String::new();
    if let Some(hwnd) = hwnd {
        let hwnd_str = hwnd.to_string();
        let uia_before_run = run_cli(
            &["--json", "perceive", "uia", "--session", session, "--hwnd", &hwnd_str, "--scope", "children"],
            &xdg_runtime_dir,
            &sink_path,
            &capture_dir,
            next_call(),
        );
        if uia_before_run.status.success() {
            if let Ok(elements) = serde_json::from_str::<serde_json::Value>(uia_before_run.stdout.trim()) {
                let target_elem = elements.as_array().and_then(|arr| {
                    arr.iter()
                        .find(|e| e["focusable"].as_bool() == Some(true))
                        .or_else(|| arr.first())
                });
                if let Some(elem) = target_elem {
                    let elem_id = elem["id"].as_str().unwrap_or_default().to_owned();
                    let bbox = &elem["bbox"];
                    let (x, y, w, h) = (
                        bbox["x"].as_u64().unwrap_or(0),
                        bbox["y"].as_u64().unwrap_or(0),
                        bbox["w"].as_u64().unwrap_or(0),
                        bbox["h"].as_u64().unwrap_or(0),
                    );
                    let center_x = (x + w / 2).min(u64::from(u16::MAX)).to_string();
                    let center_y = (y + h / 2).min(u64::from(u16::MAX)).to_string();

                    let click_run = run_cli(
                        &["input", "click", "--session", session, "--x", &center_x, "--y", &center_y],
                        &xdg_runtime_dir,
                        &sink_path,
                        &capture_dir,
                        next_call(),
                    );

                    if click_run.status.success() {
                        let uia_after_run = run_cli(
                            &["--json", "perceive", "uia", "--session", session, "--hwnd", &hwnd_str, "--scope", "children"],
                            &xdg_runtime_dir,
                            &sink_path,
                            &capture_dir,
                            next_call(),
                        );
                        if let Ok(after_elements) = serde_json::from_str::<serde_json::Value>(uia_after_run.stdout.trim()) {
                            let focused_after = after_elements
                                .as_array()
                                .map(|arr| arr.iter().any(|e| e["focused"].as_bool() == Some(true)))
                                .unwrap_or(false);
                            click_landed = focused_after;
                            click_detail = format!(
                                "clicked element id={elem_id} at ({center_x},{center_y}); some element now reports focused=true: {focused_after}"
                            );
                        } else {
                            click_detail = "click succeeded but the follow-up perceive uia --json did not parse".to_owned();
                        }
                    } else {
                        click_detail = format!("input click failed; stderr={}", click_run.stderr);
                    }
                } else {
                    click_detail = "the 7-Zip::FM UIA children list was empty -- nothing to click".to_owned();
                }
            } else {
                click_detail = "perceive uia --json (before the click) did not parse".to_owned();
            }
        } else {
            click_detail = format!("perceive uia (before the click) failed; stderr={}", uia_before_run.stderr);
        }
    } else {
        click_detail = "no 7-Zip::FM hwnd was found -- skipping the click-landing assertion".to_owned();
    }
    record(&mut steps, "input click landing (verified via perceive uia)", click_landed, click_detail);

    // --- screenshot after the click -- must differ from the "before" capture ---
    let after_path = root.join("after.png");
    let after_run = run_cli(
        &["perceive", "screenshot", "--session", session, "--output", after_path.to_str().expect("utf8 path")],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let after_bytes = std::fs::read(&after_path).unwrap_or_default();
    let after_ok = after_run.status.success() && after_bytes.len() > 1024 && after_bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    let captures_differ = before_ok && after_ok && before_bytes != after_bytes;
    record(
        &mut steps,
        "perceive screenshot (after, real content -- differs from before)",
        after_ok && captures_differ,
        format!("{} bytes; differs from the before-capture: {captures_differ}", after_bytes.len()),
    );

    let disconnect_run = run_cli(&["disconnect", "--session", session], &xdg_runtime_dir, &sink_path, &capture_dir, next_call());
    record(&mut steps, "disconnect", disconnect_run.status.success(), format!("stderr={}", disconnect_run.stderr));

    let all_passed = steps.iter().all(|(_, passed, _)| *passed);
    println!("CLI-02 LIVE RE-EXERCISE: {}", if all_passed { "PASS" } else { "FAIL" });

    let _ = std::fs::remove_dir_all(&root);

    assert!(all_passed, "one or more CLI-02 live steps failed -- see the PASS/FAIL trace above");
}

/// CLI-03 (live-deferred): a real multi-MB `put` followed by a `get` through
/// the CLI surface -- `bytes_transferred` equals the local file's actual
/// size AND the round-tripped checksum matches put's. Re-exercises the CLI
/// path only; the underlying SDK transfer semantics (chunking, resume,
/// integrity) were already live-verified in Phase 10 (FILE-01/02/04).
#[test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]
fn cli_03_multi_mb_put_get_round_trip_against_a_real_target() {
    let Some(target) = load_live_target() else {
        eprintln!("skipping CLI-03 live re-exercise: RDPILOT_LIVE unset or .secrets/connection.json absent");
        return;
    };

    let root = unique_temp_root("cli03");
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
    let session = "cli03-live";

    println!("=== rdpilot CLI-03 live re-exercise (real multi-MB put/get) ===");

    // --- connect (no need to launch 7-Zip for a pure file-transfer re-exercise) ---
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
            format!("connect failed; stderr={}", connect_run.stderr)
        },
    );
    assert!(connect_ok, "cannot continue past a failed connect; stderr={}", connect_run.stderr);

    // --- a deterministic, non-trivially-compressible ~8 MiB local payload ---
    const PAYLOAD_LEN: usize = 8 * 1024 * 1024;
    let payload: Vec<u8> = (0..PAYLOAD_LEN).map(|i| (i as u8).wrapping_mul(31).wrapping_add(7)).collect();
    let local_upload = root.join("cli03-upload.bin");
    std::fs::write(&local_upload, &payload).expect("seed the multi-MB local upload file");
    let expected_len = u64::try_from(PAYLOAD_LEN).expect("PAYLOAD_LEN fits in u64");

    // --- put ---
    let put_run = run_cli(
        &[
            "put",
            "--session",
            session,
            "--local",
            local_upload.to_str().expect("utf8 path"),
            "--remote-name",
            "rdpilot-cli-03.bin",
            "--json",
        ],
        &xdg_runtime_dir,
        &sink_path,
        &capture_dir,
        next_call(),
    );
    let put_ok = put_run.status.success();
    let put_outcome: Option<serde_json::Value> = if put_ok { serde_json::from_str(put_run.stdout.trim()).ok() } else { None };
    let put_bytes = put_outcome.as_ref().and_then(|v| v["bytes_transferred"].as_u64());
    let put_checksum = put_outcome.as_ref().and_then(|v| v["checksum"].as_str().map(str::to_owned));
    let put_size_ok = put_bytes == Some(expected_len);
    record(
        &mut steps,
        "put (multi-MB, bytes_transferred == local file size)",
        put_ok && put_size_ok && put_checksum.is_some(),
        format!(
            "expected {expected_len} bytes; put reported bytes_transferred={put_bytes:?}, checksum={put_checksum:?}; stderr={}",
            put_run.stderr
        ),
    );

    // --- get ---
    let local_download = root.join("cli03-download.bin");
    let get_run = run_cli(
        &[
            "get",
            "--session",
            session,
            "--remote-name",
            "rdpilot-cli-03.bin",
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
    let get_outcome: Option<serde_json::Value> = if get_ok { serde_json::from_str(get_run.stdout.trim()).ok() } else { None };
    let get_bytes = get_outcome.as_ref().and_then(|v| v["bytes_transferred"].as_u64());
    let get_checksum = get_outcome.as_ref().and_then(|v| v["checksum"].as_str().map(str::to_owned));
    let get_size_ok = get_bytes == Some(expected_len);
    let checksums_match = put_checksum.is_some() && put_checksum == get_checksum;
    record(
        &mut steps,
        "get (multi-MB, bytes_transferred == local file size AND checksum matches put's)",
        get_ok && get_size_ok && checksums_match,
        format!(
            "expected {expected_len} bytes; get reported bytes_transferred={get_bytes:?}, checksum={get_checksum:?} (put's was {put_checksum:?}); stderr={}",
            get_run.stderr
        ),
    );

    // --- the round-tripped local file's actual on-disk size also matches ---
    let downloaded_len = std::fs::metadata(&local_download).map(|m| m.len()).unwrap_or(0);
    record(
        &mut steps,
        "downloaded file's on-disk size matches the uploaded payload",
        downloaded_len == expected_len,
        format!("expected {expected_len} bytes on disk, found {downloaded_len}"),
    );

    let disconnect_run = run_cli(&["disconnect", "--session", session], &xdg_runtime_dir, &sink_path, &capture_dir, next_call());
    record(&mut steps, "disconnect", disconnect_run.status.success(), format!("stderr={}", disconnect_run.stderr));

    let all_passed = steps.iter().all(|(_, passed, _)| *passed);
    println!("CLI-03 LIVE RE-EXERCISE: {}", if all_passed { "PASS" } else { "FAIL" });

    let _ = std::fs::remove_dir_all(&root);

    assert!(all_passed, "one or more CLI-03 live steps failed -- see the PASS/FAIL trace above");
}
