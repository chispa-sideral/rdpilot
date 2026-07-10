//! [`WireError`] / [`WireErrorCode`] — the D-28 wire error taxonomy.
//!
//! Only the *types* live here (Decision 1, Phase 11 scope): the
//! `rdpilot::Error -> WireErrorCode` mapping is intentionally NOT defined in
//! this crate. Implementing it as `impl From<&rdpilot::Error> for
//! WireErrorCode` would force `rdpilot-ipc` — and therefore every CLI/MCP
//! client binary that links it — to depend on `rdpilot`, pulling in
//! IronRDP/rustls/tokio-full and breaking the thin-client invariant (D-17).
//! That mapping function lives in the Phase 12 daemon crate instead, which
//! is the only consumer that legitimately depends on both `rdpilot` and
//! `rdpilot-ipc`.

use serde::{Deserialize, Serialize};

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
}

/// The D-28 fixed wire error code set, plus an `internal` catch-all
/// (Decision 3) for the ~9 SDK [`rdpilot::Error`] variants outside D-28's
/// five fixed codes (e.g. `Connect`, `Tls`, `Decode`, `Session`,
/// `CoordinateOutOfBounds`, `Dvc`, `Bootstrap`, `SensorRejected`, `Config`).
///
/// `#[non_exhaustive]`: Phase 11 ships exactly these six variants; a future
/// phase may need a more granular code without this being a breaking wire
/// change for existing consumers matching exhaustively today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum WireErrorCode {
    /// The requested session id has no corresponding live session
    /// (daemon-only concept — no `rdpilot::Error` equivalent).
    /// Wire string: `session-not-found`.
    SessionNotFound,
    /// The daemon process could not be reached over local IPC (daemon-only
    /// concept — no `rdpilot::Error` equivalent). Wire string:
    /// `daemon-unreachable`.
    DaemonUnreachable,
    /// A file-transfer (`Put`/`Get`) operation failed for a reason other
    /// than a path-traversal rejection or a checksum mismatch. Wire string:
    /// `transfer-failed`.
    TransferFailed,
    /// Maps 1:1 from `rdpilot::Error::PathTraversal`. Wire string:
    /// `path-traversal`.
    PathTraversal,
    /// Maps 1:1 from `rdpilot::Error::ChecksumMismatch`. Wire string:
    /// `checksum-mismatch`.
    ChecksumMismatch,
    /// Catch-all (Decision 3) for every `rdpilot::Error` variant outside the
    /// five fixed D-28 codes above. Wire string: `internal`.
    Internal,
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
        ] {
            let code: WireErrorCode = serde_json::from_str(json)?;
            let round_tripped = serde_json::to_string(&code)?;
            assert_eq!(round_tripped, json);
        }
        Ok(())
    }

    #[test]
    fn wire_error_display_includes_code_and_message() {
        let err = WireError {
            code: WireErrorCode::SessionNotFound,
            message: "no such session".to_owned(),
        };
        let rendered = format!("{err}");
        assert!(rendered.contains("SessionNotFound"));
        assert!(rendered.contains("no such session"));
    }
}
