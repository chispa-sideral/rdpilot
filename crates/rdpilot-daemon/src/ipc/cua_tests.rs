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
}
struct FakeAttachment {
    busy: Arc<AtomicBool>,
    dead: watch::Receiver<bool>,
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
        _: std::path::PathBuf,
        _: String,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async {
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
}
struct Connector;
impl SessionConnector for Connector {
    fn connect(
        &self,
        _: rdpilot::ConnectionConfig,
    ) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        Box::pin(async {
            let (dead, _) = watch::channel(false);
            Ok(Box::new(FakeSession {
                busy: Arc::new(AtomicBool::new(false)),
                dead,
            }) as Box<dyn ManagedSession>)
        })
    }
}
fn server(registry: Arc<Registry>, capacity: usize) -> tokio::io::DuplexStream {
    let (client, daemon) = tokio::io::duplex(capacity);
    tokio::task::spawn_local(async move {
        serve_connection(daemon, &registry, None).await;
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
    tokio::task::LocalSet::new().run_until(async {
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
 }).await;
}

#[tokio::test]
async fn caller_eof_releases_attachment_and_backpressure_does_not_hold_session_lock() {
    tokio::task::LocalSet::new()
        .run_until(async {
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
        })
        .await;
}

#[tokio::test(start_paused = true)]
async fn stalled_attachment_write_times_out_without_blocking_native_control() {
    tokio::task::LocalSet::new()
        .run_until(async {
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
        })
        .await;
}

#[tokio::test]
async fn failed_acknowledgement_releases_provisional_attachment() {
    tokio::task::LocalSet::new()
        .run_until(async {
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
        })
        .await;
}
