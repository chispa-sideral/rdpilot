//! Portable upgrade/lifetime proof; no Cua desktop schemas are reproduced.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::seams::{
    BoxFuture, DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector,
};
use rdpilot_ipc::SessionLifecycle;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};

struct FakeSession {
    busy: Arc<AtomicBool>,
    dead: watch::Sender<bool>,
    gated: Option<Arc<GatedSink>>,
}
struct FakeAttachment {
    busy: Arc<AtomicBool>,
    dead: watch::Receiver<bool>,
    gated: Option<Arc<GatedSink>>,
    tx: mpsc::Sender<Value>,
    rx: mpsc::Receiver<Value>,
}
impl Drop for FakeAttachment {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::SeqCst);
    }
}
impl ManagedCua for FakeAttachment {
    fn identity(&self) -> (u64, u64, u64) {
        (101, 202, 303)
    }
    fn send(&self, message: Value) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async move {
            if *self.dead.borrow() {
                return Err(DaemonError::Connect("closed".into()));
            }
            if message.get("stall").is_some() {
                std::future::pending::<()>().await;
            }
            if let Some(gated) = &self.gated {
                gated.log.lock().unwrap().push(Seen::Cua);
            }
            self.tx
                .send(message)
                .await
                .map_err(|_| DaemonError::Connect("closed".into()))
        })
    }
    fn recv(&mut self) -> BoxFuture<'_, Result<Option<Value>, DaemonError>> {
        Box::pin(async move {
            if *self.dead.borrow() {
                return Ok(None);
            }
            tokio::select! { _ = self.dead.changed() => Ok(None), message = self.rx.recv() => Ok(message) }
        })
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
}
impl ManagedSession for FakeSession {
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>> {
        Box::pin(async move {
            let _ = self.dead.send(true);
            Ok(())
        })
    }
    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
    }
    fn attach_cua(&self) -> BoxFuture<'_, Result<Box<dyn ManagedCua>, DaemonError>> {
        Box::pin(async move {
            if self.busy.swap(true, Ordering::SeqCst) {
                return Err(DaemonError::Connect("busy".into()));
            }
            let (tx, rx) = mpsc::channel(2);
            tx.send(json!({"jsonrpc":"2.0","id":"server","method":"sampling/createMessage"}))
                .await
                .unwrap();
            Ok(Box::new(FakeAttachment {
                busy: self.busy.clone(),
                dead: self.dead.subscribe(),
                gated: self.gated.clone(),
                tx,
                rx,
            }) as Box<dyn ManagedCua>)
        })
    }
    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async { Ok(rdpilot::Screenshot::from_rgba(1, 1, vec![0, 0, 0, 255])?) })
    }
    fn send_mouse(&self, _: rdpilot::MouseAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn send_key(&self, _: rdpilot::KeyAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn upload_file(
        &self,
        local: std::path::PathBuf,
        _: String,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async move {
            if local.to_string_lossy().contains("fail") {
                return Err(DaemonError::Connect(format!(
                    "cannot read {}",
                    local.display()
                )));
            }
            Ok(rdpilot::TransferOutcome {
                bytes_transferred: 0,
                checksum: String::new(),
            })
        })
    }
    fn download_file(
        &self,
        _: String,
        _: std::path::PathBuf,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async {
            Ok(rdpilot::TransferOutcome {
                bytes_transferred: 0,
                checksum: String::new(),
            })
        })
    }
    fn ping(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn desktop_size(&self) -> (u32, u32) {
        (1, 1)
    }
    fn deploy_and_launch(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn human_input(&self) -> Option<Arc<dyn crate::seams::HumanInput>> {
        self.gated
            .clone()
            .map(|gated| gated as Arc<dyn crate::seams::HumanInput>)
    }
}

/// What a gated session saw, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    Human(rdpilot_vocab::RawInput),
    Cua,
}

/// Human input that waits at a gate; it and the Cua attachment record into
/// one sequence.
struct GatedSink {
    log: std::sync::Mutex<Vec<Seen>>,
    gate: tokio::sync::Semaphore,
}

impl crate::seams::HumanInput for GatedSink {
    fn send(
        &self,
        events: Vec<rdpilot_vocab::RawInput>,
    ) -> crate::seams::BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async move {
            drop(self.gate.acquire().await.unwrap());
            self.log
                .lock()
                .unwrap()
                .extend(events.into_iter().map(Seen::Human));
            Ok(())
        })
    }
}

/// A fresh fake session; with `gated`, its human input and Cua calls
/// record into that sink.
fn fake_session(gated: Option<Arc<GatedSink>>) -> Box<dyn ManagedSession> {
    let (dead, _) = watch::channel(false);
    Box::new(FakeSession {
        busy: Arc::new(AtomicBool::new(false)),
        dead,
        gated,
    })
}

struct GatedConnector(Arc<GatedSink>);
impl SessionConnector for GatedConnector {
    fn connect(
        &self,
        _: rdpilot::ConnectionConfig,
    ) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        let gated = Arc::clone(&self.0);
        Box::pin(async move { Ok(fake_session(Some(gated))) })
    }
}

struct Connector;
impl SessionConnector for Connector {
    fn connect(
        &self,
        _: rdpilot::ConnectionConfig,
    ) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        Box::pin(async { Ok(fake_session(None)) })
    }
}
fn server(registry: Arc<Registry>, capacity: usize) -> tokio::io::DuplexStream {
    let (client, daemon) = tokio::io::duplex(capacity);
    tokio::spawn(async move {
        serve_connection(daemon, &registry, None, None).await;
    });
    client
}
async fn open(registry: &Registry, name: &str) -> rdpilot_ipc::SessionId {
    registry
        .open(
            Some(name.into()),
            "fake".into(),
            rdpilot::ConnectionConfig::new("fake", "u", "p"),
        )
        .await
        .unwrap()
}
async fn attach(client: &mut tokio::io::DuplexStream, id: rdpilot_ipc::SessionId) -> u64 {
    write_frame(client, &Request::CuaAttach { session: id })
        .await
        .unwrap();
    match read_frame::<_, WireResponse>(client).await.unwrap() {
        WireResponse::CuaAttached {
            session_incarnation,
            bridge_generation: 101,
            runtime_generation: 202,
            attachment_id: 303,
        } => session_incarnation,
        other => panic!("unexpected attachment {other:?}"),
    }
}
async fn native_ping(registry: Arc<Registry>, id: rdpilot_ipc::SessionId) {
    let mut client = server(registry, 1024);
    write_frame(&mut client, &Request::Ping { session: id })
        .await
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            read_frame::<_, WireResponse>(&mut client)
        )
        .await
        .unwrap()
        .unwrap(),
        WireResponse::Ack
    ));
}

#[tokio::test]
async fn streaming_releases_both_locks_preserves_partial_frames_and_invalidates_old_incarnations() {
    async {
  let registry=Arc::new(Registry::new(Arc::new(Connector),Arc::new(NoopReconciliationSink)));
  let a=open(&registry,"a").await; let b=open(&registry,"b").await;
  let mut ca=server(registry.clone(),512); let old=attach(&mut ca,a.clone()).await;
  assert!(!registry.live_idle_durations().iter().any(|(id,_)| id==&a), "active stream must not be reaped while idle");
  let mut cb=server(registry.clone(),512); attach(&mut cb,b.clone()).await;
  let value=json!({"jsonrpc":"2.0","id":null,"method":"opaque","params":{"session":"not-a-target","payload":"x".repeat(20_000)}});
  let body=serde_json::to_vec(&CuaStreamFrame::Message{message:value.clone()}).unwrap();
  // Give the daemon only a partial header, then read an unsolicited reverse request.
  ca.write_all(&(body.len() as u32).to_be_bytes()[..2]).await.unwrap();
  assert!(matches!(read_frame::<_,CuaStreamFrame>(&mut ca).await.unwrap(),CuaStreamFrame::Message{message} if message["id"]=="server"));
  assert!(matches!(read_frame::<_,CuaStreamFrame>(&mut cb).await.unwrap(),CuaStreamFrame::Message{..}));
  native_ping(registry.clone(),a.clone()).await;
  native_ping(registry.clone(),b.clone()).await;
  let mut second=server(registry.clone(),1024);
  write_frame(&mut second,&Request::CuaAttach{session:a.clone()}).await.unwrap();
  assert!(matches!(read_frame::<_,WireResponse>(&mut second).await.unwrap(),WireResponse::Error(e) if e.message.contains("busy")));
  ca.write_all(&(body.len() as u32).to_be_bytes()[2..]).await.unwrap();
  ca.write_all(&body).await.unwrap();
  assert_eq!(read_frame::<_,CuaStreamFrame>(&mut ca).await.unwrap(),CuaStreamFrame::Message{message:value});
  // Disconnect uses a separate authenticated connection while the old stream is idle.
  let mut management=server(registry.clone(),1024);
  write_frame(&mut management,&Request::Disconnect{session:a.clone()}).await.unwrap();
  assert!(matches!(tokio::time::timeout(Duration::from_millis(500),read_frame::<_,WireResponse>(&mut management)).await.unwrap().unwrap(),WireResponse::Ack));
  assert!(matches!(read_frame::<_,CuaStreamFrame>(&mut ca).await.unwrap(),CuaStreamFrame::Closed{..}));
  let a=open(&registry,"a").await; let mut fresh=server(registry.clone(),1024);
  assert_ne!(attach(&mut fresh,a).await,old);
  let message=json!({"id":"b","result":{"only":"b"}});
  write_frame(&mut cb,&CuaStreamFrame::Message{message:message.clone()}).await.unwrap();
  assert_eq!(read_frame::<_,CuaStreamFrame>(&mut cb).await.unwrap(),CuaStreamFrame::Message{message});
 }.await;
}

#[tokio::test]
async fn caller_eof_releases_attachment_and_backpressure_does_not_hold_session_lock() {
    async {
        let registry = Arc::new(Registry::new(
            Arc::new(Connector),
            Arc::new(NoopReconciliationSink),
        ));
        let id = open(&registry, "a").await;
        let mut client = server(registry.clone(), 256);
        attach(&mut client, id.clone()).await;
        let _ = read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap();
        write_frame(
            &mut client,
            &CuaStreamFrame::Message {
                message: json!({"data":"x".repeat(50_000)}),
            },
        )
        .await
        .unwrap();
        // The server is now blocked writing the large reply to this deliberately unread socket.
        native_ping(registry.clone(), id.clone()).await;
        drop(client);
        for _ in 0..20 {
            tokio::task::yield_now().await;
            let mut next = server(registry.clone(), 1024);
            write_frame(
                &mut next,
                &Request::CuaAttach {
                    session: id.clone(),
                },
            )
            .await
            .unwrap();
            if matches!(
                read_frame::<_, WireResponse>(&mut next).await.unwrap(),
                WireResponse::CuaAttached { .. }
            ) {
                return;
            }
        }
        panic!("caller EOF did not release attachment");
    }
    .await;
}

#[tokio::test(start_paused = true)]
async fn stalled_attachment_write_times_out_without_blocking_native_control() {
    async {
        let registry = Arc::new(Registry::new(
            Arc::new(Connector),
            Arc::new(NoopReconciliationSink),
        ));
        let id = open(&registry, "stall").await;
        let mut client = server(registry.clone(), 1024);
        attach(&mut client, id.clone()).await;
        let _ = read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap();
        write_frame(
            &mut client,
            &CuaStreamFrame::Message {
                message: json!({"stall":true}),
            },
        )
        .await
        .unwrap();
        native_ping(registry.clone(), id).await;
        assert!(matches!(
            read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap(),
            CuaStreamFrame::Closed { .. }
        ));
    }
    .await;
}

#[tokio::test]
async fn failed_acknowledgement_releases_provisional_attachment() {
    async {
        let registry = Arc::new(Registry::new(
            Arc::new(Connector),
            Arc::new(NoopReconciliationSink),
        ));
        let id = open(&registry, "ack").await;
        let mut client = server(registry.clone(), 1024);
        write_frame(
            &mut client,
            &Request::CuaAttach {
                session: id.clone(),
            },
        )
        .await
        .unwrap();
        drop(client);
        // Run the provisional acquisition and failed ACK to completion.
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(registry.live_idle_durations().len(), 1);
        let mut fresh = server(registry.clone(), 1024);
        attach(&mut fresh, id).await;
    }
    .await;
}

use crate::events::{CallOutcome, EventKind, EventSource};

const MARKER: &str = "SECRET-MARKER-7f3a";

async fn exchange(client: &mut tokio::io::DuplexStream, message: Value) {
    write_frame(
        client,
        &CuaStreamFrame::Message {
            message: message.clone(),
        },
    )
    .await
    .unwrap();
    let echoed = tokio::time::timeout(
        Duration::from_secs(2),
        read_frame::<_, CuaStreamFrame>(client),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(echoed, CuaStreamFrame::Message { message });
}

async fn wait_for_kind(log: &crate::events::SessionEvents, want: fn(&EventKind) -> bool) {
    for _ in 0..200 {
        if log.after(0).events.iter().any(|e| want(&e.kind)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("event not recorded");
}

#[tokio::test]
async fn cua_stream_records_calls_and_forwards_messages_unchanged() {
    async {
            let registry = Arc::new(Registry::new(
                Arc::new(Connector),
                Arc::new(NoopReconciliationSink),
            ));
            let id = open(&registry, "rec").await;
            let log = registry.events(&id).unwrap();
            assert!(log.after(0).events.is_empty(), "connect records nothing");
            // A reader that never reads must not slow anything down.
            let _stalled = log.subscribe();
            let mut client = server(registry.clone(), 4096);
            attach(&mut client, id.clone()).await;
            let _ = read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap();
            let image = format!("{MARKER}{}", "A".repeat(200_000));
            exchange(&mut client, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"type_text","arguments":{"text":MARKER,"path":format!("C:\\{MARKER}.txt")}}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"image","data":image}]}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":"e","method":"tools/call","params":{"name":"click","arguments":{"x":1}}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":"e","error":{"code":-32000,"message":MARKER}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"note":MARKER}})).await;
            exchange(&mut client, json!({"not":"json-rpc","id":[1,2]})).await;
            exchange(&mut client, json!([{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"batch"}}])).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"wait"}})).await;
            // Many round trips while the stalled reader holds its receiver.
            tokio::time::timeout(Duration::from_secs(5), async {
                for n in 0..60 {
                    exchange(&mut client, json!({"jsonrpc":"2.0","id":format!("n{n}"),"method":"tools/call","params":{"name":"zoom"}})).await;
                    exchange(&mut client, json!({"jsonrpc":"2.0","id":format!("n{n}"),"result":{}})).await;
                }
            })
            .await
            .expect("forwarding stalled");
            write_frame(&mut client, &CuaStreamFrame::Closed { reason: MARKER.into() })
                .await
                .unwrap();
            assert!(matches!(
                read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap(),
                CuaStreamFrame::Closed { .. }
            ));
            wait_for_kind(&log, |k| matches!(k, EventKind::CuaDetached { .. })).await;

            let page = log.after(0);
            let kinds: Vec<&EventKind> = page.events.iter().map(|e| &e.kind).collect();
            assert!(page.events.iter().all(|e| e.source == EventSource::Cua));
            assert_eq!(kinds[0], &EventKind::CuaAttached { attachment: 303 });
            assert!(matches!(kinds[1], EventKind::CallStarted { call: 1, name } if name == "type_text"));
            assert!(matches!(kinds[2], EventKind::CallFinished { call: 1, outcome: CallOutcome::Ok, .. }));
            assert!(matches!(kinds[3], EventKind::CallStarted { call: 2, name } if name == "click"));
            assert!(matches!(kinds[4], EventKind::CallFinished { call: 2, outcome: CallOutcome::Error, .. }));
            assert!(matches!(kinds[5], EventKind::CallStarted { call: 3, name } if name == "wait"));
            let n = kinds.len();
            assert!(matches!(kinds[n - 2], EventKind::CallFinished { call: 3, outcome: CallOutcome::NoReply, .. }));
            assert_eq!(
                kinds[n - 1],
                &EventKind::CuaDetached {
                    attachment: 303,
                    reason: "caller closed or malformed frame".into()
                }
            );
            let serialized = serde_json::to_string(&page).unwrap();
            assert!(!serialized.contains(MARKER));
            assert!(!serialized.contains("\"e\""), "JSON-RPC ids are never stored");
        }
        .await;
}

async fn native(registry: &Arc<Registry>, request: Request) -> WireResponse {
    let mut client = server(registry.clone(), 1024);
    write_frame(&mut client, &request).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        read_frame::<_, WireResponse>(&mut client),
    )
    .await
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn native_verbs_record_name_outcome_and_duration_only() {
    async {
        let registry = Arc::new(Registry::new(
            Arc::new(Connector),
            Arc::new(NoopReconciliationSink),
        ));
        let id = open(&registry, "verbs").await;
        let log = registry.events(&id).unwrap();
        let session = || id.clone();
        native(&registry, Request::List {}).await;
        native(&registry, Request::Ping { session: session() }).await;
        assert!(
            log.after(0).events.is_empty(),
            "list and ping record nothing"
        );

        native(&registry, Request::Screenshot { session: session() }).await;
        native(
            &registry,
            Request::Mouse {
                session: session(),
                action: rdpilot::MouseAction::Move { x: 4242, y: 4343 },
            },
        )
        .await;
        native(
            &registry,
            Request::Key {
                session: session(),
                action: rdpilot::KeyAction::Type(MARKER.into()),
            },
        )
        .await;
        native(&registry, Request::DesktopSize { session: session() }).await;
        let failed = native(
            &registry,
            Request::Put {
                session: session(),
                local_path: format!("/tmp/fail-{MARKER}"),
                remote_name: MARKER.into(),
            },
        )
        .await;
        assert!(matches!(failed, WireResponse::Error(e) if e.message.contains(MARKER)));
        native(
            &registry,
            Request::Get {
                session: session(),
                remote_name: MARKER.into(),
                local_path: format!("/tmp/{MARKER}"),
            },
        )
        .await;
        // An unknown session records nothing anywhere.
        native(
            &registry,
            Request::Screenshot {
                session: "missing".parse().unwrap(),
            },
        )
        .await;

        let page = log.after(0);
        assert!(page.events.iter().all(|e| e.source == EventSource::Cli));
        let finished: Vec<(String, CallOutcome)> = page
            .events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::CallFinished { name, outcome, .. } => Some((name.clone(), *outcome)),
                _ => None,
            })
            .collect();
        let ok = CallOutcome::Ok;
        assert_eq!(
            finished,
            vec![
                ("screenshot".into(), ok),
                ("mouse".into(), ok),
                ("key".into(), ok),
                ("desktop_size".into(), ok),
                ("put".into(), CallOutcome::Error),
                ("get".into(), ok),
            ]
        );
        assert_eq!(page.events.len(), 12, "one start and one finish per verb");
        let serialized = serde_json::to_string(&page).unwrap();
        assert!(!serialized.contains(MARKER));
        assert!(!serialized.contains("4242") && !serialized.contains("4343"));

        let weak = Arc::downgrade(&log);
        drop(log);
        native(&registry, Request::Disconnect { session: session() }).await;
        assert!(weak.upgrade().is_none(), "disconnect drops the log");
    }
    .await;
}

/// With a recording on, Cua messages still cross unchanged (each `exchange`
/// compares the echo), and the recording holds the calls without any
/// argument, result or error text.
#[tokio::test]
async fn cua_forwarding_is_unchanged_while_recording() {
    async {
            let root = crate::recording::store::tests::temp_root("cua-rec");
            let service = crate::recording::RecordingService::fixed(
                crate::recording::StorageSettings {
                    root: root.clone(),
                    max_fps: 4.0,
                    budget_bytes: 1 << 30,
                },
                Arc::new(crate::recording::encoder::tests::FakeFactory::default()),
                60_000,
            );
            let registry = Arc::new(Registry::with_recordings(
                Arc::new(Connector),
                Arc::new(NoopReconciliationSink),
                service,
            ));
            let id = open(&registry, "rec").await;
            let started = native(&registry, Request::RecordStart { session: id.clone() }).await;
            assert!(matches!(started, WireResponse::RecordingChanged { changed: true, .. }));
            let mut client = server(registry.clone(), 4096);
            attach(&mut client, id.clone()).await;
            let _ = read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap();
            exchange(&mut client, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"type_text","arguments":{"text":MARKER,MARKER:1}}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":MARKER}]}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"click"}})).await;
            exchange(&mut client, json!({"jsonrpc":"2.0","id":2,"error":{"code":-1,"message":MARKER}})).await;
            write_frame(&mut client, &CuaStreamFrame::Closed { reason: "done".into() })
                .await
                .unwrap();
            let _ = read_frame::<_, CuaStreamFrame>(&mut client).await;
            let log = registry.events(&id).unwrap();
            wait_for_kind(&log, |k| matches!(k, EventKind::CuaDetached { .. })).await;
            native(&registry, Request::RecordStop { session: id.clone() }).await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let dir = std::fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
            let lines = crate::recording::store::read_events(&dir);
            let kinds: Vec<&str> = lines.iter().filter_map(|e| e["kind"].as_str()).collect();
            assert!(kinds.contains(&"cua_attached") && kinds.contains(&"cua_detached"));
            assert_eq!(kinds.iter().filter(|k| **k == "call_finished").count(), 2);
            let raw = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
            assert!(!raw.contains(MARKER));
            assert!(!raw.contains("arguments"));
            let _ = std::fs::remove_dir_all(root);
        }
        .await;
}

/// Through the real stream: under a human lease an acting call is answered
/// locally and never reaches Cua (the fake attachment echoes what it gets),
/// a read-only call reaches Cua, and the same acting call with
/// `"takeover": true` ends the lease and reaches Cua without `takeover`.
/// The refused call is recorded as an error in the activity log.
#[tokio::test]
async fn the_stream_gates_acting_calls_under_a_human_lease() {
    async {
            let registry = Arc::new(Registry::new(
                Arc::new(Connector),
                Arc::new(NoopReconciliationSink),
            ));
            let id = open(&registry, "notepad").await;
            let control = registry.control(&id).unwrap();
            control
                .take("100.64.0.5".parse().unwrap(), async {})
                .await
                .unwrap();
            let mut client = server(registry.clone(), 4096);
            attach(&mut client, id.clone()).await;
            let _ = read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap();
            let call = |id: u64, name: &str, args: Value| CuaStreamFrame::Message {
                message: json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}),
            };
            write_frame(&mut client, &call(1, "type_text", json!({"text":"a"})))
                .await
                .unwrap();
            let CuaStreamFrame::Message { message: refused } =
                read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap()
            else {
                panic!("expected a message");
            };
            assert_eq!(refused["id"], 1);
            assert_eq!(refused["result"]["isError"], true);
            assert!(refused.get("method").is_none(), "not the echo");
            write_frame(&mut client, &call(2, "list_windows", json!({})))
                .await
                .unwrap();
            let CuaStreamFrame::Message { message: echoed } =
                read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap()
            else {
                panic!("expected a message");
            };
            assert_eq!(echoed["params"]["name"], "list_windows", "reached Cua");
            write_frame(
                &mut client,
                &call(3, "type_text", json!({"text":"a","takeover":true})),
            )
            .await
            .unwrap();
            let CuaStreamFrame::Message { message: echoed } =
                read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap()
            else {
                panic!("expected a message");
            };
            assert_eq!(echoed["id"], 3);
            assert_eq!(echoed["params"]["arguments"], json!({"text":"a"}));
            assert!(control.check_agent().is_ok());
            let events = registry.events(&id).unwrap().after(0).events;
            let refused = events
                .iter()
                .find_map(|e| match &e.kind {
                    crate::events::EventKind::CallFinished { name, outcome, .. }
                        if name == "type_text" =>
                    {
                        Some(*outcome)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(refused, crate::events::CallOutcome::Error);
            assert!(events.iter().any(|e| e.source == crate::events::EventSource::Cua
                && matches!(e.kind, crate::events::EventKind::ControlTakenOver { .. })));
        }
        .await;
}

/// Through the real stream: a Release while the holder's key press still
/// waits for the input channel, then an acting Cua call. The call reaches
/// Cua only after the press and its release.
#[tokio::test]
async fn an_acting_cua_call_after_a_release_waits_for_the_releases() {
    async {
            let gated = Arc::new(GatedSink {
                log: std::sync::Mutex::new(Vec::new()),
                gate: tokio::sync::Semaphore::new(0),
            });
            let registry = Arc::new(Registry::new(
                Arc::new(GatedConnector(Arc::clone(&gated))),
                Arc::new(NoopReconciliationSink),
            ));
            let id = open(&registry, "notepad").await;
            let viewer = crate::registry::ViewerControl::new(Arc::clone(&registry));
            let (grant, _) = viewer
                .take(&id, "100.64.0.5".parse().unwrap())
                .await
                .unwrap();
            let mut client = server(registry.clone(), 4096);
            attach(&mut client, id.clone()).await;
            let _ = read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap();
            let shift = |down| rdpilot_vocab::RawInput::Key {
                code: 0x2A,
                extended: false,
                down,
            };
            let human = {
                let viewer = viewer.clone();
                let id = id.clone();
                let lease = grant.lease.clone();
                tokio::spawn(async move {
                    viewer
                        .input(&id, &lease, grant.generation, (1, 1), vec![shift(true)])
                        .await
                })
            };
            tokio::task::yield_now().await;
            let release = {
                let viewer = viewer.clone();
                let id = id.clone();
                let lease = grant.lease.clone();
                tokio::spawn(async move { viewer.release(&id, &lease).await })
            };
            tokio::task::yield_now().await;
            write_frame(
                &mut client,
                &CuaStreamFrame::Message {
                    message: json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"type_text","arguments":{"text":"a"}}}),
                },
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            gated.gate.add_permits(1);
            let CuaStreamFrame::Message { message: echoed } =
                read_frame::<_, CuaStreamFrame>(&mut client).await.unwrap()
            else {
                panic!("expected a message");
            };
            assert_eq!(echoed["id"], 1, "forwarded, not refused");
            assert!(echoed.get("method").is_some(), "the echo from Cua");
            human.await.unwrap().unwrap();
            release.await.unwrap().unwrap();
            assert_eq!(
                *gated.log.lock().unwrap(),
                vec![Seen::Human(shift(true)), Seen::Human(shift(false)), Seen::Cua]
            );
        }
        .await;
}
