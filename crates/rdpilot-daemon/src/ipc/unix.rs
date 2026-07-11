//! Unix local-user-scoped transport: `0700` runtime-dir socket +
//! `tokio::net::UnixStream::peer_cred()` uid check (Plan 12-04; DAEMON-02,
//! research Pitfall 5).
//!
//! Socket-path resolution (`socket_dir`/`socket_path`) was relocated into
//! `rdpilot_ipc::transport` (Plan 13-01) so a thin CLI/MCP client resolves
//! the SAME path without depending on this crate (T-13-02) — this module
//! now only owns the LISTENER-side security primitives (T-13-03):
//!
//! - [`bind`] creates the listener at `rdpilot_ipc::transport::socket_path()`,
//!   cleaning up a stale socket file left by a killed predecessor
//!   (connect-then-unlink-then-rebind) — it never blindly reuses a path it
//!   finds on disk (T-12-10).
//! - [`accept_and_authorize`] accepts a connection and rejects it unless
//!   the peer's uid matches the daemon's own effective uid (T-12-09).
//! - [`authorize_uid`] is the pure decision function factored out of
//!   [`accept_and_authorize`] so it is directly unit-testable without a
//!   real socket (Task 3, SC#4 [BLOCKING]).

use std::io;

use rdpilot_ipc::transport::socket_path;
use tokio::net::{UnixListener, UnixStream};

/// Bind the daemon's Unix listener at [`socket_path`].
///
/// If a socket file already exists at that path, this function first
/// attempts to CONNECT to it: a successful connect means another daemon
/// instance is genuinely live, and `bind` returns an `AddrInUse` error
/// (Plan 12-06's bind-as-mutex pattern uses this to exit cleanly rather
/// than compete with a running daemon). A failed connect means the file is
/// stale — left behind by a killed predecessor (`kill -9`, T-12-10) — and
/// is removed before rebinding. The stale path is NEVER reused blindly.
///
/// # Errors
///
/// Returns [`io::ErrorKind::AddrInUse`] if a live daemon already holds the
/// path, or the underlying filesystem/bind error otherwise.
pub async fn bind() -> io::Result<UnixListener> {
    let path = socket_path()?;
    if path.exists() {
        match UnixStream::connect(&path).await {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("a daemon is already listening on {}", path.display()),
                ));
            }
            Err(_) => {
                // Stale socket left by a killed predecessor — remove it
                // before rebinding (never a blind reuse, T-12-10).
                std::fs::remove_file(&path)?;
            }
        }
    }
    UnixListener::bind(&path)
}

/// The daemon's own effective uid, via `libc::geteuid()` — the
/// authorization baseline every accepted connection's peer uid is compared
/// against.
///
/// The ONLY `unsafe` in this crate (the crate defaults to
/// `#![deny(unsafe_code)]`; this is a deliberate, localized,
/// `#[allow]`-annotated exception): `geteuid()` takes no arguments, cannot
/// fail, and has no aliasing/lifetime hazards to uphold — FFI is required
/// only because it is a raw libc call with no safe Rust wrapper in this
/// crate's dependency set.
#[allow(unsafe_code)]
fn effective_uid() -> u32 {
    // SAFETY: `geteuid()` is a pure syscall wrapper with no arguments, no
    // failure mode, and no memory/aliasing precondition — it is always
    // safe to call.
    unsafe { libc::geteuid() }
}

/// The pure per-connection authorization decision (DAEMON-02): a `peer_uid`
/// that does not match `our_uid` is rejected. Factored out of
/// [`accept_and_authorize`] so it is directly unit-testable offline,
/// without a real socket connection (Task 3, SC#4 [BLOCKING]).
///
/// # Errors
///
/// Returns an [`io::ErrorKind::PermissionDenied`] error when
/// `peer_uid != our_uid`.
pub fn authorize_uid(peer_uid: u32, our_uid: u32) -> io::Result<()> {
    if peer_uid == our_uid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("peer uid {peer_uid} does not match the daemon's effective uid {our_uid}"),
        ))
    }
}

/// Accept one connection from `listener` and authorize its peer uid
/// (DAEMON-02, T-12-09) before handing the stream back to the caller.
///
/// # Errors
///
/// Returns the underlying `accept()`/`peer_cred()` I/O error verbatim, or
/// [`io::ErrorKind::PermissionDenied`] (via [`authorize_uid`]) when the
/// peer's uid does not match the daemon's own effective uid.
pub async fn accept_and_authorize(listener: &UnixListener) -> io::Result<UnixStream> {
    let (stream, _addr) = listener.accept().await?;
    let peer = stream.peer_cred()?; // stable tokio API (research: std's equivalent is nightly-only)
    authorize_uid(peer.uid(), effective_uid())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_uid_accepts_a_matching_uid_and_rejects_a_mismatch() {
        assert!(authorize_uid(1000, 1000).is_ok());
        let err = authorize_uid(1000, 1001).expect_err("a mismatched uid must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn accept_and_authorize_accepts_a_same_uid_connection() {
        let listener = bind().await.expect("bind should succeed on this host");
        let path = socket_path().expect("socket_path should resolve");

        // The test process connects to itself: its own peer uid always
        // matches the daemon's own effective uid, so this is the
        // same-uid control case (the genuine different-uid rejection is
        // Task 3's BLOCKING integration test).
        let (accepted, connected) = tokio::join!(accept_and_authorize(&listener), UnixStream::connect(&path));

        assert!(accepted.is_ok(), "a same-uid connection must be authorized: {accepted:?}");
        assert!(connected.is_ok());

        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
