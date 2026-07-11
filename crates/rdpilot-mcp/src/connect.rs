//! The transport client: resolve the daemon socket + sibling
//! `rdpilot-daemon` binary, auto-start it via
//! `rdpilot_ipc::transport::connect_or_spawn` on first use, and round-trip
//! exactly one `Request`/`WireResponse` frame pair per tool call over a
//! FRESH `UnixStream` (never pooled/shared — a shared stream would
//! reintroduce the head-of-line blocking MCP-06 forbids: a slow `Put`/`Get`
//! would occupy the one stream and stall every other call's framing on it).
//!
//! [`round_trip_bounded`] adds the MCP-06 Layer 3 mechanism this crate
//! contributes on top of that already-proven Phase 13 transport: an
//! explicit `tokio::time::timeout` around the whole round trip, per
//! [`crate::timeouts`]'s verb-class bound. An exceeded bound never hangs
//! the calling agent's loop indefinitely — it maps to
//! [`crate::error::McpError::Timeout`].

use std::future::Future;
use std::time::Duration;

use rdpilot_ipc::{Request, WireResponse, connect_or_spawn, read_frame, socket_path, write_frame};
use tokio::net::UnixStream;

use crate::error::McpError;

/// The sibling `rdpilot-daemon` binary's file name, installed alongside
/// this binary — the same convention `rdpilot-cli::connect` uses.
// Declared now (this plan's transport spine); consumed via `open_stream`
// below, which itself has no caller yet — see that function's own
// `#[allow(dead_code)]` note.
#[allow(dead_code)]
#[cfg(windows)]
const DAEMON_BINARY_NAME: &str = "rdpilot-daemon.exe";
#[allow(dead_code)]
#[cfg(not(windows))]
const DAEMON_BINARY_NAME: &str = "rdpilot-daemon";

/// Resolve the well-known daemon socket path and the sibling
/// `rdpilot-daemon` executable, then connect — auto-starting the daemon on
/// first use if it is not already listening.
///
/// # Errors
///
/// Returns [`McpError::DaemonUnreachable`] if the socket path or this
/// binary's own executable path cannot be resolved, or if
/// `connect_or_spawn`'s bounded backoff exhausts without the daemon
/// becoming reachable.
// Declared now (this plan's transport spine, 14-02); no `#[tool]` method
// calls it yet — Plans 14-03/14-04 add the tool handlers that do. Mirrors
// `rdpilot-cli::exit_codes::CliError::NoClobber`'s identical
// "declared now, consumed by a later plan" precedent.
#[allow(dead_code)]
pub async fn open_stream() -> Result<UnixStream, McpError> {
    let socket = socket_path().map_err(McpError::daemon_unreachable)?;
    let daemon_exe = std::env::current_exe()
        .map_err(McpError::daemon_unreachable)?
        .with_file_name(DAEMON_BINARY_NAME);
    connect_or_spawn(&socket, &daemon_exe).await.map_err(McpError::daemon_unreachable)
}

/// Open a FRESH stream (auto-starting the daemon if needed) and send `req`,
/// returning exactly one `WireResponse` frame read back. Never call this
/// directly from a tool handler — go through [`round_trip_bounded`] so
/// every daemon round trip carries an explicit MCP-06 bound.
///
/// # Errors
///
/// Propagates [`open_stream`]'s errors, or [`McpError::Transport`] if the
/// frame write/read itself fails.
#[allow(dead_code)] // see `open_stream`'s note above
async fn round_trip(req: Request) -> Result<WireResponse, McpError> {
    let mut stream = open_stream().await?;
    write_frame(&mut stream, &req).await.map_err(McpError::transport)?;
    read_frame(&mut stream).await.map_err(McpError::transport)
}

/// Wrap `fut` in `tokio::time::timeout(bound, fut)`, mapping an elapsed
/// bound to [`McpError::Timeout`]. Factored out of [`round_trip_bounded`]
/// so the timeout-mapping behavior itself is directly unit-testable against
/// a pending future, without needing a live daemon socket.
#[allow(dead_code)] // see `open_stream`'s note above; exercised directly by this module's tests
async fn timeout_wrap<F, T>(bound: Duration, fut: F) -> Result<T, McpError>
where
    F: Future<Output = Result<T, McpError>>,
{
    tokio::time::timeout(bound, fut).await.map_err(|_elapsed| McpError::Timeout(bound))?
}

/// Every tool handler's ONLY entry point into the daemon: a fresh one-shot
/// round trip, bounded by `bound` (one of [`crate::timeouts`]'s per-verb-class
/// constants). MCP-06: an exceeded bound yields [`McpError::Timeout`], never
/// an indefinite hang.
///
/// # Errors
///
/// [`McpError::Timeout`] if `bound` elapses before the round trip
/// completes; otherwise propagates [`round_trip`]'s errors.
#[allow(dead_code)] // see `open_stream`'s note above
pub async fn round_trip_bounded(req: Request, bound: Duration) -> Result<WireResponse, McpError> {
    timeout_wrap(bound, round_trip(req)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MCP-06's core behavioral contract: `round_trip_bounded(req,
    /// Duration::ZERO)` against a round trip that never resolves maps the
    /// elapsed timeout to `McpError::Timeout` — not a hang, not a
    /// transport error. Exercised directly against `timeout_wrap` (the
    /// exact wrapper `round_trip_bounded` uses) with a genuinely pending
    /// future, so this test needs no live daemon socket to prove the
    /// mapping.
    #[tokio::test]
    async fn timeout_wrap_maps_a_pending_future_to_mcp_error_timeout() {
        let bound = Duration::ZERO;
        let never = std::future::pending::<Result<WireResponse, McpError>>();

        let result = timeout_wrap(bound, never).await;

        match result {
            Err(McpError::Timeout(elapsed_bound)) => assert_eq!(elapsed_bound, bound),
            other => panic!("expected Err(McpError::Timeout(Duration::ZERO)), got {other:?}"),
        }
    }

    /// A future that resolves well within the bound is passed through
    /// unchanged — `timeout_wrap` only intercepts the elapsed case.
    #[tokio::test]
    async fn timeout_wrap_passes_through_a_fast_ready_future() {
        let bound = Duration::from_secs(15);
        let immediate = async { Ok(()) };

        let result: Result<(), McpError> = timeout_wrap(bound, immediate).await;

        assert!(result.is_ok());
    }
}
