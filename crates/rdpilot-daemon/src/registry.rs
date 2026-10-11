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
use rdpilot_ipc::{SessionId, SessionStatus};
use rdpilot_vocab::RawInput;
use tokio::sync::oneshot;

use crate::control::{
    EndReason, Grant, InputError, InputReport, NotHeld, SessionControl, TakeError,
};
use crate::events::{CloseReason, EventSource, RecordingTrigger, SessionEvents};
use crate::reconcile::RecordBook;
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
    records: Arc<RecordBook>,
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

/// Why [`Registry::expire_retained`] removes entries; it names the cause in
/// the log line of each removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The retention period passed.
    Retention,
    /// The daemon exits.
    DaemonExit,
}

impl Removal {
    fn describe(self) -> &'static str {
        match self {
            Removal::Retention => "after the retention period",
            Removal::DaemonExit => "at daemon exit",
        }
    }
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
/// no session is dropped bare. Its scope is the `Connecting` entry only:
/// `open_tracked` does not await after promotion, so a dropped future never
/// leaves a `Live` entry without its lease.
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
            records: Arc::new(RecordBook::new(sink)),
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
        self.recording_target_of(id, false)
    }

    /// Like [`Registry::recording_target`], for a recording start: a session
    /// that has ended cannot start a recording. Stop and annotate keep using
    /// [`Registry::recording_target`], so a recording in progress can be
    /// finished.
    ///
    /// # Errors
    ///
    /// As for [`Registry::recording_target`], plus
    /// [`DaemonError::SessionEnded`].
    pub(crate) fn recording_start_target(&self, id: &SessionId) -> Result<Target, DaemonError> {
        self.recording_target_of(id, true)
    }

    fn recording_target_of(&self, id: &SessionId, start: bool) -> Result<Target, DaemonError> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        let found = guard.get(id);
        if start {
            refuse_ended(id, found)?;
        }
        match found {
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
    /// (research Pattern 1). Returns `Err(DuplicateSession)` if `id` has any
    /// entry; [`Registry::open_tracked`] first frees a name held by an
    /// `Orphaned` or ended entry. On success it
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
    /// claimed or live (an `Orphaned` entry, or a session with the name that
    /// has ended, listed as `Disconnected`, is replaced instead), or the
    /// connector's error (mapped through [`DaemonError`]) if the connect
    /// itself fails — in which case the
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
        Ok(self.open_tracked(name, host, cfg).await?.id)
    }

    /// Like [`Registry::open`], but returns an internal exact-generation
    /// ownership handle for the IPC response path. It does not await after
    /// the session is inserted and recorded, so a dropped future leaves either
    /// no entry or an entry whose lease the caller holds.
    pub(crate) async fn open_tracked(
        &self,
        name: Option<String>,
        host: String,
        cfg: ConnectionConfig,
    ) -> Result<ConnectLease, DaemonError> {
        let (id, attempt, mut cancel_rx) = match &name {
            Some(n) => {
                let id = SessionId::from_str(n).map_err(DaemonError::Connect)?;
                // An Orphaned entry or a session with this name that has
                // ended (listed as Disconnected) is replaced. Its close never
                // fails the new connect: the entry and its record are gone
                // either way.
                if let Err(error) = self.release_name(&id).await {
                    eprintln!(
                        "rdpilot-daemon: closing ended session {} for a new connect failed: {error}",
                        id.as_str()
                    );
                }
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
        let lifecycle = session.lifecycle();
        // A single wall-clock capture shared by the entry's
        // `connected_since_wall`/`last_activity_wall` and the
        // reconciliation record — avoids two independent
        // `SystemTime::now()` reads racing apart by a few milliseconds for
        // what is conceptually one event.
        let now_wall = iso8601_now();
        // The record is written before the promotion, so the end hook (and
        // every close) can only remove a record that exists. A crash between
        // this write and the promotion leaves the record of a connected
        // session, which a restart correctly shows as Orphaned.
        self.records.opened(&id, generation, &host, &now_wall);
        let not_promoted = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            // Promote only the placeholder of this attempt. The check and the
            // insert share one lock scope with no `.await` in between.
            if matches!(
                guard.get(&id),
                Some(SessionEntry::Connecting { attempt: current, .. }) if *current == attempt
            ) {
                // Only a registered session gets an end hook, so a session
                // that is not promoted never reports an end. Spawning does
                // not await.
                let end_hook = crate::events::watch_session_end(
                    lifecycle.clone(),
                    Arc::clone(&events),
                    Arc::clone(&control),
                    Arc::clone(&self.records),
                    id.clone(),
                    generation,
                );
                guard.insert(
                    id.clone(),
                    SessionEntry::Live {
                        session: Arc::new(tokio::sync::Mutex::new(Some(session))),
                        generation,
                        lifecycle,
                        connected_since: Instant::now(),
                        connected_since_wall: now_wall.clone(),
                        name,
                        host: host.clone(),
                        last_activity: Instant::now(),
                        cua_leases: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                        frame,
                        events,
                        end_hook,
                        control,
                        last_activity_wall: now_wall.clone(),
                        ended_seen: None,
                    },
                );
                None
            } else {
                Some((session, events, control))
            }
        }; // guard dropped here — never held across the .await below
        if let Some((session, events, control)) = not_promoted {
            // The placeholder was removed while the session was in flight.
            // The session was never registered and has no end hook; remove
            // its record, then close it, awaited. If this future is dropped
            // during that close, the session drops mid-close; its
            // synchronous part has already run (as in `ConnectGuard::drop`)
            // and no entry or record is left to clean up.
            drop((events, control));
            self.records.closed(&id, generation);
            let _ = session.close().await;
            // A Disconnect sends its cause before the lock that removed the
            // placeholder is released, so it is already here.
            return Err(match cancel_rx.try_recv() {
                Ok(cause) => cancelled_by(cause),
                Err(_) => connect_cancelled(),
            });
        }
        Ok(lease)
    }

    /// Start the recording a connect asked for on the session `id` that
    /// [`Registry::open_tracked`] has just inserted. The outcome (the id, or
    /// why it could not start) never fails the connect. `None` when `record`
    /// is `None`.
    pub(crate) async fn start_connect_recording(
        &self,
        id: &SessionId,
        record: Option<RecordingTrigger>,
    ) -> Option<Result<String, String>> {
        let trigger = record?;
        Some(match self.recording_start_target(id) {
            Ok(target) => self
                .recordings
                .start(target, trigger, EventSource::Cli)
                .await
                .map(|started| started.id),
            Err(e) => Err(e.to_string()),
        })
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
            Some(entry @ SessionEntry::Live { .. }) => self.close_removed(id, entry, reason).await,
            Some(SessionEntry::Connecting { .. }) => {
                // Unreachable: a placeholder is handled under the lock above.
                Err(DaemonError::StillConnecting(id.as_str().to_owned()))
            }
            Some(SessionEntry::Orphaned { .. }) => {
                // Explicit removal of an orphan (DAEMON-04): its record, if
                // a failed purge left it, goes too; there is no live OS
                // thread to join.
                self.records.closed_orphan(id);
                Ok(())
            }
            None => Err(DaemonError::SessionNotFound(id.as_str().to_owned())),
        }
    }

    /// Close a `Live` entry that was removed from the map under the registry
    /// lock: the one close sequence of every removal path. Its own close is
    /// not a server-side end, so the end hook is dropped before the session
    /// loop is told to stop. The record is removed whatever the close
    /// returns, and the close error still reaches the caller.
    ///
    /// For a session that has already ended, the record is removed before
    /// the close is awaited, so a caller dropped during the close (a
    /// replacing Connect whose peer left) leaves no record. A session that is
    /// still live keeps its record until the close returns: a crash during
    /// that close must still show the session as Orphaned.
    async fn close_removed(
        &self,
        id: &SessionId,
        entry: SessionEntry,
        reason: CloseReason,
    ) -> Result<(), DaemonError> {
        let ended = entry.end_cause().is_some();
        let SessionEntry::Live {
            session,
            generation,
            end_hook,
            events,
            control,
            ..
        } = entry
        else {
            return Ok(());
        };
        drop(end_hook);
        if ended {
            self.records.closed(id, generation);
        }
        end_lease(&control, EndReason::SessionEnded).await;
        self.recordings.session_closed(&events, reason);
        // Take the SIZED `Box` out of the `Option` (never the unsized
        // `dyn ManagedSession` itself, which does not compile out of an
        // `Arc` -- research Pitfall 2) and close it -- NEVER a bare drop
        // (DAEMON-01). `try_lock` cannot be used here: `close` is this
        // session's terminal operation and MUST wait for any concurrent
        // `Registry::call` in flight to finish before reclaiming ownership,
        // not race past it. A box already taken by a concurrent close is a
        // clean success, not a double-close error.
        let boxed = { session.lock().await.take() };
        let result = match boxed {
            Some(boxed) => boxed.close().await, // joins the OS thread
            None => Ok(()),
        };
        self.records.closed(id, generation);
        result
    }

    /// Free the name `id` for a Connect that reuses it: remove an `Orphaned`
    /// entry, or remove and close a `Live` session that has ended (listed as
    /// `Disconnected`). The close reason of a recording is `disconnect`.
    /// Other entries stay. Returns whether it removed an entry.
    ///
    /// The record is removed before the close is awaited, so a Connect
    /// future dropped during the close leaves no record; the session it
    /// drops mid-close has already ended its thread's work and had its
    /// recording closed.
    ///
    /// # Errors
    ///
    /// The close error of the removed session.
    async fn release_name(&self, id: &SessionId) -> Result<bool, DaemonError> {
        let entry = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.get(id) {
                Some(SessionEntry::Orphaned { .. }) => guard.remove(id),
                Some(entry) if entry.end_cause().is_some() => guard.remove(id),
                _ => None,
            }
        };
        match entry {
            Some(SessionEntry::Orphaned { .. }) => {
                self.records.closed_orphan(id);
                Ok(true)
            }
            Some(entry) => self
                .close_removed(id, entry, CloseReason::Disconnect)
                .await
                .map(|()| true),
            None => Ok(false),
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

        match entry {
            Some(entry) => self
                .close_removed(&lease.id, entry, CloseReason::ConnectAborted)
                .await
                .map(|()| true),
            None => Ok(false),
        }
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
            let found = guard.get(id);
            refuse_ended(id, found)?;
            match found {
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
            let found = guard.get(id);
            refuse_ended(id, found)?;
            match found {
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

    /// The control lease of live session `id`; `None` once it has ended.
    pub(crate) fn control(&self, id: &SessionId) -> Option<Arc<SessionControl>> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(entry @ SessionEntry::Live { control, .. }) if entry.end_cause().is_none() => {
                Some(Arc::clone(control))
            }
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
            let found = guard.get(id);
            refuse_ended(id, found)?;
            match found {
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
    /// `None` once the session has ended.
    pub(crate) fn take_parts(&self, id: &SessionId) -> Option<(SessionSlot, Arc<SessionControl>)> {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(
                entry @ SessionEntry::Live {
                    session, control, ..
                },
            ) if entry.end_cause().is_none() => Some((Arc::clone(session), Arc::clone(control))),
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
            let found = guard.get(id);
            refuse_ended(id, found)?;
            match found {
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
    /// DAEMON-04). Its retention period starts now.
    pub fn seed_orphan(&self, id: SessionId, host: String, connected_since: String) {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let mut guard = self.sessions.lock().expect("registry mutex poisoned");
        guard.insert(
            id,
            SessionEntry::Orphaned {
                seeded_at: Instant::now(),
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

    /// `true` when the registry has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Delete the reconciliation records of the entries seeded at startup.
    /// A failed delete is logged and retried at the next write.
    pub(crate) fn purge_seeded_records(&self, ids: &[SessionId]) {
        self.records.purge_seeded(ids);
    }

    /// `true` while an entry keeps the daemon running: a connect in flight
    /// or a session that has not ended. `Orphaned` and ended entries do not
    /// (DAEMON-03's self-shutdown watcher polls this).
    #[must_use]
    pub(crate) fn holds_active(&self) -> bool {
        #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        guard.values().any(|entry| match entry {
            SessionEntry::Connecting { .. } => true,
            SessionEntry::Live { .. } => entry.end_cause().is_none(),
            SessionEntry::Orphaned { .. } => false,
        })
    }

    /// Remove the `Orphaned` entries and the ended `Live` entries whose
    /// retention period has passed, and log one line per removal.
    ///
    /// A `Live` entry's retention period starts when this sweep first sees
    /// it ended. The sweep stamps that time before it selects, in the same
    /// pass, so a `retention` of zero removes every ended entry, seen or
    /// not. Selection and removal share one registry lock scope, so an entry
    /// that replaced a removed name is never touched. An ended entry is
    /// closed through the one close path, with the recording close reason
    /// `idle_reap`; an `Orphaned` entry's record is removed unless a later
    /// session of the same name owns it.
    pub(crate) async fn expire_retained(&self, retention: std::time::Duration, cause: Removal) {
        let now = Instant::now();
        let expired: Vec<(SessionId, SessionEntry)> = {
            #[allow(clippy::expect_used)] // A poisoned registry mutex is unrecoverable.
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            let due: Vec<SessionId> = guard
                .iter_mut()
                .filter_map(|(id, entry)| {
                    let ended = entry.end_cause().is_some();
                    let since = match entry {
                        SessionEntry::Orphaned { seeded_at, .. } => *seeded_at,
                        SessionEntry::Live { ended_seen, .. } if ended => {
                            *ended_seen.get_or_insert(now)
                        }
                        _ => return None,
                    };
                    (now.duration_since(since) >= retention).then(|| id.clone())
                })
                .collect();
            due.into_iter()
                .filter_map(|id| guard.remove(&id).map(|entry| (id, entry)))
                .collect()
        };
        for (id, entry) in expired {
            let kind = if matches!(entry, SessionEntry::Orphaned { .. }) {
                self.records.closed_orphan(&id);
                "orphaned"
            } else {
                if let Err(error) = self.close_removed(&id, entry, CloseReason::IdleReap).await {
                    eprintln!(
                        "rdpilot-daemon: closing disconnected session {} failed: {error}",
                        id.as_str()
                    );
                }
                "disconnected"
            };
            eprintln!(
                "rdpilot-daemon: removed {kind} session {} {}",
                id.as_str(),
                cause.describe()
            );
        }
    }

    /// Every `Live` entry's id paired with the elapsed time since its
    /// `last_activity` (DAEMON-03's idle reaper, Plan 12-06, uses this to
    /// decide which sessions are stale). `Connecting`/`Orphaned` entries
    /// have no `last_activity` to age and are never candidates for idle
    /// reap -- only a `Live` entry that has not ended is included. An ended
    /// entry leaves through [`Registry::expire_retained`] instead.
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
                } => (entry.end_cause().is_none()
                    && cua_leases.load(Ordering::Relaxed) == 0
                    && !control.human_active())
                .then(|| {
                    // Human input and lease ends count as activity.
                    let latest = control
                        .human_activity()
                        .map_or(*last_activity, |(at, _)| at.max(*last_activity));
                    (id.clone(), latest.elapsed())
                }),
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

/// [`DaemonError::SessionEnded`] when `entry` is a session that has ended
/// (listed as `Disconnected`): operational calls name the end instead of
/// reaching a session whose loop is gone.
fn refuse_ended(id: &SessionId, entry: Option<&SessionEntry>) -> Result<(), DaemonError> {
    match entry.and_then(SessionEntry::end_cause) {
        Some(cause) => Err(DaemonError::SessionEnded {
            id: id.as_str().to_owned(),
            cause: cause.to_string(),
        }),
        None => Ok(()),
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
        let target = self
            .inner
            .recording_start_target(id)
            .map_err(Refusal::from)?;
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
            // The viewer has nothing to show of an orphan; `list` keeps it.
            .filter(|(_, entry)| !matches!(entry, SessionEntry::Orphaned { .. }))
            .map(|(id, entry)| {
                let status = self.status_of(id, entry);
                let ended = status.status == rdpilot_ipc::SessionLifecycle::Disconnected;
                let frames = matches!(entry, SessionEntry::Live { frame: Some(_), .. });
                let input =
                    matches!(entry, SessionEntry::Live { control, .. } if control.has_input());
                ViewerSession {
                    status,
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
        /// The session's lifecycle; a test ends it to simulate a server end.
        lifecycle: LifecycleTx,
        /// `close` returns an injected error (after it counted).
        close_fails: bool,
        /// `close` waits for a permit here before it counts.
        close_gate: Option<Arc<tokio::sync::Semaphore>>,
        /// `ping` waits for a permit here (an operation in flight).
        ping_gate: Option<Arc<tokio::sync::Semaphore>>,
    }

    type LifecycleTx = Arc<tokio::sync::watch::Sender<rdpilot::SessionState>>;

    fn live_lifecycle() -> LifecycleTx {
        Arc::new(
            tokio::sync::watch::channel(rdpilot::SessionState {
                lifecycle: rdpilot::Lifecycle::Live,
                desktop_size: (1920, 1080),
            })
            .0,
        )
    }

    /// End `lifecycle` with `cause` unless it has ended, as the SDK does.
    fn end_lifecycle(lifecycle: &LifecycleTx, cause: rdpilot::EndCause) {
        lifecycle.send_if_modified(|state| {
            if state.lifecycle == rdpilot::Lifecycle::Live {
                state.lifecycle = rdpilot::Lifecycle::Ended(cause);
                true
            } else {
                false
            }
        });
    }

    fn server_end(lifecycle: &LifecycleTx) {
        end_lifecycle(
            lifecycle,
            rdpilot::EndCause::ServerEnded("fake server end".to_owned()),
        );
    }

    impl FakeSession {
        fn new(closes: Arc<AtomicU32>) -> Self {
            FakeSession {
                closes,
                lifecycle: live_lifecycle(),
                close_fails: false,
                close_gate: None,
                ping_gate: None,
            }
        }
    }

    impl ManagedSession for FakeSession {
        /// Counts when the returned future is polled, not when `close` is
        /// called: a future that is dropped unpolled closes nothing.
        fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
            let closes = Arc::clone(&self.closes);
            end_lifecycle(&self.lifecycle, rdpilot::EndCause::ClientClosed);
            let fails = self.close_fails;
            let gate = self.close_gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    #[allow(clippy::expect_used)]
                    gate.acquire().await.expect("gate is never closed").forget();
                }
                closes.fetch_add(1, Ordering::SeqCst);
                if fails {
                    Err(DaemonError::Connect("injected close failure".to_owned()))
                } else {
                    Ok(())
                }
            })
        }
        fn lifecycle(&self) -> tokio::sync::watch::Receiver<rdpilot::SessionState> {
            self.lifecycle.subscribe()
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
            let gate = self.ping_gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    #[allow(clippy::expect_used)]
                    gate.acquire().await.expect("gate is never closed").forget();
                }
                Ok(std::time::Duration::from_millis(0))
            })
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
                    Ok(Box::new(FakeSession::new(Arc::new(AtomicU32::new(0))))
                        as Box<dyn ManagedSession>)
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
        // The entry drops it at once; the aborted end hook as soon as the
        // runtime drops the task.
        until("the log is dropped with the entry", || {
            weak.upgrade().is_none()
        })
        .await;
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
                Ok(Box::new(FakeSession::new(closes)) as Box<dyn ManagedSession>)
            })
        }
    }

    fn gated_registry(calls: usize) -> (Arc<Registry>, Vec<GatedCall>, Arc<AtomicU32>) {
        let (connector, handles, closes) = gated(calls);
        let registry = Arc::new(Registry::new(connector, Arc::new(NoopReconciliationSink)));
        (registry, handles, closes)
    }

    type Opened = Result<ConnectLease, DaemonError>;

    /// Poll `open_tracked` for `name` exactly once and hand back the pinned,
    /// unfinished future.
    async fn start_open<'a>(
        registry: &'a Registry,
        name: &str,
    ) -> Pin<Box<dyn Future<Output = Opened> + Send + 'a>> {
        let mut open: Pin<Box<dyn Future<Output = Opened> + Send + 'a>> =
            Box::pin(registry.open_tracked(Some(name.to_owned()), "h".to_owned(), test_cfg()));
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
        let lease = second.await.expect("attempt 2 succeeds");
        assert_eq!(lease.generation, second_attempt);
        assert_eq!(live_generation(&registry, "s"), Some(second_attempt));
        assert_eq!(closes.load(Ordering::SeqCst), 1, "attempt 2 stays open");
        assert!(!registry.cancel_connecting(&lease_of("s", first_attempt)));
        assert_eq!(live_generation(&registry, "s"), Some(second_attempt));
    }

    #[tokio::test]
    async fn a_stale_delivered_attempt_never_removes_the_record_of_a_later_attempt() {
        let (connector, calls, _closes) = gated(2);
        let sink = Arc::new(CountingSink::default());
        let registry = Registry::new(connector, Arc::clone(&sink) as Arc<dyn ReconciliationSink>);

        // Attempt 1 delivers its session, then loses its placeholder.
        let first = start_open(&registry, "s").await;
        until("attempt 1 started", || flag(&calls[0].started)).await;
        let first_attempt = connecting_attempt(&registry, "s").expect("attempt 1");
        calls[0].gate.add_permits(1);
        until("attempt 1 delivered", || flag(&calls[0].delivered)).await;
        assert!(registry.cancel_connecting(&lease_of("s", first_attempt)));

        // Attempt 2 claims the name, delivers and is promoted first.
        let second = start_open(&registry, "s").await;
        until("attempt 2 started", || flag(&calls[1].started)).await;
        calls[1].gate.add_permits(1);
        let lease = second.await.expect("attempt 2 succeeds");

        // Only then does attempt 1 see its delivered session.
        assert!(first.await.is_err());
        assert_eq!(
            sink.count("closed", "s"),
            0,
            "a stale attempt removed the record of the live session"
        );
        assert_eq!(live_generation(&registry, "s"), Some(lease.generation));

        registry.close(&sid("s")).await.expect("disconnect");
        assert_eq!(sink.count("closed", "s"), 1);
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
        let lease = second.await.expect("attempt 2 succeeds");

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
        let lease = open.await.expect("the connect succeeds");

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
        let lease = second.await.expect("the second connect succeeds");
        assert_eq!(lease.generation, second_attempt);
        assert_eq!(live_generation(&registry, "s"), Some(second_attempt));
    }

    #[tokio::test]
    async fn disconnect_while_connecting_leaves_an_independent_live_session() {
        let (registry, calls, closes) = gated_registry(2);
        let keep = start_open(&registry, "keep").await;
        until("the first connect started", || flag(&calls[0].started)).await;
        calls[0].gate.add_permits(1);
        let keep_lease = keep.await.expect("the first connect succeeds");

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
            .open_tracked(Some("s".to_owned()), "h".to_owned(), test_cfg())
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
        let lease = registry
            .open_tracked(Some("s".to_owned()), "h".to_owned(), test_cfg())
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
                Box::new(FakeSession::new(Arc::clone(closes))) as Box<dyn ManagedSession>,
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

    // --- Session lifecycle: status, server end, records (S2) --------------

    /// A connector whose sessions the test can end, fail to close, and gate.
    /// It keeps every delivered session's lifecycle sender in order.
    #[derive(Default)]
    struct ScriptedConnector {
        lifecycles: Mutex<Vec<LifecycleTx>>,
        closes: Arc<AtomicU32>,
        close_fails: AtomicBool,
        /// The next delivered session's close waits on this.
        close_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
        /// The next delivered session's ping waits on this.
        ping_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
        /// The next connect waits on this before it delivers.
        connect_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    }

    impl ScriptedConnector {
        /// The lifecycle sender of the `n`-th delivered session.
        fn lifecycle(&self, n: usize) -> LifecycleTx {
            #[allow(clippy::expect_used)]
            Arc::clone(&self.lifecycles.lock().expect("test mutex")[n])
        }
        fn closes(&self) -> u32 {
            self.closes.load(Ordering::SeqCst)
        }
    }

    impl SessionConnector for ScriptedConnector {
        fn connect(
            &self,
            _cfg: ConnectionConfig,
        ) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            let mut session = FakeSession::new(Arc::clone(&self.closes));
            session.close_fails = self.close_fails.load(Ordering::SeqCst);
            #[allow(clippy::expect_used)]
            {
                session.close_gate = self.close_gate.lock().expect("test mutex").take();
                session.ping_gate = self.ping_gate.lock().expect("test mutex").take();
                self.lifecycles
                    .lock()
                    .expect("test mutex")
                    .push(Arc::clone(&session.lifecycle));
            }
            #[allow(clippy::expect_used)]
            let gate = self.connect_gate.lock().expect("test mutex").take();
            Box::pin(async move {
                if let Some(gate) = gate {
                    #[allow(clippy::expect_used)]
                    gate.acquire().await.expect("gate is never closed").forget();
                }
                Ok(Box::new(session) as Box<dyn ManagedSession>)
            })
        }
    }

    /// Counts reconciliation writes per id, in order.
    #[derive(Default)]
    struct CountingSink {
        writes: Mutex<Vec<(&'static str, String)>>,
    }

    impl ReconciliationSink for CountingSink {
        fn record_open(&self, id: &SessionId, _host: &str, _since: &str) {
            #[allow(clippy::expect_used)]
            self.writes
                .lock()
                .expect("test mutex")
                .push(("open", id.as_str().to_owned()));
        }
        fn record_closed(&self, id: &SessionId) {
            #[allow(clippy::expect_used)]
            self.writes
                .lock()
                .expect("test mutex")
                .push(("closed", id.as_str().to_owned()));
        }
    }

    impl CountingSink {
        fn count(&self, kind: &str, id: &str) -> usize {
            #[allow(clippy::expect_used)]
            self.writes
                .lock()
                .expect("test mutex")
                .iter()
                .filter(|(k, i)| *k == kind && i == id)
                .count()
        }
    }

    fn scripted() -> (Arc<Registry>, Arc<ScriptedConnector>, Arc<CountingSink>) {
        let connector = Arc::new(ScriptedConnector::default());
        let sink = Arc::new(CountingSink::default());
        let registry = Arc::new(Registry::new(
            Arc::clone(&connector) as Arc<dyn SessionConnector>,
            Arc::clone(&sink) as Arc<dyn ReconciliationSink>,
        ));
        (registry, connector, sink)
    }

    fn sid(name: &str) -> SessionId {
        SessionId::from_str(name).expect("non-empty literal")
    }

    async fn open_named(registry: &Registry, name: &str) -> ConnectLease {
        registry
            .open_tracked(Some(name.to_owned()), "h".to_owned(), test_cfg())
            .await
            .expect("open")
    }

    fn listed(registry: &Registry, name: &str) -> Option<SessionLifecycle> {
        registry
            .list()
            .into_iter()
            .find(|s| s.id == name)
            .map(|s| s.status)
    }

    fn viewer_view(registry: &Arc<Registry>, name: &str) -> Option<(SessionLifecycle, bool)> {
        ViewerRegistry::new(Arc::clone(registry))
            .sessions()
            .into_iter()
            .find(|s| s.status.id == name)
            .map(|s| (s.status.status, s.ended))
    }

    /// `describe()` of the registered session, read without the registry's
    /// own refusal checks.
    fn described(registry: &Registry, name: &str) -> Option<SessionLifecycle> {
        #[allow(clippy::expect_used)]
        let slot = match registry
            .sessions
            .lock()
            .expect("registry mutex")
            .get(&sid(name))
        {
            Some(SessionEntry::Live { session, .. }) => Arc::clone(session),
            _ => return None,
        };
        let guard = slot.try_lock().ok()?;
        guard.as_deref().map(|session| session.describe())
    }

    fn ended_events(registry: &Registry, name: &str) -> usize {
        registry
            .events(&sid(name))
            .map_or(0, |log| ended_count(&log))
    }

    fn ended_count(log: &SessionEvents) -> usize {
        log.after(0)
            .events
            .iter()
            .filter(|e| e.kind == crate::events::EventKind::SessionEnded)
            .count()
    }

    #[tokio::test]
    async fn status_views_agree_across_connecting_live_and_server_end() {
        let (registry, connector, _sink) = scripted();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *connector.connect_gate.lock().expect("test mutex") = Some(Arc::clone(&gate));
        let open = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { open_named(&registry, "alpha").await })
        };
        until("the placeholder exists", || {
            listed(&registry, "alpha").is_some()
        })
        .await;
        assert_eq!(
            listed(&registry, "alpha"),
            Some(SessionLifecycle::Connecting)
        );
        assert_eq!(
            viewer_view(&registry, "alpha"),
            Some((SessionLifecycle::Connecting, false))
        );

        gate.add_permits(1);
        open.await.expect("join");
        assert_eq!(listed(&registry, "alpha"), Some(SessionLifecycle::Live));
        assert_eq!(described(&registry, "alpha"), Some(SessionLifecycle::Live));
        assert_eq!(
            viewer_view(&registry, "alpha"),
            Some((SessionLifecycle::Live, false))
        );
        assert_eq!(ended_events(&registry, "alpha"), 0);

        server_end(&connector.lifecycle(0));
        assert_eq!(
            listed(&registry, "alpha"),
            Some(SessionLifecycle::Disconnected)
        );
        assert_eq!(
            described(&registry, "alpha"),
            Some(SessionLifecycle::Disconnected)
        );
        assert_eq!(
            viewer_view(&registry, "alpha"),
            Some((SessionLifecycle::Disconnected, true))
        );
        until("the end hook recorded the end", || {
            ended_events(&registry, "alpha") == 1
        })
        .await;
        let log = registry.events(&sid("alpha")).expect("log");
        registry.close(&sid("alpha")).await.expect("disconnect");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(ended_count(&log), 1, "a later Disconnect adds nothing");
    }

    #[tokio::test]
    async fn status_views_agree_across_live_and_client_close() {
        let (registry, connector, _sink) = scripted();
        open_named(&registry, "alpha").await;
        let log = registry.events(&sid("alpha")).expect("log");
        registry.close(&sid("alpha")).await.expect("disconnect");
        assert_eq!(listed(&registry, "alpha"), None);
        assert_eq!(viewer_view(&registry, "alpha"), None);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(ended_count(&log), 0, "a client close is not a server end");
        assert_eq!(connector.closes(), 1);
    }

    fn assert_ended_refusal(error: DaemonError) {
        assert!(
            matches!(&error, DaemonError::SessionEnded { id, .. } if id == "alpha"),
            "{error:?}"
        );
        let wire = rdpilot_ipc::WireError::from(error);
        assert_eq!(wire.code, rdpilot_ipc::WireErrorCode::SessionNotFound);
        assert!(wire.message.contains("ended"), "{}", wire.message);
        assert!(wire.message.contains("fake server end"), "{}", wire.message);
    }

    #[tokio::test]
    async fn a_disconnected_entry_refuses_operations_and_keeps_reads() {
        let (registry, connector, _sink) = scripted();
        open_named(&registry, "alpha").await;
        server_end(&connector.lifecycle(0));
        let id = sid("alpha");

        assert_ended_refusal(
            registry
                .call(&id, |session| session.ping())
                .await
                .expect_err("ping"),
        );
        assert_ended_refusal(
            registry
                .call_acting(&id, |session| {
                    session.send_key(rdpilot::KeyAction::Combo(vec![rdpilot::Key::Enter]))
                })
                .await
                .expect_err("key"),
        );
        assert_ended_refusal(registry.attach_cua(&id).await.err().expect("attach"));
        assert_ended_refusal(
            registry
                .takeover(&id, EventSource::Cli)
                .await
                .expect_err("takeover"),
        );
        assert_ended_refusal(
            registry
                .recording_start_target(&id)
                .err()
                .expect("recording start"),
        );
        assert!(registry.control(&id).is_none());
        assert!(registry.take_parts(&id).is_none());
        let control = ViewerControl::new(Arc::clone(&registry));
        assert!(matches!(
            control.heartbeat(&id, "lease"),
            Err(ControlRefusal::NoSession)
        ));
        assert!(matches!(
            control
                .take(&id, std::net::IpAddr::from([127, 0, 0, 1]))
                .await,
            Err(ControlRefusal::NoSession)
        ));

        // Reads still answer.
        assert_eq!(
            listed(&registry, "alpha"),
            Some(SessionLifecycle::Disconnected)
        );
        assert!(viewer_view(&registry, "alpha").is_some());
        assert!(registry.events(&id).is_some());
        assert!(registry.recording_target(&id).is_ok(), "stop and annotate");
        assert!(!matches!(
            ViewerRegistry::new(Arc::clone(&registry)).frame_source(&id),
            FrameLookup::Closed
        ));

        registry.close(&id).await.expect("disconnect removes it");
        assert!(registry.list().is_empty());
        assert_eq!(connector.closes(), 1);
    }

    #[tokio::test]
    async fn a_connect_with_the_same_name_replaces_a_disconnected_entry() {
        let (registry, connector, sink) = scripted();
        let first = open_named(&registry, "alpha").await;
        server_end(&connector.lifecycle(0));

        let second = open_named(&registry, "alpha").await;
        assert!(second.generation > first.generation);
        assert_eq!(connector.closes(), 1, "the ended session was closed once");
        let statuses = registry.list();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].status, SessionLifecycle::Live);
        assert_eq!(live_generation(&registry, "alpha"), Some(second.generation));
        until("the old record went", || sink.count("closed", "alpha") == 1).await;
        assert_eq!(sink.count("open", "alpha"), 2);

        // A live entry with the name still refuses a second connect.
        let duplicate = registry
            .open(Some("alpha".to_owned()), "h".to_owned(), test_cfg())
            .await;
        assert!(matches!(duplicate, Err(DaemonError::DuplicateSession(_))));
    }

    #[tokio::test]
    async fn server_end_removes_the_record_at_once() {
        let (registry, connector, sink) = scripted();
        open_named(&registry, "alpha").await;
        assert_eq!(sink.count("open", "alpha"), 1);
        server_end(&connector.lifecycle(0));
        until("the record went", || sink.count("closed", "alpha") == 1).await;
        assert_eq!(
            listed(&registry, "alpha"),
            Some(SessionLifecycle::Disconnected)
        );
        registry.close(&sid("alpha")).await.expect("disconnect");
        assert_eq!(sink.count("closed", "alpha"), 1, "nothing more");
    }

    #[tokio::test]
    async fn the_end_hook_never_touches_a_later_entry_with_the_same_name() {
        let (registry, connector, sink) = scripted();
        let first = open_named(&registry, "alpha").await;
        server_end(&connector.lifecycle(0));
        until("the first record went", || {
            sink.count("closed", "alpha") == 1
        })
        .await;
        let second = open_named(&registry, "alpha").await;
        // A late hook or close of the first generation writes nothing.
        assert!(!registry.records.closed(&sid("alpha"), first.generation));
        assert_eq!(sink.count("closed", "alpha"), 1);
        assert_eq!(sink.count("open", "alpha"), 2);
        assert_eq!(live_generation(&registry, "alpha"), Some(second.generation));
        // The first session's lifecycle ending again changes nothing.
        server_end(&connector.lifecycle(0));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(sink.count("closed", "alpha"), 1);
        assert_eq!(listed(&registry, "alpha"), Some(SessionLifecycle::Live));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_racing_close_and_end_remove_the_record_once() {
        let (registry, connector, sink) = scripted();
        for round in 0..50usize {
            open_named(&registry, "alpha").await;
            let lifecycle = connector.lifecycle(round);
            let end = tokio::spawn(async move { server_end(&lifecycle) });
            let close = {
                let registry = Arc::clone(&registry);
                tokio::spawn(async move { registry.close(&sid("alpha")).await })
            };
            end.await.expect("join");
            close.await.expect("join").expect("disconnect");
            until("the record went", || sink.count("closed", "alpha") > round).await;
            tokio::time::sleep(Duration::from_millis(5)).await;
            assert_eq!(sink.count("closed", "alpha"), round + 1, "round {round}");
            assert_eq!(connector.closes() as usize, round + 1, "round {round}");
        }
    }

    #[tokio::test]
    async fn a_failed_close_still_removes_the_record() {
        let (registry, connector, sink) = scripted();
        connector.close_fails.store(true, Ordering::SeqCst);
        open_named(&registry, "alpha").await;
        let error = registry
            .close(&sid("alpha"))
            .await
            .expect_err("the injected close failure is returned");
        assert!(
            matches!(&error, DaemonError::Connect(m) if m == "injected close failure"),
            "{error:?}"
        );
        assert_eq!(sink.count("closed", "alpha"), 1);
        assert!(registry.list().is_empty());

        let lease = open_named(&registry, "beta").await;
        let error = registry
            .close_if_generation(&lease)
            .await
            .expect_err("the injected close failure is returned");
        assert!(matches!(&error, DaemonError::Connect(_)), "{error:?}");
        assert_eq!(sink.count("closed", "beta"), 1);
        assert!(registry.list().is_empty());
    }

    #[tokio::test]
    async fn a_dropped_replacing_connect_leaves_no_record_of_the_ended_entry() {
        let (registry, connector, sink) = scripted();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *connector.close_gate.lock().expect("test mutex") = Some(Arc::clone(&gate));
        open_named(&registry, "alpha").await;
        // End the session and start the replacing connect before the end
        // hook can run: this test runs on one thread and does not yield.
        server_end(&connector.lifecycle(0));
        let replacing = start_open(&registry, "alpha").await;
        assert_eq!(registry.len(), 0, "the ended entry was removed");
        assert_eq!(connector.closes(), 0, "its close waits on the gate");
        drop(replacing);
        assert_eq!(sink.count("closed", "alpha"), 1, "the record is gone");
        gate.add_permits(1);
    }

    fn seed(registry: &Registry, name: &str) {
        registry.seed_orphan(sid(name), "h".to_owned(), "2026-01-01T00:00:00Z".to_owned());
    }

    const SHORT: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn an_orphan_is_removed_after_the_retention_and_not_before() {
        let (registry, connector, sink) = scripted();
        seed(&registry, "o");
        registry.expire_retained(SHORT, Removal::Retention).await;
        assert_eq!(listed(&registry, "o"), Some(SessionLifecycle::Orphaned));
        assert_eq!(sink.count("closed", "o"), 0);

        tokio::time::sleep(SHORT + Duration::from_millis(10)).await;
        registry.expire_retained(SHORT, Removal::Retention).await;
        assert_eq!(listed(&registry, "o"), None);
        assert_eq!(sink.count("closed", "o"), 1, "its record goes too");
        assert_eq!(connector.closes(), 0, "an orphan has no session to close");
    }

    #[tokio::test]
    async fn an_ended_entry_is_removed_after_the_retention_through_its_close() {
        let (registry, connector, sink) = scripted();
        open_named(&registry, "alpha").await;
        server_end(&connector.lifecycle(0));
        until("the record went", || sink.count("closed", "alpha") == 1).await;

        registry.expire_retained(SHORT, Removal::Retention).await;
        assert_eq!(
            listed(&registry, "alpha"),
            Some(SessionLifecycle::Disconnected)
        );
        assert_eq!(connector.closes(), 0);

        tokio::time::sleep(SHORT + Duration::from_millis(10)).await;
        registry.expire_retained(SHORT, Removal::Retention).await;
        assert_eq!(listed(&registry, "alpha"), None);
        assert_eq!(connector.closes(), 1, "removed through its close");
        assert_eq!(sink.count("closed", "alpha"), 1, "the record went once");
    }

    #[tokio::test]
    async fn a_zero_retention_removes_an_ended_entry_the_sweep_never_saw() {
        let (registry, connector, _sink) = scripted();
        open_named(&registry, "alpha").await;
        server_end(&connector.lifecycle(0));
        registry
            .expire_retained(Duration::ZERO, Removal::DaemonExit)
            .await;
        assert!(registry.is_empty());
        assert_eq!(connector.closes(), 1);
    }

    #[tokio::test]
    async fn the_sweep_never_touches_a_live_entry_or_one_that_replaced_an_orphan() {
        let (registry, connector, sink) = scripted();
        seed(&registry, "a");
        let lease = open_named(&registry, "a").await;
        registry
            .expire_retained(Duration::ZERO, Removal::Retention)
            .await;
        assert_eq!(live_generation(&registry, "a"), Some(lease.generation));
        assert_eq!(connector.closes(), 0);
        assert!(registry.holds_active());
        // The open wrote the record after the orphan's removal; nothing
        // removed it since.
        #[allow(clippy::expect_used)]
        let writes = sink.writes.lock().expect("test mutex").clone();
        assert_eq!(writes.last(), Some(&("open", "a".to_owned())), "{writes:?}");
    }

    #[tokio::test]
    async fn live_idle_durations_excludes_ended_entries() {
        let (registry, connector, _sink) = scripted();
        open_named(&registry, "alpha").await;
        open_named(&registry, "beta").await;
        server_end(&connector.lifecycle(0));
        let idle = registry.live_idle_durations();
        assert_eq!(idle.len(), 1, "{idle:?}");
        assert_eq!(idle[0].0.as_str(), "beta");
    }

    #[tokio::test]
    async fn holds_active_ignores_orphaned_and_ended_entries() {
        let (registry, connector, _sink) = scripted();
        assert!(!registry.holds_active());
        seed(&registry, "o");
        assert!(!registry.holds_active());
        open_named(&registry, "alpha").await;
        assert!(registry.holds_active());
        server_end(&connector.lifecycle(0));
        assert!(!registry.holds_active());
        assert_eq!(registry.len(), 2);
    }

    #[tokio::test]
    async fn the_viewer_list_omits_orphans_and_list_keeps_them() {
        let (registry, _connector, _sink) = scripted();
        seed(&registry, "o");
        open_named(&registry, "alpha").await;
        assert_eq!(viewer_view(&registry, "o"), None);
        assert_eq!(
            viewer_view(&registry, "alpha"),
            Some((SessionLifecycle::Live, false))
        );
        assert_eq!(listed(&registry, "o"), Some(SessionLifecycle::Orphaned));
    }

    #[tokio::test]
    async fn a_connect_with_an_orphans_name_replaces_it() {
        let (registry, connector, sink) = scripted();
        seed(&registry, "a");
        let lease = open_named(&registry, "a").await;
        let statuses = registry.list();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].status, SessionLifecycle::Live);
        assert_eq!(live_generation(&registry, "a"), Some(lease.generation));
        assert_eq!(connector.closes(), 0);
        assert_eq!(sink.count("open", "a"), 1);
        assert_eq!(sink.count("closed", "a"), 1, "the orphan's record went");
    }

    #[tokio::test]
    async fn list_does_not_wait_for_an_operation_in_flight() {
        let (registry, connector, _sink) = scripted();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *connector.ping_gate.lock().expect("test mutex") = Some(Arc::clone(&gate));
        open_named(&registry, "alpha").await;
        let op = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.call(&sid("alpha"), |s| s.ping()).await })
        };
        until("the operation holds the session", || {
            described(&registry, "alpha").is_none()
        })
        .await;
        let started = std::time::Instant::now();
        assert_eq!(listed(&registry, "alpha"), Some(SessionLifecycle::Live));
        assert_eq!(
            viewer_view(&registry, "alpha"),
            Some((SessionLifecycle::Live, false))
        );
        server_end(&connector.lifecycle(0));
        assert_eq!(
            listed(&registry, "alpha"),
            Some(SessionLifecycle::Disconnected)
        );
        assert_eq!(
            viewer_view(&registry, "alpha"),
            Some((SessionLifecycle::Disconnected, true))
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        gate.add_permits(1);
        op.await.expect("join").expect("the operation finishes");
    }
}
