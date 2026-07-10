//! Seams that decouple the registry (Plan 12-03) from `rdpilot::Session`
//! (research Wave 0 Gap): a trait-based session-connect abstraction so the
//! DAEMON-01 leak-soak and SESSION-01/04 concurrency tests run OFFLINE (no
//! real RDP target) by injecting a deterministic fake that still owns a
//! real OS thread.
//!
//! - [`SessionConnector`] abstracts `rdpilot::Session::connect` — production
//!   code uses [`RealConnector`]; tests inject a fake.
//! - [`ManagedSession`] abstracts the owned session's `close()` contract —
//!   `rdpilot::Session` implements it directly (joining the real OS
//!   thread); a test fake implements it by joining its own spawned thread.
//! - [`ReconciliationSink`] lets the registry emit state transitions
//!   without knowing about disk persistence (Plan 12-05 provides the real
//!   JSON-disk implementation; a no-op impl is provided here for tests).
//! - [`SessionEntry`] is the registry's per-session map value.
//! - [`DaemonError`] is this crate's error type; its `Display` never embeds
//!   a credential (D-31) — messages reference ids/hosts only.
//!
//! Both async trait methods here return a boxed, pinned future rather than
//! using native `async fn` in the trait: [`SessionConnector`]/
//! [`ManagedSession`] must be usable as `dyn` trait objects (the registry
//! holds `Arc<dyn SessionConnector>` and `Box<dyn ManagedSession>`), and
//! native async-fn-in-traits are not `dyn`-compatible without boxing the
//! returned future by hand.

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use rdpilot::{ConnectionConfig, Session};
use rdpilot_ipc::{SessionId, SessionLifecycle, SessionStatus};

/// A boxed, pinned future — the manual "boxed async fn in a dyn trait"
/// shape used by [`SessionConnector::connect`] and [`ManagedSession::close`].
///
/// Deliberately NOT `+ Send`: a compile-probe during this plan's authoring
/// confirmed `rdpilot::Session::connect`'s returned future is NOT `Send`
/// (root cause: the same higher-ranked `&dyn PduHint`-across-`.await`
/// limitation `crates/rdpilot/src/session.rs`'s own doc comment documents
/// for the reactivation step of the session loop — it turns out to reach
/// the initial connect handshake too, not only the ongoing loop). Since a
/// `dyn Future` trait object erases auto-trait information unless declared
/// on the object type itself, requiring `+ Send` here would make
/// `RealConnector` (the production, real-`rdpilot`-backed implementation)
/// impossible to implement at all.
///
/// **Consequence for callers (Plan 12-03 `registry.rs` / Plan 12-04
/// `dispatch.rs` / Plan 12-06 `server.rs`):** any async code path that
/// `.await`s a `SessionConnector::connect`/`ManagedSession::close` call
/// (directly or via `Registry::open`/`Registry::close`) must run on a
/// single-threaded Tokio context — a `tokio::task::LocalSet` +
/// `spawn_local`, NOT a bare `tokio::spawn` on the default multi-threaded
/// runtime, which requires `Send` futures. This is a workload-appropriate
/// constraint (research: "session counts are small... human-scale
/// connect/disconnect/list traffic, not thousands of req/s") and mirrors
/// `rdpilot::Session`'s own established pattern of sidestepping this exact
/// HRTB limitation via a dedicated single-threaded execution context rather
/// than fighting it.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Abstracts the owned, live session handle's close contract.
///
/// Mirrors [`rdpilot::Session::close`]'s awaited thread-join contract
/// exactly (research Pattern 2): `close` takes `self` by value (via
/// `Box<Self>`, since this is used behind `dyn`) and returns once the
/// session's background thread has been joined — never a bare drop.
pub trait ManagedSession: Send + 'static {
    /// Gracefully close the session, awaiting the background thread's join.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError`] if the underlying close/join failed.
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>>;

    /// The session's current lifecycle status (D-30), for `list` rendering.
    fn describe(&self) -> SessionLifecycle;
}

impl ManagedSession for Session {
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>> {
        Box::pin(async move { (*self).close().await.map_err(DaemonError::Sdk) })
    }

    fn describe(&self) -> SessionLifecycle {
        // Baseline until Plan 12-03/12-06 derives a finer-grained status
        // from the SDK keepalive signal (D-30) — a `Session` handle only
        // exists here while it is live.
        SessionLifecycle::Live
    }
}

/// Abstracts `rdpilot::Session::connect` so the registry (Plan 12-03) can
/// run its concurrency/leak tests against a deterministic fake, with no
/// real RDP target.
pub trait SessionConnector: Send + Sync + 'static {
    /// Connect to `cfg`, returning an owned, type-erased managed session.
    ///
    /// # Errors
    ///
    /// Returns [`DaemonError::Sdk`] (production) or a fake-specific
    /// [`DaemonError::Connect`] (tests) on failure.
    fn connect(&self, cfg: ConnectionConfig) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>>;
}

/// The production [`SessionConnector`]: delegates to
/// [`rdpilot::Session::connect`].
#[derive(Debug, Default, Clone, Copy)]
#[allow(dead_code)] // Wired into the registry by Plan 12-06's server assembly (interface-first).
pub struct RealConnector;

impl SessionConnector for RealConnector {
    fn connect(&self, cfg: ConnectionConfig) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        Box::pin(async move {
            let session = Session::connect(&cfg).await.map_err(DaemonError::Sdk)?;
            Ok(Box::new(session) as Box<dyn ManagedSession>)
        })
    }
}

/// Lets the registry (Plan 12-03) emit session-state transitions without
/// knowing about disk persistence (Plan 12-05 provides the real JSON-disk
/// implementation).
///
/// Deliberately SYNCHRONOUS (no `.await`) so the registry can call these
/// while holding no lock, with no async complexity in teardown paths.
pub trait ReconciliationSink: Send + Sync + 'static {
    /// Record that session `id` opened against `host` at `connected_since`
    /// (an ISO-8601 timestamp string).
    fn record_open(&self, id: &SessionId, host: &str, connected_since: &str);

    /// Record that session `id` closed (explicit disconnect, idle reap, or
    /// orphan reconciliation).
    fn record_closed(&self, id: &SessionId);
}

/// A no-op [`ReconciliationSink`] — used by tests that do not exercise
/// disk persistence (the real Plan 12-05 impl is exercised separately).
#[derive(Debug, Default, Clone, Copy)]
#[allow(dead_code)] // Consumed by Plan 12-03's registry unit tests (interface-first).
pub struct NoopReconciliationSink;

impl ReconciliationSink for NoopReconciliationSink {
    fn record_open(&self, _id: &SessionId, _host: &str, _connected_since: &str) {}
    fn record_closed(&self, _id: &SessionId) {}
}

/// The registry's per-session map value (Plan 12-03).
pub enum SessionEntry {
    /// A name/id has been atomically claimed under the registry lock but
    /// the (slow, async) connect has not yet completed (research Pattern 1
    /// — never held across an `.await` itself); it is the placeholder
    /// upgraded or removed once `connect` resolves.
    Connecting {
        /// When the claim was made (for stuck-connect diagnostics).
        claimed_at: Instant,
    },
    /// A live, connected session.
    Live {
        /// The owned, type-erased managed session.
        session: Box<dyn ManagedSession>,
        /// When the connect completed (monotonic; idle-reap arithmetic).
        connected_since: Instant,
        /// The caller-supplied name, if any (an auto-generated id has no
        /// separate name — `None`).
        name: Option<String>,
        /// The target host (non-secret target addressing, D-31).
        host: String,
        /// Monotonic timestamp of the last observed activity (idle-reap,
        /// Plan 12-06).
        last_activity: Instant,
    },
    /// A possibly-still-live remote Windows session surfaced after a
    /// crash-restart (DAEMON-04) — never silently torn down; requires an
    /// explicit reclaim/teardown request.
    Orphaned {
        /// The target host (non-secret target addressing, D-31).
        host: String,
        /// ISO-8601 timestamp of when the session originally connected, as
        /// recorded in the on-disk reconciliation state (Plan 12-05) — a
        /// wall-clock string, not a monotonic `Instant`, since it survives
        /// the daemon's own process lifetime.
        connected_since: String,
    },
}

impl SessionEntry {
    /// Render this entry (plus its registry key) into the credential-free
    /// wire DTO `list` returns (D-30/D-31).
    ///
    /// `Live`'s `connected_since`/`last_activity` are intentionally `None`
    /// here: they are tracked internally as monotonic [`Instant`]s (for
    /// idle-reap arithmetic, Plan 12-06), which cannot be rendered as an
    /// ISO-8601 wall-clock string without a separate capture — the
    /// registry (Plan 12-03/12-04) is responsible for tracking and
    /// supplying that wall-clock string alongside the `Instant` once `list`
    /// is wired end-to-end.
    #[must_use]
    pub fn to_status(&self, id: &SessionId) -> SessionStatus {
        match self {
            SessionEntry::Connecting { .. } => SessionStatus {
                id: id.as_str().to_owned(),
                name: None,
                host: String::new(),
                status: SessionLifecycle::Connecting,
                connected_since: None,
                last_activity: None,
            },
            SessionEntry::Live { session, name, host, .. } => SessionStatus {
                id: id.as_str().to_owned(),
                name: name.clone(),
                host: host.clone(),
                status: session.describe(),
                connected_since: None,
                last_activity: None,
            },
            SessionEntry::Orphaned { host, connected_since } => SessionStatus {
                id: id.as_str().to_owned(),
                name: None,
                host: host.clone(),
                status: SessionLifecycle::Orphaned,
                connected_since: Some(connected_since.clone()),
                last_activity: None,
            },
        }
    }
}

/// This crate's error type (`#[non_exhaustive]` — a future variant is not a
/// breaking change for exhaustive matchers elsewhere in this crate).
///
/// `Display` MUST NOT embed a credential (D-31) — every message references
/// ids/hosts only. Mapped onto the wire `WireError` taxonomy by
/// [`crate::error_map`] (Plan 12-02 Task 3).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DaemonError {
    /// A `Connect` request's session name/id collided with an already-live
    /// or in-flight-connecting session (SESSION-04).
    #[error("session \"{0}\" already exists")]
    DuplicateSession(String),
    /// The requested session id has no corresponding entry in the registry.
    #[error("no such session \"{0}\"")]
    SessionNotFound(String),
    /// A disconnect was requested for a session whose connect is still in
    /// flight (research Pattern 2) — the in-flight connect is never
    /// interrupted.
    #[error("session \"{0}\" is still connecting")]
    StillConnecting(String),
    /// An underlying `rdpilot` SDK error. `rdpilot::Error`'s own `Display`
    /// is already external-detail-only (never embeds a credential), so
    /// this variant's message reuse is safe by construction.
    #[error(transparent)]
    Sdk(#[from] rdpilot::Error),
    /// A connect-path failure with no corresponding `rdpilot::Error` (e.g.
    /// a test fake's synthetic failure).
    #[error("connect failed: {0}")]
    Connect(String),
    /// A local I/O failure (e.g. reconciliation-state disk read/write,
    /// Plan 12-05).
    #[error("io error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLANTED_SECRET: &str = "RDPILOT-PLANTED-SECRET-SENTINEL";

    /// Compile-time assertion that `RealConnector` implements
    /// `SessionConnector` as a `dyn` trait object (the shape the registry
    /// actually stores it as, `Arc<dyn SessionConnector>`).
    fn _assert_real_connector_is_a_session_connector(_c: &dyn SessionConnector) {}

    #[test]
    fn real_connector_implements_session_connector() {
        let connector = RealConnector;
        _assert_real_connector_is_a_session_connector(&connector);
    }

    #[test]
    fn noop_reconciliation_sink_implements_reconciliation_sink() {
        fn assert_impl(_s: &dyn ReconciliationSink) {}
        assert_impl(&NoopReconciliationSink);
    }

    /// D-31: constructing every `DaemonError` variant with benign
    /// placeholder id/host messages (never the planted sentinel itself)
    /// and rendering `Display` never happens to surface the planted
    /// secret sentinel — a structural regression guard mirroring
    /// `rdpilot-ipc`'s CONFIG-03 planted-secret test: it passes because no
    /// `DaemonError` variant has a `password`/`credential`-shaped field at
    /// all (only id/host `String`s and the credential-free `Sdk` wrapper),
    /// not because the sentinel was traced through a running pipeline.
    #[test]
    fn no_daemon_error_variant_display_carries_the_planted_secret() {
        let cases: Vec<DaemonError> = vec![
            DaemonError::DuplicateSession("brave-otter".to_owned()),
            DaemonError::SessionNotFound("brave-otter".to_owned()),
            DaemonError::StillConnecting("brave-otter".to_owned()),
            DaemonError::Sdk(rdpilot::Error::Connect("host unreachable".to_owned())),
            DaemonError::Connect("host unreachable".to_owned()),
            DaemonError::Io("disk full".to_owned()),
        ];
        for err in cases {
            let rendered = format!("{err}");
            assert!(
                !rendered.contains(PLANTED_SECRET),
                "leak found in DaemonError Display: {rendered}"
            );
        }
    }

    /// `DaemonError::Sdk`'s message reuses `rdpilot::Error`'s own `Display`
    /// (guaranteed credential-free by that crate) — confirm the wrapper
    /// does not add anything that could embed a credential.
    #[test]
    fn sdk_error_display_reuses_the_inner_error_message_verbatim() {
        let inner = rdpilot::Error::Connect(PLANTED_SECRET.to_owned());
        let inner_message = inner.to_string();
        let err = DaemonError::Sdk(inner);
        assert_eq!(format!("{err}"), inner_message);
    }
}

