//! [`McpError`] — the `rdpilot-mcp` crate's client-side error taxonomy, and
//! its rendering as an rmcp tool error (D-28).
//!
//! Mirrors `rdpilot-cli::exit_codes::CliError`'s split: a typed daemon-side
//! [`WireError`] (D-28) wrapped as-is, plus client-local failure classes
//! that never cross the wire (`Timeout`, `DaemonUnreachable`, `Transport`)
//! — the daemon itself never observes an MCP-server-side timeout; the MCP
//! server simply gave up waiting (MCP-06).

use std::time::Duration;

use rdpilot_ipc::WireError;

/// The client-side error type every tool handler / transport helper
/// returns.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// A typed error returned by the daemon over the wire (D-28). Rendered
    /// with both `code` and `message` in the tool error (see
    /// `impl From<McpError> for rmcp::ErrorData`).
    #[error("{0}")]
    Wire(WireError),

    /// `round_trip_bounded`'s explicit per-verb-class bound (MCP-06)
    /// elapsed before the daemon round trip completed. Client-local — the
    /// daemon never observed a timeout.
    #[error("bounded timeout of {0:?} exceeded waiting for the daemon round trip")]
    Timeout(Duration),

    /// The daemon socket/sibling binary could not be resolved, or
    /// `connect_or_spawn`'s bounded backoff exhausted without the daemon
    /// becoming reachable (mirrors `CliError::DaemonUnreachable`'s
    /// client-only precedent).
    #[error("daemon unreachable: {0}")]
    DaemonUnreachable(String),

    /// A transport-layer failure (frame encode/decode, I/O) below the wire
    /// protocol itself.
    #[error("transport error: {0}")]
    Transport(String),
}

impl McpError {
    /// Construct a [`McpError::DaemonUnreachable`] from any displayable
    /// error, for use as a `.map_err` target.
    pub fn daemon_unreachable(err: impl std::fmt::Display) -> Self {
        McpError::DaemonUnreachable(err.to_string())
    }

    /// Construct a [`McpError::Transport`] from any displayable error, for
    /// use as a `.map_err` target.
    pub fn transport(err: impl std::fmt::Display) -> Self {
        McpError::Transport(err.to_string())
    }
}

impl From<WireError> for McpError {
    fn from(err: WireError) -> Self {
        McpError::Wire(err)
    }
}

/// The kebab-case discriminant string for `err` — mirrors
/// `rdpilot-cli::exit_codes::code_str_for`'s convention exactly, so the
/// wire's own kebab-case `WireErrorCode` strings and this crate's
/// client-local classes share one legible vocabulary.
fn code_str_for(err: &McpError) -> String {
    match err {
        McpError::Wire(wire) => serde_json::to_string(&wire.code)
            .ok()
            .map(|s| s.trim_matches('"').to_owned())
            .unwrap_or_else(|| "internal".to_owned()),
        McpError::Timeout(_) => "timeout".to_owned(),
        McpError::DaemonUnreachable(_) => "daemon-unreachable".to_owned(),
        McpError::Transport(_) => "transport".to_owned(),
    }
}

/// D-28: render `err` as an rmcp tool error carrying BOTH the fixed
/// discriminant (via `data.code`) and the human-readable `message` text —
/// an MCP client/LLM reading the tool error sees the same two fields the
/// CLI's `--json` error payload exposes (`exit_codes::code_str_for` +
/// `Display`).
impl From<McpError> for rmcp::ErrorData {
    fn from(err: McpError) -> Self {
        let discriminant = code_str_for(&err);
        let message = err.to_string();
        rmcp::ErrorData::internal_error(message, Some(serde_json::json!({ "code": discriminant })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdpilot_ipc::WireErrorCode;

    #[test]
    fn wire_error_renders_both_code_and_message_in_the_rmcp_error() {
        let wire = WireError { code: WireErrorCode::SessionNotFound, message: "no such session".to_owned() };
        let mcp_err = McpError::from(wire);
        let rmcp_err: rmcp::ErrorData = mcp_err.into();
        assert!(
            rmcp_err.message.contains("no such session"),
            "message missing from rendered error: {}",
            rmcp_err.message
        );
        match rmcp_err.data {
            Some(data) => assert_eq!(data["code"], "session-not-found"),
            None => panic!("data must carry the code discriminant (D-28)"),
        }
    }

    #[test]
    fn timeout_renders_a_clearly_worded_message_naming_the_elapsed_bound() {
        let err = McpError::Timeout(Duration::from_secs(15));
        let rendered = err.to_string();
        assert!(rendered.contains("15s") || rendered.contains("15"), "message should name the elapsed bound: {rendered}");
        assert!(rendered.to_lowercase().contains("timeout"), "message should clearly say timeout: {rendered}");
    }

    #[test]
    fn client_local_classes_render_legibly() {
        assert!(McpError::daemon_unreachable("no socket").to_string().contains("no socket"));
        assert!(McpError::transport("broken pipe").to_string().contains("broken pipe"));
    }
}
