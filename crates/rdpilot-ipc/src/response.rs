//! [`WireResponse`] — the daemon-to-client wire response set, plus the
//! credential-free [`SessionStatus`]/[`SessionLifecycle`] list DTO (D-30).
//!
//! No `WireResponse` variant, nor [`SessionStatus`], ever defines a
//! password/secret/credential-shaped field (D-31): this crate does not
//! depend on `rdpilot-config`, so a `Credentials`-shaped type structurally
//! cannot enter this wire graph. CONFIG-03's planted-sentinel test below is
//! a regression guard for that structural guarantee, not a live end-to-end
//! secret trace — that live trace only becomes meaningful once Phase 12's
//! session registry exists.

use serde::{Deserialize, Serialize};

use crate::error::WireError;
use crate::session_id::SessionId;
use crate::transfer::TransferOutcome;

/// The D-30 session lifecycle vocabulary, derived from the SDK keepalive
/// signal (Phase 12 populates it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SessionLifecycle {
    /// The session is being established.
    Connecting,
    /// The session is active and healthy.
    Live,
    /// The session dropped and is being re-established.
    Reconnecting,
    /// The session is no longer active.
    Disconnected,
    /// A possibly-still-live remote Windows session left behind by a
    /// crashed/restarted daemon (DAEMON-04, research Pitfall 9). Surfaced
    /// distinctly, never silently forgotten or auto-torn-down — the daemon
    /// requires an explicit reclaim/teardown of an `Orphaned` entry.
    Orphaned,
}

/// A credential-free summary of one daemon-managed session (D-30/D-31).
///
/// No field here is ever password/secret/credential-shaped — `host` is
/// target addressing information, not a secret, and is explicitly allowed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    /// The session's wire identifier.
    pub id: String,
    /// An optional caller-assigned display name.
    pub name: Option<String>,
    /// The target host (non-secret target addressing, D-31).
    pub host: String,
    /// The D-30 lifecycle status.
    pub status: SessionLifecycle,
    /// ISO-8601 timestamp of when the session connected, if known.
    pub connected_since: Option<String>,
    /// ISO-8601 timestamp of the last observed activity, if known.
    pub last_activity: Option<String>,
    /// The id of the session's active recording, if it is recording.
    #[serde(default)]
    pub recording: Option<String>,
    /// Who controls the session's input: the agent (default) or a human
    /// viewer tab. `None` for sessions that cannot take input (connecting,
    /// orphaned).
    #[serde(default)]
    pub controller: Option<WireController>,
}

/// Who may send input to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireControllerKind {
    /// Any rdpilot client acting through the daemon (native verbs, Cua).
    Agent,
    /// One live viewer tab holding the control lease.
    Human,
}

/// The current controller of a session. For a human viewer, `address` is
/// the client address its tab connects from and `since` the UTC time it
/// took control (`YYYY-MM-DDTHH:MM:SSZ`). Never carries a lease id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireController {
    pub kind: WireControllerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

impl WireController {
    /// The agent controller.
    #[must_use]
    pub fn agent() -> Self {
        WireController {
            kind: WireControllerKind::Agent,
            address: None,
            since: None,
        }
    }
}

/// Whether the session records after a connect. A recording failure never
/// fails the connect.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireRecordingState {
    /// Not recording.
    #[default]
    Off,
    /// Recording under this id.
    On { id: String },
    /// Recording was asked for and could not start.
    Failed { reason: String },
}

/// One recording on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRecording {
    pub id: String,
    /// The session id.
    pub session: String,
    /// The session name, when one was given.
    #[serde(default)]
    pub session_name: Option<String>,
    /// The address the daemon connected to.
    pub host: String,
    /// UTC start (`YYYY-MM-DDTHH:MM:SS.sssZ`).
    pub started_at: String,
    /// Length so far (active) or in total.
    pub duration_ms: u64,
    /// Bytes on disk.
    pub bytes: u64,
    /// Still being written.
    pub active: bool,
    /// Marked keep: never pruned, not counted in the budget.
    pub kept: bool,
}

/// Responses never contain connection credentials. Cua messages use a separate stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireResponse {
    Ack,
    Connected {
        session: SessionId,
        #[serde(default)]
        connect_ack_required: bool,
        #[serde(default)]
        bridge_live: bool,
        #[serde(default)]
        recording: WireRecordingState,
        /// Notes for the user, for example a bridge version that differs
        /// from the daemon or an offline fallback to a cached Cua version.
        #[serde(default)]
        warnings: Vec<String>,
    },
    Screenshot {
        png_base64: String,
    },
    Transfer(TransferOutcome),
    SessionList {
        sessions: Vec<SessionStatus>,
        #[serde(default)]
        compatibility_version: Option<u32>,
    },
    DesktopSize {
        width: u16,
        height: u16,
    },
    CuaAttached {
        session_incarnation: u64,
        bridge_generation: u64,
        runtime_generation: u64,
        attachment_id: u64,
    },
    /// The live viewer is listening. Each address is `http://host:port/`;
    /// the per-start token is carried separately and is redacted in `Debug`.
    ViewerStarted {
        addresses: Vec<String>,
        token: ViewerToken,
        notices: Vec<String>,
    },
    /// A recording action's result. `changed` is `false` when there was
    /// nothing to do (already recording, not recording, mark unchanged).
    RecordingChanged {
        id: Option<String>,
        changed: bool,
        message: String,
    },
    /// The recordings on disk, oldest first, with the kept and unkept
    /// totals and the budget.
    Recordings {
        recordings: Vec<WireRecording>,
        kept_bytes: u64,
        unkept_bytes: u64,
        budget_bytes: u64,
        kept_over_budget: bool,
    },
    /// The result of `Takeover`: the controller before the request, and
    /// whether control changed (`false` when the agent already controlled).
    TakenOver {
        previous: WireController,
        changed: bool,
    },
    Error(WireError),
}

/// The live viewer's per-start access token. It crosses only the owner-only
/// IPC channel; `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewerToken(pub String);

impl std::fmt::Debug for ViewerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ViewerToken(<redacted>)")
    }
}

#[cfg(test)]
mod viewer_tests {
    use super::*;

    #[test]
    fn viewer_started_round_trips_and_redacts_the_token_in_debug() {
        let token = "0123456789abcdef".repeat(4);
        let response = WireResponse::ViewerStarted {
            addresses: vec!["http://127.0.0.1:4000/".into()],
            token: ViewerToken(token.clone()),
            notices: vec![],
        };
        assert!(!format!("{response:?}").contains(&token));
        let json = serde_json::to_string(&response).unwrap_or_default();
        assert!(json.contains(&token));
        let back: WireResponse = serde_json::from_str(&json).unwrap_or(WireResponse::Ack);
        assert!(matches!(back, WireResponse::ViewerStarted { token: t, .. } if t.0 == token));
    }
}

#[cfg(test)]
mod recording_tests {
    use super::*;

    #[test]
    fn connected_defaults_to_not_recording_and_round_trips_states() {
        let old = serde_json::json!({"Connected": {"session": "web"}});
        let back: WireResponse = serde_json::from_value(old).unwrap_or(WireResponse::Ack);
        assert!(matches!(
            back,
            WireResponse::Connected {
                recording: WireRecordingState::Off,
                ..
            }
        ));
        for state in [
            WireRecordingState::On {
                id: "20260101T000000Z-0123abcd".into(),
            },
            WireRecordingState::Failed {
                reason: "disk full".into(),
            },
        ] {
            let json = serde_json::to_value(&state).unwrap_or_default();
            let back: WireRecordingState =
                serde_json::from_value(json).unwrap_or(WireRecordingState::Off);
            assert_eq!(back, state);
        }
    }

    #[test]
    fn session_status_recording_is_optional_on_the_wire() {
        let json = serde_json::json!({
            "id": "a", "name": null, "host": "h", "status": "Live",
            "connected_since": null, "last_activity": null,
        });
        let status: SessionStatus = serde_json::from_value(json).unwrap_or(SessionStatus {
            id: String::new(),
            name: None,
            host: String::new(),
            status: SessionLifecycle::Disconnected,
            connected_since: None,
            last_activity: None,
            recording: Some("x".into()),
            controller: None,
        });
        assert_eq!(status.id, "a");
        assert_eq!(status.recording, None);
        assert_eq!(status.controller, None);
    }

    #[test]
    fn recordings_round_trip() {
        let response = WireResponse::Recordings {
            recordings: vec![WireRecording {
                id: "20260101T000000Z-0123abcd".into(),
                session: "web".into(),
                session_name: Some("web".into()),
                host: "10.0.0.5".into(),
                started_at: "2026-01-01T00:00:00.000Z".into(),
                duration_ms: 5,
                bytes: 10,
                active: false,
                kept: true,
            }],
            kept_bytes: 10,
            unkept_bytes: 0,
            budget_bytes: 20,
            kept_over_budget: false,
        };
        let json = serde_json::to_string(&response).unwrap_or_default();
        let back: WireResponse = serde_json::from_str(&json).unwrap_or(WireResponse::Ack);
        assert!(matches!(back, WireResponse::Recordings { recordings, .. } if recordings[0].kept));
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;

    #[test]
    fn controller_and_taken_over_round_trip() {
        let agent = serde_json::to_value(WireController::agent()).unwrap_or_default();
        assert_eq!(agent, serde_json::json!({"kind": "agent"}));
        let human = WireController {
            kind: WireControllerKind::Human,
            address: Some("127.0.0.1".into()),
            since: Some("2026-01-01T00:00:00Z".into()),
        };
        let response = WireResponse::TakenOver {
            previous: human.clone(),
            changed: true,
        };
        let json = serde_json::to_string(&response).unwrap_or_default();
        let back: WireResponse = serde_json::from_str(&json).unwrap_or(WireResponse::Ack);
        assert!(
            matches!(back, WireResponse::TakenOver { previous, changed: true } if previous == human)
        );
    }
}
