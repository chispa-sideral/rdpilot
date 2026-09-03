//! `connect`/`list`/`disconnect` — CLI-01's session-lifecycle verbs. Each is
//! a thin invoke-and-exit `rdpilot-ipc` client: resolve config (`connect`
//! only), round-trip exactly one `Request`/`WireResponse` frame pair over
//! the auto-started daemon, and render the SPECIFIC response variant it
//! expects (table by default, `--json` opt-in).

use rdpilot_ipc::{Request, SessionLifecycle, WireResponse};

use crate::cli::{ConnectArgs, SessionArg};
use crate::connect::{connect_round_trip, round_trip};
use crate::exit_codes::CliError;
use crate::render::{print_json, render_table};

/// `connect [--name] [config flags]` (D-29, SESSION-01).
///
/// # Errors
///
/// [`CliError::MissingConfig`] if host/username/password are absent after
/// file->env->flag resolution; the daemon's own error (typically
/// `DuplicateSession`) if `Connect` is rejected; or a transport/auto-start
/// failure.
pub async fn connect(args: ConnectArgs, json: bool) -> Result<(), CliError> {
    let resolved =
        rdpilot_config::resolve(args.config.into_overrides()).map_err(|e| CliError::MissingConfig(e.to_string()))?;

    let host = resolved
        .host
        .ok_or_else(|| CliError::MissingConfig("host is required (config file, RDPILOT_HOST, or --host)".to_owned()))?;
    let username = resolved.username.ok_or_else(|| {
        CliError::MissingConfig("username is required (config file, RDPILOT_USERNAME, or --username)".to_owned())
    })?;
    let password = resolved.password.ok_or_else(|| {
        CliError::MissingConfig("password is required (config file, RDPILOT_PASSWORD, or --password)".to_owned())
    })?;

    let req = Request::Connect {
        name: args.name,
        host,
        port: resolved.port,
        username,
        password,
        domain: resolved.domain,
        accept_invalid_certs: resolved.accept_invalid_certs,
        connect_ack: true,
    };

    match connect_round_trip(req).await? {
        WireResponse::Connected { session, sensor_live, .. } => {
            if json {
                print_json(&serde_json::json!({ "session": session.as_str(), "sensor_live": sensor_live }))
            } else {
                println!("{}", connect_status_message(session.as_str(), sensor_live));
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to Connect: {other:?}"))),
    }
}

/// Human-readable result for a successful connect.
///
/// A session can be usable for basic RDP operations without the optional
/// sensor executable, but sensor-backed perception and automation will not
/// work. Make that degraded mode unmissable instead of presenting it as a
/// routine connection status.
fn connect_status_message(session: &str, sensor_live: bool) -> String {
    if sensor_live {
        format!("connected {session}; sensor live")
    } else {
        format!("connected {session}; sensor not configured; session management available")
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
        WireResponse::SessionList { sessions } => {
            if json {
                print_json(&sessions)
            } else {
                const HEADERS: [&str; 6] = ["id", "name", "host", "status", "connected-since", "last-activity"];
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
        other => Err(CliError::Internal(format!("unexpected response to List: {other:?}"))),
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

#[cfg(test)]
mod tests {
    use super::connect_status_message;

    #[test]
    fn sensorless_status_reports_session_management_only() {
        assert_eq!(
            connect_status_message("desktop", false),
            "connected desktop; sensor not configured; session management available"
        );
    }

    #[test]
    fn live_sensor_status_remains_concise() {
        assert_eq!(
            connect_status_message("desktop", true),
            "connected desktop; sensor live"
        );
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
        other => Err(CliError::Internal(format!("unexpected response to Disconnect: {other:?}"))),
    }
}
