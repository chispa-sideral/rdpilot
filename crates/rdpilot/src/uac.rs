//! Owned SDK types for [`crate::Session::uac_respond`] (UAC/elevation
//! consent-prompt response, ticket BF8Q9K6FGZ2APN8F).
//!
//! Only these owned types leave this crate's public API (D-09) — the
//! Tab-navigate + Unicode-Enter sequence and the decision-specific
//! structural outcome confirmation both live in `session.rs`, never here.

use crate::screenshot::Screenshot;

/// Which way to respond to an active UAC/elevation consent prompt
/// (`Session::uac_respond`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UacDecision {
    /// Approve the prompt (Tab, Tab, Unicode Enter — moves focus No ->
    /// "Show more details" -> Yes, then activates it).
    Approve,
    /// Reject the prompt (one Unicode Enter — activates the default-focused
    /// "No" button).
    Reject,
}

/// The confirmed outcome of a completed [`crate::Session::uac_respond`]
/// call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UacResponseOutcome {
    /// Which decision was requested.
    pub decision: UacDecision,
    /// The confirming screenshot taken after the response sequence
    /// settled — an artifact for the caller/log, per the acceptance's own
    /// literal verification method ("confirmed via a follow-up
    /// screenshot"). NOT the pass/fail signal: `uac_respond`'s outcome
    /// confirmation is a decision-specific structural recheck off the
    /// process tree (see `session.rs`), never a pixel diff.
    pub confirmation: Screenshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `UacDecision` serializes as a bare unit-variant string (matches this
    /// crate's other owned enum conventions, e.g. `UiaScope::Children`).
    #[test]
    fn uac_decision_serializes_as_a_bare_string() {
        assert_eq!(serde_json::to_value(UacDecision::Approve).expect("serializes"), serde_json::json!("Approve"));
        assert_eq!(serde_json::to_value(UacDecision::Reject).expect("serializes"), serde_json::json!("Reject"));
    }

    /// A fully-populated `UacResponseOutcome` serializes without error and
    /// exposes its documented fields.
    #[test]
    fn uac_response_outcome_serializes_with_expected_fields() {
        let outcome = UacResponseOutcome {
            decision: UacDecision::Approve,
            confirmation: Screenshot::from_rgba(1, 1, vec![0, 0, 0, 0]).expect("1x1 rgba is valid"),
        };
        let value = serde_json::to_value(&outcome).expect("UacResponseOutcome serializes");
        let obj = value.as_object().expect("serializes as a JSON object");
        assert_eq!(obj.get("decision").and_then(|v| v.as_str()), Some("Approve"));
        assert!(obj.contains_key("confirmation"));
    }
}
