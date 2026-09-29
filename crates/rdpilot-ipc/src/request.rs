//! Explicitly target-bound daemon management, native recovery and transfer requests.
use crate::{SessionId, WireKeyAction, WireMouseAction};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    Connect {
        name: Option<String>,
        host: String,
        port: Option<u16>,
        username: String,
        password: String,
        domain: Option<String>,
        accept_invalid_certs: bool,
        /// Deploy the Cua bridge for this session. Required (no default): an
        /// older daemon would drop it and deploy the bridge for `CuaEnabled no`.
        cua_enabled: bool,
        #[serde(default)]
        connect_ack: bool,
    },
    ConnectAck {
        session: SessionId,
    },
    List {},
    Disconnect {
        session: SessionId,
    },
    Ping {
        session: SessionId,
    },
    Screenshot {
        session: SessionId,
    },
    Mouse {
        session: SessionId,
        action: WireMouseAction,
    },
    Key {
        session: SessionId,
        action: WireKeyAction,
    },
    DesktopSize {
        session: SessionId,
    },
    Put {
        session: SessionId,
        local_path: String,
        remote_name: String,
    },
    Get {
        session: SessionId,
        remote_name: String,
        local_path: String,
    },
    /// Upgrade the authenticated connection to an opaque Cua MCP stream.
    CuaAttach {
        session: SessionId,
    },
    /// Start the read-only live viewer. The listener lives only while this
    /// IPC connection stays open; closing it stops the viewer.
    ViewerStart {
        bind: WireViewerBind,
        /// Explicit Tailscale IPv4 address; `None` detects it.
        #[serde(default)]
        tailnet_address: Option<String>,
    },
}
/// Which addresses the live viewer binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireViewerBind {
    /// `127.0.0.1` only.
    Loopback,
    /// `127.0.0.1` plus this host's Tailscale address, when one is found.
    LoopbackAndTailnet,
}
pub trait SessionScoped {
    fn session(&self) -> Option<&SessionId>;
}
impl SessionScoped for Request {
    fn session(&self) -> Option<&SessionId> {
        match self {
            Self::Connect { .. } | Self::List {} | Self::ViewerStart { .. } => None,
            Self::ConnectAck { session }
            | Self::Disconnect { session }
            | Self::Ping { session }
            | Self::Screenshot { session }
            | Self::Mouse { session, .. }
            | Self::Key { session, .. }
            | Self::DesktopSize { session }
            | Self::Put { session, .. }
            | Self::Get { session, .. }
            | Self::CuaAttach { session } => Some(session),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operational_requests_require_a_session() {
        for op in [
            "ConnectAck",
            "Disconnect",
            "Ping",
            "Screenshot",
            "Mouse",
            "Key",
            "DesktopSize",
            "Put",
            "Get",
            "CuaAttach",
        ] {
            assert!(serde_json::from_value::<Request>(serde_json::json!({"op":op})).is_err());
        }
    }
    #[test]
    #[allow(clippy::unwrap_used)]
    fn connect_round_trips_cua_enabled_and_requires_it() {
        let json = serde_json::json!({
            "op": "Connect", "name": null, "host": "h", "port": null,
            "username": "u", "password": "p", "domain": null,
            "accept_invalid_certs": false, "cua_enabled": false,
        });
        let req: Request = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(
            req,
            Request::Connect {
                cua_enabled: false,
                ..
            }
        ));
        assert_eq!(serde_json::to_value(&req).unwrap()["cua_enabled"], false);
        let mut missing = json;
        missing.as_object_mut().unwrap().remove("cua_enabled");
        assert!(serde_json::from_value::<Request>(missing).is_err());
    }
    #[test]
    #[allow(clippy::unwrap_used)]
    fn viewer_start_round_trips_and_is_not_session_scoped() {
        let req = Request::ViewerStart {
            bind: WireViewerBind::LoopbackAndTailnet,
            tailnet_address: Some("100.64.0.1".into()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["op"], "ViewerStart");
        let back: Request = serde_json::from_value(json).unwrap();
        assert!(back.session().is_none());
        assert!(matches!(
            back,
            Request::ViewerStart {
                bind: WireViewerBind::LoopbackAndTailnet,
                tailnet_address: Some(_)
            }
        ));
    }
}
