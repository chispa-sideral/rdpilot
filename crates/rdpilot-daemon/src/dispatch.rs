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
    Request, TransferOutcome as WireTransferOutcome, WireRecordingState, WireResponse,
    IPC_COMPATIBILITY_VERSION,
};

use crate::bundle::{Arch, BundleRequest};
use crate::diagnostics::{Diagnostics, Stage};
use crate::events::{CliCall, EventSource, RecordingTrigger, StopReason};
use crate::registry::{ConnectLease, Registry};
use crate::seams::{BundleSource, DaemonError};

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

/// The fields of `Request::Connect`.
pub(crate) struct ConnectFields {
    pub(crate) name: Option<String>,
    pub(crate) host: String,
    pub(crate) port: Option<u16>,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) domain: Option<String>,
    pub(crate) accept_invalid_certs: bool,
    pub(crate) cua_enabled: bool,
    pub(crate) cua_version: String,
    pub(crate) cua_auto_download: bool,
    pub(crate) connect_ack: bool,
    pub(crate) record: Option<RecordingTrigger>,
}

/// Open a session. With `cua_enabled`, `bundles` must supply a verified
/// bundle before the RDP connection starts; otherwise the connect fails
/// closed and no RDP session is opened. With `cua_enabled` off, `bundles`
/// is never asked.
pub(crate) async fn dispatch_connect(
    registry: &Registry,
    fields: ConnectFields,
    diagnostics: Option<&Diagnostics>,
    bundles: &dyn BundleSource,
) -> DispatchOutcome {
    let ConnectFields {
        name,
        host,
        port,
        username,
        password,
        domain,
        accept_invalid_certs,
        cua_enabled,
        cua_version,
        cua_auto_download,
        connect_ack,
        record,
    } = fields;
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
    // The bundle is daemon-local: callers select a Cua version but cannot
    // supply executable paths. It is prepared before the RDP logon, so a
    // missing or unverifiable component fails the connect closed. With
    // `cua_enabled` off no bundle is prepared and the session stays
    // native-only.
    let mut warnings = Vec::new();
    let bridge_configured = if cua_enabled {
        let request = BundleRequest {
            cua_version,
            auto_download: cua_auto_download,
            arch: Arch::X86_64,
        };
        match bundles.prepare(request).await {
            Ok(bundle) => {
                cfg = cfg.bundle_path(bundle.dir).bundle_id(bundle.bundle_id);
                for warning in &bundle.warnings {
                    eprintln!("rdpilot-daemon: warning: {warning}");
                }
                warnings = bundle.warnings;
                true
            }
            Err(e) => {
                return DispatchOutcome {
                    response: WireResponse::Error(DaemonError::Bundle(e.to_string()).into()),
                    connect_lease: None,
                }
            }
        }
    } else {
        false
    };
    let (lease, recording) = match registry.open_tracked(name, host, cfg, record).await {
        Ok(opened) => opened,
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
    DispatchOutcome {
        // A successful configured bootstrap has observed a bridge
        // pong. Surface that fact to clients so they do not infer
        // bridge readiness merely from an RDP connection.
        response: WireResponse::Connected {
            session,
            connect_ack_required: connect_ack,
            bridge_live: bridge_configured,
            recording: match recording {
                None => WireRecordingState::Off,
                Some(Ok(id)) => WireRecordingState::On { id },
                Some(Err(reason)) => WireRecordingState::Failed { reason },
            },
            warnings,
        },
        connect_lease: Some(lease),
    }
}

/// Dispatch one request for the IPC server. Connect is special: the returned
/// private lease is valid only until the matching Connected frame is written.
pub(crate) async fn dispatch_for_ipc(
    registry: &Registry,
    req: Request,
    diagnostics: Option<&Diagnostics>,
) -> DispatchOutcome {
    // Native verbs are recorded by name only (never coordinates, keys or
    // paths). Unknown or connecting sessions have no log and record nothing.
    let cli_call = native_verb(&req).and_then(|(session, name)| {
        registry
            .events(session)
            .map(|events| CliCall::start(events, name))
    });
    let response = match req {
        Request::Connect {
            name,
            host,
            port,
            username,
            password,
            domain,
            accept_invalid_certs,
            cua_enabled,
            cua_version,
            cua_auto_download,
            connect_ack,
            record,
        } => {
            return dispatch_connect(
                registry,
                ConnectFields {
                    name,
                    host,
                    port,
                    username,
                    password,
                    domain,
                    accept_invalid_certs,
                    cua_enabled,
                    cua_version,
                    cua_auto_download,
                    connect_ack,
                    record,
                },
                diagnostics,
                registry.bundle_source(),
            )
            .await;
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
            match registry
                .call_acting(&session, move |s| s.send_mouse(action))
                .await
            {
                Ok(()) => WireResponse::Ack,
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Key { session, action } => {
            match registry
                .call_acting(&session, move |s| s.send_key(action))
                .await
            {
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
        Request::ViewerStart { .. } => WireResponse::Error(
            DaemonError::Connect("ViewerStart requires a held IPC connection".into()).into(),
        ),
        Request::RecordStart { session } => {
            record_start(registry, &session, RecordingTrigger::Cli, EventSource::Cli).await
        }
        Request::RecordStop { session } => record_stop(registry, &session, EventSource::Cli),
        Request::Annotate { session, text } => {
            annotate(registry, &session, &text, EventSource::Cli)
        }
        Request::RecordingList {} => recording_list(registry).await,
        Request::RecordingKeep { id, keep } => recording_keep(registry, id, keep).await,
        Request::Takeover { session } => {
            match registry.takeover(&session, EventSource::Cli).await {
                Ok((previous, changed)) => WireResponse::TakenOver { previous, changed },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
    };
    if let Some(call) = cli_call {
        call.finish(!matches!(response, WireResponse::Error(_)));
    }
    DispatchOutcome {
        response,
        connect_lease: None,
    }
}

/// Start recording `session` now. Takes only the registry's brief outer
/// lock; never a per-session lock or an activity change.
pub(crate) async fn record_start(
    registry: &Registry,
    session: &rdpilot_ipc::SessionId,
    trigger: RecordingTrigger,
    source: EventSource,
) -> WireResponse {
    let target = match registry.recording_target(session) {
        Ok(target) => target,
        Err(e) => return WireResponse::Error(e.into()),
    };
    match registry.recordings().start(target, trigger, source).await {
        Ok(started) => WireResponse::RecordingChanged {
            message: if started.changed {
                format!("recording started ({})", started.id)
            } else {
                format!("already recording ({}); nothing changed", started.id)
            },
            id: Some(started.id),
            changed: started.changed,
        },
        Err(reason) => WireResponse::Error(DaemonError::Recording(reason).into()),
    }
}

/// Stop the recording of `session`; the session continues.
pub(crate) fn record_stop(
    registry: &Registry,
    session: &rdpilot_ipc::SessionId,
    source: EventSource,
) -> WireResponse {
    let target = match registry.recording_target(session) {
        Ok(target) => target,
        Err(e) => return WireResponse::Error(e.into()),
    };
    match registry
        .recordings()
        .stop(&target.events, source, StopReason::Requested)
    {
        Some(id) => WireResponse::RecordingChanged {
            message: format!("recording stopped ({id})"),
            id: Some(id),
            changed: true,
        },
        None => WireResponse::RecordingChanged {
            id: None,
            changed: false,
            message: "session is not recording; nothing changed".into(),
        },
    }
}

/// Add an annotation to the active recording of `session`.
pub(crate) fn annotate(
    registry: &Registry,
    session: &rdpilot_ipc::SessionId,
    text: &str,
    source: EventSource,
) -> WireResponse {
    let target = match registry.recording_target(session) {
        Ok(target) => target,
        Err(e) => return WireResponse::Error(e.into()),
    };
    match registry.recordings().annotate(&target.events, source, text) {
        Ok(id) => WireResponse::RecordingChanged {
            message: format!("annotation added to {id}"),
            id: Some(id),
            changed: true,
        },
        Err(reason) => WireResponse::Error(DaemonError::Recording(reason).into()),
    }
}

/// The recordings on disk (read on a blocking thread).
pub(crate) async fn recording_list(registry: &Registry) -> WireResponse {
    let recordings = std::sync::Arc::clone(registry.recordings());
    match tokio::task::spawn_blocking(move || recordings.list()).await {
        Ok(Ok(listing)) => WireResponse::Recordings {
            recordings: listing.recordings,
            kept_bytes: listing.kept_bytes,
            unkept_bytes: listing.unkept_bytes,
            budget_bytes: listing.budget_bytes,
            kept_over_budget: listing.kept_over_budget,
        },
        Ok(Err(reason)) => WireResponse::Error(DaemonError::Recording(reason).into()),
        Err(_) => {
            WireResponse::Error(DaemonError::Recording("recording list failed".into()).into())
        }
    }
}

/// Mark or unmark a recording keep (written on a blocking thread).
pub(crate) async fn recording_keep(registry: &Registry, id: String, keep: bool) -> WireResponse {
    let recordings = std::sync::Arc::clone(registry.recordings());
    let for_task = id.clone();
    match tokio::task::spawn_blocking(move || recordings.keep(&for_task, keep)).await {
        Ok(Ok(changed)) => WireResponse::RecordingChanged {
            message: match (keep, changed) {
                (true, true) => format!("{id} marked keep"),
                (false, true) => format!("{id} no longer kept"),
                (true, false) => format!("{id} is already kept; nothing changed"),
                (false, false) => format!("{id} is not kept; nothing changed"),
            },
            id: Some(id),
            changed,
        },
        Ok(Err(reason)) => WireResponse::Error(DaemonError::Recording(reason).into()),
        Err(_) => WireResponse::Error(DaemonError::Recording("keep failed".into()).into()),
    }
}

/// The session and recorded name of a native verb, or `None` for requests
/// that are not recorded.
fn native_verb(req: &Request) -> Option<(&rdpilot_ipc::SessionId, &'static str)> {
    match req {
        Request::Screenshot { session } => Some((session, "screenshot")),
        Request::Mouse { session, .. } => Some((session, "mouse")),
        Request::Key { session, .. } => Some((session, "key")),
        Request::DesktopSize { session } => Some((session, "desktop_size")),
        Request::Put { session, .. } => Some((session, "put")),
        Request::Get { session, .. } => Some((session, "get")),
        _ => None,
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

/// File -> env layered [`rdpilot_config::ResolvedConfig`] resolution
/// (`share_root` is not a wire field).
fn resolve_identity_config() -> Result<rdpilot_config::ResolvedConfig, DaemonError> {
    rdpilot_config::resolve().map_err(|e| DaemonError::Config(e.to_string()))
}

/// Encode a captured [`rdpilot::Screenshot`] as base64 PNG bytes (the
/// `WireResponse::Screenshot`
/// convention).
fn screenshot_to_base64(shot: &rdpilot::Screenshot) -> Result<String, DaemonError> {
    let png = shot.to_png().map_err(DaemonError::Sdk)?;
    Ok(BASE64_STANDARD.encode(png))
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

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
        deploys: Arc<AtomicUsize>,
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
            self.deploys.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(std::time::Duration::from_millis(0)) })
        }
    }

    /// A fake `SessionConnector` that always succeeds and records what it saw.
    #[derive(Default)]
    struct FakeConnector {
        deploys: Arc<AtomicUsize>,
        bundles: Arc<Mutex<Vec<Option<PathBuf>>>>,
    }

    impl SessionConnector for FakeConnector {
        fn connect(
            &self,
            cfg: ConnectionConfig,
        ) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            if let Ok(mut bundles) = self.bundles.lock() {
                bundles.push(cfg.get_bundle_path().map(std::path::Path::to_path_buf));
            }
            let deploys = self.deploys.clone();
            Box::pin(async move {
                Ok(Box::new(FakeSession {
                    closed: Arc::new(AtomicBool::new(false)),
                    deploys,
                }) as Box<dyn ManagedSession>)
            })
        }
    }

    fn test_registry() -> Registry {
        Registry::new(
            Arc::new(FakeConnector::default()),
            Arc::new(NoopReconciliationSink),
        )
    }

    fn connect_fields(cua_enabled: bool) -> ConnectFields {
        ConnectFields {
            name: Some("web".to_owned()),
            host: "10.0.0.5".to_owned(),
            port: None,
            username: "user".to_owned(),
            password: "pw".to_owned(),
            domain: None,
            accept_invalid_certs: false,
            cua_enabled,
            cua_version: "latest-dev".to_owned(),
            cua_auto_download: true,
            connect_ack: false,
            record: None,
        }
    }

    /// A bundle source that counts calls and answers with `result`.
    struct StubBundles {
        calls: Arc<AtomicUsize>,
        result: Result<crate::bundle::PreparedBundle, crate::bundle::BundleError>,
    }

    impl BundleSource for StubBundles {
        fn prepare(
            &self,
            _request: BundleRequest,
        ) -> BoxFuture<'_, Result<crate::bundle::PreparedBundle, crate::bundle::BundleError>>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let result = self.result.clone();
            Box::pin(async move { result })
        }
    }

    fn prepared() -> crate::bundle::PreparedBundle {
        crate::bundle::PreparedBundle {
            dir: PathBuf::from("/bundle"),
            bundle_id: "bundle-under-test".to_owned(),
            warnings: vec!["using cached Cua".to_owned()],
        }
    }

    struct ConnectRun {
        response: WireResponse,
        source_calls: usize,
        connects: usize,
        deploys: usize,
        bundles: Vec<Option<PathBuf>>,
    }

    /// Run `dispatch_connect` against a counting connector and bundle source.
    async fn run_connect(
        cua_enabled: bool,
        result: Result<crate::bundle::PreparedBundle, crate::bundle::BundleError>,
    ) -> ConnectRun {
        let connector = FakeConnector::default();
        let (deploys, bundles) = (connector.deploys.clone(), connector.bundles.clone());
        let registry = Registry::new(Arc::new(connector), Arc::new(NoopReconciliationSink));
        let calls = Arc::new(AtomicUsize::new(0));
        let source = StubBundles {
            calls: calls.clone(),
            result,
        };
        let outcome = dispatch_connect(&registry, connect_fields(cua_enabled), None, &source).await;
        let recorded = bundles.lock().map(|b| b.clone()).unwrap_or_default();
        ConnectRun {
            response: outcome.response,
            source_calls: calls.load(Ordering::SeqCst),
            connects: recorded.len(),
            deploys: deploys.load(Ordering::SeqCst),
            bundles: recorded,
        }
    }

    #[tokio::test]
    async fn cua_disabled_never_prepares_a_bundle_or_deploys_a_bridge() {
        let run = run_connect(false, Ok(prepared())).await;
        assert_eq!(run.source_calls, 0, "the bundle source must not run");
        assert_eq!(run.deploys, 0, "no bridge may be deployed");
        assert_eq!(run.bundles, vec![None], "cfg.bundle_path must stay unset");
        assert!(matches!(
            run.response,
            WireResponse::Connected {
                bridge_live: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn cua_enabled_deploys_the_prepared_bundle_and_returns_its_warnings() {
        let run = run_connect(true, Ok(prepared())).await;
        assert_eq!(run.source_calls, 1);
        assert_eq!(run.deploys, 1);
        assert_eq!(run.bundles, vec![Some(PathBuf::from("/bundle"))]);
        match run.response {
            WireResponse::Connected {
                bridge_live: true,
                warnings,
                ..
            } => assert_eq!(warnings, vec!["using cached Cua".to_owned()]),
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cua_enabled_without_a_bundle_fails_closed_before_any_rdp_session() {
        let error = crate::bundle::BundleError {
            component: crate::bundle::Component::CuaDriver,
            version: "latest-dev".to_owned(),
            arch: Arch::X86_64,
            cause: crate::bundle::Cause::Offline("connection refused".to_owned()),
        };
        let run = run_connect(true, Err(error)).await;
        assert_eq!(run.source_calls, 1);
        assert_eq!(run.connects, 0, "the RDP connector must never be called");
        assert_eq!(run.deploys, 0);
        match run.response {
            WireResponse::Error(WireError { code, message, .. }) => {
                assert_eq!(code, WireErrorCode::BundleUnavailable);
                assert!(
                    message.contains("Cua driver latest-dev (x86_64)"),
                    "{message}"
                );
                assert!(message.contains("CuaEnabled=no"), "{message}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_registry_without_a_bundle_source_fails_cua_connects_closed() {
        let registry = test_registry();
        let mut request = connect_request(Some("web"), "10.0.0.5");
        if let Request::Connect { cua_enabled, .. } = &mut request {
            *cua_enabled = true;
        }
        let response = dispatch(&registry, request).await;
        assert!(matches!(response, WireResponse::Error(_)), "{response:?}");
        assert_eq!(registry.len(), 0);
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
            cua_enabled: false,
            cua_version: "latest-dev".to_owned(),
            cua_auto_download: true,
            connect_ack: false,
            record: None,
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
        let action = rdpilot::MouseAction::Move { x: 1, y: 2 };
        let response = dispatch(&registry, Request::Mouse { session, action }).await;
        assert!(
            matches!(response, WireResponse::Ack),
            "expected Ack, got {response:?}"
        );
    }

    /// Take the human lease of `session` for a tab at 100.101.102.103.
    async fn human_take(registry: &Registry, session: &SessionId) {
        registry
            .control(session)
            .expect("live session")
            .take("100.101.102.103".parse().unwrap(), async {})
            .await
            .expect("take");
    }

    #[tokio::test]
    async fn native_input_during_a_human_lease_fails_with_the_takeover_command() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        human_take(&registry, &session).await;
        let mut command = String::new();
        for request in [
            Request::Key {
                session: session.clone(),
                action: rdpilot::KeyAction::Type("hi".into()),
            },
            Request::Mouse {
                session: session.clone(),
                action: rdpilot::MouseAction::Move { x: 1, y: 2 },
            },
        ] {
            match dispatch(&registry, request).await {
                WireResponse::Error(WireError {
                    code: WireErrorCode::HumanControl,
                    message,
                    controller: Some(controller),
                }) => {
                    assert_eq!(controller.kind, rdpilot_ipc::WireControllerKind::Human);
                    assert_eq!(controller.address.as_deref(), Some("100.101.102.103"));
                    assert!(controller.since.is_some());
                    assert!(message.starts_with(
                        "session \"web\" is controlled by human viewer 100.101.102.103 since "
                    ));
                    command = message
                        .rsplit_once("take over with: ")
                        .expect("names the command")
                        .1
                        .to_owned();
                }
                other => panic!("expected HumanControl, got {other:?}"),
            }
        }
        assert_eq!(command, "rdpilot takeover --session web");
        // Native reads keep working during the lease.
        for request in [
            Request::Screenshot {
                session: session.clone(),
            },
            Request::DesktopSize {
                session: session.clone(),
            },
            Request::List {},
        ] {
            assert!(!matches!(
                dispatch(&registry, request).await,
                WireResponse::Error(_)
            ));
        }
        let statuses = registry.list();
        assert_eq!(
            statuses[0].controller.as_ref().map(|c| c.kind),
            Some(rdpilot_ipc::WireControllerKind::Human)
        );
        // Running the printed command returns control; the same call works.
        match dispatch(
            &registry,
            Request::Takeover {
                session: session.clone(),
            },
        )
        .await
        {
            WireResponse::TakenOver { previous, changed } => {
                assert!(changed);
                assert_eq!(previous.address.as_deref(), Some("100.101.102.103"));
            }
            other => panic!("expected TakenOver, got {other:?}"),
        }
        let action = rdpilot::KeyAction::Type("hi".into());
        assert!(matches!(
            dispatch(&registry, Request::Key { session, action }).await,
            WireResponse::Ack
        ));
    }

    #[tokio::test]
    async fn takeover_is_a_no_op_under_agent_control_and_fails_for_an_unknown_session() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        assert!(matches!(
            dispatch(&registry, Request::Takeover { session }).await,
            WireResponse::TakenOver {
                changed: false,
                previous
            } if previous.kind == rdpilot_ipc::WireControllerKind::Agent
        ));
        let ghost = "ghost".parse().unwrap();
        assert!(matches!(
            dispatch(&registry, Request::Takeover { session: ghost }).await,
            WireResponse::Error(WireError {
                code: WireErrorCode::SessionNotFound,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn key_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let action = rdpilot::KeyAction::Type("hi".to_owned());
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
