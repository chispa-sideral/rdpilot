//! Dispatch: `rdpilot-ipc::Request` -> registry operations -> `WireResponse`
//! (Plan 12-04).
//!
//! [`dispatch`]'s `match` over `Request` is EXHAUSTIVE — no wildcard arm —
//! so a future wire verb added to `rdpilot-ipc::Request` without a
//! corresponding arm here is a compile error, not a silent runtime gap.
//!
//! CRITICAL (D-31): this module never logs the raw `Request` it is handed —
//! `Request::Connect` carries a plaintext password field. No
//! `println!`/`tracing`/`eprintln!` call anywhere in this file prints a
//! `req` value.

// `dispatch` is exercised by this file's own inline tests but is not yet
// CALLED from any non-test crate code — `ipc::mod::serve_connection`
// (Plan 12-04, same wave) calls it, but `serve_connection` itself is only
// wired into the accept loop by `server.rs` (Plan 12-06). Silence the
// resulting `dead_code` lint at the module level, mirroring `registry.rs`'s
// identical interface-first rationale.
#![allow(dead_code)]

use rdpilot::ConnectionConfig;
use rdpilot_ipc::{Request, SessionId, WireError, WireErrorCode, WireResponse};

use crate::registry::Registry;

/// Route one decoded `Request` to `registry` and produce the corresponding
/// `WireResponse`.
///
/// - `Connect` builds a `rdpilot::ConnectionConfig` from the request's
///   fields and calls `registry.open` (atomic claim-then-connect).
/// - `List` returns the registry's credential-free snapshot (SESSION-03).
/// - `Disconnect` calls `registry.close`.
/// - Every other (operational) verb is not wired to a live session yet in
///   this phase (research Open Question 3 — a full operational-verb
///   implementation is out of Plan 12-04's success criteria): an unknown
///   session resolves to `SessionNotFound`; a known session resolves to an
///   explicit `Internal` "not implemented in Phase 12" error, never a
///   silently-dropped request.
pub async fn dispatch(registry: &Registry, req: Request) -> WireResponse {
    match req {
        Request::Connect {
            name,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certs,
        } => {
            let mut cfg = ConnectionConfig::new(host.clone(), username, password).accept_invalid_certs(accept_invalid_certs);
            if let Some(port) = port {
                cfg = cfg.port(port);
            }
            if let Some(domain) = domain {
                cfg = cfg.domain(domain);
            }
            match registry.open(name, host, cfg).await {
                Ok(session) => WireResponse::Connected { session },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::List {} => WireResponse::SessionList { sessions: registry.list() },
        Request::Disconnect { session } => match registry.close(&session).await {
            Ok(()) => WireResponse::Ack,
            Err(e) => WireResponse::Error(e.into()),
        },
        // Deferred operational verbs (Phase 13/14 wire these against a
        // live `Session`): kept in an exhaustive match arm (not a
        // wildcard) so each is individually acknowledged here, per D-31 —
        // none of these branches inspects or logs `password`/credential
        // fields (none of these variants carry one).
        Request::Ping { session }
        | Request::Screenshot { session }
        | Request::LaunchProcess { session, .. }
        | Request::SetForeground { session, .. }
        | Request::Put { session, .. }
        | Request::Get { session, .. } => not_implemented_for(registry, &session),
    }
}

/// Resolve `session` against `registry` for an as-yet-unwired operational
/// verb: `SessionNotFound` if no such session exists, otherwise an
/// explicit `Internal` not-implemented error (never a silent no-op).
fn not_implemented_for(registry: &Registry, session: &SessionId) -> WireResponse {
    let exists = registry.list().iter().any(|s| s.id == session.as_str());
    if exists {
        WireResponse::Error(WireError {
            code: WireErrorCode::Internal,
            message: "operational verb not implemented in Phase 12".to_owned(),
        })
    } else {
        WireResponse::Error(WireError {
            code: WireErrorCode::SessionNotFound,
            message: format!("no such session \"{}\"", session.as_str()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rdpilot_ipc::SessionLifecycle;

    use super::*;
    use crate::seams::{DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector};

    type TestFuture<T> = Pin<Box<dyn Future<Output = T>>>;

    /// A fake, immediately-resolving `ManagedSession` — mirrors
    /// `registry.rs`'s own inline test fake (this module cannot reuse that
    /// one directly: it is private to `registry.rs`'s own `#[cfg(test)]
    /// mod tests`).
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

    /// A fake `SessionConnector` that always succeeds.
    struct FakeConnector;

    impl SessionConnector for FakeConnector {
        fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            Box::pin(async { Ok(Box::new(FakeSession { closed: Arc::new(AtomicBool::new(false)) }) as Box<dyn ManagedSession>) })
        }
    }

    fn test_registry() -> Registry {
        Registry::new(Arc::new(FakeConnector), Arc::new(NoopReconciliationSink))
    }

    fn connect_request(name: Option<&str>, host: &str) -> Request {
        Request::Connect {
            name: name.map(str::to_owned),
            host: host.to_owned(),
            port: None,
            username: "user".to_owned(),
            password: "pw".to_owned(),
            domain: None,
            accept_invalid_certs: false,
        }
    }

    #[tokio::test]
    async fn connect_dispatches_to_registry_open_and_returns_connected() {
        let registry = test_registry();
        let response = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        match response {
            WireResponse::Connected { session } => assert_eq!(session.as_str(), "web"),
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_duplicate_connect_returns_a_duplicate_session_error() {
        let registry = test_registry();
        let first = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        assert!(matches!(first, WireResponse::Connected { .. }));

        let second = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        match second {
            WireResponse::Error(WireError { code: WireErrorCode::DuplicateSession, .. }) => {}
            other => panic!("expected Error(DuplicateSession), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_returns_a_session_list_with_every_field_populated() {
        let registry = test_registry();
        dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;

        let response = dispatch(&registry, Request::List {}).await;
        match response {
            WireResponse::SessionList { sessions } => {
                assert_eq!(sessions.len(), 1);
                let s = &sessions[0];
                assert_eq!(s.id, "web");
                assert_eq!(s.name.as_deref(), Some("web"));
                assert_eq!(s.host, "10.0.0.5");
                assert_eq!(s.status, SessionLifecycle::Live);
                assert!(s.connected_since.is_some(), "connected_since must be populated (SESSION-03)");
                assert!(s.last_activity.is_some(), "last_activity must be populated (SESSION-03)");
            }
            other => panic!("expected SessionList, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn disconnect_closes_the_session_and_returns_ack() {
        let registry = test_registry();
        let connected = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        let WireResponse::Connected { session } = connected else {
            panic!("expected Connected");
        };

        let response = dispatch(&registry, Request::Disconnect { session }).await;
        assert!(matches!(response, WireResponse::Ack));
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn disconnect_of_an_unknown_session_returns_session_not_found() {
        let registry = test_registry();
        let session: SessionId = "ghost".parse().expect("non-empty literal");
        let response = dispatch(&registry, Request::Disconnect { session }).await;
        match response {
            WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, .. }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_operational_verb_for_an_unknown_session_returns_session_not_found() {
        let registry = test_registry();
        let session: SessionId = "ghost".parse().expect("non-empty literal");
        let response = dispatch(&registry, Request::Ping { session }).await;
        match response {
            WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, .. }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_operational_verb_for_a_known_session_returns_an_internal_not_implemented_error() {
        let registry = test_registry();
        let connected = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        let WireResponse::Connected { session } = connected else {
            panic!("expected Connected");
        };

        let response = dispatch(&registry, Request::Screenshot { session }).await;
        match response {
            WireResponse::Error(WireError { code: WireErrorCode::Internal, message }) => {
                assert!(message.contains("not implemented"), "message: {message}");
            }
            other => panic!("expected Error(Internal), got {other:?}"),
        }
    }
}
