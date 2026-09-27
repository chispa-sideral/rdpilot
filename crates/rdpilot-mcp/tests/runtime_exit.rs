#![cfg(unix)]
//! A transport failure must exit even while Tokio's blocking stdin read is pending.
use rdpilot_ipc::{
    read_frame, write_frame, CuaStreamFrame, Request, WireResponse, IPC_COMPATIBILITY_VERSION,
};
#[tokio::test]
async fn closed_attachment_exits_binary_with_open_stdin_and_no_protocol_output() {
    let root = std::env::temp_dir().join(format!("rdpilot-mcp-exit-{}", std::process::id()));
    let socket_dir = root.join("rdpilot");
    std::fs::create_dir_all(&socket_dir).unwrap();
    let listener = tokio::net::UnixListener::bind(socket_dir.join("daemon.sock")).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_rdpilot-mcp"))
        .args(["--session", "existing"])
        .env("XDG_RUNTIME_DIR", &root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let (mut stream, _) = listener.accept().await.unwrap();
    assert!(matches!(
        read_frame::<_, Request>(&mut stream).await.unwrap(),
        Request::List {}
    ));
    write_frame(
        &mut stream,
        &WireResponse::SessionList {
            sessions: vec![],
            compatibility_version: Some(IPC_COMPATIBILITY_VERSION),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame::<_, Request>(&mut stream).await.unwrap(),
        Request::CuaAttach { .. }
    ));
    write_frame(
        &mut stream,
        &WireResponse::CuaAttached {
            session_incarnation: 1,
            bridge_generation: 2,
            runtime_generation: 3,
            attachment_id: 4,
        },
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    write_frame(
        &mut stream,
        &CuaStreamFrame::Closed {
            reason: "runtime exited".into(),
        },
    )
    .await
    .unwrap();
    let output = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait_with_output())
        .await
        .expect("binary remained alive with open stdin")
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("runtime exited"));
    drop(stdin);
    drop(listener);
    std::fs::remove_dir_all(root).unwrap();
}
