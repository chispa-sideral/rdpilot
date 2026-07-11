//! Linux-hostable live-gated orphan-liveness + connect/list/disconnect e2e
//! assertions against a real remote Windows target (Plan 15-01, closing
//! `12-07-PLAN.md`'s live half of DAEMON-04 and SESSION-01/03/04).
//!
//! This is the Linux-hostable split half (binding directions 2/3 of
//! `15-RESEARCH.md`'s recommended `12-07-PLAN.md` file split): it drives
//! the REAL compiled `rdpilot-daemon` binary (`env!("CARGO_BIN_EXE_rdpilot-daemon")`,
//! mirroring `tests/autostart_lifecycle.rs`'s own real-binary pattern) over
//! its real Unix IPC transport (already offline-proven) against the real
//! remote Windows VM -- no Windows build host needed, only network
//! reachability to the target `.secrets/connection.json` names.
//!
//! Every test is `#[ignore]` + `RDPILOT_LIVE`-gated (D-18), loading the
//! live target the same way `crates/rdpilot/tests/common/mod.rs` does
//! (duplicated locally -- that module is test-only code private to the
//! `rdpilot` crate and unreachable from this crate's own `tests/`
//! compilation unit, mirroring how `tests/ipc_security.rs`/
//! `tests/autostart_lifecycle.rs` already duplicate their own small
//! test-local helpers rather than depending on a sibling crate's private
//! test support). Absent `RDPILOT_LIVE` or `.secrets/connection.json`, both
//! tests print `[SKIP]` and return cleanly -- nothing here runs during
//! offline CI. Real execution happens from the Linux host against the
//! Azure VM in Plan 15-06.
//!
//! **12-07-PLAN.md must_haves covered by this file:**
//! - "After kill -9 mid-session + restart against a live target, the
//!   daemon surfaces the possibly-still-live remote session as Orphaned
//!   and reconciles it explicitly" -- `kill_minus_9_mid_session_then_restart_surfaces_the_orphan`.
//! - "connect / list / disconnect work end-to-end against a real Windows
//!   RDP target" -- `connect_list_disconnect_e2e_against_a_real_target`.

// Unix-only: this file drives the daemon over a real `tokio::net::UnixStream`
// directly (module doc: "Linux-hostable" -- runs from the Linux
// orchestration host against the remote Windows VM target, never ON
// Windows itself). Live-VM-confirmed compile fix, Plan 15-05: this
// file never had a cfg gate before, which broke
// `cargo test -p rdpilot-daemon` on a Windows target.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use rdpilot_ipc::{Request, SessionLifecycle, WireResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Name of the opt-in env var that arms this live suite (D-18) -- mirrors
/// `crates/rdpilot/tests/common/mod.rs::LIVE_ENV`.
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// A real live RDP target, loaded from `.secrets/connection.json` -- the
/// non-secret fields (`host`/`port`) plus the credential fields
/// (`user`/`password`), which this file never logs (mirrors
/// `crates/rdpilot/tests/common/mod.rs::load_config`'s own D-31 discipline:
/// the password is read into a local only to be handed straight to the
/// `Request::Connect` frame, never printed/formatted).
struct LiveTarget {
    host: String,
    port: u16,
    user: String,
    password: String,
}

/// Locate `.secrets/connection.json` relative to the workspace root.
/// `CARGO_MANIFEST_DIR` points at `crates/rdpilot-daemon`, the same depth
/// below the workspace root as `crates/rdpilot`
/// (`tests/common/mod.rs::connection_file`'s own two-levels-up resolution).
fn connection_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("connection.json")
}

/// Load the live target, or `None` if the suite is not armed (D-18 gate 1:
/// `RDPILOT_LIVE` unset; gate 2: `.secrets/connection.json` absent). A
/// present-but-malformed file panics with a descriptive message -- a
/// deliberate armed-run misconfiguration is a hard error, not a silent
/// skip -- but the panic message never includes the password (mirrors
/// `tests/common/mod.rs::load_config` exactly).
fn load_live_target() -> Option<LiveTarget> {
    std::env::var_os(LIVE_ENV)?;
    let path = connection_file();
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(&path).expect("RDPILOT_LIVE is set and .secrets/connection.json exists but could not be read");
    let json: serde_json::Value = serde_json::from_str(&raw).expect(".secrets/connection.json is not valid JSON");
    let host = json.get("host").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `host`").to_owned();
    let user = json.get("user").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `user`").to_owned();
    let password =
        json.get("password").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `password`").to_owned();
    let port: u16 = json.get("rdpPort").and_then(serde_json::Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(3389);
    Some(LiveTarget { host, port, user, password })
}

/// Mirrors `ipc::framing`'s wire format (a 4-byte big-endian `u32` length
/// prefix + that many bytes of UTF-8 JSON), duplicated locally exactly as
/// `tests/autostart_lifecycle.rs` already does -- `framing` is a private
/// implementation detail of `ipc::serve_connection`.
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

async fn write_frame<T: serde::Serialize>(stream: &mut UnixStream, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).expect("test request/response must serialize");
    let len = u32::try_from(body.len()).expect("test frame body fits in u32");
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> std::io::Result<T> {
    let mut len_buf = [0_u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    assert!(len <= MAX_FRAME_LEN, "test frame length {len} exceeds the sanity cap");
    let mut body = vec![0_u8; len as usize];
    stream.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).expect("test response must deserialize"))
}

/// A unique temp root per test invocation -- isolates `XDG_RUNTIME_DIR`
/// (the daemon's socket directory) and the reconciliation-state sink path,
/// mirroring `tests/autostart_lifecycle.rs::unique_temp_root`.
fn unique_temp_root(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock should be after the Unix epoch").as_nanos();
    std::env::temp_dir().join(format!("rdpilot-daemon-live-e2e-{label}-{}-{nanos}", std::process::id()))
}

/// Connect to the daemon socket with a bounded retry backoff -- spawning a
/// real OS process and waiting for it to bind is inherently racy; this
/// mirrors `rdpilot_ipc::transport::connect_or_spawn`'s own backoff
/// sequence but keeps the spawned `Child` handle (needed to `kill -9` it
/// later), which `connect_or_spawn` itself does not expose.
async fn connect_with_backoff(socket_path: &std::path::Path) -> UnixStream {
    const BACKOFF_MS: &[u64] = &[50, 100, 200, 400, 800, 1600];
    if let Ok(stream) = UnixStream::connect(socket_path).await {
        return stream;
    }
    for backoff in BACKOFF_MS {
        tokio::time::sleep(Duration::from_millis(*backoff)).await;
        if let Ok(stream) = UnixStream::connect(socket_path).await {
            return stream;
        }
    }
    panic!("daemon at {} did not become reachable within the bounded backoff", socket_path.display());
}

/// Spawn the real compiled `rdpilot-daemon` binary, isolated to a fresh
/// `XDG_RUNTIME_DIR` + reconciliation sink path -- returns the child
/// process handle (for a later `kill -9`) and the socket path it will
/// bind. Deliberately does NOT set `RDPILOT_DAEMON_TEST_CONNECTOR`: this
/// suite needs the REAL `RealConnector` (a genuine RDP connect to the live
/// target), unlike `tests/autostart_lifecycle.rs`'s offline fake-connector
/// run.
fn spawn_real_daemon(root: &std::path::Path) -> (std::process::Child, PathBuf) {
    let xdg_runtime_dir = root.join("xdg-runtime");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    let sink_path = root.join("sessions.json");
    let socket_path = xdg_runtime_dir.join("rdpilot").join("daemon.sock");

    let daemon_exe = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-daemon"));
    let child = std::process::Command::new(&daemon_exe)
        .env("XDG_RUNTIME_DIR", &xdg_runtime_dir)
        .env("RDPILOT_DAEMON_SINK_PATH", &sink_path)
        // Long idle timeout -- this suite's own Connect/List/Disconnect
        // round trip and the deliberate kill-9 gap must never be reaped
        // mid-test by the idle reaper.
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "600000")
        .spawn()
        .expect("failed to spawn the real rdpilot-daemon binary");

    (child, socket_path)
}

/// (b) SESSION-01/03/04: connect / list / disconnect succeed end-to-end
/// against the real target, with correct name/id/status/timestamps.
#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1, .secrets/connection.json, and network reachability to the live target"]
async fn connect_list_disconnect_e2e_against_a_real_target() {
    let name = "connect_list_disconnect_e2e_against_a_real_target";
    let Some(target) = load_live_target() else {
        println!("[SKIP] {name}: {LIVE_ENV} unset or .secrets/connection.json absent");
        return;
    };

    let root = unique_temp_root("connect-list-disconnect");
    let (mut child, socket_path) = spawn_real_daemon(&root);
    let mut stream = connect_with_backoff(&socket_path).await;
    println!("[PASS] {name}: daemon reachable at {}", socket_path.display());

    // --- Connect (explicit session id, D-29) ---
    write_frame(
        &mut stream,
        &Request::Connect {
            name: Some("live-e2e".to_owned()),
            host: target.host.clone(),
            port: Some(target.port),
            username: target.user.clone(),
            password: target.password.clone(),
            domain: None,
            accept_invalid_certs: true, // lab VM self-signed cert (D-15) -- test-only opt-out
        },
    )
    .await
    .expect("write Connect frame");
    let session = match read_frame::<WireResponse>(&mut stream).await.expect("read Connect response") {
        WireResponse::Connected { session } => session,
        other => panic!("[FAIL] {name}: expected Connected, got {other:?}"),
    };
    println!("[PASS] {name}: connected, session id = {}", session.as_str());

    // --- List -> exactly one Live session with the right host/name ---
    write_frame(&mut stream, &Request::List {}).await.expect("write List frame");
    match read_frame::<WireResponse>(&mut stream).await.expect("read List response") {
        WireResponse::SessionList { sessions } => {
            assert_eq!(sessions.len(), 1, "[FAIL] {name}: exactly one session should be listed after Connect");
            let s = &sessions[0];
            assert_eq!(s.id, session.as_str());
            assert_eq!(s.name.as_deref(), Some("live-e2e"));
            assert_eq!(s.host, target.host);
            assert_eq!(s.status, SessionLifecycle::Live, "[FAIL] {name}: a freshly connected session must be Live");
            assert!(s.connected_since.is_some(), "[FAIL] {name}: connected_since must be populated");
            println!("[PASS] {name}: list shows exactly one Live session with correct name/host/timestamps");
        }
        other => panic!("[FAIL] {name}: expected SessionList, got {other:?}"),
    }

    // --- Disconnect -> Ack ---
    write_frame(&mut stream, &Request::Disconnect { session }).await.expect("write Disconnect frame");
    match read_frame::<WireResponse>(&mut stream).await.expect("read Disconnect response") {
        WireResponse::Ack => println!("[PASS] {name}: disconnect acknowledged"),
        other => panic!("[FAIL] {name}: expected Ack, got {other:?}"),
    }

    drop(stream);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

/// (a) DAEMON-04 live: after `kill -9` mid-session against a real target
/// and a restart, the daemon surfaces the possibly-still-live remote
/// session as `Orphaned` (never silently forgotten, never auto-killed --
/// D-31) and an explicit disconnect reconciles it.
///
/// Deliberately NOT proven here (inherently outside what an automated Rust
/// assertion can establish): that the remote Windows RDP session is
/// GENUINELY still logged in server-side at the moment of the kill (vs.
/// having already logged off on its own) -- that is a human/live-run-time
/// observation. What IS proven automatically, against the real compiled
/// binary and a real target: the local daemon-side bookkeeping never
/// forgets a session it did not get to gracefully close, surfaces it
/// distinctly as `Orphaned` with the correct host preserved, and only ever
/// clears it via an explicit reconcile -- exactly DAEMON-04's contract.
#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1, .secrets/connection.json, and network reachability to the live target"]
async fn kill_minus_9_mid_session_then_restart_surfaces_the_orphan_which_is_then_explicitly_reconciled() {
    let name = "kill_minus_9_mid_session_then_restart_surfaces_the_orphan_which_is_then_explicitly_reconciled";
    let Some(target) = load_live_target() else {
        println!("[SKIP] {name}: {LIVE_ENV} unset or .secrets/connection.json absent");
        return;
    };

    let root = unique_temp_root("kill9-orphan");

    // --- Phase A: daemon A connects to the real target. ---
    let (mut child_a, socket_path) = spawn_real_daemon(&root);
    let mut stream_a = connect_with_backoff(&socket_path).await;

    write_frame(
        &mut stream_a,
        &Request::Connect {
            name: Some("live-orphan".to_owned()),
            host: target.host.clone(),
            port: Some(target.port),
            username: target.user.clone(),
            password: target.password.clone(),
            domain: None,
            accept_invalid_certs: true,
        },
    )
    .await
    .expect("write Connect frame");
    let session = match read_frame::<WireResponse>(&mut stream_a).await.expect("read Connect response") {
        WireResponse::Connected { session } => session,
        other => panic!("[FAIL] {name}: expected Connected, got {other:?}"),
    };
    println!("[PASS] {name}: daemon A connected to the real target, session id = {}", session.as_str());

    // --- Phase B: kill -9 the daemon process mid-session (no graceful
    // teardown -- Disconnect is never sent, mirroring a genuine crash). ---
    drop(stream_a);
    child_a.kill().expect("kill -9 the daemon process");
    child_a.wait().expect("reap the killed daemon process");
    println!("[PASS] {name}: daemon A killed (SIGKILL) mid-session -- no graceful teardown ran");

    // --- Phase C: restart -- daemon B, SAME root (`spawn_real_daemon`
    // resolves the identical XDG_RUNTIME_DIR/sink path from `root` every
    // time it is called), so daemon B resolves the SAME reconciliation-
    // state sink file daemon A wrote to -- the whole point of this test is
    // restart-on-the-same-state, not a fresh one.
    let (mut child_b, socket_path_b) = spawn_real_daemon(&root);
    assert_eq!(socket_path_b, socket_path, "daemon B must resolve the identical socket path as daemon A (same root)");
    let mut stream_b = connect_with_backoff(&socket_path).await;
    println!("[PASS] {name}: daemon B restarted and reachable at the same socket path");

    // --- Anti-pattern guard (DAEMON-04's exact failure mode): list must
    // surface the orphan, never silently forget it. ---
    write_frame(&mut stream_b, &Request::List {}).await.expect("write List frame");
    match read_frame::<WireResponse>(&mut stream_b).await.expect("read List response") {
        WireResponse::SessionList { sessions } => {
            assert!(!sessions.is_empty(), "[FAIL] {name}: the orphan must be surfaced after restart, never silently forgotten (DAEMON-04)");
            assert_eq!(sessions.len(), 1);
            let s = &sessions[0];
            assert_eq!(s.id, session.as_str());
            assert_eq!(s.host, target.host, "[FAIL] {name}: the orphan's host must be preserved across the restart");
            assert_eq!(
                s.status,
                SessionLifecycle::Orphaned,
                "[FAIL] {name}: a leftover reconciliation record must surface as Orphaned, not Live/Connecting -- \
                 it is never blindly reconnected or auto-killed (D-31)"
            );
            println!("[PASS] {name}: the possibly-still-live remote session surfaced as Orphaned with the correct host, never forgotten");
        }
        other => panic!("[FAIL] {name}: expected SessionList, got {other:?}"),
    }

    // --- Phase D: explicit reconcile -- Disconnect on the surfaced orphan. ---
    write_frame(&mut stream_b, &Request::Disconnect { session }).await.expect("write Disconnect frame");
    match read_frame::<WireResponse>(&mut stream_b).await.expect("read Disconnect response") {
        WireResponse::Ack => println!("[PASS] {name}: explicit disconnect reconciled the orphan"),
        other => panic!("[FAIL] {name}: expected Ack, got {other:?}"),
    }

    write_frame(&mut stream_b, &Request::List {}).await.expect("write List frame");
    match read_frame::<WireResponse>(&mut stream_b).await.expect("read final List response") {
        WireResponse::SessionList { sessions } => {
            assert!(sessions.is_empty(), "[FAIL] {name}: the orphan must no longer be listed once explicitly reconciled");
            println!("[PASS] {name}: the orphan no longer appears in list after explicit reconciliation");
        }
        other => panic!("[FAIL] {name}: expected SessionList, got {other:?}"),
    }

    drop(stream_b);
    let _ = child_b.kill();
    let _ = child_b.wait();
    let _ = std::fs::remove_dir_all(&root);
}
