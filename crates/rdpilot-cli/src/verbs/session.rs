//! `connect`/`list`/`disconnect` — CLI-01's session-lifecycle verbs. Each is
//! a thin invoke-and-exit `rdpilot-ipc` client: resolve config (`connect`
//! only), round-trip exactly one `Request`/`WireResponse` frame pair over
//! the auto-started daemon, and render the SPECIFIC response variant it
//! expects (table by default, `--json` opt-in).

use rdpilot_ipc::{
    Request, SessionLifecycle, WireController, WireControllerKind, WireRecordTrigger,
    WireRecordingState, WireResponse,
};

use rdpilot_config::hosts::expand_password_command;

use crate::cli::{ConnectArgs, SessionArg};
use crate::connect::{connect_round_trip, round_trip};
use crate::exit_codes::CliError;
use crate::password_command;
use crate::render::{print_json, render_table};
use crate::verbs::config::resolve_target;

/// `connect <target> [-o KEYWORD=VALUE]... [-F FILE] [--name]` (D-29, SESSION-01).
///
/// # Errors
///
/// [`CliError::Config`] for a hosts-file, target, option or PasswordCommand
/// problem; [`CliError::MissingConfig`] if no `User` or no credential
/// resolves; the daemon's own error (typically `DuplicateSession`) if
/// `Connect` is rejected; or a transport/auto-start failure.
pub async fn connect(args: ConnectArgs, json: bool) -> Result<(), CliError> {
    let host = resolve_target(&args.target)?;
    let label = host.target().to_string();
    // The switch is keyed on the target as given (alias or URL host).
    let record = rdpilot_config::resolve_recording()
        .map_err(|e| CliError::Config(e.to_string()))?
        .switch_for(host.target().name(), args.record_flag())
        .map(|trigger| match trigger {
            rdpilot_config::RecordTrigger::GlobalConfig => WireRecordTrigger::GlobalConfig,
            rdpilot_config::RecordTrigger::HostConfig => WireRecordTrigger::HostConfig,
            rdpilot_config::RecordTrigger::ConnectFlag => WireRecordTrigger::ConnectFlag,
        });

    let username = host.user().ok_or_else(|| {
        CliError::MissingConfig(format!(
            "no User for {label} (set User in the hosts file, use -o User=..., or rdp://user@host)"
        ))
    })?;
    let password = if let Some(password) = host.password() {
        password
    } else if host.password_command().is_some() {
        let command = expand_password_command(&host, &|k| std::env::var(k).ok())
            .map_err(|e| CliError::Config(e.to_string()))?;
        password_command::run(&command, &label, password_command::TIMEOUT).await?
    } else {
        return Err(CliError::MissingConfig(format!(
            "no Password or PasswordCommand for {label}"
        )));
    };

    let req = Request::Connect {
        name: args.name,
        host: host.address().to_owned(),
        port: host.port(),
        username: username.to_owned(),
        password: password.expose().to_owned(),
        domain: host.domain().map(str::to_owned),
        accept_invalid_certs: host.accept_invalid_certs(),
        cua_enabled: host.cua_enabled(),
        cua_version: host.cua_version().to_owned(),
        cua_auto_download: host.cua_auto_download(),
        connect_ack: true,
        record,
    };

    match connect_round_trip(req).await? {
        WireResponse::Connected {
            session,
            bridge_live,
            recording,
            warnings,
            ..
        } => {
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
            if json {
                let recording = match &recording {
                    WireRecordingState::Off => serde_json::json!({ "state": "off" }),
                    WireRecordingState::On { id } => serde_json::json!({ "state": "on", "id": id }),
                    WireRecordingState::Failed { reason } => {
                        serde_json::json!({ "state": "failed", "reason": reason })
                    }
                };
                print_json(&serde_json::json!({
                    "session": session.as_str(),
                    "bridge_live": bridge_live,
                    "recording": recording,
                }))
            } else {
                println!("{}", connect_status_message(session.as_str(), bridge_live));
                match recording {
                    WireRecordingState::Off => {}
                    WireRecordingState::On { id } => println!("recording: on ({id})"),
                    WireRecordingState::Failed { reason } => {
                        eprintln!("recording could not start: {reason}");
                    }
                }
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to Connect: {other:?}"
        ))),
    }
}

/// Human-readable result for a successful connect.
///
/// A Cua-enabled connect either has a live bridge or fails, so a session
/// without a bridge is one connected with `CuaEnabled no`. Say so plainly:
/// Cua tools and file transfer are unavailable in it.
fn connect_status_message(session: &str, bridge_live: bool) -> String {
    if bridge_live {
        format!("connected {session}; bridge live")
    } else {
        format!("connected {session}; native only (CuaEnabled no): no Cua tools or put/get")
    }
}

/// `list` (D-30, SESSION-03): renders id/name/host/status/connected-since/
/// last-activity using the D-30 status vocabulary verbatim.
///
/// # Errors
///
/// The daemon's own error, or a transport/auto-start failure.
pub async fn list(json: bool) -> Result<(), CliError> {
    match round_trip(Request::List {}).await? {
        WireResponse::SessionList { sessions, .. } => {
            if json {
                print_json(&sessions)
            } else {
                const HEADERS: [&str; 8] = [
                    "id",
                    "name",
                    "host",
                    "status",
                    "connected-since",
                    "last-activity",
                    "recording",
                    "control",
                ];
                let rows: Vec<Vec<String>> = sessions
                    .iter()
                    .map(|s| {
                        vec![
                            s.id.clone(),
                            s.name.clone().unwrap_or_default(),
                            s.host.clone(),
                            lifecycle_str(s.status).to_owned(),
                            s.connected_since.clone().unwrap_or_default(),
                            s.last_activity.clone().unwrap_or_default(),
                            s.recording.clone().unwrap_or_else(|| "-".to_owned()),
                            s.controller
                                .as_ref()
                                .map_or_else(|| "-".to_owned(), controller_str),
                        ]
                    })
                    .collect();
                print!("{}", render_table(&HEADERS, &rows));
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to List: {other:?}"
        ))),
    }
}

/// `agent`, or `human <address> since <HH:MM:SS> UTC`.
fn controller_str(controller: &WireController) -> String {
    match controller.kind {
        WireControllerKind::Agent => "agent".to_owned(),
        WireControllerKind::Human => format!("human {}", holder_since(controller)),
    }
}

/// `<address> since <HH:MM:SS> UTC` of a human controller.
fn holder_since(controller: &WireController) -> String {
    format!(
        "{} since {}",
        controller.address.as_deref().unwrap_or("?"),
        controller
            .since
            .as_deref()
            .map_or_else(|| "?".to_owned(), clock_time)
    )
}

/// `HH:MM:SS UTC` of an ISO-8601 UTC time.
fn clock_time(iso: &str) -> String {
    iso.get(11..19)
        .map_or_else(|| iso.to_owned(), |hms| format!("{hms} UTC"))
}

/// `takeover --session <id>`: end any human viewer's control lease and
/// return control to the agent.
///
/// # Errors
///
/// The daemon's error (`SessionNotFound` for an unknown session), or a
/// transport/auto-start failure.
pub async fn takeover(args: SessionArg, json: bool) -> Result<(), CliError> {
    let name = args.session;
    let session = name.parse().map_err(CliError::Internal)?;
    match round_trip(Request::Takeover { session }).await? {
        WireResponse::TakenOver { previous, changed } => {
            if json {
                print_json(&serde_json::json!({ "changed": changed, "previous": previous }))
            } else {
                if changed {
                    println!(
                        "control of {name} returned to the agent (was {})",
                        human_str(&previous)
                    );
                } else {
                    println!("the agent already controls {name}; nothing changed");
                }
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to Takeover: {other:?}"
        ))),
    }
}

/// `human viewer <address> since <HH:MM:SS> UTC`, or `the agent`.
fn human_str(controller: &WireController) -> String {
    match controller.kind {
        WireControllerKind::Agent => "the agent".to_owned(),
        WireControllerKind::Human => format!("human viewer {}", holder_since(controller)),
    }
}

/// The D-30 session-lifecycle vocabulary, rendered verbatim.
fn lifecycle_str(status: SessionLifecycle) -> &'static str {
    match status {
        SessionLifecycle::Connecting => "Connecting",
        SessionLifecycle::Live => "Live",
        SessionLifecycle::Reconnecting => "Reconnecting",
        SessionLifecycle::Disconnected => "Disconnected",
        SessionLifecycle::Orphaned => "Orphaned",
    }
}

/// `disconnect --session <id>` (SESSION-04). A `WireResponse::Error` with
/// code `SessionNotFound` maps to `CliError` -> exit code 2 (D-28).
///
/// # Errors
///
/// [`CliError::Internal`] if `--session` is an empty string (rejected by
/// `SessionId::from_str`); the daemon's own error otherwise; or a
/// transport/auto-start failure.
pub async fn disconnect(args: SessionArg, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::Disconnect { session }).await? {
        WireResponse::Ack => {
            if json {
                print_json(&serde_json::json!({ "ok": true }))
            } else {
                println!("disconnected");
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to Disconnect: {other:?}"
        ))),
    }
}

pub async fn ping(args: SessionArg, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::Ping { session }).await? {
        WireResponse::Ack => {
            if json {
                print_json(&serde_json::json!({"ok":true}))
            } else {
                println!("ok");
                Ok(())
            }
        }
        WireResponse::Error(e) => Err(e.into()),
        other => Err(CliError::Internal(format!(
            "unexpected ping response: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::connect_status_message;

    #[test]
    fn bridgeless_status_reports_native_only() {
        assert_eq!(
            connect_status_message("desktop", false),
            "connected desktop; native only (CuaEnabled no): no Cua tools or put/get"
        );
    }

    #[test]
    fn live_bridge_status_remains_concise() {
        assert_eq!(
            connect_status_message("desktop", true),
            "connected desktop; bridge live"
        );
    }
}
