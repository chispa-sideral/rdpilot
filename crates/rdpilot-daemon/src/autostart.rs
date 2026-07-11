//! Client-side connect-or-spawn helper (Plan 12-06; DAEMON-03) -- implemented
//! and tested here so DAEMON-03 has real coverage now, consumed later by the
//! Phase 13/14 CLI/MCP thin clients.
//!
//! Unix-only for now (`#[cfg(unix)]`): the Windows named-pipe transport
//! (`ipc::windows`) remains a stub deferred to the live-gate Plan 12-07, so
//! there is no Windows stream type to connect this helper against yet on
//! this development host.
//!
//! The daemon side's single-instance guarantee comes ENTIRELY from
//! `ipc::unix::bind`'s `AddrInUse`-on-a-live-listener behavior (Plan 12-04):
//! the bind IS the mutex. This module contains no PID-file logic
//! whatsoever -- research explicitly flags a PID file as the anti-pattern
//! here (stale after `kill -9`, requires its own race-prone check).

#![cfg(unix)]

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::net::UnixStream;

/// The bounded backoff sequence (milliseconds) `connect_or_spawn` retries
/// on after spawning the daemon (research Pattern 3's exact sequence).
const BACKOFF_MS: &[u64] = &[50, 100, 200, 400, 800];

/// Attempt one connection to `socket_path`. ANY failure (no such file,
/// connection refused, a stale special file left by a killed predecessor,
/// etc.) is uniformly treated as "not listening" by the caller -- this
/// function does not try to distinguish failure kinds, matching research
/// Pattern 3's guidance that a connect-refused/timeout on an existing
/// special file must proceed to spawn, not be surfaced as a hard error.
async fn try_connect(socket_path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(socket_path).await
}

/// Connect to the daemon at `socket_path`, auto-starting it via
/// `daemon_exe` if it is not already listening (DAEMON-03).
///
/// Always tries a plain connect FIRST. Only on failure does it spawn
/// `daemon_exe` (detached -- never waited on; the daemon backgrounds
/// itself) and retry the connect over [`BACKOFF_MS`]'s bounded sequence,
/// returning the first successful stream.
///
/// Two clients racing to auto-start the daemon simultaneously both reach
/// this "spawn" branch and both spawn a daemon process -- this is a benign,
/// expected outcome (research Pattern 3): exactly one daemon process wins
/// the bind on `socket_path` (`ipc::unix::bind`'s `AddrInUse` behavior,
/// Plan 12-04) and keeps running; the loser's `server::run()` (Plan 12-06)
/// observes `AddrInUse` and exits immediately without error. Both racing
/// clients' backoff-retry loop below eventually connects to whichever
/// daemon won.
///
/// # Errors
///
/// Returns [`io::ErrorKind::TimedOut`] if the daemon never becomes
/// reachable within the backoff sequence, or the underlying spawn error if
/// `daemon_exe` could not be launched at all.
pub async fn connect_or_spawn(socket_path: &Path, daemon_exe: &Path) -> io::Result<UnixStream> {
    if let Ok(stream) = try_connect(socket_path).await {
        return Ok(stream);
    }

    // No listener yet (or a stale socket file) -- spawn the daemon
    // detached. Never `.wait()` on the child: the daemon is meant to keep
    // running as a long-lived background process independent of this
    // client's own lifetime.
    std::process::Command::new(daemon_exe).spawn()?;

    for backoff in BACKOFF_MS {
        tokio::time::sleep(Duration::from_millis(*backoff)).await;
        if let Ok(stream) = try_connect(socket_path).await {
            return Ok(stream);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("daemon at {} did not become reachable after spawning {}", socket_path.display(), daemon_exe.display()),
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // Test-only fail-fast assertions -- mirrors this crate's other test modules' established convention (e.g. registry.rs/dispatch.rs/reconcile.rs), which this crate-wide #![deny] has never actually been enforced against with `cargo clippy --all-targets` until this plan's own verification pass.
mod tests {
    use super::*;

    /// The not-listening -> would-spawn decision, exercised without
    /// actually launching a real process: `try_connect` against a path
    /// that definitely has no listener (a nonexistent path under a fresh
    /// temp directory) fails -- exactly the condition `connect_or_spawn`
    /// treats as "proceed to spawn". The full spawn+backoff+reconnect path
    /// is exercised end-to-end by the real-binary `autostart_lifecycle`
    /// integration test (Task 3).
    #[tokio::test]
    async fn try_connect_against_a_socket_with_no_listener_fails_the_would_spawn_condition() {
        let dir = std::env::temp_dir().join(format!("rdpilot-daemon-autostart-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket_path = dir.join("no-listener.sock");
        // Deliberately never bound -- no daemon, no stale file, nothing.
        let result = try_connect(&socket_path).await;
        assert!(result.is_err(), "connecting to a path with no listener must fail (the would-spawn condition)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Control case: a real listener at the path IS reachable via
    /// `try_connect`, so `connect_or_spawn` would take its early-return
    /// path without ever touching the spawn branch.
    #[tokio::test]
    async fn try_connect_against_a_real_listener_succeeds() {
        let dir = std::env::temp_dir().join(format!("rdpilot-daemon-autostart-test-listener-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket_path = dir.join("listener.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind should succeed in a fresh temp dir");

        let (accept_result, connect_result) = tokio::join!(listener.accept(), try_connect(&socket_path));
        assert!(accept_result.is_ok());
        assert!(connect_result.is_ok(), "connecting to a real listener must succeed");

        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Structural guard: `BACKOFF_MS` matches research Pattern 3's exact
    /// bounded sequence.
    #[test]
    fn backoff_sequence_matches_research_pattern_3() {
        assert_eq!(BACKOFF_MS, &[50, 100, 200, 400, 800]);
    }
}
