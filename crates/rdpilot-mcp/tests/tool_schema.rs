//! MCP-01 — `tools/list` schema-shape test.
//!
//! `rdpilot-mcp` is a bin-only crate (no `[lib]` target — see
//! `scale_to_native.rs`'s own doc comment for the established precedent):
//! this integration test pulls the WHOLE module tree in directly via
//! `#[path]`, mirroring `main.rs`'s own `mod` declarations exactly, rather
//! than importing the crate as a library dependency. Each `#[path]`-loaded
//! file's own internal (non-`#[path]`) submodule declarations resolve
//! relative to that file's OWN physical directory (`src/computer/mod.rs`'s
//! `pub mod dispatch;`/`pub mod scale;` still find `src/computer/dispatch.rs`/
//! `src/computer/scale.rs` correctly), so the module tree assembles exactly
//! as it does inside the real `rdpilot-mcp` binary.
//!
//! This test constructs `RdpilotMcpHandler`'s two `#[tool_router]`-generated
//! routers directly and sums them (`crate::handler`'s own
//! `#[tool_handler(router = ...)]` expression) — pure in-process
//! introspection. No daemon socket, no subprocess, no stdio transport is
//! ever touched.

#[path = "../src/computer/mod.rs"]
mod computer;
#[path = "../src/config_params.rs"]
mod config_params;
#[path = "../src/connect.rs"]
mod connect;
#[path = "../src/error.rs"]
mod error;
#[path = "../src/handler.rs"]
mod handler;
#[path = "../src/native_tools.rs"]
mod native_tools;
#[path = "../src/timeouts.rs"]
mod timeouts;

use handler::RdpilotMcpHandler;

/// The full advertised dual surface: `computer` plus the eleven `rdpilot_*`
/// native tools (MCP-01/MCP-03).
const EXPECTED_TOOL_NAMES: [&str; 12] = [
    "computer",
    "rdpilot_world_state",
    "rdpilot_uia",
    "rdpilot_window_list",
    "rdpilot_process_list",
    "rdpilot_launch",
    "rdpilot_foreground",
    "rdpilot_connect",
    "rdpilot_list",
    "rdpilot_disconnect",
    "rdpilot_put",
    "rdpilot_get",
];

/// The two tools that are structurally session-LESS by wire-level design
/// (SESSION-01/03): `rdpilot_connect` maps onto `Request::Connect` (it
/// creates a session — there is nothing yet to target) and `rdpilot_list`
/// maps onto `Request::List` (it targets no single session). These are
/// EXACTLY the two variants `rdpilot_ipc::request::SessionScoped::session()`
/// itself returns `None` for — every other tool below (including
/// `computer`, per D-29/Pitfall 4) requires `session`.
const SESSION_LESS_TOOLS: [&str; 2] = ["rdpilot_connect", "rdpilot_list"];

/// Sum `computer`'s router with the eleven native tools' router — the exact
/// expression `crate::handler`'s own `#[tool_handler(router = ...)]`
/// attribute uses to build the live server's advertised tool set.
fn all_tools() -> Vec<rmcp::model::Tool> {
    (RdpilotMcpHandler::computer_tool_router() + RdpilotMcpHandler::native_tool_router()).list_all()
}

/// Recursively search `schema` for ANY `"required"` array (at any nesting
/// depth — schemars may represent a flattened internally-tagged enum as
/// `allOf`/nested objects) containing `field`. For an `allOf`-combined
/// schema this is the semantically correct test: a data instance can only
/// satisfy the WHOLE `allOf` if it satisfies every branch, so a `field`
/// required in ANY branch is required overall.
fn schema_requires_field(schema: &serde_json::Value, field: &str) -> bool {
    match schema {
        serde_json::Value::Object(map) => {
            let required_here = match map.get("required") {
                Some(serde_json::Value::Array(required)) => required.iter().any(|v| v.as_str() == Some(field)),
                _ => false,
            };
            required_here || map.values().any(|v| schema_requires_field(v, field))
        }
        serde_json::Value::Array(items) => items.iter().any(|v| schema_requires_field(v, field)),
        _ => false,
    }
}

/// `true` if `schema` declares a top-level (or `allOf`-nested)
/// `session`/... `properties` entry at all — used to assert the two
/// session-less tools declare NO such property, not merely that it's
/// non-required.
fn schema_declares_property(schema: &serde_json::Value, property: &str) -> bool {
    match schema {
        serde_json::Value::Object(map) => {
            let declared_here = match map.get("properties") {
                Some(serde_json::Value::Object(props)) => props.contains_key(property),
                _ => false,
            };
            declared_here || map.values().any(|v| schema_declares_property(v, property))
        }
        serde_json::Value::Array(items) => items.iter().any(|v| schema_declares_property(v, property)),
        _ => false,
    }
}

#[test]
fn tools_list_advertises_exactly_computer_plus_the_eleven_native_tools() {
    let tools = all_tools();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    let mut expected: Vec<String> = EXPECTED_TOOL_NAMES.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(names, expected, "advertised tool set must be exactly `computer` plus the 11 `rdpilot_*` tools");
}

#[test]
fn every_session_scoped_tool_requires_session_in_its_input_schema() {
    let tools = all_tools();
    let mut checked = 0;
    for tool in &tools {
        if SESSION_LESS_TOOLS.contains(&tool.name.as_ref()) {
            continue;
        }
        let schema = tool.schema_as_json_value();
        assert!(
            schema_requires_field(&schema, "session"),
            "tool `{}` must require `session` (D-29): schema = {schema:#?}",
            tool.name
        );
        checked += 1;
    }
    // 12 total tools - 2 session-less (`rdpilot_connect`/`rdpilot_list`) = 10.
    assert_eq!(checked, EXPECTED_TOOL_NAMES.len() - SESSION_LESS_TOOLS.len());
}

#[test]
fn the_two_structurally_session_less_tools_declare_no_session_property_at_all() {
    let tools = all_tools();
    let mut checked = 0;
    for tool in &tools {
        if !SESSION_LESS_TOOLS.contains(&tool.name.as_ref()) {
            continue;
        }
        let schema = tool.schema_as_json_value();
        assert!(
            !schema_declares_property(&schema, "session"),
            "session-less tool `{}` (SESSION-01/03, mirrors Request::Connect/List's own \
             SessionScoped::session() -> None) must not declare a `session` property at all: schema = {schema:#?}",
            tool.name
        );
        checked += 1;
    }
    assert_eq!(checked, SESSION_LESS_TOOLS.len());
}

#[test]
fn tool_schema_introspection_needs_no_daemon_or_live_mcp_host() {
    // Pure in-process construction/introspection of the handler's
    // advertised tool set -- no socket, no subprocess, no stdio transport
    // is touched by any assertion in this file.
    assert_eq!(all_tools().len(), EXPECTED_TOOL_NAMES.len());
}
