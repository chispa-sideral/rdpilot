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
}
pub trait SessionScoped {
    fn session(&self) -> Option<&SessionId>;
}
impl SessionScoped for Request {
    fn session(&self) -> Option<&SessionId> {
        match self {
            Self::Connect { .. } | Self::List {} => None,
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
}
