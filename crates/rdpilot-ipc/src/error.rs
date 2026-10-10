//! [`WireError`] — the wire error: a fixed [`WireErrorCode`] plus a message.
//!
//! The code set itself lives in `rdpilot-vocab` and is re-exported here. The
//! `rdpilot::Error -> WireErrorCode` mapping is not defined in this crate:
//! `impl From<&rdpilot::Error> for WireErrorCode` would force `rdpilot-ipc`,
//! and so every CLI and MCP client binary that links it, to depend on
//! `rdpilot` and pull in IronRDP, rustls and tokio-full. That mapping lives in
//! the daemon crate, the only consumer that depends on both `rdpilot` and
//! `rdpilot-ipc`.

pub use rdpilot_vocab::WireErrorCode;
use serde::{Deserialize, Serialize};

use crate::response::WireController;

/// A wire-level error: a fixed [`WireErrorCode`] plus a human-readable
/// message.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct WireError {
    /// The fixed error class.
    pub code: WireErrorCode,
    /// A human-readable description. Never contains a credential (D-31) —
    /// callers constructing this from an SDK error must not embed secret
    /// material in the message.
    pub message: String,
    /// For [`WireErrorCode::HumanControl`]: the human controller (kind,
    /// address, since). Absent for every other code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<WireController>,
}

impl WireError {
    /// An error without structured controller facts.
    #[must_use]
    pub fn new(code: WireErrorCode, message: impl Into<String>) -> Self {
        WireError {
            code,
            message: message.into(),
            controller: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_error_code_emits_exact_kebab_case_strings() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (WireErrorCode::SessionNotFound, "\"session-not-found\""),
            (WireErrorCode::DaemonUnreachable, "\"daemon-unreachable\""),
            (WireErrorCode::TransferFailed, "\"transfer-failed\""),
            (WireErrorCode::PathTraversal, "\"path-traversal\""),
            (WireErrorCode::ChecksumMismatch, "\"checksum-mismatch\""),
            (WireErrorCode::Internal, "\"internal\""),
            (WireErrorCode::DuplicateSession, "\"duplicate-session\""),
            (WireErrorCode::Recording, "\"recording\""),
            (WireErrorCode::BundleUnavailable, "\"bundle-unavailable\""),
            (WireErrorCode::HumanControl, "\"human-control\""),
        ];
        for (code, expected) in cases {
            let json = serde_json::to_string(&code)?;
            assert_eq!(json, expected);
        }
        Ok(())
    }

    #[test]
    fn wire_error_code_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        for json in [
            "\"session-not-found\"",
            "\"daemon-unreachable\"",
            "\"transfer-failed\"",
            "\"path-traversal\"",
            "\"checksum-mismatch\"",
            "\"internal\"",
            "\"duplicate-session\"",
            "\"recording\"",
            "\"bundle-unavailable\"",
            "\"human-control\"",
        ] {
            let code: WireErrorCode = serde_json::from_str(json)?;
            let round_tripped = serde_json::to_string(&code)?;
            assert_eq!(round_tripped, json);
        }
        Ok(())
    }

    #[test]
    fn wire_error_display_includes_code_and_message() {
        let err = WireError::new(WireErrorCode::SessionNotFound, "no such session");
        let rendered = format!("{err}");
        assert!(rendered.contains("SessionNotFound"));
        assert!(rendered.contains("no such session"));
    }

    #[test]
    fn human_control_error_carries_the_controller_and_older_errors_parse(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let err = WireError {
            code: WireErrorCode::HumanControl,
            message: "controlled".into(),
            controller: Some(WireController {
                kind: crate::WireControllerKind::Human,
                address: Some("100.64.0.9".into()),
                since: Some("2026-01-01T14:02:07Z".into()),
            }),
        };
        let json = serde_json::to_value(&err)?;
        assert_eq!(json["code"], "human-control");
        assert_eq!(json["controller"]["kind"], "human");
        assert_eq!(json["controller"]["address"], "100.64.0.9");
        let back: WireError = serde_json::from_value(json)?;
        assert_eq!(back.controller, err.controller);
        let plain = serde_json::to_value(WireError::new(WireErrorCode::Internal, "x"))?;
        assert!(plain.get("controller").is_none());
        let old: WireError = serde_json::from_str(r#"{"code":"internal","message":"x"}"#)?;
        assert!(old.controller.is_none());
        Ok(())
    }
}
