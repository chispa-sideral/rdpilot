//! Local-user-scoped IPC transport — cfg-gated Unix/Windows listener +
//! shared framing (Plan 12-04; DAEMON-02).
//!
//! The Unix path (`unix.rs`) is fully implemented and offline-testable on
//! this Linux host: `0700` runtime-dir socket + per-connection
//! `peer_cred()` uid check. The Windows path (`windows.rs`) remains a
//! `#[cfg(windows)]` stub — explicit-DACL named-pipe security requires the
//! pinned Windows machine and is deferred to the live-gate Plan 12-07.

// `serve_connection` is only wired into the accept loop by `server.rs`
// (Plan 12-06); exercised directly by this wave's own tests until then
// (mirrors `registry.rs`'s identical interface-first rationale).
#![allow(dead_code)]

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

mod framing;

#[cfg(unix)]
pub use unix::{accept_and_authorize, authorize_uid, bind, socket_path};

#[cfg(windows)]
pub use windows::{accept_and_authorize, bind, socket_path};

use tokio::io::{AsyncRead, AsyncWrite};

use crate::dispatch::dispatch;
use crate::registry::Registry;
use framing::{read_frame, write_frame};

/// Serve one accepted connection: loop `read_frame::<Request>` ->
/// `dispatch` -> `write_frame::<WireResponse>` until the peer closes the
/// stream or a transport error occurs.
///
/// Generic over any `AsyncRead + AsyncWrite` stream — the same loop serves
/// both the Unix `UnixStream` (this plan) and the Windows named pipe
/// (Plan 12-07). Used by Plan 12-06's accept loop.
pub(crate) async fn serve_connection<S>(mut stream: S, registry: &Registry)
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
        let resp = dispatch(registry, req).await;
        if write_frame(&mut stream, &resp).await.is_err() {
            return;
        }
    }
}
