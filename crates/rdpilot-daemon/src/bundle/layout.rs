//! Cua archive layout check: a flat archive that contains `cua-driver.exe`.
//! The per-entry hashes go into the bundle manifest so the bridge can verify
//! what it extracts.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use super::cache::sha256_reader;
use rdpilot_bridge_protocol::{check_archive_entry_name, CUA_DRIVER_EXE_NAME};

/// Most entries accepted.
const MAX_ENTRIES: usize = 4096;
/// Largest total uncompressed size accepted.
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Entry name -> SHA-256 of the uncompressed entry, or why the layout is
/// not supported.
pub(crate) fn inspect(archive: &Path) -> Result<BTreeMap<String, String>, String> {
    let file = File::open(archive).map_err(|e| format!("cannot open archive: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("not a zip archive: {e}"))?;
    if zip.len() > MAX_ENTRIES {
        return Err(format!("more than {MAX_ENTRIES} entries"));
    }
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("unreadable entry: {e}"))?;
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| "an entry name is not UTF-8".to_owned())?
            .to_owned();
        if entry.is_dir() || entry.is_symlink() {
            return Err(format!("entry {name:?} is not a plain file"));
        }
        check_archive_entry_name(&name)?;
        let budget = MAX_TOTAL_BYTES - total;
        let (digest, copied) = sha256_reader((&mut entry).take(budget + 1))
            .map_err(|e| format!("cannot read entry {name:?}: {e}"))?;
        if copied > budget {
            return Err("archive expands beyond the size limit".to_owned());
        }
        total += copied;
        let lower = name.to_ascii_lowercase();
        if files
            .keys()
            .any(|k: &String| k.to_ascii_lowercase() == lower)
        {
            return Err(format!("entry {name:?} appears more than once"));
        }
        files.insert(name, digest);
    }
    if !files.contains_key(CUA_DRIVER_EXE_NAME) {
        return Err(format!("no {CUA_DRIVER_EXE_NAME} at the archive root"));
    }
    Ok(files)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::bundle::http::hex;
    use sha2::{Digest, Sha256};
    use std::io::{self, Write as _};

    /// A zip with the given (name, bytes) entries.
    pub(crate) fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in entries {
                if name.ends_with('/') {
                    writer.add_directory(*name, options).unwrap();
                } else {
                    writer.start_file(*name, options).unwrap();
                    writer.write_all(bytes).unwrap();
                }
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    fn inspect_bytes(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.zip");
        std::fs::write(&path, bytes).unwrap();
        inspect(&path)
    }

    #[test]
    fn flat_archive_with_the_driver_is_accepted() {
        let files =
            inspect_bytes(&make_zip(&[("cua-driver.exe", b"MZ"), ("x.dll", b"y")])).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files["x.dll"], hex(&Sha256::digest(b"y")));
    }

    #[test]
    fn unsupported_layouts_are_rejected() {
        for entries in [
            vec![("../x", &b"a"[..]), ("cua-driver.exe", b"MZ")],
            vec![("dir/", &b""[..]), ("cua-driver.exe", b"MZ")],
            vec![("dir/cua-driver.exe", &b"MZ"[..])],
            vec![("other.exe", &b"MZ"[..])],
            vec![("cua-driver.exe", &b"MZ"[..]), ("CUA-DRIVER.EXE", b"MZ")],
        ] {
            assert!(inspect_bytes(&make_zip(&entries)).is_err(), "{entries:?}");
        }
        assert!(inspect_bytes(b"not a zip").is_err());
    }
}
