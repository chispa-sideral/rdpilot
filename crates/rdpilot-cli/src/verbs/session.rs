//! `connect`/`list`/`disconnect` — CLI-01's session-lifecycle verbs. Each is
//! a thin invoke-and-exit `rdpilot-ipc` client: resolve config (`connect`
//! only), round-trip exactly one `Request`/`WireResponse` frame pair over
//! the auto-started daemon, and render the SPECIFIC response variant it
//! expects (table by default, `--json` opt-in).

use rdpilot_ipc::{Request, SessionLifecycle, WireResponse};

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
        record: None,
    };

    match connect_round_trip(req).await? {
        WireResponse::Connected {
            session,
            bridge_live,
            warnings,
            ..
        } => {
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
            if json {
                print_json(
                    &serde_json::json!({ "session": session.as_str(), "bridge_live": bridge_live }),
                )
            } else {
                println!("{}", connect_status_message(session.as_str(), bridge_live));
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
                const HEADERS: [&str; 6] = [
                    "id",
                    "name",
                    "host",
                    "status",
                    "connected-since",
                    "last-activity",
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
