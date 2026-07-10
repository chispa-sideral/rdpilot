//! [`Request`] — the daemon-bound wire verbs, and [`SessionScoped`], the
//! compile-time forcing function behind SESSION-02.
//!
//! Every [`Request`] variant is a struct variant that declares
//! `session: SessionId` as a plain, non-`Option`, no-`#[serde(default)]`
//! field — never merged in from a wrapper struct via serde's field-merging
//! container attribute. Merging a field whose type is an internally-tagged
//! enum that way is a known, still-open serde limitation (serde-rs/serde#1189):
//! it serializes fine but fails to deserialize. Per-variant repetition
//! sidesteps this entirely and gives the simplest possible mental model: one
//! struct field per variant, no macro magic, and the field's required
//! non-`Option`-ness is exactly what produces SESSION-02's hard rejection.
//!
//! [`SessionScoped::session`]'s exhaustive `match` over every `Request`
//! variant is the structural forcing function: a future verb added to this
//! enum without a `session` field fails to compile here, not just a test.

use serde::{Deserialize, Serialize};

use crate::session_id::SessionId;

/// A daemon-bound wire request.
///
/// Wire verb scope for Phase 11 (per the plan's scope decision): the
/// file-transfer verbs (`Put`/`Get`, mandatory per the Phase 10 dependency)
/// plus the primitive-field operational verbs (`Ping`, `Screenshot`,
/// `LaunchProcess`, `SetForeground`). Richer input/perception verbs
/// (mouse/key/UIA/world_state/window+process list) are deferred to Phases
/// 13/14, whose exact wire shape depends on decisions out of Phase 11's
/// boundary (the Anthropic computer-use action schema, D-21; CLI flag
/// design, Phase 13).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    /// Round-trip a sensor ping (mirrors `Session::ping`).
    Ping {
        /// The session to operate on.
        session: SessionId,
    },
    /// Capture a screenshot of the remote desktop (mirrors
    /// `Session::screenshot`).
    Screenshot {
        /// The session to operate on.
        session: SessionId,
    },
    /// Launch a process on the remote machine (mirrors
    /// `Session::launch_process`).
    LaunchProcess {
        /// The session to operate on.
        session: SessionId,
        /// The executable path.
        exe: String,
        /// Optional command-line arguments.
        args: Option<String>,
        /// Optional working directory.
        cwd: Option<String>,
    },
    /// Bring a remote window to the foreground (mirrors
    /// `Session::set_foreground_window`).
    SetForeground {
        /// The session to operate on.
        session: SessionId,
        /// The target window handle.
        hwnd: u64,
    },
    /// Upload a local file to the remote transfer root (mirrors
    /// `Session::upload_file`).
    Put {
        /// The session to operate on.
        session: SessionId,
        /// The local file path to upload.
        local_path: String,
        /// The destination name under the remote transfer root.
        remote_name: String,
    },
    /// Download a file from the remote transfer root (mirrors
    /// `Session::download_file`).
    Get {
        /// The session to operate on.
        session: SessionId,
        /// The source name under the remote transfer root.
        remote_name: String,
        /// The local destination path.
        local_path: String,
    },
}

/// Compile-time forcing function (SESSION-02): a future `Request` variant
/// added without a `session` field is a match-arm error here, not just a
/// runtime test gap.
pub trait SessionScoped {
    /// The session this request targets.
    fn session(&self) -> &SessionId;
}

impl SessionScoped for Request {
    fn session(&self) -> &SessionId {
        match self {
            Request::Ping { session }
            | Request::Screenshot { session }
            | Request::LaunchProcess { session, .. }
            | Request::SetForeground { session, .. }
            | Request::Put { session, .. }
            | Request::Get { session, .. } => session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_verb_rejects_a_missing_session_field() {
        let cases = [
            r#"{"op":"Ping"}"#,
            r#"{"op":"Screenshot"}"#,
            r#"{"op":"LaunchProcess","exe":"x"}"#,
            r#"{"op":"SetForeground","hwnd":1}"#,
            r#"{"op":"Put","local_path":"a","remote_name":"b"}"#,
            r#"{"op":"Get","remote_name":"a","local_path":"b"}"#,
        ];
        for json in cases {
            let result: Result<Request, _> = serde_json::from_str(json);
            assert!(result.is_err(), "expected rejection for {json}");
        }
    }

    #[test]
    fn every_request_verb_accepts_a_present_session_field() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            r#"{"op":"Ping","session":"s"}"#,
            r#"{"op":"Screenshot","session":"s"}"#,
            r#"{"op":"LaunchProcess","session":"s","exe":"x","args":null,"cwd":null}"#,
            r#"{"op":"SetForeground","session":"s","hwnd":1}"#,
            r#"{"op":"Put","session":"s","local_path":"a","remote_name":"b"}"#,
            r#"{"op":"Get","session":"s","remote_name":"a","local_path":"b"}"#,
        ];
        for json in cases {
            let result: Request = serde_json::from_str(json)?;
            assert_eq!(result.session().as_str(), "s");
        }
        Ok(())
    }

    #[test]
    fn session_scoped_returns_the_embedded_id_for_every_variant() -> Result<(), Box<dyn std::error::Error>> {
        let requests: Vec<Request> = vec![
            serde_json::from_str(r#"{"op":"Ping","session":"a"}"#)?,
            serde_json::from_str(r#"{"op":"Screenshot","session":"a"}"#)?,
            serde_json::from_str(r#"{"op":"LaunchProcess","session":"a","exe":"x","args":null,"cwd":null}"#)?,
            serde_json::from_str(r#"{"op":"SetForeground","session":"a","hwnd":1}"#)?,
            serde_json::from_str(r#"{"op":"Put","session":"a","local_path":"l","remote_name":"r"}"#)?,
            serde_json::from_str(r#"{"op":"Get","session":"a","remote_name":"r","local_path":"l"}"#)?,
        ];
        for req in &requests {
            assert_eq!(req.session().as_str(), "a");
        }
        Ok(())
    }
}
