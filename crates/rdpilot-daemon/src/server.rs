//! Top-level daemon assembly -- binds the IPC listener, constructs the
//! registry, seeds startup reconciliation orphans, spawns the lifecycle
//! tasks, and serves accepted+authorized connections (Plan 12-06).
//!
//! ## Non-Send execution model
//!
//! `rdpilot::Session::connect`'s returned future is NOT `Send` (`seams.rs`'s
//! `BoxFuture` doc comment), which makes `Registry::open`/`Registry::close`
//! (and therefore `ipc::serve_connection`, `lifecycle::idle_reaper`) NOT
//! `Send` either. [`run`] therefore drives EVERYTHING -- the accept loop,
//! every per-connection task, and both lifecycle watchers -- inside a
//! `tokio::task::LocalSet` via `tokio::task::spawn_local`, NEVER a bare
//! `tokio::spawn` (which requires `F: Send` and would fail to compile
//! against this crate's own registry/session types, exactly as
//! `seams.rs`'s doc comment warns). `LocalSet::run_until` works
//! irrespective of the ambient runtime's flavor (`main.rs`'s
//! `#[tokio::main]` multi-thread runtime is untouched) -- it just pins
//! every `spawn_local` task to the single OS thread that polls
//! `run_until`'s own future.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rdpilot::ConnectionConfig;
use rdpilot_ipc::SessionLifecycle;

use crate::bundle::{BundleError, BundleRequest, DaemonBundleSource, PreparedBundle};
use crate::diagnostics::Diagnostics;
use crate::ipc::ViewerContext;
use crate::lifecycle::{self, LifecycleConfig, ShutdownSignal};
use crate::reconcile::{self, JsonReconciliationSink};
use crate::recording::RecordingService;
use crate::registry::{Registry, ViewerRegistry};
use crate::seams::{
    BoxFuture, BundleSource, DaemonError, ManagedCua, ManagedSession, RealConnector,
    ReconciliationSink, SessionConnector, ViewFrameSource,
};
use crate::synthetic_frames::SyntheticFrames;
use crate::viewer::ViewerGate;

/// When set (to any value), [`run`] selects [`FakeTestConnector`] instead
/// of [`RealConnector`] -- lets the offline `autostart_lifecycle`
/// integration test (Task 3) drive the REAL compiled binary's full
/// Connect/List/Disconnect + auto-start/self-shutdown lifecycle without a
/// real RDP target.
const TEST_CONNECTOR_ENV: &str = "RDPILOT_DAEMON_TEST_CONNECTOR";

/// When set (to a valid `u64` millisecond count) alongside
/// [`TEST_CONNECTOR_ENV`], makes [`FakeTestSession`]'s slow-class methods
/// (`upload_file`/`download_file`) genuinely
/// `tokio::time::sleep` that long before resolving -- an env-gated,
/// TEST-only hook (14-05, MCP-06) that lets a slow daemon round trip be
/// exercised offline. Absent or unparseable -> `0` (no sleep, identical to
/// this env being entirely unset). Read ONCE in [`run_inner`] and threaded
/// into every [`FakeTestSession`] the process's [`FakeTestConnector`]
/// produces -- never consulted anywhere near [`RealConnector`], so the
/// production connector is untouched (T-14-16).
const TEST_SLOW_MS_ENV: &str = "RDPILOT_DAEMON_TEST_SLOW_MS";

/// Fake-connector-only delay before the canned bridge bootstrap reports
/// success. It is read only when the explicit fake connector is selected and
/// is unreachable by [`RealConnector`].
const TEST_BOOTSTRAP_DELAY_MS_ENV: &str = "RDPILOT_DAEMON_TEST_BOOTSTRAP_DELAY_MS";

/// When set (to any value) alongside [`TEST_CONNECTOR_ENV`], every
/// [`FakeTestSession`] carries a synthetic, changing frame source for the
/// live viewer's offline tests and the proof harness's fake mode. Read only
/// when the fake connector is selected; unreachable by [`RealConnector`].
const TEST_FRAMES_ENV: &str = "RDPILOT_DAEMON_TEST_FRAMES";

/// Fake-connector-only host value: a fake session connected with this host
/// ends its synthetic frame source 4 s after connect, standing in for an
/// RDP session that the server ended (logoff, network loss).
const TEST_FRAMES_SERVER_END_HOST: &str = "fake-server-end";

/// Fake-connector-only host value: a fake session connected with this host
/// shows one frame at connect and one changed frame 1 s later, then keeps
/// its display unchanged (a still desktop for recording tests).
const TEST_FRAMES_STILL_HOST: &str = "fake-still";

/// When set (to any value) alongside [`TEST_CONNECTOR_ENV`], every
/// [`FakeTestSession`] accepts Cua attachments served by [`FakeCua`], an
/// in-process MCP responder, so `rdpilot-mcp` can drive tool calls offline.
/// Read only when the fake connector is selected; unreachable by
/// [`RealConnector`].
const TEST_CUA_ENV: &str = "RDPILOT_DAEMON_TEST_CUA";

/// When set (to a file path) alongside [`TEST_CONNECTOR_ENV`], every
/// [`FakeTestSession`] accepts human viewer input and appends one JSON line
/// of counts per batch to that file: events taken so far and keys or
/// buttons held now. Never key codes or coordinates. Read only when the fake
/// connector is selected; unreachable by [`RealConnector`].
const TEST_INPUT_LOG_ENV: &str = "RDPILOT_DAEMON_TEST_INPUT_LOG";

/// The env var overriding [`RunConfig`]'s reconciliation-state sink path
/// (test injection point -- production always resolves the platform
/// cache-dir default via [`JsonReconciliationSink::new`]).
const SINK_PATH_ENV: &str = "RDPILOT_DAEMON_SINK_PATH";

/// Startup configuration for [`run`].
///
/// The well-known IPC socket path itself is NOT overridable here: `ipc::unix::bind`
/// (Plan 12-04) always resolves it via `directories::BaseDirs::runtime_dir()`,
/// which reads the standard `XDG_RUNTIME_DIR` OS env var -- the
/// `autostart_lifecycle` integration test (Task 3) achieves temp-path
/// isolation the same way any other process on this host would: by setting
/// `XDG_RUNTIME_DIR` to a fresh temp directory before spawning the daemon,
/// rather than by adding a parallel, never-otherwise-exercised parameterized
/// bind path to `ipc::unix` (out of this plan's file scope, and would leave
/// the production `bind()` call path partially untested by its own new
/// sibling).
#[derive(Debug, Clone, Default)]
pub struct RunConfig {
    /// Overrides the reconciliation-state sink's on-disk path (test
    /// injection point). `None` resolves the platform cache-dir default
    /// via [`JsonReconciliationSink::new`].
    pub sink_path_override: Option<PathBuf>,
    /// The idle-reap / empty-registry-grace / poll-interval durations
    /// (D-31).
    pub lifecycle: LifecycleConfig,
}

impl RunConfig {
    /// Build a [`RunConfig`] from the production defaults, overridden by
    /// [`SINK_PATH_ENV`] and [`LifecycleConfig::from_env`]'s own
    /// `RDPILOT_DAEMON_*_MS` env vars when present. Used by `main.rs`'s
    /// real entry point AND by the `autostart_lifecycle` integration test
    /// (which sets these env vars on itself before spawning the real
    /// binary -- env vars are the only config channel that crosses the
    /// process boundary `connect_or_spawn` creates).
    #[must_use]
    pub fn from_env() -> Self {
        RunConfig {
            sink_path_override: std::env::var(SINK_PATH_ENV).ok().map(PathBuf::from),
            lifecycle: LifecycleConfig::from_env(),
        }
    }
}

/// The daemon's public entry point, called by `main.rs`.
///
/// Steps: (1) bind the well-known socket -- `AddrInUse` means another
/// daemon already won the single-instance race (Plan 12-04's bind-as-mutex,
/// research Pattern 3); this process exits cleanly (`Ok(())`), not an
/// error. (2) Build the reconciliation sink and the registry (selecting
/// [`FakeTestConnector`] instead of [`RealConnector`] when
/// [`TEST_CONNECTOR_ENV`] is set). (3) Run the startup reconciliation
/// scan + seed BEFORE accepting any client (DAEMON-04 orphans must be
/// visible to the first `list`). (4) Spawn the idle reaper and
/// empty-registry watcher. (5) Accept + authorize + serve connections
/// until the empty-registry watcher fires the shutdown signal, then clean
/// up the socket file and return.
///
/// # Errors
///
/// Returns [`DaemonError`] if the bind fails for a reason OTHER than
/// `AddrInUse`, or if the reconciliation sink's path cannot be resolved.
pub async fn run(config: RunConfig) -> Result<(), DaemonError> {
    let local = tokio::task::LocalSet::new();
    local.run_until(run_inner(config)).await
}

async fn run_inner(config: RunConfig) -> Result<(), DaemonError> {
    let listener = match crate::ipc::bind().await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            // Benign single-instance loser exit (research Pattern 3): another
            // daemon already won the bind race. Never an error.
            eprintln!("rdpilot-daemon: another daemon is already listening -- exiting");
            return Ok(());
        }
        Err(err) => return Err(DaemonError::Io(err.to_string())),
    };

    let sink = Arc::new(match config.sink_path_override {
        Some(path) => JsonReconciliationSink::at(path),
        None => JsonReconciliationSink::new().ok_or_else(|| {
            DaemonError::Io(
                "could not resolve the platform cache directory for reconciliation state"
                    .to_owned(),
            )
        })?,
    });

    let fake = std::env::var(TEST_CONNECTOR_ENV).is_ok();
    let bundles: Arc<dyn BundleSource> = if fake {
        Arc::new(FakeBundleSource)
    } else {
        Arc::new(DaemonBundleSource::from_environment())
    };
    let connector: Arc<dyn SessionConnector> = if fake {
        let slow_ms = std::env::var(TEST_SLOW_MS_ENV)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let bootstrap_delay_ms = std::env::var(TEST_BOOTSTRAP_DELAY_MS_ENV)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        Arc::new(FakeTestConnector {
            slow_ms,
            bootstrap_delay_ms,
            frames: std::env::var(TEST_FRAMES_ENV).is_ok(),
            cua: std::env::var(TEST_CUA_ENV).is_ok(),
            input_log: std::env::var_os(TEST_INPUT_LOG_ENV).map(PathBuf::from),
        })
    } else {
        Arc::new(RealConnector)
    };

    let recordings = RecordingService::from_config();
    let registry = Arc::new(
        Registry::with_recordings(
            connector,
            sink.clone() as Arc<dyn ReconciliationSink>,
            Arc::clone(&recordings),
        )
        .with_bundle_source(bundles),
    );
    let diagnostics = Diagnostics::from_env().map(Arc::new);
    // Finish recordings an earlier daemon left open and prune, off the IPC
    // thread; a failure is logged by the service and never stops the daemon.
    {
        let recordings = Arc::clone(&recordings);
        let _ = tokio::task::spawn_blocking(move || recordings.startup()).await;
    }

    // Startup reconciliation (DAEMON-04): scan + seed BEFORE the accept
    // loop below, so any leftover orphan from a crashed predecessor is
    // already visible to the very first `list` a client sends.
    let orphans = reconcile::scan_orphans(sink.path());
    reconcile::seed_into(orphans, &registry);

    let shutdown = ShutdownSignal::new();
    let reaper_handle = tokio::task::spawn_local(lifecycle::idle_reaper(
        Arc::clone(&registry),
        config.lifecycle,
        shutdown.clone(),
    ));
    let watcher_handle = tokio::task::spawn_local(lifecycle::empty_watcher(
        Arc::clone(&registry),
        config.lifecycle,
        shutdown.clone(),
    ));
    // The live viewer sees only this facade (list + passive frame lookup).
    let viewer = ViewerContext {
        registry: ViewerRegistry::new(Arc::clone(&registry)),
        control: crate::registry::ViewerControl::new(Arc::clone(&registry)),
        gate: ViewerGate::default(),
    };

    loop {
        tokio::select! {
            accepted = crate::ipc::accept_and_authorize(&listener) => {
                match accepted {
                    Ok(stream) => {
                        let registry_for_conn = Arc::clone(&registry);
                        let diagnostics_for_conn = diagnostics.clone();
                        let viewer_for_conn = viewer.clone();
                        tokio::task::spawn_local(async move {
                            crate::ipc::serve_connection(stream, &registry_for_conn, diagnostics_for_conn.as_deref(), Some(&viewer_for_conn)).await;
                        });
                    }
                    Err(err) => {
                        // A rejected/failed connection (e.g. a different-uid
                        // peer, DAEMON-02) is logged and dropped -- never
                        // fatal to the accept loop. D-31: this error string
                        // never carries a `Request`/credential -- the
                        // connection was rejected before any frame was ever
                        // read.
                        eprintln!("rdpilot-daemon: rejected/failed connection: {err}");
                    }
                }
            }
            () = shutdown.wait() => break,
        }
    }

    // Graceful join (not abort): both watchers select on the SAME
    // `shutdown` this loop just observed, so they are already unwinding.
    let _ = reaper_handle.await;
    let _ = watcher_handle.await;
    // Return control to the agent, with held-key releases, before the
    // recordings close so the lease end is in them.
    registry
        .end_all_leases(crate::control::EndReason::DaemonStopped)
        .await;
    recordings.shutdown().await;

    if let Ok(path) = crate::ipc::socket_path() {
        let _ = std::fs::remove_file(&path);
    }

    Ok(())
}

/// A light, in-process fake [`ManagedSession`] (no real OS thread, no real
/// RDP target) -- the [`FakeTestConnector`]'s product. This is the offline
/// lifecycle test's stand-in only; the DAEMON-01 thread-owning soak proof
/// lives in `tests/thread_leak_soak.rs`'s own `ThreadOwningFakeSession`,
/// exercised separately.
///
/// Every method resolves immediately EXCEPT the three slow-class methods
/// (`upload_file`/`download_file`), which -- when
/// `slow_ms` is non-zero (14-05, MCP-06) -- `tokio::time::sleep(slow_ms)`
/// before returning their existing canned result. `slow_ms` is `0` unless
/// [`TEST_SLOW_MS_ENV`] was set at daemon startup, so every other existing
/// daemon/CLI test (which never sets that env var) sees byte-for-byte the
/// same immediately-resolving behavior as before this plan.
struct FakeTestSession {
    slow_ms: u64,
    bootstrap_delay_ms: u64,
    /// Synthetic frames when [`TEST_FRAMES_ENV`] is set.
    frames: Option<Arc<SyntheticFrames>>,
    /// Serve Cua attachments with [`FakeCua`] when [`TEST_CUA_ENV`] is set.
    cua: bool,
    /// Counting human input sink when [`TEST_INPUT_LOG_ENV`] is set.
    input: Option<Arc<CountingInput>>,
}

/// The fake connector's human input sink: counts only.
struct CountingInput {
    log: PathBuf,
    state: std::sync::Mutex<InputCounts>,
}

/// Events taken so far, and the keys or buttons held now.
type InputCounts = (u64, std::collections::BTreeSet<crate::control::Held>);

impl crate::seams::HumanInput for CountingInput {
    fn send(
        &self,
        events: Vec<crate::control::HumanEvent>,
    ) -> crate::seams::SendFuture<'_, Result<(), DaemonError>> {
        use crate::control::{Held, HumanEvent};
        let line = {
            let mut state = match self.state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            for event in &events {
                state.0 += 1;
                let (held, down) = match *event {
                    HumanEvent::Key {
                        code,
                        extended,
                        down,
                    } => (Held::Key { code, extended }, down),
                    HumanEvent::Button { button, down } => (Held::Button(button), down),
                    HumanEvent::Move { .. } | HumanEvent::Wheel { .. } => continue,
                };
                if down {
                    state.1.insert(held);
                } else {
                    state.1.remove(&held);
                }
            }
            serde_json::json!({ "events": state.0, "held": state.1.len() }).to_string()
        };
        Box::pin(async move {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)
                .map_err(|e| DaemonError::Io(e.to_string()))?;
            writeln!(file, "{line}").map_err(|e| DaemonError::Io(e.to_string()))
        })
    }
}

impl ManagedSession for FakeTestSession {
    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), DaemonError>>>> {
        if let Some(frames) = &self.frames {
            frames.end();
        }
        Box::pin(async { Ok(()) })
    }

    fn frame_source(&self) -> Option<Arc<dyn ViewFrameSource>> {
        self.frames
            .as_ref()
            .map(|f| Arc::clone(f) as Arc<dyn ViewFrameSource>)
    }

    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
    }

    fn human_input(&self) -> Option<Arc<dyn crate::seams::HumanInput>> {
        self.input
            .as_ref()
            .map(|input| Arc::clone(input) as Arc<dyn crate::seams::HumanInput>)
    }

    fn attach_cua(&self) -> BoxFuture<'_, Result<Box<dyn ManagedCua>, DaemonError>> {
        let cua = self.cua;
        Box::pin(async move {
            if cua {
                Ok(Box::new(FakeCua::new()) as Box<dyn ManagedCua>)
            } else {
                Err(DaemonError::Connect("Cua bridge unavailable".into()))
            }
        })
    }

    // --- Canned operational stubs (Phase 13) --------------------------
    //
    // Every method below returns a plausible, non-empty fixed value so the
    // offline CLI-02 integration proof (Plan 13-06) can render a real table
    // against this fake connector with no live RDP target. Dispatch is not
    // wired to these yet (Plan 13-04) -- this crate does not call them
    // outside tests until then.

    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async {
            Ok(rdpilot::Screenshot {
                width: 2,
                height: 1,
                rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
            })
        })
    }

    fn send_mouse(&self, _action: rdpilot::MouseAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }

    fn send_key(&self, _action: rdpilot::KeyAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }

    fn upload_file(
        &self,
        _local: std::path::PathBuf,
        _remote_name: String,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        let slow_ms = self.slow_ms;
        Box::pin(async move {
            if slow_ms > 0 {
                tokio::time::sleep(Duration::from_millis(slow_ms)).await;
            }
            Ok(rdpilot::TransferOutcome {
                bytes_transferred: 1024,
                checksum: "0".repeat(64),
            })
        })
    }

    fn download_file(
        &self,
        _remote_name: String,
        local: std::path::PathBuf,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        let slow_ms = self.slow_ms;
        Box::pin(async move {
            if slow_ms > 0 {
                tokio::time::sleep(Duration::from_millis(slow_ms)).await;
            }
            std::fs::write(local, vec![0_u8; 1024]).map_err(|e| DaemonError::Io(e.to_string()))?;
            Ok(rdpilot::TransferOutcome {
                bytes_transferred: 1024,
                checksum: "0".repeat(64),
            })
        })
    }

    fn ping(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
        Box::pin(async { Ok(std::time::Duration::from_millis(5)) })
    }

    fn desktop_size(&self) -> (u32, u32) {
        (1920, 1080)
    }
    fn deploy_and_launch(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
        let bootstrap_delay_ms = self.bootstrap_delay_ms;
        Box::pin(async move {
            if bootstrap_delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(bootstrap_delay_ms)).await;
            }
            Ok(std::time::Duration::from_millis(0))
        })
    }
}

/// A light, in-process fake [`SessionConnector`] selected by [`run`]
/// instead of [`RealConnector`] when [`TEST_CONNECTOR_ENV`] is set --
/// makes the daemon's auto-start/idle-reap/self-shutdown lifecycle fully
/// provable offline (no RDP target) against the REAL compiled binary
/// (`tests/autostart_lifecycle.rs`, Task 3).
struct FakeTestConnector {
    /// Threaded into every [`FakeTestSession`] this connector produces --
    /// see [`TEST_SLOW_MS_ENV`].
    slow_ms: u64,
    bootstrap_delay_ms: u64,
    /// See [`TEST_FRAMES_ENV`].
    frames: bool,
    /// See [`TEST_CUA_ENV`].
    cua: bool,
    /// See [`TEST_INPUT_LOG_ENV`].
    input_log: Option<PathBuf>,
}

impl SessionConnector for FakeTestConnector {
    fn connect(
        &self,
        cfg: ConnectionConfig,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn ManagedSession>, DaemonError>>>> {
        let slow_ms = self.slow_ms;
        let bootstrap_delay_ms = self.bootstrap_delay_ms;
        let cua = self.cua;
        let input = self.input_log.clone().map(|log| {
            Arc::new(CountingInput {
                log,
                state: std::sync::Mutex::default(),
            })
        });
        let frames = self.frames.then(|| {
            let frames = SyntheticFrames::new();
            if cfg.host() == TEST_FRAMES_STILL_HOST {
                frames.publish_solid(64, 48, 1);
                let weak = Arc::downgrade(&frames);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    if let Some(frames) = weak.upgrade() {
                        frames.publish_solid(64, 48, 2);
                    }
                });
                return frames;
            }
            frames.spawn_animation();
            if cfg.host() == TEST_FRAMES_SERVER_END_HOST {
                let weak = Arc::downgrade(&frames);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    if let Some(frames) = weak.upgrade() {
                        frames.end();
                    }
                });
            }
            frames
        });
        Box::pin(async move {
            Ok(Box::new(FakeTestSession {
                slow_ms,
                bootstrap_delay_ms,
                frames,
                cua,
                input,
            }) as Box<dyn ManagedSession>)
        })
    }
}

/// Bundle source paired with [`FakeTestConnector`]: a fixed bundle id and a
/// directory the fake sessions never read. Never touches the network.
struct FakeBundleSource;

impl BundleSource for FakeBundleSource {
    fn prepare(
        &self,
        _request: BundleRequest,
    ) -> BoxFuture<'_, Result<PreparedBundle, BundleError>> {
        Box::pin(async {
            Ok(PreparedBundle {
                dir: std::env::temp_dir().join("rdpilot-fake-bundle"),
                bundle_id: "fake-bundle".to_owned(),
                warnings: Vec::new(),
            })
        })
    }
}

/// The fake connector's in-process Cua: answers `initialize`, `tools/list`
/// and `tools/call` like a minimal MCP server. Tool `fail` answers with
/// `isError: true`, tool `hold` never answers, any other tool answers `ok`.
/// `list_windows` is its one read-only tool. A call whose arguments still
/// carry `takeover` answers `isError` "takeover reached Cua" (the daemon
/// must strip it). Unknown methods get a JSON-RPC error; notifications and responses get
/// nothing. Replies never echo arguments.
struct FakeCua {
    attachment: u64,
    replies: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    queued: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    closed: bool,
}

impl FakeCua {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let (replies, queued) = tokio::sync::mpsc::unbounded_channel();
        Self {
            attachment: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            replies,
            queued,
            closed: false,
        }
    }

    fn reply(message: &serde_json::Value) -> Option<serde_json::Value> {
        use serde_json::json;
        let id = message.get("id")?.clone();
        let method = message.get("method")?.as_str()?;
        let params = message.get("params");
        let result = match method {
            "initialize" => json!({
                "protocolVersion": params
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "rdpilot-fake-cua", "version": "0" },
            }),
            "tools/list" => {
                let tools = ["echo", "fail", "hold", "list_windows"]
                    .map(|name| json!({ "name": name, "inputSchema": { "type": "object" } }));
                json!({ "tools": tools })
            }
            "tools/call"
                if params
                    .and_then(|p| p.get("arguments"))
                    .and_then(|a| a.get("takeover"))
                    .is_some() =>
            {
                json!({
                    "content": [{ "type": "text", "text": "takeover reached Cua" }],
                    "isError": true,
                })
            }
            "tools/call" => match params.and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
                Some("hold") => return None,
                Some("fail") => json!({
                    "content": [{ "type": "text", "text": "failed" }],
                    "isError": true,
                }),
                _ => json!({ "content": [{ "type": "text", "text": "ok" }] }),
            },
            _ => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "method not found" },
                }))
            }
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }
}

impl ManagedCua for FakeCua {
    fn identity(&self) -> (u64, u64, u64) {
        (1, 1, self.attachment)
    }

    fn send(&self, message: serde_json::Value) -> BoxFuture<'_, Result<(), DaemonError>> {
        let reply = Self::reply(&message);
        Box::pin(async move {
            if self.closed {
                return Err(DaemonError::Connect("Cua attachment closed".into()));
            }
            if let Some(reply) = reply {
                let _ = self.replies.send(reply);
            }
            Ok(())
        })
    }

    fn recv(&mut self) -> BoxFuture<'_, Result<Option<serde_json::Value>, DaemonError>> {
        Box::pin(async move {
            if self.closed {
                return Ok(None);
            }
            Ok(self.queued.recv().await)
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.closed = true;
        self.queued.close();
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// The lighter, always-on (never spawns a real process) regression
    /// guard the plan calls for alongside the heavier real-binary
    /// integration test: `run()` with an immediately-empty registry and a
    /// short empty_grace self-shuts-down on its own (no client ever
    /// connects at all), proving the assembly wiring end-to-end without
    /// process spawning.
    fn fake_download_path(slow_ms: u64) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rdpilot-fake-download-{}-{slow_ms}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn run_with_an_immediately_empty_registry_and_a_short_grace_self_shuts_down() {
        let dir = std::env::temp_dir().join(format!(
            "rdpilot-daemon-server-run-test-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let sink_path = dir.join("sessions.json");

        let config = RunConfig {
            sink_path_override: Some(sink_path.clone()),
            lifecycle: LifecycleConfig {
                idle_timeout: Duration::from_secs(3600),
                empty_grace: Duration::from_millis(30),
                reap_interval: Duration::from_millis(10),
            },
        };

        // Bypasses `ipc::unix::bind` entirely (no socket, no client) --
        // this test exercises the reconciliation-seed + lifecycle-task
        // assembly `run_inner` performs, run to completion under a bounded
        // timeout so a regression that fails to self-shut-down fails this
        // test instead of hanging the suite.
        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(tokio::time::timeout(
                Duration::from_secs(5),
                run_inner_without_bind_for_test(config),
            ))
            .await;

        assert!(
            result.is_ok(),
            "run_inner's lifecycle assembly must self-shut-down well within the timeout"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A thin harness mirroring `run_inner`'s post-bind steps (registry +
    /// reconciliation-seed + lifecycle-task spawn + shutdown-wait), minus
    /// the `ipc::unix::bind`/accept-loop portion -- this file's own
    /// `#[tokio::test]` above cannot bind a real socket (no unique-per-test
    /// path parameter exists on `ipc::unix::bind`, per this file's own
    /// `RunConfig` doc comment), but every OTHER piece of `run`'s assembly
    /// (sink resolution, connector selection, orphan seed ordering,
    /// reaper/watcher spawn, graceful join) is exercised identically.
    async fn run_inner_without_bind_for_test(config: RunConfig) {
        let sink = Arc::new(match config.sink_path_override {
            Some(path) => JsonReconciliationSink::at(path),
            None => JsonReconciliationSink::new()
                .expect("platform cache dir should resolve in this test environment"),
        });

        let connector: Arc<dyn SessionConnector> = Arc::new(FakeTestConnector {
            slow_ms: 0,
            bootstrap_delay_ms: 0,
            frames: false,
            cua: false,
            input_log: None,
        });
        let registry = Arc::new(Registry::new(
            connector,
            sink.clone() as Arc<dyn ReconciliationSink>,
        ));

        let orphans = reconcile::scan_orphans(sink.path());
        reconcile::seed_into(orphans, &registry);

        let shutdown = ShutdownSignal::new();
        let reaper_handle = tokio::task::spawn_local(lifecycle::idle_reaper(
            Arc::clone(&registry),
            config.lifecycle,
            shutdown.clone(),
        ));
        let watcher_handle = tokio::task::spawn_local(lifecycle::empty_watcher(
            Arc::clone(&registry),
            config.lifecycle,
            shutdown.clone(),
        ));

        shutdown.wait().await;
        let _ = reaper_handle.await;
        let _ = watcher_handle.await;
    }

    // --- 14-05 (MCP-06): the env-gated slowness hook -----------------

    /// [`TEST_SLOW_MS_ENV`] regression guard: `slow_ms: 0` (the value used
    /// whenever the env var is absent/invalid -- i.e. every existing
    /// daemon/CLI test that never sets it) resolves every slow-class
    /// method immediately, byte-for-byte the same as before this plan.
    #[tokio::test]
    async fn fake_session_slow_class_methods_resolve_immediately_when_slow_ms_is_zero() {
        let session = FakeTestSession {
            slow_ms: 0,
            bootstrap_delay_ms: 0,
            frames: None,
            cua: false,
            input: None,
        };
        let immediate_bound = Duration::from_millis(50);

        let start = std::time::Instant::now();
        session
            .upload_file(std::path::PathBuf::from("/tmp/x"), "remote".to_owned())
            .await
            .expect("upload_file should succeed");
        assert!(
            start.elapsed() < immediate_bound,
            "slow_ms=0 upload_file must resolve immediately (env-unset regression guard)"
        );

        let start = std::time::Instant::now();
        session
            .download_file("remote".to_owned(), fake_download_path(session.slow_ms))
            .await
            .expect("download_file should succeed");
        assert!(
            start.elapsed() < immediate_bound,
            "slow_ms=0 download_file must resolve immediately (env-unset regression guard)"
        );
        let _ = std::fs::remove_file(fake_download_path(session.slow_ms));
    }

    /// With `slow_ms` set, every slow-class method genuinely sleeps at
    /// least that long before resolving -- the mechanism `tests/non_blocking.rs`
    /// (14-05, MCP-06) relies on to exercise a genuine slow daemon round
    /// trip offline. Fast methods (`ping`) are unaffected regardless.
    #[tokio::test]
    async fn fake_session_slow_class_methods_sleep_the_configured_slow_ms() {
        const SLOW_MS: u64 = 30;
        let session = FakeTestSession {
            slow_ms: SLOW_MS,
            bootstrap_delay_ms: 0,
            frames: None,
            cua: false,
            input: None,
        };
        let bound = Duration::from_millis(SLOW_MS);

        let start = std::time::Instant::now();
        session
            .upload_file(std::path::PathBuf::from("/tmp/x"), "remote".to_owned())
            .await
            .expect("upload_file should succeed");
        assert!(
            start.elapsed() >= bound,
            "slow_ms={SLOW_MS} upload_file must sleep at least that long"
        );

        let start = std::time::Instant::now();
        session
            .download_file("remote".to_owned(), fake_download_path(session.slow_ms))
            .await
            .expect("download_file should succeed");
        assert!(
            start.elapsed() >= bound,
            "slow_ms={SLOW_MS} download_file must sleep at least that long"
        );
        let _ = std::fs::remove_file(fake_download_path(session.slow_ms));

        // Fast methods stay immediate regardless of slow_ms (only the
        // three slow-class methods above consult it at all).
        let start = std::time::Instant::now();
        session.ping().await.expect("ping should succeed");
        assert!(
            start.elapsed() < bound,
            "ping must stay immediate even when slow_ms is set"
        );
    }

    #[tokio::test]
    async fn fake_cua_answers_like_a_minimal_mcp_server() {
        use serde_json::json;
        let session = FakeTestSession {
            slow_ms: 0,
            bootstrap_delay_ms: 0,
            frames: None,
            cua: true,
            input: None,
        };
        let mut cua = session.attach_cua().await.expect("fake Cua attaches");
        let call = |id: u64, method: &str, params: serde_json::Value| json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        cua.send(call(
            1,
            "initialize",
            json!({ "protocolVersion": "2025-06-18" }),
        ))
        .await
        .unwrap();
        cua.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
            .unwrap();
        cua.send(call(2, "tools/list", json!({}))).await.unwrap();
        cua.send(call(3, "tools/call", json!({ "name": "hold" })))
            .await
            .unwrap();
        cua.send(call(
            4,
            "tools/call",
            json!({ "name": "echo", "arguments": { "text": "secret" } }),
        ))
        .await
        .unwrap();
        cua.send(call(5, "tools/call", json!({ "name": "fail" })))
            .await
            .unwrap();
        cua.send(call(6, "nope", json!({}))).await.unwrap();
        let mut replies = Vec::new();
        for _ in 0..5 {
            replies.push(cua.recv().await.unwrap().unwrap());
        }
        let ids: Vec<u64> = replies.iter().map(|r| r["id"].as_u64().unwrap()).collect();
        assert_eq!(
            ids,
            vec![1, 2, 4, 5, 6],
            "no reply to notifications or hold"
        );
        assert_eq!(
            replies[0]["result"]["serverInfo"]["name"],
            "rdpilot-fake-cua"
        );
        assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 4);
        assert!(!replies[2].to_string().contains("secret"));
        assert_eq!(replies[3]["result"]["isError"], true);
        assert_eq!(replies[4]["error"]["code"], -32601);
        cua.close().await.unwrap();
        assert!(cua.recv().await.unwrap().is_none());
        assert!(cua.send(call(7, "tools/list", json!({}))).await.is_err());

        let without = FakeTestSession {
            slow_ms: 0,
            bootstrap_delay_ms: 0,
            frames: None,
            cua: false,
            input: None,
        };
        assert!(without.attach_cua().await.is_err());
    }

    #[tokio::test]
    async fn fake_input_logs_counts_only() {
        use crate::control::HumanEvent;
        use crate::seams::HumanInput;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("input.jsonl");
        let input = CountingInput {
            log: log.clone(),
            state: std::sync::Mutex::default(),
        };
        let key = |down| HumanEvent::Key {
            code: 0x2A,
            extended: false,
            down,
        };
        input
            .send(vec![HumanEvent::Move { x: 777, y: 555 }, key(true)])
            .await
            .unwrap();
        input.send(vec![key(false)]).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            text,
            "{\"events\":2,\"held\":1}\n{\"events\":3,\"held\":0}\n"
        );
        assert!(!text.contains("777") && !text.contains("42"));
    }
}
