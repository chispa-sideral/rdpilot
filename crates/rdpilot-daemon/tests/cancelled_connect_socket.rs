//! Unix-socket regressions for a cancelled Connect.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rdpilot_ipc::{Request, SessionLifecycle, WireErrorCode, WireResponse};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

async fn write_frame<T: serde::Serialize>(stream: &mut UnixStream, value: &T) {
    rdpilot_ipc::write_frame(stream, value)
        .await
        .expect("write test frame");
}

async fn read_frame(stream: &mut UnixStream) -> WireResponse {
    // Bounded, so a regression fails the test instead of hanging it.
    tokio::time::timeout(Duration::from_secs(5), rdpilot_ipc::read_frame(stream))
        .await
        .expect("a response within 5 s")
        .expect("read test response")
}

fn root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rdp-cancel-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ))
}

fn spawn(root: &Path) -> std::process::Child {
    spawn_with(root, &[])
}

fn spawn_with(root: &Path, extra_env: &[(&str, &str)]) -> std::process::Child {
    let runtime = root.join("run");
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    std::process::Command::new(env!("CARGO_BIN_EXE_rdpilot-daemon"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("RDPILOT_DAEMON_SINK_PATH", root.join("sessions.json"))
        .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
        .env(
            "RDPILOT_DAEMON_TEST_CONNECT_GATE_PATH",
            root.join("connect-gate"),
        )
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "600000")
        .envs(extra_env.iter().copied())
        .spawn()
        .expect("spawn fake daemon")
}

fn socket(root: &Path) -> PathBuf {
    root.join("run").join("rdpilot").join("daemon.sock")
}

fn release_connect(root: &Path) {
    std::fs::write(root.join("connect-gate"), b"release").expect("release fake connector");
}

async fn connect(path: &Path) -> UnixStream {
    for _ in 0..50 {
        if let Ok(stream) = UnixStream::connect(path).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("daemon did not bind {}", path.display());
}

fn request(name: &str) -> Request {
    Request::Connect {
        name: Some(name.to_owned()),
        host: "192.0.2.1".to_owned(),
        port: None,
        username: "test-user".to_owned(),
        password: "test-password".to_owned(),
        domain: None,
        accept_invalid_certs: false,
        cua_enabled: false,
        cua_version: "latest-dev".to_owned(),
        cua_auto_download: true,
        connect_ack: true,
        record: None,
    }
}

fn request_with_cua(name: &str) -> Request {
    match request(name) {
        Request::Connect {
            name,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certs,
            cua_version,
            cua_auto_download,
            connect_ack,
            record,
            ..
        } => Request::Connect {
            name,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certs,
            cua_enabled: true,
            cua_version,
            cua_auto_download,
            connect_ack,
            record,
        },
        other => other,
    }
}

fn stop(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

async fn acknowledge(stream: &mut UnixStream, name: &str) -> rdpilot_ipc::SessionId {
    write_frame(stream, &request(name)).await;
    let session = match read_frame(stream).await {
        WireResponse::Connected {
            session,
            connect_ack_required: true,
            ..
        } => session,
        other => panic!("expected provisional Connected, got {other:?}"),
    };
    write_frame(
        stream,
        &Request::ConnectAck {
            session: session.clone(),
        },
    )
    .await;
    assert!(matches!(read_frame(stream).await, WireResponse::Ack));
    session
}

/// A client that leaves while its connect is still in flight cancels it:
/// nothing is listed or recorded, now or after the gate opens, and a restart
/// finds no orphan. The gate stays closed, so the connect cannot finish
/// before the peer is seen to be gone.
#[tokio::test]
async fn peer_close_during_the_handshake_cancels_the_connect_and_leaves_no_record() {
    let root = root("early");
    let mut daemon = spawn(&root);
    let path = socket(&root);

    let mut cancelled = connect(&path).await;
    write_frame(&mut cancelled, &request("cancelled")).await;
    let mut observer = connect(&path).await;
    wait_until_connecting(&mut observer, "cancelled").await;

    cancelled.shutdown().await.expect("close client write side");
    drop(cancelled);
    wait_until_absent(&mut observer, "cancelled").await;
    assert!(rdpilot_daemon::scan_orphans(&root.join("sessions.json")).is_empty());

    // The connect would finish now, but its attempt is gone.
    release_connect(&root);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(list(&mut observer).await.is_empty());
    assert!(rdpilot_daemon::scan_orphans(&root.join("sessions.json")).is_empty());

    assert_name_free_then_restart_clean(&root, &mut daemon, &mut observer, "cancelled", &[]).await;
    let _ = std::fs::remove_dir_all(root);
}

/// A client that leaves during the bridge bootstrap closes its entry by the
/// connect lease, and an independent Live session is untouched.
#[tokio::test]
async fn peer_close_during_the_bootstrap_closes_the_entry_and_keeps_an_independent_session() {
    let root = root("bootstrap");
    let env = [
        ("RDPILOT_DAEMON_TEST_CUA", "1"),
        ("RDPILOT_DAEMON_TEST_BOOTSTRAP_DELAY_MS", "30000"),
    ];
    let mut daemon = spawn_with(&root, &env);
    let path = socket(&root);
    release_connect(&root);

    let mut control = connect(&path).await;
    let control_id = acknowledge(&mut control, "keep").await;

    let mut leaving = connect(&path).await;
    write_frame(&mut leaving, &request_with_cua("boot")).await;
    let mut observer = connect(&path).await;
    wait_until_live(&mut observer, "boot").await;

    leaving.shutdown().await.expect("close client write side");
    drop(leaving);
    wait_until_absent(&mut observer, "boot").await;

    let sessions = list(&mut observer).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, control_id.as_str());
    assert_eq!(sessions[0].status, SessionLifecycle::Live);
    write_frame(
        &mut observer,
        &Request::Ping {
            session: control_id.clone(),
        },
    )
    .await;
    assert!(matches!(read_frame(&mut observer).await, WireResponse::Ack));
    let records = rdpilot_daemon::scan_orphans(&root.join("sessions.json"));
    assert_eq!(
        records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        [control_id.as_str()]
    );

    write_frame(
        &mut observer,
        &Request::Disconnect {
            session: control_id,
        },
    )
    .await;
    assert!(matches!(read_frame(&mut observer).await, WireResponse::Ack));
    drop(observer);
    drop(control);
    stop(&mut daemon);

    let mut restarted = spawn_with(&root, &env);
    let mut observer = connect(&path).await;
    assert!(list(&mut observer).await.is_empty());
    drop(observer);
    stop(&mut restarted);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn post_write_cancellation_preserves_an_independent_live_control_session() {
    let root = root("control");
    let mut daemon = spawn(&root);
    let path = socket(&root);
    release_connect(&root);

    let mut control = connect(&path).await;
    let control_id = acknowledge(&mut control, "control").await;

    let mut cancelled = connect(&path).await;
    write_frame(&mut cancelled, &request("cancelled")).await;
    assert!(matches!(
        read_frame(&mut cancelled).await,
        WireResponse::Connected {
            connect_ack_required: true,
            ..
        }
    ));
    cancelled
        .shutdown()
        .await
        .expect("close without ConnectAck");
    drop(cancelled);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut observer = connect(&path).await;
    write_frame(&mut observer, &Request::List {}).await;
    assert!(matches!(
        read_frame(&mut observer).await,
        WireResponse::SessionList { ref sessions, .. }
            if sessions.len() == 1
                && sessions[0].id == control_id.as_str()
                && sessions[0].status == SessionLifecycle::Live
    ));
    write_frame(
        &mut observer,
        &Request::Ping {
            session: control_id.clone(),
        },
    )
    .await;
    assert!(matches!(read_frame(&mut observer).await, WireResponse::Ack));
    write_frame(
        &mut observer,
        &Request::Disconnect {
            session: control_id,
        },
    )
    .await;
    assert!(matches!(read_frame(&mut observer).await, WireResponse::Ack));
    drop(observer);
    drop(control);
    stop(&mut daemon);
    let _ = std::fs::remove_dir_all(root);
}

async fn list(stream: &mut UnixStream) -> Vec<rdpilot_ipc::SessionStatus> {
    write_frame(stream, &Request::List {}).await;
    match read_frame(stream).await {
        WireResponse::SessionList { sessions, .. } => sessions,
        other => panic!("expected a session list, got {other:?}"),
    }
}

async fn wait_until_connecting(observer: &mut UnixStream, name: &str) {
    for _ in 0..250 {
        if list(observer)
            .await
            .iter()
            .any(|s| s.id == name && s.status == SessionLifecycle::Connecting)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{name} never showed as Connecting");
}

async fn wait_until_live(observer: &mut UnixStream, name: &str) {
    for _ in 0..250 {
        if list(observer)
            .await
            .iter()
            .any(|s| s.id == name && s.status == SessionLifecycle::Live)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{name} never showed as Live");
}

/// Bounded to about 2 s, so a regression fails the test instead of hanging it.
async fn wait_until_absent(observer: &mut UnixStream, name: &str) {
    for _ in 0..100 {
        if !list(observer).await.iter().any(|s| s.id == name) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{name} stayed listed after its client left");
}

/// After a failed connect: nothing is listed or recorded, the name connects
/// again once the gate opens, and a restarted daemon finds no orphan.
async fn assert_name_free_then_restart_clean(
    root: &Path,
    daemon: &mut std::process::Child,
    observer: &mut UnixStream,
    name: &str,
    extra_env: &[(&str, &str)],
) {
    let path = socket(root);
    assert!(list(observer).await.is_empty());
    assert!(rdpilot_daemon::scan_orphans(&root.join("sessions.json")).is_empty());

    release_connect(root);
    let mut again = connect(&path).await;
    let session = acknowledge(&mut again, name).await;
    write_frame(&mut again, &Request::Disconnect { session }).await;
    assert!(matches!(read_frame(&mut again).await, WireResponse::Ack));
    drop(again);

    stop(daemon);
    let mut restarted = spawn_with(root, extra_env);
    let mut observer = connect(&path).await;
    assert!(list(&mut observer).await.is_empty());
    drop(observer);
    stop(&mut restarted);
}

#[tokio::test]
async fn disconnect_of_a_connecting_session_acks_fails_the_connect_and_frees_the_name() {
    let root = root("disconnect");
    let mut daemon = spawn(&root);
    let path = socket(&root);

    let mut held = connect(&path).await;
    write_frame(&mut held, &request("held")).await;
    let mut observer = connect(&path).await;
    wait_until_connecting(&mut observer, "held").await;

    write_frame(
        &mut observer,
        &Request::Disconnect {
            session: "held".parse().expect("non-empty id"),
        },
    )
    .await;
    assert!(matches!(read_frame(&mut observer).await, WireResponse::Ack));

    match read_frame(&mut held).await {
        WireResponse::Error(error) => {
            assert_eq!(error.code, WireErrorCode::Internal);
            assert!(
                error.message.contains("cancelled by disconnect"),
                "{}",
                error.message
            );
        }
        other => panic!("expected the connect to fail, got {other:?}"),
    }
    drop(held);

    assert_name_free_then_restart_clean(&root, &mut daemon, &mut observer, "held", &[]).await;
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn connect_deadline_expiry_fails_the_connect_and_frees_the_name() {
    let root = root("deadline");
    let env = [("RDPILOT_DAEMON_CONNECT_DEADLINE_MS", "300")];
    let mut daemon = spawn_with(&root, &env);
    let path = socket(&root);

    let mut slow = connect(&path).await;
    write_frame(&mut slow, &request("slow")).await;
    match read_frame(&mut slow).await {
        WireResponse::Error(error) => {
            assert_eq!(error.code, WireErrorCode::Internal);
            assert!(error.message.contains("timed out"), "{}", error.message);
        }
        other => panic!("expected the connect to time out, got {other:?}"),
    }
    drop(slow);

    let mut observer = connect(&path).await;
    assert_name_free_then_restart_clean(&root, &mut daemon, &mut observer, "slow", &env).await;
    let _ = std::fs::remove_dir_all(root);
}
