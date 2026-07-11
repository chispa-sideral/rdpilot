//! The transport client: resolve the daemon socket + sibling
//! `rdpilot-daemon` binary, auto-start it via
//! `rdpilot_ipc::transport::connect_or_spawn` on first use (CLI-01), and
//! round-trip exactly one `Request`/`WireResponse` frame pair per verb
//! invocation (this binary is an invoke-and-exit process, never a
//! persistent client).

use rdpilot_ipc::{Request, WireResponse, connect_or_spawn, read_frame, socket_path, write_frame};
use tokio::net::UnixStream;

use crate::exit_codes::CliError;

/// The sibling `rdpilot-daemon` binary's file name, installed alongside
/// this binary (production convention; `tests/cli_lifecycle.rs` locates the
/// same sibling binary the same way).
#[cfg(windows)]
const DAEMON_BINARY_NAME: &str = "rdpilot-daemon.exe";
#[cfg(not(windows))]
const DAEMON_BINARY_NAME: &str = "rdpilot-daemon";

/// Resolve the well-known daemon socket path and the sibling
/// `rdpilot-daemon` executable, then connect — auto-starting the daemon on
/// first use if it is not already listening (CLI-01/DAEMON-03).
///
/// # Errors
///
/// Returns [`CliError::DaemonUnreachable`] if the socket path or this
/// binary's own executable path cannot be resolved, or if
/// `connect_or_spawn`'s bounded backoff exhausts without the daemon
/// becoming reachable.
pub async fn open_stream() -> Result<UnixStream, CliError> {
    let socket = socket_path().map_err(|e| CliError::DaemonUnreachable(e.to_string()))?;
    let daemon_exe = std::env::current_exe()
        .map_err(|e| CliError::DaemonUnreachable(e.to_string()))?
        .with_file_name(DAEMON_BINARY_NAME);
    connect_or_spawn(&socket, &daemon_exe).await.map_err(|e| CliError::DaemonUnreachable(e.to_string()))
}

/// Open a stream (auto-starting the daemon if needed) and send `req`,
/// returning exactly one `WireResponse` frame read back.
///
/// # Errors
///
/// Propagates [`open_stream`]'s errors, or [`CliError::Transport`] if the
/// frame write/read itself fails.
pub async fn round_trip(req: Request) -> Result<WireResponse, CliError> {
    let mut stream = open_stream().await?;
    write_frame(&mut stream, &req).await.map_err(|e| CliError::Transport(e.to_string()))?;
    read_frame(&mut stream).await.map_err(|e| CliError::Transport(e.to_string()))
}
