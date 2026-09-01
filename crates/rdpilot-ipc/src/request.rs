//! [`Request`] — the daemon-bound wire verbs, and [`SessionScoped`], the
//! compile-time forcing function behind SESSION-02.
//!
//! Every session-scoped [`Request`] variant is a struct variant that
//! declares `session: SessionId` as a plain, non-`Option`, no-`#[serde(default)]`
//! field — never merged in from a wrapper struct via serde's field-merging
//! container attribute. Merging a field whose type is an internally-tagged
//! enum that way is a known, still-open serde limitation (serde-rs/serde#1189):
//! it serializes fine but fails to deserialize. Per-variant repetition
//! sidesteps this entirely and gives the simplest possible mental model: one
//! struct field per variant, no macro magic, and the field's required
//! non-`Option`-ness is exactly what produces SESSION-02's hard rejection.
//!
//! [`Request::Connect`] and [`Request::List`] are the two intentionally
//! session-**less** verbs (Phase 12, SESSION-01/SESSION-03): `Connect`
//! targets no existing session (it creates one), and `List` targets no
//! single session at all. Every other verb — including [`Request::Disconnect`]
//! — remains required-session.
//!
//! [`SessionScoped::session`]'s exhaustive `match` over every `Request`
//! variant is the structural forcing function: a future verb added to this
//! enum without an explicit arm fails to compile here, not just a test. The
//! match's return type is `Option<&SessionId>` — `Some` for every
//! session-targeting verb, `None` for `Connect`/`List`.

use serde::{Deserialize, Serialize};

use crate::input::{WireKeyAction, WireMouseAction};
use crate::perception::{WireUiaScope, WireWorldStateOptions};
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
///
/// Phase 12 extends this enum with the three session-lifecycle verbs Phase
/// 11 deliberately left out of scope: `Connect`, `List`, `Disconnect`
/// (SESSION-01/03/04; D-29/D-30 already delegate ownership of the session-
/// identity and status-vocabulary *types* in this crate to Phase 12, and
/// `WireErrorCode` is `#[non_exhaustive]` for exactly this kind of
/// phase-scoped extension).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    /// Open a new session against a remote RDP target (D-29, SESSION-01).
    ///
    /// `name` is `Option`: a caller-supplied name reserves that exact id; a
    /// caller that supplies `None` receives a short human-legible
    /// adjective-noun auto-generated id (e.g. `brave-otter`) minted by the
    /// daemon's registry. This is the one wire verb that carries NO
    /// `session` field — there is no existing session to target yet.
    Connect {
        /// Optional caller-supplied session name. `None` requests an
        /// auto-generated id.
        name: Option<String>,
        /// Hostname or IP address of the RDP target.
        host: String,
        /// Optional TCP port (daemon applies `rdpilot`'s default when
        /// omitted).
        port: Option<u16>,
        /// Username for NLA/CredSSP authentication.
        username: String,
        /// Password for NLA/CredSSP authentication.
        ///
        /// This is the ONE legitimate plaintext-credential carrier on the
        /// wire (D-31: supplied once to the daemon at connect time). It
        /// MUST NOT be logged raw by any consumer — the daemon (Plan
        /// 12-06/its logging) is responsible for redaction at every log
        /// site; this crate adds no `Debug`/`Display` impl that would widen
        /// exposure beyond the plain derive already required for the
        /// struct variant.
        password: String,
        /// Optional Windows domain.
        domain: Option<String>,
        /// When `true`, the server certificate is accepted without
        /// validation (D-15 risk-named passthrough).
        accept_invalid_certs: bool,
        /// Opt in to the same-stream ownership acknowledgement used by
        /// current clients. Missing fields preserve legacy clients.
        #[serde(default)]
        connect_ack: bool,
    },
    /// Accept a capability-negotiated successful Connect response.
    ConnectAck {
        /// The session returned by the preceding Connected response.
        session: SessionId,
    },
    /// List the sessions currently known to the daemon (D-30, SESSION-03).
    ///
    /// Carries no fields and no `session` — it targets no single existing
    /// session.
    List {},
    /// Disconnect a named/identified session (SESSION-04).
    ///
    /// Remains a session-SCOPED verb: `session` is a required, non-`Option`
    /// field exactly like every Phase 11 operational verb (SESSION-02
    /// preserved) — omitting it is a hard `serde_json` deserialize error.
    Disconnect {
        /// The session to disconnect.
        session: SessionId,
    },
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
    /// Fetch the top-level window list (mirrors `Session::get_window_list`).
    WindowList {
        /// The session to operate on.
        session: SessionId,
    },
    /// Fetch the remote process tree (mirrors `Session::get_process_tree`).
    ProcessList {
        /// The session to operate on.
        session: SessionId,
    },
    /// Fetch a UI Automation tree for one window (mirrors
    /// `Session::get_uia_tree`).
    Uia {
        /// The session to operate on.
        session: SessionId,
        /// The target window handle.
        hwnd: u64,
        /// How deep to walk the UIA tree.
        scope: WireUiaScope,
    },
    /// Fetch a correlated desktop snapshot (mirrors `Session::world_state`).
    WorldState {
        /// The session to operate on.
        session: SessionId,
        /// Which components to capture.
        options: WireWorldStateOptions,
    },
    /// Send a mouse action (mirrors `Session::send_mouse`).
    Mouse {
        /// The session to operate on.
        session: SessionId,
        /// The mouse action to perform.
        action: WireMouseAction,
    },
    /// Send a keyboard action (mirrors `Session::send_key`).
    Key {
        /// The session to operate on.
        session: SessionId,
        /// The keyboard action to perform.
        action: WireKeyAction,
    },
    /// Fetch a session's native desktop dimensions (mirrors
    /// `Session::desktop_size`; research recommendation, MCP-04's
    /// coordinate bridge sources native dims over the wire rather than by
    /// sniffing screenshot PNG headers).
    DesktopSize {
        /// The session to operate on.
        session: SessionId,
    },
}

/// Compile-time forcing function (SESSION-02): a future `Request` variant
/// added without an explicit arm is a match-arm error here, not just a
/// runtime test gap.
pub trait SessionScoped {
    /// The session this request targets, or `None` for the two intentionally
    /// session-less verbs (`Connect`, `List`).
    fn session(&self) -> Option<&SessionId>;
}

impl SessionScoped for Request {
    fn session(&self) -> Option<&SessionId> {
        match self {
            Request::Connect { .. } | Request::List {} => None,
            Request::ConnectAck { session }
            | Request::Disconnect { session }
            | Request::Ping { session }
            | Request::Screenshot { session }
            | Request::LaunchProcess { session, .. }
            | Request::SetForeground { session, .. }
            | Request::Put { session, .. }
            | Request::Get { session, .. }
            | Request::WindowList { session }
            | Request::ProcessList { session }
            | Request::Uia { session, .. }
            | Request::WorldState { session, .. }
            | Request::Mouse { session, .. }
            | Request::Key { session, .. }
            | Request::DesktopSize { session } => Some(session),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operational_request_verb_rejects_a_missing_session_field() {
        let cases = [
            r#"{"op":"Ping"}"#,
            r#"{"op":"Screenshot"}"#,
            r#"{"op":"LaunchProcess","exe":"x"}"#,
            r#"{"op":"SetForeground","hwnd":1}"#,
            r#"{"op":"Put","local_path":"a","remote_name":"b"}"#,
            r#"{"op":"Get","remote_name":"a","local_path":"b"}"#,
            r#"{"op":"WindowList"}"#,
            r#"{"op":"ProcessList"}"#,
            r#"{"op":"Uia","hwnd":1,"scope":"Children"}"#,
            r#"{"op":"WorldState","options":{"screenshot":true,"window_list":true,"uia":"None"}}"#,
            r#"{"op":"Mouse","action":{"Move":{"x":1,"y":2}}}"#,
            r#"{"op":"Key","action":{"Type":"hi"}}"#,
            r#"{"op":"DesktopSize"}"#,
        ];
        for json in cases {
            let result: Result<Request, _> = serde_json::from_str(json);
            assert!(result.is_err(), "expected rejection for {json}");
        }
    }

    #[test]
    fn every_operational_request_verb_accepts_a_present_session_field() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            r#"{"op":"Ping","session":"s"}"#,
            r#"{"op":"Screenshot","session":"s"}"#,
            r#"{"op":"LaunchProcess","session":"s","exe":"x","args":null,"cwd":null}"#,
            r#"{"op":"SetForeground","session":"s","hwnd":1}"#,
            r#"{"op":"Put","session":"s","local_path":"a","remote_name":"b"}"#,
            r#"{"op":"Get","session":"s","remote_name":"a","local_path":"b"}"#,
            r#"{"op":"WindowList","session":"s"}"#,
            r#"{"op":"ProcessList","session":"s"}"#,
            r#"{"op":"Uia","session":"s","hwnd":1,"scope":"Children"}"#,
            r#"{"op":"WorldState","session":"s","options":{"screenshot":true,"window_list":true,"uia":"None"}}"#,
            r#"{"op":"Mouse","session":"s","action":{"Move":{"x":1,"y":2}}}"#,
            r#"{"op":"Key","session":"s","action":{"Type":"hi"}}"#,
            r#"{"op":"DesktopSize","session":"s"}"#,
        ];
        for json in cases {
            let result: Request = serde_json::from_str(json)?;
            assert_eq!(result.session().map(SessionId::as_str), Some("s"));
        }
        Ok(())
    }

    #[test]
    fn session_scoped_returns_the_embedded_id_for_every_operational_variant() -> Result<(), Box<dyn std::error::Error>>
    {
        let requests: Vec<Request> = vec![
            serde_json::from_str(r#"{"op":"Ping","session":"a"}"#)?,
            serde_json::from_str(r#"{"op":"Screenshot","session":"a"}"#)?,
            serde_json::from_str(r#"{"op":"LaunchProcess","session":"a","exe":"x","args":null,"cwd":null}"#)?,
            serde_json::from_str(r#"{"op":"SetForeground","session":"a","hwnd":1}"#)?,
            serde_json::from_str(r#"{"op":"Put","session":"a","local_path":"l","remote_name":"r"}"#)?,
            serde_json::from_str(r#"{"op":"Get","session":"a","remote_name":"r","local_path":"l"}"#)?,
            serde_json::from_str(r#"{"op":"WindowList","session":"a"}"#)?,
            serde_json::from_str(r#"{"op":"ProcessList","session":"a"}"#)?,
            serde_json::from_str(r#"{"op":"Uia","session":"a","hwnd":1,"scope":"Children"}"#)?,
            serde_json::from_str(
                r#"{"op":"WorldState","session":"a","options":{"screenshot":true,"window_list":true,"uia":"None"}}"#,
            )?,
            serde_json::from_str(r#"{"op":"Mouse","session":"a","action":{"Move":{"x":1,"y":2}}}"#)?,
            serde_json::from_str(r#"{"op":"Key","session":"a","action":{"Type":"hi"}}"#)?,
            serde_json::from_str(r#"{"op":"DesktopSize","session":"a"}"#)?,
        ];
        for req in &requests {
            assert_eq!(req.session().map(SessionId::as_str), Some("a"));
        }
        Ok(())
    }

    #[test]
    fn connect_without_a_session_field_is_accepted_and_session_is_none() -> Result<(), Box<dyn std::error::Error>> {
        let json = r#"{"op":"Connect","host":"h","username":"u","password":"p","name":null,"port":null,"domain":null,"accept_invalid_certs":false}"#;
        let req: Request = serde_json::from_str(json)?;
        assert!(req.session().is_none());
        assert!(matches!(req, Request::Connect { connect_ack: false, .. }));
        Ok(())
    }

    #[test]
    fn connect_ack_is_session_scoped() -> Result<(), Box<dyn std::error::Error>> {
        let req: Request = serde_json::from_str(r#"{"op":"ConnectAck","session":"s"}"#)?;
        assert_eq!(req.session().map(SessionId::as_str), Some("s"));
        Ok(())
    }

    #[test]
    fn list_without_a_session_field_is_accepted_and_session_is_none() -> Result<(), Box<dyn std::error::Error>> {
        let json = r#"{"op":"List"}"#;
        let req: Request = serde_json::from_str(json)?;
        assert!(req.session().is_none());
        assert!(matches!(req, Request::List {}));
        Ok(())
    }

    #[test]
    fn disconnect_with_a_session_field_is_accepted_and_session_is_some() -> Result<(), Box<dyn std::error::Error>> {
        let json = r#"{"op":"Disconnect","session":"s"}"#;
        let req: Request = serde_json::from_str(json)?;
        assert_eq!(req.session().map(SessionId::as_str), Some("s"));
        assert!(matches!(req, Request::Disconnect { .. }));
        Ok(())
    }

    #[test]
    fn disconnect_without_a_session_field_is_a_hard_rejection() {
        let result: Result<Request, _> = serde_json::from_str(r#"{"op":"Disconnect"}"#);
        assert!(result.is_err(), "Disconnect must hard-reject a missing session (SESSION-02)");
    }
}
