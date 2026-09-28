//! RDP connection, native recovery input/framebuffer, deployment and file transfer.
use crate::{
    bridge::BridgeShared,
    connect,
    framebuffer::{FrameWatch, SharedFrame},
    session_loop::{self, RdpInputEvent},
};
use crate::{
    BootstrapStage, ConnectionConfig, CuaAttachment, Error, Key, KeyAction, MouseAction, Result,
    Screenshot,
};
use ironrdp::pdu::input::fast_path::FastPathInputEvent;
use ironrdp_input::Database;
use rdpilot_bridge_protocol::{FileTransferOp, FileTransferRequest, Message};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::mpsc;
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(100);
const DRAG_STEP_GAP: Duration = Duration::from_millis(15);
const INPUT_CHANNEL_CAPACITY: usize = 16;
const RUN_DIALOG_SETTLE: Duration = Duration::from_secs(2);
const SESSION_SETTLE: Duration = Duration::from_secs(10);
const TYPE_CHUNK_LEN: usize = 1;
const TYPE_CHUNK_GAP: Duration = Duration::from_millis(150);
pub struct Session {
    thread: Option<JoinHandle<Result<()>>>,
    input_tx: mpsc::Sender<RdpInputEvent>,
    frame: SharedFrame,
    input_db: Mutex<Database>,
    desktop_size: (u32, u32),
    bridge: Arc<BridgeShared>,
    next_req_id: AtomicU64,
    share_root: Option<PathBuf>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    pub bytes_transferred: u64,
    pub checksum: String,
}

fn chunk_str(s: &str, max_len: usize) -> Vec<String> {
    let max_len = max_len.max(1);
    let chars: Vec<char> = s.chars().collect();
    chars
        .chunks(max_len)
        .map(|c| c.iter().collect::<String>())
        .collect()
}
fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = fs::File::open(path).map_err(|e| {
        Error::dvc(format!(
            "failed to open {} for checksum: {e}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let read = file.read(&mut buf).map_err(|e| {
            Error::dvc(format!(
                "failed to read {} while hashing: {e}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}
impl Session {
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Session> {
        let (connection_result, framed, bridge) = connect::connect(cfg).await?;

        let desktop_size = (
            u32::from(connection_result.desktop_size.width),
            u32::from(connection_result.desktop_size.height),
        );

        let frame = SharedFrame::new();
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);

        let loop_frame = frame.clone();
        let loop_bridge = bridge.clone();
        let thread = std::thread::Builder::new()
            .name("rdpilot-session".to_owned())
            .spawn(move || -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| Error::Session(format!("could not start session runtime: {e}")))?;
                let result = runtime.block_on(session_loop::run(
                    framed,
                    connection_result,
                    input_rx,
                    loop_frame.clone(),
                    loop_bridge.clone(),
                ));
                // Passive observers (the live viewer) see the end of the RDP
                // session even while the registry entry still exists.
                loop_frame.mark_ended();
                if let Err(error) = &result {
                    tracing::error!(%error,"RDP session loop ended");
                    loop_bridge.invalidate(&error.to_string());
                }
                result
            })
            .map_err(|e| Error::Session(format!("could not spawn session thread: {e}")))?;

        Ok(Session {
            thread: Some(thread),
            input_tx,
            frame,
            input_db: Mutex::new(Database::new()),
            desktop_size,
            bridge,
            next_req_id: AtomicU64::new(1),
            share_root: cfg.get_share_root().map(Path::to_path_buf),
        })
    }

    pub fn desktop_size(&self) -> (u32, u32) {
        self.desktop_size
    }

    /// A passive, read-only observer of this session's framebuffer. It holds
    /// only the shared frame (never the session), carries no input, Cua or
    /// transfer capability, and reports `ended` once the RDP session loop
    /// returns or the session is closed or dropped.
    #[must_use]
    pub fn frame_watch(&self) -> FrameWatch {
        self.frame.watch()
    }

    fn check_bounds(&self, action: &MouseAction) -> Result<()> {
        let (w, h) = self.desktop_size;
        for (x, y) in action.coordinates() {
            let (x, y) = (u32::from(x), u32::from(y));
            if x >= w || y >= h {
                return Err(Error::coordinate_out_of_bounds(x, y, w, h));
            }
        }
        Ok(())
    }

    pub async fn send_mouse(&self, action: MouseAction) -> Result<()> {
        self.check_bounds(&action)?;

        let gap = match action {
            MouseAction::DoubleClick { .. } => Some(DOUBLE_CLICK_GAP),
            MouseAction::Drag { .. } => Some(DRAG_STEP_GAP),
            MouseAction::Move { .. } | MouseAction::Click { .. } | MouseAction::Scroll { .. } => {
                None
            }
        };

        let batches = crate::input::mouse_operations(&action);
        let last_index = batches.len().saturating_sub(1);

        for (i, batch) in batches.into_iter().enumerate() {
            let events: Vec<FastPathInputEvent> = {
                let mut db = self
                    .input_db
                    .lock()
                    .map_err(|_| Error::Session("input state lock poisoned".to_owned()))?;
                db.apply(batch).into_iter().collect()
            }; // guard dropped here — never held across the .await below

            self.input_tx
                .send(RdpInputEvent::FastPath(events))
                .await
                .map_err(|_| {
                    Error::Session(
                        self.bridge
                            .failure()
                            .unwrap_or_else(|| "input channel closed".to_owned()),
                    )
                })?;

            if let Some(gap) = gap {
                if i != last_index {
                    tokio::time::sleep(gap).await;
                }
            }
        }

        Ok(())
    }

    pub async fn send_key(&self, action: KeyAction) -> Result<()> {
        let ops = crate::input::key_operations(&action);

        let events: Vec<FastPathInputEvent> = {
            let mut db = self
                .input_db
                .lock()
                .map_err(|_| Error::Session("input state lock poisoned".to_owned()))?;
            db.apply(ops).into_iter().collect()
        }; // guard dropped here — never held across the .await below

        self.input_tx
            .send(RdpInputEvent::FastPath(events))
            .await
            .map_err(|_| {
                Error::Session(
                    self.bridge
                        .failure()
                        .unwrap_or_else(|| "input channel closed".to_owned()),
                )
            })?;

        Ok(())
    }

    fn unique_share_name(&self) -> String {
        let counter = self
            .next_req_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(
            "rdpilot-transfer-{nanos}-{}-{counter}.tmp",
            std::process::id()
        )
    }

    pub fn bootstrap_stages(&self) -> Vec<BootstrapStage> {
        self.bridge.bootstrap.snapshot()
    }

    fn bootstrap_summary(&self) -> String {
        let stages = self.bootstrap_stages();
        if stages.is_empty() {
            "none".to_owned()
        } else {
            stages
                .into_iter()
                .map(BootstrapStage::as_str)
                .collect::<Vec<_>>()
                .join(",")
        }
    }

    pub async fn screenshot(&self) -> Result<Screenshot> {
        let snap = self.frame.read();
        if snap.is_empty() {
            return Err(Error::Session(
                "no framebuffer captured yet (awaiting the first graphics update)".to_owned(),
            ));
        }
        Screenshot::from_rgba(snap.width, snap.height, snap.rgba)
    }

    pub async fn close(mut self) -> Result<()> {
        self.frame.mark_ended();
        self.bridge.stop("session closed");
        let _ = self.input_tx.try_send(RdpInputEvent::Close);

        let (dummy_tx, _dummy_rx) = mpsc::channel(1);
        let _ = std::mem::replace(&mut self.input_tx, dummy_tx);

        if let Some(thread) = self.thread.take() {
            tokio::time::timeout(
                Duration::from_secs(15),
                tokio::task::spawn_blocking(move || thread.join()),
            )
            .await
            .map_err(|_| Error::Session("session shutdown deadline exceeded".into()))?
            .map_err(|e| Error::Session(format!("join task failed: {e}")))?
            .map_err(|_| Error::Session("session thread panicked".to_owned()))?
        } else {
            Ok(())
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.frame.mark_ended();
        self.bridge.stop("session dropped");
        let _ = self.input_tx.try_send(RdpInputEvent::Close);
    }
}

impl Session {
    /// Attach native Cua MCP to this immutable RDP connection incarnation.
    pub async fn attach_cua(&self) -> Result<CuaAttachment> {
        self.bridge.attach(self.input_tx.clone()).await
    }
    pub async fn ping(&self) -> Result<Duration> {
        let started = std::time::Instant::now();
        match self
            .bridge
            .request(&self.input_tx, Message::Ping, Duration::from_millis(500))
            .await?
        {
            Message::Pong => Ok(started.elapsed()),
            _ => Err(Error::dvc("unexpected ping response")),
        }
    }
    /// Start the shipped, hash-verified bundle over RDPDR. One launch per generation.
    pub async fn deploy_and_launch(&self) -> Result<Duration> {
        if self.bridge.ready() {
            return self.ping().await;
        }
        if self
            .bridge
            .bootstrap_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(Error::bootstrap(
                "launch already attempted for this generation; reconnect explicitly",
            ));
        }
        tokio::time::sleep(SESSION_SETTLE).await;
        for _ in 0..3 {
            self.inject_bootstrap().await?;
            for _ in 0..60 {
                if let Some(reason) = self.bridge.failure() {
                    return Err(Error::bootstrap(reason));
                }
                if let Ok(pong) = self.ping().await {
                    return Ok(pong);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        Err(Error::bootstrap(format!(
            "Cua bundle did not become ready; stages={}",
            self.bootstrap_summary()
        )))
    }
    async fn inject_bootstrap(&self) -> Result<()> {
        // Retrying before Ready does not replay MCP. The guest generation mutex
        // makes repeated script invocations no-ops while their owner is alive.
        self.bridge
            .bootstrap
            .record(BootstrapStage::LaunchInputAttempted);
        self.send_key(KeyAction::Combo(vec![Key::Win, Key::R]))
            .await?;
        tokio::time::sleep(RUN_DIALOG_SETTLE).await;
        self.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A]))
            .await?;
        let command = format!(
            r#"powershell.exe -NoProfile -ExecutionPolicy Bypass -File "\\tsclient\RDPILOT\bundle\bootstrap.ps1" -Generation {}"#,
            self.bridge.generation
        );
        for chunk in chunk_str(&command, TYPE_CHUNK_LEN) {
            self.send_key(KeyAction::Type(chunk)).await?;
            tokio::time::sleep(TYPE_CHUNK_GAP).await;
        }
        self.send_key(KeyAction::Combo(vec![Key::Enter])).await?;
        self.bridge
            .bootstrap
            .record(BootstrapStage::LaunchInputSent);
        Ok(())
    }
    async fn transfer(
        &self,
        op: FileTransferOp,
        remote: &str,
        share: String,
    ) -> Result<rdpilot_bridge_protocol::FileTransferData> {
        let response = self
            .bridge
            .request(
                &self.input_tx,
                Message::FileTransfer(FileTransferRequest {
                    op,
                    remote_path: remote.into(),
                    share_name: share,
                }),
                Duration::from_secs(30),
            )
            .await?;
        match response {
            Message::FileTransferResult(result) => {
                if result.success {
                    return result
                        .data
                        .ok_or_else(|| Error::dvc("missing transfer result"));
                }
                let reason = result.error.unwrap_or_else(|| "transfer failed".into());
                if result.error_kind.as_deref() == Some("path_traversal") {
                    Err(Error::path_traversal(reason))
                } else {
                    Err(Error::BridgeRejected(reason))
                }
            }
            _ => Err(Error::dvc("unexpected transfer response")),
        }
    }
    /// Upload to a relative path under the guest's `%TEMP%\rdpilot-transfer-root`.
    pub async fn upload_file(&self, local: &Path, remote_name: &str) -> Result<TransferOutcome> {
        let root = self
            .share_root
            .as_ref()
            .ok_or_else(|| Error::Config("file transfer requires share_root".into()))?;
        let name = self.unique_share_name();
        let staged = root.join(&name);
        let _cleanup = StagedFile(staged.clone());
        // create_new avoids clobbering even if staging was concurrently substituted.
        let mut source = fs::File::open(local).map_err(|e| Error::dvc(e.to_string()))?;
        let mut dest = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staged)
            .map_err(|e| Error::dvc(e.to_string()))?;
        std::io::copy(&mut source, &mut dest).map_err(|e| Error::dvc(e.to_string()))?;
        drop(dest);
        let hash = sha256_file(&staged)?;
        let result = self
            .transfer(FileTransferOp::Upload, remote_name, name)
            .await?;
        if !hash.eq_ignore_ascii_case(&result.sha256) {
            return Err(Error::checksum_mismatch(result.sha256, hash));
        }
        Ok(TransferOutcome {
            bytes_transferred: result.bytes_transferred,
            checksum: hash,
        })
    }
    /// Download from the guest transfer root. The local destination must not exist.
    pub async fn download_file(&self, remote_name: &str, local: &Path) -> Result<TransferOutcome> {
        let root = self
            .share_root
            .as_ref()
            .ok_or_else(|| Error::Config("file transfer requires share_root".into()))?;
        let name = self.unique_share_name();
        let staged = root.join(&name);
        let _cleanup = StagedFile(staged.clone());
        let result = self
            .transfer(FileTransferOp::Download, remote_name, name)
            .await?;
        let hash = sha256_file(&staged)?;
        if !hash.eq_ignore_ascii_case(&result.sha256) {
            return Err(Error::checksum_mismatch(result.sha256, hash));
        }
        let mut input = fs::File::open(&staged).map_err(|e| Error::dvc(e.to_string()))?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(local)
            .map_err(|e| Error::dvc(e.to_string()))?;
        if let Err(e) = std::io::copy(&mut input, &mut output) {
            drop(output);
            let _ = fs::remove_file(local);
            return Err(Error::dvc(e.to_string()));
        }
        drop(output);
        let written_hash = sha256_file(local)?;
        if !written_hash.eq_ignore_ascii_case(&hash) {
            let _ = fs::remove_file(local);
            return Err(Error::checksum_mismatch(hash, written_hash));
        }
        Ok(TransferOutcome {
            bytes_transferred: result.bytes_transferred,
            checksum: hash,
        })
    }
}
struct StagedFile(PathBuf);
impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> (Session, mpsc::Receiver<RdpInputEvent>) {
        let (tx, rx) = mpsc::channel(16);
        (
            Session {
                thread: None,
                input_tx: tx,
                frame: SharedFrame::new(),
                input_db: Mutex::new(Database::new()),
                desktop_size: (100, 100),
                bridge: Arc::new(BridgeShared::new()),
                next_req_id: AtomicU64::new(1),
                share_root: None,
            },
            rx,
        )
    }
    #[tokio::test]
    async fn native_recovery_stays_available_with_dead_cua_bridge() {
        let (s, mut rx) = session();
        s.bridge.invalidate("Cua crashed");
        s.frame.write(2, 2, vec![255; 16]);
        assert_eq!(s.screenshot().await.unwrap().width, 2);
        s.send_key(KeyAction::Combo(vec![Key::Win, Key::R]))
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(RdpInputEvent::FastPath(_))));
        s.send_mouse(MouseAction::Move { x: 10, y: 10 })
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(RdpInputEvent::FastPath(_))));
        assert!(s.attach_cua().await.is_err());
    }
    #[tokio::test]
    async fn invalid_native_coordinates_send_nothing() {
        let (s, mut rx) = session();
        assert!(matches!(
            s.send_mouse(MouseAction::Move { x: 100, y: 10 }).await,
            Err(Error::CoordinateOutOfBounds { .. })
        ));
        assert!(rx.try_recv().is_err());
    }
    #[tokio::test]
    async fn transfer_without_share_fails_without_wire_work() {
        let (s, mut rx) = session();
        assert!(matches!(
            s.upload_file(Path::new("unused"), "remote").await,
            Err(Error::Config(_))
        ));
        assert!(matches!(
            s.download_file("remote", Path::new("unused")).await,
            Err(Error::Config(_))
        ));
        assert!(rx.try_recv().is_err());
    }
    /// The frame watch added for the live viewer does not change what
    /// `screenshot()` returns: the bytes equal the written frame exactly.
    #[tokio::test]
    async fn screenshot_bytes_are_unchanged_by_the_frame_watch() {
        let (s, _rx) = session();
        let rgba: Vec<u8> = (0u8..=255).cycle().take(3 * 2 * 4).collect();
        let watch = s.frame_watch();
        s.frame.write(3, 2, rgba.clone());
        let shot = s.screenshot().await.unwrap();
        assert_eq!((shot.width, shot.height), (3, 2));
        assert_eq!(shot.rgba, rgba);
        let (seq, watched) = watch.capture().unwrap();
        assert_eq!(seq, 1);
        assert_eq!(watched.rgba, shot.rgba);
        assert_eq!(watched.to_png().unwrap(), shot.to_png().unwrap());
    }
    #[tokio::test]
    async fn dropping_the_session_ends_its_frame_watch() {
        let (s, _rx) = session();
        let watch = s.frame_watch();
        assert!(!watch.status().ended);
        drop(s);
        assert!(watch.status().ended);
    }
}
