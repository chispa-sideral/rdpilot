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
    future::Future,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(100);
const DRAG_STEP_GAP: Duration = Duration::from_millis(15);
const INPUT_CHANNEL_CAPACITY: usize = 16;
const RUN_DIALOG_SETTLE: Duration = Duration::from_secs(2);
const SESSION_SETTLE: Duration = Duration::from_secs(10);
const TYPE_CHUNK_LEN: usize = 1;
const TYPE_CHUNK_GAP: Duration = Duration::from_millis(150);
const NATIVE_ONLY_PING: &str =
    "native-only session (CuaEnabled no): there is no rdpilot-bridge to ping";
pub struct Session {
    thread: Option<JoinHandle<Result<()>>>,
    input_tx: mpsc::Sender<RdpInputEvent>,
    frame: SharedFrame,
    input_db: Arc<Mutex<Database>>,
    desktop_size: (u32, u32),
    bridge: Arc<BridgeShared>,
    next_req_id: AtomicU64,
    share_root: Option<PathBuf>,
    /// False for a native-only session (no bundle configured): there is no
    /// bridge to reach.
    cua: bool,
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
/// What the session thread tells the caller once the handshake is over.
type Ready = Result<(Arc<BridgeShared>, (u32, u32))>;

/// [`Session::connect`] with the name resolution injected, so a test can stall
/// or fail it.
async fn connect_with_resolver(
    cfg: ConnectionConfig,
    resolve: impl Future<Output = Result<SocketAddr>> + Send + 'static,
) -> Result<Session> {
    let addr = resolve.await?;

    let frame = SharedFrame::new();
    let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
    let (ready_tx, ready_rx) = oneshot::channel();
    let share_root = cfg.get_share_root().map(Path::to_path_buf);
    let cua = cfg.get_bundle_path().is_some();

    let thread_frame = frame.clone();
    let thread = std::thread::Builder::new()
        .name("rdpilot-session".to_owned())
        .spawn(move || run_session_thread(cfg, addr, thread_frame, input_rx, ready_tx))
        .map_err(|e| Error::Session(format!("could not spawn session thread: {e}")))?;

    // Dropping this future drops `ready_rx`; the session thread sees the
    // closed channel and ends the handshake.
    let (bridge, desktop_size) = ready_rx.await.map_err(|_| {
        Error::Session("session thread ended before the handshake finished".to_owned())
    })??;

    Ok(Session {
        thread: Some(thread),
        input_tx,
        frame,
        input_db: Arc::new(Mutex::new(Database::new())),
        desktop_size,
        bridge,
        next_req_id: AtomicU64::new(1),
        share_root,
        cua,
    })
}

/// Body of the `rdpilot-session` thread: run the handshake, report it through
/// `ready_tx`, then run the session loop. The handshake ends early when the
/// caller drops `ready_tx`'s receiver.
fn run_session_thread(
    cfg: ConnectionConfig,
    addr: SocketAddr,
    frame: SharedFrame,
    input_rx: mpsc::Receiver<RdpInputEvent>,
    mut ready_tx: oneshot::Sender<Ready>,
) -> Result<()> {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = ready_tx.send(Err(Error::Session(format!(
                "could not start session runtime: {e}"
            ))));
            return Ok(());
        }
    };
    runtime.block_on(async move {
        let connected = tokio::select! {
            result = connect::connect(&cfg, addr) => Some(result),
            () = ready_tx.closed() => None,
        };
        let (connection_result, framed, bridge) = match connected {
            None => return Ok(()),
            Some(Err(error)) => {
                let _ = ready_tx.send(Err(error));
                return Ok(());
            }
            Some(Ok(connected)) => connected,
        };
        let desktop_size = (
            u32::from(connection_result.desktop_size.width),
            u32::from(connection_result.desktop_size.height),
        );
        // A caller that is gone drops `framed`, which closes the connection.
        if ready_tx.send(Ok((bridge.clone(), desktop_size))).is_err() {
            return Ok(());
        }
        let result = session_loop::run(
            framed,
            connection_result,
            input_rx,
            frame.clone(),
            bridge.clone(),
        )
        .await;
        // Passive observers (the live viewer) see the end of the RDP
        // session even while the registry entry still exists.
        frame.mark_ended();
        if let Err(error) = &result {
            tracing::error!(%error,"RDP session loop ended");
            bridge.invalidate(&error.to_string());
        }
        result
    })
}

/// The entry point of the session thread. It takes the whole owned config.
type SessionThreadBody = fn(
    ConnectionConfig,
    SocketAddr,
    SharedFrame,
    mpsc::Receiver<RdpInputEvent>,
    oneshot::Sender<Ready>,
) -> Result<()>;

// Fixes at compile time that the whole owned config reaches the session
// thread, not a part of it.
const _: SessionThreadBody = run_session_thread;

impl Session {
    /// Open an RDP session.
    ///
    /// The returned future is `Send + 'static`: the config is cloned at call
    /// time, so it does not borrow `cfg`. The host name is resolved on the
    /// caller's runtime; the TCP connect, TLS, CredSSP and finalize steps and
    /// then the session loop run on the `rdpilot-session` thread's own
    /// current-thread runtime. The whole moved [`ConnectionConfig`] is the only
    /// input channel construction reads there, which is where a configured
    /// channel set belongs.
    ///
    /// Dropping the future, or timing it out, at any point aborts the
    /// handshake and, within a bounded wait, leaves no session and no
    /// `rdpilot-session` thread. A drop during name resolution leaves no thread
    /// (none was started); at most one pooled blocking worker of the caller's
    /// runtime finishes an operating-system lookup already in progress. A
    /// timeout around the future therefore also bounds the resolution. A caller
    /// dropped after the handshake finished ends the session loop by closing
    /// its input channel.
    ///
    /// # Errors
    ///
    /// [`Error::Connect`] when the name cannot be resolved or the handshake
    /// fails; [`Error::Session`] when the session thread or its runtime cannot
    /// start, or ends before the handshake finishes.
    pub fn connect(
        cfg: &ConnectionConfig,
    ) -> impl Future<Output = Result<Session>> + Send + 'static {
        let cfg = cfg.clone();
        let resolve = connect::resolve(cfg.host().to_owned(), cfg.get_port());
        connect_with_resolver(cfg, resolve)
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

    /// A `Send` input path for a daemon-side human viewer. It applies raw
    /// events through the same input state as [`Session::send_mouse`] and
    /// [`Session::send_key`], so pressed keys and buttons stay consistent
    /// between the two. It holds no session lock and does not keep the
    /// session alive: after close, sending fails.
    #[must_use]
    pub fn input_handle(&self) -> InputHandle {
        InputHandle {
            tx: self.input_tx.clone(),
            db: Arc::clone(&self.input_db),
            frame: self.frame.clone(),
            bridge: Arc::clone(&self.bridge),
        }
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

/// See [`Session::input_handle`].
#[derive(Clone)]
pub struct InputHandle {
    tx: mpsc::Sender<RdpInputEvent>,
    db: Arc<Mutex<Database>>,
    frame: SharedFrame,
    bridge: Arc<BridgeShared>,
}

impl std::fmt::Debug for InputHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputHandle").finish_non_exhaustive()
    }
}

impl InputHandle {
    /// Apply `events` in order and hand the result to the session loop.
    /// Pointer moves outside the current frame are refused.
    ///
    /// # Errors
    ///
    /// [`Error::CoordinateOutOfBounds`] for a move outside the frame (or
    /// before the first frame); [`Error::Session`] when the session's input
    /// channel is closed.
    pub async fn send(&self, events: Vec<crate::RawInput>) -> Result<()> {
        let (w, h) = self.frame.size();
        for event in &events {
            if let crate::RawInput::PointerMove { x, y } = *event {
                let (x, y) = (u32::from(x), u32::from(y));
                if x >= w || y >= h {
                    return Err(Error::coordinate_out_of_bounds(x, y, w, h));
                }
            }
        }
        let ops = crate::input::raw_operations(&events);
        let fast_path: Vec<FastPathInputEvent> = {
            let mut db = self
                .db
                .lock()
                .map_err(|_| Error::Session("input state lock poisoned".to_owned()))?;
            db.apply(ops).into_iter().collect()
        };
        if fast_path.is_empty() {
            return Ok(());
        }
        self.tx
            .send(RdpInputEvent::FastPath(fast_path))
            .await
            .map_err(|_| {
                Error::Session(
                    self.bridge
                        .failure()
                        .unwrap_or_else(|| "input channel closed".to_owned()),
                )
            })
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
        if !self.cua {
            return Err(Error::Session(NATIVE_ONLY_PING.to_owned()));
        }
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
    /// Launch a local copy of the served bridge (`install`, which verifies
    /// and installs the bundle, then starts the installed copy) through the
    /// Run dialog; see [`bridge_launch_command`]. One launch per generation.
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
            "rdpilot-bridge did not start (possible security prompt in the guest, or a bridge \
             that does not speak bridge protocol {}); stages={}",
            rdpilot_bridge_protocol::PROTOCOL_VERSION,
            self.bootstrap_summary()
        )))
    }
    async fn inject_bootstrap(&self) -> Result<()> {
        // Retrying before Ready does not replay MCP. The guest generation mutex
        // makes repeated launches no-ops while their owner is alive.
        self.bridge
            .bootstrap
            .record(BootstrapStage::LaunchInputAttempted);
        self.send_key(KeyAction::Combo(vec![Key::Win, Key::R]))
            .await?;
        tokio::time::sleep(RUN_DIALOG_SETTLE).await;
        self.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A]))
            .await?;
        let command = bridge_launch_command(self.bridge.generation);
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

/// The directory the daemon serves the bundle in, as the guest sees it.
const SERVED_BUNDLE_DIR: &str = r"\\tsclient\RDPILOT\bundle";

/// The longest line [`bridge_launch_command`] may produce. The Run dialog
/// keeps only the first 259 characters (`MAX_PATH - 1`) of what is typed; a
/// cut line leaves a `(` open, and cmd then runs nothing. 240 leaves a margin
/// below that limit for every `u64` generation.
const RUN_LINE_LIMIT: usize = 240;

/// Name of the local launcher copy for `generation`: `l` and the generation
/// in base 36, padded to 13 digits (the width of `u64::MAX`). The mapping is
/// one-to-one, so every generation has its own name, and every name has the
/// same length, starts with a letter, holds only `[0-9a-z]` and is never a
/// reserved device name.
fn launcher_name(generation: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = [b'0'; 13];
    let mut rest = generation;
    for digit in digits.iter_mut().rev() {
        *digit = DIGITS[(rest % 36) as usize];
        rest /= 36;
    }
    let digits: String = digits.iter().map(|&b| char::from(b)).collect();
    format!("l{digits}.exe")
}

/// What the Run dialog receives. Windows asks for confirmation before it
/// starts an executable from the redirected drive, so `cmd` first copies the
/// served bridge into `%LOCALAPPDATA%\rdpilot\` under [`launcher_name`],
/// starts that local copy with `install --generation <generation>` (which
/// verifies the served bundle and its own image against the manifest,
/// installs, starts the installed copy and exits), then deletes the copy. The
/// name is unique per generation, so concurrent connects of the same user do
/// not share a launcher; a retried launch for the same generation fails at
/// `copy` while the first launcher still runs. A copy left by an interrupted
/// launch is inside `%LOCALAPPDATA%\rdpilot`, which `cleanup` removes. `&&`
/// chains stop at the first failure, so nothing runs from an unexpected
/// directory. Only `%LOCALAPPDATA%` can contain spaces; it is quoted, and
/// every other token is a fixed name or a number. The line is at most
/// [`RUN_LINE_LIMIT`] characters long.
fn bridge_launch_command(generation: u64) -> String {
    let launcher = launcher_name(generation);
    let line = format!(
        r#"cmd /d /c cd /d "%LOCALAPPDATA%" && (md rdpilot 2>nul & cd rdpilot) && copy /y {SERVED_BUNDLE_DIR}\{exe} {launcher} && (.\{launcher} install --generation {generation} & del {launcher})"#,
        exe = rdpilot_bridge_protocol::BRIDGE_EXE_NAME,
    );
    debug_assert!(
        line.len() <= RUN_LINE_LIMIT,
        "{} > {RUN_LINE_LIMIT}",
        line.len()
    );
    line
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
                input_db: Arc::new(Mutex::new(Database::new())),
                desktop_size: (100, 100),
                bridge: Arc::new(BridgeShared::new()),
                next_req_id: AtomicU64::new(1),
                share_root: None,
                cua: true,
            },
            rx,
        )
    }
    #[tokio::test]
    async fn ping_on_a_native_only_session_says_so_at_once() {
        let (mut s, mut rx) = session();
        s.cua = false;
        let started = std::time::Instant::now();
        let error = s.ping().await.unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(error.to_string().contains("native-only session"), "{error}");
        // Nothing was sent towards the guest.
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn launch_command_copies_the_served_bridge_and_starts_the_local_copy() {
        assert_eq!(
            bridge_launch_command(42),
            r#"cmd /d /c cd /d "%LOCALAPPDATA%" && (md rdpilot 2>nul & cd rdpilot) && copy /y \\tsclient\RDPILOT\bundle\rdpilot-bridge.exe l0000000000016.exe && (.\l0000000000016.exe install --generation 42 & del l0000000000016.exe)"#
        );
    }

    #[test]
    fn launch_command_never_starts_an_executable_from_the_redirected_drive() {
        let command = bridge_launch_command(7);
        assert!(!command.to_lowercase().contains("powershell"));
        // The only reference to the drive is the copy source.
        assert_eq!(command.matches(r"\\tsclient").count(), 1);
        assert!(command.contains(r"copy /y \\tsclient\RDPILOT\bundle\rdpilot-bridge.exe "));
        // The started image is the local copy in %LOCALAPPDATA%\rdpilot.
        assert!(command.contains(r"&& (.\l0000000000007.exe install --generation 7 "));
        assert!(command.contains(r#"cd /d "%LOCALAPPDATA%" && (md rdpilot 2>nul & cd rdpilot) &&"#));
    }

    #[test]
    fn launch_command_fits_the_run_dialog_for_every_generation() {
        // The longest generation gives the longest line.
        let longest = bridge_launch_command(u64::MAX);
        assert!(
            longest.len() <= RUN_LINE_LIMIT,
            "{} characters: {longest}",
            longest.len()
        );
        // The Run dialog keeps 259 characters (MAX_PATH - 1).
        const { assert!(RUN_LINE_LIMIT < 259) };
        for generation in [0, 1, 1_801_369_873_856_485_411, u64::MAX - 1] {
            assert!(bridge_launch_command(generation).len() <= longest.len());
        }
        assert!(longest.is_ascii());
    }

    #[test]
    fn launch_command_passes_the_full_generation_once() {
        let generation = u64::MAX;
        let command = bridge_launch_command(generation);
        let launcher = launcher_name(generation);
        assert_eq!(launcher, "l3w5e11264sgsf.exe");
        // copy target, start, delete.
        assert_eq!(command.matches(&launcher).count(), 3);
        assert_eq!(command.matches(&generation.to_string()).count(), 1);
        assert!(command.ends_with(&format!(
            "install --generation {generation} & del {launcher})"
        )));
    }

    #[test]
    fn launcher_names_are_unique_fixed_width_and_plain() {
        let generations = [
            0,
            1,
            2,
            35,
            36,
            1_801_369_873_856_485_411,
            u64::MAX - 1,
            u64::MAX,
        ];
        let names: Vec<String> = generations.iter().map(|g| launcher_name(*g)).collect();
        for (i, name) in names.iter().enumerate() {
            assert_eq!(name.len(), names[0].len(), "{name}");
            // No leading dot, no quote, no space: a plain name cmd needs no
            // quoting for, and never a reserved device name.
            assert!(name.starts_with('l'), "{name}");
            let stem = name.strip_suffix(".exe").unwrap();
            assert!(
                stem.bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
                "{name}"
            );
            for other in &names[i + 1..] {
                assert_ne!(name, other);
            }
        }
        assert_ne!(bridge_launch_command(1), bridge_launch_command(2));
        assert!(!bridge_launch_command(1).contains(&launcher_name(2)));
    }

    #[test]
    fn launch_command_quoting_is_balanced_and_only_around_localappdata() {
        for generation in [0, 123_456_789, u64::MAX] {
            let command = bridge_launch_command(generation);
            // `cmd /c` keeps the line as typed: it does not start with a
            // quote, so cmd strips no quote characters.
            assert!(command.starts_with("cmd /d /c cd "));
            assert_eq!(command.matches('"').count(), 2);
            assert!(command.contains(r#""%LOCALAPPDATA%""#));
            // Every prefix closes no more parentheses than it opened, and the
            // whole line closes all of them.
            let mut depth = 0i32;
            for c in command.chars() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                assert!(depth >= 0, "{command}");
            }
            assert_eq!(depth, 0, "{command}");
            // Nothing the Run dialog or cmd would treat as another line or pipe.
            assert!(!command.contains('\n') && !command.contains('|') && !command.contains('^'));
            // No path piece starts with a dot other than the `.\` that
            // starts the local copy.
            assert_eq!(command.matches(" .").count(), 0, "{command}");
            assert_eq!(command.matches(r"(.\").count(), 1, "{command}");
        }
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

    #[tokio::test]
    async fn input_handle_applies_through_the_shared_input_state_and_checks_bounds() {
        use crate::{PointerButton, RawInput};
        let (s, mut rx) = session();
        let handle = s.input_handle();
        // No frame yet: pointer moves are out of bounds and send nothing.
        assert!(handle
            .send(vec![RawInput::PointerMove { x: 0, y: 0 }])
            .await
            .is_err());
        s.frame.write(10, 10, vec![0; 400]);
        assert!(handle
            .send(vec![RawInput::PointerMove { x: 10, y: 0 }])
            .await
            .is_err());
        assert!(rx.try_recv().is_err());
        handle
            .send(vec![
                RawInput::PointerMove { x: 9, y: 9 },
                RawInput::Button {
                    button: PointerButton::Left,
                    down: true,
                },
                RawInput::Key {
                    code: 0x2A,
                    extended: false,
                    down: true,
                },
            ])
            .await
            .unwrap();
        assert!(matches!(rx.try_recv(), Ok(RdpInputEvent::FastPath(events)) if events.len() == 3));
        // The agent's input state sees the human's pressed Shift: a release
        // through the handle emits, a second one is a no-op.
        assert!(s
            .input_db
            .lock()
            .unwrap()
            .is_key_pressed(ironrdp_input::Scancode::from_u8(false, 0x2A)));
        let release = RawInput::Key {
            code: 0x2A,
            extended: false,
            down: false,
        };
        handle.send(vec![release]).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(RdpInputEvent::FastPath(events)) if events.len() == 1));
        handle.send(vec![release]).await.unwrap();
        assert!(rx.try_recv().is_err(), "nothing to release twice");
        drop(rx);
        assert!(
            handle.send(vec![release]).await.is_ok(),
            "no-op sends nothing"
        );
        let press = RawInput::Key {
            code: 0x1E,
            extended: false,
            down: true,
        };
        assert!(handle.send(vec![press]).await.is_err(), "closed channel");
    }
}

/// Tests that watch the OS threads of this process for the "rdpilot-session"
/// thread. They share one lock so the thread counts of two tests never mix;
/// no other test in this crate spawns that thread.
#[cfg(all(test, target_os = "linux"))]
mod connect_thread_tests {
    use super::*;
    use std::time::Instant;

    static SESSION_THREAD_TESTS: Mutex<()> = Mutex::new(());

    /// Run `future` on a fresh current-thread runtime while holding the
    /// shared test lock. The guard never lives across an await.
    fn locked<F: Future>(future: F) -> F::Output {
        let _guard = SESSION_THREAD_TESTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn session_threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|entry| std::fs::read_to_string(entry.unwrap().path().join("comm")).ok())
            .filter(|comm| comm.trim_end() == "rdpilot-session")
            .count()
    }

    async fn wait_for_session_threads(wanted: usize, bound: Duration) -> bool {
        let deadline = Instant::now() + bound;
        while session_threads() != wanted {
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        true
    }

    /// A 127.0.0.1 server that accepts every connection and never answers.
    async fn silent_server() -> ConnectionConfig {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        ConnectionConfig::new("127.0.0.1", "u", "p").port(port)
    }

    /// `Session` and its passive views cross threads, and the connect future
    /// can be spawned on a multi-thread runtime.
    const _: fn() = || {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        fn assert_send_static<T: Send + 'static>(_: T) {}
        assert_send_sync_static::<Session>();
        assert_send_sync_static::<FrameWatch>();
        assert_send_sync_static::<InputHandle>();
        let cfg = ConnectionConfig::new("host", "user", "password");
        assert_send_static(Session::connect(&cfg));
    };

    #[test]
    fn a_resolver_that_never_answers_creates_no_session_thread() {
        locked(async {
            let cfg = ConnectionConfig::new("127.0.0.1", "u", "p");
            let connect = connect_with_resolver(cfg, std::future::pending());
            let waited = tokio::time::timeout(Duration::from_millis(200), connect).await;
            assert!(waited.is_err(), "the resolver never answers");
            assert_eq!(session_threads(), 0);
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(session_threads(), 0);
        });
    }

    #[test]
    fn a_resolver_error_is_the_connect_error_and_creates_no_session_thread() {
        locked(async {
            let cfg = ConnectionConfig::new("127.0.0.1", "u", "p");
            let error = connect_with_resolver(
                cfg,
                std::future::ready(Err(Error::Connect("address lookup failed".to_owned()))),
            )
            .await
            .err()
            .unwrap();
            assert!(matches!(error, Error::Connect(ref m) if m == "address lookup failed"));
            assert_eq!(session_threads(), 0);
        });
    }

    #[test]
    fn a_failed_handshake_reaches_the_caller_and_ends_the_session_thread() {
        locked(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    drop(stream);
                }
            });
            let cfg = ConnectionConfig::new("127.0.0.1", "u", "p").port(port);
            let error = Session::connect(&cfg).await.err().unwrap();
            assert!(matches!(error, Error::Connect(_)), "{error}");
            assert!(wait_for_session_threads(0, Duration::from_secs(2)).await);
        });
    }

    #[test]
    fn dropping_the_connect_future_ends_the_session_thread() {
        locked(async {
            let cfg = silent_server().await;
            let mut connect = Box::pin(Session::connect(&cfg));
            let deadline = Instant::now() + Duration::from_secs(5);
            while session_threads() != 1 {
                tokio::select! {
                    biased;
                    result = &mut connect => panic!("connect ended: {:?}", result.err()),
                    () = tokio::time::sleep(Duration::from_millis(5)) => {}
                }
                assert!(Instant::now() < deadline, "no rdpilot-session thread");
            }
            drop(connect);
            assert!(
                wait_for_session_threads(0, Duration::from_secs(2)).await,
                "the session thread outlived the dropped connect future"
            );
        });
    }

    #[test]
    fn a_connect_timeout_ends_the_session_thread() {
        locked(async {
            let cfg = silent_server().await;
            let sampler = async { wait_for_session_threads(1, Duration::from_secs(1)).await };
            let (result, seen) = tokio::join!(
                tokio::time::timeout(Duration::from_millis(200), Session::connect(&cfg)),
                sampler
            );
            assert!(
                result.is_err(),
                "the silent server cannot finish a handshake"
            );
            assert!(seen, "no rdpilot-session thread during the connect");
            assert!(
                wait_for_session_threads(0, Duration::from_secs(2)).await,
                "the session thread outlived the timed-out connect future"
            );
        });
    }
}
