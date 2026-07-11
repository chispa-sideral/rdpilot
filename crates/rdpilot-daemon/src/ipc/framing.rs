//! Length-prefixed `serde_json` read/write framing over an accepted
//! Unix/Windows local-socket stream (Plan 12-04).
//!
//! Wire format: a 4-byte big-endian `u32` length prefix followed by that
//! many bytes of UTF-8 JSON body. Generic over any `AsyncRead`/
//! `AsyncWrite` — the same functions serve both the Unix `UnixStream`
//! (this plan) and the Windows named pipe (Plan 12-07).

// Called by `serve_connection` (`ipc/mod.rs`), which is itself wired into
// the accept loop by `server.rs` (Plan 12-06); exercised directly by this
// file's own tests until then (mirrors `registry.rs`'s identical
// interface-first rationale).
#![allow(dead_code)]

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::seams::DaemonError;

/// The maximum accepted declared frame length, in bytes (16 MiB).
///
/// Guards against an untrusted client forcing a huge allocation via a
/// bogus declared length: the length prefix is checked BEFORE any body
/// bytes are read or a buffer of that size is allocated (T-12-11, V5 input
/// validation).
const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Write `value` as one length-prefixed JSON frame to `w`.
///
/// # Errors
///
/// Returns [`DaemonError::Io`] if serialization or the underlying
/// transport write fails.
pub async fn write_frame<W, T>(w: &mut W, value: &T) -> Result<(), DaemonError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value).map_err(|e| DaemonError::Io(format!("frame encode failed: {e}")))?;
    let len = u32::try_from(body.len())
        .map_err(|_| DaemonError::Io("frame body exceeds u32::MAX bytes, cannot encode length prefix".to_owned()))?;
    w.write_all(&len.to_be_bytes()).await.map_err(|e| DaemonError::Io(e.to_string()))?;
    w.write_all(&body).await.map_err(|e| DaemonError::Io(e.to_string()))?;
    w.flush().await.map_err(|e| DaemonError::Io(e.to_string()))?;
    Ok(())
}

/// Read one length-prefixed JSON frame from `r`.
///
/// # Errors
///
/// Returns [`DaemonError::Io`] if the declared length exceeds
/// [`MAX_FRAME_LEN`] (rejected before any body allocation), or if the
/// underlying transport read or JSON decode fails.
pub async fn read_frame<R, T>(r: &mut R) -> Result<T, DaemonError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0_u8; 4];
    r.read_exact(&mut len_buf).await.map_err(|e| DaemonError::Io(e.to_string()))?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(DaemonError::Io(format!(
            "declared frame length {len} exceeds the {MAX_FRAME_LEN}-byte cap"
        )));
    }

    let mut body = vec![0_u8; len as usize];
    r.read_exact(&mut body).await.map_err(|e| DaemonError::Io(e.to_string()))?;
    serde_json::from_slice(&body).map_err(|e| DaemonError::Io(format!("frame decode failed: {e}")))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use tokio::io::AsyncWriteExt;

    use super::*;

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

        let result: Result<Sample, DaemonError> = read_frame(&mut server).await;
        assert!(matches!(result, Err(DaemonError::Io(_))), "expected an Io error for an oversized frame, got {result:?}");
    }

    #[tokio::test]
    async fn read_frame_on_a_closed_stream_with_no_bytes_is_an_io_error() {
        let (client, mut server) = tokio::io::duplex(4096);
        drop(client);

        let result: Result<Sample, DaemonError> = read_frame(&mut server).await;
        assert!(matches!(result, Err(DaemonError::Io(_))));
    }
}
