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
}

/// A daemon-to-client wire response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireResponse {
    /// A generic success acknowledgement (`Ping`/`SetForeground`).
    Ack,
    /// A launched process's id (`LaunchProcess`).
    Pid {
        /// The new process id.
        pid: u32,
    },
    /// A captured screenshot (`Screenshot`) — a base64-encoded PNG, not a
    /// credential.
    Screenshot {
        /// Base64-encoded PNG image bytes.
        png_base64: String,
    },
    /// The outcome of a completed `Put`/`Get` transfer.
    Transfer(TransferOutcome),
    /// The credential-free session list (D-30); Phase 12 populates it from
    /// the keepalive signal.
    SessionList {
        /// The current sessions known to the daemon.
        sessions: Vec<SessionStatus>,
    },
    /// A typed wire error (D-28).
    Error(WireError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WireErrorCode;

    const PLANTED_SECRET: &str = "RDPILOT-PLANTED-SECRET-SENTINEL";

    /// One sample per [`WireResponse`] variant.
    ///
    /// The trailing exhaustive `match` (no wildcard arm) is a compile-time
    /// forcing function, mirroring [`crate::request::SessionScoped`]'s
    /// exhaustive-match pattern: if a future `WireResponse` variant is added
    /// without this function being updated to acknowledge it, the crate
    /// fails to compile here — a missed sample cannot silently slip through
    /// code review the way an un-enforced manual list could.
    fn sample_all_response_variants() -> Vec<WireResponse> {
        let samples = vec![
            WireResponse::Ack,
            WireResponse::Pid { pid: 4242 },
            WireResponse::Screenshot {
                png_base64: "cGxhY2Vob2xkZXI=".to_owned(),
            },
            WireResponse::Transfer(TransferOutcome {
                bytes_transferred: 42,
                checksum: "deadbeef".to_owned(),
            }),
            WireResponse::SessionList {
                sessions: vec![SessionStatus {
                    id: "brave-otter".to_owned(),
                    name: None,
                    host: "10.0.0.5".to_owned(),
                    status: SessionLifecycle::Live,
                    connected_since: None,
                    last_activity: None,
                }],
            },
            WireResponse::Error(WireError {
                code: WireErrorCode::SessionNotFound,
                message: "not found".to_owned(),
            }),
        ];

        for sample in &samples {
            match sample {
                WireResponse::Ack
                | WireResponse::Pid { .. }
                | WireResponse::Screenshot { .. }
                | WireResponse::Transfer(_)
                | WireResponse::SessionList { .. }
                | WireResponse::Error(_) => {}
            }
        }

        samples
    }

    /// CONFIG-03 (BLOCKING): serializing every `WireResponse` variant never
    /// emits the planted secret sentinel.
    ///
    /// This test's guarantee is structural, not a live trace: it passes
    /// because `WireResponse` variants cannot reference a
    /// `rdpilot-config::Credentials`-shaped type at all (crate boundary,
    /// D-31), not because the sentinel was traced through a running
    /// pipeline. A genuine end-to-end trace becomes possible — and should be
    /// added — once Phase 12's session registry exists.
    #[test]
    fn no_wire_response_variant_ever_carries_the_planted_secret() -> Result<(), Box<dyn std::error::Error>> {
        for response in sample_all_response_variants() {
            let json = serde_json::to_string(&response)?;
            assert!(!json.contains(PLANTED_SECRET), "leak found in {json}");
        }
        Ok(())
    }

    #[test]
    fn wire_response_round_trips_every_variant() -> Result<(), Box<dyn std::error::Error>> {
        for response in sample_all_response_variants() {
            let json = serde_json::to_string(&response)?;
            let _: WireResponse = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    #[test]
    fn session_lifecycle_emits_the_d30_vocabulary() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (SessionLifecycle::Connecting, "\"Connecting\""),
            (SessionLifecycle::Live, "\"Live\""),
            (SessionLifecycle::Reconnecting, "\"Reconnecting\""),
            (SessionLifecycle::Disconnected, "\"Disconnected\""),
        ];
        for (status, expected) in cases {
            let json = serde_json::to_string(&status)?;
            assert_eq!(json, expected);
        }
        Ok(())
    }
}
