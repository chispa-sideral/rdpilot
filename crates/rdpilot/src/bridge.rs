//! Target-incarnation-bound Cua carrier. Never interprets MCP tools or IDs.
use crate::{
    bootstrap::BootstrapProgress, session_loop::RdpInputEvent, BootstrapStage, Error, Result,
};
use ironrdp::core::{ensure_size, impl_as_any, Encode, EncodeResult, WriteCursor};
use ironrdp::pdu::PduResult;
use ironrdp_dvc::{DvcEncode, DvcMessage, DvcProcessor};
use rdpilot_bridge_protocol::{Decoder, Envelope, Message, BUNDLE_ID, CHANNEL_NAME};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch, Notify};

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const STREAM_DEPTH: usize = 8;

struct AttachmentState {
    id: u64,
    runtime: u64,
    output: mpsc::Sender<serde_json::Value>,
    closed: watch::Sender<Option<String>>,
}
#[derive(Default)]
struct State {
    ready: bool,
    failure: Option<String>,
    last_rx: Option<tokio::time::Instant>,
    dead: bool,
    pending: HashMap<u64, oneshot::Sender<Message>>,
    attachment: Option<AttachmentState>,
    outgoing: VecDeque<Envelope>,
    retire: Option<Envelope>,
}
pub(crate) struct BridgeShared {
    pub generation: u64,
    pub bootstrap_started: AtomicBool,
    next: AtomicU64,
    state: Mutex<State>,
    pub bootstrap: BootstrapProgress,
    pub shutdown: Notify,
    pub data_ready: Notify,
    pub retire_ready: Notify,
}
impl std::fmt::Debug for BridgeShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeShared")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}
impl BridgeShared {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            generation: nanos
                ^ (u64::from(std::process::id()) << 32)
                ^ NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            bootstrap_started: AtomicBool::new(false),
            next: AtomicU64::new(1),
            state: Mutex::new(State::default()),
            bootstrap: BootstrapProgress::new(),
            shutdown: Notify::new(),
            data_ready: Notify::new(),
            retire_ready: Notify::new(),
        }
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn invalidate(&self, reason: &str) {
        let mut state = self.state();
        state.failure = Some(reason.to_owned());
        state.dead = true;
        state.ready = false;
        state.pending.clear();
        state.outgoing.clear();
        if let Some(a) = state.attachment.take() {
            let _ = a.closed.send(Some(reason.into()));
        }
    }
    pub fn failure(&self) -> Option<String> {
        self.state().failure.clone()
    }
    pub fn stop(&self, reason: &str) {
        self.invalidate(reason);
        self.shutdown.notify_one();
    }
    fn close_attachment(&self, id: u64, reason: &str) {
        let mut state = self.state();
        if state.attachment.as_ref().is_some_and(|a| a.id == id) {
            if let Some(a) = state.attachment.take() {
                let _ = a.closed.send(Some(reason.into()));
            }
        }
    }
    pub fn take_data(&self) -> Option<Envelope> {
        let mut s = self.state();
        let next = s.outgoing.pop_front();
        if !s.outgoing.is_empty() {
            self.data_ready.notify_one();
        }
        next
    }
    pub fn take_retire(&self) -> Option<Envelope> {
        self.state().retire.take()
    }
    fn retire(&self, id: u64, reason: &str) {
        self.close_attachment(id, reason);
        let mut s = self.state();
        s.outgoing
            .retain(|e| !matches!(e.message,Message::Mcp{attachment_id,..} if attachment_id==id));
        // Only one guest runtime can be active. Preserve the highest ID so a
        // dropped old handle can never replace retirement of its successor.
        let old = s
            .retire
            .as_ref()
            .and_then(|e| {
                if let Message::Close { attachment_id, .. } = e.message {
                    Some(attachment_id)
                } else {
                    None
                }
            })
            .unwrap_or(0);
        if id >= old {
            s.retire = Some(Envelope::new(
                self.generation,
                0,
                Message::Close {
                    attachment_id: id,
                    reason: reason.into(),
                },
            ));
        }
        self.retire_ready.notify_one();
    }
    pub fn check_liveness(&self) {
        let expired = {
            let s = self.state();
            s.ready
                && s.last_rx
                    .is_some_and(|at| at.elapsed() > Duration::from_secs(35))
        };
        if expired {
            self.invalidate("bridge heartbeat deadline exceeded; reconnect required");
        }
    }
    pub fn ready(&self) -> bool {
        let s = self.state();
        s.ready && !s.dead
    }
    pub async fn request(
        self: &Arc<Self>,
        input: &mpsc::Sender<RdpInputEvent>,
        message: Message,
        duration: Duration,
    ) -> Result<Message> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut s = self.state();
            if s.dead {
                return Err(Error::dvc("bridge disconnected"));
            }
            if s.pending.len() >= 32 {
                return Err(Error::dvc("bridge control queue full"));
            }
            s.pending.insert(id, tx);
        }
        let _pending = PendingRequest {
            bridge: self.clone(),
            id,
        };
        let envelope = Envelope::new(self.generation, id, message);
        let result = tokio::time::timeout(duration, async {
            input
                .send(RdpInputEvent::Request(envelope))
                .await
                .map_err(|_| Error::dvc("RDP session closed"))?;
            rx.await
                .map_err(|_| Error::dvc("bridge closed before replying"))
        })
        .await
        .map_err(|_| Error::dvc("bridge control deadline exceeded"));
        self.state().pending.remove(&id);
        match result? {
            Ok(Message::Error { reason }) => Err(Error::dvc(reason)),
            r => r,
        }
    }
    pub async fn attach(
        self: &Arc<Self>,
        input: mpsc::Sender<RdpInputEvent>,
    ) -> Result<CuaAttachment> {
        if !self.ready() {
            return Err(Error::dvc(
                "Cua bridge is not ready; deploy the bundle first",
            ));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (output, rx) = mpsc::channel(STREAM_DEPTH);
        let (closed, close_rx) = watch::channel(None);
        {
            let mut state = self.state();
            if state.attachment.is_some() {
                return Err(Error::dvc("target already has an active Cua attachment"));
            }
            state.attachment = Some(AttachmentState {
                id,
                runtime: 0,
                output,
                closed,
            });
        }
        // Provisional handle owns cleanup even if the attach future is cancelled.
        let mut attachment = CuaAttachment {
            bridge: self.clone(),
            input,
            attachment_id: id,
            runtime_generation: 0,
            output: rx,
            closed: close_rx,
        };
        let response = self
            .request(
                &attachment.input,
                Message::Open { attachment_id: id },
                CONTROL_TIMEOUT,
            )
            .await?;
        if let Message::Opened {
            attachment_id,
            runtime_generation,
        } = response
        {
            if attachment_id == id {
                attachment.runtime_generation = runtime_generation;
                let mut state = self.state();
                match state.attachment.as_mut().filter(|a| a.id == id) {
                    Some(a) => a.runtime = runtime_generation,
                    None => return Err(Error::dvc("attachment closed during open")),
                }
                return Ok(attachment);
            }
        }
        Err(Error::dvc("invalid bridge open acknowledgement"))
    }
}
/// Owned native MCP stream bound to one RDP incarnation. Drop/close kills its Cua runtime.
/// A fresh attachment starts a fresh MCP session; requests are never replayed.
pub struct CuaAttachment {
    bridge: Arc<BridgeShared>,
    input: mpsc::Sender<RdpInputEvent>,
    attachment_id: u64,
    runtime_generation: u64,
    output: mpsc::Receiver<serde_json::Value>,
    closed: watch::Receiver<Option<String>>,
}
impl CuaAttachment {
    pub fn attachment_id(&self) -> u64 {
        self.attachment_id
    }
    pub fn runtime_generation(&self) -> u64 {
        self.runtime_generation
    }
    pub fn bridge_generation(&self) -> u64 {
        self.bridge.generation
    }
    pub async fn send(&self, message: serde_json::Value) -> Result<()> {
        if let Some(reason) = self.closed.borrow().clone() {
            return Err(Error::dvc(reason));
        }
        let envelope = Envelope::new(
            self.bridge.generation,
            0,
            Message::Mcp {
                attachment_id: self.attachment_id,
                runtime_generation: self.runtime_generation,
                message,
            },
        );
        rdpilot_bridge_protocol::encode(&envelope).map_err(|e| Error::dvc(e.to_string()))?;
        let mut state = self.bridge.state();
        if state.dead
            || !state
                .attachment
                .as_ref()
                .is_some_and(|a| a.id == self.attachment_id)
        {
            return Err(Error::dvc("attachment closed"));
        }
        if state.outgoing.len() >= STREAM_DEPTH {
            drop(state);
            self.end("Cua outbound queue full");
            return Err(Error::dvc("Cua outbound queue full"));
        }
        state.outgoing.push_back(envelope);
        self.bridge.data_ready.notify_one();
        Ok(())
    }
    pub async fn recv(&mut self) -> Result<Option<serde_json::Value>> {
        if let Some(reason) = self.closed.borrow().clone() {
            return Err(Error::dvc(reason));
        }
        tokio::select! { biased;
            _=self.closed.changed()=>Err(Error::dvc(self.closed.borrow().clone().unwrap_or_else(||"bridge closed".into()))),
            value=self.output.recv()=>Ok(value),
        }
    }
    pub async fn close(&mut self) -> Result<()> {
        self.end("caller closed attachment");
        Ok(())
    }
    fn end(&self, reason: &str) {
        self.bridge.retire(self.attachment_id, reason);
    }
}
impl Drop for CuaAttachment {
    fn drop(&mut self) {
        self.end("caller detached");
    }
}

struct Frame(Vec<u8>);
impl Encode for Frame {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ensure_size!(in:dst,size:self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }
    fn name(&self) -> &'static str {
        "rdpilot-cua"
    }
    fn size(&self) -> usize {
        self.0.len()
    }
}
impl DvcEncode for Frame {}
pub(crate) struct BridgeProcessor {
    shared: Arc<BridgeShared>,
    decoder: Decoder,
}
impl_as_any!(BridgeProcessor);
impl BridgeProcessor {
    pub fn new(shared: Arc<BridgeShared>) -> Self {
        Self {
            shared,
            decoder: Decoder::new(),
        }
    }
    pub fn encode_request(&self, envelope: Envelope) -> std::io::Result<Vec<DvcMessage>> {
        Ok(vec![Box::new(Frame(rdpilot_bridge_protocol::encode(
            &envelope,
        )?))])
    }
}
impl Drop for BridgeProcessor {
    fn drop(&mut self) {
        self.shared.invalidate("RDP bridge disconnected");
    }
}
impl DvcProcessor for BridgeProcessor {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }
    fn start(&mut self, _: u32) -> PduResult<Vec<DvcMessage>> {
        {
            let mut state = self.shared.state();
            state.ready = false;
            state.pending.clear();
            state.outgoing.clear();
            if let Some(a) = state.attachment.take() {
                let _ = a.closed.send(Some("bridge channel restarted".into()));
            }
        }
        self.shared
            .bootstrap
            .record(BootstrapStage::DvcChannelCreated);
        self.encode_request(Envelope::new(
            self.shared.generation,
            0,
            Message::Hello {
                bundle_id: BUNDLE_ID.into(),
            },
        ))
        .map_err(|e| ironrdp::pdu::pdu_other_err!("bridge hello",source:e))
    }
    fn process(&mut self, _: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        let frames = match self.decoder.push(payload) {
            Ok(f) => f,
            Err(e) => {
                self.shared
                    .invalidate(&format!("invalid bridge frame: {e}"));
                return Ok(vec![]);
            }
        };
        let mut responses = Vec::new();
        for envelope in frames {
            if envelope.generation != self.shared.generation {
                continue;
            }
            let mut s = self.shared.state();
            if s.dead {
                continue;
            }
            s.last_rx = Some(tokio::time::Instant::now());
            match &envelope.message {
                Message::Ready { bundle_id } => {
                    if bundle_id == BUNDLE_ID {
                        s.ready = true;
                        self.shared
                            .bootstrap
                            .record(BootstrapStage::VersionReceived);
                    } else {
                        drop(s);
                        self.shared.invalidate("bridge bundle mismatch");
                        continue;
                    }
                }
                Message::Opened {
                    attachment_id,
                    runtime_generation,
                } => {
                    if let Some(a) = s.attachment.as_mut().filter(|a| a.id == *attachment_id) {
                        a.runtime = *runtime_generation;
                    }
                }
                Message::Mcp {
                    attachment_id,
                    runtime_generation,
                    message,
                } => {
                    if let Some(a) = s
                        .attachment
                        .as_ref()
                        .filter(|a| a.id == *attachment_id && a.runtime == *runtime_generation)
                    {
                        if a.output.try_send(message.clone()).is_err() {
                            let id = a.id;
                            drop(s);
                            self.shared
                                .close_attachment(id, "Cua caller is not reading");
                            responses.extend(
                                self.encode_request(Envelope::new(
                                    self.shared.generation,
                                    0,
                                    Message::Close {
                                        attachment_id: id,
                                        reason: "caller backpressure".into(),
                                    },
                                ))
                                .map_err(
                                    |e| ironrdp::pdu::pdu_other_err!("bridge close",source:e),
                                )?,
                            );
                            continue;
                        }
                    }
                }
                Message::Closed {
                    attachment_id,
                    reason,
                } => {
                    let id = *attachment_id;
                    let reason = reason.clone();
                    drop(s);
                    self.shared.close_attachment(id, &reason);
                    continue;
                }
                Message::Pong => self.shared.bootstrap.record(BootstrapStage::PongReceived),
                _ => {}
            }
            if let Some(tx) = s.pending.remove(&envelope.control_id) {
                let _ = tx.send(envelope.message);
            }
        }
        Ok(responses)
    }
}

struct PendingRequest {
    bridge: Arc<BridgeShared>,
    id: u64,
}
impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.bridge.state().pending.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn peer(shared: Arc<BridgeShared>) -> BridgeProcessor {
        let mut peer = BridgeProcessor::new(shared.clone());
        deliver(
            &mut peer,
            Envelope::new(
                shared.generation,
                0,
                Message::Ready {
                    bundle_id: BUNDLE_ID.into(),
                },
            ),
        );
        peer
    }
    fn deliver(peer: &mut BridgeProcessor, message: Envelope) {
        let bytes = rdpilot_bridge_protocol::encode(&message).unwrap();
        for chunk in bytes.chunks(101) {
            peer.process(1, chunk).unwrap();
        }
    }
    async fn attached(
        shared: Arc<BridgeShared>,
        peer: &mut BridgeProcessor,
    ) -> (CuaAttachment, mpsc::Receiver<RdpInputEvent>) {
        let (tx, mut rx) = mpsc::channel(16);
        let task = tokio::spawn({
            let shared = shared.clone();
            async move { shared.attach(tx).await }
        });
        let RdpInputEvent::Request(e) = rx.recv().await.unwrap() else {
            panic!("open")
        };
        let Message::Open { attachment_id } = e.message else {
            panic!("open")
        };
        deliver(
            peer,
            Envelope::new(
                shared.generation,
                e.control_id,
                Message::Opened {
                    attachment_id,
                    runtime_generation: 99,
                },
            ),
        );
        (task.await.unwrap().unwrap(), rx)
    }
    #[tokio::test]
    async fn stream_preserves_large_unsolicited_messages_and_rejects_cross_target_generation() {
        let a = Arc::new(BridgeShared::new());
        let b = Arc::new(BridgeShared::new());
        let mut pa = peer(a.clone());
        let mut pb = peer(b.clone());
        let (mut ca, _) = attached(a.clone(), &mut pa).await;
        let (mut cb, _) = attached(b.clone(), &mut pb).await;
        let value = json!({"jsonrpc":"2.0","id":null,"method":"server/request","params":{"data":"x".repeat(90_000)}});
        let packet = Envelope::new(
            a.generation,
            0,
            Message::Mcp {
                attachment_id: ca.attachment_id,
                runtime_generation: 99,
                message: value.clone(),
            },
        );
        deliver(&mut pa, packet.clone());
        deliver(&mut pb, packet);
        assert_eq!(ca.recv().await.unwrap(), Some(value));
        assert!(tokio::time::timeout(Duration::from_millis(5), cb.recv())
            .await
            .is_err());
        drop(pa);
        assert!(ca.recv().await.is_err());
        assert!(b.ready());
    }
    #[tokio::test]
    async fn busy_detach_and_new_attachment_do_not_replay() {
        let s = Arc::new(BridgeShared::new());
        let mut p = peer(s.clone());
        let (a, _input) = attached(s.clone(), &mut p).await;
        let (tx, _) = mpsc::channel(2);
        assert!(s.attach(tx).await.is_err());
        a.send(json!({"id":"opaque","method":"tools/call","params":{"x":1}}))
            .await
            .unwrap();
        let e = s.take_data().unwrap();
        assert!(matches!(e.message, Message::Mcp { .. }));
        let id = a.attachment_id;
        drop(a);
        let e = s.take_retire().unwrap();
        assert!(matches!(e.message,Message::Close{attachment_id,..} if attachment_id==id));
        let (fresh, mut rx) = attached(s, &mut p).await;
        assert_ne!(id, fresh.attachment_id);
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test(start_paused = true)]
    async fn silent_bridge_loss_bounds_receive_and_does_not_stop_native_rdp() {
        let s = Arc::new(BridgeShared::new());
        let mut p = peer(s.clone());
        let (mut a, _) = attached(s.clone(), &mut p).await;
        tokio::time::advance(Duration::from_secs(36)).await;
        s.check_liveness();
        assert!(a.recv().await.is_err());
        assert!(!s.ready());
        assert!(
            tokio::time::timeout(Duration::from_millis(1), s.shutdown.notified())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn outgoing_saturation_retires_cua_without_consuming_native_queue() {
        let s = Arc::new(BridgeShared::new());
        let mut p = peer(s.clone());
        let (a, mut native) = attached(s.clone(), &mut p).await;
        for _ in 0..STREAM_DEPTH {
            a.send(json!({"method":"notification"})).await.unwrap();
        }
        assert!(a.send(json!({"method":"overflow"})).await.is_err());
        assert!(s.ready());
        assert!(native.try_recv().is_err());
        assert!(matches!(
            s.take_retire().unwrap().message,
            Message::Close { .. }
        ));
        assert!(s.take_data().is_none());
    }
    #[tokio::test]
    async fn cancelled_control_cleans_pending_and_caller_backpressure_closes_attachment() {
        let s = Arc::new(BridgeShared::new());
        let mut p = peer(s.clone());
        let (a, _) = attached(s.clone(), &mut p).await;
        for _ in 0..=STREAM_DEPTH {
            deliver(
                &mut p,
                Envelope::new(
                    s.generation,
                    0,
                    Message::Mcp {
                        attachment_id: a.attachment_id,
                        runtime_generation: 99,
                        message: json!({"method":"notice"}),
                    },
                ),
            );
        }
        assert!(a.send(json!({"id":9,"method":"never"})).await.is_err());
        assert!(s.ready());
        let (tx, mut rx) = mpsc::channel(1);
        let task = tokio::spawn({
            let s = s.clone();
            async move { s.request(&tx, Message::Ping, Duration::from_secs(10)).await }
        });
        rx.recv().await.unwrap();
        task.abort();
        let _ = task.await;
        assert!(s.state().pending.is_empty());
    }
}
