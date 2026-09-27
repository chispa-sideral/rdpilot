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
    Error(WireError),
}
