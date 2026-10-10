//! Why a recording starts, and the spellings each surface uses for it.

use serde::{Deserialize, Serialize};

/// What started a recording.
///
/// The serde spelling is snake_case (`"config"`, `"host"`, `"connect_flag"`,
/// `"cli"`, `"viewer"`): the daemon writes it to the event log and the
/// recording manifest ([`as_str`](RecordingTrigger::as_str) gives the same
/// text). A `Connect` request carries only the three triggers a client can
/// name, in a different spelling: see [`connect_record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingTrigger {
    /// `[recording] enabled` in `config.toml`.
    Config,
    /// A `[[recording.hosts]]` entry.
    Host,
    /// `rdpilot connect --record`.
    ConnectFlag,
    /// `rdpilot record start`.
    Cli,
    /// The viewer page's Start recording control.
    Viewer,
}

impl RecordingTrigger {
    /// The manifest spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RecordingTrigger::Config => "config",
            RecordingTrigger::Host => "host",
            RecordingTrigger::ConnectFlag => "connect_flag",
            RecordingTrigger::Cli => "cli",
            RecordingTrigger::Viewer => "viewer",
        }
    }
}

/// The IPC codec of `Connect.record`, for `#[serde(with = "...")]` on an
/// `Option<RecordingTrigger>` field.
///
/// The IPC spellings are `"GlobalConfig"` (for [`RecordingTrigger::Config`]),
/// `"HostConfig"` (for [`RecordingTrigger::Host`]) and `"ConnectFlag"`;
/// `None` is `null`. [`RecordingTrigger::Cli`] and
/// [`RecordingTrigger::Viewer`] start a recording of a connected session and
/// have no IPC spelling: serializing one is an error, and deserializing
/// `"Cli"`, `"Viewer"` or any other text is an error.
pub mod connect_record {
    use serde::ser::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::RecordingTrigger;

    /// The three triggers a `Connect` request can name.
    #[derive(Serialize, Deserialize)]
    enum Spelling {
        GlobalConfig,
        HostConfig,
        ConnectFlag,
    }

    /// Serialize the trigger in its IPC spelling.
    ///
    /// # Errors
    ///
    /// Fails for [`RecordingTrigger::Cli`] and [`RecordingTrigger::Viewer`].
    pub fn serialize<S: Serializer>(
        value: &Option<RecordingTrigger>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let spelling = match value {
            None => None,
            Some(RecordingTrigger::Config) => Some(Spelling::GlobalConfig),
            Some(RecordingTrigger::Host) => Some(Spelling::HostConfig),
            Some(RecordingTrigger::ConnectFlag) => Some(Spelling::ConnectFlag),
            Some(other) => {
                return Err(S::Error::custom(format!(
                    "recording trigger '{}' cannot be sent on connect",
                    other.as_str()
                )))
            }
        };
        spelling.serialize(serializer)
    }

    /// Deserialize the trigger from its IPC spelling.
    ///
    /// # Errors
    ///
    /// Fails for every text other than the three IPC spellings.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<RecordingTrigger>, D::Error> {
        Ok(
            Option::<Spelling>::deserialize(deserializer)?.map(|spelling| match spelling {
                Spelling::GlobalConfig => RecordingTrigger::Config,
                Spelling::HostConfig => RecordingTrigger::Host,
                Spelling::ConnectFlag => RecordingTrigger::ConnectFlag,
            }),
        )
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Holder {
        #[serde(default, with = "connect_record")]
        record: Option<RecordingTrigger>,
    }

    #[test]
    fn the_serde_spelling_matches_as_str() {
        for trigger in [
            RecordingTrigger::Config,
            RecordingTrigger::Host,
            RecordingTrigger::ConnectFlag,
            RecordingTrigger::Cli,
            RecordingTrigger::Viewer,
        ] {
            let json = serde_json::to_string(&trigger).expect("serializes");
            assert_eq!(json, format!("\"{}\"", trigger.as_str()));
        }
    }

    #[test]
    fn the_ipc_codec_round_trips_the_three_client_triggers_and_none() {
        for (trigger, json) in [
            (None, r#"{"record":null}"#),
            (
                Some(RecordingTrigger::Config),
                r#"{"record":"GlobalConfig"}"#,
            ),
            (Some(RecordingTrigger::Host), r#"{"record":"HostConfig"}"#),
            (
                Some(RecordingTrigger::ConnectFlag),
                r#"{"record":"ConnectFlag"}"#,
            ),
        ] {
            let holder = Holder { record: trigger };
            assert_eq!(serde_json::to_string(&holder).expect("serializes"), json);
            assert_eq!(
                serde_json::from_str::<Holder>(json).expect("parses"),
                holder
            );
        }
        let absent: Holder = serde_json::from_str("{}").expect("absent is off");
        assert_eq!(absent.record, None);
    }

    #[test]
    fn the_ipc_codec_rejects_cli_and_viewer_on_serialization() {
        for trigger in [RecordingTrigger::Cli, RecordingTrigger::Viewer] {
            let holder = Holder {
                record: Some(trigger),
            };
            let err = serde_json::to_string(&holder).expect_err("rejected");
            assert!(err.to_string().contains(trigger.as_str()), "{err}");
        }
    }

    #[test]
    fn the_ipc_codec_rejects_cli_viewer_and_the_daemon_spellings_on_deserialization() {
        for text in [
            "Cli",
            "Viewer",
            "config",
            "host",
            "connect_flag",
            "cli",
            "viewer",
            "",
        ] {
            let json = format!(r#"{{"record":"{text}"}}"#);
            assert!(serde_json::from_str::<Holder>(&json).is_err(), "{text}");
        }
        let err = serde_json::from_str::<Holder>(r#"{"record":"Cli"}"#).expect_err("rejected");
        assert!(
            err.to_string().starts_with(
                "unknown variant `Cli`, expected one of `GlobalConfig`, `HostConfig`, `ConnectFlag`"
            ),
            "{err}"
        );
    }
}
