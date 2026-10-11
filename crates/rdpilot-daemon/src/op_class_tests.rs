//! Operation classes of one session without a per-session lock: reads, agent
//! input, the human take, the transfer worker and close, through the
//! registry and the IPC dispatch.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdpilot::InputEpoch;
use rdpilot_ipc::{Request, SessionId, WireErrorCode, WireResponse};
use rdpilot_vocab::{PointerButton, RawInput};

use crate::registry::{Registry, ViewerControl};
use crate::seams::{
    BoxFuture, DaemonError, HumanInput, LoopInputModel, ManagedSession, NoopReconciliationSink,
    SessionConnector,
};

/// The small bound every "at once" answer must meet.
pub(crate) const BOUND: Duration = Duration::from_millis(500);

/// What one fake session did, and the gates that hold its operations.
pub(crate) struct Ops {
    /// Which session generation this is (the connect count).
    pub(crate) number: usize,
    /// Starts and ends of transfer jobs, in order.
    log: Mutex<Vec<String>>,
    running: AtomicUsize,
    max_running: AtomicUsize,
    /// Holds "hold" transfers and the bootstrap.
    pub(crate) gate: tokio::sync::Semaphore,
    /// Holds native agent input between its first and second batch.
    pub(crate) input_gate: tokio::sync::Semaphore,
    hold_input: AtomicBool,
    /// Holds reads (screenshots).
    pub(crate) read_gate: tokio::sync::Semaphore,
    hold_reads: AtomicBool,
    /// Holds "sync", "copy" and "stage" transfers on their blocking thread.
    sync: Mutex<std::sync::mpsc::Receiver<()>>,
    release_sync: Mutex<std::sync::mpsc::Sender<()>>,
    /// The session loop's input state, and what reached the peer.
    model: Mutex<LoopInputModel>,
    peer: Mutex<Vec<RawInput>>,
    closed: tokio::sync::watch::Sender<bool>,
    closes: AtomicUsize,
    /// A staged file was dropped while its thread panicked.
    pub(crate) dropped_panicking: AtomicBool,
}

impl Ops {
    pub(crate) fn new(number: usize) -> Self {
        let (release_sync, sync) = std::sync::mpsc::channel();
        Ops {
            number,
            log: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            max_running: AtomicUsize::new(0),
            gate: tokio::sync::Semaphore::new(0),
            input_gate: tokio::sync::Semaphore::new(0),
            hold_input: AtomicBool::new(false),
            read_gate: tokio::sync::Semaphore::new(0),
            hold_reads: AtomicBool::new(false),
            sync: Mutex::new(sync),
            release_sync: Mutex::new(release_sync),
            model: Mutex::new(LoopInputModel::default()),
            peer: Mutex::new(Vec::new()),
            closed: tokio::sync::watch::channel(false).0,
            closes: AtomicUsize::new(0),
            dropped_panicking: AtomicBool::new(false),
        }
    }

    pub(crate) fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn note(&self, line: String) {
        self.log.lock().unwrap().push(line);
    }

    pub(crate) fn started(&self, label: &str) -> bool {
        self.log().contains(&format!("start {label}"))
    }

    pub(crate) fn left(&self, label: &str) -> bool {
        self.log().contains(&format!("left {label}"))
    }

    pub(crate) fn max_running(&self) -> usize {
        self.max_running.load(Ordering::SeqCst)
    }

    pub(crate) fn release_sync(&self) {
        self.release_sync.lock().unwrap().send(()).unwrap();
    }

    fn wait_sync(&self) {
        let _ = self.sync.lock().unwrap().recv();
    }

    pub(crate) fn peer(&self) -> Vec<RawInput> {
        self.peer.lock().unwrap().clone()
    }

    fn apply(&self, epoch: InputEpoch, events: Vec<RawInput>) -> Result<(), DaemonError> {
        let sent = self.model.lock().unwrap().send(epoch, events)?;
        self.peer.lock().unwrap().extend(sent);
        Ok(())
    }

    /// A native agent action: a press, the input gate when held, a move and
    /// the release, each its own batch, refused at the first stale one.
    async fn act(&self, epoch: InputEpoch) -> Result<(), DaemonError> {
        self.apply(epoch, vec![left(true)])?;
        if self.hold_input.load(Ordering::SeqCst) {
            self.input_gate.acquire().await.unwrap().forget();
        }
        self.apply(epoch, vec![RawInput::PointerMove { x: 1, y: 1 }])?;
        self.apply(epoch, vec![left(false)])
    }

    /// One transfer job named `label`; see the module tests for the forms.
    async fn transfer(&self, label: String) -> Result<rdpilot::TransferOutcome, DaemonError> {
        let _running = Running::enter(self, &label);
        if label.starts_with("hold") {
            self.gate.acquire().await.unwrap().forget();
        } else if label.starts_with("sync") {
            self.wait_sync();
        } else if let Some(path) = label.strip_prefix("copy:") {
            copy_and_hash(Path::new(path));
            self.wait_sync();
        } else if let Some(path) = label.strip_prefix("stage:") {
            let _staged = Staged(PathBuf::from(path), self);
            std::fs::write(path, b"staged").unwrap();
            self.wait_sync();
            // The bridge request after the synchronous step has a timer.
            tokio::time::sleep(Duration::from_secs(30)).await;
        } else if label.starts_with("fail") {
            // Fails as a stopped bridge does: when the session closes.
            let _ = self.closed.subscribe().wait_for(|c| *c).await;
            return Err(DaemonError::Sdk(rdpilot::Error::Session(
                "bridge closed before replying".into(),
            )));
        }
        Ok(rdpilot::TransferOutcome {
            bytes_transferred: 0,
            checksum: label,
        })
    }
}

/// A local copy and sha256 of both files, as a Put stages a file.
fn copy_and_hash(source: &Path) {
    use sha2::Digest;
    let target = source.with_extension("copy");
    std::fs::copy(source, &target).unwrap();
    for path in [source, target.as_path()] {
        use std::io::Read as _;
        let mut hasher = sha2::Sha256::new();
        let mut file = std::fs::File::open(path).unwrap();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let _ = hasher.finalize();
    }
}

/// Counts a running job and logs its start and its end (also when dropped).
struct Running<'a>(&'a Ops, String);

impl<'a> Running<'a> {
    fn enter(ops: &'a Ops, label: &str) -> Self {
        let now = ops.running.fetch_add(1, Ordering::SeqCst) + 1;
        ops.max_running.fetch_max(now, Ordering::SeqCst);
        ops.note(format!("start {label}"));
        Running(ops, label.to_owned())
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
        self.0.note(format!("left {}", self.1));
    }
}

/// A staged file, removed when dropped.
struct Staged<'a>(PathBuf, &'a Ops);

impl Drop for Staged<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.1.dropped_panicking.store(true, Ordering::SeqCst);
        }
        let _ = std::fs::remove_file(&self.0);
    }
}

impl HumanInput for Ops {
    fn send(
        &self,
        events: Vec<RawInput>,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        let applied = self.apply(epoch, events);
        Box::pin(async move { applied })
    }

    fn set_epoch(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        let releases = self.model.lock().unwrap().set_epoch(epoch);
        self.peer.lock().unwrap().extend(releases);
        Box::pin(async { Ok(()) })
    }
}

pub(crate) struct OpSession(pub(crate) Arc<Ops>);

impl ManagedSession for OpSession {
    fn close(&self) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.0.closed.send_replace(true);
        Box::pin(async move {
            self.0.closes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async move {
            if self.0.hold_reads.load(Ordering::SeqCst) {
                self.0.note("start read".to_owned());
                self.0.read_gate.acquire().await.unwrap().forget();
            }
            Ok(rdpilot::Screenshot::from_rgba(1, 1, vec![0; 4])?)
        })
    }
    fn send_mouse(
        &self,
        _: rdpilot::MouseAction,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(self.0.act(epoch))
    }
    fn send_key(
        &self,
        _: rdpilot::KeyAction,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(self.0.act(epoch))
    }
    fn upload_file(
        &self,
        _: PathBuf,
        remote_name: String,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(self.0.transfer(remote_name))
    }
    fn download_file(
        &self,
        remote_name: String,
        _: PathBuf,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(self.0.transfer(remote_name))
    }
    fn ping(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn desktop_size(&self) -> (u32, u32) {
        (100, 100)
    }
    fn deploy_and_launch(&self, _: InputEpoch) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async move {
            let _running = Running::enter(&self.0, "bootstrap");
            self.0.gate.acquire().await.unwrap().forget();
            Ok(Duration::ZERO)
        })
    }
    fn human_input(&self) -> Option<Arc<dyn HumanInput>> {
        Some(Arc::clone(&self.0) as Arc<dyn HumanInput>)
    }
}

/// Connects a new [`OpSession`] each time and keeps its [`Ops`].
#[derive(Default)]
pub(crate) struct OpConnector {
    made: Mutex<VecDeque<Arc<Ops>>>,
}

impl OpConnector {
    pub(crate) fn ops(&self, n: usize) -> Arc<Ops> {
        Arc::clone(&self.made.lock().unwrap()[n])
    }
}

impl SessionConnector for OpConnector {
    fn connect(
        &self,
        _: rdpilot::ConnectionConfig,
    ) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        let mut made = self.made.lock().unwrap();
        let ops = Arc::new(Ops::new(made.len()));
        made.push_back(Arc::clone(&ops));
        Box::pin(async move { Ok(Box::new(OpSession(ops)) as Box<dyn ManagedSession>) })
    }
}

pub(crate) async fn opened() -> (Arc<Registry>, Arc<OpConnector>, SessionId) {
    let connector = Arc::new(OpConnector::default());
    let registry = Arc::new(Registry::new(
        Arc::clone(&connector) as Arc<dyn SessionConnector>,
        Arc::new(NoopReconciliationSink),
    ));
    let id = connect(&registry).await;
    (registry, connector, id)
}

async fn connect(registry: &Registry) -> SessionId {
    registry
        .open(
            Some("web".into()),
            "h".into(),
            rdpilot::ConnectionConfig::new("h", "u", "p"),
        )
        .await
        .unwrap()
}

fn left(down: bool) -> RawInput {
    RawInput::Button {
        button: PointerButton::Left,
        down,
    }
}

fn tab(last: u8) -> std::net::IpAddr {
    std::net::IpAddr::from([100, 64, 0, last])
}

fn put(id: &SessionId, label: &str) -> Request {
    Request::Put {
        session: id.clone(),
        local_path: "unused".into(),
        remote_name: label.into(),
    }
}

fn get(id: &SessionId, label: &str) -> Request {
    Request::Get {
        session: id.clone(),
        remote_name: label.into(),
        local_path: "unused".into(),
    }
}

fn mouse(id: &SessionId) -> Request {
    Request::Mouse {
        session: id.clone(),
        action: rdpilot::MouseAction::Move { x: 1, y: 1 },
    }
}

fn spawn_dispatch(
    registry: &Arc<Registry>,
    request: Request,
) -> tokio::task::JoinHandle<WireResponse> {
    let registry = Arc::clone(registry);
    tokio::spawn(async move { crate::dispatch::dispatch(&registry, request).await })
}

async fn dispatch(registry: &Registry, request: Request) -> WireResponse {
    crate::dispatch::dispatch(registry, request).await
}

/// `response` is the error of a transfer cut by a close.
#[track_caller]
pub(crate) fn assert_session_closed(response: &WireResponse) {
    match response {
        WireResponse::Error(error) => {
            assert_eq!(error.code, WireErrorCode::TransferFailed, "{error:?}");
            assert_eq!(error.message, "session closed");
        }
        other => panic!("expected transfer-failed, got {other:?}"),
    }
}

pub(crate) async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

async fn within<T>(what: &str, future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(BOUND, future)
        .await
        .unwrap_or_else(|_| panic!("not within the bound: {what}"))
}

/// A transfer held open delays neither a read nor a take, and input of
/// the old controller after the take is refused.
#[tokio::test]
async fn a_held_put_delays_neither_a_screenshot_nor_a_take() {
    let (registry, connector, id) = opened().await;
    let ops = connector.ops(0);
    let putting = spawn_dispatch(&registry, put(&id, "hold"));
    until("the put runs", || ops.started("hold")).await;
    let shot = within(
        "screenshot",
        dispatch(
            &registry,
            Request::Screenshot {
                session: id.clone(),
            },
        ),
    )
    .await;
    assert!(matches!(shot, WireResponse::Screenshot { .. }), "{shot:?}");
    let viewer = ViewerControl::new(Arc::clone(&registry));
    within("take", viewer.take(&id, tab(1))).await.unwrap();
    match dispatch(&registry, mouse(&id)).await {
        WireResponse::Error(error) => assert_eq!(error.code, WireErrorCode::HumanControl),
        other => panic!("{other:?}"),
    }
    let stale = HumanInput::send(&*ops, vec![left(true)], InputEpoch::INITIAL).await;
    assert!(
        matches!(
            stale,
            Err(DaemonError::Sdk(rdpilot::Error::StaleInput { .. }))
        ),
        "{stale:?}"
    );
    ops.gate.add_permits(1);
    assert!(matches!(putting.await.unwrap(), WireResponse::Transfer(_)));
}

/// A close answers the running job, every queued job and a caller waiting
/// for queue capacity at once with transfer-failed "session closed".
#[tokio::test]
async fn close_fails_running_queued_and_capacity_waiting_transfers() {
    let (registry, connector, id) = opened().await;
    let ops = connector.ops(0);
    let mut callers = vec![spawn_dispatch(&registry, put(&id, "hold-0"))];
    until("the first put runs", || ops.started("hold-0")).await;
    for n in 1..=crate::transfer::TRANSFER_QUEUE_BOUND {
        callers.push(spawn_dispatch(&registry, get(&id, &format!("hold-{n}"))));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The queue is full: this caller waits for capacity.
    callers.push(spawn_dispatch(&registry, put(&id, "hold-waiting")));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(callers.iter().all(|c| !c.is_finished()));
    let closed = within(
        "close",
        dispatch(
            &registry,
            Request::Disconnect {
                session: id.clone(),
            },
        ),
    )
    .await;
    assert!(matches!(closed, WireResponse::Ack), "{closed:?}");
    for caller in callers {
        assert_session_closed(&within("caller", caller).await.unwrap());
    }
    // The running job stops on its own thread after its caller was answered.
    until("the running job stopped", || ops.left("hold-0")).await;
    assert_eq!(
        ops.log(),
        vec!["start hold-0", "left hold-0"],
        "no other job ran"
    );
}

/// A close does not wait for native input or a read in flight; both end
/// when released.
#[tokio::test]
async fn close_does_not_wait_for_held_input_or_a_held_read() {
    let (registry, connector, id) = opened().await;
    let ops = connector.ops(0);
    ops.hold_input.store(true, Ordering::SeqCst);
    ops.hold_reads.store(true, Ordering::SeqCst);
    let acting = spawn_dispatch(&registry, mouse(&id));
    let reading = spawn_dispatch(
        &registry,
        Request::Screenshot {
            session: id.clone(),
        },
    );
    until("the input and the read are held", || {
        ops.peer() == vec![left(true)] && ops.log().contains(&"start read".to_owned())
    })
    .await;
    let closed = within(
        "close",
        dispatch(
            &registry,
            Request::Disconnect {
                session: id.clone(),
            },
        ),
    )
    .await;
    assert!(matches!(closed, WireResponse::Ack), "{closed:?}");
    ops.input_gate.add_permits(1);
    ops.read_gate.add_permits(1);
    within("held input", acting).await.unwrap();
    within("held read", reading).await.unwrap();
    assert_eq!(ops.closes.load(Ordering::SeqCst), 1);
}

/// Agent native input in flight when a human take is granted is cut at its
/// next batch: the peer sees the release of the press and no later batch,
/// and the caller gets human-control naming the holder. With no human lease
/// current, the stale answer is internal.
#[tokio::test]
async fn agent_input_in_flight_is_cut_by_a_take_at_its_next_batch() {
    let (registry, connector, id) = opened().await;
    let ops = connector.ops(0);
    ops.hold_input.store(true, Ordering::SeqCst);
    let viewer = ViewerControl::new(Arc::clone(&registry));

    let acting = spawn_dispatch(&registry, mouse(&id));
    until("the press is sent", || ops.peer() == vec![left(true)]).await;
    let (grant, _) = within("take", viewer.take(&id, tab(1))).await.unwrap();
    assert_eq!(
        ops.peer(),
        vec![left(true), left(false)],
        "released by the take"
    );
    ops.input_gate.add_permits(1);
    match within("cut input", acting).await.unwrap() {
        WireResponse::Error(error) => {
            assert_eq!(error.code, WireErrorCode::HumanControl);
            assert_eq!(
                error.controller.and_then(|c| c.address).as_deref(),
                Some("100.64.0.1")
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(ops.peer(), vec![left(true), left(false)], "no later batch");

    viewer.release(&id, &grant.lease).await.unwrap();
    ops.peer.lock().unwrap().clear();
    let acting = spawn_dispatch(&registry, mouse(&id));
    until("the press is sent", || ops.peer() == vec![left(true)]).await;
    let (grant, _) = within("take", viewer.take(&id, tab(2))).await.unwrap();
    viewer.release(&id, &grant.lease).await.unwrap();
    ops.input_gate.add_permits(1);
    match within("cut input", acting).await.unwrap() {
        WireResponse::Error(error) => assert_eq!(error.code, WireErrorCode::Internal),
        other => panic!("{other:?}"),
    }
}

/// Reads racing a close answer with a snapshot or the refusal, never hang.
#[tokio::test]
async fn reads_racing_a_close_answer() {
    let (registry, _connector, id) = opened().await;
    let reads: Vec<_> = (0..20)
        .map(|_| {
            spawn_dispatch(
                &registry,
                Request::Screenshot {
                    session: id.clone(),
                },
            )
        })
        .collect();
    let closed = within(
        "close",
        dispatch(
            &registry,
            Request::Disconnect {
                session: id.clone(),
            },
        ),
    )
    .await;
    assert!(matches!(closed, WireResponse::Ack), "{closed:?}");
    for read in reads {
        match within("read", read).await.unwrap() {
            WireResponse::Screenshot { .. } => {}
            WireResponse::Error(error) => assert_eq!(error.code, WireErrorCode::SessionNotFound),
            other => panic!("{other:?}"),
        }
    }
}

/// Put, Get and bootstrap jobs run one at a time in the order they came.
#[tokio::test]
async fn transfer_jobs_run_one_at_a_time_in_order() {
    let (registry, connector, id) = opened().await;
    let ops = connector.ops(0);
    let first = spawn_dispatch(&registry, put(&id, "hold-a"));
    until("the put runs", || ops.started("hold-a")).await;
    let second = spawn_dispatch(&registry, get(&id, "sync-b"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    let third = {
        let registry = Arc::clone(&registry);
        let id = id.clone();
        tokio::spawn(async move {
            registry
                .transfer(
                    &id,
                    |s| Box::pin(async move { s.deploy_and_launch(InputEpoch::INITIAL).await }),
                    false,
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    ops.gate.add_permits(1);
    until("the get runs", || ops.started("sync-b")).await;
    ops.release_sync();
    until("the bootstrap runs", || ops.started("bootstrap")).await;
    ops.gate.add_permits(1);
    first.await.unwrap();
    second.await.unwrap();
    third.await.unwrap().unwrap();
    assert_eq!(
        ops.log(),
        [
            "start hold-a",
            "left hold-a",
            "start sync-b",
            "left sync-b",
            "start bootstrap",
            "left bootstrap"
        ]
    );
    assert_eq!(ops.max_running(), 1);
}

/// A job queued on a generation that is closed never runs on the next
/// generation of the same name, and a new job runs on the new one.
#[tokio::test]
async fn a_replaced_generation_never_serves_the_new_one() {
    let (registry, connector, id) = opened().await;
    let old = connector.ops(0);
    let running = spawn_dispatch(&registry, put(&id, "hold-old"));
    until("the old put runs", || old.started("hold-old")).await;
    let queued = spawn_dispatch(&registry, put(&id, "queued"));
    tokio::time::sleep(Duration::from_millis(20)).await;
    dispatch(
        &registry,
        Request::Disconnect {
            session: id.clone(),
        },
    )
    .await;
    assert_eq!(connect(&registry).await, id);
    let new = connector.ops(1);
    assert_eq!((old.number, new.number), (0, 1));
    assert_session_closed(&running.await.unwrap());
    assert_session_closed(&queued.await.unwrap());
    let fresh = dispatch(&registry, put(&id, "fresh")).await;
    assert!(matches!(fresh, WireResponse::Transfer(_)), "{fresh:?}");
    assert!(!old.started("queued") && !old.started("fresh"));
    assert!(!new.started("queued") && new.started("fresh"));
}

/// An SDK error a job returns after the close started (its bridge stopped)
/// is reported as the close.
#[tokio::test]
async fn an_sdk_error_after_the_close_is_reported_as_the_close() {
    let (registry, connector, id) = opened().await;
    let ops = connector.ops(0);
    let failing = spawn_dispatch(&registry, put(&id, "fail"));
    until("the put runs", || ops.started("fail")).await;
    dispatch(
        &registry,
        Request::Disconnect {
            session: id.clone(),
        },
    )
    .await;
    assert_session_closed(&within("caller", failing).await.unwrap());
}

/// A local copy and hash of a large file runs off the runtime: on a
/// runtime with one worker, reads stay within the bound while it runs and
/// is held, and a close answers its caller at once.
#[test]
fn reads_stay_fast_while_a_real_local_copy_runs() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("large.bin");
    std::fs::write(&source, vec![7u8; 16 * 1024 * 1024]).unwrap();
    let label = format!("copy:{}", source.display());
    let ops = runtime.block_on(async move {
        let (registry, connector, id) = opened().await;
        let ops = connector.ops(0);
        let putting = spawn_dispatch(&registry, put(&id, &label));
        until("the copy runs", || ops.started(&label)).await;
        for _ in 0..3 {
            let shot = within(
                "screenshot",
                dispatch(
                    &registry,
                    Request::Screenshot {
                        session: id.clone(),
                    },
                ),
            )
            .await;
            assert!(matches!(shot, WireResponse::Screenshot { .. }), "{shot:?}");
            let listed = registry.list();
            assert_eq!(listed.len(), 1);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let closed = within(
            "close",
            dispatch(
                &registry,
                Request::Disconnect {
                    session: id.clone(),
                },
            ),
        )
        .await;
        assert!(matches!(closed, WireResponse::Ack), "{closed:?}");
        assert_session_closed(&within("caller", putting).await.unwrap());
        ops
    });
    ops.release_sync();
    drop(runtime);
    assert!(ops.left(&format!("copy:{}", source.display())));
}
