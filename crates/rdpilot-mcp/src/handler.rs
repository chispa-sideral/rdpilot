//! [`RdpilotMcpHandler`] — the rmcp `ServerHandler` implementation.
//!
//! Plan 14-02 registered ZERO tools (an empty `ToolRouter::new()`). Plan
//! 14-03 adds the first: the Anthropic-`computer_20250124`-compatible
//! `computer` mega-tool (MCP-02/MCP-04) as a `#[tool]` method on this same
//! `impl` block — its actual dispatch logic lives in
//! `crate::computer::dispatch::dispatch_computer` (a separate `impl
//! RdpilotMcpHandler` block in `computer/dispatch.rs`), so this method is a
//! thin adapter: extract `Parameters<ComputerArgs>`, dispatch, map
//! `McpError` to `rmcp::ErrorData` (D-28). Plan 14-04 adds the remaining
//! `rdpilot_*` native tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};

use crate::computer::ComputerArgs;

/// The rmcp server handler. Holds no state today — every tool call is a
/// fresh one-shot daemon round trip via `crate::connect::round_trip_bounded`,
/// so the handler itself never tracks a live connection (D-14.1's
/// stateless-per-process invariant).
#[derive(Debug, Default, Clone, Copy)]
pub struct RdpilotMcpHandler;

/// `server_handler` emits `#[tool_handler] impl ServerHandler for
/// RdpilotMcpHandler` for us (the tools-only-server shorthand from rmcp's
/// own `handler::server::router::tool` module docs), wiring `call_tool`,
/// `list_tools`, `get_tool`, and a default `get_info` from this impl's
/// `tool_router()`.
#[tool_router(server_handler)]
impl RdpilotMcpHandler {
    /// Construct a handler instance. `main.rs`'s `#[tokio::main]` entry
    /// constructs exactly one of these per process and hands it to
    /// `.serve(stdio())`.
    pub fn new() -> Self {
        Self
    }

    /// The Anthropic `computer_20250124`-compatible mega-tool (D-21/D-14.2,
    /// MCP-02): a single schema-discriminated action covering
    /// screenshot/mouse/keyboard/scroll, mapped onto rdpilot's ipc input
    /// and capture verbs. Coordinates are supplied in a FIXED advertised
    /// 1280x800 space and bridged to the target session's native 96-DPI
    /// pixels via `scale_to_native` (MCP-04, BLOCKING). Every call requires
    /// a `session` id (D-29) alongside the action.
    ///
    /// Supported actions: `screenshot`, `mouse_move`, `left_click`,
    /// `right_click`, `middle_click`, `double_click`, `triple_click`,
    /// `left_click_drag`, `key`, `type`, `scroll` (vertical only),
    /// `hold_key` (approximated as an atomic press+release), `wait`.
    ///
    /// Explicitly UNSUPPORTED (returns a tool error naming the action,
    /// never a silent no-op): `left_mouse_down`, `left_mouse_up` (no
    /// half-click primitive — use `left_click_drag`), `cursor_position` (no
    /// pointer-position getter — take a `screenshot` instead), and
    /// horizontal `scroll_direction` (`left`/`right` — rdpilot's scroll is
    /// vertical-only).
    #[tool(
        name = "computer",
        description = "Anthropic computer_20250124-compatible remote-desktop control: screenshot, mouse, keyboard, and scroll actions against a connected rdpilot session, with coordinates in a fixed 1280x800 space. Requires a session id. Does NOT support left_mouse_down/left_mouse_up, cursor_position, or horizontal scroll (left/right) — these return an explicit error."
    )]
    pub async fn computer(
        &self,
        Parameters(args): Parameters<ComputerArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.dispatch_computer(args).await.map_err(Into::into)
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
