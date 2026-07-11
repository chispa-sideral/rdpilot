//! SC#3 [BLOCKING]: after N connect/disconnect cycles, OS thread count and
//! process RSS return to baseline (DAEMON-01) — the direct, offline-
//! testable proof that every registry teardown path routes through an
//! awaited `close()`, never a bare drop.
//!
//! This is a separate `tests/` integration-test crate root (like
//! `rdpilot`'s own `tests/live_session.rs`) — ordinary `.expect()`/
//! `.unwrap()` calls below do not trip `rdpilot-daemon/src/lib.rs`'s inner
//! `#![deny(clippy::expect_used)]`/`unwrap_used` (those scope to the lib
//! crate's own compilation unit only).
//!
//! **Control assertion (documented, not executed):** if `Registry::close`'s
//! `Live` arm were changed to let the removed `SessionEntry` simply drop
//! (bypassing `session.close().await`), `ThreadOwningFakeSession`'s
//! background thread would never receive its stop signal and would never
//! be joined — the post-run thread count would climb by roughly one per
//! leaked cycle instead of returning to baseline, and this test would fail
//! deterministically. This is the exact DAEMON-01 regression the
//! close-not-drop discipline (research Pattern 2) exists to prevent; the
//! test is not run against an intentionally-broken `Registry` (there is
//! only one `Registry` implementation in this crate), but the mechanism by
//! which a regression would be caught is exactly this thread-count
//! assertion.

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;

use rdpilot::ConnectionConfig;
use rdpilot_daemon::{DaemonError, ManagedSession, Registry, ReconciliationSink, SessionConnector};
use rdpilot_ipc::SessionLifecycle;
use sysinfo::{ProcessesToUpdate, System};

type TestFuture<T> = Pin<Box<dyn Future<Output = T>>>;
/// Like `TestFuture`, but lifetime-parameterized -- required for the
/// `&self`-based operational `ManagedSession` methods (Phase 13), whose
/// trait-declared `BoxFuture<'_, T>` ties the returned future's lifetime
/// to the `&self` borrow (not `'static`, unlike `close`/`connect`).
type OpFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Mimics `rdpilot::Session`'s dedicated-OS-thread-per-session model: a
/// real `std::thread` spawned on connect, parked (blocked on a channel
/// recv) until `close()` signals it to stop, then joined via
/// `spawn_blocking` — mirroring `Session::close`'s own join shape exactly.
struct ThreadOwningFakeSession {
    stop_tx: std_mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl ManagedSession for ThreadOwningFakeSession {
    fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
        Box::pin(async move {
            let mut this = self;
            // Best-effort: if the thread already exited for some reason,
            // the send simply fails and is ignored (mirrors
            // `Session::close`'s own best-effort shutdown-request send).
            let _ = this.stop_tx.send(());
            if let Some(thread) = this.thread.take() {
                tokio::task::spawn_blocking(move || thread.join())
                    .await
                    .map_err(|e| DaemonError::Io(format!("join task failed: {e}")))?
                    .map_err(|_| DaemonError::Io("fake session thread panicked".to_owned()))?;
            }
            Ok(())
        })
    }

    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
    }

    fn screenshot(&self) -> OpFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async { Ok(rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] }) })
    }
    fn world_state(&self, _opts: rdpilot::WorldStateOptions) -> OpFuture<'_, Result<rdpilot::WorldState, DaemonError>> {
        Box::pin(async {
            Ok(rdpilot::WorldState {
                timestamp: std::time::SystemTime::now(),
                capture_span: std::time::Duration::from_millis(0),
                screenshot: None,
                window_list: None,
                uia: None,
            })
        })
    }
    fn get_window_list(&self) -> OpFuture<'_, Result<Vec<rdpilot::WindowInfo>, DaemonError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn get_process_tree(&self) -> OpFuture<'_, Result<Vec<rdpilot::ProcessInfo>, DaemonError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn get_uia_tree(&self, _hwnd: u64, _scope: rdpilot::UiaScope) -> OpFuture<'_, Result<Vec<rdpilot::UiaElement>, DaemonError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn send_mouse(&self, _action: rdpilot::MouseAction) -> OpFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn send_key(&self, _action: rdpilot::KeyAction) -> OpFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn set_foreground_window(&self, _hwnd: u64) -> OpFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn launch_process(
        &self,
        _exe: String,
        _args: Option<String>,
        _cwd: Option<String>,
    ) -> OpFuture<'_, Result<u32, DaemonError>> {
        Box::pin(async { Ok(0) })
    }
    fn upload_file(
        &self,
        _local: std::path::PathBuf,
        _remote_name: String,
    ) -> OpFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Ok(rdpilot::TransferOutcome { bytes_transferred: 0, checksum: String::new() }) })
    }
    fn download_file(
        &self,
        _remote_name: String,
        _local: std::path::PathBuf,
    ) -> OpFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Ok(rdpilot::TransferOutcome { bytes_transferred: 0, checksum: String::new() }) })
    }
    fn ping(&self) -> OpFuture<'_, Result<std::time::Duration, DaemonError>> {
        Box::pin(async { Ok(std::time::Duration::from_millis(0)) })
    }
    fn desktop_size(&self) -> (u32, u32) {
        (1920, 1080)
    }
}

struct ThreadOwningFakeConnector;

impl SessionConnector for ThreadOwningFakeConnector {
    fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
        Box::pin(async move {
            let (stop_tx, stop_rx) = std_mpsc::channel::<()>();
            let thread = std::thread::Builder::new()
                .name("rdpilot-daemon-fake-session".to_owned())
                .spawn(move || {
                    // Park until told to stop — mirrors a real session
                    // loop's dedicated OS thread staying alive until
                    // `close()`'s shutdown signal arrives.
                    let _ = stop_rx.recv();
                })
                .map_err(|e| DaemonError::Io(format!("failed to spawn fake session thread: {e}")))?;
            Ok(Box::new(ThreadOwningFakeSession { stop_tx, thread: Some(thread) }) as Box<dyn ManagedSession>)
        })
    }
}

struct NoopSink;

impl ReconciliationSink for NoopSink {
    fn record_open(&self, _id: &rdpilot_ipc::SessionId, _host: &str, _connected_since: &str) {}
    fn record_closed(&self, _id: &rdpilot_ipc::SessionId) {}
}

fn test_cfg() -> ConnectionConfig {
    ConnectionConfig::new("10.0.0.5", "user", "pw")
}

/// Serializes the two tests in this file against each other. `Threads:` in
/// `/proc/self/status` is a WHOLE-PROCESS metric — Rust's default test
/// harness runs `#[test]` functions in PARALLEL within one process, so
/// without this lock the fast-smoke and full-soak tests' own Tokio runtime
/// worker/blocking threads would contaminate each other's baseline/after
/// readings (live-observed during this file's authoring: the fast test's
/// still-shutting-down runtime threads were captured as part of the soak
/// test's "baseline", producing a nonsensical after < baseline result).
static SOAK_SERIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read the current process's live OS thread count from `/proc/self/status`
/// (Linux-native, offline-testable on this host — filtered line read, never
/// a bare `grep -c` shell-out).
fn read_thread_count() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Threads:") {
            return rest.trim().parse().expect("parse Threads: value");
        }
    }
    panic!("Threads: line not found in /proc/self/status");
}

/// Read the current process's RSS in bytes via `sysinfo`.
fn read_rss_bytes() -> u64 {
    let mut sys = System::new();
    let pid = sysinfo::get_current_pid().expect("get_current_pid");
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).map(sysinfo::Process::memory).unwrap_or(0)
}

/// Run `cycles` sequential open->close cycles against a fresh registry
/// backed by [`ThreadOwningFakeConnector`], returning the
/// `(thread_count, rss_bytes)` baseline captured before any session opens
/// and the same pair captured after every cycle has completed.
///
/// Builds its OWN Tokio runtime (rather than relying on `#[tokio::test]`'s
/// default multi-thread runtime) with a very short `thread_keep_alive`
/// (research/live discovery, this file's authoring): `Registry::close`'s
/// `spawn_blocking(move || thread.join())` call (mirroring
/// `Session::close`'s own join shape, research Pattern 2) runs on Tokio's
/// BLOCKING thread pool, whose threads linger for tokio's 10-SECOND
/// default keep-alive after finishing work before being torn down — an
/// idle pooled blocking-thread is a legitimate, harmless Tokio
/// implementation detail, but it is NOT the `ThreadOwningFakeSession`
/// thread under test, and it would make a short post-cycle settle window
/// observe a stale, still-lingering pool thread as a false-positive
/// "leak". A short `thread_keep_alive` (10ms) makes the blocking pool
/// reap its idle threads promptly, so the settle delay below reliably
/// observes the TRUE thread count.
fn run_cycles(cycles: usize) -> ((u64, u64), (u64, u64)) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_keep_alive(std::time::Duration::from_millis(10))
        .enable_all()
        .build()
        .expect("build a dedicated runtime with a short blocking-pool keep-alive");

    rt.block_on(async {
        // Settle before the baseline capture so any prior test's threads/
        // allocations in this process have had a chance to quiesce, and so
        // this runtime's own worker threads have fully started.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let baseline = (read_thread_count(), read_rss_bytes());

        let registry = Registry::new(std::sync::Arc::new(ThreadOwningFakeConnector), std::sync::Arc::new(NoopSink));

        for _ in 0..cycles {
            let id = registry
                .open(None, "10.0.0.5".to_owned(), test_cfg())
                .await
                .expect("open should succeed against the thread-owning fake connector");
            registry.close(&id).await.expect("close should join the fake session's thread");
        }

        // Settle past the short `thread_keep_alive` window so any blocking-
        // pool thread that ran a `spawn_blocking(join)` call has been
        // reaped, letting the thread-count assertion observe the TRUE
        // post-cycle count rather than a still-lingering pool thread.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let after = (read_thread_count(), read_rss_bytes());

        (baseline, after)
    })
}

/// Fast smoke variant (N=3) — runs in the default `cargo test` suite so the
/// close-not-drop discipline is regression-guarded even without opting
/// into the full soak.
#[test]
fn thread_count_returns_to_baseline_after_a_few_cycles() {
    let _guard = SOAK_SERIALIZE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (baseline, after) = run_cycles(3);
    assert_eq!(
        after.0, baseline.0,
        "thread count must fully return to baseline after close() cycles: baseline={}, after={}",
        baseline.0, after.0
    );
}

/// The full SC#3 [BLOCKING] soak: N=50 sequential connect/disconnect
/// cycles, asserting BOTH thread count (exact return-to-baseline) and RSS
/// (within a tolerance band, accounting for allocator retention).
///
/// `#[ignore]`-gated per the existing `crates/rdpilot/tests/live_session.rs`
/// convention (heavy test, opt-in via `-- --ignored`/`--include-ignored`),
/// mirrored by this plan's own `<verify>` block.
#[test]
#[ignore = "heavy soak (N=50 real OS thread spawn/join cycles) -- run via `-- --include-ignored`"]
fn thread_and_rss_return_to_baseline_after_fifty_cycles() {
    let _guard = SOAK_SERIALIZE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    const CYCLES: usize = 50;
    // Allocator retention (glibc/jemalloc arenas rarely fully return freed
    // pages to the OS) means RSS is not expected to land back at EXACTLY
    // the pre-run byte count — a generous few-MiB tolerance band is used,
    // while thread count (a hard OS-kernel-tracked integer with no
    // retention concept) is asserted EXACTLY equal to baseline.
    const RSS_TOLERANCE_BYTES: u64 = 8 * 1024 * 1024;

    let (baseline, after) = run_cycles(CYCLES);

    assert_eq!(
        after.0, baseline.0,
        "thread count must fully return to baseline after {CYCLES} connect/disconnect cycles: baseline={}, after={}",
        baseline.0, after.0
    );
    assert!(
        after.1 <= baseline.1 + RSS_TOLERANCE_BYTES,
        "RSS must return to within {RSS_TOLERANCE_BYTES} bytes of baseline after {CYCLES} cycles: baseline={} bytes, after={} bytes",
        baseline.1,
        after.1
    );
}
