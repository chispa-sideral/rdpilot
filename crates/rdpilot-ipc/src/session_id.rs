//! [`SessionId`] — the wire-level session identifier every session-scoped
//! [`crate::Request`] variant embeds.
//!
//! Serializes as a bare JSON string (`#[serde(transparent)]`), and
//! deserializes via a hand-written `Deserialize` that rejects the empty
//! string — closing the trivial `"session": ""` bypass around the
//! non-`Option`-field required-field guarantee SESSION-02 demands (a client
//! could otherwise satisfy "field is present" while supplying no real
//! session identity).

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A wire-level session identifier.
///
/// Never empty (enforced at deserialize time — see [`FromStr::from_str`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// The identifier's string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SessionId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            Err("session id must not be empty".to_owned())
        } else {
            Ok(SessionId(s.to_owned()))
        }
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_string() -> Result<(), Box<dyn std::error::Error>> {
        let result: Result<SessionId, _> = serde_json::from_str("\"\"");
        assert!(result.is_err(), "empty session id must be rejected");
        Ok(())
    }

    #[test]
    fn accepts_non_empty_string() -> Result<(), Box<dyn std::error::Error>> {
        let id: SessionId = serde_json::from_str("\"brave-otter\"")?;
        assert_eq!(id.as_str(), "brave-otter");
        Ok(())
    }

    #[test]
    fn serializes_as_bare_string_not_object() -> Result<(), Box<dyn std::error::Error>> {
        let id = SessionId::from_str("x")?;
        let json = serde_json::to_string(&id)?;
        assert_eq!(json, "\"x\"");
        Ok(())
    }
}
