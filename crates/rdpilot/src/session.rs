//! RDP connection, native recovery input/framebuffer, deployment and file transfer.
use crate::{
    bridge::BridgeShared,
    connect,
    framebuffer::{FrameWatch, SharedFrame},
    lifecycle::{EndCause, LifecycleOwner, SessionState},
    session_loop::{self, LoopCommand},
};
use crate::{
    BootstrapStage, ConnectionConfig, CuaAttachment, Error, InputEpoch, Key, KeyAction,
    MouseAction, Result, Screenshot,
};
use rdpilot_bridge_protocol::{FileTransferOp, FileTransferRequest, Message};
use std::{
    fs,
    future::Future,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{atomic::AtomicU64, Arc},
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
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
    thread: Option<JoinHandle<()>>,
    input_tx: mpsc::Sender<LoopCommand>,
    frame: SharedFrame,
    lifecycle: LifecycleOwner,
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
/// The error of an input path whose session loop is gone: the bridge's
/// failure reason if it has one.
fn input_closed(bridge: &BridgeShared) -> Error {
    Error::Session(
        bridge
            .failure()
            .unwrap_or_else(|| "input channel closed".to_owned()),
    )
}

/// Send one batch of ordered input of `epoch` to the session loop and wait
/// until the loop has accepted or refused it.
async fn send_input(
    tx: &mpsc::Sender<LoopCommand>,
    bridge: &BridgeShared,
    epoch: InputEpoch,
    ops: Vec<ironrdp_input::Operation>,
) -> Result<()> {
    let (reply, answer) = oneshot::channel();
    tx.send(LoopCommand::Input { epoch, ops, reply })
        .await
        .map_err(|_| input_closed(bridge))?;
    answer.await.map_err(|_| input_closed(bridge))?
}

/// Change the input epoch of the session loop to `epoch` and wait until the
/// loop has written the releases of the old epoch.
async fn set_epoch(
    tx: &mpsc::Sender<LoopCommand>,
    bridge: &BridgeShared,
    epoch: InputEpoch,
) -> Result<()> {
    let (reply, answer) = oneshot::channel();
    tx.send(LoopCommand::SetEpoch { epoch, reply })
        .await
        .map_err(|_| input_closed(bridge))?;
    answer.await.map_err(|_| input_closed(bridge))
}

/// What the session thread tells the caller once the handshake is over.
type Ready = Result<(Arc<BridgeShared>, LifecycleOwner)>;

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
    let (bridge, lifecycle) = ready_rx.await.map_err(|_| {
        Error::Session("session thread ended before the handshake finished".to_owned())
    })??;

    Ok(Session {
        thread: Some(thread),
        input_tx,
        frame,
        lifecycle,
        bridge,
        next_req_id: AtomicU64::new(1),
        share_root,
        cua,
    })
}

/// Body of the `rdpilot-session` thread: run the handshake, report it through
/// `ready_tx`, then run the session loop. The handshake ends early when the
/// caller drops `ready_tx`'s receiver. A loop error is reported once, as the
/// [`EndCause::Error`] of the session's lifecycle.
fn run_session_thread(
    cfg: ConnectionConfig,
    addr: SocketAddr,
    frame: SharedFrame,
    input_rx: mpsc::Receiver<LoopCommand>,
    mut ready_tx: oneshot::Sender<Ready>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = ready_tx.send(Err(Error::Session(format!(
                "could not start session runtime: {e}"
            ))));
            return;
        }
    };
    runtime.block_on(async move {
        let connected = tokio::select! {
            result = connect::connect(&cfg, addr) => Some(result),
            () = ready_tx.closed() => None,
        };
        let (connection_result, framed, bridge) = match connected {
            None => return,
            Some(Err(error)) => {
                let _ = ready_tx.send(Err(error));
                return;
            }
            Some(Ok(connected)) => connected,
        };
        let desktop_size = (
            u32::from(connection_result.desktop_size.width),
            u32::from(connection_result.desktop_size.height),
        );
        let lifecycle = LifecycleOwner::new(frame.clone(), desktop_size);
        // A caller that is gone drops `framed`, which closes the connection.
        if ready_tx
            .send(Ok((bridge.clone(), lifecycle.clone())))
            .is_err()
        {
            return;
        }
        // `run` ends the lifecycle (and so the frame), which passive
        // observers (the live viewer) see while the registry entry exists.
        let result = session_loop::run(
            framed,
            connection_result,
            input_rx,
            frame,
            bridge.clone(),
            lifecycle,
        )
        .await;
        if let Err(error) = &result {
            tracing::error!(%error,"RDP session loop ended");
            bridge.invalidate(&error.to_string());
        }
    });
}

/// The entry point of the session thread. It takes the whole owned config.
type SessionThreadBody = fn(
    ConnectionConfig,
    SocketAddr,
    SharedFrame,
    mpsc::Receiver<LoopCommand>,
    oneshot::Sender<Ready>,
);

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

    /// The desktop size of the current activation. It follows a server
    /// reactivation to a new size.
    pub fn desktop_size(&self) -> (u32, u32) {
        self.lifecycle.desktop_size()
    }

    /// A receiver of this session's lifecycle and current desktop size. It
    /// starts at [`Lifecycle::Live`](crate::Lifecycle::Live) and changes to
    /// [`Lifecycle::Ended`](crate::Lifecycle::Ended) once, with the first end
    /// cause: the server ended the session, the session loop failed, or the
    /// session was closed or dropped. The frame watch reports `ended` from the
    /// same transition, after it.
    #[must_use]
    pub fn lifecycle(&self) -> watch::Receiver<SessionState> {
        self.lifecycle.subscribe()
    }

    /// A passive, read-only observer of this session's framebuffer. It holds
    /// only the shared frame (never the session), carries no input, Cua or
    /// transfer capability, and reports `ended` once the RDP session loop
    /// returns or the session is closed or dropped.
    #[must_use]
    pub fn frame_watch(&self) -> FrameWatch {
        self.frame.watch()
    }

    /// A `Send` input path for a daemon-side human viewer. Its events go
    /// through the same session loop input state and input epoch as
    /// [`Session::send_mouse`] and [`Session::send_key`], so pressed keys and
    /// buttons stay consistent between the two. It holds no session lock and
    /// does not keep the session alive: after close, sending fails.
    #[must_use]
    pub fn input_handle(&self) -> InputHandle {
        InputHandle {
            tx: self.input_tx.clone(),
            frame: self.frame.clone(),
            bridge: Arc::clone(&self.bridge),
        }
    }

    fn check_bounds(&self, action: &MouseAction) -> Result<()> {
        let (w, h) = self.desktop_size();
        for (x, y) in action.coordinates() {
            let (x, y) = (u32::from(x), u32::from(y));
            if x >= w || y >= h {
                return Err(Error::coordinate_out_of_bounds(x, y, w, h));
            }
        }
        Ok(())
    }

    /// Send a mouse action as input of `epoch`.
    ///
    /// Each timed batch of the action waits until the session loop has
    /// accepted it; the first refused batch ends the action, so no later
    /// batch is sent.
    ///
    /// # Errors
    ///
    /// [`Error::CoordinateOutOfBounds`] for a coordinate outside the desktop
    /// (nothing is sent); [`Error::StaleInput`] when `epoch` is older than
    /// the session's input epoch; [`Error::Session`] when the session loop
    /// is gone.
    pub async fn send_mouse(&self, action: MouseAction, epoch: InputEpoch) -> Result<()> {
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
            send_input(&self.input_tx, &self.bridge, epoch, batch).await?;

            if let Some(gap) = gap {
                if i != last_index {
                    tokio::time::sleep(gap).await;
                }
            }
        }

        Ok(())
    }

    /// Send a key action as input of `epoch`.
    ///
    /// # Errors
    ///
    /// [`Error::StaleInput`] when `epoch` is older than the session's input
    /// epoch; [`Error::Session`] when the session loop is gone.
    pub async fn send_key(&self, action: KeyAction, epoch: InputEpoch) -> Result<()> {
        send_input(
            &self.input_tx,
            &self.bridge,
            epoch,
            crate::input::key_operations(&action),
        )
        .await
    }

    /// Change the session's input epoch to `epoch`. When `epoch` is newer,
    /// the session loop first releases every key and button still pressed;
    /// this returns after those releases are written. An epoch that is not
    /// newer changes nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Session`] when the session loop is gone.
    pub async fn set_input_epoch(&self, epoch: InputEpoch) -> Result<()> {
        set_epoch(&self.input_tx, &self.bridge, epoch).await
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

    /// Close the session and wait (at most 15 s) for its thread to end.
    ///
    /// The lifecycle ends as [`EndCause::ClientClosed`] unless it has already
    /// ended. A session that already ended with an error closes with `Ok`: the
    /// error is reported once, as the end cause of its lifecycle.
    ///
    /// # Errors
    ///
    /// [`Error::Session`] when the thread does not end within the deadline,
    /// the join task fails or the thread panicked.
    pub async fn close(mut self) -> Result<()> {
        self.lifecycle.end(EndCause::ClientClosed);
        self.bridge.stop("session closed");

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
            .map_err(|_| Error::Session("session thread panicked".to_owned()))
        } else {
            Ok(())
        }
    }
}

/// See [`Session::input_handle`].
#[derive(Clone)]
pub struct InputHandle {
    tx: mpsc::Sender<LoopCommand>,
    frame: SharedFrame,
    bridge: Arc<BridgeShared>,
}

impl std::fmt::Debug for InputHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputHandle").finish_non_exhaustive()
    }
}

impl InputHandle {
    /// Send `events` in order as input of `epoch` and wait until the session
    /// loop has accepted them. Pointer moves outside the current frame are
    /// refused before anything is sent.
    ///
    /// # Errors
    ///
    /// [`Error::CoordinateOutOfBounds`] for a move outside the frame (or
    /// before the first frame); [`Error::StaleInput`] when `epoch` is older
    /// than the session's input epoch; [`Error::Session`] when the session's
    /// input channel is closed.
    pub async fn send(&self, events: Vec<crate::RawInput>, epoch: InputEpoch) -> Result<()> {
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
        if ops.is_empty() {
            return Ok(());
        }
        send_input(&self.tx, &self.bridge, epoch, ops).await
    }

    /// Change the session's input epoch, as [`Session::set_input_epoch`]
    /// does.
    ///
    /// # Errors
    ///
    /// [`Error::Session`] when the session's input channel is closed.
    pub async fn set_epoch(&self, epoch: InputEpoch) -> Result<()> {
        set_epoch(&self.tx, &self.bridge, epoch).await
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.lifecycle.end(EndCause::ClientClosed);
        self.bridge.stop("session dropped");
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
    /// The launch keys are input of `epoch`; a refused key ends the launch
    /// with that error.
    pub async fn deploy_and_launch(&self, epoch: InputEpoch) -> Result<Duration> {
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
            self.inject_bootstrap(epoch).await?;
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
    async fn inject_bootstrap(&self, epoch: InputEpoch) -> Result<()> {
        // Retrying before Ready does not replay MCP. The guest generation mutex
        // makes repeated launches no-ops while their owner is alive.
        self.bridge
            .bootstrap
            .record(BootstrapStage::LaunchInputAttempted);
        self.send_key(KeyAction::Combo(vec![Key::Win, Key::R]), epoch)
            .await?;
        tokio::time::sleep(RUN_DIALOG_SETTLE).await;
        self.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A]), epoch)
            .await?;
        let command = bridge_launch_command(self.bridge.generation);
        for chunk in chunk_str(&command, TYPE_CHUNK_LEN) {
            self.send_key(KeyAction::Type(chunk), epoch).await?;
            tokio::time::sleep(TYPE_CHUNK_GAP).await;
        }
        self.send_key(KeyAction::Combo(vec![Key::Enter]), epoch)
            .await?;
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
    use ironrdp::pdu::input::fast_path::{FastPathInput, FastPathInputEvent, KeyboardFlags};
    use session_loop::tests::{Peer, PEER_DEADLINE};

    fn session() -> (Session, mpsc::Receiver<LoopCommand>) {
        let (tx, rx) = mpsc::channel(16);
        let frame = SharedFrame::new();
        (
            Session {
                thread: None,
                input_tx: tx,
                lifecycle: LifecycleOwner::new(frame.clone(), (100, 100)),
                frame,
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
        let (sent, ops) = tokio::join!(
            s.send_key(
                KeyAction::Combo(vec![Key::Win, Key::R]),
                InputEpoch::INITIAL
            ),
            accept_next(&mut rx)
        );
        sent.unwrap();
        assert!(!ops.is_empty());
        let (sent, ops) = tokio::join!(
            s.send_mouse(MouseAction::Move { x: 10, y: 10 }, InputEpoch::INITIAL),
            accept_next(&mut rx)
        );
        sent.unwrap();
        assert!(!ops.is_empty());
        assert!(s.attach_cua().await.is_err());
    }
    #[tokio::test]
    async fn invalid_native_coordinates_send_nothing() {
        let (s, mut rx) = session();
        assert!(matches!(
            s.send_mouse(MouseAction::Move { x: 100, y: 10 }, InputEpoch::INITIAL)
                .await,
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
        let lifecycle = s.lifecycle();
        assert!(!watch.status().ended);
        assert_eq!(lifecycle.borrow().lifecycle, crate::Lifecycle::Live);
        drop(s);
        assert!(watch.status().ended);
        assert_eq!(
            lifecycle.borrow().lifecycle,
            crate::Lifecycle::Ended(EndCause::ClientClosed)
        );
    }

    #[tokio::test]
    async fn close_ends_the_lifecycle_as_client_closed() {
        let (s, _rx) = session();
        let watch = s.frame_watch();
        let lifecycle = s.lifecycle();
        s.close().await.unwrap();
        assert!(watch.status().ended);
        assert_eq!(
            lifecycle.borrow().lifecycle,
            crate::Lifecycle::Ended(EndCause::ClientClosed)
        );
    }

    /// A session whose loop runs on the local set against a scripted peer.
    fn pumped_session() -> (
        Session,
        session_loop::tests::Peer,
        tokio::task::JoinHandle<Result<()>>,
    ) {
        let pump = session_loop::tests::spawn_pump();
        (
            Session {
                thread: None,
                input_tx: pump.input_tx,
                frame: pump.frame,
                lifecycle: pump.lifecycle,
                bridge: pump.bridge,
                next_req_id: AtomicU64::new(1),
                share_root: None,
                cua: true,
            },
            pump.peer,
            pump.task,
        )
    }

    #[tokio::test]
    async fn desktop_size_and_bounds_follow_a_reactivation() {
        use ironrdp::pdu::input::fast_path::FastPathInput;
        use session_loop::tests::{serve_reactivation, PEER_DEADLINE};
        tokio::task::LocalSet::new()
            .run_until(async {
                let (s, mut peer, task) = pumped_session();
                assert_eq!(s.desktop_size(), (100, 100));
                serve_reactivation(&mut peer, true, 200, 50)
                    .await
                    .expect("reactivation served");
                let mut rx = s.lifecycle();
                tokio::time::timeout(
                    PEER_DEADLINE,
                    rx.wait_for(|st| st.desktop_size == (200, 50)),
                )
                .await
                .expect("the new size is published")
                .expect("the lifecycle sender lives");
                assert_eq!(s.desktop_size(), (200, 50));
                // Inside the new size, outside the old one: accepted and sent.
                s.send_mouse(MouseAction::Move { x: 150, y: 20 }, InputEpoch::INITIAL)
                    .await
                    .expect("inside the new size");
                let (_, frame) = tokio::time::timeout(PEER_DEADLINE, peer.read_pdu())
                    .await
                    .expect("input frame deadline")
                    .expect("input frame read");
                let pdu: FastPathInput = ironrdp::core::decode(&frame).expect("fast-path input");
                assert!(!pdu.input_events().is_empty());
                // Inside the old size, outside the new one: refused.
                assert!(matches!(
                    s.send_mouse(MouseAction::Move { x: 50, y: 80 }, InputEpoch::INITIAL)
                        .await,
                    Err(Error::CoordinateOutOfBounds { .. })
                ));
                s.close().await.unwrap();
                let joined = tokio::time::timeout(PEER_DEADLINE, task)
                    .await
                    .expect("pump join deadline")
                    .expect("pump task joins");
                drop(joined);
            })
            .await;
    }

    #[tokio::test]
    async fn close_after_a_loop_error_is_ok() {
        let (input_tx, input_rx) = mpsc::channel(16);
        let (client, server) = tokio::io::duplex(1024);
        drop(server);
        let framed =
            ironrdp_tokio::TokioFramed::new(Box::new(client) as crate::connect::UpgradedStream);
        let frame = SharedFrame::new();
        let lifecycle = LifecycleOwner::new(frame.clone(), (100, 100));
        let bridge = Arc::new(BridgeShared::new());
        let thread = {
            let (frame, lifecycle, bridge) = (frame.clone(), lifecycle.clone(), bridge.clone());
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let result = session_loop::run(
                        framed,
                        session_loop::tests::connection_result_fixture(),
                        input_rx,
                        frame,
                        bridge.clone(),
                        lifecycle,
                    )
                    .await;
                    if let Err(error) = &result {
                        bridge.invalidate(&error.to_string());
                    }
                });
            })
        };
        let s = Session {
            thread: Some(thread),
            input_tx,
            frame,
            lifecycle,
            bridge,
            next_req_id: AtomicU64::new(1),
            share_root: None,
            cua: true,
        };
        let mut rx = s.lifecycle();
        tokio::time::timeout(
            Duration::from_secs(5),
            rx.wait_for(|st| st.lifecycle != crate::Lifecycle::Live),
        )
        .await
        .expect("the loop ends")
        .expect("the lifecycle sender lives");
        s.close()
            .await
            .expect("close of an error-ended session is Ok");
        assert!(matches!(
            rx.borrow().lifecycle,
            crate::Lifecycle::Ended(EndCause::Error(_))
        ));
    }

    /// Answer the next loop command, which must be an input batch, as
    /// accepted, and return its operations.
    async fn accept_next(rx: &mut mpsc::Receiver<LoopCommand>) -> Vec<ironrdp_input::Operation> {
        match rx.recv().await {
            Some(LoopCommand::Input { ops, reply, .. }) => {
                let _ = reply.send(Ok(()));
                ops
            }
            _ => panic!("expected an input batch"),
        }
    }

    /// The input events of the next client PDU the peer reads.
    async fn next_input(peer: &mut Peer) -> Vec<FastPathInputEvent> {
        let (_, frame) = tokio::time::timeout(PEER_DEADLINE, peer.read_pdu())
            .await
            .expect("input frame deadline")
            .expect("input frame read");
        input_events(&frame)
    }

    /// The input events of the PDU the peer reads within 200 ms, or `None`
    /// when the loop has written none.
    async fn written(peer: &mut Peer) -> Option<Vec<FastPathInputEvent>> {
        let (_, frame) = tokio::time::timeout(Duration::from_millis(200), peer.read_pdu())
            .await
            .ok()?
            .expect("input frame read");
        Some(input_events(&frame))
    }

    /// The input events of one fast-path input PDU.
    fn input_events(frame: &[u8]) -> Vec<FastPathInputEvent> {
        let pdu: FastPathInput = ironrdp::core::decode(frame).expect("fast-path input");
        pdu.input_events().to_vec()
    }

    const SHIFT: u8 = 0x2A;
    const A_KEY: u8 = 0x1E;

    fn key(code: u8, down: bool) -> crate::RawInput {
        crate::RawInput::Key {
            code,
            extended: false,
            down,
        }
    }

    fn key_event(code: u8, down: bool) -> FastPathInputEvent {
        let flags = if down {
            KeyboardFlags::empty()
        } else {
            KeyboardFlags::RELEASE
        };
        FastPathInputEvent::KeyboardEvent(flags, code)
    }

    fn left_button(down: bool) -> crate::RawInput {
        crate::RawInput::Button {
            button: crate::PointerButton::Left,
            down,
        }
    }

    fn is_left_release(event: &FastPathInputEvent) -> bool {
        use ironrdp::pdu::input::mouse::PointerFlags;
        matches!(event, FastPathInputEvent::MouseEvent(pdu)
            if pdu.flags.contains(PointerFlags::LEFT_BUTTON)
                && !pdu.flags.contains(PointerFlags::DOWN))
    }

    fn is_stale(result: &Result<()>, sent: InputEpoch, current: InputEpoch) -> bool {
        matches!(result, Err(Error::StaleInput { sent: s, current: c }) if *s == sent && *c == current)
    }

    /// Run `test` with a pumped session on a local set, then close it.
    async fn with_pumped<F, Fut>(test: F)
    where
        F: FnOnce(Session, Peer) -> Fut,
        Fut: Future<Output = (Session, Peer)>,
    {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (s, peer, task) = pumped_session();
                let (s, _peer) = test(s, peer).await;
                s.close().await.unwrap();
                let joined = tokio::time::timeout(PEER_DEADLINE, task)
                    .await
                    .expect("pump join deadline")
                    .expect("pump task joins");
                assert!(joined.is_ok(), "{joined:?}");
            })
            .await;
    }

    /// The close and drop of a session reach the loop when its command queue
    /// is full: the loop sits in a write the peer does not read, every slot
    /// of the queue is taken, and the loop still ends.
    async fn ends_with_a_full_queue(close: bool) {
        tokio::task::LocalSet::new()
            .run_until(async {
                let pump = session_loop::tests::spawn_pump_with_buffer(1024);
                let mut peer = pump.peer;
                let task = pump.task;
                let s = Session {
                    thread: None,
                    input_tx: pump.input_tx,
                    frame: pump.frame,
                    lifecycle: pump.lifecycle,
                    bridge: pump.bridge,
                    next_req_id: AtomicU64::new(1),
                    share_root: None,
                    cua: true,
                };
                let lifecycle = s.lifecycle();
                // A long Type stream the 1 KiB duplex cannot hold.
                let stuck = s.send_key(KeyAction::Type("x".repeat(2_000)), InputEpoch::INITIAL);
                stuck.await.expect("accepted at dequeue, before the write");
                let mut queued = 0;
                loop {
                    let (reply, _answer) = oneshot::channel();
                    let command = LoopCommand::Input {
                        epoch: InputEpoch::INITIAL,
                        ops: crate::input::key_operations(&KeyAction::Type("y".to_owned())),
                        reply,
                    };
                    match s.input_tx.try_send(command) {
                        Ok(()) => queued += 1,
                        Err(mpsc::error::TrySendError::Full(_)) => break,
                        Err(other) => panic!("queue closed: {other}"),
                    }
                }
                assert_eq!(queued, INPUT_CHANNEL_CAPACITY);
                if close {
                    s.close().await.unwrap();
                } else {
                    drop(s);
                }
                // The peer reads again, so the loop leaves its write.
                let drain =
                    tokio::task::spawn_local(async move { while peer.read_pdu().await.is_ok() {} });
                let joined = tokio::time::timeout(Duration::from_secs(15), task)
                    .await
                    .expect("the loop ends within the close bound")
                    .expect("pump task joins");
                assert!(joined.is_ok(), "{joined:?}");
                assert_eq!(
                    lifecycle.borrow().lifecycle,
                    crate::Lifecycle::Ended(EndCause::ClientClosed)
                );
                drain.abort();
            })
            .await;
    }

    #[tokio::test]
    async fn close_is_not_lost_with_a_full_queue() {
        ends_with_a_full_queue(true).await;
    }

    #[tokio::test]
    async fn drop_is_not_lost_with_a_full_queue() {
        ends_with_a_full_queue(false).await;
    }

    #[tokio::test]
    async fn stale_input_is_rejected_at_dequeue_and_leaves_no_press() {
        with_pumped(|s, mut peer| async move {
            let n = InputEpoch::INITIAL;
            let (n1, n2) = (n.next(), n.next().next());
            let handle = s.input_handle();
            s.set_input_epoch(n1).await.unwrap();
            let refused = s.send_key(KeyAction::Combo(vec![Key::Shift]), n).await;
            assert!(is_stale(&refused, n, n1), "{refused:?}");
            let refused = handle.send(vec![left_button(true)], n).await;
            assert!(is_stale(&refused, n, n1), "{refused:?}");
            // Nothing of the refused input is held: a change writes nothing.
            s.set_input_epoch(n2).await.unwrap();
            handle.send(vec![key(A_KEY, true)], n2).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(A_KEY, true)]);
            (s, peer)
        })
        .await;
    }

    #[tokio::test]
    async fn an_epoch_change_releases_held_keys_and_buttons_before_confirming() {
        with_pumped(|s, mut peer| async move {
            let n = InputEpoch::INITIAL;
            let handle = s.input_handle();
            handle
                .send(vec![key(SHIFT, true), left_button(true)], n)
                .await
                .unwrap();
            assert_eq!(next_input(&mut peer).await.len(), 2);
            s.set_input_epoch(n.next()).await.unwrap();
            // Confirmed: the releases are already written, buttons first.
            let releases = written(&mut peer).await.expect("releases written");
            assert_eq!(releases.len(), 2, "{releases:?}");
            assert!(is_left_release(&releases[0]), "{releases:?}");
            assert_eq!(releases[1], key_event(SHIFT, false));
            handle.send(vec![key(A_KEY, true)], n.next()).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(A_KEY, true)]);
            (s, peer)
        })
        .await;
    }

    #[tokio::test]
    async fn a_drag_cut_by_an_epoch_change_stops_at_the_first_stale_batch() {
        with_pumped(|s, mut peer| async move {
            let n = InputEpoch::INITIAL;
            let s = std::rc::Rc::new(s);
            let drag = tokio::task::spawn_local({
                let s = std::rc::Rc::clone(&s);
                async move {
                    s.send_mouse(
                        MouseAction::Drag {
                            from_x: 10,
                            from_y: 10,
                            to_x: 90,
                            to_y: 90,
                            button: crate::Button::Left,
                        },
                        n,
                    )
                    .await
                }
            });
            // The first batch: the move to the start and the press.
            assert_eq!(next_input(&mut peer).await.len(), 2);
            s.set_input_epoch(n.next()).await.unwrap();
            let result = drag.await.unwrap();
            assert!(is_stale(&result, n, n.next()), "{result:?}");
            // Moves the drag sent before the change, then the release of
            // the change; no drag batch after it.
            let mut released = false;
            while let Some(events) = written(&mut peer).await {
                assert!(!released, "a drag batch after the release: {events:?}");
                released = events.iter().any(is_left_release);
            }
            assert!(released);
            let s = std::rc::Rc::try_unwrap(s)
                .ok()
                .expect("the drag task is done");
            (s, peer)
        })
        .await;
    }

    #[tokio::test]
    async fn a_newer_epoch_acts_as_an_epoch_change() {
        with_pumped(|s, mut peer| async move {
            let n = InputEpoch::INITIAL;
            let handle = s.input_handle();
            handle.send(vec![key(SHIFT, true)], n).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(SHIFT, true)]);
            // Input of a newer epoch without a change: the release comes
            // first, in the same write.
            handle.send(vec![key(A_KEY, true)], n.next()).await.unwrap();
            assert_eq!(
                next_input(&mut peer).await,
                vec![key_event(SHIFT, false), key_event(A_KEY, true)]
            );
            let refused = handle.send(vec![key(A_KEY, false)], n).await;
            assert!(is_stale(&refused, n, n.next()), "{refused:?}");
            (s, peer)
        })
        .await;
    }

    #[tokio::test]
    async fn an_epoch_change_that_is_not_newer_is_an_idempotent_ok() {
        with_pumped(|s, mut peer| async move {
            let n1 = InputEpoch::INITIAL.next();
            let handle = s.input_handle();
            s.set_input_epoch(n1).await.unwrap();
            handle.send(vec![key(SHIFT, true)], n1).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(SHIFT, true)]);
            s.set_input_epoch(n1).await.unwrap();
            s.set_input_epoch(InputEpoch::INITIAL).await.unwrap();
            assert_eq!(written(&mut peer).await, None, "no release");
            handle.send(vec![key(SHIFT, false)], n1).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(SHIFT, false)]);
            (s, peer)
        })
        .await;
    }

    #[tokio::test]
    async fn input_handle_and_session_share_the_loop_input_state_and_check_bounds() {
        with_pumped(|s, mut peer| async move {
            let n = InputEpoch::INITIAL;
            let handle = s.input_handle();
            // No frame yet: pointer moves are out of bounds and send nothing.
            assert!(matches!(
                handle
                    .send(vec![crate::RawInput::PointerMove { x: 0, y: 0 }], n)
                    .await,
                Err(Error::CoordinateOutOfBounds { .. })
            ));
            s.frame.write(10, 10, vec![0; 400]);
            assert!(matches!(
                handle
                    .send(vec![crate::RawInput::PointerMove { x: 10, y: 0 }], n)
                    .await,
                Err(Error::CoordinateOutOfBounds { .. })
            ));
            assert_eq!(written(&mut peer).await, None);
            handle.send(vec![key(SHIFT, true)], n).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(SHIFT, true)]);
            // The session's Shift press sees the handle's: a repeat press
            // (release, press), then its release.
            s.send_key(KeyAction::Combo(vec![Key::Shift]), n)
                .await
                .unwrap();
            assert_eq!(
                next_input(&mut peer).await,
                vec![
                    key_event(SHIFT, false),
                    key_event(SHIFT, true),
                    key_event(SHIFT, false)
                ]
            );
            // Nothing is left to release.
            handle.send(vec![key(SHIFT, false)], n).await.unwrap();
            handle.send(vec![key(A_KEY, true)], n).await.unwrap();
            assert_eq!(next_input(&mut peer).await, vec![key_event(A_KEY, true)]);
            (s, peer)
        })
        .await;
    }

    #[tokio::test]
    async fn input_after_close_fails() {
        let (s, rx) = session();
        let handle = s.input_handle();
        drop(rx);
        assert!(matches!(
            handle
                .send(vec![key(A_KEY, true)], InputEpoch::INITIAL)
                .await,
            Err(Error::Session(_))
        ));
        assert!(matches!(
            handle.set_epoch(InputEpoch::INITIAL.next()).await,
            Err(Error::Session(_))
        ));
        // A send with nothing to apply is not sent at all.
        assert!(handle.send(vec![], InputEpoch::INITIAL).await.is_ok());
    }
}

/// Tests that watch the OS threads of this process for the "rdpilot-session"
/// thread. They share one lock so the thread counts of two tests never mix;
/// no other test in this crate spawns that thread.
#[cfg(all(test, target_os = "linux"))]
mod connect_thread_tests {
    use super::*;
    use std::sync::Mutex;
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
