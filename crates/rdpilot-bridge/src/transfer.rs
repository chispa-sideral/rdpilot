//! RDPDR transfer boundary: both names are relative to fixed process-owned roots.
use rdpilot_bridge_protocol::{
    FileTransferData, FileTransferOp, FileTransferRequest, FileTransferResponse,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "path escapes transfer root or uses a Windows special name",
    )
}

/// Reject Windows aliases/ADS on every platform so portable tests protect Windows.
pub fn resolve(root: &Path, candidate: &str) -> io::Result<PathBuf> {
    let candidate = candidate.replace('\\', "/");
    if candidate.is_empty() || candidate.starts_with('/') {
        return Err(invalid());
    }
    let mut path = root.to_path_buf();
    for part in candidate.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || part.chars().any(|c| c < ' ' || ":*?\"<>|".contains(c))
        {
            return Err(invalid());
        }
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid());
        }
        path.push(part);
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() {
                return Err(invalid());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                // Includes junctions and other reparse points, not only symbolic links.
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(path)
}

pub fn execute(
    root: &Path,
    share: &Path,
    request: &FileTransferRequest,
    cancelled: &AtomicBool,
) -> FileTransferResponse {
    let result = (|| {
        fs::create_dir_all(root)?;
        let remote = resolve(root, &request.remote_path)?;
        let shared = resolve(share, &request.share_name)?;
        let (source, destination) = match request.op {
            FileTransferOp::Upload => (shared, remote),
            FileTransferOp::Download => (remote, shared),
        };
        copy(&source, &destination, cancelled)
    })();
    match result {
        Ok(data) => FileTransferResponse {
            success: true,
            data: Some(data),
            error: None,
            error_kind: None,
        },
        Err(error) => FileTransferResponse {
            success: false,
            data: None,
            error_kind: Some(
                if error.kind() == io::ErrorKind::PermissionDenied {
                    "path_traversal"
                } else {
                    "io"
                }
                .into(),
            ),
            error: Some(error.to_string()),
        },
    }
}

fn copy(source: &Path, destination: &Path, cancelled: &AtomicBool) -> io::Result<FileTransferData> {
    let mut input = File::open(source)?;
    // CREATE_NEW on the final destination makes no-clobber atomic, including concurrent transfers.
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = (|| {
        let mut hash = Sha256::new();
        let mut total = 0;
        let mut buffer = [0; 64 * 1024];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "transfer cancelled",
                ));
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
            hash.update(&buffer[..n]);
            total += n as u64;
        }
        output.flush()?;
        Ok(FileTransferData {
            bytes_transferred: total,
            sha256: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(),
        })
    })();
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_path_boundary() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "../x",
            "a/../../x",
            "C:\\x",
            "\\\\server\\x",
            "/x",
            "a:b",
            "NUL",
            "a/COM1.txt",
            "a./x",
            "a /x",
            "",
        ] {
            assert!(resolve(root.path(), name).is_err(), "{name}");
        }
        assert!(resolve(root.path(), "has..dots/file").is_ok());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/tmp", root.path().join("escape")).unwrap();
            assert!(resolve(root.path(), "escape/file").is_err());
        }
    }
    #[test]
    fn hashes_no_clobber_and_cancel_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let bytes = vec![42; 150_000];
        fs::write(share.path().join("in"), &bytes).unwrap();
        let request = FileTransferRequest {
            op: FileTransferOp::Upload,
            remote_path: "out".into(),
            share_name: "in".into(),
        };
        let cancel = AtomicBool::new(false);
        let result = execute(root.path(), share.path(), &request, &cancel);
        assert!(result.success);
        assert_eq!(
            result.data.unwrap().sha256,
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        assert!(!execute(root.path(), share.path(), &request, &cancel).success);
        assert_eq!(fs::read(root.path().join("out")).unwrap(), bytes);
        let download = FileTransferRequest {
            op: FileTransferOp::Download,
            remote_path: "out".into(),
            share_name: "back".into(),
        };
        assert!(execute(root.path(), share.path(), &download, &cancel).success);
        assert_eq!(fs::read(share.path().join("back")).unwrap(), bytes);
        cancel.store(true, Ordering::Relaxed);
        let request = FileTransferRequest {
            remote_path: "cancelled".into(),
            ..request
        };
        assert!(!execute(root.path(), share.path(), &request, &cancel).success);
        assert!(!root.path().join("cancelled").exists());
    }
}
