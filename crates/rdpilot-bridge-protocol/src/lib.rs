//! Transport framing only. Inner MCP values are never translated.
use serde::{Deserialize, Serialize};
use std::io;

pub const CHANNEL_NAME: &str = "RDPILOT_CUA_V1";
pub const PROTOCOL_VERSION: u32 = 1;
pub const BUNDLE_ID: &str = "cua-driver-rs-v0.28.2";
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const QUEUE_DEPTH: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub version: u32,
    pub generation: u64,
    pub control_id: u64,
    pub message: Message,
}

impl Envelope {
    pub fn new(generation: u64, control_id: u64, message: Message) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            generation,
            control_id,
            message,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum Message {
    Hello {
        bundle_id: String,
    },
    Ready {
        bundle_id: String,
    },
    Ping,
    Pong,
    Open {
        attachment_id: u64,
    },
    Opened {
        attachment_id: u64,
        runtime_generation: u64,
    },
    Mcp {
        attachment_id: u64,
        runtime_generation: u64,
        message: serde_json::Value,
    },
    Close {
        attachment_id: u64,
        reason: String,
    },
    Closed {
        attachment_id: u64,
        reason: String,
    },
    FileTransfer(FileTransferRequest),
    FileTransferResult(FileTransferResponse),
    Error {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum FileTransferOp {
    Upload,
    Download,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileTransferRequest {
    pub op: FileTransferOp,
    pub remote_path: String,
    pub share_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileTransferData {
    pub bytes_transferred: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileTransferResponse {
    pub success: bool,
    pub data: Option<FileTransferData>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

pub fn encode(envelope: &Envelope) -> io::Result<Vec<u8>> {
    let payload = serde_json::to_vec(envelope)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(invalid("outer frame exceeds 16 MiB"));
    }
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

fn invalid(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

/// Incremental bounded decoder; accepts arbitrary fragment/coalescing boundaries.
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    expected: Option<usize>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, mut bytes: &[u8]) -> io::Result<Vec<Envelope>> {
        let mut frames = Vec::new();
        while !bytes.is_empty() {
            let target = self.expected.unwrap_or(4);
            let take = (target - self.buffer.len()).min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() != target {
                continue;
            }
            if self.expected.is_none() {
                let len = u32::from_le_bytes(self.buffer[..4].try_into().unwrap()) as usize;
                self.buffer.clear();
                if len == 0 || len > MAX_FRAME_BYTES {
                    return Err(invalid("invalid outer frame length"));
                }
                self.expected = Some(len);
            } else {
                let envelope: Envelope = serde_json::from_slice(&self.buffer)?;
                self.buffer.clear();
                self.expected = None;
                if envelope.version != PROTOCOL_VERSION {
                    return Err(invalid("unsupported bridge protocol"));
                }
                frames.push(envelope);
            }
        }
        Ok(frames)
    }
    pub fn finish(&self) -> io::Result<()> {
        if self.buffer.is_empty() && self.expected.is_none() {
            Ok(())
        } else {
            Err(invalid("truncated outer frame"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_and_coalesced_large_frames() {
        let e = Envelope::new(
            9,
            3,
            Message::Mcp {
                attachment_id: 1,
                runtime_generation: 2,
                message: serde_json::json!({"id":null,"result":"a".repeat(80_000)}),
            },
        );
        let mut bytes = encode(&e).unwrap();
        bytes.extend(encode(&e).unwrap());
        let mut decoder = Decoder::new();
        let mut output = Vec::new();
        for chunk in bytes.chunks(997) {
            output.extend(decoder.push(chunk).unwrap());
        }
        assert_eq!(output, vec![e.clone(), e]);
        decoder.finish().unwrap();
    }
    #[test]
    fn invalid_lengths_and_truncation() {
        for n in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
            assert!(Decoder::new().push(&n.to_le_bytes()).is_err());
        }
        let mut decoder = Decoder::new();
        decoder.push(&[1]).unwrap();
        assert!(decoder.finish().is_err());
        let huge = Envelope::new(
            1,
            0,
            Message::Error {
                reason: "a".repeat(MAX_FRAME_BYTES),
            },
        );
        assert!(encode(&huge).is_err());
    }
}
