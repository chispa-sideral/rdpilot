//! Idle-session reaper (periodic tokio task) and empty-registry
//! grace-period self-shutdown (Plan 12-06; DAEMON-03, D-31).
//!
//! - [`LifecycleConfig`] carries the three config-overridable durations
//!   D-31 requires: how long a session may sit idle before it is reaped,
//!   how long the registry must stay empty before the daemon self-exits
//!   (the anti-thrash grace period), and how often both watchers poll the
//!   registry. Every field defaults to a sane production value but is
//!   fully injectable so a test can drive the lifecycle on millisecond
//!   timescales.
//! - [`ShutdownSignal`] is a `tokio::sync::watch`-backed broadcast a
//!   `server::run()` accept loop selects on to know when to stop accepting
//!   new connections and exit. Built on `watch` (not `Notify`) specifically
//!   so a LATE waiter (one that starts `wait()`ing after `fire()` already
//!   ran) still observes the fired state -- `Notify::notify_waiters()`
//!   would silently miss that waiter, which is exactly the race an
//!   accept-loop `select!` could hit.
//! - [`idle_reaper`] periodically closes `Live` sessions whose
//!   `last_activity` has exceeded `idle_timeout`, ALWAYS via the awaited
//!   `Registry::close` (DAEMON-01 discipline -- never a bare removal).
//! - [`empty_watcher`] periodically checks whether the registry is empty;
//!   on finding it empty, it sleeps `empty_grace` and re-checks -- if the
//!   registry gained a session during the grace window, the watcher loops
//!   back (anti-thrash, D-31: a rapid disconnect->reconnect must not pay a
//!   full daemon cold-start); if it is STILL empty after the grace, the
//!   watcher fires [`ShutdownSignal`] and returns.

use std::sync::Arc;
use std::time::Duration;

use rdpilot_ipc::SessionId;
use tokio::sync::watch;

use crate::registry::Registry;

/// The three config-overridable lifecycle durations (D-31).
///
/// Every field has a sane production default ([`LifecycleConfig::default`])
/// but is fully constructible/injectable so tests can drive the reaper and
/// watcher on millisecond timescales -- `server::run()` (Plan 12-06) reads
/// these from environment overrides (`RDPILOT_DAEMON_IDLE_TIMEOUT_MS` /
/// `RDPILOT_DAEMON_EMPTY_GRACE_MS` / `RDPILOT_DAEMON_REAP_INTERVAL_MS`) so
/// the offline `autostart_lifecycle` integration test can tune the real
/// compiled binary's lifecycle without a config-file/CLI-flag round trip
/// that does not exist yet in this phase.
#[derive(Debug, Clone, Copy)]
pub struct LifecycleConfig {
    /// How long a `Live` session may sit with no observed activity before
    /// [`idle_reaper`] closes it.
    pub idle_timeout: Duration,
    /// How long the registry must remain empty, after first being observed
    /// empty, before [`empty_watcher`] fires the shutdown signal (the
    /// anti-thrash grace period, D-31).
    pub empty_grace: Duration,
    /// The poll interval both [`idle_reaper`] and [`empty_watcher`] use to
    /// re-scan the registry.
    pub reap_interval: Duration,
}

impl LifecycleConfig {
    /// Production default idle timeout: 30 minutes of no observed activity.
    pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
    /// Production default empty-registry grace period: 30 seconds.
    pub const DEFAULT_EMPTY_GRACE: Duration = Duration::from_secs(30);
    /// Production default poll interval: 15 seconds.
    pub const DEFAULT_REAP_INTERVAL: Duration = Duration::from_secs(15);

    /// The env var overriding [`LifecycleConfig::idle_timeout`], in
    /// milliseconds.
    pub const ENV_IDLE_TIMEOUT_MS: &'static str = "RDPILOT_DAEMON_IDLE_TIMEOUT_MS";
    /// The env var overriding [`LifecycleConfig::empty_grace`], in
    /// milliseconds.
    pub const ENV_EMPTY_GRACE_MS: &'static str = "RDPILOT_DAEMON_EMPTY_GRACE_MS";
    /// The env var overriding [`LifecycleConfig::reap_interval`], in
    /// milliseconds.
    pub const ENV_REAP_INTERVAL_MS: &'static str = "RDPILOT_DAEMON_REAP_INTERVAL_MS";

    /// Build a [`LifecycleConfig`] from the production defaults, overridden
    /// by any of the three `RDPILOT_DAEMON_*_MS` env vars that are present
    /// and parse as a `u64` millisecond count. An absent or unparseable env
    /// var silently falls back to the default for that field -- a
    /// malformed override must never block daemon startup.
    #[must_use]
    pub fn from_env() -> Self {
        let mut cfg = LifecycleConfig::default();
        if let Some(ms) = env_millis(Self::ENV_IDLE_TIMEOUT_MS) {
            cfg.idle_timeout = Duration::from_millis(ms);
        }
        if let Some(ms) = env_millis(Self::ENV_EMPTY_GRACE_MS) {
            cfg.empty_grace = Duration::from_millis(ms);
        }
        if let Some(ms) = env_millis(Self::ENV_REAP_INTERVAL_MS) {
            cfg.reap_interval = Duration::from_millis(ms);
        }
        cfg
    }
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        LifecycleConfig {
            idle_timeout: Self::DEFAULT_IDLE_TIMEOUT,
            empty_grace: Self::DEFAULT_EMPTY_GRACE,
            reap_interval: Self::DEFAULT_REAP_INTERVAL,
        }
    }
}

fn env_millis(var: &str) -> Option<u64> {
    std::env::var(var).ok().and_then(|s| s.parse::<u64>().ok())
}

/// A one-shot, multi-waiter shutdown broadcast a `server::run()` accept
/// loop selects on.
///
/// Backed by `tokio::sync::watch` rather than `tokio::sync::Notify`
/// specifically because `watch::Receiver::changed()` (via the
/// `borrow()`-first check in [`ShutdownSignal::wait`]) observes an
/// already-fired signal even for a waiter that starts waiting AFTER
/// [`ShutdownSignal::fire`] already ran -- `Notify::notify_waiters()` only
/// wakes tasks that are waiting AT THE MOMENT it is called, which would
/// silently miss a late `select!` arm.
#[derive(Debug, Clone)]
pub struct ShutdownSignal {
    tx: watch::Sender<bool>,
}

impl ShutdownSignal {
    /// Construct a fresh, not-yet-fired signal.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        ShutdownSignal { tx }
    }

    /// Fire the signal -- every current and future [`ShutdownSignal::wait`]
    /// caller observes it. Idempotent: firing an already-fired signal is a
    /// no-op (the `watch` channel already holds `true`).
    ///
    /// Deliberately `send_replace`, NOT `Sender::send`: `send` fails
    /// (silently, if its `Result` is discarded) when zero receivers are
    /// currently subscribed -- exactly the state a freshly constructed
    /// [`ShutdownSignal`] is in before any `wait()` caller has ever
    /// subscribed (`ShutdownSignal::new` does not retain the `channel()`
    /// constructor's initial receiver). `send_replace` unconditionally
    /// updates the held value regardless of receiver count, so a `fire()`
    /// that races ahead of every `wait()` call still leaves the correct
    /// `true` value for every subscriber that calls `wait()` afterward
    /// (observed via the immediate `*rx.borrow()` check at the top of
    /// [`ShutdownSignal::wait`]).
    pub fn fire(&self) {
        let _ = self.tx.send_replace(true);
    }

    /// Resolve once the signal has fired (immediately, if it already had).
    pub async fn wait(&self) {
        let mut rx = self.tx.subscribe();
        if *rx.borrow() {
            return;
        }
        // `changed()` only errors if every `Sender` was dropped, which
        // cannot happen here since `self` still holds one -- the `let _`
        // below deliberately ignores that unreachable-in-practice error
        // rather than panicking, mirroring this crate's no-`unwrap`/no-
        // `expect`-outside-poisoned-mutex discipline.
        let _ = rx.changed().await;
    }

    /// `true` once [`ShutdownSignal::fire`] has been called.
    ///
    /// Not yet called from any non-test production code path in this
    /// plan (`server.rs`'s accept loop observes the fired state via
    /// `wait()`, not a poll of `is_fired`) -- kept `pub` as a diagnostic/
    /// completeness API for this type and exercised directly by this
    /// module's own inline tests (mirrors `seams.rs`'s identical
    /// interface-first `#[allow(dead_code)]` treatment of
    /// `RealConnector`/`NoopReconciliationSink`).
    #[must_use]
    #[allow(dead_code)]
    pub fn is_fired(&self) -> bool {
        *self.tx.borrow()
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        ShutdownSignal::new()
    }
}

/// Periodically scan `registry` for `Live` sessions whose `last_activity`
/// has exceeded `cfg.idle_timeout` and close each one via the awaited
/// `Registry::close` (DAEMON-01 discipline: never a bare removal). Returns
/// once `shutdown` fires.
pub async fn idle_reaper(registry: Arc<Registry>, cfg: LifecycleConfig, shutdown: ShutdownSignal) {
    let mut interval = tokio::time::interval(cfg.reap_interval);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let stale: Vec<SessionId> = registry
                    .live_idle_durations()
                    .into_iter()
                    .filter(|(_, idle)| *idle >= cfg.idle_timeout)
                    .map(|(id, _)| id)
                    .collect();
                for id in stale {
                    // Best-effort: a concurrent explicit disconnect may have
                    // already removed this id between the scan above and
                    // this close call -- that race is benign (the session
                    // is gone either way), so the close error is not
                    // treated as fatal to the reaper loop.
                    let _ = registry.close(&id).await;
                }
            }
            () = shutdown.wait() => return,
        }
    }
}

/// Periodically check whether `registry` is empty; on finding it empty,
/// sleep `cfg.empty_grace` and re-check -- if a session appeared during the
/// grace window, loop back (anti-thrash, D-31); if the registry is STILL
/// empty after the grace, fire `shutdown` and return.
pub async fn empty_watcher(registry: Arc<Registry>, cfg: LifecycleConfig, shutdown: ShutdownSignal) {
    let mut interval = tokio::time::interval(cfg.reap_interval);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if registry.is_empty() {
                    tokio::time::sleep(cfg.empty_grace).await;
                    if registry.is_empty() {
                        shutdown.fire();
                        return;
                    }
                    // A session opened during the grace window -- anti-thrash:
                    // do not fire, loop back to the outer scan.
                }
            }
            () = shutdown.wait() => return,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // Test-only fail-fast assertions -- mirrors this crate's other test modules' established convention (e.g. registry.rs/dispatch.rs/reconcile.rs), which this crate-wide #![deny] has never actually been enforced against with `cargo clippy --all-targets` until this plan's own verification pass.
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rdpilot::ConnectionConfig;
    use rdpilot_ipc::SessionLifecycle;

    use super::*;
    use crate::seams::{BoxFuture, DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector};

    type TestFuture<T> = Pin<Box<dyn Future<Output = T>>>;

    /// A fake, immediately-resolving `ManagedSession` -- mirrors
    /// `registry.rs`'s own inline test fake (private to that module's own
    /// `#[cfg(test)]`, so this module defines its own).
    struct FakeSession {
        closed: Arc<AtomicBool>,
    }

    impl ManagedSession for FakeSession {
        fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
            self.closed.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
        fn describe(&self) -> SessionLifecycle {
            SessionLifecycle::Live
        }

        fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
            Box::pin(async { Ok(rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] }) })
        }
        fn world_state(&self, _opts: rdpilot::WorldStateOptions) -> BoxFuture<'_, Result<rdpilot::WorldState, DaemonError>> {
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
        fn get_window_list(&self) -> BoxFuture<'_, Result<Vec<rdpilot::WindowInfo>, DaemonError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn get_process_tree(&self) -> BoxFuture<'_, Result<Vec<rdpilot::ProcessInfo>, DaemonError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn get_uia_tree(&self, _hwnd: u64, _scope: rdpilot::UiaScope) -> BoxFuture<'_, Result<Vec<rdpilot::UiaElement>, DaemonError>> {
            Box::pin(async { Ok(vec![]) })
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
            Box::pin(async { Ok(0) })
        }
        fn upload_file(
            &self,
            _local: std::path::PathBuf,
            _remote_name: String,
        ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
            Box::pin(async { Ok(rdpilot::TransferOutcome { bytes_transferred: 0, checksum: String::new() }) })
        }
        fn download_file(
            &self,
            _remote_name: String,
            _local: std::path::PathBuf,
        ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
            Box::pin(async { Ok(rdpilot::TransferOutcome { bytes_transferred: 0, checksum: String::new() }) })
        }
        fn ping(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
            Box::pin(async { Ok(std::time::Duration::from_millis(0)) })
        }
    }

    struct FakeConnector;

    impl SessionConnector for FakeConnector {
        fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            Box::pin(async { Ok(Box::new(FakeSession { closed: Arc::new(AtomicBool::new(false)) }) as Box<dyn ManagedSession>) })
        }
    }

    fn test_cfg() -> ConnectionConfig {
        ConnectionConfig::new("10.0.0.5", "user", "pw")
    }

    fn fake_registry() -> Arc<Registry> {
        Arc::new(Registry::new(Arc::new(FakeConnector), Arc::new(NoopReconciliationSink)))
    }

    fn tiny_cfg() -> LifecycleConfig {
        LifecycleConfig {
            idle_timeout: Duration::from_millis(20),
            empty_grace: Duration::from_millis(60),
            reap_interval: Duration::from_millis(10),
        }
    }

    // `idle_reaper` awaits `Registry::close`, which awaits a
    // `ManagedSession::close` boxed future that is deliberately NOT `Send`
    // (`seams.rs`'s `BoxFuture` doc comment: `rdpilot::Session::connect`'s
    // real future is not `Send`, so `RealConnector` could not exist if the
    // trait required it). That non-`Send`-ness means `idle_reaper`'s own
    // future is not `Send` either, so it cannot be handed to `tokio::spawn`
    // (which requires `F: Send`) -- exactly the constraint `server.rs`
    // (Plan 12-06) must respect via `LocalSet`/`spawn_local` rather than a
    // bare `tokio::spawn`. These tests honor that same constraint by
    // driving `idle_reaper` and its own timing/assertion future CONCURRENTLY
    // within the SAME task via `tokio::join!` (which polls both futures
    // cooperatively without spawning either onto another thread, so neither
    // needs to be `Send`) instead of spawning a second task.

    #[tokio::test]
    async fn idle_reaper_closes_a_stale_session_via_registry_close_not_a_bare_remove() {
        let registry = fake_registry();
        registry
            .open(Some("web".to_owned()), "10.0.0.5".to_owned(), test_cfg())
            .await
            .expect("open should succeed");
        assert_eq!(registry.len(), 1);

        let cfg = tiny_cfg();
        let shutdown = ShutdownSignal::new();

        let registry_for_driver = Arc::clone(&registry);
        let shutdown_for_driver = shutdown.clone();
        let driver = async move {
            // Give the session time to exceed idle_timeout and the reaper
            // at least one poll cycle to observe and close it.
            tokio::time::sleep(cfg.idle_timeout + cfg.reap_interval * 3).await;
            assert_eq!(registry_for_driver.len(), 0, "the stale session must have been reaped via close()");
            shutdown_for_driver.fire();
        };

        tokio::join!(idle_reaper(Arc::clone(&registry), cfg, shutdown.clone()), driver);
    }

    #[tokio::test]
    async fn idle_reaper_leaves_a_freshly_active_session_alone() {
        let registry = fake_registry();
        registry
            .open(Some("web".to_owned()), "10.0.0.5".to_owned(), test_cfg())
            .await
            .expect("open should succeed");

        // A LONG idle_timeout relative to the observation window: the
        // session's last_activity (set at open time, moments ago) never
        // exceeds it, so it must survive.
        let cfg = LifecycleConfig {
            idle_timeout: Duration::from_secs(3600),
            empty_grace: Duration::from_millis(60),
            reap_interval: Duration::from_millis(10),
        };
        let shutdown = ShutdownSignal::new();

        let registry_for_driver = Arc::clone(&registry);
        let shutdown_for_driver = shutdown.clone();
        let driver = async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(registry_for_driver.len(), 1, "a freshly active session must not be reaped");
            shutdown_for_driver.fire();
        };

        tokio::join!(idle_reaper(Arc::clone(&registry), cfg, shutdown.clone()), driver);
    }

    #[tokio::test]
    async fn empty_watcher_fires_shutdown_after_the_grace_period_elapses_while_still_empty() {
        let registry = fake_registry();
        assert!(registry.is_empty());

        let cfg = tiny_cfg();
        let shutdown = ShutdownSignal::new();
        empty_watcher(Arc::clone(&registry), cfg, shutdown.clone()).await;

        assert!(shutdown.is_fired(), "empty_watcher must fire shutdown once the grace elapses while still empty");
    }

    #[tokio::test]
    async fn empty_watcher_does_not_fire_shutdown_when_a_session_appears_during_the_grace_window() {
        let registry = fake_registry();
        let cfg = tiny_cfg();
        let shutdown = ShutdownSignal::new();

        let watcher_registry = Arc::clone(&registry);
        let watcher_shutdown = shutdown.clone();
        let watcher = tokio::spawn(async move {
            empty_watcher(watcher_registry, cfg, watcher_shutdown).await;
        });

        // Open a session partway through the grace window -- this must
        // cancel the pending shutdown (anti-thrash, D-31).
        tokio::time::sleep(cfg.reap_interval + cfg.empty_grace / 2).await;
        registry
            .open(Some("web".to_owned()), "10.0.0.5".to_owned(), test_cfg())
            .await
            .expect("open should succeed");

        // Give the watcher's re-check (after the grace it was already
        // sleeping through) time to observe the non-empty registry and
        // loop back, plus a further full grace window to prove it does NOT
        // fire on the now-empty-again path either (the session is still
        // live -- the registry is not empty at all during this wait).
        tokio::time::sleep(cfg.empty_grace + cfg.reap_interval * 2).await;
        assert!(
            !shutdown.is_fired(),
            "a session appearing during the grace window must cancel the shutdown (anti-thrash)"
        );

        shutdown.fire(); // stop the watcher task so this test does not hang the runtime on drop
        watcher.await.expect("empty_watcher task should join cleanly after shutdown fires");
    }
}
