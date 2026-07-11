//! [`RdpilotMcpHandler`] — the rmcp `ServerHandler` implementation.
//!
//! This plan (14-02) registers ZERO tools: `#[tool_router(server_handler)]`
//! below has no `#[tool]`-annotated methods yet, so the generated
//! `tool_router()` is an empty `ToolRouter::new()` with no routes. Plans
//! 14-03/14-04 add the `rdpilot_*` native tools and the Anthropic-compatible
//! `computer` mega-tool as `#[tool]` methods on this same `impl` block —
//! this scaffold exists so the stdio transport, dispatch plumbing, and
//! `ServerHandler`/`get_info` wiring compile and start before any tool
//! implementation lands.

use rmcp::tool_router;

/// The rmcp server handler. Holds no state today — every tool call (once
/// added) is a fresh one-shot daemon round trip via
/// `crate::connect::round_trip_bounded`, so the handler itself never tracks
/// a live connection (D-14.1's stateless-per-process invariant).
#[derive(Debug, Default, Clone, Copy)]
pub struct RdpilotMcpHandler;

/// `server_handler` emits `#[tool_handler] impl ServerHandler for
/// RdpilotMcpHandler` for us (the tools-only-server shorthand from rmcp's
/// own `handler::server::router::tool` module docs), wiring `call_tool`,
/// `list_tools`, `get_tool`, and a default `get_info` from this impl's
/// (currently empty) `tool_router()`.
#[tool_router(server_handler)]
impl RdpilotMcpHandler {
    /// Construct a handler instance. `main.rs`'s `#[tokio::main]` entry
    /// constructs exactly one of these per process and hands it to
    /// `.serve(stdio())`.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use rmcp::ServerHandler;

    use super::*;

    /// The empty tool router still produces a valid `ServerInfo` with the
    /// `tools` capability enabled (`#[tool_handler]`'s auto-generated
    /// `get_info`) — proves the macro-generated `ServerHandler` impl
    /// compiles and is callable even with zero registered tools.
    #[test]
    fn handler_reports_tools_capability_even_with_zero_registered_tools() {
        let handler = RdpilotMcpHandler::new();
        let info = handler.get_info();
        assert!(info.capabilities.tools.is_some(), "tools capability should be enabled by #[tool_handler]");
    }
}
