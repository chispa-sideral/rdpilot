//! `record start|stop`, `annotate` and `recording list|keep|unkeep`: thin
//! requests to the daemon's recording service. None of them counts as
//! session activity.

use rdpilot_ipc::{Request, WireRecording, WireResponse};

use crate::cli::{AnnotateArgs, RecordingIdArg, SessionArg};
use crate::connect::round_trip;
use crate::exit_codes::CliError;
use crate::render::{print_json, render_table};

/// Print a `RecordingChanged` answer.
fn changed(response: WireResponse, verb: &str, json: bool) -> Result<(), CliError> {
    match response {
        WireResponse::RecordingChanged {
            id,
            changed,
            message,
        } => {
            if json {
                print_json(&serde_json::json!({
                    "id": id,
                    "changed": changed,
                    "message": message,
                }))
            } else {
                println!("{message}");
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to {verb}: {other:?}"
        ))),
    }
}

/// `record start --session NAME`.
///
/// # Errors
///
/// The daemon's error (unknown session, recording unavailable) or a
/// transport failure.
pub async fn start(args: SessionArg, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    changed(
        round_trip(Request::RecordStart { session }).await?,
        "record start",
        json,
    )
}

/// `record stop --session NAME`.
///
/// # Errors
///
/// As for [`start`].
pub async fn stop(args: SessionArg, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    changed(
        round_trip(Request::RecordStop { session }).await?,
        "record stop",
        json,
    )
}

/// `annotate --session NAME TEXT`.
///
/// # Errors
///
/// Not recording, empty or over 4 KiB, recording busy; or as for [`start`].
pub async fn annotate(args: AnnotateArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    changed(
        round_trip(Request::Annotate {
            session,
            text: args.text,
        })
        .await?,
        "annotate",
        json,
    )
}

/// `recording keep ID` (`keep`) or `recording unkeep ID`.
///
/// # Errors
///
/// Unknown id; or as for [`start`].
pub async fn keep(args: RecordingIdArg, keep: bool, json: bool) -> Result<(), CliError> {
    changed(
        round_trip(Request::RecordingKeep { id: args.id, keep }).await?,
        if keep {
            "recording keep"
        } else {
            "recording unkeep"
        },
        json,
    )
}

/// `1.2 MiB`-style size.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `1h02m03s`-style duration.
fn duration(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

fn row(r: &WireRecording) -> Vec<String> {
    vec![
        r.id.clone(),
        r.session_name.clone().unwrap_or_else(|| r.session.clone()),
        r.host.clone(),
        r.started_at.clone(),
        duration(r.duration_ms),
        size(r.bytes),
        if r.active { "yes" } else { "no" }.to_owned(),
        if r.kept { "yes" } else { "no" }.to_owned(),
    ]
}

/// `recording list`.
///
/// # Errors
///
/// As for [`start`].
pub async fn list(json: bool) -> Result<(), CliError> {
    match round_trip(Request::RecordingList {}).await? {
        WireResponse::Recordings {
            recordings,
            kept_bytes,
            unkept_bytes,
            budget_bytes,
            kept_over_budget,
        } => {
            if json {
                return print_json(&serde_json::json!({
                    "recordings": recordings,
                    "kept_bytes": kept_bytes,
                    "unkept_bytes": unkept_bytes,
                    "budget_bytes": budget_bytes,
                    "kept_over_budget": kept_over_budget,
                }));
            }
            const HEADERS: [&str; 8] = [
                "id", "session", "host", "started", "duration", "size", "active", "kept",
            ];
            let rows: Vec<Vec<String>> = recordings.iter().map(row).collect();
            print!("{}", render_table(&HEADERS, &rows));
            println!(
                "kept: {}; unkept: {} of {} budget",
                size(kept_bytes),
                size(unkept_bytes),
                size(budget_bytes)
            );
            if kept_over_budget {
                println!(
                    "warning: kept recordings use more than the budget; nothing is deleted automatically"
                );
            }
            Ok(())
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to recording list: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_durations_are_short() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(2 * 1024 * 1024 * 1024), "2.0 GiB");
        assert_eq!(duration(4_500), "4s");
        assert_eq!(duration(125_000), "2m05s");
        assert_eq!(duration(3_723_000), "1h02m03s");
    }
}
