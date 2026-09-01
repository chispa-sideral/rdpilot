//! SC#5 [BLOCKING] offline (DAEMON-03): the daemon auto-starts on first
//! client connect and self-shuts-down after the registry empties past the
//! anti-thrash grace period -- driven against the REAL compiled
//! `rdpilot-daemon` binary via `env!("CARGO_BIN_EXE_rdpilot-daemon")`, not a
//! mock.
//!
//! This is a separate `tests/` integration-test crate root (like this
//! crate's sibling `tests/ipc_security.rs`/`tests/thread_leak_soak.rs`/
//! `tests/crash_restart_reconcile.rs`) -- ordinary `.expect()`/`.unwrap()`
//! calls below do not trip `rdpilot-daemon/src/lib.rs`'s inner
//! `#![deny(clippy::expect_used)]`/`unwrap_used` (those scope to the lib
//! crate's own compilation unit only).
//!
//! **What this proves, end to end, against the real binary:**
//! 1. **Auto-start (DAEMON-03):** before this test runs, no daemon is
//!    listening at the (test-isolated) well-known socket path.
//!    `connect_or_spawn` (Plan 12-06 Task 2) spawns the real
//!    `rdpilot-daemon` binary and becomes reachable via its own bounded
//!    backoff -- no manual daemon startup.
//! 2. **A live round trip against the real binary's dispatch/registry:**
//!    `Connect` (against the env-selected in-process fake connector,
//!    `RDPILOT_DAEMON_TEST_CONNECTOR=1` -- no RDP target needed) ->
//!    `List` (one session) -> `Disconnect` -> `Ack`, all length-prefixed
//!    JSON frames over the real Unix socket.
//! 3. **Self-shutdown-on-empty (DAEMON-03):** once the registry empties
//!    (after `Disconnect`), the real daemon process's `empty_watcher`
//!    (Plan 12-06 Task 1) fires its shutdown signal after the
//!    (test-tuned, short) anti-thrash grace period, `server::run()`
//!    removes its own socket file on the way out, and the daemon process
//!    exits on its own -- observed here as (a) the socket special file
//!    disappearing from disk and (b) a fresh connection attempt
//!    afterward being refused (nothing listening any more).
//!
//! **Isolation:** `ipc::unix::bind` (Plan 12-04) always resolves the
//! well-known socket path via `directories::BaseDirs::runtime_dir()`,
//! which reads the standard `XDG_RUNTIME_DIR` OS env var -- this test
//! isolates itself (and never collides with a real daemon or another test
//! run) by pointing `XDG_RUNTIME_DIR` at a fresh temp directory before
//! spawning the daemon (the child process inherits the parent's
//! environment by default). The reconciliation-state sink path and the
//! lifecycle durations are isolated/tuned the same way, via the
//! `RDPILOT_DAEMON_SINK_PATH`/`RDPILOT_DAEMON_*_MS` env vars `RunConfig`
//! (Plan 12-06 Task 3) reads at daemon startup.
//!
//! **`#[ignore]`-gated** (spawns a real child process; mirrors this
//! crate's existing `thread_leak_soak.rs` heavy-soak convention) --
//! exercised via `cargo test -- --include-ignored` (this plan's own
//! `<verify>` block), never in a bare default `cargo test` pass.

// Unix-only: relies on `XDG_RUNTIME_DIR`-based socket-path isolation and
// a real `tokio::net::UnixStream` directly. Live-VM-confirmed compile
// fix, Plan 15-05: this file never had a cfg gate before, which broke
// `cargo test -p rdpilot-daemon` on a Windows target.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rdpilot_daemon::connect_or_spawn;
use rdpilot_ipc::{Request, WireResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Mirrors `ipc::framing`'s wire format (a 4-byte big-endian `u32` length
/// prefix + that many bytes of UTF-8 JSON) -- duplicated locally rather
/// than exported from the crate, since this test only needs the two
/// trivial read/write primitives and `framing` is otherwise a private
/// implementation detail of `ipc::serve_connection`.
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

async fn write_frame<T: serde::Serialize>(stream: &mut UnixStream, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).expect("test request/response must serialize");
    let len = u32::try_from(body.len()).expect("test frame body fits in u32");
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> std::io::Result<T> {
    let mut len_buf = [0_u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    assert!(len <= MAX_FRAME_LEN, "test frame length {len} exceeds the sanity cap");
    let mut body = vec![0_u8; len as usize];
    stream.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).expect("test response must deserialize"))
}

/// A unique temp root per test invocation -- isolates `XDG_RUNTIME_DIR`
/// (the daemon's socket directory) and the reconciliation-state sink path.
fn unique_temp_root() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("rdpilot-daemon-autostart-lifecycle-{}-{nanos}", std::process::id()))
}

#[tokio::test]
#[ignore = "spawns a real child process (the compiled rdpilot-daemon binary) -- run via `-- --include-ignored`"]
async fn daemon_auto_starts_on_first_connect_and_self_exits_once_the_registry_empties() {
    let root = unique_temp_root();
    let xdg_runtime_dir = root.join("xdg-runtime");
    std::fs::create_dir_all(&xdg_runtime_dir).expect("create the isolated XDG_RUNTIME_DIR");
    let sink_path = root.join("sessions.json");

    // Cross-process config injection: env vars are the only channel that
    // survives `connect_or_spawn`'s detached `Command::spawn()` (the child
    // inherits the parent's environment by default).
    std::env::set_var("XDG_RUNTIME_DIR", &xdg_runtime_dir);
    std::env::set_var("RDPILOT_DAEMON_SINK_PATH", &sink_path);
    std::env::set_var("RDPILOT_DAEMON_TEST_CONNECTOR", "1");
    // Long enough that the reaper never touches the session mid-test; the
    // test's own Connect->List->Disconnect sequence completes in
    // milliseconds.
    std::env::set_var("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "60000");
    // Short enough to keep this test fast, long enough to comfortably
    // outlast the Connect->List->Disconnect round trip that happens WHILE
    // the registry is non-empty (so the watcher's grace window is never
    // even entered until after the real Disconnect).
    std::env::set_var("RDPILOT_DAEMON_EMPTY_GRACE_MS", "300");
    std::env::set_var("RDPILOT_DAEMON_REAP_INTERVAL_MS", "20");

    let daemon_exe = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-daemon"));
    let socket_path = xdg_runtime_dir.join("rdpilot").join("daemon.sock");

    // Precondition: nothing is running yet.
    assert!(!socket_path.exists(), "no daemon should be listening before connect_or_spawn auto-starts one");

    // --- Auto-start (DAEMON-03) ---
    let mut stream = tokio::time::timeout(Duration::from_secs(10), connect_or_spawn(&socket_path, &daemon_exe))
        .await
        .expect("connect_or_spawn must not time out spawning+reaching the real daemon binary")
        .expect("connect_or_spawn must auto-start the daemon and return a connected stream");

    // --- Connect (fake session, no RDP target needed) -> Connected ---
    let connect_req = Request::Connect {
        name: Some("autostart-lifecycle-test".to_owned()),
        host: "10.0.0.5".to_owned(),
        port: None,
        username: "user".to_owned(),
        password: "pw".to_owned(),
        domain: None,
        accept_invalid_certs: false,
        connect_ack: false,
    };
    write_frame(&mut stream, &connect_req).await.expect("write Connect frame");
    let session = match read_frame::<WireResponse>(&mut stream).await.expect("read Connect response") {
        WireResponse::Connected { session, .. } => session,
        other => panic!("expected WireResponse::Connected, got {other:?}"),
    };

    // --- List -> exactly one session ---
    write_frame(&mut stream, &Request::List {}).await.expect("write List frame");
    match read_frame::<WireResponse>(&mut stream).await.expect("read List response") {
        WireResponse::SessionList { sessions } => {
            assert_eq!(sessions.len(), 1, "exactly one session should be listed after Connect");
            assert_eq!(sessions[0].id, session.as_str());
        }
        other => panic!("expected WireResponse::SessionList, got {other:?}"),
    }

    // --- Disconnect -> Ack (the registry becomes empty here) ---
    write_frame(&mut stream, &Request::Disconnect { session }).await.expect("write Disconnect frame");
    match read_frame::<WireResponse>(&mut stream).await.expect("read Disconnect response") {
        WireResponse::Ack => {}
        other => panic!("expected WireResponse::Ack, got {other:?}"),
    }

    drop(stream);

    // --- Self-shutdown-on-empty (DAEMON-03) ---
    // `server::run()`'s shutdown path removes its own socket file on the
    // way out (Plan 12-06 Task 3) -- poll for that removal within a
    // bounded window comfortably longer than empty_grace plus a few
    // reap_interval poll cycles.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut self_exited = false;
    while Instant::now() < deadline {
        if !socket_path.exists() {
            self_exited = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        self_exited,
        "the daemon must remove its own socket file and self-exit once the registry empties past the grace period (DAEMON-03)"
    );

    // Corroborating evidence: nothing is listening at the path any more.
    let reconnect_attempt = UnixStream::connect(&socket_path).await;
    assert!(reconnect_attempt.is_err(), "no daemon should be reachable after self-shutdown");

    let _ = std::fs::remove_dir_all(&root);
}
