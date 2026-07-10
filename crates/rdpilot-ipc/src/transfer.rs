//! [`TransferOutcome`] — the owned wire mirror of `rdpilot::TransferOutcome`.
//!
//! Deliberately duplicates `rdpilot::TransferOutcome`'s field shape rather
//! than importing it: `rdpilot-ipc` must never depend on `rdpilot` (Decision
//! 1), so every wire DTO that corresponds to an SDK type is its own owned
//! mirror, kept in sync by convention/tests, not by a shared type.

use serde::{Deserialize, Serialize};

/// The outcome of a completed `Put`/`Get` file-transfer wire operation —
/// mirrors `rdpilot::TransferOutcome { bytes_transferred, checksum }`
/// exactly (10-04-SUMMARY.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferOutcome {
    /// The number of bytes transferred.
    pub bytes_transferred: u64,
    /// The SHA-256 digest (lowercase hex, no separators) of the transferred
    /// file, independently verified on the SDK side (D-10.5).
    pub checksum: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let outcome = TransferOutcome {
            bytes_transferred: 42,
            checksum: "deadbeef".to_owned(),
        };
        let json = serde_json::to_string(&outcome)?;
        let parsed: TransferOutcome = serde_json::from_str(&json)?;
        assert_eq!(parsed, outcome);
        Ok(())
    }
}
