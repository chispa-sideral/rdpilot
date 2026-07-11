//! Daemon<->client transport primitives: length-prefixed `serde_json`
//! framing, well-known socket-path resolution, and the client-side
//! connect-or-spawn auto-start helper (Plan 13-01; CLI-01).
//!
//! Relocated verbatim from `rdpilot-daemon` (`ipc::framing`, `ipc::unix`'s
//! `socket_dir`/`socket_path`, and `autostart::connect_or_spawn`) so that a
//! thin CLI/MCP client can auto-start and reach the daemon over this same
//! transport WITHOUT ever depending on `rdpilot`/`rdpilot-daemon` — the only
//! change from the originals is the error type (this crate cannot reference
//! `rdpilot-daemon`'s `DaemonError`, so a crate-local [`TransportError`]
//! replaces it) and the `rdpilot_ipc`-relative crate paths.
//!
//! Unix-only for now (`#[cfg(unix)]`, matching the original `autostart.rs`):
//! the Windows named-pipe transport remains a stub deferred to the
//! Windows live-gate plan, so there is no Windows stream type to build this
//! module against yet on this development host.
//!
//! The daemon's single-instance guarantee, listener-side peer-uid
//! authorization (`bind`/`accept_and_authorize`/`authorize_uid`/
//! `effective_uid`), and PID-file-free design remain entirely in
//! `rdpilot-daemon::ipc::unix` — deliberately NOT moved here (T-13-03): a
//! CLI-side transport module has no business owning listener-side security
//! decisions.

#![cfg(unix)]

use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use directories::BaseDirs;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

/// A transport-layer error: I/O failure, frame encode failure, or frame
/// decode failure.
///
/// `rdpilot-ipc` cannot reference `rdpilot-daemon`'s `DaemonError` (the
/// dependency runs the other way), so this crate-local type replaces it
/// one-for-one at every call site the moved framing code used to return
/// `DaemonError::Io` from.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransportError {
    /// An underlying I/O failure (read/write/connect/spawn), or a declared
    /// frame length that violates protocol bounds.
    #[error("io error: {0}")]
    Io(String),
    /// A value failed to serialize into a frame body.
    #[error("frame encode failed: {0}")]
    Encode(String),
    /// A frame body failed to deserialize into the expected type.
    #[error("frame decode failed: {0}")]
    Decode(String),
}

// ---------------------------------------------------------------------
// Framing (relocated verbatim from `rdpilot-daemon/src/ipc/framing.rs`)
// ---------------------------------------------------------------------

/// The maximum accepted declared frame length, in bytes (16 MiB).
///
/// Guards against an untrusted client forcing a huge allocation via a
/// bogus declared length: the length prefix is checked BEFORE any body
/// bytes are read or a buffer of that size is allocated (T-13-01, V5 input
/// validation) — preserved exactly from the original `rdpilot-daemon`
/// implementation, not weakened by this relocation.
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Write `value` as one length-prefixed JSON frame to `w`.
///
/// Wire format: a 4-byte big-endian `u32` length prefix followed by that
/// many bytes of UTF-8 JSON body. Generic over any `AsyncWrite` — the same
/// function serves both the Unix `UnixStream` (this plan) and a future
/// Windows named pipe.
///
/// # Errors
///
/// Returns [`TransportError::Encode`] if serialization (or encoding the
/// length prefix) fails, or [`TransportError::Io`] if the underlying
/// transport write fails.
pub async fn write_frame<W, T>(w: &mut W, value: &T) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value).map_err(|e| TransportError::Encode(format!("frame encode failed: {e}")))?;
    let len = u32::try_from(body.len())
        .map_err(|_| TransportError::Encode("frame body exceeds u32::MAX bytes, cannot encode length prefix".to_owned()))?;
    w.write_all(&len.to_be_bytes()).await.map_err(|e| TransportError::Io(e.to_string()))?;
    w.write_all(&body).await.map_err(|e| TransportError::Io(e.to_string()))?;
    w.flush().await.map_err(|e| TransportError::Io(e.to_string()))?;
    Ok(())
}

/// Read one length-prefixed JSON frame from `r`.
///
/// # Errors
///
/// Returns [`TransportError::Io`] if the declared length exceeds
/// [`MAX_FRAME_LEN`] (rejected before any body allocation) or the
/// underlying transport read fails, or [`TransportError::Decode`] if the
/// body fails to deserialize as JSON.
pub async fn read_frame<R, T>(r: &mut R) -> Result<T, TransportError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0_u8; 4];
    r.read_exact(&mut len_buf).await.map_err(|e| TransportError::Io(e.to_string()))?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(TransportError::Io(format!(
            "declared frame length {len} exceeds the {MAX_FRAME_LEN}-byte cap"
        )));
    }

    let mut body = vec![0_u8; len as usize];
    r.read_exact(&mut body).await.map_err(|e| TransportError::Io(e.to_string()))?;
    serde_json::from_slice(&body).map_err(|e| TransportError::Decode(format!("frame decode failed: {e}")))
}

// ---------------------------------------------------------------------
// Socket-path resolution (relocated verbatim from
// `rdpilot-daemon/src/ipc/unix.rs` lines 39-56 — `bind`/
// `accept_and_authorize`/`authorize_uid`/`effective_uid` deliberately stay
// in the daemon, see module doc)
// ---------------------------------------------------------------------

/// Resolve (and create, if absent) the `0700`-mode runtime directory the
/// daemon socket lives under: `<XDG_RUNTIME_DIR>/rdpilot`, falling back to
/// `<cache_dir>/rdpilot` when no runtime dir is available on this
/// platform.
///
/// Mode is applied ATOMICALLY at creation via
/// [`std::os::unix::fs::DirBuilderExt::mode`] — never a post-creation
/// `chmod`, which would leave a brief TOCTOU window where the directory is
/// world-traversable (V4).
///
/// # Errors
///
/// Returns an error if no home/runtime directory can be resolved on this
/// platform, or if directory creation fails.
pub fn socket_dir() -> io::Result<PathBuf> {
    let base = BaseDirs::new()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not resolve a home/runtime directory on this platform"))?;
    let dir = base.runtime_dir().unwrap_or_else(|| base.cache_dir()).join("rdpilot");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700) // atomic at creation — no TOCTOU window (V4)
        .create(&dir)?;
    Ok(dir)
}

/// The daemon socket's full path: `<socket_dir>/daemon.sock`.
///
/// Both the daemon (binding) and any client (connecting/auto-spawning)
/// resolve this SAME implementation — a single shared resolver, not two
/// synchronized copies, closes the socket-path-drift tampering surface
/// (T-13-02).
///
/// # Errors
///
/// Propagates [`socket_dir`]'s errors.
pub fn socket_path() -> io::Result<PathBuf> {
    Ok(socket_dir()?.join("daemon.sock"))
}

// ---------------------------------------------------------------------
// Connect-or-spawn (relocated verbatim from
// `rdpilot-daemon/src/autostart.rs`)
// ---------------------------------------------------------------------

/// The bounded backoff sequence (milliseconds) `connect_or_spawn` retries
/// on after spawning the daemon.
const BACKOFF_MS: &[u64] = &[50, 100, 200, 400, 800];

/// Attempt one connection to `socket_path`. ANY failure (no such file,
/// connection refused, a stale special file left by a killed predecessor,
/// etc.) is uniformly treated as "not listening" by the caller -- this
/// function does not try to distinguish failure kinds.
async fn try_connect(socket_path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(socket_path).await
}

/// Connect to the daemon at `socket_path`, auto-starting it via
/// `daemon_exe` if it is not already listening (CLI-01/DAEMON-03).
///
/// Always tries a plain connect FIRST. Only on failure does it spawn
/// `daemon_exe` (detached -- never waited on; the daemon backgrounds
/// itself) and retry the connect over [`BACKOFF_MS`]'s bounded sequence,
/// returning the first successful stream.
///
/// Two clients racing to auto-start the daemon simultaneously both reach
/// this "spawn" branch and both spawn a daemon process -- this is a benign,
/// expected outcome: exactly one daemon process wins the bind on
/// `socket_path` and keeps running; the loser's `server::run()` observes
/// `AddrInUse` and exits immediately without error. Both racing clients'
/// backoff-retry loop below eventually connects to whichever daemon won.
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
#[allow(clippy::expect_used, clippy::unwrap_used)] // Test-only fail-fast assertions -- mirrors this crate's other test modules' established convention.
mod tests {
    use serde::Deserialize;
    use tokio::io::AsyncWriteExt as _;

    use super::*;

    // --- Framing tests (moved from `rdpilot-daemon/src/ipc/framing.rs`) ---

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        a: String,
        b: u32,
    }

    #[tokio::test]
    async fn write_frame_then_read_frame_round_trips_over_a_duplex_pipe() -> Result<(), Box<dyn std::error::Error>> {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let value = Sample { a: "hello".to_owned(), b: 42 };

        write_frame(&mut client, &value).await?;
        let received: Sample = read_frame(&mut server).await?;

        assert_eq!(received, value);
        Ok(())
    }

    #[tokio::test]
    async fn an_oversized_declared_length_is_rejected_before_allocating_the_body() {
        let (mut client, mut server) = tokio::io::duplex(4096);

        // Write only the 4-byte length prefix (declaring a length far
        // beyond MAX_FRAME_LEN) and nothing else — if `read_frame` tried to
        // allocate/read the declared body length before checking the cap,
        // this test would hang waiting for bytes that never arrive rather
        // than failing fast.
        let oversized = MAX_FRAME_LEN + 1;
        client
            .write_all(&oversized.to_be_bytes())
            .await
            .expect("writing the length prefix must succeed");
        drop(client); // no body bytes ever follow

        let result: Result<Sample, TransportError> = read_frame(&mut server).await;
        assert!(matches!(result, Err(TransportError::Io(_))), "expected an Io error for an oversized frame, got {result:?}");
    }

    #[tokio::test]
    async fn read_frame_on_a_closed_stream_with_no_bytes_is_an_io_error() {
        let (client, mut server) = tokio::io::duplex(4096);
        drop(client);

        let result: Result<Sample, TransportError> = read_frame(&mut server).await;
        assert!(matches!(result, Err(TransportError::Io(_))));
    }

    // --- Socket-path tests (moved from `rdpilot-daemon/src/ipc/unix.rs`) ---

    #[test]
    fn socket_dir_is_created_with_mode_0700() {
        use std::os::unix::fs::MetadataExt;

        let dir = socket_dir().expect("socket_dir should succeed on this host");
        let meta = std::fs::metadata(&dir).expect("stat should succeed on a freshly created dir");
        assert_eq!(meta.mode() & 0o777, 0o700, "expected mode 0700, got {:o}", meta.mode() & 0o777);
    }

    // --- Connect-or-spawn tests (moved from
    // `rdpilot-daemon/src/autostart.rs`) ---

    /// The not-listening -> would-spawn decision, exercised without
    /// actually launching a real process: `try_connect` against a path
    /// that definitely has no listener (a nonexistent path under a fresh
    /// temp directory) fails -- exactly the condition `connect_or_spawn`
    /// treats as "proceed to spawn". The full spawn+backoff+reconnect path
    /// is exercised end-to-end by the real-binary `autostart_lifecycle`
    /// integration test in `rdpilot-daemon`.
    #[tokio::test]
    async fn try_connect_against_a_socket_with_no_listener_fails_the_would_spawn_condition() {
        let dir = std::env::temp_dir().join(format!("rdpilot-ipc-transport-autostart-test-{}", std::process::id()));
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
        let dir = std::env::temp_dir().join(format!("rdpilot-ipc-transport-autostart-test-listener-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let socket_path = dir.join("listener.sock");
        let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind should succeed in a fresh temp dir");

        let (accept_result, connect_result) = tokio::join!(listener.accept(), try_connect(&socket_path));
        assert!(accept_result.is_ok());
        assert!(connect_result.is_ok(), "connecting to a real listener must succeed");

        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Structural guard: `BACKOFF_MS` matches the exact bounded sequence
    /// the original `rdpilot-daemon::autostart` module used.
    #[test]
    fn backoff_sequence_is_unchanged_by_the_relocation() {
        assert_eq!(BACKOFF_MS, &[50, 100, 200, 400, 800]);
    }
}
