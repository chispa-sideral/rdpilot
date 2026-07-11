//! MCP-06 [BLOCKING] non-blocking isolation proof: a slow in-flight tool
//! call must NOT block a concurrent unrelated fast tool call.
//!
//! `rdpilot-mcp` is a bin-only crate (no `[lib]` target -- see
//! `tests/tool_schema.rs`/`tests/scale_to_native.rs`'s own doc comments for
//! the established precedent): this integration test pulls the WHOLE
//! module tree in directly via `#[path]`, mirroring `main.rs`'s own `mod`
//! declarations exactly, rather than importing the crate as a library
//! dependency.
//!
//! **What this proves, end to end, against the REAL compiled daemon:**
//! 1. **A genuine slow daemon round trip, offline:** `RDPILOT_DAEMON_TEST_SLOW_MS=2000`
//!    (14-05, `rdpilot-daemon/src/server.rs`) makes the fake test
//!    connector's session genuinely `tokio::time::sleep(2000ms)` inside its
//!    `upload_file` implementation -- so `rdpilot_put` below is a REAL slow
//!    round trip, not a simulated one, with no live RDP target required.
//! 2. **Per-call task isolation (T-14-15):** a concurrently-spawned fast
//!    `rdpilot_list` call returns in well under 500ms while the slow
//!    `rdpilot_put` call is proven STILL PENDING (`JoinHandle::is_finished()
//!    == false`) at that moment -- a real ordering/overlap proof, not
//!    merely "both eventually completed quickly".
//! 3. **The composition this proves:** rmcp's per-request task spawn
//!    (Layer 1) + the daemon's per-connection/per-session isolation
//!    (`ipc::accept_and_authorize` + `tokio::task::spawn_local` per
//!    connection, Layer 2) + `crate::connect::round_trip_bounded`'s fresh,
//!    never-pooled `UnixStream` per call (Layer 2b) together mean a slow
//!    `Put` occupies only ITS OWN connection/task -- never a shared stream
//!    or a shared daemon-side dispatch loop that a concurrent `List` would
//!    have to wait behind.
//!
//! **Daemon reachability (read this before touching `connect.rs`):**
//! `crate::connect::open_stream` resolves the sibling `rdpilot-daemon`
//! binary via `std::env::current_exe().with_file_name(..)` -- when THIS
//! test binary calls it in-process, `current_exe()` is the TEST binary's
//! own path (`target/<triple>/debug/deps/non_blocking-<hash>`), not
//! `rdpilot-mcp`'s own installed-alongside-the-daemon location. That
//! resolution is only ever CONSULTED if `connect_or_spawn`'s first plain
//! `try_connect` fails -- so this test sidesteps the mismatch entirely by
//! pre-starting the real daemon itself (below, via a directly-and-correctly
//! resolved `rdpilot-daemon` path: `env!("CARGO_BIN_EXE_rdpilot-mcp")`'s
//! sibling, mirroring `rdpilot-cli/tests/cli_lifecycle.rs`'s identical
//! same-package `CARGO_BIN_EXE_*` convention) before any handler call is
//! made. Once the daemon is listening, EVERY subsequent internal
//! `open_stream()` call's plain `try_connect` succeeds immediately, so its
//! own (differently-resolved, and never exercised) `daemon_exe` value is
//! irrelevant.

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

use std::path::PathBuf;
use std::time::{Duration, Instant};

use handler::RdpilotMcpHandler;
use rdpilot_ipc::{connect_or_spawn, socket_path};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;

/// A unique temp root per test invocation -- isolates `XDG_RUNTIME_DIR`
/// (the daemon's socket directory) so this test never collides with a real
/// daemon or another concurrent test run.
fn unique_temp_root() -> PathBuf {
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    std::env::temp_dir().join(format!("rdpilot-mcp-non-blocking-{}-{nanos}", std::process::id()))
}

/// Join every text content block's text -- mirrors `native_tools.rs`'s own
/// test helper of the same name/shape.
fn text_of(result: &CallToolResult) -> String {
    result.content.iter().filter_map(rmcp::model::ContentBlock::as_text).map(|t| t.text.clone()).collect::<Vec<_>>().join(" ")
}

/// The MCP-06 [BLOCKING] isolation proof, driven against the REAL compiled
/// `rdpilot-daemon` binary. `flavor = "multi_thread"` (rather than the
/// `#[tokio::test]` default single-threaded flavor) mirrors `main.rs`'s own
/// `#[tokio::main]` runtime shape (multi-thread by default) so the
/// concurrency this test proves is the SAME shape the real MCP server runs
/// under, not merely single-thread cooperative interleaving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_tool_call_does_not_block_a_concurrent_fast_tool_call_mcp06() {
    let root = unique_temp_root();
    let xdg_runtime_dir = root.join("xdg-runtime");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    let sink_path = root.join("sessions.json");

    // Cross-process config injection: env vars are the only channel that
    // survives `connect_or_spawn`'s detached `Command::spawn()` (the child
    // inherits this process's environment by default) -- mirrors
    // `rdpilot-daemon/tests/autostart_lifecycle.rs`'s identical isolation
    // pattern.
    std::env::set_var("XDG_RUNTIME_DIR", &xdg_runtime_dir);
    std::env::set_var("RDPILOT_DAEMON_SINK_PATH", &sink_path);
    std::env::set_var("RDPILOT_DAEMON_TEST_CONNECTOR", "1");
    // The genuine slow round trip (14-05): the fake session's upload_file
    // sleeps this long before resolving.
    std::env::set_var("RDPILOT_DAEMON_TEST_SLOW_MS", "2000");
    // Long enough that neither the idle reaper nor the empty-registry grace
    // period fires mid-test.
    std::env::set_var("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "60000");
    std::env::set_var("RDPILOT_DAEMON_EMPTY_GRACE_MS", "60000");
    std::env::set_var("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50");

    // Locate the sibling `rdpilot-daemon` binary via THIS package's own
    // `rdpilot-mcp` bin target -- see this file's module doc comment for
    // why this (not `connect.rs`'s internal `current_exe()`-based
    // resolution) is the correct path to pre-start the daemon from.
    let mcp_bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-mcp"));
    let daemon_bin = mcp_bin.with_file_name(if cfg!(windows) { "rdpilot-daemon.exe" } else { "rdpilot-daemon" });
    assert!(
        daemon_bin.exists(),
        "expected the rdpilot-daemon binary at {daemon_bin:?} -- run `cargo build --workspace`          before this test (rdpilot-mcp intentionally never depends on rdpilot-daemon, so          `cargo test -p rdpilot-mcp` alone cannot build it)"
    );

    // --- Pre-start the real daemon (so every later handler call's own
    // internal `connect_or_spawn` reaches it via a plain connect) ---
    let socket = socket_path().expect("resolve the (isolated) daemon socket path");
    let pre_start = tokio::time::timeout(Duration::from_secs(10), connect_or_spawn(&socket, &daemon_bin))
        .await
        .expect("connect_or_spawn must not time out spawning+reaching the real daemon binary")
        .expect("connect_or_spawn must auto-start the daemon and return a connected stream");
    drop(pre_start);

    let handler = RdpilotMcpHandler::new();

    // --- rdpilot_connect against the fake connector (no live RDP target) ---
    let connect_args = native_tools::ConnectArgs {
        name: Some("mcp06-isolation".to_owned()),
        params: config_params::McpConnectParams {
            host: Some("10.0.0.5".to_owned()),
            port: None,
            username: Some("user".to_owned()),
            password: Some("pw".to_owned()),
            domain: None,
            accept_invalid_certs: false,
        },
    };
    let connect_result =
        handler.rdpilot_connect(Parameters(connect_args)).await.expect("rdpilot_connect must succeed against the fake connector");
    let connect_json: serde_json::Value =
        serde_json::from_str(&text_of(&connect_result)).expect("rdpilot_connect result must be JSON");
    let session = connect_json["session"].as_str().expect("rdpilot_connect result must include a session id").to_owned();

    // --- a real (planted) local file for the slow rdpilot_put to read ---
    let local_path = root.join("upload-me.bin");
    std::fs::write(&local_path, b"mcp-06 isolation proof payload").expect("plant the local file rdpilot_put reads");

    // --- spawn the SLOW call: rdpilot_put (TRANSFER class), delayed
    //     RDPILOT_DAEMON_TEST_SLOW_MS=2000ms by the fake session's
    //     upload_file (14-05) ---
    let slow_handler = handler;
    let slow_session = session.clone();
    let slow_local_path = local_path.to_string_lossy().into_owned();
    let slow_start = Instant::now();
    let slow_handle = tokio::spawn(async move {
        let put_args = native_tools::PutArgs {
            session: slow_session,
            local_path: slow_local_path,
            remote_name: "mcp06-isolation.bin".to_owned(),
        };
        let result = slow_handler.rdpilot_put(Parameters(put_args)).await;
        (result, slow_start.elapsed())
    });

    // Give the slow task a real chance to be scheduled and reach its own
    // await point (the fake session's 2000ms sleep) before racing the fast
    // call below -- comfortably shorter than the 2000ms delay, so the slow
    // call is certain to still be in flight at this point.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !slow_handle.is_finished(),
        "sanity: the slow rdpilot_put must still be in flight ~100ms after spawn (the fake session sleeps 2000ms)"
    );

    // --- concurrently issue the FAST call: rdpilot_list ---
    let fast_start = Instant::now();
    let fast_result = handler.rdpilot_list().await;
    let fast_elapsed = fast_start.elapsed();

    assert!(fast_result.is_ok(), "rdpilot_list must succeed: {fast_result:?}");
    assert!(
        fast_elapsed < Duration::from_millis(500),
        "MCP-06: the fast rdpilot_list must return well under the slow call's 2000ms delay -- took {fast_elapsed:?}"
    );
    // THE overlap proof: at the instant the fast call already returned, the
    // slow call must STILL be pending -- not merely "both were fast".
    assert!(
        !slow_handle.is_finished(),
        "MCP-06: the slow rdpilot_put must STILL be pending when the fast rdpilot_list already returned --          a slow in-flight call must not block a concurrent unrelated fast call"
    );

    // --- let the slow call finish, and confirm it genuinely took the
    //     configured delay (proving the round trip was really slow, not a
    //     no-op the fast call merely raced past by coincidence) ---
    let (slow_result, slow_elapsed) = slow_handle.await.expect("the slow rdpilot_put task must not panic");
    assert!(slow_result.is_ok(), "rdpilot_put must eventually succeed: {slow_result:?}");
    assert!(
        slow_elapsed >= Duration::from_millis(2000),
        "the slow rdpilot_put should have taken at least the configured RDPILOT_DAEMON_TEST_SLOW_MS=2000: took {slow_elapsed:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
