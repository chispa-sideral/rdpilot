//! Wire mirror of `rdpilot::UacDecision` (UAC/elevation consent-prompt
//! response, ticket BF8Q9K6FGZ2APN8F) — deliberately duplicates the SDK
//! type's field shape rather than importing it (the `transfer.rs`
//! convention, Decision 1): `rdpilot-ipc` must never depend on `rdpilot`.

use serde::{Deserialize, Serialize};

/// Wire mirror of `rdpilot::UacDecision` — which way to respond to an
/// active UAC/elevation consent prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireUacDecision {
    /// Approve the prompt.
    Approve,
    /// Reject the prompt.
    Reject,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_uac_decision_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        for decision in [WireUacDecision::Approve, WireUacDecision::Reject] {
            let json = serde_json::to_string(&decision)?;
            let parsed: WireUacDecision = serde_json::from_str(&json)?;
            assert_eq!(parsed, decision);
        }
        Ok(())
    }
}
