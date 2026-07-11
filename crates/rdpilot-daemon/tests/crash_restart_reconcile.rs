//! SC#5 [BLOCKING] offline: crash (`kill -9`, simulated) -> restart ->
//! orphan surfaced in `list` -> explicit reconcile (DAEMON-04, Pitfall 9).
//!
//! This is a separate `tests/` integration-test crate root (like
//! `rdpilot`'s own `tests/live_session.rs` and this crate's sibling
//! `tests/registry_concurrency.rs`/`tests/thread_leak_soak.rs`) — it does
//! NOT inherit `rdpilot-daemon/src/lib.rs`'s inner
//! `#![deny(clippy::expect_used)]`/`unwrap_used` (those scope to the lib
//! crate's own compilation unit only), so ordinary `.expect()`/`.unwrap()`
//! calls below are fine, mirroring the established codebase convention.
//!
//! **What this proves (offline):** a session opened through `Registry` A
//! (backed by a `JsonReconciliationSink` at a shared on-disk path) leaves a
//! record on disk. Dropping `Registry` A WITHOUT calling `close()` models a
//! `kill -9` — no graceful teardown runs, so `record_closed` is never
//! invoked and the on-disk record survives the "crash". A fresh `Registry`
//! B constructed on the SAME sink path, after running `scan_orphans` +
//! `seed_into` (the startup reconciliation Plan 12-06's `run()` performs),
//! surfaces the prior session as `SessionLifecycle::Orphaned` in `list` —
//! never silently forgotten (DAEMON-04's core failure mode) and never
//! auto-killed (D-31) — only an explicit `Registry::close` call on the
//! orphan reconciles it, at which point the disk record is removed too.
//!
//! **Deliberately NOT proven here (deferred to Plan 12-07's live gate):**
//! that the remote Windows session is genuinely STILL LIVE server-side
//! (vs. having actually logged off) — that claim requires a real RDP
//! target and cannot be established offline.

use std::future::Future;
use std::pin::Pin;

use rdpilot::ConnectionConfig;
use rdpilot_daemon::{
    DaemonError, JsonReconciliationSink, ManagedSession, ReconciliationRecord, Registry, SessionConnector,
};
use rdpilot_ipc::SessionLifecycle;

type TestFuture<T> = Pin<Box<dyn Future<Output = T>>>;
/// Like `TestFuture`, but lifetime-parameterized -- required for the
/// `&self`-based operational `ManagedSession` methods (Phase 13), whose
/// trait-declared `BoxFuture<'_, T>` ties the returned future's lifetime
/// to the `&self` borrow (not `'static`, unlike `close`/`connect`).
type OpFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A fake, immediately-resolving `ManagedSession` — this test is about the
/// disk record + surfacing, not about a real OS thread (the thread-owning
/// fake used by the DAEMON-01 soak proof lives in `tests/thread_leak_soak.rs`).
struct FakeSession;

impl ManagedSession for FakeSession {
    fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
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
}

/// A fake `SessionConnector` that always succeeds immediately, producing a
/// `FakeSession` — reused (in shape) from `registry.rs`'s own inline test
/// fakes, kept local here since this file is a separate compilation unit.
struct FakeConnector;

impl SessionConnector for FakeConnector {
    fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
        Box::pin(async { Ok(Box::new(FakeSession) as Box<dyn ManagedSession>) })
    }
}

fn test_cfg() -> ConnectionConfig {
    ConnectionConfig::new("10.0.0.9", "user", "pw")
}

/// A unique temp state-file path per test run (no `tempfile` crate
/// dependency — a monotonic-ish suffix from the process id + a
/// nanosecond-resolution timestamp keeps concurrent test binaries from
/// colliding, mirroring `reconcile.rs`'s own inline-test path helper).
fn unique_state_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("rdpilot-daemon-crash-restart-reconcile-{}-{nanos}.json", std::process::id()))
}

#[tokio::test]
async fn kill_minus_9_then_restart_surfaces_the_orphan_which_is_then_explicitly_reconciled() {
    let state_path = unique_state_path();

    // --- Phase A: Registry-A opens a session -> a record lands on disk. ---
    let sink_a = std::sync::Arc::new(JsonReconciliationSink::at(state_path.clone()));
    let registry_a = Registry::new(std::sync::Arc::new(FakeConnector), sink_a);
    let payroll_id = registry_a
        .open(Some("payroll".to_owned()), "10.0.0.9".to_owned(), test_cfg())
        .await
        .expect("open should succeed against the fake connector");
    assert_eq!(payroll_id.as_str(), "payroll");

    let after_open: Vec<ReconciliationRecord> = rdpilot_daemon::scan_orphans(&state_path);
    assert_eq!(after_open.len(), 1, "the disk record must exist immediately after a successful open");
    assert_eq!(after_open[0].id, "payroll");
    assert_eq!(after_open[0].host, "10.0.0.9");

    // --- Phase B: simulate `kill -9` -- drop Registry-A WITHOUT close(). ---
    // No graceful teardown runs: `record_closed` is never invoked, so the
    // disk record survives the "crash" exactly as it would if the real
    // daemon process had been killed with no chance to clean up.
    drop(registry_a);

    let after_crash: Vec<ReconciliationRecord> = rdpilot_daemon::scan_orphans(&state_path);
    assert_eq!(
        after_crash.len(),
        1,
        "the record must still be on disk after an unclean drop (the kill -9 surrogate)"
    );

    // --- Phase C: "restart" -- a fresh Registry-B on the SAME sink path. ---
    let sink_b = std::sync::Arc::new(JsonReconciliationSink::at(state_path.clone()));
    let registry_b = Registry::new(std::sync::Arc::new(FakeConnector), sink_b);

    let scanned = rdpilot_daemon::scan_orphans(&state_path);
    rdpilot_daemon::seed_into(scanned, &registry_b);

    // Anti-pattern guard (DAEMON-04's exact failure mode): `list()` must
    // NOT be empty immediately after restart -- the orphan was surfaced,
    // not silently forgotten.
    let after_restart = registry_b.list();
    assert!(
        !after_restart.is_empty(),
        "the orphan must be surfaced after restart, never silently forgotten (DAEMON-04)"
    );
    assert_eq!(after_restart.len(), 1);
    assert_eq!(after_restart[0].id, "payroll");
    assert_eq!(after_restart[0].host, "10.0.0.9");
    assert_eq!(
        after_restart[0].status,
        SessionLifecycle::Orphaned,
        "a leftover reconciliation record must surface as Orphaned, not Live/Connecting -- \
         it is never blindly reconnected or auto-killed (D-31)"
    );

    // --- Phase D: explicit reconcile -- Registry-B.close() on the orphan. ---
    registry_b
        .close(&payroll_id)
        .await
        .expect("reconciling (closing) a surfaced orphan should succeed");

    let after_reconcile_scan = rdpilot_daemon::scan_orphans(&state_path);
    assert!(
        after_reconcile_scan.is_empty(),
        "the disk record must be removed once the orphan is explicitly reconciled"
    );
    let after_reconcile_list = registry_b.list();
    assert!(
        after_reconcile_list.is_empty(),
        "the orphan must no longer be listed once explicitly reconciled"
    );

    let _ = std::fs::remove_file(&state_path);
}
