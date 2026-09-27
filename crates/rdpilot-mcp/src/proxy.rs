use rdpilot_ipc::{
    read_frame, write_frame, CuaStreamFrame, Request, SessionId, WireResponse,
    IPC_COMPATIBILITY_VERSION,
};
use std::{io, time::Duration};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt};

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LINE: usize = rdpilot_ipc::transport::MAX_FRAME_LEN as usize;

fn failure(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

pub async fn attach<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    session: SessionId,
) -> io::Result<()> {
    tokio::time::timeout(ATTACH_TIMEOUT, async {
        write_frame(stream, &Request::List {})
            .await
            .map_err(|e| failure(e.to_string()))?;
        match read_frame::<_, WireResponse>(stream)
            .await
            .map_err(|e| failure(e.to_string()))?
        {
            WireResponse::SessionList {
                compatibility_version: Some(IPC_COMPATIBILITY_VERSION),
                ..
            } => (),
            WireResponse::SessionList {
                compatibility_version,
                ..
            } => {
                return Err(failure(rdpilot_ipc::daemon_incompatible_message(
                    compatibility_version,
                )))
            }
            _ => return Err(failure("daemon compatibility handshake failed")),
        }
        write_frame(stream, &Request::CuaAttach { session })
            .await
            .map_err(|e| failure(e.to_string()))?;
        match read_frame::<_, WireResponse>(stream)
            .await
            .map_err(|e| failure(e.to_string()))?
        {
            WireResponse::CuaAttached { .. } => Ok(()),
            WireResponse::Error(e) => Err(failure(e.message)),
            _ => Err(failure("daemon did not acknowledge Cua attachment")),
        }
    })
    .await
    .map_err(|_| failure("Cua attachment timed out"))?
}

/// Check each chunk before extending: a peer cannot allocate an unbounded line.
async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<serde_json::Value>> {
    let mut line = Vec::new();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(failure("unterminated MCP input line"))
            };
        }
        let count = chunk
            .iter()
            .position(|&b| b == b'\n')
            .map_or(chunk.len(), |n| n + 1);
        if line.len() + count > MAX_LINE {
            return Err(failure("MCP input exceeds frame limit"));
        }
        line.extend_from_slice(&chunk[..count]);
        reader.consume(count);
        if line.last() == Some(&b'\n') {
            return serde_json::from_slice(&line)
                .map(Some)
                .map_err(|_| failure("malformed MCP JSON"));
        }
    }
}

/// Independent futures preserve partial frame/line reads. Completion of either direction
/// cancels the other and drops the stream; a new generation requires an explicit invocation.
pub async fn forward<S, R, W>(stream: S, mut input: R, mut output: W) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut incoming, mut outgoing) = tokio::io::split(stream);
    let to_cua = async {
        while let Some(message) = read_line(&mut input).await? {
            tokio::time::timeout(
                WRITE_TIMEOUT,
                write_frame(&mut outgoing, &CuaStreamFrame::Message { message }),
            )
            .await
            .map_err(|_| failure("Cua input write timed out"))?
            .map_err(|e| failure(e.to_string()))?;
        }
        // Caller EOF tears down the runtime; never consume more local input or replay it.
        let _ = tokio::time::timeout(
            WRITE_TIMEOUT,
            write_frame(
                &mut outgoing,
                &CuaStreamFrame::Closed {
                    reason: "caller EOF".into(),
                },
            ),
        )
        .await;
        Ok(())
    };
    let from_cua = async {
        loop {
            match read_frame::<_, CuaStreamFrame>(&mut incoming)
                .await
                .map_err(|e| failure(e.to_string()))?
            {
                CuaStreamFrame::Message { message } => {
                    let mut line =
                        serde_json::to_vec(&message).map_err(|e| failure(e.to_string()))?;
                    if line.len() + 1 > MAX_LINE {
                        return Err(failure("MCP output exceeds line limit"));
                    }
                    line.push(b'\n');
                    tokio::time::timeout(WRITE_TIMEOUT, async {
                        output.write_all(&line).await?;
                        output.flush().await
                    })
                    .await
                    .map_err(|_| failure("MCP stdout write timed out"))??;
                }
                CuaStreamFrame::Closed { reason } => {
                    return Err(failure(format!("Cua attachment closed: {reason}")))
                }
            }
        }
    };
    tokio::select! { result = to_cua => result, result = from_cua => result }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, BufReader};

    #[tokio::test]
    async fn opaque_messages_both_directions_survive_fragmented_input_and_large_images() {
        let (client, mut daemon) = tokio::io::duplex(1024);
        let (mut stdin, input) = tokio::io::duplex(1024);
        let (output, stdout) = tokio::io::duplex(1024);
        let proxy = tokio::spawn(forward(client, BufReader::new(input), output));
        let mut stdout = BufReader::new(stdout);
        // A partial input line must not suppress or corrupt unsolicited output.
        stdin
            .write_all(br#"{"jsonrpc":"2.0","id":"client","method":"tools/"#)
            .await
            .unwrap();
        let image = json!({"jsonrpc":"2.0","method":"notifications/image","params":{"data":"X".repeat(100_000)}});
        let output_task = tokio::spawn(async move {
            write_frame(
                &mut daemon,
                &CuaStreamFrame::Message {
                    message: image.clone(),
                },
            )
            .await
            .unwrap();
            (daemon, image)
        });
        let mut line = String::new();
        stdout.read_line(&mut line).await.unwrap();
        let (mut daemon, image) = output_task.await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            image
        );
        stdin.write_all(b"list\"}\n").await.unwrap();
        assert_eq!(
            read_frame::<_, CuaStreamFrame>(&mut daemon).await.unwrap(),
            CuaStreamFrame::Message {
                message: json!({"jsonrpc":"2.0","id":"client","method":"tools/list"})
            }
        );
        // Reverse requests retain IDs/types, even when equal to a caller's ID.
        let request = json!({"jsonrpc":"2.0","id":"client","method":"sampling/createMessage","params":{"x":true}});
        write_frame(
            &mut daemon,
            &CuaStreamFrame::Message {
                message: request.clone(),
            },
        )
        .await
        .unwrap();
        line.clear();
        stdout.read_line(&mut line).await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&line).unwrap(),
            request
        );
        stdin
            .write_all(b"{\"id\":null,\"result\":{}}\n")
            .await
            .unwrap();
        assert_eq!(
            read_frame::<_, CuaStreamFrame>(&mut daemon).await.unwrap(),
            CuaStreamFrame::Message {
                message: json!({"id":null,"result":{}})
            }
        );
        drop(stdin);
        assert!(matches!(
            read_frame::<_, CuaStreamFrame>(&mut daemon).await.unwrap(),
            CuaStreamFrame::Closed { .. }
        ));
        assert!(proxy.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn oversized_or_incomplete_input_fails_without_sending_any_message() {
        for bytes in [
            vec![b'x'; MAX_LINE + 1],
            b"{\"id\":1}".to_vec(),
            b"not-json\n".to_vec(),
        ] {
            let mut input = BufReader::new(bytes.as_slice());
            assert!(read_line(&mut input).await.is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_stdout_is_bounded_and_closes_transport() {
        let (client, mut daemon) = tokio::io::duplex(1024);
        let (_stdin, input) = tokio::io::duplex(10);
        let (output, _stdout) = tokio::io::duplex(1);
        let proxy = tokio::spawn(forward(client, BufReader::new(input), output));
        write_frame(
            &mut daemon,
            &CuaStreamFrame::Message {
                message: json!({"id":1,"result":"test"}),
            },
        )
        .await
        .unwrap();
        assert!(proxy
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("stdout write timed out"));
        let mut byte = [0];
        assert_eq!(daemon.read(&mut byte).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn attach_checks_compatibility_then_upgrades_the_same_socket() {
        let (mut client, mut daemon) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            assert!(matches!(
                read_frame::<_, Request>(&mut daemon).await.unwrap(),
                Request::List {}
            ));
            write_frame(
                &mut daemon,
                &WireResponse::SessionList {
                    sessions: vec![],
                    compatibility_version: Some(IPC_COMPATIBILITY_VERSION),
                },
            )
            .await
            .unwrap();
            match read_frame::<_, Request>(&mut daemon).await.unwrap() {
                Request::CuaAttach { session } => assert_eq!(session.as_str(), "target-a"),
                _ => panic!("missing attach"),
            }
            write_frame(
                &mut daemon,
                &WireResponse::CuaAttached {
                    session_incarnation: 11,
                    bridge_generation: 12,
                    runtime_generation: 13,
                    attachment_id: 14,
                },
            )
            .await
            .unwrap();
        });
        attach(&mut client, "target-a".parse().unwrap())
            .await
            .unwrap();
        task.await.unwrap();
    }
}
