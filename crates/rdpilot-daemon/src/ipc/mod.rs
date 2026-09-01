//! Local-user-scoped IPC transport — cfg-gated Unix/Windows listener
//! (Plan 12-04; DAEMON-02).
//!
//! The Unix path (`unix.rs`) is fully implemented and offline-testable on
//! this Linux host: `0700` runtime-dir socket + per-connection
//! `peer_cred()` uid check. The Windows path (`windows.rs`, Plan 15-01,
//! closing 12-07's Windows half of DAEMON-02) is authored — explicit
//! owner-only DACL + `first_pipe_instance(true)` — but its
//! `#[cfg(windows)]` gate means this Linux host never compiles it; the
//! real Windows compile+run is confirmed on the pinned Azure VM in
//! Plan 15-05.
//!
//! Socket-path resolution and length-prefixed JSON framing are shared with
//! any thin client via `rdpilot_ipc::transport` (Plan 13-01) — this module
//! only owns the LISTENER-side security primitives that stay daemon-only
//! (bind/accept_and_authorize/authorize_uid, T-13-03).

// `serve_connection` is only wired into the accept loop by `server.rs`
// (Plan 12-06); exercised directly by this wave's own tests until then
// (mirrors `registry.rs`'s identical interface-first rationale).
#![allow(dead_code)]

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{accept_and_authorize, authorize_uid, bind};
#[cfg(unix)]
pub use rdpilot_ipc::transport::socket_path;

#[cfg(windows)]
pub use windows::{accept_and_authorize, bind, socket_path};

use tokio::io::{AsyncRead, AsyncWrite};

use crate::diagnostics::{Diagnostics, Stage};
use crate::dispatch::dispatch_for_ipc;
use crate::registry::Registry;
use rdpilot_ipc::transport::{read_frame, write_frame};

/// Serve one accepted connection: loop `read_frame::<Request>` ->
/// `dispatch` -> `write_frame::<WireResponse>` until the peer closes the
/// stream or a transport error occurs.
///
/// Generic over any `AsyncRead + AsyncWrite` stream — the same loop serves
/// both the Unix `UnixStream` (this plan) and the Windows named pipe
/// (Plan 12-07). Used by Plan 12-06's accept loop.
pub(crate) async fn serve_connection<S>(mut stream: S, registry: &Registry, diagnostics: Option<&Diagnostics>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let req = match read_frame(&mut stream).await {
            Ok(req) => req,
            // Transport closed or malformed frame — end the connection.
            // Deliberately not logged with request content (D-31 applies
            // to the happy path too; nothing here ever had access to a
            // decoded `Request` to begin with on this branch).
            Err(_) => return,
        };
        let outcome = dispatch_for_ipc(registry, req, diagnostics).await;
        if write_frame(&mut stream, &outcome.response).await.is_err() {
            if let Some(lease) = outcome.connect_lease {
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(lease.id.as_str(), Stage::IpcPeerClosed);
                }
                if matches!(registry.close_if_generation(&lease).await, Ok(true)) {
                    if let Some(diagnostics) = diagnostics {
                        diagnostics.record(lease.id.as_str(), Stage::RegistryClosed);
                    }
                }
            }
            return;
        }
        if let Some(lease) = outcome.connect_lease {
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(lease.id.as_str(), Stage::IpcResponseWritten);
            }
        }
    }
}
