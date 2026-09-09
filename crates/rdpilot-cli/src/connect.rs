//! The transport client: resolve the daemon socket + sibling
//! `rdpilot-daemon` binary, auto-start it via
//! `rdpilot_ipc::transport::connect_or_spawn` on first use (CLI-01), and
//! preflight every fresh stream with a credential-free `List` compatibility
//! probe before dispatching the requested frame (this binary is an
//! invoke-and-exit process, never a persistent client).

use rdpilot_ipc::{
    IPC_COMPATIBILITY_VERSION, Request, WireResponse, connect_or_spawn, read_frame, socket_path, write_frame,
};
use tokio::io::{AsyncRead, AsyncWrite};
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

async fn write_and_read<S>(stream: &mut S, request: &Request) -> Result<WireResponse, CliError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_frame(stream, request).await.map_err(|error| CliError::Transport(error.to_string()))?;
    read_frame(stream).await.map_err(|error| CliError::Transport(error.to_string()))
}

fn validate_preflight(response: &WireResponse) -> Result<(), CliError> {
    match response {
        WireResponse::SessionList { compatibility_version: Some(version), .. }
            if *version == IPC_COMPATIBILITY_VERSION => Ok(()),
        WireResponse::SessionList { compatibility_version, .. } => Err(CliError::DaemonIncompatible(*compatibility_version)),
        WireResponse::Error(error) => Err(error.clone().into()),
        other => Err(CliError::Internal(format!("unexpected response to compatibility List probe: {other:?}"))),
    }
}

/// Send a credential-free List probe, validate its daemon identity, then
/// send `req`. A user-requested List returns the validated probe result and
/// does not send a duplicate List frame.
async fn verified_round_trip<S>(stream: &mut S, req: Request) -> Result<WireResponse, CliError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let preflight = write_and_read(stream, &Request::List {}).await?;
    validate_preflight(&preflight)?;
    if matches!(req, Request::List {}) {
        return Ok(preflight);
    }
    write_and_read(stream, &req).await
}

/// Open a stream (auto-starting the daemon if needed), validate the daemon's
/// compatibility identity, then send `req`.
///
/// # Errors
///
/// Propagates [`open_stream`]'s errors, or [`CliError::Transport`] if the
/// frame write/read itself fails.
pub async fn round_trip(req: Request) -> Result<WireResponse, CliError> {
    let mut stream = open_stream().await?;
    verified_round_trip(&mut stream, req).await
}

pub async fn connect_round_trip(req: Request) -> Result<WireResponse, CliError> {
    let mut stream = open_stream().await?;
    let response = verified_round_trip(&mut stream, req).await?;
    let WireResponse::Connected { session, connect_ack_required, .. } = &response else {
        return Ok(response);
    };
    if *connect_ack_required {
        match write_and_read(&mut stream, &Request::ConnectAck { session: session.clone() }).await? {
            WireResponse::Ack => {}
            WireResponse::Error(error) => return Err(CliError::from(error)),
            other => return Err(CliError::Internal(format!("unexpected response to ConnectAck: {other:?}"))),
        }
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn verified_list_uses_its_preflight_response_without_a_second_frame() {
        let (mut client, mut daemon) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            assert!(matches!(read_frame::<_, Request>(&mut daemon).await.unwrap(), Request::List {}));
            write_frame(
                &mut daemon,
                &WireResponse::SessionList { sessions: vec![], compatibility_version: Some(IPC_COMPATIBILITY_VERSION) },
            )
            .await
            .unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_millis(10), read_frame::<_, Request>(&mut daemon)).await.is_err());
        });
        assert!(matches!(verified_round_trip(&mut client, Request::List {}).await, Ok(WireResponse::SessionList { .. })));
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn incompatible_daemon_never_receives_the_requested_frame() {
        let (mut client, mut daemon) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            assert!(matches!(read_frame::<_, Request>(&mut daemon).await.unwrap(), Request::List {}));
            write_frame(&mut daemon, &WireResponse::SessionList { sessions: vec![], compatibility_version: None }).await.unwrap();
            assert!(tokio::time::timeout(std::time::Duration::from_millis(10), read_frame::<_, Request>(&mut daemon)).await.is_err());
        });
        assert!(matches!(
            verified_round_trip(
                &mut client,
                Request::Ping { session: "test".parse().unwrap_or_else(|_| unreachable!()) }
            )
            .await,
            Err(CliError::DaemonIncompatible(None))
        ));
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn compatible_daemon_receives_preflight_before_the_requested_frame() {
        let (mut client, mut daemon) = tokio::io::duplex(4096);
        let peer = tokio::spawn(async move {
            assert!(matches!(read_frame::<_, Request>(&mut daemon).await.unwrap(), Request::List {}));
            write_frame(
                &mut daemon,
                &WireResponse::SessionList { sessions: vec![], compatibility_version: Some(IPC_COMPATIBILITY_VERSION) },
            )
            .await
            .unwrap();
            assert!(matches!(read_frame::<_, Request>(&mut daemon).await.unwrap(), Request::Ping { .. }));
            write_frame(&mut daemon, &WireResponse::Ack).await.unwrap();
        });
        let request = Request::Ping { session: "test".parse().unwrap_or_else(|_| unreachable!()) };
        assert!(matches!(verified_round_trip(&mut client, request).await, Ok(WireResponse::Ack)));
        peer.await.unwrap();
    }
}
