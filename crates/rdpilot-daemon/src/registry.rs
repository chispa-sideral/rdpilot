//! The in-memory session registry: atomic claim-then-connect insert,
//! close-not-drop teardown, auto-id minting, and the credential-free
//! `list` snapshot (SESSION-01/04, DAEMON-01).
//!
//! The two hardest guarantees this file proves (research Patterns 1/2):
//!
//! - **Atomic claim-then-connect** ([`Registry::open`]): a name/id is
//!   reserved under the registry [`Mutex`] synchronously — no `.await`
//!   while the lock is held — released, THEN the slow async
//!   [`SessionConnector::connect`] runs on its own task. On success the
//!   registry is re-locked briefly to upgrade the placeholder, but only the
//!   placeholder of the same attempt; on failure it is re-locked briefly to
//!   remove the claim (so a retry with the same name can succeed). Removing
//!   the placeholder cancels the connect; a `Disconnect` is the one caller
//!   that removes another future's placeholder, and it sends the cause.
//!   This is the ONLY correct way to make "N simultaneous same-name
//!   connects yield exactly one live session" true.
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
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rdpilot::ConnectionConfig;
use rdpilot_ipc::{SessionId, SessionLifecycle, SessionStatus};
use rdpilot_vocab::RawInput;
use tokio::sync::oneshot;

use crate::control::{
    EndReason, Grant, InputError, InputReport, NotHeld, SessionControl, TakeError,
};
use crate::events::{CloseReason, EventSource, RecordingTrigger, SessionEvents};
use crate::recording::{RecordingService, Target};
use crate::seams::{
    BoxFuture, BundleSource, CancelCause, DaemonError, ManagedSession, NoBundleSource,
    ReconciliationSink, SessionConnector, SessionEntry, ViewFrameSource,
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
    bundles: Arc<dyn BundleSource>,
    next_generation: AtomicU64,
    recordings: Arc<RecordingService>,
}

/// Private ownership handle retained until the `Connected` IPC response is
/// written. A public session id alone cannot safely reclaim a reused name.
#[derive(Debug, Clone)]
pub(crate) struct ConnectLease {
    pub(crate) id: SessionId,
    generation: u64,
}

/// The error of a connect whose placeholder was removed before its session
/// was registered. It maps to the wire code `internal`.
fn connect_cancelled() -> DaemonError {
    DaemonError::Connect("connect cancelled before the session was registered".to_owned())
}

/// The error of a connect that a `Disconnect` cancelled on purpose. It maps to
/// the wire code `internal`.
fn cancelled_by(cause: CancelCause) -> DaemonError {
    match cause {
        CancelCause::Disconnect => {
            DaemonError::Connect("connect cancelled by disconnect".to_owned())
        }
    }
}

/// What the connect task hands to [`Registry::open_tracked`]: the session
/// together with the cancel receiver of its placeholder, so a promotion that
/// finds the placeholder gone can read why, or the error of the connect.
type Delivered = Result<(Box<dyn ManagedSession>, oneshot::Receiver<CancelCause>), DaemonError>;

/// Held by [`Registry::open_tracked`] while it waits for the connect task.
/// When the `open_tracked` future is dropped in that wait (the IPC peer left,
/// or a timeout elapsed), it removes the placeholder of its attempt, which
/// cancels the connect, and closes a session that was already delivered, so
/// no session is dropped bare. Its scope is the `Connecting` entry only: a
/// future dropped after promotion leaves its `Live` entry to the lease-based
/// cleanup.
struct ConnectGuard<'a> {
    registry: &'a Registry,
    lease: ConnectLease,
    rx: oneshot::Receiver<Delivered>,
    armed: bool,
}

impl Drop for ConnectGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.registry.cancel_connecting(&self.lease);
        // After `close`, a send fails and the connect task closes its own
        // session; a send that happened before is drained here.
        self.rx.close();
        if let Ok(Ok((session, _))) = self.rx.try_recv() {
            close_detached(session);
        }
    }
}

/// Close `session` without awaiting it, from a synchronous `Drop`. The close
/// runs on a short-lived thread with its own runtime, so it does not depend on
/// the state of the caller's runtime: it works inside a running runtime, with
/// no runtime, and while a runtime shuts down (a task spawned then is dropped
/// unpolled). If the thread or its runtime cannot start, the session drops,
/// which stops it on a best-effort basis.
///
/// The thread is detached. A process that exits right after drops the thread
/// together with the session thread and its socket.
fn close_detached(session: Box<dyn ManagedSession>) {
    let spawned = std::thread::Builder::new()
        .name("rdpilot-close".to_owned())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    if let Err(error) = runtime.block_on(session.close()) {
                        eprintln!("rdpilot-daemon: closing a cancelled connect failed: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("rdpilot-daemon: could not start a runtime to close a cancelled connect: {error}");
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("rdpilot-daemon: could not start a thread to close a cancelled connect: {error}");
    }
}

impl Registry {
    /// Construct an empty registry over the given (injectable — production
    /// wiring in Plan 12-06, fakes in tests) connector and reconciliation
    /// sink.
    /// Recording is not available in a registry built this way.
    #[must_use]
    pub fn new(connector: Arc<dyn SessionConnector>, sink: Arc<dyn ReconciliationSink>) -> Self {
        Self::with_recordings(connector, sink, RecordingService::disabled())
    }

    /// A registry whose sessions can be recorded through `recordings`.
    #[must_use]
    pub(crate) fn with_recordings(
        connector: Arc<dyn SessionConnector>,
        sink: Arc<dyn ReconciliationSink>,
        recordings: Arc<RecordingService>,
    ) -> Self {
        Registry {
            sessions: Mutex::new(HashMap::new()),
            connector,
            sink,
            bundles: Arc::new(NoBundleSource),
            next_generation: AtomicU64::new(1),
            recordings,
        }
    }

    /// The recording service.
    pub(crate) fn recordings(&self) -> &Arc<RecordingService> {
        &self.recordings
    }

    /// What a recording of live session `id` needs, under the brief outer
    /// lock only (no per-session lock, no activity change).
    ///
    /// # Errors
    ///
    /// [`DaemonError::SessionNotFound`] or [`DaemonError::StillConnecting`].
    pub(crate) fn recording_target(&self, id: &SessionId) -> Result<Target, DaemonError> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live {
                events,
                frame,
                name,
                host,
                ..
            }) => Ok(Target {
                events: Arc::clone(events),
                frame: frame.clone(),
                session: id.as_str().to_owned(),
                name: name.clone(),
                host: host.clone(),
            }),
            Some(SessionEntry::Connecting { .. }) => {
                Err(DaemonError::StillConnecting(id.as_str().to_owned()))
            }
            _ => Err(DaemonError::SessionNotFound(id.as_str().to_owned())),
        }
    }

    /// Use `bundles` for Cua-enabled connects (builder). Without it every
    /// Cua-enabled connect fails closed.
    #[must_use]
    pub(crate) fn with_bundle_source(mut self, bundles: Arc<dyn BundleSource>) -> Self {
        self.bundles = bundles;
        self
    }

    /// The source of Cua bundles.
    pub(crate) fn bundle_source(&self) -> &dyn BundleSource {
        self.bundles.as_ref()
    }

    /// Atomically claim `id` as a `Connecting` placeholder.
    ///
    /// The ENTIRE lock scope is synchronous (no `.await` inside) — this is
    /// what makes the claim atomic w.r.t. every other concurrent caller
    /// (research Pattern 1). Returns `Err(DuplicateSession)` if `id` is
    /// already claimed (`Connecting`), live, or orphaned. On success it
    /// returns the attempt identity drawn for this claim and the receiver
    /// side of the placeholder's cancel handle. The number is drawn only for
    /// a claim that succeeds.
    fn claim(&self, id: &SessionId) -> Result<(u64, oneshot::Receiver<CancelCause>), DaemonError> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let mut guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.entry(id.clone()) {
            Entry::Occupied(_) => Err(DaemonError::DuplicateSession(id.as_str().to_owned())),
            Entry::Vacant(slot) => {
                let attempt = self.next_generation.fetch_add(1, Ordering::Relaxed);
                let (cancel, cancel_rx) = oneshot::channel();
                slot.insert(SessionEntry::Connecting {
                    claimed_at: Instant::now(),
                    attempt,
                    cancel,
                });
                Ok((attempt, cancel_rx))
            }
        }
    }

    /// Remove the `Connecting` placeholder of `lease`, and nothing else: an
    /// entry that is gone, `Live`, `Orphaned`, or a `Connecting` of a later
    /// attempt under the same name stays. Dropping the placeholder drops its
    /// cancel handle, which cancels the connect task. Returns whether it
    /// removed the placeholder.
    ///
    /// Locks with poison recovery: the connect guard calls this from a
    /// `Drop`, where a panic during unwinding would abort the process.
    pub(crate) fn cancel_connecting(&self, lease: &ConnectLease) -> bool {
        let mut guard = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        match guard.get(&lease.id) {
            Some(SessionEntry::Connecting { attempt, .. }) if *attempt == lease.generation => {
                guard.remove(&lease.id);
                true
            }
            _ => false,
        }
    }

    /// Mint and atomically claim a fresh auto-generated id (D-29), retrying
    /// with a freshly minted pair on collision — reusing [`Registry::claim`],
    /// the SAME atomic insert path a caller-supplied name uses (research
    /// "Don't Hand-Roll": no second "generate and hope" code path).
    fn claim_with_auto_id(
        &self,
    ) -> Result<(SessionId, u64, oneshot::Receiver<CancelCause>), DaemonError> {
        for _ in 0..AUTO_ID_MAX_ATTEMPTS {
            let candidate = generate_auto_id();
            let Ok(id) = SessionId::from_str(&candidate) else {
                // `generate_auto_id` always produces a non-empty string;
                // this branch is unreachable in practice but handled
                // without unwrap/expect/panic (API-01 discipline mirrored
                // from `rdpilot`).
                continue;
            };
            if let Ok((attempt, cancel_rx)) = self.claim(&id) {
                return Ok((id, attempt, cancel_rx));
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
    /// claim is released so a retry with the same name can succeed. A connect
    /// whose placeholder was removed before its session was registered fails
    /// with [`DaemonError::Connect`], and its session is closed.
    ///
    /// Dropping the returned future while the connect is pending removes the
    /// placeholder, cancels the connect and closes a session that was already
    /// delivered.
    pub async fn open(
        &self,
        name: Option<String>,
        host: String,
        cfg: ConnectionConfig,
    ) -> Result<SessionId, DaemonError> {
        Ok(self.open_tracked(name, host, cfg, None).await?.0.id)
    }

    /// Like [`Registry::open`], but returns an internal exact-generation
    /// ownership handle for the IPC response path. With `record`, a
    /// recording starts right after the session is inserted; its outcome
    /// (the id, or why it could not start) never fails the connect.
    pub(crate) async fn open_tracked(
        &self,
        name: Option<String>,
        host: String,
        cfg: ConnectionConfig,
        record: Option<RecordingTrigger>,
    ) -> Result<(ConnectLease, Option<Result<String, String>>), DaemonError> {
        let (id, attempt, mut cancel_rx) = match &name {
            Some(n) => {
                let id = SessionId::from_str(n).map_err(DaemonError::Connect)?;
                let (attempt, cancel_rx) = self.claim(&id)?;
                (id, attempt, cancel_rx)
            }
            None => self.claim_with_auto_id()?,
        }; // claim() has already released the lock by this point — nothing
           // is held across the `.await` below.
        let lease = ConnectLease {
            id: id.clone(),
            generation: attempt,
        };

        // The connect runs on its own task so that removing the placeholder
        // can cancel it: the placeholder owns the cancel handle, and the task
        // stops when the handle is dropped or sent a cause. A cause becomes the
        // error of the connect; a dropped handle needs no answer. A session
        // whose delivery fails (the receiver is gone) is closed here, never
        // dropped. The biased order lets a cause that was sent win over a
        // connect that is ready at the same time.
        let connect = self.connector.connect(cfg);
        let (result_tx, result_rx) = oneshot::channel::<Delivered>();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                cause = &mut cancel_rx => {
                    if let Ok(cause) = cause {
                        let _ = result_tx.send(Err(cancelled_by(cause)));
                    }
                }
                result = connect => {
                    let result = result.map(|session| (session, cancel_rx));
                    if let Err(Ok((session, _))) = result_tx.send(result) {
                        let _ = session.close().await;
                    }
                }
            }
        });
        // Built before the first `.await`, so the placeholder is released and
        // a delivered session is closed wherever this future is dropped.
        let mut guard = ConnectGuard {
            registry: self,
            lease: lease.clone(),
            rx: result_rx,
            armed: true,
        };

        let delivered = (&mut guard.rx).await;
        // The wait is over: every exit below settles the placeholder itself.
        guard.armed = false;
        let (session, mut cancel_rx) = match delivered {
            Ok(Ok(delivered)) => delivered,
            Ok(Err(error)) => {
                self.cancel_connecting(&lease);
                return Err(error);
            }
            // The task ended without a result: the placeholder was removed
            // (the cancel), or the connect task panicked.
            Err(_) => {
                self.cancel_connecting(&lease);
                return Err(connect_cancelled());
            }
        };

        let frame = session.frame_source();
        let generation = attempt;
        let events = Arc::new(SessionEvents::new(&id, generation, frame.clone()));
        let control = Arc::new(SessionControl::new(
            id.as_str(),
            generation,
            Arc::clone(&events),
            frame.clone(),
            session.human_input(),
        ));
        let ended_watch = frame.clone().map(|frame| {
            crate::events::watch_session_end(frame, Arc::clone(&events), Arc::clone(&control))
        });
        // A single wall-clock capture shared by the entry's
        // `connected_since_wall`/`last_activity_wall` and the
        // reconciliation sink's `record_open` call below — avoids
        // two independent `SystemTime::now()` reads racing apart
        // by a few milliseconds for what is conceptually one event.
        let now_wall = iso8601_now();
        let not_promoted = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            // Promote only the placeholder of this attempt. The check and the
            // insert share one lock scope with no `.await` in between.
            if matches!(
                guard.get(&id),
                Some(SessionEntry::Connecting { attempt: current, .. }) if *current == attempt
            ) {
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
                        ended_watch,
                        control,
                        last_activity_wall: now_wall.clone(),
                    },
                );
                None
            } else {
                Some((session, ended_watch, events, control))
            }
        }; // guard dropped here — never held across the .await below
        if let Some((session, ended_watch, events, control)) = not_promoted {
            // The placeholder was removed while the session was in flight.
            // The session was never registered, so its watcher must not
            // report an end; then close it, awaited. If this future is
            // dropped during that close, the session drops mid-close; its
            // synchronous part has already run (as in `ConnectGuard::drop`)
            // and no entry is left to clean up.
            drop(ended_watch);
            drop((events, control));
            let _ = session.close().await;
            // A Disconnect sends its cause before the lock that removed the
            // placeholder is released, so it is already here.
            return Err(match cancel_rx.try_recv() {
                Ok(cause) => cancelled_by(cause),
                Err(_) => connect_cancelled(),
            });
        }
        self.sink.record_open(&id, &host, &now_wall);
        let recording = match record {
            Some(trigger) => Some(match self.recording_target(&id) {
                Ok(target) => self
                    .recordings
                    .start(target, trigger, EventSource::Cli)
                    .await
                    .map(|started| started.id),
                Err(e) => Err(e.to_string()),
            }),
            None => None,
        };
        Ok((lease, recording))
    }

    /// Close `id`: extract the entry, then await its `close()` — never let
    /// a removed `Live` entry drop bare (research Pattern 2, DAEMON-01's
    /// critical invariant).
    ///
    /// A connect still in flight is cancelled instead: its placeholder is
    /// removed, the connect task is told why, and the pending connect fails
    /// with "connect cancelled by disconnect". No record is written, because
    /// none was. The call returns before the connect future is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::SessionNotFound`] if `id` has no entry, or the
    /// underlying close/join error.
    pub async fn close(&self, id: &SessionId) -> Result<(), DaemonError> {
        self.close_with(id, CloseReason::Disconnect).await
    }

    /// Like [`Registry::close`], for the idle reaper: a recording of the
    /// session ends with the reason `idle_reap`. The reaper closes by name
    /// after a snapshot, so a connect in flight is never cancelled here: it
    /// may be a later attempt that reused the name of a reaped session.
    ///
    /// # Errors
    ///
    /// As for [`Registry::close`], plus [`DaemonError::StillConnecting`] if
    /// `id`'s connect is still in flight (the placeholder stays untouched).
    pub async fn close_idle(&self, id: &SessionId) -> Result<(), DaemonError> {
        self.close_with(id, CloseReason::IdleReap).await
    }

    async fn close_with(&self, id: &SessionId, reason: CloseReason) -> Result<(), DaemonError> {
        let entry = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            // Inspect and remove under this one lock.
            if matches!(guard.get(id), Some(SessionEntry::Connecting { .. })) {
                if reason != CloseReason::Disconnect {
                    return Err(DaemonError::StillConnecting(id.as_str().to_owned()));
                }
                // Cancel the connect: remove its placeholder and say why
                // while the lock is held, so a promotion that finds the
                // placeholder gone also finds the cause. The name is free
                // when the lock drops.
                if let Some(SessionEntry::Connecting { cancel, .. }) = guard.remove(id) {
                    let _ = cancel.send(CancelCause::Disconnect);
                }
                return Ok(());
            }
            guard.remove(id) // extract, don't let it drop in this scope
        };

        match entry {
            Some(SessionEntry::Live {
                session,
                ended_watch,
                events,
                control,
                ..
            }) => {
                // Our own close is not a server-side end: stop the watcher
                // before the session loop is told to stop.
                drop(ended_watch);
                end_lease(&control, EndReason::SessionEnded).await;
                self.recordings.session_closed(&events, reason);
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
            Some(SessionEntry::Connecting { .. }) => {
                // Unreachable: a placeholder is handled under the lock above.
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

        let Some(SessionEntry::Live {
            session,
            ended_watch,
            events,
            control,
            ..
        }) = entry
        else {
            return Ok(false);
        };
        drop(ended_watch);
        end_lease(&control, EndReason::SessionEnded).await;
        self.recordings
            .session_closed(&events, CloseReason::ConnectAborted);
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

    /// Like [`Registry::call`], for agent input (native Mouse and Key): after
    /// the per-session lock is taken, the call is refused while a human
    /// viewer holds (or is taking) the control lease, and waits for the
    /// releases of an ended lease to be sent. Checking under the
    /// per-session lock is what orders it with a human take, which marks
    /// the lease first and then waits for this lock.
    ///
    /// # Errors
    ///
    /// As for [`Registry::call`], plus [`DaemonError::HumanControl`].
    pub(crate) async fn call_acting<T>(
        &self,
        id: &SessionId,
        op: impl for<'a> FnOnce(&'a dyn ManagedSession) -> BoxFuture<'a, Result<T, DaemonError>>,
    ) -> Result<T, DaemonError> {
        let (entry, control) = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(id) {
                Some(SessionEntry::Live {
                    session, control, ..
                }) => (Arc::clone(session), Arc::clone(control)),
                Some(SessionEntry::Connecting { .. }) => {
                    return Err(DaemonError::StillConnecting(id.as_str().to_owned()));
                }
                Some(SessionEntry::Orphaned { .. }) | None => {
                    return Err(DaemonError::SessionNotFound(id.as_str().to_owned()));
                }
            }
        };
        let guard = entry.lock().await;
        control.admit_agent().await?;
        match guard.as_deref() {
            Some(session) => op(session).await,
            None => Err(DaemonError::SessionNotFound(id.as_str().to_owned())),
        }
    }

    /// The control lease of live session `id`.
    pub(crate) fn control(&self, id: &SessionId) -> Option<Arc<SessionControl>> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live { control, .. }) => Some(Arc::clone(control)),
            _ => None,
        }
    }

    /// Every live session's control lease.
    pub(crate) fn controls(&self) -> Vec<Arc<SessionControl>> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard
            .values()
            .filter_map(|entry| match entry {
                SessionEntry::Live { control, .. } => Some(Arc::clone(control)),
                _ => None,
            })
            .collect()
    }

    /// End every human lease for `reason`, with its releases (viewer stop,
    /// daemon exit).
    pub(crate) async fn end_all_leases(&self, reason: EndReason) {
        for control in self.controls() {
            end_lease(&control, reason).await;
        }
    }

    /// The agent takes control of `id` back from any human viewer
    /// (`rdpilot takeover`). Returns the previous controller and whether
    /// anything changed.
    ///
    /// # Errors
    ///
    /// [`DaemonError::SessionNotFound`] or [`DaemonError::StillConnecting`].
    pub(crate) async fn takeover(
        &self,
        id: &SessionId,
        source: EventSource,
    ) -> Result<(rdpilot_ipc::WireController, bool), DaemonError> {
        let control = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(id) {
                Some(SessionEntry::Live { control, .. }) => Arc::clone(control),
                Some(SessionEntry::Connecting { .. }) => {
                    return Err(DaemonError::StillConnecting(id.as_str().to_owned()));
                }
                _ => return Err(DaemonError::SessionNotFound(id.as_str().to_owned())),
            }
        };
        let (previous, transition) = control.agent_takeover(source);
        let changed = transition.is_some();
        if let Some(transition) = transition {
            control.discharge(transition).await;
        }
        Ok((previous, changed))
    }

    /// The per-session lock of `id` plus its control, for a human take: the
    /// take waits on the lock to let an in-flight native operation finish.
    pub(crate) fn take_parts(&self, id: &SessionId) -> Option<(SessionSlot, Arc<SessionControl>)> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live {
                session, control, ..
            }) => Some((Arc::clone(session), Arc::clone(control))),
            _ => None,
        }
    }

    /// Capture the incarnation and acquire a stream under one short session lease.
    /// Neither the global registry lock nor the per-session lock escapes this call.
    /// Also returns the session's event log, so the stream records without
    /// touching the registry per message.
    pub(crate) async fn attach_cua(&self, id: &SessionId) -> Result<CuaAttach, DaemonError> {
        let (generation, entry, cua_leases, events, control) = {
            #[allow(clippy::expect_used)]
            let guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(id) {
                Some(SessionEntry::Live {
                    generation,
                    session,
                    cua_leases,
                    events,
                    control,
                    ..
                }) => (
                    *generation,
                    Arc::clone(session),
                    Arc::clone(cua_leases),
                    Arc::clone(events),
                    Arc::clone(control),
                ),
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
        Ok(CuaAttach {
            generation,
            events,
            control,
            attachment: Box::new(CuaLease {
                inner: attachment,
                active: cua_leases,
            }),
        })
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
            .map(|(id, entry)| self.status_of(id, entry))
            .collect()
    }

    /// The list entry of `entry`, with its active recording.
    fn status_of(&self, id: &SessionId, entry: &SessionEntry) -> SessionStatus {
        let mut status = entry.to_status(id);
        if let SessionEntry::Live { events, .. } = entry {
            status.recording = self.recordings.recording_of(&events.header().log_id);
        }
        status
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
                    control,
                    ..
                } => (cua_leases.load(Ordering::Relaxed) == 0 && !control.human_active()).then(
                    || {
                        // Human input and lease ends count as activity.
                        let latest = control
                            .human_activity()
                            .map_or(*last_activity, |(at, _)| at.max(*last_activity));
                        (id.clone(), latest.elapsed())
                    },
                ),
                SessionEntry::Connecting { .. } | SessionEntry::Orphaned { .. } => None,
            })
            .collect()
    }
}

/// The per-session lock around a live session.
pub(crate) type SessionSlot = Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>;

/// End `control`'s human lease (if any) for `reason` and send its releases.
pub(crate) async fn end_lease(control: &SessionControl, reason: EndReason) {
    if let Some(transition) = control.end_human(reason) {
        control.discharge(transition).await;
    }
}

/// What [`Registry::attach_cua`] hands the IPC stream.
pub(crate) struct CuaAttach {
    pub(crate) generation: u64,
    pub(crate) events: Arc<SessionEvents>,
    pub(crate) control: Arc<SessionControl>,
    pub(crate) attachment: Box<dyn crate::seams::ManagedCua>,
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

/// Why a viewer recording action was refused.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The session is not in the registry (or still connecting).
    NoSession(String),
    /// The request is not possible now (not recording, bad text, busy).
    Rejected(String),
    /// Recording could not start (storage, configuration).
    Unavailable(String),
}

impl From<DaemonError> for Refusal {
    fn from(e: DaemonError) -> Self {
        Refusal::NoSession(e.to_string())
    }
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
    /// `true` when a viewer tab can take control and send input.
    pub(crate) input: bool,
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

    /// Start recording live session `id` from the viewer. Returns the
    /// recording id and whether it changed anything.
    pub(crate) async fn record_start(&self, id: &SessionId) -> Result<(String, bool), Refusal> {
        let target = self.inner.recording_target(id).map_err(Refusal::from)?;
        self.inner
            .recordings
            .start(target, RecordingTrigger::Viewer, EventSource::Viewer)
            .await
            .map(|s| (s.id, s.changed))
            .map_err(Refusal::Unavailable)
    }

    /// Stop recording live session `id` from the viewer; `None` when it was
    /// not recording.
    pub(crate) fn record_stop(&self, id: &SessionId) -> Result<Option<String>, Refusal> {
        let target = self.inner.recording_target(id).map_err(Refusal::from)?;
        Ok(self.inner.recordings.stop(
            &target.events,
            EventSource::Viewer,
            crate::events::StopReason::Requested,
        ))
    }

    /// Annotate the active recording of live session `id` from the viewer.
    pub(crate) fn annotate(&self, id: &SessionId, text: &str) -> Result<String, Refusal> {
        let target = self.inner.recording_target(id).map_err(Refusal::from)?;
        self.inner
            .recordings
            .annotate(&target.events, EventSource::Viewer, text)
            .map_err(Refusal::Rejected)
    }

    /// The recording service (listing, keep and file reads, which the
    /// viewer runs on blocking threads).
    pub(crate) fn recordings(&self) -> Arc<RecordingService> {
        Arc::clone(&self.inner.recordings)
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

/// Why a viewer control request was refused.
#[derive(Debug)]
pub(crate) enum ControlRefusal {
    /// The session is not live, or it has no input path.
    NoSession,
    /// The request names a lease that is not current.
    NotHeld(NotHeld),
    /// A take could not complete.
    Take(TakeError),
    /// The session's input channel did not take input in time.
    Unavailable,
}

/// The only control surface the live viewer receives: human lease
/// operations and human input for one session at a time. It cannot reach
/// Cua, file transfer, connect, disconnect, native agent input, or another
/// session, and it cannot perform an agent takeover.
#[derive(Clone)]
pub(crate) struct ViewerControl {
    inner: Arc<Registry>,
}

impl ViewerControl {
    pub(crate) fn new(registry: Arc<Registry>) -> Self {
        ViewerControl { inner: registry }
    }

    fn control(&self, id: &SessionId) -> Result<Arc<SessionControl>, ControlRefusal> {
        self.inner
            .control(id)
            .filter(|control| control.has_input())
            .ok_or(ControlRefusal::NoSession)
    }

    /// Take the lease of `id` for a tab at `address`. The take waits for
    /// the per-session lock, so an in-flight native agent operation ends
    /// first.
    pub(crate) async fn take(
        &self,
        id: &SessionId,
        address: std::net::IpAddr,
    ) -> Result<(Grant, Option<(u32, u32)>), ControlRefusal> {
        let (slot, control) = self
            .inner
            .take_parts(id)
            .filter(|(_, control)| control.has_input())
            .ok_or(ControlRefusal::NoSession)?;
        let agent_idle = async move {
            drop(slot.lock().await);
        };
        let grant = control
            .take(address, agent_idle)
            .await
            .map_err(ControlRefusal::Take)?;
        Ok((grant, control.geometry()))
    }

    /// The holder's heartbeat.
    pub(crate) fn heartbeat(&self, id: &SessionId, lease: &str) -> Result<(), ControlRefusal> {
        self.control(id)?
            .heartbeat(lease)
            .map_err(ControlRefusal::NotHeld)
    }

    /// The holder's page lost focus: release what it holds.
    pub(crate) async fn blur(&self, id: &SessionId, lease: &str) -> Result<(), ControlRefusal> {
        let control = self.control(id)?;
        let transition = control.blur(lease).map_err(ControlRefusal::NotHeld)?;
        control.discharge(transition).await;
        Ok(())
    }

    /// The holder releases control to the agent.
    pub(crate) async fn release(&self, id: &SessionId, lease: &str) -> Result<(), ControlRefusal> {
        let control = self.control(id)?;
        let transition = control.release(lease).map_err(ControlRefusal::NotHeld)?;
        control.discharge(transition).await;
        Ok(())
    }

    /// Apply the holder's input.
    pub(crate) async fn input(
        &self,
        id: &SessionId,
        lease: &str,
        generation: u64,
        geometry: (u32, u32),
        events: Vec<RawInput>,
    ) -> Result<InputReport, ControlRefusal> {
        self.control(id)?
            .input(lease, generation, geometry, events)
            .await
            .map_err(|InputError::Unavailable| ControlRefusal::Unavailable)
    }

    /// End leases whose heartbeat or input stopped.
    pub(crate) async fn sweep(&self, idle_timeout: std::time::Duration) {
        let now = Instant::now();
        for control in self.inner.controls() {
            if let Some(transition) = control.expire(now, idle_timeout) {
                control.discharge(transition).await;
            }
        }
    }

    /// End every human lease (the viewer stopped).
    pub(crate) async fn end_all(&self) {
        self.inner.end_all_leases(EndReason::ViewerStopped).await;
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
                let input =
                    matches!(entry, SessionEntry::Live { control, .. } if control.has_input());
                ViewerSession {
                    status: self.status_of(id, entry),
                    ended,
                    frames,
                    input,
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
    use std::time::Duration;

    use rdpilot_ipc::SessionLifecycle;

    use super::*;
    use crate::seams::{ManagedSession, NoopReconciliationSink, SessionConnector};

    type TestFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

    /// A fake, immediately-resolving `ManagedSession` (no real OS thread —
    /// the thread-owning fake used by the DAEMON-01 soak proof lives in
    /// `tests/thread_leak_soak.rs`, Task 3).
    struct FakeSession {
        closes: Arc<AtomicU32>,
    }

    impl ManagedSession for FakeSession {
        /// Counts when the returned future is polled, not when `close` is
        /// called: a future that is dropped unpolled closes nothing.
        fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
            let closes = Arc::clone(&self.closes);
            Box::pin(async move {
                closes.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
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
                        closes: Arc::new(AtomicU32::new(0)),
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
    async fn close_on_a_still_connecting_id_cancels_it_and_removes_the_placeholder() {
        let registry = Registry::new(
            Arc::new(FakeConnector::succeeding()),
            Arc::new(NoopReconciliationSink),
        );
        // Manually seed a Connecting placeholder (as `open` would, mid-flight)
        // without actually running a connect, to exercise `close`'s
        // still-connecting branch in isolation.
        let id = SessionId::from_str("web").expect("non-empty literal");
        let (_, mut cancel_rx) = registry
            .claim(&id)
            .expect("claim should succeed on an empty registry");

        registry
            .close(&id)
            .await
            .expect("a Disconnect cancels the connect in flight");
        assert_eq!(registry.len(), 0, "the placeholder is removed");
        assert_eq!(cancel_rx.try_recv(), Ok(CancelCause::Disconnect));
    }

    #[tokio::test]
    async fn close_idle_on_a_still_connecting_id_is_rejected_and_the_placeholder_survives() {
        let registry = Registry::new(
            Arc::new(FakeConnector::succeeding()),
            Arc::new(NoopReconciliationSink),
        );
        let id = SessionId::from_str("web").expect("non-empty literal");
        let (attempt, mut cancel_rx) = registry
            .claim(&id)
            .expect("claim should succeed on an empty registry");

        // The idle reaper closes by name after a snapshot, so it must never
        // cancel a later attempt that reused the name.
        let result = registry.close_idle(&id).await;
        assert!(matches!(result, Err(DaemonError::StillConnecting(name)) if name == "web"));
        assert_eq!(registry.len(), 1);
        assert_eq!(connecting_attempt(&registry, "web"), Some(attempt));
        assert!(cancel_rx.try_recv().is_err(), "no cause was sent");
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

    // --- Cancelling a pending connect (S1) -------------------------------

    /// What the test sees of one gated connect call.
    #[derive(Clone)]
    struct GatedCall {
        /// Releases the connect: add a permit.
        gate: Arc<tokio::sync::Semaphore>,
        /// The connect future was polled at least once.
        started: Arc<AtomicBool>,
        /// The connect future finished with a session.
        delivered: Arc<AtomicBool>,
        /// The connect future was dropped (finished or not).
        dropped: Arc<AtomicBool>,
    }

    struct SetOnDrop(Arc<AtomicBool>);
    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A connector whose n-th `connect` waits on the n-th call's gate. Every
    /// session it delivers adds to one shared close count.
    struct GatedConnector {
        calls: Mutex<std::collections::VecDeque<GatedCall>>,
        closes: Arc<AtomicU32>,
    }

    fn gated(calls: usize) -> (Arc<GatedConnector>, Vec<GatedCall>, Arc<AtomicU32>) {
        let handles: Vec<GatedCall> = (0..calls)
            .map(|_| GatedCall {
                gate: Arc::new(tokio::sync::Semaphore::new(0)),
                started: Arc::new(AtomicBool::new(false)),
                delivered: Arc::new(AtomicBool::new(false)),
                dropped: Arc::new(AtomicBool::new(false)),
            })
            .collect();
        let closes = Arc::new(AtomicU32::new(0));
        let connector = Arc::new(GatedConnector {
            calls: Mutex::new(handles.iter().cloned().collect()),
            closes: Arc::clone(&closes),
        });
        (connector, handles, closes)
    }

    impl SessionConnector for GatedConnector {
        fn connect(
            &self,
            _cfg: ConnectionConfig,
        ) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            #[allow(clippy::expect_used)]
            let call = self
                .calls
                .lock()
                .expect("test mutex")
                .pop_front()
                .expect("more connects than gated calls");
            let closes = Arc::clone(&self.closes);
            // Owned by the future from the start, so an unpolled future that
            // is dropped sets it too.
            let dropped = SetOnDrop(call.dropped);
            Box::pin(async move {
                let _dropped = dropped;
                call.started.store(true, Ordering::SeqCst);
                #[allow(clippy::expect_used)]
                call.gate
                    .acquire()
                    .await
                    .expect("gate is never closed")
                    .forget();
                call.delivered.store(true, Ordering::SeqCst);
                Ok(Box::new(FakeSession { closes }) as Box<dyn ManagedSession>)
            })
        }
    }

    fn gated_registry(calls: usize) -> (Arc<Registry>, Vec<GatedCall>, Arc<AtomicU32>) {
        let (connector, handles, closes) = gated(calls);
        let registry = Arc::new(Registry::new(connector, Arc::new(NoopReconciliationSink)));
        (registry, handles, closes)
    }

    type Opened = Result<(ConnectLease, Option<Result<String, String>>), DaemonError>;

    /// Poll `open_tracked` for `name` exactly once and hand back the pinned,
    /// unfinished future.
    async fn start_open<'a>(
        registry: &'a Registry,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Opened> + Send + 'a>> {
        let mut open: Pin<Box<dyn Future<Output = Opened> + Send + 'a>> = Box::pin(
            registry.open_tracked(Some(name.to_owned()), "h".to_owned(), test_cfg(), None),
        );
        tokio::select! {
            biased;
            _ = &mut open => panic!("the gated connect cannot have finished"),
            () = std::future::ready(()) => {}
        }
        open
    }

    async fn until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(std::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    fn flag(flag: &Arc<AtomicBool>) -> bool {
        flag.load(Ordering::SeqCst)
    }

    fn connecting_attempt(registry: &Registry, name: &str) -> Option<u64> {
        let id = SessionId::from_str(name).expect("non-empty literal");
        #[allow(clippy::expect_used)]
        let guard = registry.sessions.lock().expect("registry mutex poisoned");
        match guard.get(&id) {
            Some(SessionEntry::Connecting { attempt, .. }) => Some(*attempt),
            _ => None,
        }
    }

    fn live_generation(registry: &Registry, name: &str) -> Option<u64> {
        let id = SessionId::from_str(name).expect("non-empty literal");
        #[allow(clippy::expect_used)]
        let guard = registry.sessions.lock().expect("registry mutex poisoned");
        match guard.get(&id) {
            Some(SessionEntry::Live { generation, .. }) => Some(*generation),
            _ => None,
        }
    }

    fn lease_of(name: &str, attempt: u64) -> ConnectLease {
        ConnectLease {
            id: SessionId::from_str(name).expect("non-empty literal"),
            generation: attempt,
        }
    }

    #[tokio::test]
    async fn dropping_open_tracked_mid_connect_removes_the_placeholder_and_the_connect() {
        let (registry, calls, closes) = gated_registry(1);
        let open = start_open(&registry, "s").await;
        until("the connect started", || flag(&calls[0].started)).await;
        assert_eq!(registry.len(), 1);

        drop(open);

        assert_eq!(registry.len(), 0, "the placeholder is gone at once");
        until("the connect future was dropped", || flag(&calls[0].dropped)).await;
        assert!(!flag(&calls[0].delivered));
        assert_eq!(closes.load(Ordering::SeqCst), 0);
        registry
            .claim(&SessionId::from_str("s").expect("non-empty literal"))
            .expect("the name can be claimed again");
    }

    #[tokio::test]
    async fn a_session_delivered_after_the_placeholder_was_cancelled_is_closed_once() {
        let (registry, calls, closes) = gated_registry(1);
        let open = start_open(&registry, "s").await;
        until("the connect started", || flag(&calls[0].started)).await;
        let attempt = connecting_attempt(&registry, "s").expect("a placeholder");

        // The session is delivered before the placeholder is cancelled, and
        // only then does `open_tracked` see it.
        calls[0].gate.add_permits(1);
        until("the session was delivered", || flag(&calls[0].delivered)).await;
        assert!(registry.cancel_connecting(&lease_of("s", attempt)));

        let error = open.await.expect_err("a cancelled connect fails");
        assert!(
            matches!(&error, DaemonError::Connect(m) if m == "connect cancelled before the session was registered"),
            "{error:?}"
        );
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert_eq!(registry.len(), 0, "nothing was inserted");
    }

    #[tokio::test]
    async fn a_stale_attempt_never_touches_the_placeholder_or_live_entry_of_a_later_attempt() {
        let (registry, calls, closes) = gated_registry(2);

        // Attempt 1 delivers its session, then loses its placeholder.
        let first = start_open(&registry, "s").await;
        until("attempt 1 started", || flag(&calls[0].started)).await;
        let first_attempt = connecting_attempt(&registry, "s").expect("attempt 1");
        calls[0].gate.add_permits(1);
        until("attempt 1 delivered", || flag(&calls[0].delivered)).await;
        assert!(registry.cancel_connecting(&lease_of("s", first_attempt)));

        // Attempt 2 claims the same name and is held.
        let second = start_open(&registry, "s").await;
        until("attempt 2 started", || flag(&calls[1].started)).await;
        let second_attempt = connecting_attempt(&registry, "s").expect("attempt 2");
        assert_ne!(first_attempt, second_attempt);

        // Attempt 1 finishes: it closes its session and leaves attempt 2 alone.
        assert!(first.await.is_err());
        assert_eq!(closes.load(Ordering::SeqCst), 1);
        assert_eq!(connecting_attempt(&registry, "s"), Some(second_attempt));
        assert!(!registry.cancel_connecting(&lease_of("s", first_attempt)));
        assert_eq!(connecting_attempt(&registry, "s"), Some(second_attempt));

        // Attempt 2 completes and is promoted under its own attempt.
        calls[1].gate.add_permits(1);
        let (lease, _) = second.await.expect("attempt 2 succeeds");
        assert_eq!(lease.generation, second_attempt);
        assert_eq!(live_generation(&registry, "s"), Some(second_attempt));
        assert_eq!(closes.load(Ordering::SeqCst), 1, "attempt 2 stays open");
        assert!(!registry.cancel_connecting(&lease_of("s", first_attempt)));
        assert_eq!(live_generation(&registry, "s"), Some(second_attempt));
    }

    #[tokio::test]
    async fn a_dropped_stale_future_leaves_the_live_entry_of_a_later_attempt() {
        let (registry, calls, closes) = gated_registry(2);
        let first = start_open(&registry, "s").await;
        until("attempt 1 started", || flag(&calls[0].started)).await;
        let first_attempt = connecting_attempt(&registry, "s").expect("attempt 1");
        calls[0].gate.add_permits(1);
        until("attempt 1 delivered", || flag(&calls[0].delivered)).await;
        assert!(registry.cancel_connecting(&lease_of("s", first_attempt)));

        let second = start_open(&registry, "s").await;
        until("attempt 2 started", || flag(&calls[1].started)).await;
        calls[1].gate.add_permits(1);
        let (lease, _) = second.await.expect("attempt 2 succeeds");

        // Attempt 1's future is dropped without being polled again: its guard
        // removes nothing of attempt 2, and closes the session it was handed.
        drop(first);
        until("attempt 1's session was closed", || {
            closes.load(Ordering::SeqCst) == 1
        })
        .await;
        assert_eq!(live_generation(&registry, "s"), Some(lease.generation));
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn the_lease_generation_is_the_attempt_drawn_at_the_claim() {
        let (registry, calls, _closes) = gated_registry(1);
        let open = start_open(&registry, "s").await;
        until("the connect started", || flag(&calls[0].started)).await;
        let attempt = connecting_attempt(&registry, "s").expect("a placeholder");

        calls[0].gate.add_permits(1);
        let (lease, _) = open.await.expect("the connect succeeds");

        assert_eq!(lease.generation, attempt);
        assert_eq!(live_generation(&registry, "s"), Some(attempt));
    }

    #[tokio::test]
    async fn a_session_delivered_to_a_dropped_open_tracked_is_closed_once() {
        let (registry, calls, closes) = gated_registry(1);
        let open = start_open(&registry, "s").await;
        until("the connect started", || flag(&calls[0].started)).await;

        calls[0].gate.add_permits(1);
        until("the session was delivered", || flag(&calls[0].delivered)).await;
        drop(open);

        until("the delivered session was closed", || {
            closes.load(Ordering::SeqCst) >= 1
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(closes.load(Ordering::SeqCst), 1, "closed exactly once");
        assert_eq!(registry.len(), 0);
        registry
            .claim(&SessionId::from_str("s").expect("non-empty literal"))
            .expect("the name can be claimed again");
    }

    #[tokio::test]
    async fn disconnect_of_a_connecting_id_cancels_the_connect_and_frees_the_name() {
        let (registry, calls, closes) = gated_registry(2);
        let open = start_open(&registry, "s").await;
        until("the connect started", || flag(&calls[0].started)).await;
        let first_attempt = connecting_attempt(&registry, "s").expect("a placeholder");

        let id = SessionId::from_str("s").expect("non-empty literal");
        registry
            .close(&id)
            .await
            .expect("a Disconnect cancels the connect in flight");
        assert_eq!(connecting_attempt(&registry, "s"), None);
        assert_eq!(registry.len(), 0, "the claim is gone at once");
        // The Ack precedes the drop of the connect future.
        until("the connect future was dropped", || flag(&calls[0].dropped)).await;
        assert!(!flag(&calls[0].delivered));

        let error = open.await.expect_err("the cancelled connect fails");
        assert!(
            matches!(&error, DaemonError::Connect(m) if m == "connect cancelled by disconnect"),
            "{error:?}"
        );
        assert_eq!(
            rdpilot_ipc::WireError::from(error).code,
            rdpilot_ipc::WireErrorCode::Internal
        );
        assert_eq!(closes.load(Ordering::SeqCst), 0);
        assert_eq!(registry.len(), 0);

        // The name is free: a second connect with it claims and completes.
        let second = start_open(&registry, "s").await;
        until("the second connect started", || flag(&calls[1].started)).await;
        let second_attempt = connecting_attempt(&registry, "s").expect("a new placeholder");
        assert!(second_attempt > first_attempt);
        calls[1].gate.add_permits(1);
        let (lease, _) = second.await.expect("the second connect succeeds");
        assert_eq!(lease.generation, second_attempt);
        assert_eq!(live_generation(&registry, "s"), Some(second_attempt));
    }

    #[tokio::test]
    async fn disconnect_while_connecting_leaves_an_independent_live_session() {
        let (registry, calls, closes) = gated_registry(2);
        let keep = start_open(&registry, "keep").await;
        until("the first connect started", || flag(&calls[0].started)).await;
        calls[0].gate.add_permits(1);
        let (keep_lease, _) = keep.await.expect("the first connect succeeds");

        let victim = start_open(&registry, "victim").await;
        until("the second connect started", || flag(&calls[1].started)).await;
        let victim_id = SessionId::from_str("victim").expect("non-empty literal");
        registry
            .close(&victim_id)
            .await
            .expect("a Disconnect cancels the connect in flight");
        assert!(victim.await.is_err());

        assert_eq!(
            live_generation(&registry, "keep"),
            Some(keep_lease.generation)
        );
        assert_eq!(closes.load(Ordering::SeqCst), 0);
        registry
            .call(&keep_lease.id, |session| session.ping())
            .await
            .expect("the independent session still answers");
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn disconnect_in_the_promotion_window_names_the_disconnect() {
        let (registry, calls, closes) = gated_registry(1);
        let open = start_open(&registry, "s").await;
        until("the connect started", || flag(&calls[0].started)).await;

        // The session is delivered, but `open_tracked` has not promoted it
        // yet: the placeholder is still there when the Disconnect arrives.
        calls[0].gate.add_permits(1);
        until("the session was delivered", || flag(&calls[0].delivered)).await;
        let id = SessionId::from_str("s").expect("non-empty literal");
        registry
            .close(&id)
            .await
            .expect("a Disconnect cancels the connect in flight");

        let error = open.await.expect_err("the cancelled connect fails");
        assert!(
            matches!(&error, DaemonError::Connect(m) if m == "connect cancelled by disconnect"),
            "{error:?}"
        );
        until("the delivered session was closed", || {
            closes.load(Ordering::SeqCst) == 1
        })
        .await;
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn connect_deadline_ends_a_hung_connect_and_frees_the_name() {
        let (connector, calls, closes) = gated(2);
        let registry = Registry::new(
            Arc::new(crate::seams::DeadlineConnector {
                inner: connector,
                deadline: crate::seams::DEFAULT_CONNECT_DEADLINE,
            }),
            Arc::new(NoopReconciliationSink),
        );
        let began = tokio::time::Instant::now();

        let error = registry
            .open_tracked(Some("s".to_owned()), "h".to_owned(), test_cfg(), None)
            .await
            .expect_err("a connect that is never released times out");

        let waited = began.elapsed();
        assert!(
            waited >= Duration::from_secs(60) && waited < Duration::from_secs(61),
            "{waited:?}"
        );
        assert!(
            matches!(&error, DaemonError::Connect(m) if m == "connect timed out after 60s"),
            "{error:?}"
        );
        assert_eq!(
            rdpilot_ipc::WireError::from(error).code,
            rdpilot_ipc::WireErrorCode::Internal
        );
        assert!(flag(&calls[0].dropped), "the connect future was dropped");
        assert_eq!(registry.len(), 0);
        assert_eq!(closes.load(Ordering::SeqCst), 0);

        // The name is free again.
        calls[1].gate.add_permits(1);
        let (lease, _) = registry
            .open_tracked(Some("s".to_owned()), "h".to_owned(), test_cfg(), None)
            .await
            .expect("the name connects again");
        assert_eq!(live_generation(&registry, "s"), Some(lease.generation));
    }

    /// A guard that holds a delivered session, as if its `open_tracked` future
    /// were dropped just now.
    fn guard_with_delivered_session<'a>(
        registry: &'a Registry,
        closes: &Arc<AtomicU32>,
    ) -> ConnectGuard<'a> {
        let id = SessionId::from_str("s").expect("non-empty literal");
        let (attempt, cancel_rx) = registry.claim(&id).expect("claim");
        let (tx, rx) = oneshot::channel();
        assert!(tx
            .send(Ok((
                Box::new(FakeSession {
                    closes: Arc::clone(closes),
                }) as Box<dyn ManagedSession>,
                cancel_rx,
            )))
            .is_ok());
        ConnectGuard {
            registry,
            lease: ConnectLease {
                id,
                generation: attempt,
            },
            rx,
            armed: true,
        }
    }

    fn wait_for_closes(closes: &Arc<AtomicU32>, wanted: u32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while closes.load(Ordering::SeqCst) != wanted {
            assert!(
                std::time::Instant::now() < deadline,
                "close count {} != {wanted}",
                closes.load(Ordering::SeqCst)
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_guard_closes_a_delivered_session_when_dropped_outside_any_runtime() {
        let registry = Registry::new(
            Arc::new(FakeConnector::succeeding()),
            Arc::new(NoopReconciliationSink),
        );
        let closes = Arc::new(AtomicU32::new(0));
        let guard = guard_with_delivered_session(&registry, &closes);
        assert!(tokio::runtime::Handle::try_current().is_err());

        drop(guard);

        wait_for_closes(&closes, 1);
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn the_guard_closes_a_delivered_session_while_its_runtime_shuts_down() {
        let registry = Arc::new(Registry::new(
            Arc::new(FakeConnector::succeeding()),
            Arc::new(NoopReconciliationSink),
        ));
        let closes = Arc::new(AtomicU32::new(0));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        // The task owns the guard and never finishes; the runtime drops it.
        // A task spawned from the guard's `Drop` at that point is dropped
        // unpolled, so the close must not depend on the runtime.
        let task_registry = Arc::clone(&registry);
        let task_closes = Arc::clone(&closes);
        runtime.spawn(async move {
            let _guard = guard_with_delivered_session(&task_registry, &task_closes);
            std::future::pending::<()>().await;
        });
        runtime.block_on(tokio::task::yield_now());
        assert_eq!(closes.load(Ordering::SeqCst), 0);

        drop(runtime);

        wait_for_closes(&closes, 1);
    }
}
