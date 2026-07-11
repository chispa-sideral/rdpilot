//! PROOF-03 -- the scripted MCP end-to-end proof harness (gated, live-only).
//!
//! Drives the REAL compiled `rdpilot-mcp` binary as an rmcp CLIENT subprocess
//! (`TokioChildProcess` + `().serve(...)`, 15-RESEARCH.md Pattern 2) against a
//! REAL remote Windows target (`.secrets/connection.json`) -- speaking actual
//! MCP JSON-RPC tool calls over stdio, with NO live LLM in the loop (that is
//! the capstone, Plan 15-04). This is PROOF-03's permanent, re-runnable
//! artifact proving the MCP surface exactly as any real MCP host sees it:
//! `rmcp` is the SAME crate the production `rdpilot-mcp` server links, so the
//! client and server are guaranteed wire-compatible by construction -- never
//! a hand-rolled JSON-RPC client.
//!
//! Exercises: `tools/list` discovery -> `rdpilot_connect` -> `rdpilot_list`
//! (D-30 status) -> one perception read (`rdpilot_world_state`) ->
//! `rdpilot_put` -> `rdpilot_get` (MCP-05 metadata-only checksum round trip)
//! -> `rdpilot_disconnect`. Every session-scoped call passes an explicit
//! `session` (D-29).
//!
//! Prints a step-by-step `PASS`/`FAIL` trace (D-9.4 style, mirroring
//! `crates/rdpilot-cli/tests/live_proof.rs`'s PROOF-02 harness) and a final
//! `PROOF: PASS`/`PROOF: FAIL` line.
//!
//! # Gating (D-18)
//!
//! `#[ignore]`-gated, and additionally early-returns cleanly (never a hard
//! failure) unless BOTH:
//! 1. `RDPILOT_LIVE` is set (the explicit opt-in), and
//! 2. `.secrets/connection.json` exists and parses.
//!
//! `RDPILOT_LIVE=1 cargo test -p rdpilot-mcp --test live_proof -- --ignored`
//! runs it against a provisioned VM. Plain `cargo test -p rdpilot-mcp` lists
//! it (`--list`) but never executes it -- nothing here runs offline or in CI.
//!
//! # Why `tests/*.rs`, not `--example` (15-RESEARCH.md Pitfall 1)
//!
//! `env!("CARGO_BIN_EXE_rdpilot-mcp")` -- the mechanism used below to locate
//! the compiled `rdpilot-mcp` binary -- is only populated by Cargo for
//! `tests/`/`benches/` targets, never for `--example` binaries. Since
//! PROOF-03 must spawn the REAL compiled MCP binary as a subprocess (not
//! construct the handler in-process the way `tests/tool_schema.rs`/
//! `tests/non_blocking.rs` do), it lives here as a gated integration test,
//! mirroring `crates/rdpilot-cli/tests/cli_lifecycle.rs`'s already-proven
//! pattern.
//!
//! # Thin-client invariant (D-17) / dev-dependency scope
//!
//! The `rmcp` client + transport-child-process features this file uses are
//! `[dev-dependencies]`-only (`crates/rdpilot-mcp/Cargo.toml`) -- excluded
//! from a production `cargo tree` invocation, so the production binary's
//! thin-client surface is unaffected.
//!
//! # Security
//!
//! Reads the gitignored `.secrets/connection.json` and hands the credentials
//! to `rdpilot_connect` as MCP tool-call ARGUMENTS (JSON over the child's
//! stdin) -- never as an environment variable or command-line argument on
//! the spawned subprocess, so the password never appears in the child's
//! `env`/`argv` (a stronger guarantee than the CLI harness's necessarily
//! argv-based approach, T-15-06). The password is read into a local, handed
//! straight to `serde_json::json!`, and NEVER included in any `println!`/
//! `format!`/`panic!` message in this file.

use std::path::PathBuf;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

/// Name of the opt-in env var that arms this live suite (D-18), mirroring
/// `crates/rdpilot/tests/common/mod.rs::LIVE_ENV` -- that module is not
/// reachable from here (`rdpilot-mcp` never depends on `rdpilot`, D-17), so
/// the name/convention is duplicated, not imported.
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// A live connection target, loaded from `.secrets/connection.json` at the
/// workspace root. Deliberately has NO `Debug`/`Display` impl -- nothing in
/// this file is allowed to accidentally format the whole struct (and thus
/// the password) into a message.
struct LiveTarget {
    host: String,
    user: String,
    password: String,
    port: u16,
}

/// Locate `.secrets/connection.json` relative to the workspace root.
/// `CARGO_MANIFEST_DIR` points at `crates/rdpilot-mcp`; the secrets file
/// lives two levels up, exactly as `crates/rdpilot/tests/common/mod.rs`
/// resolves it from `crates/rdpilot`.
fn connection_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("connection.json")
}

/// Locate the byte-verified sensor executable relayed by Plan 15-05
/// (`.secrets/sensor-build/rdpilot-sensor.exe`), same two-levels-up
/// resolution as [`connection_file`]. The spawned `rdpilot-mcp` subprocess's
/// `RDPILOT_SENSOR_BINARY_PATH` is pointed at this path below --
/// **live-diagnosed (Plan 15-06, mirrored here Plan 15-07):** without it,
/// the real `Connect` path never deploys a sensor at all, so every
/// sensor-backed step this file exercises (`rdpilot_world_state`,
/// `rdpilot_put`/`rdpilot_get`) would otherwise fail.
fn sensor_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("sensor-build").join("rdpilot-sensor.exe")
}

/// Load the live target, or `None` if the suite is not armed (D-18 gate:
/// `RDPILOT_LIVE` unset, or the secrets file absent). A present-but-
/// malformed file panics with a descriptive message that never includes the
/// password (mirrors `crates/rdpilot/tests/common/mod.rs::load_config`).
fn load_live_target() -> Option<LiveTarget> {
    std::env::var_os(LIVE_ENV)?;

    let path = connection_file();
    if !path.exists() {
        return None;
    }

    let raw = std::fs::read_to_string(&path)
        .expect("RDPILOT_LIVE is set and .secrets/connection.json exists but could not be read");
    let json: serde_json::Value = serde_json::from_str(&raw).expect(".secrets/connection.json is not valid JSON");

    let host =
        json.get("host").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `host`").to_owned();
    let user =
        json.get("user").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `user`").to_owned();
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `password`")
        .to_owned();
    let port: u16 =
        json.get("rdpPort").and_then(serde_json::Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(3389);

    Some(LiveTarget { host, user, password, port })
}

/// Record one D-9.4-style `PASS`/`FAIL` step: print it immediately (so a
/// long live run streams progress) and append it to `steps` for the final
/// summary/assert. Mirrors `crates/rdpilot-cli/tests/live_proof.rs`'s
/// identically-named/shaped helper.
fn record(steps: &mut Vec<(String, bool, String)>, name: &str, passed: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("  [{}] {name}: {detail}", if passed { "PASS" } else { "FAIL" });
    steps.push((name.to_owned(), passed, detail));
}

/// Join every text content block's text -- mirrors
/// `crate::native_tools`'s/`tests/non_blocking.rs`'s own `text_of` helper of
/// the same name/shape.
fn text_of(result: &CallToolResult) -> String {
    result.content.iter().filter_map(ContentBlock::as_text).map(|t| t.text.clone()).collect::<Vec<_>>().join(" ")
}

/// `value` MUST be a JSON object -- the shape every `rdpilot_*` tool's
/// arguments take. Panics (a harness-authoring bug, not a runtime
/// possibility) if it is not.
fn args_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        other => panic!("expected a JSON object for tool-call arguments, got {other:?}"),
    }
}

/// Call `name` with `arguments` (a `serde_json::json!` object) and classify
/// the outcome. An `Err` here is a JSON-RPC PROTOCOL-level error -- how
/// every `rdpilot_*`/`computer` tool actually surfaces an `McpError` today
/// (`handler.rs`/`native_tools.rs` map `Result<_, McpError>` into
/// `Result<CallToolResult, rmcp::ErrorData>`, never into a successful
/// `CallToolResult{is_error: Some(true)}`) -- but `is_error` is also
/// checked defensively in case that changes.
async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &'static str,
    arguments: serde_json::Value,
) -> Result<CallToolResult, String> {
    client
        .call_tool(CallToolRequestParams::new(name).with_arguments(args_object(arguments)))
        .await
        .map_err(|e| format!("{name} call failed: {e}"))
        .and_then(|result| {
            if result.is_error == Some(true) {
                Err(format!("{name} returned is_error=true: {}", text_of(&result)))
            } else {
                Ok(result)
            }
        })
}

/// Parse a successful tool result's text content as JSON, or `None` if it
/// failed or did not parse.
fn json_of(result: &Result<CallToolResult, String>) -> Option<serde_json::Value> {
    result.as_ref().ok().and_then(|r| serde_json::from_str::<serde_json::Value>(&text_of(r)).ok())
}

#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]
async fn mcp_end_to_end_against_a_real_target() {
    let Some(target) = load_live_target() else {
        eprintln!("skipping PROOF-03: RDPILOT_LIVE unset or .secrets/connection.json absent");
        return;
    };

    let mut steps: Vec<(String, bool, String)> = Vec::new();
    let session = "proof-mcp";

    println!("=== rdpilot PROOF-03 MCP end-to-end proof harness ===");

    // --- spawn the REAL rdpilot-mcp binary as an rmcp client subprocess
    // (Pattern 2) and complete the MCP initialize handshake ---
    let mcp_bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-mcp"));
    let mut mcp_command = Command::new(&mcp_bin);
    // Live-diagnosed (Plan 15-06, mirrored here): the real Connect path
    // only deploys a sensor when this is set -- required for
    // rdpilot_world_state/rdpilot_put/rdpilot_get to work against a real
    // target.
    mcp_command.env("RDPILOT_SENSOR_BINARY_PATH", sensor_binary_path());
    let transport =
        TokioChildProcess::new(mcp_command).expect("spawn the compiled rdpilot-mcp binary as a subprocess");
    let client = ()
        .serve(transport)
        .await
        .expect("complete the MCP initialize handshake against the real rdpilot-mcp binary");

    // --- Step 1: tools/list discovery -- the MCP-01/MCP-03 advertised
    // surface, exactly as any MCP host would see it on connect ---
    let expected_tools =
        ["rdpilot_connect", "rdpilot_list", "rdpilot_world_state", "rdpilot_put", "rdpilot_get", "rdpilot_disconnect"];
    let discovery = client.list_all_tools().await;
    let (discovery_ok, discovery_detail) = match &discovery {
        Ok(tools) => {
            let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
            let missing: Vec<&str> = expected_tools.iter().filter(|e| !names.contains(e)).copied().collect();
            if missing.is_empty() {
                (true, format!("{} tools advertised, all expected rdpilot_* tools present", names.len()))
            } else {
                (false, format!("missing tools: {missing:?}; advertised: {names:?}"))
            }
        }
        Err(e) => (false, format!("list_tools failed: {e}")),
    };
    record(&mut steps, "tools/list discovery", discovery_ok, discovery_detail);
    assert!(discovery_ok, "PROOF-03 cannot continue without the expected rdpilot_* tools advertised");

    // --- Step 2: rdpilot_connect (D-27 layered config via MCP-init params,
    // SESSION-01: no `session` param -- it creates one) ---
    let connect_args = serde_json::json!({
        "name": session,
        "host": target.host,
        "port": target.port,
        "username": target.user,
        "password": target.password,
        // The disposable lab VM uses a self-signed cert (D-15, test-only
        // risk-named passthrough -- mirrors
        // crates/rdpilot/tests/common/mod.rs::load_config).
        "accept_invalid_certs": true,
    });
    let connect_result = call(&client, "rdpilot_connect", connect_args).await;
    let connected_session =
        json_of(&connect_result).and_then(|v| v.get("session").and_then(|s| s.as_str()).map(str::to_owned));
    let connect_ok = connected_session.is_some();
    record(
        &mut steps,
        "rdpilot_connect",
        connect_ok,
        match (&connect_result, &connected_session) {
            (Ok(_), Some(s)) => format!("connected as session {s:?}"),
            (Ok(r), None) => format!("connect succeeded but no session id in result: {}", text_of(r)),
            (Err(e), _) => e.clone(),
        },
    );
    assert!(connect_ok, "PROOF-03 cannot continue past a failed rdpilot_connect");
    let session_id = connected_session.expect("checked by connect_ok above");

    // --- Step 3: rdpilot_list -- D-30 status vocabulary, SESSION-03: no
    // `session` param (it targets none in particular / lists all) ---
    let list_result = call(&client, "rdpilot_list", serde_json::json!({})).await;
    let session_entry = json_of(&list_result)
        .and_then(|v| v.get("sessions").and_then(|s| s.as_array().cloned()))
        .and_then(|sessions| sessions.into_iter().find(|s| s.get("id").and_then(|i| i.as_str()) == Some(session_id.as_str())));
    let session_listed = session_entry.is_some();
    record(
        &mut steps,
        "rdpilot_list (D-30 status)",
        session_listed,
        match (&list_result, &session_entry) {
            (Ok(_), Some(entry)) => format!("session {session_id:?} present, status={:?}", entry.get("status")),
            (Ok(r), None) => format!("session {session_id:?} not found in list: {}", text_of(r)),
            (Err(e), _) => e.clone(),
        },
    );

    // --- Step 4: one perception read -- rdpilot_world_state with a
    // screenshot, D-29 explicit session ---
    let world_state_args = serde_json::json!({ "session": session_id, "screenshot": true });
    let world_state_result = call(&client, "rdpilot_world_state", world_state_args).await;
    let world_state_ok = world_state_result.is_ok();
    record(
        &mut steps,
        "rdpilot_world_state (perception read)",
        world_state_ok,
        match &world_state_result {
            Ok(r) => {
                let text = text_of(r);
                format!("received {} bytes of world-state JSON (nonempty screenshot field present={})", text.len(), text.contains("\"screenshot\""))
            }
            Err(e) => e.clone(),
        },
    );

    // --- Step 5: rdpilot_put -- a real local file, uploaded to the
    // session's remote transfer root ---
    let local_upload = std::env::temp_dir().join(format!("rdpilot-mcp-proof-03-upload-{}.bin", std::process::id()));
    std::fs::write(&local_upload, b"rdpilot PROOF-03 payload\n").expect("seed the local upload file");
    let put_args = serde_json::json!({
        "session": session_id,
        "local_path": local_upload.to_string_lossy(),
        "remote_name": "rdpilot-proof-03.bin",
    });
    let put_result = call(&client, "rdpilot_put", put_args).await;
    let put_meta = json_of(&put_result);
    let put_checksum = put_meta.as_ref().and_then(|v| v.get("checksum").and_then(|c| c.as_str()).map(str::to_owned));
    let put_bytes = put_meta.as_ref().and_then(|v| v.get("bytes_transferred").and_then(serde_json::Value::as_u64));
    let put_ok = put_checksum.is_some() && put_bytes.is_some();
    record(
        &mut steps,
        "rdpilot_put",
        put_ok,
        match (&put_result, &put_checksum, put_bytes) {
            (Ok(_), Some(cs), Some(b)) => format!("uploaded rdpilot-proof-03.bin, bytes_transferred={b}, checksum={cs}"),
            (Ok(r), _, _) => format!("put succeeded but no checksum/bytes_transferred in result: {}", text_of(r)),
            (Err(e), _, _) => e.clone(),
        },
    );

    // --- Step 6: rdpilot_get -- MCP-05 metadata-only round trip: assert
    // checksum AND bytes_transferred both match put's ---
    let local_download = std::env::temp_dir().join(format!("rdpilot-mcp-proof-03-download-{}.bin", std::process::id()));
    let get_args = serde_json::json!({
        "session": session_id,
        "remote_name": "rdpilot-proof-03.bin",
        "local_path": local_download.to_string_lossy(),
    });
    let get_result = call(&client, "rdpilot_get", get_args).await;
    let get_meta = json_of(&get_result);
    let get_checksum = get_meta.as_ref().and_then(|v| v.get("checksum").and_then(|c| c.as_str()).map(str::to_owned));
    let get_bytes = get_meta.as_ref().and_then(|v| v.get("bytes_transferred").and_then(serde_json::Value::as_u64));
    let checksums_match = put_checksum.is_some() && put_checksum == get_checksum;
    let bytes_match = put_bytes.is_some() && put_bytes == get_bytes;
    let get_ok = checksums_match && bytes_match;
    record(
        &mut steps,
        "rdpilot_get (MCP-05 metadata round trip)",
        get_ok,
        if get_ok {
            format!("checksum and bytes_transferred matched put's: checksum={} bytes_transferred={}", get_checksum.as_deref().unwrap_or(""), get_bytes.unwrap_or(0))
        } else {
            match &get_result {
                Ok(r) => format!(
                    "mismatch: put checksum={put_checksum:?} bytes={put_bytes:?} vs get checksum={get_checksum:?} bytes={get_bytes:?}; raw={}",
                    text_of(r)
                ),
                Err(e) => e.clone(),
            }
        },
    );

    // --- Step 7: rdpilot_disconnect ---
    let disconnect_result = call(&client, "rdpilot_disconnect", serde_json::json!({ "session": session_id })).await;
    let disconnect_ok = disconnect_result.is_ok();
    record(
        &mut steps,
        "rdpilot_disconnect",
        disconnect_ok,
        match &disconnect_result {
            Ok(_) => format!("disconnected session {session_id:?}"),
            Err(e) => e.clone(),
        },
    );

    // --- shut down the MCP client / rdpilot-mcp subprocess cleanly ---
    let _ = client.cancel().await;

    let _ = std::fs::remove_file(&local_upload);
    let _ = std::fs::remove_file(&local_download);

    let all_passed = steps.iter().all(|(_, passed, _)| *passed);
    println!("PROOF: {}", if all_passed { "PASS" } else { "FAIL" });

    assert!(all_passed, "one or more PROOF-03 steps failed -- see the PASS/FAIL trace above");
}
