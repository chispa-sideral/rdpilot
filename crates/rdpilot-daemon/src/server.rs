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

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::future::Future;
use std::time::Duration;

use rdpilot::ConnectionConfig;
use rdpilot_ipc::SessionLifecycle;

use crate::diagnostics::Diagnostics;
use crate::lifecycle::{self, LifecycleConfig, ShutdownSignal};
use crate::reconcile::{self, JsonReconciliationSink};
use crate::registry::Registry;
use crate::seams::{BoxFuture, DaemonError, ManagedSession, RealConnector, ReconciliationSink, SessionConnector};

/// When set (to any value), [`run`] selects [`FakeTestConnector`] instead
/// of [`RealConnector`] -- lets the offline `autostart_lifecycle`
/// integration test (Task 3) drive the REAL compiled binary's full
/// Connect/List/Disconnect + auto-start/self-shutdown lifecycle without a
/// real RDP target.
const TEST_CONNECTOR_ENV: &str = "RDPILOT_DAEMON_TEST_CONNECTOR";

/// When set (to a valid `u64` millisecond count) alongside
/// [`TEST_CONNECTOR_ENV`], makes [`FakeTestSession`]'s slow-class methods
/// (`upload_file`/`download_file`/`launch_process`) genuinely
/// `tokio::time::sleep` that long before resolving -- an env-gated,
/// TEST-only hook (14-05, MCP-06) that lets a slow daemon round trip be
/// exercised offline. Absent or unparseable -> `0` (no sleep, identical to
/// this env being entirely unset). Read ONCE in [`run_inner`] and threaded
/// into every [`FakeTestSession`] the process's [`FakeTestConnector`]
/// produces -- never consulted anywhere near [`RealConnector`], so the
/// production connector is untouched (T-14-16).
const TEST_SLOW_MS_ENV: &str = "RDPILOT_DAEMON_TEST_SLOW_MS";

/// Fake-connector-only delay before the canned sensor bootstrap reports
/// success. It is read only when the explicit fake connector is selected and
/// is unreachable by [`RealConnector`].
const TEST_BOOTSTRAP_DELAY_MS_ENV: &str = "RDPILOT_DAEMON_TEST_BOOTSTRAP_DELAY_MS";

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
        None => JsonReconciliationSink::new()
            .ok_or_else(|| DaemonError::Io("could not resolve the platform cache directory for reconciliation state".to_owned()))?,
    });

    let connector: Arc<dyn SessionConnector> = if std::env::var(TEST_CONNECTOR_ENV).is_ok() {
        let slow_ms = std::env::var(TEST_SLOW_MS_ENV).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let bootstrap_delay_ms = std::env::var(TEST_BOOTSTRAP_DELAY_MS_ENV).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        Arc::new(FakeTestConnector { slow_ms, bootstrap_delay_ms })
    } else {
        Arc::new(RealConnector)
    };

    let registry = Arc::new(Registry::new(connector, sink.clone() as Arc<dyn ReconciliationSink>));
    let diagnostics = Diagnostics::from_env().map(Arc::new);

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

    loop {
        tokio::select! {
            accepted = crate::ipc::accept_and_authorize(&listener) => {
                match accepted {
                    Ok(stream) => {
                        let registry_for_conn = Arc::clone(&registry);
                        let diagnostics_for_conn = diagnostics.clone();
                        tokio::task::spawn_local(async move {
                            crate::ipc::serve_connection(stream, &registry_for_conn, diagnostics_for_conn.as_deref()).await;
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
/// (`upload_file`/`download_file`/`launch_process`), which -- when
/// `slow_ms` is non-zero (14-05, MCP-06) -- `tokio::time::sleep(slow_ms)`
/// before returning their existing canned result. `slow_ms` is `0` unless
/// [`TEST_SLOW_MS_ENV`] was set at daemon startup, so every other existing
/// daemon/CLI test (which never sets that env var) sees byte-for-byte the
/// same immediately-resolving behavior as before this plan.
struct FakeTestSession {
    slow_ms: u64,
    bootstrap_delay_ms: u64,
}

impl ManagedSession for FakeTestSession {
    fn close(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), DaemonError>>>> {
        Box::pin(async { Ok(()) })
    }

    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
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

    fn world_state(&self, opts: rdpilot::WorldStateOptions) -> BoxFuture<'_, Result<rdpilot::WorldState, DaemonError>> {
        Box::pin(async move {
            let screenshot = if opts.screenshot {
                Some(rdpilot::Screenshot {
                    width: 2,
                    height: 1,
                    rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
                })
            } else {
                None
            };
            let window_list = if opts.window_list { Some(vec![canned_window_info()]) } else { None };
            let uia = match opts.uia {
                rdpilot::UiaMode::None => None,
                rdpilot::UiaMode::Foreground | rdpilot::UiaMode::Hwnd(_) | rdpilot::UiaMode::AllTopLevel => {
                    Some(vec![(1u64, vec![canned_uia_element()])])
                }
            };
            Ok(rdpilot::WorldState {
                timestamp: std::time::SystemTime::now(),
                capture_span: std::time::Duration::from_millis(1),
                screenshot,
                window_list,
                uia,
            })
        })
    }

    fn get_window_list(&self) -> BoxFuture<'_, Result<Vec<rdpilot::WindowInfo>, DaemonError>> {
        Box::pin(async { Ok(vec![canned_window_info()]) })
    }

    fn get_process_tree(&self) -> BoxFuture<'_, Result<Vec<rdpilot::ProcessInfo>, DaemonError>> {
        Box::pin(async {
            Ok(vec![rdpilot::ProcessInfo {
                pid: 1234,
                parent_pid: 4,
                name: "notepad.exe".to_owned(),
                path: r"C:\Windows\notepad.exe".to_owned(),
                command_line: None,
                owner: None,
            }])
        })
    }

    fn get_uia_tree(&self, _hwnd: u64, _scope: rdpilot::UiaScope) -> BoxFuture<'_, Result<Vec<rdpilot::UiaElement>, DaemonError>> {
        Box::pin(async { Ok(vec![canned_uia_element()]) })
    }

    fn send_mouse(&self, _action: rdpilot::MouseAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }

    fn send_key(&self, _action: rdpilot::KeyAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }

    fn set_foreground_window(&self, _hwnd: u64) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }

    fn launch_process(
        &self,
        _exe: String,
        _args: Option<String>,
        _cwd: Option<String>,
    ) -> BoxFuture<'_, Result<u32, DaemonError>> {
        let slow_ms = self.slow_ms;
        Box::pin(async move {
            if slow_ms > 0 {
                tokio::time::sleep(Duration::from_millis(slow_ms)).await;
            }
            Ok(4242)
        })
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
        _local: std::path::PathBuf,
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

/// A canned [`rdpilot::WindowInfo`] shared by [`FakeTestSession::get_window_list`]
/// and [`FakeTestSession::world_state`] so both return the same plausible value.
fn canned_window_info() -> rdpilot::WindowInfo {
    rdpilot::WindowInfo {
        hwnd: 1,
        title: "Notepad".to_owned(),
        rect: rdpilot::Rect { x: 0, y: 0, w: 800, h: 600 },
        z_order: 0,
        state: rdpilot::WindowState::Normal,
        class_name: "Notepad".to_owned(),
        pid: 1234,
    }
}

/// A canned [`rdpilot::UiaElement`] shared by [`FakeTestSession::get_uia_tree`]
/// and [`FakeTestSession::world_state`] so both return the same plausible value.
fn canned_uia_element() -> rdpilot::UiaElement {
    rdpilot::UiaElement {
        id: "42".to_owned(),
        role: "Button".to_owned(),
        name: "OK".to_owned(),
        bbox: rdpilot::Rect { x: 10, y: 10, w: 80, h: 24 },
        enabled: true,
        visible: true,
        focusable: true,
        focused: false,
        depth: 1,
        parent_id: String::new(),
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
}

impl SessionConnector for FakeTestConnector {
    fn connect(&self, _cfg: ConnectionConfig) -> Pin<Box<dyn Future<Output = Result<Box<dyn ManagedSession>, DaemonError>>>> {
        let slow_ms = self.slow_ms;
        let bootstrap_delay_ms = self.bootstrap_delay_ms;
        Box::pin(async move { Ok(Box::new(FakeTestSession { slow_ms, bootstrap_delay_ms }) as Box<dyn ManagedSession>) })
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
    #[tokio::test]
    async fn run_with_an_immediately_empty_registry_and_a_short_grace_self_shuts_down() {
        let dir = std::env::temp_dir().join(format!("rdpilot-daemon-server-run-test-{}", std::process::id()));
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
            .run_until(tokio::time::timeout(Duration::from_secs(5), run_inner_without_bind_for_test(config)))
            .await;

        assert!(result.is_ok(), "run_inner's lifecycle assembly must self-shut-down well within the timeout");

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
            None => JsonReconciliationSink::new().expect("platform cache dir should resolve in this test environment"),
        });

        let connector: Arc<dyn SessionConnector> = Arc::new(FakeTestConnector { slow_ms: 0, bootstrap_delay_ms: 0 });
        let registry = Arc::new(Registry::new(connector, sink.clone() as Arc<dyn ReconciliationSink>));

        let orphans = reconcile::scan_orphans(sink.path());
        reconcile::seed_into(orphans, &registry);

        let shutdown = ShutdownSignal::new();
        let reaper_handle = tokio::task::spawn_local(lifecycle::idle_reaper(Arc::clone(&registry), config.lifecycle, shutdown.clone()));
        let watcher_handle =
            tokio::task::spawn_local(lifecycle::empty_watcher(Arc::clone(&registry), config.lifecycle, shutdown.clone()));

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
        let session = FakeTestSession { slow_ms: 0, bootstrap_delay_ms: 0 };
        let immediate_bound = Duration::from_millis(50);

        let start = std::time::Instant::now();
        session
            .upload_file(std::path::PathBuf::from("/tmp/x"), "remote".to_owned())
            .await
            .expect("upload_file should succeed");
        assert!(start.elapsed() < immediate_bound, "slow_ms=0 upload_file must resolve immediately (env-unset regression guard)");

        let start = std::time::Instant::now();
        session
            .download_file("remote".to_owned(), std::path::PathBuf::from("/tmp/x"))
            .await
            .expect("download_file should succeed");
        assert!(start.elapsed() < immediate_bound, "slow_ms=0 download_file must resolve immediately (env-unset regression guard)");

        let start = std::time::Instant::now();
        session.launch_process("notepad.exe".to_owned(), None, None).await.expect("launch_process should succeed");
        assert!(start.elapsed() < immediate_bound, "slow_ms=0 launch_process must resolve immediately (env-unset regression guard)");
    }

    /// With `slow_ms` set, every slow-class method genuinely sleeps at
    /// least that long before resolving -- the mechanism `tests/non_blocking.rs`
    /// (14-05, MCP-06) relies on to exercise a genuine slow daemon round
    /// trip offline. Fast methods (`ping`) are unaffected regardless.
    #[tokio::test]
    async fn fake_session_slow_class_methods_sleep_the_configured_slow_ms() {
        const SLOW_MS: u64 = 30;
        let session = FakeTestSession { slow_ms: SLOW_MS, bootstrap_delay_ms: 0 };
        let bound = Duration::from_millis(SLOW_MS);

        let start = std::time::Instant::now();
        session
            .upload_file(std::path::PathBuf::from("/tmp/x"), "remote".to_owned())
            .await
            .expect("upload_file should succeed");
        assert!(start.elapsed() >= bound, "slow_ms={SLOW_MS} upload_file must sleep at least that long");

        let start = std::time::Instant::now();
        session
            .download_file("remote".to_owned(), std::path::PathBuf::from("/tmp/x"))
            .await
            .expect("download_file should succeed");
        assert!(start.elapsed() >= bound, "slow_ms={SLOW_MS} download_file must sleep at least that long");

        let start = std::time::Instant::now();
        session.launch_process("notepad.exe".to_owned(), None, None).await.expect("launch_process should succeed");
        assert!(start.elapsed() >= bound, "slow_ms={SLOW_MS} launch_process must sleep at least that long");

        // Fast methods stay immediate regardless of slow_ms (only the
        // three slow-class methods above consult it at all).
        let start = std::time::Instant::now();
        session.ping().await.expect("ping should succeed");
        assert!(start.elapsed() < bound, "ping must stay immediate even when slow_ms is set");
    }
}
