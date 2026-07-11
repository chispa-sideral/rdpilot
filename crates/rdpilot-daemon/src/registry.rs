//! The in-memory session registry: atomic claim-then-connect insert,
//! close-not-drop teardown, auto-id minting, and the credential-free
//! `list` snapshot (SESSION-01/04, DAEMON-01).
//!
//! The two hardest guarantees this file proves (research Patterns 1/2):
//!
//! - **Atomic claim-then-connect** ([`Registry::open`]): a name/id is
//!   reserved under the registry [`Mutex`] synchronously — no `.await`
//!   while the lock is held — released, THEN the slow async
//!   [`SessionConnector::connect`] runs. On success the registry is
//!   re-locked briefly to upgrade the placeholder; on failure it is
//!   re-locked briefly to remove the claim (so a retry with the same name
//!   can succeed). This is the ONLY correct way to make "N simultaneous
//!   same-name connects yield exactly one live session" true.
//! - **Close-not-drop teardown** ([`Registry::close`]): every code path
//!   that removes a `Live` entry from the map extracts the owned session
//!   and awaits its `close()` — it never lets the removed value simply go
//!   out of scope. `Session::drop`'s non-blocking fallback is a
//!   best-effort backstop, not the normal path.

// `Registry` and its methods are exercised by this file's own inline
// tests and the sibling `tests/registry_concurrency.rs` /
// `tests/thread_leak_soak.rs` integration tests (Tasks 2/3), but are not
// yet CALLED from any non-test crate code — `dispatch.rs` (Plan 12-04)
// and `server.rs` (Plan 12-06) are the future production callers. Silence
// the resulting `dead_code` lint at the module level rather than
// per-item, matching the interface-first nature of this plan (mirrors
// `seams.rs`'s per-item `#[allow(dead_code)]` on `RealConnector`/
// `NoopReconciliationSink` for the same reason).
#![allow(dead_code)]

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rdpilot::ConnectionConfig;
use rdpilot_ipc::{SessionId, SessionStatus};

use crate::seams::{DaemonError, ReconciliationSink, SessionConnector, SessionEntry};

/// Word lists for [`generate_auto_id`] (D-29: short, human-legible
/// adjective-noun auto-generated ids, e.g. `brave-otter`). Deliberately
/// small, fixed, and offline — no wordlist crate dependency.
const ADJECTIVES: &[&str] = &[
    "brave", "quiet", "swift", "calm", "bold", "clever", "gentle", "lucky", "quick", "wise", "eager", "sunny",
    "amber", "civil", "dapper", "earnest",
];
const NOUNS: &[&str] = &[
    "otter", "falcon", "badger", "heron", "lynx", "raven", "wolf", "fox", "hawk", "owl", "otter2", "marten",
    "kestrel", "beetle", "sparrow", "cricket",
];

/// Bound on [`Registry::claim_with_auto_id`]'s retry loop — collisions are
/// expected to be exceedingly rare (an auto-id retries with a freshly
/// minted pair on each attempt, reusing the SAME atomic insert path as a
/// caller-supplied name; research "Don't Hand-Roll": no second "generate
/// and hope" code path).
const AUTO_ID_MAX_ATTEMPTS: u32 = 32;

/// Monotonic per-process counter mixed into [`generate_auto_id`]'s seed so
/// two calls within the same nanosecond still diverge.
static AUTO_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Mint a short, human-legible adjective-noun auto-generated session id
/// (D-29, e.g. `brave-otter`). Pure formatting/selection — no I/O, no new
/// crate dependency (a `DefaultHasher`-mixed seed from the current time,
/// process id, and a monotonic counter selects the word pair).
fn generate_auto_id() -> String {
    let counter = AUTO_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (nanos, std::process::id(), counter).hash(&mut hasher);
    let seed = hasher.finish();

    let adjective = ADJECTIVES[(seed as usize) % ADJECTIVES.len()];
    let noun = NOUNS[((seed >> 32) as usize) % NOUNS.len()];
    format!("{adjective}-{noun}")
}

/// Render the current wall-clock time as an ISO-8601 / RFC 3339 UTC
/// timestamp (`YYYY-MM-DDTHH:MM:SSZ`), with no `chrono`/date-crate
/// dependency — a small, pure, offline-testable civil-calendar conversion
/// (Howard Hinnant's `civil_from_days` algorithm) from Unix seconds.
fn iso8601_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    iso8601_from_unix_seconds(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// Pure conversion from Unix seconds (UTC) to an ISO-8601 timestamp string
/// — factored out of [`iso8601_now`] so it is unit-testable against known
/// reference points without depending on the wall clock.
fn iso8601_from_unix_seconds(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's `civil_from_days`: converts a day count relative to the
/// Unix epoch (1970-01-01) into a proleptic-Gregorian `(year, month, day)`
/// triple. Widely used, numerically exact reference algorithm — see
/// <http://howardhinnant.github.io/date_algorithms.html#civil_from_days>.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1); // [1, 31]
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1); // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// The in-memory session registry (DAEMON-01, SESSION-01/04).
pub struct Registry {
    sessions: Mutex<HashMap<SessionId, SessionEntry>>,
    connector: Arc<dyn SessionConnector>,
    sink: Arc<dyn ReconciliationSink>,
}

impl Registry {
    /// Construct an empty registry over the given (injectable — production
    /// wiring in Plan 12-06, fakes in tests) connector and reconciliation
    /// sink.
    #[must_use]
    pub fn new(connector: Arc<dyn SessionConnector>, sink: Arc<dyn ReconciliationSink>) -> Self {
        Registry {
            sessions: Mutex::new(HashMap::new()),
            connector,
            sink,
        }
    }

    /// Atomically claim `id` as a `Connecting` placeholder.
    ///
    /// The ENTIRE lock scope is synchronous (no `.await` inside) — this is
    /// what makes the claim atomic w.r.t. every other concurrent caller
    /// (research Pattern 1). Returns `Err(DuplicateSession)` if `id` is
    /// already claimed (`Connecting`), live, or orphaned.
    fn claim(&self, id: &SessionId) -> Result<(), DaemonError> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let mut guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.entry(id.clone()) {
            Entry::Occupied(_) => Err(DaemonError::DuplicateSession(id.as_str().to_owned())),
            Entry::Vacant(slot) => {
                slot.insert(SessionEntry::Connecting { claimed_at: Instant::now() });
                Ok(())
            }
        }
    }

    /// Mint and atomically claim a fresh auto-generated id (D-29), retrying
    /// with a freshly minted pair on collision — reusing [`Registry::claim`],
    /// the SAME atomic insert path a caller-supplied name uses (research
    /// "Don't Hand-Roll": no second "generate and hope" code path).
    fn claim_with_auto_id(&self) -> Result<SessionId, DaemonError> {
        for _ in 0..AUTO_ID_MAX_ATTEMPTS {
            let candidate = generate_auto_id();
            let Ok(id) = SessionId::from_str(&candidate) else {
                // `generate_auto_id` always produces a non-empty string;
                // this branch is unreachable in practice but handled
                // without unwrap/expect/panic (API-01 discipline mirrored
                // from `rdpilot`).
                continue;
            };
            if self.claim(&id).is_ok() {
                return Ok(id);
            }
        }
        Err(DaemonError::Connect(
            "could not mint a unique auto-generated session id after repeated attempts".to_owned(),
        ))
    }

    /// Open a new session under `name` (or an auto-generated id when
    /// `name` is `None`) against `host` (research Pattern 1: claim, THEN
    /// connect OUTSIDE the lock, THEN upgrade/rollback).
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::DuplicateSession`] if `name` is already
    /// claimed/live/orphaned, or the connector's error (mapped through
    /// [`DaemonError`]) if the connect itself fails — in which case the
    /// claim is released so a retry with the same name can succeed.
    pub async fn open(&self, name: Option<String>, host: String, cfg: ConnectionConfig) -> Result<SessionId, DaemonError> {
        let id = match &name {
            Some(n) => {
                let id = SessionId::from_str(n).map_err(DaemonError::Connect)?;
                self.claim(&id)?;
                id
            }
            None => self.claim_with_auto_id()?,
        }; // claim() has already released the lock by this point — nothing
           // is held across the `.await` below.

        match self.connector.connect(cfg).await {
            Ok(session) => {
                // A single wall-clock capture shared by the entry's
                // `connected_since_wall`/`last_activity_wall` and the
                // reconciliation sink's `record_open` call below — avoids
                // two independent `SystemTime::now()` reads racing apart
                // by a few milliseconds for what is conceptually one event.
                let now_wall = iso8601_now();
                {
                    #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
                    let mut guard = self.sessions.lock().expect("registry mutex poisoned");
                    guard.insert(
                        id.clone(),
                        SessionEntry::Live {
                            session,
                            connected_since: Instant::now(),
                            connected_since_wall: now_wall.clone(),
                            name,
                            host: host.clone(),
                            last_activity: Instant::now(),
                            last_activity_wall: now_wall.clone(),
                        },
                    );
                } // guard dropped here — never held across the .await below
                self.sink.record_open(&id, &host, &now_wall);
                Ok(id)
            }
            Err(e) => {
                #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
                let mut guard = self.sessions.lock().expect("registry mutex poisoned");
                guard.remove(&id); // release the claim so a retry can succeed
                Err(e)
            }
        }
    }

    /// Close `id`: extract the entry, then await its `close()` — never let
    /// a removed `Live` entry drop bare (research Pattern 2, DAEMON-01's
    /// critical invariant).
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::SessionNotFound`] if `id` has no entry,
    /// [`DaemonError::StillConnecting`] if `id`'s connect is still in
    /// flight (the in-flight connect is never interrupted — the
    /// `Connecting` placeholder is put back so its upgrade/rollback re-lock
    /// still finds its slot), or the underlying close/join error.
    pub async fn close(&self, id: &SessionId) -> Result<(), DaemonError> {
        let entry = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            guard.remove(id) // extract, don't let it drop in this scope
        };

        match entry {
            Some(SessionEntry::Live { session, .. }) => {
                session.close().await?; // joins the OS thread — NEVER a bare drop
                self.sink.record_closed(id);
                Ok(())
            }
            Some(SessionEntry::Connecting { claimed_at }) => {
                // A connect is in flight for this id — do not interrupt it.
                // Put the placeholder back so Registry::open's own
                // upgrade/rollback re-lock still finds its slot.
                #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
                let mut guard = self.sessions.lock().expect("registry mutex poisoned");
                guard.insert(id.clone(), SessionEntry::Connecting { claimed_at });
                Err(DaemonError::StillConnecting(id.as_str().to_owned()))
            }
            Some(SessionEntry::Orphaned { .. }) => {
                // Explicit reclaim/teardown of an orphan (DAEMON-04) —
                // clears its record; there is no live OS thread to join.
                self.sink.record_closed(id);
                Ok(())
            }
            None => Err(DaemonError::SessionNotFound(id.as_str().to_owned())),
        }
    }

    /// A credential-free snapshot of every session currently known to the
    /// registry (D-30/D-31) — used by `list` (Plan 12-04).
    #[must_use]
    pub fn list(&self) -> Vec<SessionStatus> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard.iter().map(|(id, entry)| entry.to_status(id)).collect()
    }

    /// Insert an `Orphaned` entry directly (no I/O here — used by the
    /// startup reconciliation scan, Plan 12-05/12-06, to seed a
    /// possibly-still-live remote session surfaced after a crash-restart,
    /// DAEMON-04).
    pub fn seed_orphan(&self, id: SessionId, host: String, connected_since: String) {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let mut guard = self.sessions.lock().expect("registry mutex poisoned");
        guard.insert(id, SessionEntry::Orphaned { host, connected_since });
    }

    /// The number of entries currently in the registry (test/diagnostic
    /// helper — mirrors `list().len()` without allocating `SessionStatus`
    /// values).
    #[must_use]
    pub fn len(&self) -> usize {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard.len()
    }

    /// `true` when the registry has no entries (DAEMON-03's
    /// empty-registry self-shutdown watcher, Plan 12-06, will poll this).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every `Live` entry's id paired with the elapsed time since its
    /// `last_activity` (DAEMON-03's idle reaper, Plan 12-06, uses this to
    /// decide which sessions are stale). `Connecting`/`Orphaned` entries
    /// have no `last_activity` to age and are never candidates for idle
    /// reap -- only a `Live` entry is included.
    #[must_use]
    pub fn live_idle_durations(&self) -> Vec<(SessionId, std::time::Duration)> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard
            .iter()
            .filter_map(|(id, entry)| match entry {
                SessionEntry::Live { last_activity, .. } => Some((id.clone(), last_activity.elapsed())),
                SessionEntry::Connecting { .. } | SessionEntry::Orphaned { .. } => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU32};

    use rdpilot_ipc::SessionLifecycle;

    use super::*;
    use crate::seams::{ManagedSession, NoopReconciliationSink, SessionConnector};

    type TestFuture<T> = Pin<Box<dyn Future<Output = T>>>;

    /// A fake, immediately-resolving `ManagedSession` (no real OS thread —
    /// the thread-owning fake used by the DAEMON-01 soak proof lives in
    /// `tests/thread_leak_soak.rs`, Task 3).
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
    }

    /// A fake `SessionConnector` whose `connect` either always succeeds or
    /// always fails (configurable), optionally sleeping first to widen a
    /// concurrency race window. Tracks how many `FakeSession`s it has
    /// produced and whether each was closed, via a shared `closed` flag
    /// per session — sufficient for this file's own single-threaded-
    /// registry-behavior tests (the N-way concurrency proof lives in
    /// `tests/registry_concurrency.rs`, Task 2).
    struct FakeConnector {
        fail: bool,
        connect_count: AtomicU32,
    }

    impl FakeConnector {
        fn succeeding() -> Self {
            FakeConnector { fail: false, connect_count: AtomicU32::new(0) }
        }
        fn failing() -> Self {
            FakeConnector { fail: true, connect_count: AtomicU32::new(0) }
        }
    }

    impl SessionConnector for FakeConnector {
        fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    Err(DaemonError::Connect("fake connect failure".to_owned()))
                } else {
                    Ok(Box::new(FakeSession { closed: Arc::new(AtomicBool::new(false)) }) as Box<dyn ManagedSession>)
                }
            })
        }
    }

    fn test_cfg() -> ConnectionConfig {
        ConnectionConfig::new("10.0.0.5", "user", "pw")
    }

    fn succeeding_registry() -> Registry {
        Registry::new(Arc::new(FakeConnector::succeeding()), Arc::new(NoopReconciliationSink))
    }

    fn failing_registry() -> Registry {
        Registry::new(Arc::new(FakeConnector::failing()), Arc::new(NoopReconciliationSink))
    }

    #[tokio::test]
    async fn open_with_a_caller_supplied_name_inserts_connects_and_upgrades_to_live() {
        let registry = succeeding_registry();
        let id = registry
            .open(Some("web".to_owned()), "10.0.0.5".to_owned(), test_cfg())
            .await
            .expect("open should succeed");
        assert_eq!(id.as_str(), "web");
        assert_eq!(registry.len(), 1);
        let statuses = registry.list();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "web");
        assert_eq!(statuses[0].status, SessionLifecycle::Live);
    }

    #[tokio::test]
    async fn open_with_an_already_live_name_is_rejected_as_duplicate() {
        let registry = succeeding_registry();
        registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("first open should succeed");

        let second = registry.open(Some("web".to_owned()), "h".to_owned(), test_cfg()).await;
        assert!(matches!(second, Err(DaemonError::DuplicateSession(name)) if name == "web"));
        // Exactly one entry survives the rejected duplicate.
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn open_with_no_name_mints_an_auto_generated_id() {
        let registry = succeeding_registry();
        let id = registry.open(None, "h".to_owned(), test_cfg()).await.expect("open should succeed");
        assert!(id.as_str().contains('-'), "expected a hyphenated auto-id: {id:?}");
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_connect_removes_the_placeholder_so_the_name_is_reusable() {
        let registry = failing_registry();
        let first = registry.open(Some("web".to_owned()), "h".to_owned(), test_cfg()).await;
        assert!(first.is_err(), "fake connector is configured to fail");
        assert_eq!(registry.len(), 0, "the Connecting placeholder must be released on connect failure");

        // The name must be reusable immediately after the failure — proves
        // the claim was actually released, not merely errored past.
        let second = registry.open(Some("web".to_owned()), "h".to_owned(), test_cfg()).await;
        assert!(second.is_err(), "still using the failing connector");
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn close_extracts_the_live_entry_and_awaits_its_close() {
        let registry = succeeding_registry();
        let id = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("open should succeed");

        registry.close(&id).await.expect("close should succeed");
        assert_eq!(registry.len(), 0, "a closed session must be removed from the registry");

        // Closing again is a clean SessionNotFound, not a panic/leak.
        let again = registry.close(&id).await;
        assert!(matches!(again, Err(DaemonError::SessionNotFound(name)) if name == "web"));
    }

    #[tokio::test]
    async fn close_on_a_still_connecting_id_is_rejected_and_the_placeholder_survives() {
        let registry = Registry::new(Arc::new(FakeConnector::succeeding()), Arc::new(NoopReconciliationSink));
        // Manually seed a Connecting placeholder (as `open` would, mid-flight)
        // without actually running a connect, to exercise `close`'s
        // still-connecting branch in isolation.
        let id = SessionId::from_str("web").expect("non-empty literal");
        registry.claim(&id).expect("claim should succeed on an empty registry");

        let result = registry.close(&id).await;
        assert!(matches!(result, Err(DaemonError::StillConnecting(name)) if name == "web"));
        // The placeholder must survive the rejected close (put back), not
        // be lost — the map must still show exactly one entry.
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn close_on_an_unknown_id_is_session_not_found() {
        let registry = succeeding_registry();
        let id = SessionId::from_str("ghost").expect("non-empty literal");
        let result = registry.close(&id).await;
        assert!(matches!(result, Err(DaemonError::SessionNotFound(name)) if name == "ghost"));
    }

    #[tokio::test]
    async fn close_on_an_orphaned_entry_clears_it_without_a_close_call() {
        let registry = succeeding_registry();
        let id = SessionId::from_str("orphan").expect("non-empty literal");
        registry.seed_orphan(id.clone(), "10.0.0.9".to_owned(), "2026-01-01T00:00:00Z".to_owned());
        assert_eq!(registry.len(), 1);

        registry.close(&id).await.expect("reclaiming an orphan should succeed");
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn list_reports_connected_since_and_last_activity_for_a_live_session() {
        // SESSION-03 field-completeness: a live session's `list` entry
        // carries a real (non-`None`) `connected_since`/`last_activity`
        // wall-clock timestamp, not the placeholder `None` a `Connecting`
        // entry legitimately reports.
        let registry = succeeding_registry();
        registry
            .open(Some("web".to_owned()), "10.0.0.5".to_owned(), test_cfg())
            .await
            .expect("open should succeed");

        let statuses = registry.list();
        assert_eq!(statuses.len(), 1);
        let status = &statuses[0];
        assert_eq!(status.name.as_deref(), Some("web"));
        assert_eq!(status.host, "10.0.0.5");
        assert_eq!(status.status, SessionLifecycle::Live);
        assert!(status.connected_since.is_some(), "connected_since must be populated for a live session");
        assert!(status.last_activity.is_some(), "last_activity must be populated for a live session");
    }

    #[test]
    fn list_never_carries_a_credential_field() {
        // Structural guarantee: `SessionStatus` (rdpilot-ipc) has no
        // password/credential field at all — this test documents that the
        // registry's `list()` return type cannot leak one by construction,
        // mirroring rdpilot-ipc's own CONFIG-03 structural test style.
        let registry = succeeding_registry();
        let statuses = registry.list();
        assert!(statuses.is_empty());
        // Compile-time structural check: SessionStatus's fields are id /
        // name / host / status / connected_since / last_activity only.
        fn _assert_shape(s: &SessionStatus) {
            let _ = (&s.id, &s.name, &s.host, &s.status, &s.connected_since, &s.last_activity);
        }
    }

    #[tokio::test]
    async fn live_idle_durations_reports_only_live_entries_and_grows_with_elapsed_time() {
        let registry = succeeding_registry();
        registry
            .open(Some("web".to_owned()), "10.0.0.5".to_owned(), test_cfg())
            .await
            .expect("open should succeed");
        // Seed an Orphaned entry too -- it must never be reported here (only
        // Live entries have a last_activity to age).
        let orphan_id = SessionId::from_str("orphan").expect("non-empty literal");
        registry.seed_orphan(orphan_id, "10.0.0.9".to_owned(), "2026-01-01T00:00:00Z".to_owned());

        let first = registry.live_idle_durations();
        assert_eq!(first.len(), 1, "only the Live entry should be reported, not the Orphaned one");
        assert_eq!(first[0].0.as_str(), "web");

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let second = registry.live_idle_durations();
        assert!(second[0].1 >= first[0].1, "elapsed idle duration must not go backwards");
    }

    #[test]
    fn iso8601_from_unix_seconds_matches_known_reference_points() {
        // 1970-01-01T00:00:00Z (Unix epoch).
        assert_eq!(iso8601_from_unix_seconds(0), "1970-01-01T00:00:00Z");
        // 2024-01-01T00:00:00Z == 1704067200.
        assert_eq!(iso8601_from_unix_seconds(1_704_067_200), "2024-01-01T00:00:00Z");
        // 2000-03-01T00:00:00Z == 951868800 (crosses a leap-year boundary:
        // 2000 IS a leap year, exercising the civil_from_days century/400
        // rule correctly).
        assert_eq!(iso8601_from_unix_seconds(951_868_800), "2000-03-01T00:00:00Z");
        // A time-of-day mid-value.
        assert_eq!(iso8601_from_unix_seconds(1_704_067_200 + 3661), "2024-01-01T01:01:01Z");
    }

    #[test]
    fn generate_auto_id_produces_a_hyphenated_two_word_id() {
        let id = generate_auto_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 2, "expected exactly one hyphen: {id}");
        assert!(ADJECTIVES.contains(&parts[0]), "unexpected adjective: {id}");
        assert!(NOUNS.contains(&parts[1]), "unexpected noun: {id}");
    }

    #[test]
    fn generate_auto_id_varies_across_calls() {
        let ids: std::collections::HashSet<String> = (0..20).map(|_| generate_auto_id()).collect();
        assert!(ids.len() > 1, "20 calls should not all collide on one word pair: {ids:?}");
    }
}
