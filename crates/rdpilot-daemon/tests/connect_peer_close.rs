//! Offline regressions for cancellation-safe Connect response delivery.
//!
//! Both tests spawn the real daemon binary with its explicit fake connector.
//! They use only temporary Unix sockets and never arm `RDPILOT_LIVE` or open
//! an RDP transport.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use rdpilot_ipc::{Request, SessionId, WireResponse};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;
const BOOTSTRAP_DELAY: Duration = Duration::from_millis(250);

struct DaemonChild(Child);

impl DaemonChild {
    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn write_frame<T: serde::Serialize>(
    stream: &mut UnixStream,
    value: &T,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).expect("test frames serialize");
    let len = u32::try_from(body.len()).expect("test frame fits u32");
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> std::io::Result<T> {
    let mut len = [0_u8; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len);
    assert!(len <= MAX_FRAME_LEN, "frame length stays bounded");
    let mut body = vec![0_u8; len as usize];
    stream.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).expect("test response deserializes"))
}

fn unique_root(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rdpilot-connect-peer-close-{label}-{}-{nanos}",
        std::process::id()
    ))
}

fn socket_path(root: &Path) -> PathBuf {
    root.join("xdg-runtime").join("rdpilot").join("daemon.sock")
}

fn spawn_daemon(root: &Path, diagnostics: Option<&Path>) -> DaemonChild {
    let runtime = root.join("xdg-runtime");
    std::fs::create_dir_all(&runtime).expect("create isolated runtime directory");
    let sink = root.join("sessions.json");
    let mut command = Command::new(PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-daemon")));
    command
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("RDPILOT_DAEMON_SINK_PATH", &sink)
        .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
        .env(
            "RDPILOT_DAEMON_TEST_BOOTSTRAP_DELAY_MS",
            BOOTSTRAP_DELAY.as_millis().to_string(),
        )
        .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "60000")
        .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000")
        .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "20")
        // Fake connector never reads the binary; this only makes dispatch
        // exercise the fake-only delayed deploy_and_launch branch.
        .env("RDPILOT_SENSOR_BINARY_PATH", root.join("fake-sensor.exe"))
        .env_remove("RDPILOT_LIVE")
        .env_remove("RDPILOT_CONNECTION_FILE");
    if let Some(path) = diagnostics {
        command.env("RDPILOT_DAEMON_DIAGNOSTICS_PATH", path);
    } else {
        command.env_remove("RDPILOT_DAEMON_DIAGNOSTICS_PATH");
    }
    DaemonChild(command.spawn().expect("spawn fake-only daemon"))
}

async fn connect_with_backoff(path: &Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(path).await {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("fake daemon socket did not become reachable: {error}"),
        }
    }
}

fn connect_request(name: &str) -> Request {
    Request::Connect {
        name: Some(name.to_owned()),
        host: "127.0.0.1".to_owned(),
        port: Some(3389),
        username: "test-user".to_owned(),
        password: "test-password".to_owned(),
        domain: None,
        accept_invalid_certs: false,
    }
}

async fn list(stream: &mut UnixStream) -> Vec<rdpilot_ipc::SessionStatus> {
    write_frame(stream, &Request::List {})
        .await
        .expect("write List");
    match read_frame::<WireResponse>(stream).await.expect("read List") {
        WireResponse::SessionList { sessions } => sessions,
        other => panic!("expected SessionList, got {other:?}"),
    }
}

async fn disconnect(stream: &mut UnixStream, session: SessionId) {
    write_frame(stream, &Request::Disconnect { session })
        .await
        .expect("write Disconnect");
    assert!(matches!(
        read_frame::<WireResponse>(stream)
            .await
            .expect("read Disconnect"),
        WireResponse::Ack
    ));
}

async fn wait_for_sessions(
    stream: &mut UnixStream,
    expected: usize,
) -> Vec<rdpilot_ipc::SessionStatus> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let sessions = list(stream).await;
        if sessions.len() == expected {
            return sessions;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} sessions; got {}",
            sessions.len()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn connect_peer_close_reclaims_only_session() {
    let root = unique_root("empty");
    let diagnostics = root.join("diagnostics").join("events.json");
    let mut daemon = spawn_daemon(&root, Some(&diagnostics));
    let path = socket_path(&root);

    let mut early = connect_with_backoff(&path).await;
    write_frame(&mut early, &connect_request("early-close"))
        .await
        .expect("write delayed Connect");
    early.shutdown().await.expect("close peer before response");
    drop(early);

    let mut observer = connect_with_backoff(&path).await;
    let sessions = wait_for_sessions(&mut observer, 0).await;
    assert!(sessions.is_empty());
    let sink: Value =
        serde_json::from_slice(&std::fs::read(root.join("sessions.json")).expect("read sink"))
            .expect("sink is JSON");
    assert_eq!(
        sink,
        Value::Array(Vec::new()),
        "early close must remove reconciliation record"
    );

    let raw = std::fs::read_to_string(&diagnostics).expect("opt-in diagnostics written");
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(&diagnostics)
            .expect("diagnostic metadata")
            .permissions(),
    );
    assert_eq!(
        mode & 0o777,
        0o600,
        "Unix diagnostics file must be owner-only"
    );
    assert!(
        raw.contains("registry_opened")
            && raw.contains("ipc_peer_closed")
            && raw.contains("registry_closed")
    );
    for forbidden in [
        "127.0.0.1",
        "test-user",
        "test-password",
        "host",
        "error",
        "command",
    ] {
        assert!(
            !raw.contains(forbidden),
            "redacted diagnostics must not contain {forbidden}"
        );
    }

    // Restart after the empty sink check: an early peer close is clean
    // cancellation, not a crash record that a fresh daemon may surface as an
    // Orphaned session.
    drop(observer);
    daemon.stop();
    let mut restarted = spawn_daemon(&root, Some(&diagnostics));
    let mut observer = connect_with_backoff(&path).await;
    assert!(
        list(&mut observer).await.is_empty(),
        "restart after peer close must seed no Orphaned entry"
    );

    // A waiting client is a separate normal control after the empty-registry
    // and restart assertions, so this test does not conflate control survival
    // with empty cleanup.
    write_frame(&mut observer, &connect_request("waiting-control"))
        .await
        .expect("write control Connect");
    let control = match read_frame::<WireResponse>(&mut observer)
        .await
        .expect("read delayed control response")
    {
        WireResponse::Connected { session } => session,
        other => panic!("expected Connected control, got {other:?}"),
    };
    disconnect(&mut observer, control).await;
    drop(observer);
    restarted.stop();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn connect_peer_close_generation_does_not_close_reused_or_control_session() {
    let root = unique_root("generation");
    let diagnostics = root.join("must-not-exist.json");
    let mut daemon = spawn_daemon(&root, None);
    let path = socket_path(&root);

    let mut control_stream = connect_with_backoff(&path).await;
    write_frame(&mut control_stream, &connect_request("independent-control"))
        .await
        .expect("write control Connect");
    let control = match read_frame::<WireResponse>(&mut control_stream)
        .await
        .expect("read control Connected")
    {
        WireResponse::Connected { session } => session,
        other => panic!("expected Connected control, got {other:?}"),
    };

    let mut stale = connect_with_backoff(&path).await;
    write_frame(&mut stale, &connect_request("stale-target"))
        .await
        .expect("write stale Connect");
    stale
        .shutdown()
        .await
        .expect("close stale peer before response");
    drop(stale);

    let sessions = wait_for_sessions(&mut control_stream, 1).await;
    assert_eq!(
        sessions[0].id,
        control.as_str(),
        "exact-generation cleanup must leave the independent control Live"
    );
    assert!(
        !diagnostics.exists(),
        "diagnostics are default-off without an opt-in path"
    );
    disconnect(&mut control_stream, control).await;
    drop(control_stream);
    daemon.stop();
    let _ = std::fs::remove_dir_all(root);
}
