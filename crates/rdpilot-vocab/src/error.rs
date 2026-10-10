//! The fixed set of error codes on the wire.

use serde::{Deserialize, Serialize};

/// The fixed wire error code set. The spelling is kebab-case.
///
/// `internal` is the catch-all for every SDK error that has no code of its
/// own.
///
/// `#[non_exhaustive]`: a new code is not a breaking change for a client that
/// matches with a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum WireErrorCode {
    /// The requested session id has no live session (daemon-only concept).
    /// Wire string: `session-not-found`.
    SessionNotFound,
    /// The daemon process could not be reached over local IPC (daemon-only
    /// concept). Wire string: `daemon-unreachable`.
    DaemonUnreachable,
    /// A file-transfer (`Put`/`Get`) operation failed for a reason other
    /// than a path-traversal rejection or a checksum mismatch. Wire string:
    /// `transfer-failed`.
    TransferFailed,
    /// A path-traversal rejection; maps one-to-one from the SDK error.
    /// Wire string: `path-traversal`.
    PathTraversal,
    /// A checksum mismatch; maps one-to-one from the SDK error. Wire string:
    /// `checksum-mismatch`.
    ChecksumMismatch,
    /// Catch-all for every SDK error without a code of its own. Wire string:
    /// `internal`.
    Internal,
    /// A `Connect` request's caller-supplied or generated session name
    /// collided with a live or connecting session (daemon-only concept).
    /// Wire string: `duplicate-session`.
    DuplicateSession,
    /// A recording request that cannot be done: the session is not
    /// recording, the annotation is empty or over 4 KiB, the recording is
    /// busy, or the recording id is unknown. Wire string: `recording`.
    Recording,
    /// A `Connect` with Cua enabled failed before the RDP logon because the
    /// rdpilot-bridge or the Cua driver could not be obtained or verified
    /// (daemon-only concept). The message names the component, version,
    /// architecture, cause and fixes. Wire string: `bundle-unavailable`.
    BundleUnavailable,
    /// Agent input (native Mouse or Key) was refused because a human viewer
    /// holds the session's control lease. The message names the holder and
    /// the takeover command; the wire error's `controller` field carries the
    /// facts. Wire string: `human-control`.
    HumanControl,
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODES: [(WireErrorCode, &str); 10] = [
        (WireErrorCode::SessionNotFound, "session-not-found"),
        (WireErrorCode::DaemonUnreachable, "daemon-unreachable"),
        (WireErrorCode::TransferFailed, "transfer-failed"),
        (WireErrorCode::PathTraversal, "path-traversal"),
        (WireErrorCode::ChecksumMismatch, "checksum-mismatch"),
        (WireErrorCode::Internal, "internal"),
        (WireErrorCode::DuplicateSession, "duplicate-session"),
        (WireErrorCode::Recording, "recording"),
        (WireErrorCode::BundleUnavailable, "bundle-unavailable"),
        (WireErrorCode::HumanControl, "human-control"),
    ];

    #[test]
    fn every_code_round_trips_through_its_kebab_case_string() {
        for (code, text) in CODES {
            let json = format!("\"{text}\"");
            assert_eq!(
                serde_json::to_string(&code).ok().as_deref(),
                Some(json.as_str())
            );
            assert_eq!(
                serde_json::from_str::<WireErrorCode>(&json).ok(),
                Some(code)
            );
        }
    }
}
