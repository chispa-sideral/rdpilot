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
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use rdpilot::ConnectionConfig;
use rdpilot_ipc::{
    Request, TransferOutcome as WireTransferOutcome, WireButton, WireKey, WireKeyAction, WireMouseAction,
    WireProcessInfo, WireRect, WireResponse, WireUacDecision, WireUiaElement, WireUiaMode, WireUiaScope,
    WireWindowInfo, WireWindowState, WireWorldStateOptions,
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
/// - Every operational verb (`Ping`/`Screenshot`/`WindowList`/
///   `ProcessList`/`Uia`/`WorldState`/`Mouse`/`Key`/`LaunchProcess`/
///   `SetForeground`/`Put`/`Get`/`UacRespond`) routes through `registry.call`
///   to the live session via the `ManagedSession` seam (Plan 13-03),
///   converting wire DTOs (Plan 13-02) to/from the corresponding SDK types
///   (Plan 13-04).
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
            let mut cfg = ConnectionConfig::new(host.clone(), username, password).accept_invalid_certs(accept_invalid_certs);
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
                Err(e) => return DispatchOutcome { response: WireResponse::Error(e.into()), connect_lease: None },
            };
            cfg = cfg.share_root(share_root);
            // Live-diagnosed (Plan 15-06): the sensor binary path is the
            // SAME kind of daemon-local operational config as ‘share_root’
            // above -- it was previously never sourced at all, meaning
            // ConnectionConfig::sensor_binary_path was never set on any
            // real Connect (the RDPDR channel was never registered, and
            // share_root's own staging directory -- created only when a
            // sensor path is configured, connect.rs -- was never created
            // either). None (unconfigured) is preserved as a legitimate
            // session-management-only mode; only a Some resolved value
            // is wired onto cfg.
            let sensor_configured = match resolve_sensor_binary_path() {
                Ok(Some(sensor_binary_path)) => {
                    cfg = cfg.sensor_binary_path(sensor_binary_path);
                    true
                }
                Ok(None) => false,
                Err(e) => return DispatchOutcome { response: WireResponse::Error(e.into()), connect_lease: None },
            };
            let lease = match registry.open_tracked(name, host, cfg).await {
                Ok(lease) => lease,
                Err(e) => return DispatchOutcome { response: WireResponse::Error(e.into()), connect_lease: None },
            };
            let session = lease.id.clone();
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(session.as_str(), Stage::RegistryOpened);
            }
            // Live-diagnosed (Plan 15-06): every `rdpilot`-crate live test
            // calls `deploy_and_launch` explicitly right after connect,
            // before any sensor-mediated request -- this daemon path never
            // did, so the remote sensor was never started and every
            // subsequent perception/input/launch/put/get call timed out
            // waiting on a DVC channel with nothing listening. Only
            // attempted when a sensor path was actually configured above
            // (a sensor-less session has nothing to deploy). A failure here
            // closes the just-opened session and surfaces as a Connect
            // failure -- a caller that configured a sensor path expects
            // sensor-backed verbs to work; a silent partial success would
            // be more confusing than a clear upfront error.
            if sensor_configured {
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(session.as_str(), Stage::SensorBootstrapStarted);
                }
                let bootstrap = registry.call(&session, |s| s.deploy_and_launch()).await;
                let bootstrap_stages = registry
                    .call(&session, |s| Box::pin(async move { Ok(s.bootstrap_stages()) }))
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
                    return DispatchOutcome { response: WireResponse::Error(e.into()), connect_lease: None };
                }
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(session.as_str(), Stage::SensorBootstrapFinished);
                }
            }
            return DispatchOutcome {
                // A successful configured bootstrap has observed a sensor
                // pong. Surface that fact to clients so they do not infer
                // sensor readiness merely from an RDP connection.
                response: WireResponse::Connected {
                    session,
                    connect_ack_required: connect_ack,
                    sensor_live: sensor_configured,
                },
                connect_lease: Some(lease),
            };
        }
        Request::ConnectAck { .. } => WireResponse::Error(DaemonError::Connect(
            "ConnectAck is only valid immediately after Connect".to_owned(),
        ).into()),
        Request::List {} => WireResponse::SessionList { sessions: registry.list() },
        Request::Disconnect { session } => match registry.close(&session).await {
            Ok(()) => WireResponse::Ack,
            Err(e) => WireResponse::Error(e.into()),
        },
        Request::Ping { session } => match registry.call(&session, |s| s.ping()).await {
            Ok(_) => WireResponse::Ack,
            Err(e) => WireResponse::Error(e.into()),
        },
        Request::Screenshot { session } => match registry.call(&session, |s| s.screenshot()).await {
            Ok(shot) => match screenshot_to_base64(&shot) {
                Ok(png_base64) => WireResponse::Screenshot { png_base64 },
                Err(e) => WireResponse::Error(e.into()),
            },
            Err(e) => WireResponse::Error(e.into()),
        },
        Request::LaunchProcess { session, exe, args, cwd } => {
            match registry.call(&session, move |s| s.launch_process(exe, args, cwd)).await {
                Ok(pid) => WireResponse::Pid { pid },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::SetForeground { session, hwnd } => {
            match registry.call(&session, move |s| s.set_foreground_window(hwnd)).await {
                Ok(()) => WireResponse::Ack,
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Put { session, local_path, remote_name } => {
            match registry
                .call(&session, move |s| s.upload_file(PathBuf::from(local_path), remote_name))
                .await
            {
                Ok(outcome) => WireResponse::Transfer(wire_transfer_outcome(outcome)),
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Get { session, remote_name, local_path } => {
            match registry
                .call(&session, move |s| s.download_file(remote_name, PathBuf::from(local_path)))
                .await
            {
                Ok(outcome) => WireResponse::Transfer(wire_transfer_outcome(outcome)),
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::WindowList { session } => match registry.call(&session, |s| s.get_window_list()).await {
            Ok(windows) => WireResponse::WindowList { windows: windows.into_iter().map(wire_window_info).collect() },
            Err(e) => WireResponse::Error(e.into()),
        },
        Request::ProcessList { session } => {
            // Session-scoped elevation detection piggybacks on the same
            // process-tree fetch every `ProcessList` request already
            // makes -- zero extra sensor cost. `own_session_id()` must be
            // read from the SAME
            // `ManagedSession` inside the SAME `registry.call` closure as
            // `get_process_tree()`, since it is only ever populated as a
            // side effect of that exact round trip.
            match registry
                .call(&session, |s| {
                    Box::pin(async move {
                        let processes = s.get_process_tree().await?;
                        let own_session_id = s.own_session_id();
                        Ok((processes, own_session_id))
                    })
                })
                .await
            {
                Ok((processes, own_session_id)) => {
                    let elevation_active = rdpilot::perception::elevation_prompt_active(&processes, own_session_id);
                    WireResponse::ProcessList {
                        processes: processes.into_iter().map(wire_process_info).collect(),
                        elevation_active,
                    }
                }
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::Uia { session, hwnd, scope } => {
            let scope = sdk_uia_scope(scope);
            match registry.call(&session, move |s| s.get_uia_tree(hwnd, scope)).await {
                Ok(elements) => WireResponse::Uia { elements: elements.into_iter().map(wire_uia_element).collect() },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::WorldState { session, options } => {
            let opts = sdk_world_state_options(options);
            match registry.call(&session, move |s| s.world_state(opts)).await {
                Ok(state) => match wire_world_state(state) {
                    Ok(response) => response,
                    Err(e) => WireResponse::Error(e.into()),
                },
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
            match registry.call(&session, |s| Box::pin(async move { Ok(s.desktop_size()) })).await {
                Ok((w, h)) => WireResponse::DesktopSize { width: w as u16, height: h as u16 },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
        Request::UacRespond { session, decision } => {
            let decision = sdk_uac_decision(decision);
            match registry.call(&session, move |s| s.uac_respond(decision)).await {
                Ok(outcome) => match screenshot_to_base64(&outcome.confirmation) {
                    Ok(confirmation_png_base64) => {
                        WireResponse::UacRespond { decision: wire_uac_decision(outcome.decision), confirmation_png_base64 }
                    }
                    Err(e) => WireResponse::Error(e.into()),
                },
                Err(e) => WireResponse::Error(e.into()),
            }
        }
    };
    DispatchOutcome { response, connect_lease: None }
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

/// Resolve the daemon-local sensor executable path (Plan 15-06 live-fix),
/// via the identical file -> env layering as [`resolve_share_root`]. `Ok(None)`
/// means no sensor path is configured -- a legitimate session-management-only
/// mode, not an error.
fn resolve_sensor_binary_path() -> Result<Option<PathBuf>, DaemonError> {
    let resolved = resolve_identity_config()?;
    Ok(resolved.sensor_binary_path.map(PathBuf::from))
}

/// Shared file -> env layered [`rdpilot_config::ResolvedConfig`] resolution
/// (no flag/MCP-init override layer exists at the daemon for either
/// `share_root` or `sensor_binary_path`: neither is a wire field, so the
/// override layer passed to `rdpilot_config::resolve` is always the
/// all-`None` identity value). Factored out so [`resolve_share_root`] and
/// [`resolve_sensor_binary_path`] share one resolution call rather than two
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
        sensor_binary_path: None,
    };
    rdpilot_config::resolve(identity_overrides).map_err(|e| DaemonError::Config(e.to_string()))
}

/// Encode a captured [`rdpilot::Screenshot`] as base64 PNG bytes (the
/// `WireResponse::Screenshot`/`WireResponse::WorldState.screenshot`
/// convention).
fn screenshot_to_base64(shot: &rdpilot::Screenshot) -> Result<String, DaemonError> {
    let png = shot.to_png().map_err(DaemonError::Sdk)?;
    Ok(BASE64_STANDARD.encode(png))
}

/// Convert a completed [`rdpilot::WorldState`] into its
/// `WireResponse::WorldState` wire mirror: `SystemTime`/`Duration` (no
/// serde impl) are converted here, never in `rdpilot-ipc` (its own doc
/// comment), reusing `registry.rs`'s existing ISO-8601 civil-calendar
/// conversion rather than duplicating it.
fn wire_world_state(state: rdpilot::WorldState) -> Result<WireResponse, DaemonError> {
    let timestamp = crate::registry::iso8601_from_system_time(state.timestamp);
    let capture_span_ms = duration_as_millis_u64(state.capture_span);
    let screenshot = state.screenshot.as_ref().map(screenshot_to_base64).transpose()?;
    let window_list = state.window_list.map(|windows| windows.into_iter().map(wire_window_info).collect());
    let uia = state.uia.map(|groups| {
        groups
            .into_iter()
            .map(|(hwnd, elements)| (hwnd, elements.into_iter().map(wire_uia_element).collect()))
            .collect()
    });
    Ok(WireResponse::WorldState { timestamp, capture_span_ms, screenshot, window_list, uia, elevation_active: state.elevation_active })
}

/// `Duration::as_millis()` returns `u128`; the wire shape is `u64`
/// (`capture_span_ms`) — saturate rather than silently wrap on the
/// astronomically large durations that would only ever arise from a
/// malformed/adversarial clock, never a real capture span (API-01: no
/// unwrap/expect/panic in library code).
fn duration_as_millis_u64(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Wire mirror of `rdpilot::Rect` (identical field shape — a straight
/// field-for-field copy, no unit conversion).
fn wire_rect(r: rdpilot::Rect) -> WireRect {
    WireRect { x: r.x, y: r.y, w: r.w, h: r.h }
}

/// Wire mirror of `rdpilot::WindowState`.
fn wire_window_state(s: rdpilot::WindowState) -> WireWindowState {
    match s {
        rdpilot::WindowState::Normal => WireWindowState::Normal,
        rdpilot::WindowState::Minimized => WireWindowState::Minimized,
        rdpilot::WindowState::Maximized => WireWindowState::Maximized,
    }
}

/// Wire mirror of `rdpilot::WindowInfo`.
fn wire_window_info(w: rdpilot::WindowInfo) -> WireWindowInfo {
    WireWindowInfo {
        hwnd: w.hwnd,
        title: w.title,
        rect: wire_rect(w.rect),
        z_order: w.z_order,
        state: wire_window_state(w.state),
        class_name: w.class_name,
        pid: w.pid,
    }
}

/// Wire mirror of `rdpilot::ProcessInfo` (identical field shape).
fn wire_process_info(p: rdpilot::ProcessInfo) -> WireProcessInfo {
    WireProcessInfo {
        pid: p.pid,
        parent_pid: p.parent_pid,
        name: p.name,
        path: p.path,
        command_line: p.command_line,
        owner: p.owner,
        session_id: p.session_id,
    }
}

/// Wire `WireUacDecision` -> SDK `rdpilot::UacDecision`.
fn sdk_uac_decision(d: WireUacDecision) -> rdpilot::UacDecision {
    match d {
        WireUacDecision::Approve => rdpilot::UacDecision::Approve,
        WireUacDecision::Reject => rdpilot::UacDecision::Reject,
    }
}

/// SDK `rdpilot::UacDecision` -> wire `WireUacDecision`.
fn wire_uac_decision(d: rdpilot::UacDecision) -> WireUacDecision {
    match d {
        rdpilot::UacDecision::Approve => WireUacDecision::Approve,
        rdpilot::UacDecision::Reject => WireUacDecision::Reject,
    }
}

/// Wire mirror of `rdpilot::UiaElement`.
fn wire_uia_element(e: rdpilot::UiaElement) -> WireUiaElement {
    WireUiaElement {
        id: e.id,
        role: e.role,
        name: e.name,
        bbox: wire_rect(e.bbox),
        enabled: e.enabled,
        visible: e.visible,
        focusable: e.focusable,
        focused: e.focused,
        depth: e.depth,
        parent_id: e.parent_id,
    }
}

/// Wire `WireUiaScope` -> SDK `rdpilot::UiaScope`.
fn sdk_uia_scope(s: WireUiaScope) -> rdpilot::UiaScope {
    match s {
        WireUiaScope::Children => rdpilot::UiaScope::Children,
        WireUiaScope::Subtree { max_depth } => rdpilot::UiaScope::Subtree { max_depth },
    }
}

/// Wire `WireUiaMode` -> SDK `rdpilot::UiaMode`.
fn sdk_uia_mode(m: WireUiaMode) -> rdpilot::UiaMode {
    match m {
        WireUiaMode::None => rdpilot::UiaMode::None,
        WireUiaMode::Foreground => rdpilot::UiaMode::Foreground,
        WireUiaMode::Hwnd(hwnds) => rdpilot::UiaMode::Hwnd(hwnds),
        WireUiaMode::AllTopLevel => rdpilot::UiaMode::AllTopLevel,
    }
}

/// Wire `WireWorldStateOptions` -> SDK `rdpilot::WorldStateOptions`.
fn sdk_world_state_options(o: WireWorldStateOptions) -> rdpilot::WorldStateOptions {
    rdpilot::WorldStateOptions {
        screenshot: o.screenshot,
        window_list: o.window_list,
        uia: sdk_uia_mode(o.uia),
        elevation_check: o.elevation_check,
    }
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
        WireMouseAction::Click { x, y, button } => rdpilot::MouseAction::Click { x, y, button: sdk_button(button) },
        WireMouseAction::DoubleClick { x, y, button } => {
            rdpilot::MouseAction::DoubleClick { x, y, button: sdk_button(button) }
        }
        WireMouseAction::Scroll { x, y, dy } => rdpilot::MouseAction::Scroll { x, y, dy },
        WireMouseAction::Drag { from_x, from_y, to_x, to_y, button } => {
            rdpilot::MouseAction::Drag { from_x, from_y, to_x, to_y, button: sdk_button(button) }
        }
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
        WireKeyAction::Combo(keys) => rdpilot::KeyAction::Combo(keys.into_iter().map(sdk_key).collect()),
    }
}

/// SDK `rdpilot::TransferOutcome` -> wire mirror `rdpilot_ipc::TransferOutcome`.
fn wire_transfer_outcome(o: rdpilot::TransferOutcome) -> WireTransferOutcome {
    WireTransferOutcome { bytes_transferred: o.bytes_transferred, checksum: o.checksum }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rdpilot_ipc::{SessionId, SessionLifecycle, WireError, WireErrorCode};

    use super::*;
    use crate::seams::{BoxFuture, DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector};

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
                    elevation_active: None,
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
        fn desktop_size(&self) -> (u32, u32) {
            (1920, 1080)
        }
        fn deploy_and_launch(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
            Box::pin(async move { Ok(std::time::Duration::from_millis(0)) })
        }
        fn uac_respond(
            &self,
            decision: rdpilot::UacDecision,
        ) -> BoxFuture<'_, Result<rdpilot::UacResponseOutcome, DaemonError>> {
            Box::pin(async move {
                Ok(rdpilot::UacResponseOutcome {
                    decision,
                    confirmation: rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] },
                })
            })
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
            connect_ack: false,
        }
    }

    #[tokio::test]
    async fn connect_reports_sensor_liveness_matching_the_resolved_configuration() {
        let registry = test_registry();
        let response = dispatch(&registry, connect_request(Some("web"), "10.0.0.5")).await;
        let expected_sensor_live = resolve_sensor_binary_path()
            .expect("test configuration resolves")
            .is_some();
        match response {
            WireResponse::Connected { session, sensor_live, .. } => {
                assert_eq!(session.as_str(), "web");
                assert_eq!(
                    sensor_live, expected_sensor_live,
                    "the Connect response must distinguish a verified configured sensor from RDP-only mode"
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
        assert!(matches!(response, WireResponse::Ack), "expected Ack, got {response:?}");
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
        assert_eq!(&bytes[0..4], &[0x89, b'P', b'N', b'G'], "decoded bytes must be a PNG");
    }

    /// A `ManagedSession` fake reporting exactly one populated
    /// window/process/UIA element from each perception method -- proves the
    /// `wire_window_info`/`wire_process_info`/`wire_uia_element` field-by-
    /// field conversions actually run (an empty-vec fake could pass even
    /// with a broken mapper).
    struct RichPerceptionSession;

    impl ManagedSession for RichPerceptionSession {
        fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
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
                    elevation_active: None,
                })
            })
        }
        fn get_window_list(&self) -> BoxFuture<'_, Result<Vec<rdpilot::WindowInfo>, DaemonError>> {
            Box::pin(async {
                Ok(vec![rdpilot::WindowInfo {
                    hwnd: 65536,
                    title: "Notepad".to_owned(),
                    rect: rdpilot::Rect { x: 0, y: 0, w: 100, h: 100 },
                    z_order: 0,
                    state: rdpilot::WindowState::Normal,
                    class_name: "Notepad".to_owned(),
                    pid: 4242,
                }])
            })
        }
        fn get_process_tree(&self) -> BoxFuture<'_, Result<Vec<rdpilot::ProcessInfo>, DaemonError>> {
            Box::pin(async {
                Ok(vec![rdpilot::ProcessInfo {
                    pid: 4242,
                    parent_pid: 4,
                    name: "notepad.exe".to_owned(),
                    path: "C:\\Windows\\notepad.exe".to_owned(),
                    command_line: None,
                    owner: None,
                    session_id: None,
                }])
            })
        }
        fn get_uia_tree(&self, _hwnd: u64, _scope: rdpilot::UiaScope) -> BoxFuture<'_, Result<Vec<rdpilot::UiaElement>, DaemonError>> {
            Box::pin(async {
                Ok(vec![rdpilot::UiaElement {
                    id: "1.2.3".to_owned(),
                    role: "Button".to_owned(),
                    name: "OK".to_owned(),
                    bbox: rdpilot::Rect { x: 0, y: 0, w: 10, h: 10 },
                    enabled: true,
                    visible: true,
                    focusable: true,
                    focused: false,
                    depth: 1,
                    parent_id: "1.2".to_owned(),
                }])
            })
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
        fn desktop_size(&self) -> (u32, u32) {
            (1920, 1080)
        }
        fn deploy_and_launch(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
            Box::pin(async move { Ok(std::time::Duration::from_millis(0)) })
        }
        fn uac_respond(
            &self,
            decision: rdpilot::UacDecision,
        ) -> BoxFuture<'_, Result<rdpilot::UacResponseOutcome, DaemonError>> {
            Box::pin(async move {
                Ok(rdpilot::UacResponseOutcome {
                    decision,
                    confirmation: rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] },
                })
            })
        }
    }

    struct RichPerceptionConnector;

    impl SessionConnector for RichPerceptionConnector {
        fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            Box::pin(async { Ok(Box::new(RichPerceptionSession) as Box<dyn ManagedSession>) })
        }
    }

    fn rich_perception_registry() -> Registry {
        Registry::new(Arc::new(RichPerceptionConnector), Arc::new(NoopReconciliationSink))
    }

    #[tokio::test]
    async fn window_list_returns_the_fakes_one_element_vec_with_fields_mapped() {
        let registry = rich_perception_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::WindowList { session }).await;
        let WireResponse::WindowList { windows } = response else {
            panic!("expected WindowList, got {response:?}");
        };
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].hwnd, 65536);
        assert_eq!(windows[0].title, "Notepad");
        assert_eq!(windows[0].rect, WireRect { x: 0, y: 0, w: 100, h: 100 });
        assert_eq!(windows[0].pid, 4242);
    }

    #[tokio::test]
    async fn process_list_returns_the_fakes_one_element_vec_with_fields_mapped() {
        let registry = rich_perception_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::ProcessList { session }).await;
        let WireResponse::ProcessList { processes, elevation_active } = response else {
            panic!("expected ProcessList, got {response:?}");
        };
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].pid, 4242);
        assert_eq!(processes[0].name, "notepad.exe");
        assert!(!elevation_active, "the fake's single notepad.exe record is not an elevation prompt");
    }

    #[tokio::test]
    async fn uia_returns_the_fakes_one_element_vec_with_fields_mapped() {
        let registry = rich_perception_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::Uia { session, hwnd: 1, scope: WireUiaScope::Children }).await;
        let WireResponse::Uia { elements } = response else {
            panic!("expected Uia, got {response:?}");
        };
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].id, "1.2.3");
        assert_eq!(elements[0].role, "Button");
    }

    #[tokio::test]
    async fn world_state_converts_timestamp_and_capture_span_and_returns_the_fakes_data() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let options = WireWorldStateOptions { screenshot: false, window_list: false, uia: WireUiaMode::None, elevation_check: false };
        let response = dispatch(&registry, Request::WorldState { session, options }).await;
        let WireResponse::WorldState { timestamp, capture_span_ms, screenshot, window_list, uia, elevation_active } =
            response
        else {
            panic!("expected WorldState, got {response:?}");
        };
        assert!(!timestamp.is_empty(), "timestamp must be a non-empty ISO-8601 string");
        assert_eq!(capture_span_ms, 0, "the fake's capture_span is zero");
        assert!(screenshot.is_none(), "the fake's world_state screenshot is None");
        assert!(window_list.is_none(), "the fake's world_state window_list is None");
        assert!(uia.is_none(), "the fake's world_state uia is None");
        assert!(elevation_active.is_none(), "elevation_check was not requested");
    }

    #[tokio::test]
    async fn world_state_screenshot_when_the_fake_reports_one_decodes_to_valid_png_bytes() {
        // A second fake connector whose FakeSession reports a screenshot in
        // world_state (the shared FakeSession above always returns `None`
        // for it) -- proves the WorldState arm's screenshot conversion path
        // specifically, not just the always-None default.
        struct WithScreenshotSession;
        impl ManagedSession for WithScreenshotSession {
            fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
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
                        timestamp: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_704_067_200),
                        capture_span: std::time::Duration::from_millis(42),
                        screenshot: Some(rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] }),
                        window_list: None,
                        uia: None,
                        elevation_active: None,
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
            fn desktop_size(&self) -> (u32, u32) {
                (1920, 1080)
            }
            fn deploy_and_launch(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
                Box::pin(async move { Ok(std::time::Duration::from_millis(0)) })
            }
            fn uac_respond(
                &self,
                decision: rdpilot::UacDecision,
            ) -> BoxFuture<'_, Result<rdpilot::UacResponseOutcome, DaemonError>> {
                Box::pin(async move {
                    Ok(rdpilot::UacResponseOutcome {
                        decision,
                        confirmation: rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] },
                    })
                })
            }
        }
        struct WithScreenshotConnector;
        impl SessionConnector for WithScreenshotConnector {
            fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
                Box::pin(async { Ok(Box::new(WithScreenshotSession) as Box<dyn ManagedSession>) })
            }
        }

        let registry = Registry::new(Arc::new(WithScreenshotConnector), Arc::new(NoopReconciliationSink));
        let session = connected_session(&registry).await;
        let options = WireWorldStateOptions { screenshot: true, window_list: false, uia: WireUiaMode::None, elevation_check: false };
        let response = dispatch(&registry, Request::WorldState { session, options }).await;
        let WireResponse::WorldState { timestamp, capture_span_ms, screenshot, .. } = response else {
            panic!("expected WorldState, got {response:?}");
        };
        assert_eq!(timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(capture_span_ms, 42);
        let png_base64 = screenshot.expect("screenshot must be Some when the fake reports one");
        let bytes = BASE64_STANDARD.decode(png_base64).expect("valid base64");
        assert_eq!(&bytes[0..4], &[0x89, b'P', b'N', b'G']);
    }

    #[tokio::test]
    async fn mouse_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let action = WireMouseAction::Move { x: 1, y: 2 };
        let response = dispatch(&registry, Request::Mouse { session, action }).await;
        assert!(matches!(response, WireResponse::Ack), "expected Ack, got {response:?}");
    }

    #[tokio::test]
    async fn key_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let action = WireKeyAction::Type("hi".to_owned());
        let response = dispatch(&registry, Request::Key { session, action }).await;
        assert!(matches!(response, WireResponse::Ack), "expected Ack, got {response:?}");
    }

    #[tokio::test]
    async fn launch_process_returns_the_fakes_pid() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(
            &registry,
            Request::LaunchProcess { session, exe: "notepad.exe".to_owned(), args: None, cwd: None },
        )
        .await;
        assert!(matches!(response, WireResponse::Pid { pid: 0 }), "expected Pid{{pid:0}}, got {response:?}");
    }

    #[tokio::test]
    async fn set_foreground_returns_ack() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::SetForeground { session, hwnd: 1 }).await;
        assert!(matches!(response, WireResponse::Ack), "expected Ack, got {response:?}");
    }

    #[tokio::test]
    async fn put_returns_a_transfer_response() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(
            &registry,
            Request::Put { session, local_path: "/tmp/a".to_owned(), remote_name: "a".to_owned() },
        )
        .await;
        assert!(matches!(response, WireResponse::Transfer(_)), "expected Transfer, got {response:?}");
    }

    #[tokio::test]
    async fn get_returns_a_transfer_response() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(
            &registry,
            Request::Get { session, remote_name: "a".to_owned(), local_path: "/tmp/a".to_owned() },
        )
        .await;
        assert!(matches!(response, WireResponse::Transfer(_)), "expected Transfer, got {response:?}");
    }

    /// Every operational verb still returns `SessionNotFound` for an unknown
    /// session -- `Registry::call`'s own guarantee, exercised end-to-end
    /// through `dispatch` for a representative verb beyond `Ping` (already
    /// covered above) to prove the property holds after the not-implemented
    /// arms were replaced with live ones.
    #[tokio::test]
    async fn an_operational_verb_for_an_unknown_session_still_returns_session_not_found_after_wiring() {
        let registry = test_registry();
        let session: SessionId = "ghost".parse().expect("non-empty literal");
        let response = dispatch(&registry, Request::Screenshot { session }).await;
        match response {
            WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, .. }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
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
            WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, .. }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }

    // --- UacRespond dispatch, and session-scoped elevation_active on
    // ProcessList ---

    #[tokio::test]
    async fn uac_respond_returns_the_fakes_confirmation() {
        let registry = test_registry();
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::UacRespond { session, decision: WireUacDecision::Approve }).await;
        match response {
            WireResponse::UacRespond { decision, confirmation_png_base64 } => {
                assert!(matches!(decision, WireUacDecision::Approve));
                let bytes = BASE64_STANDARD.decode(confirmation_png_base64).expect("valid base64");
                assert_eq!(&bytes[0..4], &[0x89, b'P', b'N', b'G'], "decoded bytes must be a PNG");
            }
            other => panic!("expected UacRespond, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uac_respond_for_an_unknown_session_returns_session_not_found() {
        let registry = test_registry();
        let session: SessionId = "ghost".parse().expect("non-empty literal");
        let response = dispatch(&registry, Request::UacRespond { session, decision: WireUacDecision::Reject }).await;
        match response {
            WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, .. }) => {}
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    }

    /// A `ManagedSession` fake whose `get_process_tree` reports a single
    /// `consent.exe` record at a caller-fixed `session_id`, and whose
    /// `own_session_id()` is separately caller-fixed -- a same-session vs.
    /// different-session `consent.exe` fixture pair, exercised end-to-end
    /// through `dispatch`'s `ProcessList` arm.
    struct SessionScopedConsentSession {
        own_session_id: Option<u32>,
        consent_session_id: Option<u32>,
    }

    impl ManagedSession for SessionScopedConsentSession {
        fn close(self: Box<Self>) -> TestFuture<Result<(), DaemonError>> {
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
                    elevation_active: None,
                })
            })
        }
        fn get_window_list(&self) -> BoxFuture<'_, Result<Vec<rdpilot::WindowInfo>, DaemonError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn get_process_tree(&self) -> BoxFuture<'_, Result<Vec<rdpilot::ProcessInfo>, DaemonError>> {
            let consent_session_id = self.consent_session_id;
            Box::pin(async move {
                Ok(vec![rdpilot::ProcessInfo {
                    pid: 100,
                    parent_pid: 4,
                    name: "consent.exe".to_owned(),
                    path: "C:\\Windows\\System32\\consent.exe".to_owned(),
                    command_line: None,
                    owner: None,
                    session_id: consent_session_id,
                }])
            })
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
        fn desktop_size(&self) -> (u32, u32) {
            (1920, 1080)
        }
        fn deploy_and_launch(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>> {
            Box::pin(async move { Ok(std::time::Duration::from_millis(0)) })
        }
        fn own_session_id(&self) -> Option<u32> {
            self.own_session_id
        }
        fn uac_respond(
            &self,
            decision: rdpilot::UacDecision,
        ) -> BoxFuture<'_, Result<rdpilot::UacResponseOutcome, DaemonError>> {
            Box::pin(async move {
                Ok(rdpilot::UacResponseOutcome {
                    decision,
                    confirmation: rdpilot::Screenshot { width: 1, height: 1, rgba: vec![0, 0, 0, 0] },
                })
            })
        }
    }

    struct SessionScopedConsentConnector {
        own_session_id: Option<u32>,
        consent_session_id: Option<u32>,
    }

    impl SessionConnector for SessionScopedConsentConnector {
        fn connect(&self, _cfg: ConnectionConfig) -> TestFuture<Result<Box<dyn ManagedSession>, DaemonError>> {
            let own_session_id = self.own_session_id;
            let consent_session_id = self.consent_session_id;
            Box::pin(async move {
                Ok(Box::new(SessionScopedConsentSession { own_session_id, consent_session_id }) as Box<dyn ManagedSession>)
            })
        }
    }

    /// The core session-scoping safeguard: a `consent.exe` in THIS session
    /// reports `elevation_active: true` on `ProcessList`.
    #[tokio::test]
    async fn process_list_reports_elevation_active_true_for_a_same_session_consent_exe() {
        let registry = Registry::new(
            Arc::new(SessionScopedConsentConnector { own_session_id: Some(1), consent_session_id: Some(1) }),
            Arc::new(NoopReconciliationSink),
        );
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::ProcessList { session }).await;
        let WireResponse::ProcessList { elevation_active, .. } = response else {
            panic!("expected ProcessList, got {response:?}");
        };
        assert!(elevation_active, "a consent.exe in THIS session must report elevation_active: true");
    }

    /// The core session-scoping safeguard: a `consent.exe` in a DIFFERENT
    /// session must never false-positive `elevation_active` for this
    /// session's `ProcessList`.
    #[tokio::test]
    async fn process_list_reports_elevation_active_false_for_a_different_session_consent_exe() {
        let registry = Registry::new(
            Arc::new(SessionScopedConsentConnector { own_session_id: Some(1), consent_session_id: Some(2) }),
            Arc::new(NoopReconciliationSink),
        );
        let session = connected_session(&registry).await;
        let response = dispatch(&registry, Request::ProcessList { session }).await;
        let WireResponse::ProcessList { elevation_active, .. } = response else {
            panic!("expected ProcessList, got {response:?}");
        };
        assert!(
            !elevation_active,
            "a consent.exe in a DIFFERENT session must never false-positive this session's detection"
        );
    }
}
