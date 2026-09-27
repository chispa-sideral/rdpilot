//! Dispatch: `rdpilot-ipc::Request` -> registry operations -> `WireResponse`
//! (Plan 12-04; every operational verb wired to a live `rdpilot::Session`
//! method via `Registry::call` in Plan 13-04).
//!
//! [`dispatch`]'s `match` over `Request` is EXHAUSTIVE — no wildcard arm —
//! so a future wire verb added to `rdpilot-ipc::Request` without a
//! corresponding arm here is a compile error, not a silent runtime gap.
//!
//! CRITICAL (D-31): this module never logs the raw `Request` it is handed —
//! `Request::Connect` carries a plaintext password field. No
//! `println!`/`tracing`/`eprintln!` call anywhere in this file prints a
//! `req` value.

// `dispatch` is called from `ipc::serve_connection`'s accept loop
// (production) and exercised directly by this file's own inline tests.
// Several small wire<->SDK conversion helpers below are exercised only
// through `dispatch`'s operational arms (never called directly by a test),
// so the module-level allow stays rather than per-item annotation churn.
#![allow(dead_code)]

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use rdpilot::ConnectionConfig;
use rdpilot_ipc::{
    Request, TransferOutcome as WireTransferOutcome, WireButton, WireKey, WireKeyAction,
    WireMouseAction, WireResponse, IPC_COMPATIBILITY_VERSION,
};

use crate::diagnostics::{Diagnostics, Stage};
use crate::registry::{ConnectLease, Registry};
use crate::seams::DaemonError;

/// Response plus a private cleanup handle retained only by the IPC server
/// until the response has crossed the local transport.
pub(crate) struct DispatchOutcome {
    pub(crate) response: WireResponse,
    pub(crate) connect_lease: Option<ConnectLease>,
}

/// Route one decoded `Request` to `registry` and produce the corresponding
/// `WireResponse`.
///
/// - `Connect` builds a `rdpilot::ConnectionConfig` from the request's
///   fields, sources the daemon-local `share_root` file-transfer staging
///   path from `rdpilot-config` (Plan 13-04, research Pitfall 6 — the
///   pre-existing Phase 12 gap where `Connect` never set `share_root`), and
///   calls `registry.open` (atomic claim-then-connect).
/// - `List` returns the registry's credential-free snapshot (SESSION-03).
/// - `Disconnect` calls `registry.close`.
///
/// Native recovery and transfer operations use a short registry call.
/// CuaAttach is handled by the authenticated IPC stream upgrade.
pub async fn dispatch(registry: &Registry, req: Request) -> WireResponse {
    dispatch_for_ipc(registry, req, None).await.response
}

/// Dispatch one request for the IPC server. Connect is special: the returned
/// private lease is valid only until the matching Connected frame is written.
pub(crate) async fn dispatch_for_ipc(
    registry: &Registry,
    req: Request,
    diagnostics: Option<&Diagnostics>,
) -> DispatchOutcome {
    let response = match req {
        Request::Connect {
            name,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certs,
            connect_ack,
        } => {
            let mut cfg = ConnectionConfig::new(host.clone(), username, password)
                .accept_invalid_certs(accept_invalid_certs);
            if let Some(port) = port {
                cfg = cfg.port(port);
            }
            if let Some(domain) = domain {
                cfg = cfg.domain(domain);
            }
            // Source the daemon-local file-transfer staging root
            // (research Pitfall 6): NOT a wire field — `share_root` is
            // daemon-local operational config, never dictated per-Connect
            // by a client. host/username/password/domain/
            // accept_invalid_certs above already arrived resolved on the
            // wire; only this one value is resolved here.
            let share_root = match resolve_share_root() {
                Ok(path) => path,
                Err(e) => {
                    return DispatchOutcome {
                        response: WireResponse::Error(e.into()),
                        connect_lease: None,
                    }
                }
            };
            cfg = cfg.share_root(share_root);
            // The bundle is daemon-local: callers cannot supply arbitrary executable paths.
            let bridge_configured = match resolve_bundle_path() {
                Ok(Some(bundle_path)) => {
                    cfg = cfg.bundle_path(bundle_path);
                    true
                }
                Ok(None) => false,
                Err(e) => {
                    return DispatchOutcome {
                        response: WireResponse::Error(e.into()),
                        connect_lease: None,
                    }
                }
            };
            let lease = match registry.open_tracked(name, host, cfg).await {
                Ok(lease) => lease,
                Err(e) => {
                    return DispatchOutcome {
                        response: WireResponse::Error(e.into()),
                        connect_lease: None,
                    }
                }
            };
            let session = lease.id.clone();
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(session.as_str(), Stage::RegistryOpened);
            }
            // A configured bundle must become ready before Connect succeeds.
            if bridge_configured {
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(session.as_str(), Stage::BridgeBootstrapStarted);
                }
                let bootstrap = registry.call(&session, |s| s.deploy_and_launch()).await;
                let bootstrap_stages = registry
                    .call(&session, |s| {
                        Box::pin(async move { Ok(s.bootstrap_stages()) })
                    })
                    .await
                    .unwrap_or_default();
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record_bootstrap_stages(session.as_str(), &bootstrap_stages);
                }
                if let Err(e) = bootstrap {
                    if matches!(registry.close_if_generation(&lease).await, Ok(true)) {
                        if let Some(diagnostics) = diagnostics {
                            diagnostics.record(session.as_str(), Stage::RegistryClosed);
                        }
                    }
                    return DispatchOutcome {
                        response: WireResponse::Error(e.into()),
                        connect_lease: None,
                    };
                }
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(session.as_str(), Stage::BridgeBootstrapFinished);
                }
            }
            return DispatchOutcome {
                // A successful configured bootstrap has observed a bridge
                // pong. Surface that fact to clients so they do not infer
                // bridge readiness merely from an RDP connection.
                response: WireResponse::Connected {
                    session,
                    connect_ack_required: connect_ack,
                    bridge_live: bridge_configured,
                },
                connect_lease: Some(lease),
            };
        }
        Request::ConnectAck { .. } => WireResponse::Error(
            DaemonError::Connect("ConnectAck is only valid immediately after Connect".to_owned())
                .into(),
        ),
        Request::List {} => WireResponse::SessionList {
            sessions: registry.list(),
            compatibility_version: Some(IPC_COMPATIBILITY_VERSION),
        },
        Request::Disconnect { session } => match registry.close(&session).await {
            Ok(()) => WireResponse::Ack,
            Err(e) => WireResponse::Error(e.into()),
        },
        Request::Ping { session } => match registry.call(&session, |s| s.ping()).await {
            Ok(_) => WireResponse::Ack,
            Err(e) => WireResponse::Error(e.into()),
        },
        Request::Screenshot { session } => {
            match registry.call(&session, |s| s.screenshot()).await {
                Ok(shot) => match screenshot_to_base64(&shot) {
                    Ok(png_base64) => WireResponse::Screenshot { png_base64 },
                    Err(e) => WireResponse::Error(e.into()),
                },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Put {
            session,
            local_path,
            remote_name,
        } => {
            match registry
                .call(&session, move |s| {
                    s.upload_file(PathBuf::from(local_path), remote_name)
                })
                .await
            {
                Ok(outcome) => WireResponse::Transfer(wire_transfer_outcome(outcome)),
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Get {
            session,
            remote_name,
            local_path,
        } => {
            match registry
                .call(&session, move |s| {
                    s.download_file(remote_name, PathBuf::from(local_path))
                })
                .await
            {
                Ok(outcome) => WireResponse::Transfer(wire_transfer_outcome(outcome)),
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Mouse { session, action } => {
            let action = sdk_mouse_action(action);
            match registry.call(&session, move |s| s.send_mouse(action)).await {
                Ok(()) => WireResponse::Ack,
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Key { session, action } => {
            let action = sdk_key_action(action);
            match registry.call(&session, move |s| s.send_key(action)).await {
                Ok(()) => WireResponse::Ack,
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::DesktopSize { session } => {
            match registry
                .call(&session, |s| Box::pin(async move { Ok(s.desktop_size()) }))
                .await
            {
                Ok((w, h)) => WireResponse::DesktopSize {
                    width: w as u16,
                    height: h as u16,
                },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::CuaAttach { .. } => WireResponse::Error(
            DaemonError::Connect("CuaAttach requires a stream upgrade".into()).into(),
        ),
    };
    DispatchOutcome {
        response,
        connect_lease: None,
    }
}

/// Resolve the daemon-local file-transfer staging root (research Pitfall 6)
/// via `rdpilot-config`'s file -> env layering (no flag/MCP-init override
/// layer exists at the daemon: `share_root` is never a wire field, so the
/// override layer passed to `rdpilot_config::resolve` is always the
/// all-`None` identity value).
fn resolve_share_root() -> Result<PathBuf, DaemonError> {
    let resolved = resolve_identity_config()?;
    Ok(rdpilot_config::share_root_or_default(&resolved))
}

/// Resolve the daemon-local bridge executable path (Plan 15-06 live-fix),
/// via the identical file -> env layering as [`resolve_share_root`]. `Ok(None)`
/// means no bridge path is configured -- a legitimate session-management-only
/// mode, not an error.
fn resolve_bundle_path() -> Result<Option<PathBuf>, DaemonError> {
    let resolved = resolve_identity_config()?;
    Ok(resolved.bundle_path.map(PathBuf::from))
}

/// Shared file -> env layered [`rdpilot_config::ResolvedConfig`] resolution
/// (no flag/MCP-init override layer exists at the daemon for either
/// `share_root` or `bundle_path`: neither is a wire field, so the
/// override layer passed to `rdpilot_config::resolve` is always the
/// all-`None` identity value). Factored out so [`resolve_share_root`] and
/// [`resolve_bundle_path`] share one resolution call rather than two
/// independent (and potentially divergent) reads of the same underlying
/// config layers.
fn resolve_identity_config() -> Result<rdpilot_config::ResolvedConfig, DaemonError> {
    let identity_overrides = rdpilot_config::ResolvedConfig {
        host: None,
        port: None,
        username: None,
        password: None,
        domain: None,
        accept_invalid_certs: false,
        share_root: None,
        bundle_path: None,
    };
    rdpilot_config::resolve(identity_overrides).map_err(|e| DaemonError::Config(e.to_string()))
}

/// Encode a captured [`rdpilot::Screenshot`] as base64 PNG bytes (the
/// `WireResponse::Screenshot`
/// convention).
fn screenshot_to_base64(shot: &rdpilot::Screenshot) -> Result<String, DaemonError> {
    let png = shot.to_png().map_err(DaemonError::Sdk)?;
    Ok(BASE64_STANDARD.encode(png))
}

/// Wire `WireButton` -> SDK `rdpilot::Button`.
fn sdk_button(b: WireButton) -> rdpilot::Button {
    match b {
        WireButton::Left => rdpilot::Button::Left,
        WireButton::Right => rdpilot::Button::Right,
        WireButton::Middle => rdpilot::Button::Middle,
    }
}

/// Wire `WireMouseAction` -> SDK `rdpilot::MouseAction`.
fn sdk_mouse_action(a: WireMouseAction) -> rdpilot::MouseAction {
    match a {
        WireMouseAction::Move { x, y } => rdpilot::MouseAction::Move { x, y },
        WireMouseAction::Click { x, y, button } => rdpilot::MouseAction::Click {
            x,
            y,
            button: sdk_button(button),
        },
        WireMouseAction::DoubleClick { x, y, button } => rdpilot::MouseAction::DoubleClick {
            x,
            y,
            button: sdk_button(button),
        },
        WireMouseAction::Scroll { x, y, dy } => rdpilot::MouseAction::Scroll { x, y, dy },
        WireMouseAction::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
            button,
        } => rdpilot::MouseAction::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
            button: sdk_button(button),
        },
    }
}

/// Wire `WireKey` -> SDK `rdpilot::Key` (full 1:1 67-variant match — see
/// `rdpilot-ipc::input`'s own doc comment for the exact variant-count
/// provenance).
fn sdk_key(k: WireKey) -> rdpilot::Key {
    match k {
        WireKey::Ctrl => rdpilot::Key::Ctrl,
        WireKey::Alt => rdpilot::Key::Alt,
        WireKey::Shift => rdpilot::Key::Shift,
        WireKey::A => rdpilot::Key::A,
        WireKey::B => rdpilot::Key::B,
        WireKey::C => rdpilot::Key::C,
        WireKey::D => rdpilot::Key::D,
        WireKey::E => rdpilot::Key::E,
        WireKey::F => rdpilot::Key::F,
        WireKey::G => rdpilot::Key::G,
        WireKey::H => rdpilot::Key::H,
        WireKey::I => rdpilot::Key::I,
        WireKey::J => rdpilot::Key::J,
        WireKey::K => rdpilot::Key::K,
        WireKey::L => rdpilot::Key::L,
        WireKey::M => rdpilot::Key::M,
        WireKey::N => rdpilot::Key::N,
        WireKey::O => rdpilot::Key::O,
        WireKey::P => rdpilot::Key::P,
        WireKey::Q => rdpilot::Key::Q,
        WireKey::R => rdpilot::Key::R,
        WireKey::S => rdpilot::Key::S,
        WireKey::T => rdpilot::Key::T,
        WireKey::U => rdpilot::Key::U,
        WireKey::V => rdpilot::Key::V,
        WireKey::W => rdpilot::Key::W,
        WireKey::X => rdpilot::Key::X,
        WireKey::Y => rdpilot::Key::Y,
        WireKey::Z => rdpilot::Key::Z,
        WireKey::Digit0 => rdpilot::Key::Digit0,
        WireKey::Digit1 => rdpilot::Key::Digit1,
        WireKey::Digit2 => rdpilot::Key::Digit2,
        WireKey::Digit3 => rdpilot::Key::Digit3,
        WireKey::Digit4 => rdpilot::Key::Digit4,
        WireKey::Digit5 => rdpilot::Key::Digit5,
        WireKey::Digit6 => rdpilot::Key::Digit6,
        WireKey::Digit7 => rdpilot::Key::Digit7,
        WireKey::Digit8 => rdpilot::Key::Digit8,
        WireKey::Digit9 => rdpilot::Key::Digit9,
        WireKey::F1 => rdpilot::Key::F1,
        WireKey::F2 => rdpilot::Key::F2,
        WireKey::F3 => rdpilot::Key::F3,
        WireKey::F4 => rdpilot::Key::F4,
        WireKey::F5 => rdpilot::Key::F5,
        WireKey::F6 => rdpilot::Key::F6,
        WireKey::F7 => rdpilot::Key::F7,
        WireKey::F8 => rdpilot::Key::F8,
        WireKey::F9 => rdpilot::Key::F9,
        WireKey::F10 => rdpilot::Key::F10,
        WireKey::F11 => rdpilot::Key::F11,
        WireKey::F12 => rdpilot::Key::F12,
        WireKey::Enter => rdpilot::Key::Enter,
        WireKey::Esc => rdpilot::Key::Esc,
        WireKey::Tab => rdpilot::Key::Tab,
        WireKey::Space => rdpilot::Key::Space,
        WireKey::Backspace => rdpilot::Key::Backspace,
        WireKey::Delete => rdpilot::Key::Delete,
        WireKey::Up => rdpilot::Key::Up,
        WireKey::Down => rdpilot::Key::Down,
        WireKey::Left => rdpilot::Key::Left,
        WireKey::Right => rdpilot::Key::Right,
        WireKey::Home => rdpilot::Key::Home,
        WireKey::End => rdpilot::Key::End,
        WireKey::PageUp => rdpilot::Key::PageUp,
        WireKey::PageDown => rdpilot::Key::PageDown,
        WireKey::Insert => rdpilot::Key::Insert,
        WireKey::Win => rdpilot::Key::Win,
    }
}

/// Wire `WireKeyAction` -> SDK `rdpilot::KeyAction`.
fn sdk_key_action(a: WireKeyAction) -> rdpilot::KeyAction {
    match a {
        WireKeyAction::Type(s) => rdpilot::KeyAction::Type(s),
        WireKeyAction::Combo(keys) => {
            rdpilot::KeyAction::Combo(keys.into_iter().map(sdk_key).collect())
        }
    }
}

/// SDK `rdpilot::TransferOutcome` -> wire mirror `rdpilot_ipc::TransferOutcome`.
fn wire_transfer_outcome(o: rdpilot::TransferOutcome) -> WireTransferOutcome {
    WireTransferOutcome {
        bytes_transferred: o.bytes_transferred,
        checksum: o.checksum,
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use rdpilot_ipc::{SessionId, SessionLifecycle, WireError, WireErrorCode};

    use super::*;
    use crate::seams::{
        BoxFuture, DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector,
    };

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

    /// A fake `SessionConnector` that always succeeds.
    struct FakeConnector;

    impl SessionConnector for FakeConnector {
        fn connect(
            &self,
            _cfg: ConnectionConfig,
        ) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            Box::pin(async {
                Ok(Box::new(FakeSession {
                    closed: Arc::new(AtomicBool::new(false)),
                }) as Box<dyn ManagedSession>)
            })
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
            connect_ack: false,
        }
    }

    #[tokio::test]
    async fn connect_reports_bridge_liveness_matching_the_resolved_configuration() {
        let registry = test_registry();
        let response = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        let expected_bridge_live = resolve_bundle_path()
            .expect("test configuration resolves")
            .is_some();
        match response {
            WireResponse::Connected {
                session,
                bridge_live,
                ..
            } => {
                assert_eq!(session.as_str(), "web");
                assert_eq!(
                    bridge_live, expected_bridge_live,
                    "the Connect response must distinguish a verified configured bridge from RDP-only mode"
                );
            }
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
            WireResponse::Error(WireError {
                code: WireErrorCode::DuplicateSession,
                ..
            }) => {}
            other => panic!("expected Error(DuplicateSession), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_returns_a_session_list_with_every_field_populated() {
        let registry = test_registry();
        dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;

        let response = dispatch(&registry, Request::List {}).await;
        match response {
            WireResponse::SessionList {
                sessions,
                compatibility_version,
            } => {
                assert_eq!(compatibility_version, Some(IPC_COMPATIBILITY_VERSION));
                assert_eq!(sessions.len(), 1);
                let s = &sessions[0];
                assert_eq!(s.id, "web");
                assert_eq!(s.name.as_deref(), Some("web"));
                assert_eq!(s.host, "10.0.0.5");
                assert_eq!(s.status, SessionLifecycle::Live);
                assert!(
                    s.connected_since.is_some(),
                    "connected_since must be populated (SESSION-03)"
                );
                assert!(
                    s.last_activity.is_some(),
                    "last_activity must be populated (SESSION-03)"
                );
            }
            other => panic!("expected SessionList, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn disconnect_closes_the_session_and_returns_ack() {
        let registry = test_registry();
        let connected = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        let WireResponse::Connected { session, .. } = connected else {
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
            WireResponse::Error(WireError {
                code: WireErrorCode::SessionNotFound,
                ..
            }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_operational_verb_for_an_unknown_session_returns_session_not_found() {
        let registry = test_registry();
        let session: SessionId = "ghost".parse().expect("non-empty literal");
        let response = dispatch(&registry, Request::Ping { session }).await;
        match response {
            WireResponse::Error(WireError {
                code: WireErrorCode::SessionNotFound,
                ..
            }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }

    /// Connects a fresh `FakeSession`-backed session and returns its
    /// `SessionId`, for the per-verb dispatch tests below.
    async fn connected_session(registry: &Registry) -> SessionId {
        let connected = dispatch(registry, connect_request(Some("web"), "10.0.0.5")).await;
        let WireResponse::Connected { session, .. } = connected else {
            panic!("expected Connected, got {connected:?}");
        };
        session
    }

    // --- Task 3: every operational verb routes through registry.call to a
    // live (fake, in these offline tests) session and returns its expected
    // WireResponse variant -- proving the "not implemented" arms are truly
    // gone, not just that the match still compiles.

    #[tokio::test]
    async fn ping_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::Ping { session }).await;
        assert!(
            matches!(response, WireResponse::Ack),
            "expected Ack, got {response:?}"
        );
    }

    #[tokio::test]
    async fn screenshot_returns_a_response_whose_png_base64_decodes_to_valid_png_bytes() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::Screenshot { session }).await;
        let WireResponse::Screenshot { png_base64 } = response else {
            panic!("expected Screenshot, got {response:?}");
        };
        let bytes = BASE64_STANDARD.decode(png_base64).expect("valid base64");
        assert_eq!(
            &bytes[0..4],
            &[0x89, b'P', b'N', b'G'],
            "decoded bytes must be a PNG"
        );
    }

    #[tokio::test]
    async fn mouse_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let action = WireMouseAction::Move { x: 1, y: 2 };
        let response = dispatch(&registry, Request::Mouse { session, action }).await;
        assert!(
            matches!(response, WireResponse::Ack),
            "expected Ack, got {response:?}"
        );
    }

    #[tokio::test]
    async fn key_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let action = WireKeyAction::Type("hi".to_owned());
        let response = dispatch(&registry, Request::Key { session, action }).await;
        assert!(
            matches!(response, WireResponse::Ack),
            "expected Ack, got {response:?}"
        );
    }

    #[tokio::test]
    async fn put_returns_a_transfer_response() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(
            &registry,
            Request::Put {
                session,
                local_path: "/tmp/a".to_owned(),
                remote_name: "a".to_owned(),
            },
        )
        .await;
        assert!(
            matches!(response, WireResponse::Transfer(_)),
            "expected Transfer, got {response:?}"
        );
    }

    #[tokio::test]
    async fn get_returns_a_transfer_response() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(
            &registry,
            Request::Get {
                session,
                remote_name: "a".to_owned(),
                local_path: "/tmp/a".to_owned(),
            },
        )
        .await;
        assert!(
            matches!(response, WireResponse::Transfer(_)),
            "expected Transfer, got {response:?}"
        );
    }

    #[tokio::test]
    async fn desktop_size_returns_the_fakes_dimensions() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::DesktopSize { session }).await;
        match response {
            WireResponse::DesktopSize { width, height } => {
                assert_eq!(width, 1920);
                assert_eq!(height, 1080);
            }
            other => panic!("expected DesktopSize, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn desktop_size_for_an_unknown_session_returns_session_not_found() {
        let registry = test_registry();
        let session: SessionId = "ghost".parse().expect("non-empty literal");
        let response = dispatch(&registry, Request::DesktopSize { session }).await;
        match response {
            WireResponse::Error(WireError {
                code: WireErrorCode::SessionNotFound,
                ..
            }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }
}
