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
use crate::perception::{WireProcessInfo, WireUiaElement, WireWindowInfo};
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
}

/// A daemon-to-client wire response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireResponse {
    /// A generic success acknowledgement (`Ping`/`SetForeground`).
    Ack,
    /// A successful `Connect` (D-29): carries the (possibly
    /// auto-generated) [`SessionId`] the client should target in every
    /// subsequent session-scoped request.
    Connected {
        /// The newly opened session's id.
        session: SessionId,
        /// Whether this client must send the matching `ConnectAck` before
        /// daemon ownership is transferred.
        #[serde(default)]
        connect_ack_required: bool,
        /// Whether the configured sensor completed bootstrap and answered a
        /// ping before this response was sent. `false` means the connection
        /// is usable for RDP-only capabilities but no live sensor was
        /// configured; a configured sensor that fails bootstrap makes
        /// `Connect` fail instead of returning this response.
        #[serde(default)]
        sensor_live: bool,
    },
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
    /// The top-level window list (`WindowList`).
    WindowList {
        /// The current top-level windows.
        windows: Vec<WireWindowInfo>,
    },
    /// The remote process tree (`ProcessList`).
    ProcessList {
        /// The current processes.
        processes: Vec<WireProcessInfo>,
    },
    /// A UI Automation tree walk result (`Uia`).
    Uia {
        /// The flat list of UIA elements returned by the walk.
        elements: Vec<WireUiaElement>,
    },
    /// A correlated desktop snapshot (`WorldState`).
    WorldState {
        /// ISO-8601 batch timestamp, taken once after all sequenced
        /// component fetches complete. `SystemTime` has no serde impl —
        /// converted daemon-side (Plan 13-04), never here.
        timestamp: String,
        /// The measured wall-clock elapsed across the sequenced component
        /// fetches, in milliseconds. `Duration` has no serde impl either —
        /// converted daemon-side, never here.
        capture_span_ms: u64,
        /// The full-desktop screenshot, base64-encoded PNG, if requested —
        /// same convention as [`WireResponse::Screenshot`].
        screenshot: Option<String>,
        /// The top-level window list, if requested.
        window_list: Option<Vec<WireWindowInfo>>,
        /// UIA trees grouped by originating window handle, if requested.
        uia: Option<Vec<(u64, Vec<WireUiaElement>)>>,
    },
    /// A session's native desktop dimensions (`DesktopSize`; mirrors
    /// `Session::desktop_size`). `u16` because RDP desktop dimensions never
    /// exceed 65535 and the SDK/config width/height fields are already
    /// `u16`.
    DesktopSize {
        /// Native desktop width, in pixels.
        width: u16,
        /// Native desktop height, in pixels.
        height: u16,
    },
    /// A typed wire error (D-28).
    Error(WireError),
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::error::WireErrorCode;
    use crate::perception::{WireRect, WireWindowState};

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
            WireResponse::Connected {
                session: SessionId::from_str("brave-otter").unwrap_or_else(|_| unreachable!()),
                connect_ack_required: false,
                sensor_live: true,
            },
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
            WireResponse::WindowList {
                windows: vec![WireWindowInfo {
                    hwnd: 65536,
                    title: "Notepad".to_owned(),
                    rect: WireRect { x: 0, y: 0, w: 100, h: 100 },
                    z_order: 0,
                    state: WireWindowState::Normal,
                    class_name: "Notepad".to_owned(),
                    pid: 4242,
                }],
            },
            WireResponse::ProcessList {
                processes: vec![WireProcessInfo {
                    pid: 4242,
                    parent_pid: 4,
                    name: "notepad.exe".to_owned(),
                    path: "C:\\Windows\\notepad.exe".to_owned(),
                    command_line: None,
                    owner: None,
                }],
            },
            WireResponse::Uia {
                elements: vec![WireUiaElement {
                    id: "1.2.3".to_owned(),
                    role: "Button".to_owned(),
                    name: "OK".to_owned(),
                    bbox: WireRect { x: 0, y: 0, w: 10, h: 10 },
                    enabled: true,
                    visible: true,
                    focusable: true,
                    focused: false,
                    depth: 1,
                    parent_id: "1.2".to_owned(),
                }],
            },
            WireResponse::WorldState {
                timestamp: "2026-07-11T00:00:00Z".to_owned(),
                capture_span_ms: 42,
                screenshot: Some("cGxhY2Vob2xkZXI=".to_owned()),
                window_list: None,
                uia: None,
            },
            WireResponse::DesktopSize { width: 1920, height: 1080 },
            WireResponse::Error(WireError {
                code: WireErrorCode::SessionNotFound,
                message: "not found".to_owned(),
            }),
        ];

        for sample in &samples {
            match sample {
                WireResponse::Ack
                | WireResponse::Connected { .. }
                | WireResponse::Pid { .. }
                | WireResponse::Screenshot { .. }
                | WireResponse::Transfer(_)
                | WireResponse::SessionList { .. }
                | WireResponse::WindowList { .. }
                | WireResponse::ProcessList { .. }
                | WireResponse::Uia { .. }
                | WireResponse::WorldState { .. }
                | WireResponse::DesktopSize { .. }
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
            (SessionLifecycle::Orphaned, "\"Orphaned\""),
        ];
        for (status, expected) in cases {
            let json = serde_json::to_string(&status)?;
            assert_eq!(json, expected);
        }
        Ok(())
    }
}
