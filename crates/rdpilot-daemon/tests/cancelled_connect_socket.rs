//! Unix-socket regressions for a cancelled Connect.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rdpilot_ipc::{Request, SessionLifecycle, WireResponse};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

async fn write_frame<T: serde::Serialize>(stream: &mut UnixStream, value: &T) {
    rdpilot_ipc::write_frame(stream, value)
        .await
        .expect("write test frame");
}

async fn read_frame(stream: &mut UnixStream) -> WireResponse {
    rdpilot_ipc::read_frame(stream)
        .await
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
        connect_ack: true,
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

#[tokio::test]
async fn early_close_leaves_no_live_or_reconciliation_record_after_restart() {
    let root = root("early");
    let mut daemon = spawn(&root);
    let path = socket(&root);
    let mut cancelled = connect(&path).await;
    write_frame(&mut cancelled, &request("cancelled")).await;
    cancelled.shutdown().await.expect("close client write side");
    drop(cancelled);
    release_connect(&root);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut observer = connect(&path).await;
    write_frame(&mut observer, &Request::List {}).await;
    assert!(
        matches!(read_frame(&mut observer).await, WireResponse::SessionList { ref sessions } if sessions.is_empty())
    );
    assert!(rdpilot_daemon::scan_orphans(&root.join("sessions.json")).is_empty());
    drop(observer);
    stop(&mut daemon);

    let mut restarted = spawn(&root);
    let mut observer = connect(&path).await;
    write_frame(&mut observer, &Request::List {}).await;
    assert!(
        matches!(read_frame(&mut observer).await, WireResponse::SessionList { ref sessions } if sessions.is_empty())
    );
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
        WireResponse::SessionList { ref sessions }
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
