//! SC#1: N simultaneous same-name connects yield exactly one live session
//! and N-1 clean `DuplicateSession` rejections (SESSION-01, SESSION-04).
//!
//! This is a separate `tests/` integration-test crate root (like
//! `rdpilot`'s own `tests/live_session.rs`) — it does NOT inherit
//! `rdpilot-daemon/src/lib.rs`'s inner `#![deny(clippy::expect_used)]`/
//! `unwrap_used` attributes (those scope to the lib crate's own
//! compilation unit only), so ordinary `.expect()`/`.unwrap()` calls below
//! are fine, mirroring the established codebase convention.
//!
//! `Registry::open`/`close` await `SessionConnector`/`ManagedSession`
//! futures that are deliberately NOT `Send` (see
//! `rdpilot-daemon/src/seams.rs`'s `BoxFuture` doc comment — a real
//! constraint discovered while implementing `RealConnector` against
//! `rdpilot::Session::connect`). A plain `tokio::spawn` therefore cannot
//! host a task that awaits `registry.open(..)` (it requires a `Send`
//! future). This file instead drives its concurrent tasks via
//! `tokio::task::LocalSet` + `spawn_local`, which explicitly supports
//! `!Send` futures — still genuinely concurrent/interleaved (cooperative
//! multitasking across `.await` points on one OS thread), which is exactly
//! what proves the atomic-insert race: the fake connector's `sleep`-then-
//! succeed shape widens the window so every task reaches the claim step
//! before any of them completes its connect.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rdpilot::ConnectionConfig;
use rdpilot_daemon::{DaemonError, ManagedSession, Registry, ReconciliationSink, SessionConnector};
use rdpilot_ipc::SessionLifecycle;

type TestFuture<T> = Pin<Box<dyn Future<Output = T>>>;

struct FakeSession;

impl ManagedSession for FakeSession {
    fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
    }
}

/// Sleeps briefly before succeeding, widening the atomic-insert race
/// window so every concurrently-spawned caller reaches the claim step
/// before any of them completes its connect (research SC#1 test shape).
struct SlowFakeConnector;

impl SessionConnector for SlowFakeConnector {
    fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(Box::new(FakeSession) as Box<dyn ManagedSession>)
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

/// N=16 simultaneous same-name `open` calls: exactly one must win, the
/// other 15 must cleanly fail with `DuplicateSession`, and the registry
/// must hold exactly one entry afterward.
#[tokio::test]
async fn n_simultaneous_same_name_connects_yield_exactly_one_winner() {
    const N: usize = 16;

    let registry = Arc::new(Registry::new(Arc::new(SlowFakeConnector), Arc::new(NoopSink)));

    let local = tokio::task::LocalSet::new();
    let results: Vec<Result<rdpilot_ipc::SessionId, DaemonError>> = local
        .run_until(async {
            let mut handles = Vec::with_capacity(N);
            for _ in 0..N {
                let registry = Arc::clone(&registry);
                let cfg = test_cfg();
                handles.push(tokio::task::spawn_local(async move {
                    registry.open(Some("same-name".to_owned()), "10.0.0.5".to_owned(), cfg).await
                }));
            }
            let mut results = Vec::with_capacity(N);
            for handle in handles {
                results.push(handle.await.expect("spawned task must not panic"));
            }
            results
        })
        .await;

    let ok_count = results.iter().filter(|r| r.is_ok()).count();
    let duplicate_count = results
        .iter()
        .filter(|r| matches!(r, Err(DaemonError::DuplicateSession(name)) if name == "same-name"))
        .count();

    assert_eq!(ok_count, 1, "expected exactly one winner, got {ok_count} of {N}: {results:?}");
    assert_eq!(duplicate_count, N - 1, "expected {} DuplicateSession rejections, got {duplicate_count}", N - 1);
    assert_eq!(registry.list().len(), 1, "registry must hold exactly one entry after the race");
}

/// N unnamed (auto-id) `open` calls racing concurrently must yield N
/// distinct auto-generated ids and N live entries — no collisions leak
/// through the SAME atomic-claim path `generate_auto_id`'s retry loop
/// reuses.
#[tokio::test]
async fn n_simultaneous_unnamed_connects_yield_n_distinct_auto_ids() {
    const N: usize = 16;

    let registry = Arc::new(Registry::new(Arc::new(SlowFakeConnector), Arc::new(NoopSink)));

    let local = tokio::task::LocalSet::new();
    let results: Vec<Result<rdpilot_ipc::SessionId, DaemonError>> = local
        .run_until(async {
            let mut handles = Vec::with_capacity(N);
            for _ in 0..N {
                let registry = Arc::clone(&registry);
                let cfg = test_cfg();
                handles.push(tokio::task::spawn_local(async move {
                    registry.open(None, "10.0.0.5".to_owned(), cfg).await
                }));
            }
            let mut results = Vec::with_capacity(N);
            for handle in handles {
                results.push(handle.await.expect("spawned task must not panic"));
            }
            results
        })
        .await;

    let ids: std::collections::HashSet<String> =
        results.into_iter().map(|r| r.expect("unnamed open should never collide-reject").as_str().to_owned()).collect();

    assert_eq!(ids.len(), N, "expected {N} distinct auto-generated ids, got {}: {ids:?}", ids.len());
    assert_eq!(registry.list().len(), N);
}
