#![cfg(unix)]
//! Real child pipes and process lifetime. No Cua schema/behavior duplication.
use rdpilot_bridge::{
    runtime::{read_json, run, Config},
    transfer,
};
use rdpilot_bridge_protocol::{Envelope, Message, MAX_FRAME_BYTES, QUEUE_DEPTH};

const BUNDLE: &str = "bundle-under-test";
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio::{sync::mpsc, time::timeout};

struct Fixture {
    input: mpsc::Sender<Envelope>,
    output: mpsc::Receiver<Envelope>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _temp: tempfile::TempDir,
    runtime: u64,
    attachment: u64,
}
impl Fixture {
    async fn new(script: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("child.py");
        std::fs::write(&path, script).unwrap();
        let (input, rx) = mpsc::channel(QUEUE_DEPTH);
        let (tx, output) = mpsc::channel(QUEUE_DEPTH);
        let config = Config {
            generation: 7,
            bundle_id: BUNDLE.into(),
            executable: PathBuf::from("python3"),
            args: vec!["-u".into(), path.to_string_lossy().into()],
            transfer_root: temp.path().join("root"),
            share_root: temp.path().join("share"),
            request_timeout: Duration::from_millis(500),
        };
        let task = tokio::spawn(run(config, rx, tx));
        let mut f = Self {
            input,
            output,
            task,
            _temp: temp,
            runtime: 0,
            attachment: 11,
        };
        f.send(Message::Hello {
            bundle_id: BUNDLE.into(),
        })
        .await;
        assert!(matches!(f.next().await, Message::Ready { .. }));
        f.send(Message::Open { attachment_id: 11 }).await;
        let Message::Opened {
            runtime_generation, ..
        } = f.next().await
        else {
            panic!("not opened")
        };
        f.runtime = runtime_generation;
        f
    }
    async fn send(&self, message: Message) {
        self.input.send(Envelope::new(7, 5, message)).await.unwrap();
    }
    async fn mcp(&self, message: Value) {
        self.send(Message::Mcp {
            attachment_id: self.attachment,
            runtime_generation: self.runtime,
            message,
        })
        .await;
    }
    async fn next(&mut self) -> Message {
        timeout(Duration::from_secs(5), self.output.recv())
            .await
            .unwrap()
            .unwrap()
            .message
    }
    async fn close(mut self) {
        self.send(Message::Close {
            attachment_id: self.attachment,
            reason: "done".into(),
        })
        .await;
        assert!(matches!(self.next().await, Message::Closed { .. }));
        drop(self.input);
        timeout(Duration::from_secs(4), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
const ECHO: &str = r#"
import sys,json,time,os
for line in sys.stdin:
    v=json.loads(line)
    if v.get('method')=='stall': time.sleep(30)
    if v.get('method')=='exit': sys.exit(7)
    if v.get('method')=='invalid': print('not json',flush=True); continue
    if v.get('method')=='stderr': sys.stderr.write('e'*200000);sys.stderr.flush()
    if v.get('method')=='spontaneous':
        print(json.dumps({'jsonrpc':'2.0','method':'notice','params':{'opaque':True}}),flush=True)
        print(json.dumps({'jsonrpc':'2.0','id':v['id'],'method':'server_request'}),flush=True)
    if 'method' in v and 'id' in v:
        print(json.dumps({'jsonrpc':'2.0','id':v['id'],'result':v}),flush=True)
"#;

#[tokio::test]
async fn opaque_duplex_large_payload_stderr_and_id_directions() {
    let mut f = Fixture::new(ECHO).await;
    let message = json!({"jsonrpc":"2.0","id":"opaque-id","method":"initialize","params":{"unknown":[1,null,{"value":true}]}});
    f.mcp(message.clone()).await;
    let Message::Mcp { message: reply, .. } = f.next().await else {
        panic!()
    };
    assert_eq!(reply["result"], message);
    f.mcp(json!({"jsonrpc":"2.0","id":null,"method":"spontaneous"}))
        .await;
    let Message::Mcp {
        message: notification,
        ..
    } = f.next().await
    else {
        panic!()
    };
    assert_eq!(notification["method"], "notice");
    let Message::Mcp {
        message: server_request,
        ..
    } = f.next().await
    else {
        panic!()
    };
    assert_eq!(server_request["method"], "server_request");
    let Message::Mcp {
        message: response, ..
    } = f.next().await
    else {
        panic!()
    };
    assert!(response["id"].is_null());
    // Same null ID in opposite directions must not satisfy the server's deadline.
    f.mcp(json!({"jsonrpc":"2.0","id":null,"result":{"server_reply":true}}))
        .await;
    let large = json!({"id":1,"method":"stderr","params":{"image":"x".repeat(100_000)}});
    f.mcp(large.clone()).await;
    let Message::Mcp {
        message: response, ..
    } = f.next().await
    else {
        panic!()
    };
    assert_eq!(response["result"], large);
    f.close().await;
}

#[tokio::test]
async fn stall_ping_busy_stale_generation_and_fresh_explicit_attach() {
    let mut f = Fixture::new(ECHO).await;
    f.send(Message::Open { attachment_id: 12 }).await;
    assert!(matches!(f.next().await, Message::Error { .. }));
    f.input
        .send(Envelope::new(
            99,
            1,
            Message::Mcp {
                attachment_id: 11,
                runtime_generation: f.runtime,
                message: json!({"id":1,"method":"exit"}),
            },
        ))
        .await
        .unwrap();
    f.send(Message::Mcp {
        attachment_id: 11,
        runtime_generation: f.runtime + 1,
        message: json!({"id":1,"method":"exit"}),
    })
    .await;
    f.mcp(json!({"id":1,"method":"stall"})).await;
    f.send(Message::Ping).await;
    assert!(matches!(f.next().await, Message::Pong));
    let Message::Closed { reason, .. } = f.next().await else {
        panic!()
    };
    assert!(reason.contains("deadline"), "{reason}");
    f.send(Message::Open { attachment_id: 11 }).await;
    assert!(matches!(f.next().await, Message::Error { .. }));
    f.send(Message::Open { attachment_id: 12 }).await;
    f.attachment = 12;
    let Message::Opened {
        runtime_generation, ..
    } = f.next().await
    else {
        panic!()
    };
    assert!(runtime_generation > f.runtime);
    f.runtime = runtime_generation;
    f.send(Message::Close {
        attachment_id: 11,
        reason: "late old Close".into(),
    })
    .await;
    f.send(Message::Mcp {
        attachment_id: 11,
        runtime_generation: runtime_generation - 1,
        message: json!({"id":1,"method":"exit"}),
    })
    .await;
    f.mcp(json!({"id":2,"method":"echo"})).await;
    let Message::Mcp { message, .. } = f.next().await else {
        panic!()
    };
    assert_eq!(message["id"], 2); // no replay of old id=1
    f.close().await;
}

#[tokio::test]
async fn unexpected_exit_and_malformed_output_close_once() {
    for method in ["exit", "invalid"] {
        let mut f = Fixture::new(ECHO).await;
        f.mcp(json!({"id":1,"method":method})).await;
        assert!(matches!(f.next().await, Message::Closed { .. }));
        f.send(Message::Ping).await;
        assert!(matches!(f.next().await, Message::Pong));
        drop(f.input);
        f.task.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn caller_eof_kills_descendants_and_reaps_root() {
    let script = r#"
import os,sys,json,time,subprocess
p=subprocess.Popen(['sleep','30'])
print(json.dumps({'method':'pids','params':[os.getpid(),p.pid]}),flush=True)
for line in sys.stdin: time.sleep(30)
"#;
    let mut f = Fixture::new(script).await;
    let Message::Mcp { message, .. } = f.next().await else {
        panic!()
    };
    let pids = message["params"].as_array().unwrap().clone();
    drop(f.input);
    f.task.await.unwrap().unwrap();
    for pid in pids {
        let pid = pid.as_u64().unwrap();
        timeout(Duration::from_secs(3), async {
            loop {
                let state = std::fs::read_to_string(format!("/proc/{pid}/stat"));
                if state.is_err() || state.unwrap().split_whitespace().nth(2) == Some("Z") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn unterminated_line_is_bounded_before_allocation() {
    let bytes = vec![b'x'; MAX_FRAME_BYTES + 1];
    let mut reader = tokio::io::BufReader::new(bytes.as_slice());
    assert!(read_json(&mut reader).await.is_err());
    let mut reader = tokio::io::BufReader::new(b"{\"id\":1}".as_slice());
    assert!(read_json(&mut reader).await.is_err());
}

#[tokio::test]
async fn stalled_child_does_not_block_transfer_control() {
    let mut f = Fixture::new(ECHO).await;
    std::fs::create_dir_all(f._temp.path().join("share")).unwrap();
    std::fs::write(f._temp.path().join("share/input"), b"content").unwrap();
    f.mcp(json!({"id":1,"method":"stall"})).await;
    f.send(Message::FileTransfer(
        rdpilot_bridge_protocol::FileTransferRequest {
            op: rdpilot_bridge_protocol::FileTransferOp::Upload,
            remote_path: "out".into(),
            share_name: "input".into(),
        },
    ))
    .await;
    let Message::FileTransferResult(result) = f.next().await else {
        panic!()
    };
    assert!(result.success);
    assert_eq!(
        std::fs::read(transfer::resolve(&f._temp.path().join("root"), "out").unwrap()).unwrap(),
        b"content"
    );
    f.close().await;
}

#[tokio::test]
async fn server_request_deadline_is_not_cleared_by_same_id_client_response() {
    let mut f = Fixture::new(ECHO).await;
    f.mcp(json!({"id":"same","method":"spontaneous"})).await;
    for _ in 0..3 {
        assert!(matches!(f.next().await, Message::Mcp { .. }));
    }
    // Deliberately never answer the server request. Its same-ID response to our
    // original request is not an answer in the opposite direction.
    let Message::Closed { reason, .. } = f.next().await else {
        panic!()
    };
    assert!(reason.contains("deadline"));
    drop(f.input);
    f.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn blocked_child_stdin_has_bounded_teardown_and_live_ping() {
    let mut f = Fixture::new("import time\ntime.sleep(30)\n").await;
    // Large enough to block an OS pipe, but bounded before writer admission.
    f.mcp(json!({"id":1,"method":"opaque","params":"x".repeat(1024*1024)}))
        .await;
    f.send(Message::Ping).await;
    assert!(matches!(f.next().await, Message::Pong));
    assert!(matches!(f.next().await, Message::Closed { .. }));
    drop(f.input);
    timeout(Duration::from_secs(4), f.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn backpressured_carrier_closes_without_blocking_process_teardown() {
    let script = r#"
import sys,json,time
for line in sys.stdin:
    for i in range(1000):
        print(json.dumps({'method':'burst','params':i}),flush=True)
    time.sleep(30)
"#;
    let mut f = Fixture::new(script).await;
    f.mcp(json!({"method":"start"})).await;
    // Do not consume output. Either per-runtime saturation closes the attachment,
    // or carrier saturation closes the bridge. Both must remain bounded.
    tokio::time::sleep(Duration::from_millis(100)).await;
    if !f.task.is_finished() {
        let mut closed = false;
        while let Ok(Some(envelope)) = timeout(Duration::from_secs(2), f.output.recv()).await {
            if matches!(envelope.message, Message::Closed { .. }) {
                closed = true;
                break;
            }
        }
        assert!(closed || f.task.is_finished());
    }
    drop(f.input);
    let _ = timeout(Duration::from_secs(4), f.task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn concurrent_targets_do_not_share_child_streams() {
    let mut a = Fixture::new(ECHO).await;
    let mut b = Fixture::new(ECHO).await;
    a.mcp(json!({"id":1,"method":"stall","params":{"target":"A"}}))
        .await;
    let message = json!({"id":1,"method":"echo","params":{"target":"B"}});
    b.mcp(message.clone()).await;
    let Message::Mcp { message: reply, .. } = b.next().await else {
        panic!()
    };
    assert_eq!(reply["result"], message);
    assert!(matches!(a.next().await, Message::Closed { .. }));
    b.mcp(message.clone()).await;
    let Message::Mcp { message: reply, .. } = b.next().await else {
        panic!()
    };
    assert_eq!(reply["result"], message);
    drop(a.input);
    a.task.await.unwrap().unwrap();
    b.close().await;
}

#[tokio::test]
async fn hello_for_another_bundle_stops_the_bridge() {
    let temp = tempfile::tempdir().unwrap();
    let (input, rx) = mpsc::channel(QUEUE_DEPTH);
    let (tx, _output) = mpsc::channel(QUEUE_DEPTH);
    let config = Config {
        generation: 7,
        bundle_id: BUNDLE.into(),
        executable: PathBuf::from("python3"),
        args: Vec::new(),
        transfer_root: temp.path().join("root"),
        share_root: temp.path().join("share"),
        request_timeout: Duration::from_millis(500),
    };
    let task = tokio::spawn(run(config, rx, tx));
    input
        .send(Envelope::new(
            7,
            1,
            Message::Hello {
                bundle_id: "another-bundle".into(),
            },
        ))
        .await
        .unwrap();
    let err = timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(err.to_string().contains("another-bundle"), "{err}");
}
