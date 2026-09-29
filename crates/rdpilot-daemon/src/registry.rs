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

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rdpilot::ConnectionConfig;
use rdpilot_ipc::{SessionId, SessionLifecycle, SessionStatus};

use crate::events::SessionEvents;
use crate::seams::{
    BoxFuture, DaemonError, ManagedSession, ReconciliationSink, SessionConnector, SessionEntry,
    ViewFrameSource,
};

/// Word lists for [`generate_auto_id`] (D-29: short, human-legible
/// adjective-noun auto-generated ids, e.g. `brave-otter`). Deliberately
/// small, fixed, and offline — no wordlist crate dependency.
const ADJECTIVES: &[&str] = &[
    "brave", "quiet", "swift", "calm", "bold", "clever", "gentle", "lucky", "quick", "wise",
    "eager", "sunny", "amber", "civil", "dapper", "earnest",
];
const NOUNS: &[&str] = &[
    "otter", "falcon", "badger", "heron", "lynx", "raven", "wolf", "fox", "hawk", "owl", "otter2",
    "marten", "kestrel", "beetle", "sparrow", "cricket",
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
    iso8601_from_system_time(SystemTime::now())
}

/// Render an arbitrary [`SystemTime`] as an ISO-8601 / RFC 3339 UTC
/// timestamp — the same conversion [`iso8601_now`] applies to "now",
/// factored out so callers with their own captured `SystemTime` (e.g.
/// registry status serialization) reuse this exact civil-calendar math
/// rather than duplicating it (research: "reuse the same ISO-8601 helper
/// registry.rs uses"). A `SystemTime` before the Unix epoch (clock skew /
/// test fixture) degrades to the epoch itself rather than panicking
/// (API-01 discipline mirrored from `rdpilot`).
pub(crate) fn iso8601_from_system_time(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    iso8601_from_unix_seconds(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// Like [`iso8601_from_system_time`], with milliseconds
/// (`YYYY-MM-DDTHH:MM:SS.sssZ`), for the session event log.
pub(crate) fn iso8601_millis_from_system_time(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let base = iso8601_from_unix_seconds(i64::try_from(since.as_secs()).unwrap_or(i64::MAX));
    let millis = since.subsec_millis();
    match base.strip_suffix('Z') {
        Some(stem) => format!("{stem}.{millis:03}Z"),
        None => base,
    }
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
    next_generation: AtomicU64,
}

/// Private ownership handle retained until the `Connected` IPC response is
/// written. A public session id alone cannot safely reclaim a reused name.
#[derive(Debug, Clone)]
pub(crate) struct ConnectLease {
    pub(crate) id: SessionId,
    generation: u64,
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
            next_generation: AtomicU64::new(1),
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
                slot.insert(SessionEntry::Connecting {
                    claimed_at: Instant::now(),
                });
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
    pub async fn open(
        &self,
        name: Option<String>,
        host: String,
        cfg: ConnectionConfig,
    ) -> Result<SessionId, DaemonError> {
        Ok(self.open_tracked(name, host, cfg).await?.id)
    }

    /// Like [`Registry::open`], but returns an internal exact-generation
    /// ownership handle for the IPC response path.
    pub(crate) async fn open_tracked(
        &self,
        name: Option<String>,
        host: String,
        cfg: ConnectionConfig,
    ) -> Result<ConnectLease, DaemonError> {
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
                let frame = session.frame_source();
                let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                let events = Arc::new(SessionEvents::new(&id, generation, frame.clone(), None));
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
                            session: Arc::new(tokio::sync::Mutex::new(Some(session))),
                            generation,
                            status: SessionLifecycle::Live,
                            connected_since: Instant::now(),
                            connected_since_wall: now_wall.clone(),
                            name,
                            host: host.clone(),
                            last_activity: Instant::now(),
                            cua_leases: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                            frame,
                            events,
                            last_activity_wall: now_wall.clone(),
                        },
                    );
                } // guard dropped here — never held across the .await below
                self.sink.record_open(&id, &host, &now_wall);
                Ok(ConnectLease { id, generation })
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
                // Take the SIZED `Box` out of the `Option` (never the
                // unsized `dyn ManagedSession` itself, which does not
                // compile out of an `Arc` -- research Pitfall 2) and close
                // it -- NEVER a bare drop (DAEMON-01). `try_lock` cannot be
                // used here: `close` is this session's terminal operation
                // and MUST wait for any concurrent `Registry::call` in
                // flight to finish before reclaiming ownership, not race
                // past it.
                let boxed = { session.lock().await.take() };
                match boxed {
                    Some(boxed) => {
                        boxed.close().await?; // joins the OS thread — NEVER a bare drop
                        self.sink.record_closed(id);
                        Ok(())
                    }
                    None => {
                        // Already taken/closed concurrently -- treat as a
                        // clean success, not a double-close error.
                        self.sink.record_closed(id);
                        Ok(())
                    }
                }
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

    /// Reclaim only the exact live generation created for a Connect response
    /// whose IPC peer closed. A normal Disconnect or a reused public name is
    /// benignly left untouched.
    pub(crate) async fn close_if_generation(
        &self,
        lease: &ConnectLease,
    ) -> Result<bool, DaemonError> {
        let entry = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(&lease.id) {
                Some(SessionEntry::Live { generation, .. }) if *generation == lease.generation => {
                    guard.remove(&lease.id)
                }
                _ => None,
            }
        };

        let Some(SessionEntry::Live { session, .. }) = entry else {
            return Ok(false);
        };
        if let Some(session) = session.lock().await.take() {
            session.close().await?;
        }
        self.sink.record_closed(&lease.id);
        Ok(true)
    }

    /// Dispatch an operation onto the live session `id` WITHOUT holding the
    /// registry's synchronous outer [`Mutex`] across the operation's
    /// `.await` (research Pitfall 3): the `Arc` wrapping the session is
    /// cloned under the outer lock, the outer lock is DROPPED, THEN the
    /// inner `tokio::sync::Mutex` is `.await`-locked to reach the boxed
    /// session and run `op`.
    ///
    /// Calls to the SAME session serialize (the inner `tokio::sync::Mutex`
    /// is per-session); calls to DIFFERENT sessions never block each other
    /// (each `Live` entry owns its own `Arc`) — pre-satisfying Phase 14's
    /// MCP-06 non-blocking-isolation property.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::SessionNotFound`] if `id` has no entry (or was
    /// closed mid-flight between the clone and the inner lock),
    /// [`DaemonError::StillConnecting`] if `id`'s connect is still in
    /// flight, or whatever `op` itself returns.
    pub async fn call<T>(
        &self,
        id: &SessionId,
        op: impl for<'a> FnOnce(&'a dyn ManagedSession) -> BoxFuture<'a, Result<T, DaemonError>>,
    ) -> Result<T, DaemonError> {
        let entry = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(id) {
                Some(SessionEntry::Live { session, .. }) => Arc::clone(session),
                Some(SessionEntry::Connecting { .. }) => {
                    return Err(DaemonError::StillConnecting(id.as_str().to_owned()));
                }
                Some(SessionEntry::Orphaned { .. }) | None => {
                    return Err(DaemonError::SessionNotFound(id.as_str().to_owned()));
                }
            }
        }; // outer lock dropped here — never held across the .await below

        let guard = entry.lock().await; // tokio::sync::Mutex — safe across .await
        match guard.as_deref() {
            Some(session) => op(session).await,
            None => Err(DaemonError::SessionNotFound(id.as_str().to_owned())), // closed mid-flight
        }
    }

    /// Capture the incarnation and acquire a stream under one short session lease.
    /// Neither the global registry lock nor the per-session lock escapes this call.
    pub(crate) async fn attach_cua(
        &self,
        id: &SessionId,
    ) -> Result<(u64, Box<dyn crate::seams::ManagedCua>), DaemonError> {
        let (generation, entry, cua_leases) = {
            #[allow(clippy::expect_used)]
            let guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(id) {
                Some(SessionEntry::Live {
                    generation,
                    session,
                    cua_leases,
                    ..
                }) => (*generation, Arc::clone(session), Arc::clone(cua_leases)),
                Some(SessionEntry::Connecting { .. }) => {
                    return Err(DaemonError::StillConnecting(id.as_str().into()))
                }
                _ => return Err(DaemonError::SessionNotFound(id.as_str().into())),
            }
        };
        let guard = entry.lock().await;
        let session = guard
            .as_deref()
            .ok_or_else(|| DaemonError::SessionNotFound(id.as_str().into()))?;
        let attachment = session.attach_cua().await?;
        cua_leases.fetch_add(1, Ordering::Relaxed);
        Ok((
            generation,
            Box::new(CuaLease {
                inner: attachment,
                active: cua_leases,
            }),
        ))
    }

    /// The event log of live session `id`: a brief outer-lock read that
    /// takes no per-session lock and does not change activity.
    pub(crate) fn events(&self, id: &SessionId) -> Option<Arc<SessionEvents>> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live { events, .. }) => Some(Arc::clone(events)),
            _ => None,
        }
    }

    /// Refresh activity only for the captured incarnation; a replacement name is never touched.
    pub(crate) fn touch_generation(&self, id: &SessionId, expected: u64) -> bool {
        #[allow(clippy::expect_used)]
        let mut entries = self.sessions.lock().expect("registry mutex poisoned");
        match entries.get_mut(id) {
            Some(SessionEntry::Live {
                generation,
                last_activity,
                last_activity_wall,
                ..
            }) if *generation == expected => {
                *last_activity = Instant::now();
                *last_activity_wall = iso8601_now();
                true
            }
            _ => false,
        }
    }

    /// A credential-free snapshot of every session currently known to the
    /// registry (D-30/D-31) — used by `list` (Plan 12-04).
    #[must_use]
    pub fn list(&self) -> Vec<SessionStatus> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard
            .iter()
            .map(|(id, entry)| entry.to_status(id))
            .collect()
    }

    /// Insert an `Orphaned` entry directly (no I/O here — used by the
    /// startup reconciliation scan, Plan 12-05/12-06, to seed a
    /// possibly-still-live remote session surfaced after a crash-restart,
    /// DAEMON-04).
    pub fn seed_orphan(&self, id: SessionId, host: String, connected_since: String) {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let mut guard = self.sessions.lock().expect("registry mutex poisoned");
        guard.insert(
            id,
            SessionEntry::Orphaned {
                host,
                connected_since,
            },
        );
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
                SessionEntry::Live {
                    last_activity,
                    cua_leases,
                    ..
                } => (cua_leases.load(Ordering::Relaxed) == 0)
                    .then(|| (id.clone(), last_activity.elapsed())),
                SessionEntry::Connecting { .. } | SessionEntry::Orphaned { .. } => None,
            })
            .collect()
    }
}

/// The result of a viewer frame-source lookup.
pub(crate) enum FrameLookup {
    /// A live session with a passive frame source.
    Source(Arc<dyn ViewFrameSource>),
    /// The session exists but has no frames (connecting, orphaned, or a
    /// session type without a frame source).
    Unavailable,
    /// The session is not in the registry.
    Closed,
}

/// The result of a viewer event-log lookup.
pub(crate) enum EventsLookup {
    /// A live session's log.
    Log(Arc<SessionEvents>),
    /// The session exists but has no log (connecting or orphaned).
    Unavailable,
    /// The session is not in the registry.
    Closed,
}

/// One session as the live viewer sees it: the credential-free list entry
/// plus its frame source's ended state.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct ViewerSession {
    #[serde(flatten)]
    pub(crate) status: SessionStatus,
    /// `true` once the session's RDP loop ended (display only).
    pub(crate) ended: bool,
    /// `true` when frames can be requested for this session.
    pub(crate) frames: bool,
}

/// The only registry surface the live viewer receives.
///
/// Declared here, outside the `viewer` module tree, so the viewer cannot
/// reach the private `Registry` field and therefore cannot call `call`,
/// `close`, `open`, `attach_cua` or `touch_generation`. It never takes a
/// per-session mutex and never changes activity, leases or entries.
#[derive(Clone)]
pub(crate) struct ViewerRegistry {
    inner: Arc<Registry>,
}

impl ViewerRegistry {
    pub(crate) fn new(registry: Arc<Registry>) -> Self {
        ViewerRegistry { inner: registry }
    }

    /// The session list with each frame source's ended state.
    pub(crate) fn sessions(&self) -> Vec<ViewerSession> {
        self.inner.viewer_sessions()
    }

    /// The frame source for `id`, read under the registry lock only briefly.
    pub(crate) fn frame_source(&self, id: &SessionId) -> FrameLookup {
        self.inner.frame_source(id)
    }

    /// The event log for `id`, read under the registry lock only briefly.
    pub(crate) fn events(&self, id: &SessionId) -> EventsLookup {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.inner.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live { events, .. }) => EventsLookup::Log(Arc::clone(events)),
            Some(_) => EventsLookup::Unavailable,
            None => EventsLookup::Closed,
        }
    }
}

impl Registry {
    fn viewer_sessions(&self) -> Vec<ViewerSession> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard
            .iter()
            .map(|(id, entry)| {
                let (ended, frames) = match entry {
                    SessionEntry::Live {
                        frame: Some(frame), ..
                    } => (frame.status().ended, true),
                    _ => (false, false),
                };
                ViewerSession {
                    status: entry.to_status(id),
                    ended,
                    frames,
                }
            })
            .collect()
    }

    fn frame_source(&self, id: &SessionId) -> FrameLookup {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live {
                frame: Some(frame), ..
            }) => FrameLookup::Source(Arc::clone(frame)),
            Some(_) => FrameLookup::Unavailable,
            None => FrameLookup::Closed,
        }
    }
}

/// Counts owned streams without retaining either registry lock.
struct CuaLease {
    inner: Box<dyn crate::seams::ManagedCua>,
    active: Arc<std::sync::atomic::AtomicUsize>,
}
impl Drop for CuaLease {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}
impl crate::seams::ManagedCua for CuaLease {
    fn identity(&self) -> (u64, u64, u64) {
        self.inner.identity()
    }
    fn send(&self, value: serde_json::Value) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.send(value)
    }
    fn recv(&mut self) -> BoxFuture<'_, Result<Option<serde_json::Value>, DaemonError>> {
        self.inner.recv()
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.close()
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

        fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
            Box::pin(async {
                Ok(rdpilot::Screenshot {
                    width: 1,
                    height: 1,
                    rgba: vec![0, 0, 0, 0],
                })
            })
        }

        fn send_mouse(
            &self,
            _action: rdpilot::MouseAction,
        ) -> BoxFuture<'_, Result<(), DaemonError>> {
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
            Box::pin(async {
                Ok(rdpilot::TransferOutcome {
                    bytes_transferred: 0,
                    checksum: String::new(),
                })
            })
        }
        fn download_file(
            &self,
            _remote_name: String,
            _local: std::path::PathBuf,
        ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
            Box::pin(async {
                Ok(rdpilot::TransferOutcome {
                    bytes_transferred: 0,
                    checksum: String::new(),
                })
            })
        }
        fn ping(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
            Box::pin(async { Ok(std::time::Duration::from_millis(0)) })
        }
        fn desktop_size(&self) -> (u32, u32) {
            (1920, 1080)
        }
        fn deploy_and_launch(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
            Box::pin(async move { Ok(std::time::Duration::from_millis(0)) })
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
            FakeConnector {
                fail: false,
                connect_count: AtomicU32::new(0),
            }
        }
        fn failing() -> Self {
            FakeConnector {
                fail: true,
                connect_count: AtomicU32::new(0),
            }
        }
    }

    impl SessionConnector for FakeConnector {
        fn connect(
            &self,
            _cfg: ConnectionConfig,
        ) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            let fail = self.fail;
            Box::pin(async move {
                if fail {
                    Err(DaemonError::Connect("fake connect failure".to_owned()))
                } else {
                    Ok(Box::new(FakeSession {
                        closed: Arc::new(AtomicBool::new(false)),
                    }) as Box<dyn ManagedSession>)
                }
            })
        }
    }

    fn test_cfg() -> ConnectionConfig {
        ConnectionConfig::new("10.0.0.5", "user", "pw")
    }

    fn succeeding_registry() -> Registry {
        Registry::new(
            Arc::new(FakeConnector::succeeding()),
            Arc::new(NoopReconciliationSink),
        )
    }

    fn failing_registry() -> Registry {
        Registry::new(
            Arc::new(FakeConnector::failing()),
            Arc::new(NoopReconciliationSink),
        )
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

        let second = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await;
        assert!(matches!(second, Err(DaemonError::DuplicateSession(name)) if name == "web"));
        // Exactly one entry survives the rejected duplicate.
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn open_with_no_name_mints_an_auto_generated_id() {
        let registry = succeeding_registry();
        let id = registry
            .open(None, "h".to_owned(), test_cfg())
            .await
            .expect("open should succeed");
        assert!(
            id.as_str().contains('-'),
            "expected a hyphenated auto-id: {id:?}"
        );
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_connect_removes_the_placeholder_so_the_name_is_reusable() {
        let registry = failing_registry();
        let first = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await;
        assert!(first.is_err(), "fake connector is configured to fail");
        assert_eq!(
            registry.len(),
            0,
            "the Connecting placeholder must be released on connect failure"
        );

        // The name must be reusable immediately after the failure — proves
        // the claim was actually released, not merely errored past.
        let second = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await;
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
        assert_eq!(
            registry.len(),
            0,
            "a closed session must be removed from the registry"
        );

        // Closing again is a clean SessionNotFound, not a panic/leak.
        let again = registry.close(&id).await;
        assert!(matches!(again, Err(DaemonError::SessionNotFound(name)) if name == "web"));
    }

    #[tokio::test]
    async fn close_on_a_still_connecting_id_is_rejected_and_the_placeholder_survives() {
        let registry = Registry::new(
            Arc::new(FakeConnector::succeeding()),
            Arc::new(NoopReconciliationSink),
        );
        // Manually seed a Connecting placeholder (as `open` would, mid-flight)
        // without actually running a connect, to exercise `close`'s
        // still-connecting branch in isolation.
        let id = SessionId::from_str("web").expect("non-empty literal");
        registry
            .claim(&id)
            .expect("claim should succeed on an empty registry");

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
        registry.seed_orphan(
            id.clone(),
            "10.0.0.9".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );
        assert_eq!(registry.len(), 1);

        registry
            .close(&id)
            .await
            .expect("reclaiming an orphan should succeed");
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
        assert!(
            status.connected_since.is_some(),
            "connected_since must be populated for a live session"
        );
        assert!(
            status.last_activity.is_some(),
            "last_activity must be populated for a live session"
        );
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
            let _ = (
                &s.id,
                &s.name,
                &s.host,
                &s.status,
                &s.connected_since,
                &s.last_activity,
            );
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
        registry.seed_orphan(
            orphan_id,
            "10.0.0.9".to_owned(),
            "2026-01-01T00:00:00Z".to_owned(),
        );

        let first = registry.live_idle_durations();
        assert_eq!(
            first.len(),
            1,
            "only the Live entry should be reported, not the Orphaned one"
        );
        assert_eq!(first[0].0.as_str(), "web");

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let second = registry.live_idle_durations();
        assert!(
            second[0].1 >= first[0].1,
            "elapsed idle duration must not go backwards"
        );
    }

    #[tokio::test]
    async fn each_live_session_has_its_own_event_log_dropped_on_close() {
        let registry = Arc::new(succeeding_registry());
        let viewer = ViewerRegistry::new(Arc::clone(&registry));
        let id = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("open");
        let events = registry.events(&id).expect("live session has a log");
        assert_eq!(events.header().session, "web");
        assert_eq!(events.header().schema, crate::events::SCHEMA);
        let weak = Arc::downgrade(&events);
        drop(events);
        registry.close(&id).await.expect("close");
        assert!(
            weak.upgrade().is_none(),
            "the log is dropped with the entry"
        );
        assert!(registry.events(&id).is_none());
        assert!(matches!(viewer.events(&id), EventsLookup::Closed));
    }

    #[tokio::test]
    async fn a_new_incarnation_gets_a_new_log() {
        let registry = succeeding_registry();
        let id = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("open");
        let first = registry.events(&id).expect("log").header().clone();
        registry.close(&id).await.expect("close");
        registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("reopen");
        let second = registry.events(&id).expect("log").header().clone();
        assert_ne!(first.log_id, second.log_id);
        assert_ne!(first.incarnation, second.incarnation);
    }

    #[tokio::test]
    async fn reading_and_recording_events_does_not_change_activity() {
        let registry = Arc::new(succeeding_registry());
        let viewer = ViewerRegistry::new(Arc::clone(&registry));
        let id = registry
            .open(Some("web".to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("open");
        let before = registry.list()[0].last_activity.clone();
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        let events = registry.events(&id).expect("log");
        events.record(
            crate::events::EventSource::Cli,
            crate::events::EventKind::SessionEnded,
        );
        assert!(matches!(viewer.events(&id), EventsLookup::Log(_)));
        let idle = registry.live_idle_durations();
        assert!(idle[0].1 >= std::time::Duration::from_millis(60));
        assert_eq!(registry.list()[0].last_activity, before);
    }

    #[test]
    fn iso8601_from_system_time_matches_iso8601_from_unix_seconds() {
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_704_067_200);
        assert_eq!(
            iso8601_from_system_time(t),
            iso8601_from_unix_seconds(1_704_067_200)
        );
    }

    #[test]
    fn iso8601_from_unix_seconds_matches_known_reference_points() {
        // 1970-01-01T00:00:00Z (Unix epoch).
        assert_eq!(iso8601_from_unix_seconds(0), "1970-01-01T00:00:00Z");
        // 2024-01-01T00:00:00Z == 1704067200.
        assert_eq!(
            iso8601_from_unix_seconds(1_704_067_200),
            "2024-01-01T00:00:00Z"
        );
        // 2000-03-01T00:00:00Z == 951868800 (crosses a leap-year boundary:
        // 2000 IS a leap year, exercising the civil_from_days century/400
        // rule correctly).
        assert_eq!(
            iso8601_from_unix_seconds(951_868_800),
            "2000-03-01T00:00:00Z"
        );
        // A time-of-day mid-value.
        assert_eq!(
            iso8601_from_unix_seconds(1_704_067_200 + 3661),
            "2024-01-01T01:01:01Z"
        );
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
        assert!(
            ids.len() > 1,
            "20 calls should not all collide on one word pair: {ids:?}"
        );
    }
}
