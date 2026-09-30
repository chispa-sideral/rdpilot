//! Transport framing only. Inner MCP values are never translated.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;

pub const CHANNEL_NAME: &str = "RDPILOT_CUA_V1";
/// Bumped whenever daemon and bridge stop understanding each other.
pub const PROTOCOL_VERSION: u32 = 2;
/// Name of the bridge executable in a bundle directory and in the guest.
pub const BRIDGE_EXE_NAME: &str = "rdpilot-bridge.exe";
/// Name of the bundle manifest in a bundle directory and in the guest.
pub const MANIFEST_NAME: &str = "manifest.json";
/// Name of the Cua driver executable inside the Cua archive.
pub const CUA_DRIVER_EXE_NAME: &str = "cua-driver.exe";
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
        /// Crate version of the answering bridge (diagnostic only; the
        /// envelope version decides compatibility).
        bridge_version: String,
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

/// What the daemon serves to the guest next to the bridge and the Cua
/// archive. The daemon writes it after it has verified both files; the
/// bridge checks every file against it before it runs anything.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    /// Identity of this bridge and Cua pair; also the guest install directory
    /// name.
    pub bundle_id: String,
    /// Cua driver version, for example `0.1.2` or `0.1.3-nightly.1`.
    pub cua_version: String,
    /// File name of the Cua archive in the bundle directory.
    pub archive_name: String,
    /// Lower-case hex SHA-256 of the Cua archive.
    pub archive_sha256: String,
    /// Lower-case hex SHA-256 of `rdpilot-bridge.exe`.
    pub bridge_sha256: String,
    /// Lower-case hex SHA-256 of every archive entry, by entry name.
    pub files: BTreeMap<String, String>,
}

impl BundleManifest {
    /// Check the shape of every name and hash before anything is trusted.
    pub fn validate(&self) -> Result<(), String> {
        check_bundle_id(&self.bundle_id)?;
        check_archive_entry_name(&self.archive_name)?;
        for hash in [&self.archive_sha256, &self.bridge_sha256] {
            check_sha256(hash)?;
        }
        if !self.files.contains_key(CUA_DRIVER_EXE_NAME) {
            return Err(format!("the Cua archive has no {CUA_DRIVER_EXE_NAME}"));
        }
        for (name, hash) in &self.files {
            check_archive_entry_name(name)?;
            check_sha256(hash)?;
            if name.eq_ignore_ascii_case(BRIDGE_EXE_NAME)
                || name.eq_ignore_ascii_case(MANIFEST_NAME)
            {
                return Err(format!("the Cua archive must not contain {name}"));
            }
        }
        Ok(())
    }
}

/// A bundle id is one path component: 1-128 characters from
/// `A-Z a-z 0-9 . _ -`, not starting with `.`.
pub fn check_bundle_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(format!("invalid bundle id {id:?}"))
    }
}

/// Archive entries must be flat file names: no directory part, drive,
/// stream, parent reference or control character, and no trailing dot or
/// space (which Windows strips).
pub fn check_archive_entry_name(name: &str) -> Result<(), String> {
    let bad = name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.starts_with(' ')
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        });
    if bad {
        Err(format!("unsupported archive entry name {name:?}"))
    } else {
        Ok(())
    }
}

fn check_sha256(hash: &str) -> Result<(), String> {
    if hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(format!("invalid SHA-256 {hash:?}"))
    }
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
                    return Err(invalid(&format!(
                        "peer speaks bridge protocol {}, this side needs {PROTOCOL_VERSION}; \
                         use an rdpilot-bridge.exe built for this rdpilot version",
                        envelope.version
                    )));
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
    fn manifest() -> BundleManifest {
        let h = "a".repeat(64);
        BundleManifest {
            bundle_id: "cua-driver-rs-v9.9.9-0123456789abcdef".into(),
            cua_version: "9.9.9".into(),
            archive_name: "cua.zip".into(),
            archive_sha256: h.clone(),
            bridge_sha256: h.clone(),
            files: [(CUA_DRIVER_EXE_NAME.to_owned(), h)].into_iter().collect(),
        }
    }
    #[test]
    fn manifest_validation() {
        assert!(manifest().validate().is_ok());
        let mut m = manifest();
        m.files.clear();
        assert!(m.validate().is_err(), "cua-driver.exe is required");
        let mut m = manifest();
        m.bundle_id = "../x".into();
        assert!(m.validate().is_err());
        let mut m = manifest();
        m.bridge_sha256 = "A".repeat(64);
        assert!(m.validate().is_err(), "hashes are lower-case hex");
        let mut m = manifest();
        m.files.insert(BRIDGE_EXE_NAME.into(), "b".repeat(64));
        assert!(m.validate().is_err());
    }
    #[test]
    fn archive_entry_names_are_flat() {
        for ok in ["cua-driver.exe", "cua_driver_sdk.dll", "a..b.txt"] {
            assert!(check_archive_entry_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", ".", "..", "../x", "dir/x", "dir\\x", "C:x", "x:stream", "x.", "x ", "a\nb",
        ] {
            assert!(check_archive_entry_name(bad).is_err(), "{bad:?}");
        }
    }
    #[test]
    fn other_protocol_versions_are_rejected_with_both_versions() {
        let mut e = Envelope::new(1, 0, Message::Ping);
        e.version = PROTOCOL_VERSION + 1;
        let err = Decoder::new().push(&encode(&e).unwrap()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&(PROTOCOL_VERSION + 1).to_string()), "{text}");
        assert!(text.contains(&PROTOCOL_VERSION.to_string()), "{text}");
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
