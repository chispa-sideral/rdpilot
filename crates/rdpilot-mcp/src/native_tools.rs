//! The eleven `rdpilot_*` native MCP tools (MCP-03, D-14.2 namespace): 1:1
//! wrappers over `rdpilot-ipc`'s non-`computer` wire verbs, added to
//! [`RdpilotMcpHandler`]'s advertised `tools/list` surface via a SECOND
//! `#[tool_router]` block ([`native_tool_router`]) that
//! [`crate::handler`] sums with its own `computer_tool_router()`
//! (`ToolRouter`'s `Add` impl) into ONE combined router — see that module's
//! `#[tool_handler(router = ...)]` impl.
//!
//! **Session targeting (D-29):** every tool below whose wire verb is
//! session-SCOPED (mirrors `rdpilot_ipc::Request::SessionScoped::session()
//! -> Some(_)`) carries a required, non-`Option` `session: String` field —
//! omitting it is a hard schema-validation rejection, asserted by
//! `tests/tool_schema.rs` (MCP-01). The two EXCEPTIONS are `rdpilot_connect`
//! and `rdpilot_list`, which map onto `Request::Connect`/`Request::List` —
//! the two wire verbs `SessionScoped::session()` itself returns `None` for
//! (SESSION-01/03): `Connect` creates a session (there is nothing yet to
//! target), and `List` enumerates every session (it targets no single one).
//! Adding an unused `session` field to either tool's schema would not map
//! onto anything on the wire and would contradict the "1:1 onto a wire
//! Request" contract this module otherwise holds to — so, deliberately,
//! these two tools alone have NO `session` parameter, mirroring the wire
//! model exactly.
//!
//! **MCP-05 (metadata-only, D-22):** `rdpilot_put`/`rdpilot_get` render
//! ONLY `{path, bytes_transferred, checksum}` from
//! `WireResponse::Transfer(TransferOutcome)`. `TransferOutcome` has no
//! byte-buffer field at all (`rdpilot-ipc::transfer`, 10-04-SUMMARY.md), so
//! [`render_transfer`] is a pure passthrough — it never reads or attaches
//! file bytes, and `path` is the caller-supplied local path echoed back as
//! metadata, never file content.

use std::str::FromStr;

use rdpilot_config::ResolvedConfig;
use rdpilot_ipc::{
    Request, SessionId, SessionLifecycle, TransferOutcome, WireResponse, WireUacDecision, WireUiaMode, WireUiaScope,
    WireWorldStateOptions,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::connect::{connect_round_trip_bounded, round_trip_bounded};
use crate::error::McpError;
use crate::handler::RdpilotMcpHandler;
use crate::timeouts;

// ---------------------------------------------------------------------
// Shared rendering helpers
// ---------------------------------------------------------------------

/// Parse a caller-supplied `session` string into a [`SessionId`], mapping
/// an empty/invalid value to a legible [`McpError::InvalidArgument`]
/// (never a panic) — the one place every session-scoped tool below
/// validates its `session` field before building a `Request`.
fn parse_session(session: &str) -> Result<SessionId, McpError> {
    SessionId::from_str(session).map_err(McpError::invalid_argument)
}

/// Render any `serde::Serialize` payload as a single JSON text content
/// block — the shared "structured JSON text" rendering convention every
/// native tool (other than the `Ack`-only ones) uses.
fn json_result(value: &impl serde::Serialize) -> Result<CallToolResult, McpError> {
    let text = serde_json::to_string(value)
        .map_err(|e| McpError::invalid_argument(format!("failed to render tool result: {e}")))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A plain success acknowledgement — mirrors `computer/dispatch.rs`'s own
/// `ack_result()` convention for `WireResponse::Ack`.
fn ack_result() -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text("ok")])
}

/// The D-30 lifecycle vocabulary, rendered verbatim (mirrors
/// `rdpilot-cli::verbs::session::lifecycle_str`).
fn lifecycle_str(status: SessionLifecycle) -> &'static str {
    match status {
        SessionLifecycle::Connecting => "Connecting",
        SessionLifecycle::Live => "Live",
        SessionLifecycle::Reconnecting => "Reconnecting",
        SessionLifecycle::Disconnected => "Disconnected",
        SessionLifecycle::Orphaned => "Orphaned",
    }
}

// ---------------------------------------------------------------------
// rdpilot_world_state
// ---------------------------------------------------------------------

/// A caller-facing mirror of [`WireUiaMode`] (schema-friendly field names),
/// converted 1:1 via [`From`] before it ever reaches the wire.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum WorldStateUiaMode {
    /// Fetch no UIA tree at all (the default when `uia` is omitted).
    #[default]
    None,
    /// Fetch the UIA tree of only the current foreground window.
    Foreground,
    /// Fetch the UIA tree for each of the given window handles, in order.
    Hwnd {
        /// The window handles to fetch UIA trees for.
        hwnds: Vec<u64>,
    },
    /// Fetch the UIA tree for every top-level window currently listed.
    AllTopLevel,
}

impl From<WorldStateUiaMode> for WireUiaMode {
    fn from(mode: WorldStateUiaMode) -> Self {
        match mode {
            WorldStateUiaMode::None => WireUiaMode::None,
            WorldStateUiaMode::Foreground => WireUiaMode::Foreground,
            WorldStateUiaMode::Hwnd { hwnds } => WireUiaMode::Hwnd(hwnds),
            WorldStateUiaMode::AllTopLevel => WireUiaMode::AllTopLevel,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorldStateArgs {
    /// The session to operate on (D-29: required on every call, no
    /// default).
    pub session: String,
    /// Whether to capture a full-desktop screenshot.
    #[serde(default)]
    pub screenshot: bool,
    /// Whether to include the top-level window list in the response.
    #[serde(default)]
    pub window_list: bool,
    /// Which UIA tree(s), if any, to fetch.
    #[serde(default)]
    pub uia: WorldStateUiaMode,
    /// Whether to fetch and surface `elevation_active` (session-scoped
    /// UAC/elevation consent-prompt detection, ticket BF8Q9K6FGZ2APN8F).
    #[serde(default)]
    pub elevation_check: bool,
}

/// Build the wire [`WireWorldStateOptions`] from [`WorldStateArgs`] — a
/// pure, directly unit-testable conversion.
fn build_world_state_options(args: &WorldStateArgs) -> WireWorldStateOptions {
    WireWorldStateOptions {
        screenshot: args.screenshot,
        window_list: args.window_list,
        uia: args.uia.clone().into(),
        elevation_check: args.elevation_check,
    }
}

/// Map a `Request::WorldState` round trip's response into a
/// [`CallToolResult`] — pure, directly unit-testable against a hand-built
/// [`WireResponse`] fixture (no daemon needed).
fn render_world_state(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::WorldState { timestamp, capture_span_ms, screenshot, window_list, uia, elevation_active } => {
            json_result(&serde_json::json!({
                "timestamp": timestamp,
                "capture_span_ms": capture_span_ms,
                "screenshot": screenshot,
                "window_list": window_list,
                "uia": uia,
                "elevation_active": elevation_active,
            }))
        }
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected WorldState, got {other:?}"))),
    }
}

// ---------------------------------------------------------------------
// rdpilot_uia
// ---------------------------------------------------------------------

/// A caller-facing mirror of [`WireUiaScope`], converted 1:1 via [`From`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum UiaScopeArg {
    /// Immediate children only (wire `max_depth: 1`).
    Children,
    /// A bounded, level-by-level deeper walk.
    Subtree {
        /// How many levels below the target window to walk.
        max_depth: u32,
    },
}

impl From<UiaScopeArg> for WireUiaScope {
    fn from(scope: UiaScopeArg) -> Self {
        match scope {
            UiaScopeArg::Children => WireUiaScope::Children,
            UiaScopeArg::Subtree { max_depth } => WireUiaScope::Subtree { max_depth },
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UiaArgs {
    /// The session to operate on (D-29).
    pub session: String,
    /// The target window handle.
    pub hwnd: u64,
    /// How deep to walk the UIA tree.
    pub scope: UiaScopeArg,
}

fn render_uia(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::Uia { elements } => json_result(&serde_json::json!({ "elements": elements })),
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected Uia, got {other:?}"))),
    }
}

// ---------------------------------------------------------------------
// rdpilot_window_list / rdpilot_process_list
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WindowListArgs {
    /// The session to operate on (D-29).
    pub session: String,
}

fn render_window_list(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::WindowList { windows } => json_result(&serde_json::json!({ "windows": windows })),
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected WindowList, got {other:?}"))),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProcessListArgs {
    /// The session to operate on (D-29).
    pub session: String,
}

fn render_process_list(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::ProcessList { processes, elevation_active } => {
            json_result(&serde_json::json!({ "processes": processes, "elevation_active": elevation_active }))
        }
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected ProcessList, got {other:?}"))),
    }
}

// ---------------------------------------------------------------------
// rdpilot_launch / rdpilot_foreground
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LaunchArgs {
    /// The session to operate on (D-29).
    pub session: String,
    /// The executable path.
    pub exe: String,
    /// Optional command-line arguments.
    #[serde(default)]
    pub args: Option<String>,
    /// Optional working directory.
    #[serde(default)]
    pub cwd: Option<String>,
}

fn render_pid(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::Pid { pid } => json_result(&serde_json::json!({ "pid": pid })),
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected Pid, got {other:?}"))),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ForegroundArgs {
    /// The session to operate on (D-29).
    pub session: String,
    /// The target window handle.
    pub hwnd: u64,
}

/// Shared `WireResponse::Ack` mapping for every tool whose success shape is
/// a plain acknowledgement (`rdpilot_foreground`, `rdpilot_disconnect`).
fn render_ack(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::Ack => Ok(ack_result()),
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected Ack, got {other:?}"))),
    }
}

// ---------------------------------------------------------------------
// rdpilot_uac_respond (ticket BF8Q9K6FGZ2APN8F)
// ---------------------------------------------------------------------

/// A caller-facing mirror of [`WireUacDecision`], converted 1:1 via
/// [`From`] — mirrors [`UiaScopeArg`]'s tagged-enum pattern.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum McpUacDecision {
    /// Approve the active UAC/elevation prompt.
    Approve,
    /// Reject the active UAC/elevation prompt.
    Reject,
}

impl From<McpUacDecision> for WireUacDecision {
    fn from(decision: McpUacDecision) -> Self {
        match decision {
            McpUacDecision::Approve => WireUacDecision::Approve,
            McpUacDecision::Reject => WireUacDecision::Reject,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UacRespondArgs {
    /// The session to operate on (D-29).
    pub session: String,
    /// Which way to respond.
    pub decision: McpUacDecision,
}

/// Render a `Request::UacRespond` round trip's response as
/// `{decision, confirmation_screenshot_base64}` — mirrors
/// [`render_world_state`]'s base64 convention.
fn render_uac_respond(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::UacRespond { decision, confirmation_png_base64 } => {
            let decision_str = match decision {
                WireUacDecision::Approve => "approve",
                WireUacDecision::Reject => "reject",
            };
            json_result(&serde_json::json!({
                "decision": decision_str,
                "confirmation_screenshot_base64": confirmation_png_base64,
            }))
        }
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected UacRespond, got {other:?}"))),
    }
}

// ---------------------------------------------------------------------
// rdpilot_connect / rdpilot_list / rdpilot_disconnect
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConnectArgs {
    /// Optional caller-supplied session name. Omitted requests an
    /// auto-generated id.
    #[serde(default)]
    pub name: Option<String>,
    /// MCP-init connection params (D-27 keys, verbatim), flattened onto
    /// this tool's top-level input.
    #[serde(flatten)]
    pub params: crate::config_params::McpConnectParams,
}

/// Require `host`/`username`/`password` to be present in `resolved` after
/// the file->env->MCP-init layering (D-27) — pure, directly unit-testable
/// against a hand-built [`ResolvedConfig`] fixture (mirrors
/// `rdpilot-cli::verbs::session::connect`'s identical required-field
/// handling).
fn require_connect_fields(resolved: &ResolvedConfig) -> Result<(String, String, String), McpError> {
    let host = resolved.host.clone().ok_or_else(|| {
        McpError::invalid_argument("host is required (config file, RDPILOT_HOST env, or the MCP-init host param)")
    })?;
    let username = resolved.username.clone().ok_or_else(|| {
        McpError::invalid_argument(
            "username is required (config file, RDPILOT_USERNAME env, or the MCP-init username param)",
        )
    })?;
    let password = resolved.password.clone().ok_or_else(|| {
        McpError::invalid_argument(
            "password is required (config file, RDPILOT_PASSWORD env, or the MCP-init password param)",
        )
    })?;
    Ok((host, username, password))
}

fn render_connected(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::Connected { session, sensor_live, .. } => {
            json_result(&serde_json::json!({ "session": session.as_str(), "sensor_live": sensor_live }))
        }
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected Connected, got {other:?}"))),
    }
}

fn render_session_list(resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::SessionList { sessions } => {
            let rendered: Vec<_> = sessions
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "host": s.host,
                        "status": lifecycle_str(s.status),
                        "connected_since": s.connected_since,
                        "last_activity": s.last_activity,
                    })
                })
                .collect();
            json_result(&serde_json::json!({ "sessions": rendered }))
        }
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected SessionList, got {other:?}"))),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DisconnectArgs {
    /// The session to disconnect (D-29).
    pub session: String,
}

// ---------------------------------------------------------------------
// rdpilot_put / rdpilot_get
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PutArgs {
    /// The session to operate on (D-29).
    pub session: String,
    /// The local file path to upload. Read with the daemon's own OS
    /// permissions — see `rdpilot_put`'s tool description and the
    /// `rdpilot-mcp` README's Trust Model section (T-14-12).
    pub local_path: String,
    /// The destination name under the remote transfer root.
    pub remote_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetArgs {
    /// The session to operate on (D-29).
    pub session: String,
    /// The source name under the remote transfer root.
    pub remote_name: String,
    /// The local destination path.
    pub local_path: String,
}

/// Render a completed `Put`/`Get` transfer's outcome as `{path,
/// bytes_transferred, checksum}` — METADATA ONLY (MCP-05/D-22). `path` is
/// the caller-supplied local path, echoed back as addressing metadata;
/// `TransferOutcome` structurally has no byte-buffer field, so this
/// function never reads or attaches file bytes — pure passthrough, not
/// stripping logic.
fn render_transfer(local_path: &str, resp: WireResponse) -> Result<CallToolResult, McpError> {
    match resp {
        WireResponse::Transfer(TransferOutcome { bytes_transferred, checksum }) => json_result(&serde_json::json!({
            "path": local_path,
            "bytes_transferred": bytes_transferred,
            "checksum": checksum,
        })),
        WireResponse::Error(err) => Err(McpError::from(err)),
        other => Err(McpError::invalid_argument(format!("expected Transfer, got {other:?}"))),
    }
}

// ---------------------------------------------------------------------
// Daemon round-trip glue (one method per tool, thin: session parse ->
// build Request -> round_trip_bounded -> render_*)
// ---------------------------------------------------------------------

impl RdpilotMcpHandler {
    async fn world_state_impl(&self, args: WorldStateArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let options = build_world_state_options(&args);
        let resp = round_trip_bounded(Request::WorldState { session, options }, timeouts::FAST).await?;
        render_world_state(resp)
    }

    async fn uia_impl(&self, args: UiaArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp =
            round_trip_bounded(Request::Uia { session, hwnd: args.hwnd, scope: args.scope.into() }, timeouts::FAST)
                .await?;
        render_uia(resp)
    }

    async fn window_list_impl(&self, args: WindowListArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp = round_trip_bounded(Request::WindowList { session }, timeouts::FAST).await?;
        render_window_list(resp)
    }

    async fn process_list_impl(&self, args: ProcessListArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp = round_trip_bounded(Request::ProcessList { session }, timeouts::FAST).await?;
        render_process_list(resp)
    }

    async fn launch_impl(&self, args: LaunchArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp = round_trip_bounded(
            Request::LaunchProcess { session, exe: args.exe, args: args.args, cwd: args.cwd },
            timeouts::FAST,
        )
        .await?;
        render_pid(resp)
    }

    async fn foreground_impl(&self, args: ForegroundArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp = round_trip_bounded(Request::SetForeground { session, hwnd: args.hwnd }, timeouts::FAST).await?;
        render_ack(resp)
    }

    async fn uac_respond_impl(&self, args: UacRespondArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp =
            round_trip_bounded(Request::UacRespond { session, decision: args.decision.into() }, timeouts::FAST).await?;
        render_uac_respond(resp)
    }

    async fn connect_impl(&self, args: ConnectArgs) -> Result<CallToolResult, McpError> {
        let resolved =
            rdpilot_config::resolve(args.params.into_overrides()).map_err(|e| McpError::invalid_argument(e.to_string()))?;
        let (host, username, password) = require_connect_fields(&resolved)?;
        let req = Request::Connect {
            name: args.name,
            host,
            port: resolved.port,
            username,
            password,
            domain: resolved.domain,
            accept_invalid_certs: resolved.accept_invalid_certs,
            connect_ack: true,
        };
        let resp = connect_round_trip_bounded(req, timeouts::CONNECT).await?;
        render_connected(resp)
    }

    async fn list_impl(&self) -> Result<CallToolResult, McpError> {
        let resp = round_trip_bounded(Request::List {}, timeouts::LIFECYCLE).await?;
        render_session_list(resp)
    }

    async fn disconnect_impl(&self, args: DisconnectArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let resp = round_trip_bounded(Request::Disconnect { session }, timeouts::LIFECYCLE).await?;
        render_ack(resp)
    }

    async fn put_impl(&self, args: PutArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let local_path = args.local_path.clone();
        let resp = round_trip_bounded(
            Request::Put { session, local_path: args.local_path, remote_name: args.remote_name },
            timeouts::TRANSFER,
        )
        .await?;
        render_transfer(&local_path, resp)
    }

    async fn get_impl(&self, args: GetArgs) -> Result<CallToolResult, McpError> {
        let session = parse_session(&args.session)?;
        let local_path = args.local_path.clone();
        let resp = round_trip_bounded(
            Request::Get { session, remote_name: args.remote_name, local_path: args.local_path },
            timeouts::TRANSFER,
        )
        .await?;
        render_transfer(&local_path, resp)
    }
}

// ---------------------------------------------------------------------
// The advertised tools themselves (MCP-03) — merged into the `computer`
// tool's router via `crate::handler`'s `ToolRouter` `Add`.
// ---------------------------------------------------------------------

#[tool_router(router = native_tool_router, vis = "pub(crate)")]
impl RdpilotMcpHandler {
    #[tool(
        name = "rdpilot_world_state",
        description = "Correlated desktop snapshot: screenshot + window list + optional UIA tree(s), one batch timestamp (D-8.1). Set elevation_check to also surface elevation_active: whether a UAC/elevation consent prompt is active in this session -- if true, use rdpilot_uac_respond instead of clicks/keys. Requires session (D-29)."
    )]
    pub async fn rdpilot_world_state(
        &self,
        Parameters(args): Parameters<WorldStateArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.world_state_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_uia",
        description = "Walk a UI Automation tree rooted at a window handle: either its immediate children or a bounded-depth subtree. Requires session (D-29)."
    )]
    pub async fn rdpilot_uia(&self, Parameters(args): Parameters<UiaArgs>) -> Result<CallToolResult, rmcp::ErrorData> {
        self.uia_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_window_list",
        description = "List the remote desktop's top-level windows (handle, title, rect, z-order, state, class, owning pid). Requires session (D-29)."
    )]
    pub async fn rdpilot_window_list(
        &self,
        Parameters(args): Parameters<WindowListArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.window_list_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_process_list",
        description = "List the remote machine's process tree (pid, parent pid, image name/path, best-effort command line/owner/session id) alongside elevation_active: whether a UAC/elevation consent prompt is active in this session -- if true, use rdpilot_uac_respond instead of clicks/keys. Requires session (D-29)."
    )]
    pub async fn rdpilot_process_list(
        &self,
        Parameters(args): Parameters<ProcessListArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.process_list_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_uac_respond",
        description = "Respond to an active Windows UAC/elevation consent prompt (approve or reject), including when it is on the protected secure desktop. Use this INSTEAD OF rdpilot_click-style clicks or the computer tool's left_click/scancode key actions whenever rdpilot_process_list or rdpilot_world_state reports elevation_active: true, or after a click/key call itself failed with a secure-desktop-active error -- raw clicks and scancode key combos are silently ignored by an active UAC prompt even though they report success. RDPilot performs the correct Tab-navigate + Unicode-keyboard-event sequence internally and confirms the outcome with a follow-up screenshot. Only the plain admin-consent Yes/No dialog is supported; a credential-entry UAC prompt (asking a non-admin user for a password) returns a uac-response-unconfirmed error naming this limitation -- escalate to a human instead of retrying. Requires session (D-29)."
    )]
    pub async fn rdpilot_uac_respond(
        &self,
        Parameters(args): Parameters<UacRespondArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.uac_respond_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_launch",
        description = "Launch a process on the remote machine and return its pid (fire-and-forget: does not wait for exit). Requires session (D-29)."
    )]
    pub async fn rdpilot_launch(
        &self,
        Parameters(args): Parameters<LaunchArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.launch_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_foreground",
        description = "Bring a remote window to the foreground by its window handle. Requires session (D-29)."
    )]
    pub async fn rdpilot_foreground(
        &self,
        Parameters(args): Parameters<ForegroundArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.foreground_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_connect",
        description = "Open a new RDP session against a remote Windows target, resolving host/credentials through the same file->env->MCP-init layered config pipeline the CLI uses (D-27: host/port/username/password/domain/accept_invalid_certs). Returns the assigned session id. This tool has NO `session` parameter (it creates one) — mirrors `Request::Connect`'s own session-less wire shape (SESSION-01)."
    )]
    pub async fn rdpilot_connect(
        &self,
        Parameters(args): Parameters<ConnectArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.connect_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_list",
        description = "List every session currently known to the daemon (id, name, host, D-30 lifecycle status, connected-since/last-activity). This tool has NO `session` parameter (it targets no single session) — mirrors `Request::List`'s own session-less wire shape (SESSION-03)."
    )]
    pub async fn rdpilot_list(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        self.list_impl().await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_disconnect",
        description = "Disconnect a named session. Requires session (D-29)."
    )]
    pub async fn rdpilot_disconnect(
        &self,
        Parameters(args): Parameters<DisconnectArgs>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        self.disconnect_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_put",
        description = "Upload a local file to the remote session's transfer root, returning {path, bytes_transferred, checksum} metadata ONLY — file bytes never appear in the tool result (MCP-05). RISK: this tool reads an ARBITRARY local file path with the daemon's own OS permissions and uploads it to the remote target. A prompt-injected or compromised caller could exfiltrate any locally-readable file this way (e.g. SSH keys, credentials). rdpilot builds NO server-side local-path sandbox this phase (trusted-operator model — see the rdpilot-mcp README's Trust Model section); the MCP host is responsible for tool-approval/sandboxing before exposing this tool to an untrusted prompt source. Requires session (D-29)."
    )]
    pub async fn rdpilot_put(&self, Parameters(args): Parameters<PutArgs>) -> Result<CallToolResult, rmcp::ErrorData> {
        self.put_impl(args).await.map_err(Into::into)
    }

    #[tool(
        name = "rdpilot_get",
        description = "Download a file from the remote session's transfer root to a local path, returning {path, bytes_transferred, checksum} metadata ONLY — file bytes never appear in the tool result (MCP-05). Requires session (D-29)."
    )]
    pub async fn rdpilot_get(&self, Parameters(args): Parameters<GetArgs>) -> Result<CallToolResult, rmcp::ErrorData> {
        self.get_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use rdpilot_ipc::{WireError, WireErrorCode, WireRect};

    use super::*;

    fn text_of(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(rmcp::model::ContentBlock::as_text)
            .map(|t| t.text.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }

    // -- parse_session --

    #[test]
    fn parse_session_accepts_a_nonempty_string() {
        let session = parse_session("brave-otter").expect("nonempty session must parse");
        assert_eq!(session, SessionId::from_str("brave-otter").expect("fixture"));
    }

    #[test]
    fn parse_session_rejects_an_empty_string() {
        let err = parse_session("").expect_err("empty session must be rejected");
        assert!(err.to_string().contains("empty"), "error should explain why: {err}");
    }

    // -- world_state --

    #[test]
    fn build_world_state_options_maps_every_field_through() {
        let args = WorldStateArgs {
            session: "s".to_owned(),
            screenshot: true,
            window_list: true,
            uia: WorldStateUiaMode::Hwnd { hwnds: vec![1, 2] },
            elevation_check: true,
        };
        let options = build_world_state_options(&args);
        assert!(options.screenshot);
        assert!(options.window_list);
        assert!(options.elevation_check);
        assert!(matches!(options.uia, WireUiaMode::Hwnd(hwnds) if hwnds == vec![1, 2]));
    }

    #[test]
    fn render_world_state_renders_the_success_payload() {
        let resp = WireResponse::WorldState {
            timestamp: "2026-07-11T00:00:00Z".to_owned(),
            capture_span_ms: 42,
            screenshot: Some("cGxhY2Vob2xkZXI=".to_owned()),
            window_list: None,
            uia: None,
            elevation_active: Some(true),
        };
        let result = render_world_state(resp).expect("success must render");
        let text = text_of(&result);
        assert!(text.contains("capture_span_ms"), "missing capture_span_ms: {text}");
        assert!(text.contains("2026-07-11T00:00:00Z"), "missing timestamp: {text}");
        assert!(text.contains("\"elevation_active\":true"), "missing elevation_active: {text}");
    }

    #[test]
    fn render_world_state_surfaces_a_wire_error_by_code_and_message() {
        let resp = WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, message: "no such session".to_owned() });
        let err = render_world_state(resp).expect_err("wire error must surface as an Err");
        assert!(err.to_string().contains("no such session"));
    }

    // -- uia --

    #[test]
    fn uia_scope_arg_children_maps_to_wire_children() {
        let wire: WireUiaScope = UiaScopeArg::Children.into();
        assert!(matches!(wire, WireUiaScope::Children));
    }

    #[test]
    fn uia_scope_arg_subtree_maps_max_depth_through() {
        let wire: WireUiaScope = UiaScopeArg::Subtree { max_depth: 3 }.into();
        assert!(matches!(wire, WireUiaScope::Subtree { max_depth: 3 }));
    }

    #[test]
    fn render_uia_renders_the_element_list() {
        let resp = WireResponse::Uia {
            elements: vec![rdpilot_ipc::WireUiaElement {
                id: "1.2.3".to_owned(),
                role: "Button".to_owned(),
                name: "OK".to_owned(),
                bbox: WireRect { x: 0, y: 0, w: 10, h: 10 },
                enabled: true,
                visible: true,
                focusable: true,
                focused: false,
                depth: 1,
                parent_id: "1.2".to_owned(),
            }],
        };
        let result = render_uia(resp).expect("success must render");
        assert!(text_of(&result).contains("\"Button\""));
    }

    #[test]
    fn render_uia_surfaces_wrong_variant_as_an_explicit_error() {
        let err = render_uia(WireResponse::Ack).expect_err("Ack is the wrong variant for Uia");
        assert!(err.to_string().to_lowercase().contains("uia"));
    }

    // -- window_list / process_list --

    #[test]
    fn render_window_list_renders_the_window_list() {
        let resp = WireResponse::WindowList {
            windows: vec![rdpilot_ipc::WireWindowInfo {
                hwnd: 1,
                title: "Notepad".to_owned(),
                rect: WireRect { x: 0, y: 0, w: 100, h: 100 },
                z_order: 0,
                state: rdpilot_ipc::WireWindowState::Normal,
                class_name: "Notepad".to_owned(),
                pid: 42,
            }],
        };
        let result = render_window_list(resp).expect("success must render");
        assert!(text_of(&result).contains("Notepad"));
    }

    #[test]
    fn render_process_list_renders_the_process_list() {
        let resp = WireResponse::ProcessList {
            processes: vec![rdpilot_ipc::WireProcessInfo {
                pid: 42,
                parent_pid: 4,
                name: "notepad.exe".to_owned(),
                path: "C:\\Windows\\notepad.exe".to_owned(),
                command_line: None,
                owner: None,
                session_id: Some(1),
            }],
            elevation_active: false,
        };
        let result = render_process_list(resp).expect("success must render");
        let text = text_of(&result);
        assert!(text.contains("notepad.exe"));
        assert!(text.contains("\"elevation_active\":false"), "missing elevation_active: {text}");
    }

    // -- uac_respond (ticket BF8Q9K6FGZ2APN8F) --

    #[test]
    fn mcp_uac_decision_approve_maps_to_wire_approve() {
        let wire: WireUacDecision = McpUacDecision::Approve.into();
        assert!(matches!(wire, WireUacDecision::Approve));
    }

    #[test]
    fn mcp_uac_decision_reject_maps_to_wire_reject() {
        let wire: WireUacDecision = McpUacDecision::Reject.into();
        assert!(matches!(wire, WireUacDecision::Reject));
    }

    #[test]
    fn render_uac_respond_renders_the_decision_and_confirmation() {
        let resp = WireResponse::UacRespond {
            decision: WireUacDecision::Approve,
            confirmation_png_base64: "cGxhY2Vob2xkZXI=".to_owned(),
        };
        let result = render_uac_respond(resp).expect("success must render");
        let text = text_of(&result);
        assert!(text.contains("\"decision\":\"approve\""), "missing decision: {text}");
        assert!(text.contains("confirmation_screenshot_base64"), "missing confirmation: {text}");
    }

    #[test]
    fn render_uac_respond_surfaces_a_wire_error_by_code_and_message() {
        let resp = WireResponse::Error(WireError {
            code: WireErrorCode::UacResponseUnconfirmed,
            message: "the prompt is still present".to_owned(),
        });
        let err = render_uac_respond(resp).expect_err("wire error must surface as an Err");
        assert!(err.to_string().contains("the prompt is still present"));
    }

    #[test]
    fn render_uac_respond_surfaces_wrong_variant_as_an_explicit_error() {
        let err = render_uac_respond(WireResponse::Ack).expect_err("Ack is the wrong variant for UacRespond");
        assert!(err.to_string().to_lowercase().contains("uacrespond"));
    }

    // -- launch / foreground --

    #[test]
    fn render_pid_renders_the_launched_pid() {
        let result = render_pid(WireResponse::Pid { pid: 4242 }).expect("success must render");
        assert!(text_of(&result).contains("4242"));
    }

    #[test]
    fn render_ack_renders_ok_on_success() {
        let result = render_ack(WireResponse::Ack).expect("success must render");
        assert_eq!(text_of(&result), "ok");
    }

    #[test]
    fn render_ack_surfaces_a_wire_error() {
        let resp = WireResponse::Error(WireError { code: WireErrorCode::SessionNotFound, message: "gone".to_owned() });
        let err = render_ack(resp).expect_err("wire error must surface");
        assert!(err.to_string().contains("gone"));
    }

    // -- connect / list --

    fn empty_resolved() -> ResolvedConfig {
        ResolvedConfig {
            host: None,
            port: None,
            username: None,
            password: None,
            domain: None,
            accept_invalid_certs: false,
            share_root: None,
            sensor_binary_path: None,
        }
    }

    #[test]
    fn require_connect_fields_succeeds_when_all_three_are_present() {
        let mut resolved = empty_resolved();
        resolved.host = Some("10.0.0.5".to_owned());
        resolved.username = Some("alice".to_owned());
        resolved.password = Some("hunter2".to_owned());
        let (host, username, password) = require_connect_fields(&resolved).expect("all fields present");
        assert_eq!(host, "10.0.0.5");
        assert_eq!(username, "alice");
        assert_eq!(password, "hunter2");
    }

    #[test]
    fn require_connect_fields_rejects_a_missing_host() {
        let mut resolved = empty_resolved();
        resolved.username = Some("alice".to_owned());
        resolved.password = Some("hunter2".to_owned());
        let err = require_connect_fields(&resolved).expect_err("missing host must be rejected");
        assert!(err.to_string().contains("host"));
    }

    #[test]
    fn require_connect_fields_rejects_a_missing_username() {
        let mut resolved = empty_resolved();
        resolved.host = Some("10.0.0.5".to_owned());
        resolved.password = Some("hunter2".to_owned());
        let err = require_connect_fields(&resolved).expect_err("missing username must be rejected");
        assert!(err.to_string().contains("username"));
    }

    #[test]
    fn require_connect_fields_rejects_a_missing_password() {
        let mut resolved = empty_resolved();
        resolved.host = Some("10.0.0.5".to_owned());
        resolved.username = Some("alice".to_owned());
        let err = require_connect_fields(&resolved).expect_err("missing password must be rejected");
        assert!(err.to_string().contains("password"));
    }

    #[test]
    fn render_connected_reports_the_sensor_liveness_proof() {
        let session = SessionId::from_str("brave-otter").expect("fixture");
        let result = render_connected(WireResponse::Connected {
            session,
            connect_ack_required: false,
            sensor_live: true,
        })
        .expect("success must render");
        let text = text_of(&result);
        assert!(text.contains("brave-otter"));
        assert!(text.contains("\"sensor_live\":true"));
    }

    #[test]
    fn render_session_list_renders_the_d30_vocabulary_verbatim() {
        let resp = WireResponse::SessionList {
            sessions: vec![rdpilot_ipc::SessionStatus {
                id: "brave-otter".to_owned(),
                name: None,
                host: "10.0.0.5".to_owned(),
                status: SessionLifecycle::Live,
                connected_since: None,
                last_activity: None,
            }],
        };
        let result = render_session_list(resp).expect("success must render");
        assert!(text_of(&result).contains("\"Live\""));
    }

    // -- put / get / MCP-05 --

    #[test]
    fn render_transfer_renders_path_bytes_transferred_and_checksum() {
        let outcome = TransferOutcome { bytes_transferred: 42, checksum: "deadbeef".to_owned() };
        let result = render_transfer("/tmp/example.bin", WireResponse::Transfer(outcome)).expect("success must render");
        let text = text_of(&result);
        assert!(text.contains("/tmp/example.bin"));
        assert!(text.contains("bytes_transferred"));
        assert!(text.contains("checksum"));
    }

    /// MCP-05 (BLOCKING): a real file planted on disk with a known sentinel
    /// byte pattern MUST NOT appear anywhere in `render_transfer`'s
    /// serialized tool result. `render_transfer` never opens/reads the
    /// file at `path` at all -- this is a structural guarantee
    /// (`TransferOutcome` has no byte field to carry it), and this test is
    /// the regression guard for that guarantee, mirroring
    /// `rdpilot-ipc::response`'s own planted-secret-sentinel test style.
    #[test]
    fn put_get_transfer_result_never_carries_the_planted_file_bytes_sentinel() {
        const SENTINEL: &str = "RDPILOT-PLANTED-FILE-BYTES-SENTINEL";
        let planted_path = std::env::temp_dir().join(format!("rdpilot-mcp-mcp05-test-{}.bin", std::process::id()));
        std::fs::write(&planted_path, SENTINEL.as_bytes()).expect("failed to plant the sentinel file");

        let outcome = TransferOutcome { bytes_transferred: 4242, checksum: "deadbeef".to_owned() };
        let path_str = planted_path.to_string_lossy().into_owned();
        let result =
            render_transfer(&path_str, WireResponse::Transfer(outcome)).expect("success must render");
        let text = text_of(&result);

        assert!(text.contains("bytes_transferred"), "must carry bytes_transferred: {text}");
        assert!(text.contains("checksum"), "must carry checksum: {text}");
        assert!(!text.contains(SENTINEL), "MCP-05 leak: planted file-bytes sentinel found in {text}");

        let _ = std::fs::remove_file(&planted_path);
    }

    #[test]
    fn render_transfer_surfaces_a_wire_error() {
        let resp = WireResponse::Error(WireError { code: WireErrorCode::TransferFailed, message: "disk full".to_owned() });
        let err = render_transfer("/tmp/x", resp).expect_err("wire error must surface");
        assert!(err.to_string().contains("disk full"));
    }
}
