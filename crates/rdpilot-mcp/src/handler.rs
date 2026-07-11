//! [`RdpilotMcpHandler`] — the rmcp `ServerHandler` implementation.
//!
//! Plan 14-02 registered ZERO tools (an empty `ToolRouter::new()`). Plan
//! 14-03 added the first: the Anthropic-`computer_20250124`-compatible
//! `computer` mega-tool (MCP-02/MCP-04) as a `#[tool]` method on this same
//! `impl` block — its actual dispatch logic lives in
//! `crate::computer::dispatch::dispatch_computer` (a separate `impl
//! RdpilotMcpHandler` block in `computer/dispatch.rs`), so this method is a
//! thin adapter: extract `Parameters<ComputerArgs>`, dispatch, map
//! `McpError` to `rmcp::ErrorData` (D-28).
//!
//! Plan 14-04 adds the remaining eleven `rdpilot_*` native tools
//! (`crate::native_tools`, MCP-03) via a SECOND `#[tool_router]` block on
//! this same type, in that module. The two routers are combined into ONE
//! advertised `tools/list` surface via `ToolRouter`'s `Add` impl in this
//! file's explicit `#[tool_handler(router = ...)]` block below — `#[tool_router(server_handler)]`
//! (Plan 14-02/14-03's shorthand) only sums a single block's own router, so
//! combining two files' worth of tools requires writing that summation out
//! by hand instead.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::computer::ComputerArgs;

/// The rmcp server handler. Holds no state today — every tool call is a
/// fresh one-shot daemon round trip via `crate::connect::round_trip_bounded`,
/// so the handler itself never tracks a live connection (D-14.1's
/// stateless-per-process invariant).
#[derive(Debug, Default, Clone, Copy)]
pub struct RdpilotMcpHandler;

/// The `computer` tool's own router-generating block. `router =
/// computer_tool_router` (rather than the macro's default `tool_router`
/// name) so this file's function and `crate::native_tools`'s
/// `native_tool_router()` can coexist as two DIFFERENT inherent associated
/// functions on the same `RdpilotMcpHandler` type — both are summed below.
#[tool_router(router = computer_tool_router, vis = "pub(crate)")]
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

/// Combine the `computer` tool's router (this file) with the eleven
/// `rdpilot_*` native tools' router (`crate::native_tools::native_tool_router`)
/// into ONE advertised `tools/list` surface (MCP-01/MCP-03) —
/// `ToolRouter<Self>` implements `Add`, so summing the two static
/// router-builder functions merges their disjoint route maps. Ownership of
/// which tool lives in which source file stays exactly as each module's own
/// doc comment describes; only the final wiring lives here.
#[tool_handler(router = (Self::computer_tool_router() + Self::native_tool_router()))]
impl ServerHandler for RdpilotMcpHandler {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The combined router still produces a valid `ServerInfo` with the
    /// `tools` capability enabled (`#[tool_handler]`'s auto-generated
    /// `get_info`) — proves the macro-generated `ServerHandler` impl
    /// compiles and is callable now that it sums two routers.
    #[test]
    fn handler_reports_tools_capability() {
        let handler = RdpilotMcpHandler::new();
        let info = handler.get_info();
        assert!(info.capabilities.tools.is_some(), "tools capability should be enabled by #[tool_handler]");
    }
}
