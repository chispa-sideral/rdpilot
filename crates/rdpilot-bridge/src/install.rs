//! Guest setup and cleanup. Everything this module writes is under one
//! per-user directory (`%LOCALAPPDATA%\rdpilot`); it never touches the
//! registry, services, scheduled tasks, PATH or the firewall.
//!
//! `install` copies the bundle the daemon serves (`manifest.json`,
//! `rdpilot-bridge.exe`, the Cua archive), checks every file against the
//! manifest, extracts the archive into a staging directory, verifies each
//! extracted file, and publishes `<base>\<bundle_id>` with one rename. An
//! existing verified directory is reused; a concurrent installer either
//! wins the rename or uses the winner's directory.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rdpilot_bridge_protocol::{
    check_archive_entry_name, BundleManifest, BRIDGE_EXE_NAME, MANIFEST_NAME,
};
use sha2::{Digest, Sha256};

/// The share the daemon serves the bundle on.
pub const DEFAULT_SOURCE: &str = r"\\tsclient\RDPILOT\bundle";
/// Name of the file-transfer staging directory under `%TEMP%`.
pub const TRANSFER_ROOT_NAME: &str = "rdpilot-transfer-root";

fn error(reason: impl Into<String>) -> io::Error {
    io::Error::other(reason.into())
}

/// `%LOCALAPPDATA%\rdpilot`.
pub fn default_base() -> io::Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("rdpilot"))
        .ok_or_else(|| error("LOCALAPPDATA is not set"))
}

/// Lower-case hex SHA-256 of everything `reader` yields.
fn sha256(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    sha256(fs::File::open(path)?)
}

fn check_hash(path: &Path, expected: &str) -> io::Result<()> {
    if sha256_file(path)? == expected {
        Ok(())
    } else {
        Err(error(format!("integrity failure: {}", path.display())))
    }
}

fn read_manifest(path: &Path) -> io::Result<BundleManifest> {
    let manifest: BundleManifest = serde_json::from_slice(&fs::read(path)?)?;
    manifest.validate().map_err(error)?;
    Ok(manifest)
}

/// True when `dir` is a complete installation of `manifest`.
fn installed(dir: &Path, manifest: &BundleManifest) -> bool {
    let complete = || -> io::Result<bool> {
        if read_manifest(&dir.join(MANIFEST_NAME))? != *manifest {
            return Ok(false);
        }
        check_hash(&dir.join(BRIDGE_EXE_NAME), &manifest.bridge_sha256)?;
        for (name, hash) in &manifest.files {
            check_hash(&dir.join(name), hash)?;
        }
        Ok(true)
    };
    complete().unwrap_or(false)
}

/// Extract every archive entry into `stage`. Each entry must be a flat file
/// listed in the manifest with a matching hash; every listed file must be
/// present.
fn extract(archive: &Path, stage: &Path, files: &BTreeMap<String, String>) -> io::Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)
        .map_err(|e| error(format!("invalid Cua archive: {e}")))?;
    let mut seen = 0usize;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| error(format!("invalid Cua archive entry: {e}")))?;
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| error("archive entry name is not UTF-8"))?
            .to_owned();
        check_archive_entry_name(&name).map_err(error)?;
        let expected = files
            .get(&name)
            .ok_or_else(|| error(format!("archive entry {name} is not in the manifest")))?;
        if entry.is_dir() || entry.is_symlink() {
            return Err(error(format!("archive entry {name} is not a plain file")));
        }
        let path = stage.join(&name);
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        io::copy(&mut entry, &mut out)?;
        out.flush()?;
        drop(out);
        check_hash(&path, expected)?;
        seen += 1;
    }
    if seen != files.len() {
        return Err(error("the Cua archive lacks files listed in the manifest"));
    }
    Ok(())
}

/// Check that `image` (the running launcher, a local copy of the served
/// bridge) is the bridge that the manifest in `source` names.
pub fn verify_image(source: &Path, image: &Path) -> io::Result<()> {
    let manifest = read_manifest(&source.join(MANIFEST_NAME))?;
    check_hash(image, &manifest.bridge_sha256).map_err(|e| {
        error(format!(
            "{} is not the rdpilot-bridge of bundle {}: {e}",
            image.display(),
            manifest.bundle_id
        ))
    })
}

/// Install the bundle in `source` under `base` and return the installed
/// directory. `tag` makes the staging directory name unique per caller.
pub fn install(source: &Path, base: &Path, tag: &str) -> io::Result<PathBuf> {
    let manifest = read_manifest(&source.join(MANIFEST_NAME))?;
    fs::create_dir_all(base)?;
    let dest = base.join(&manifest.bundle_id);
    if installed(&dest, &manifest) {
        return Ok(dest);
    }
    let stage = base.join(format!(".staging-{tag}-{}", std::process::id()));
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir(&stage)?;
    let staged = (|| -> io::Result<()> {
        let archive = stage.join(&manifest.archive_name);
        fs::copy(source.join(&manifest.archive_name), &archive)?;
        check_hash(&archive, &manifest.archive_sha256)?;
        let bridge = stage.join(BRIDGE_EXE_NAME);
        fs::copy(source.join(BRIDGE_EXE_NAME), &bridge)?;
        check_hash(&bridge, &manifest.bridge_sha256)?;
        extract(&archive, &stage, &manifest.files)?;
        // The archive is not needed once its files are verified.
        if !manifest.files.contains_key(&manifest.archive_name) {
            fs::remove_file(&archive)?;
        }
        fs::write(
            stage.join(MANIFEST_NAME),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(())
    })();
    if let Err(e) = staged {
        let _ = fs::remove_dir_all(&stage);
        return Err(e);
    }
    if fs::rename(&stage, &dest).is_ok() {
        return Ok(dest);
    }
    // A concurrent installer won, or an incomplete directory is in the way.
    if installed(&dest, &manifest) {
        let _ = fs::remove_dir_all(&stage);
        return Ok(dest);
    }
    let replaced = fs::remove_dir_all(&dest).and_then(|()| fs::rename(&stage, &dest));
    if let Err(e) = replaced {
        let _ = fs::remove_dir_all(&stage);
        return if installed(&dest, &manifest) {
            Ok(dest)
        } else {
            Err(error(format!(
                "cannot replace incomplete installation {}: {e}",
                dest.display()
            )))
        };
    }
    Ok(dest)
}

/// What cleanup could not remove right away.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Leftover {
    /// The running executable, when it lives inside `base`; the caller
    /// removes it (and `base`) after this process exits.
    pub own_image: Option<PathBuf>,
}

/// Remove `base` and the file-transfer root. When `own_exe` is inside
/// `base`, everything except that file is removed and it is reported so the
/// caller can delete it after exit.
pub fn remove_footprint(base: &Path, transfer_root: &Path, own_exe: &Path) -> io::Result<Leftover> {
    if transfer_root.exists() {
        fs::remove_dir_all(transfer_root)?;
    }
    if !base.exists() {
        return Ok(Leftover::default());
    }
    let own = fs::canonicalize(own_exe).unwrap_or_else(|_| own_exe.to_path_buf());
    let canonical_base = fs::canonicalize(base)?;
    if !own.starts_with(&canonical_base) {
        fs::remove_dir_all(base)?;
        return Ok(Leftover::default());
    }
    remove_except(&canonical_base, &own)?;
    Ok(Leftover {
        own_image: Some(own),
    })
}

/// Remove everything under `dir` except `keep` and the directories that
/// lead to it.
fn remove_except(dir: &Path, keep: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path == keep {
            continue;
        }
        if keep.starts_with(&path) {
            remove_except(&path, keep)?;
        } else if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bundle {
        source: tempfile::TempDir,
        manifest: BundleManifest,
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    fn hash(bytes: &[u8]) -> String {
        sha256(bytes).unwrap()
    }

    /// A served bundle: manifest, bridge and a Cua archive.
    fn bundle(entries: &[(&str, &[u8])]) -> Bundle {
        let source = tempfile::tempdir().unwrap();
        let archive = zip_bytes(entries);
        let bridge = b"MZ bridge".to_vec();
        let files = entries
            .iter()
            .map(|(n, b)| ((*n).to_owned(), hash(b)))
            .collect();
        let manifest = BundleManifest {
            bundle_id: format!("cua-driver-rs-v9.0.0-{}", &hash(&archive)[..16]),
            cua_version: "9.0.0".into(),
            archive_name: "cua-driver-rs-9.0.0-windows-x86_64-binary.zip".into(),
            archive_sha256: hash(&archive),
            bridge_sha256: hash(&bridge),
            files,
        };
        fs::write(source.path().join(&manifest.archive_name), &archive).unwrap();
        fs::write(source.path().join(BRIDGE_EXE_NAME), &bridge).unwrap();
        fs::write(
            source.path().join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        Bundle { source, manifest }
    }

    fn standard() -> Bundle {
        bundle(&[("cua-driver.exe", b"MZ driver"), ("sdk.dll", b"sdk")])
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn install_verifies_publishes_and_reuses() {
        let b = standard();
        let base = tempfile::tempdir().unwrap();
        let dir = install(b.source.path(), base.path(), "7").unwrap();
        assert_eq!(dir, base.path().join(&b.manifest.bundle_id));
        assert_eq!(
            listing(&dir),
            [
                "cua-driver.exe",
                "manifest.json",
                "rdpilot-bridge.exe",
                "sdk.dll"
            ]
        );
        assert_eq!(listing(base.path()), vec![b.manifest.bundle_id.clone()]);
        let again = install(b.source.path(), base.path(), "8").unwrap();
        assert_eq!(again, dir);
        assert_eq!(listing(base.path()), vec![b.manifest.bundle_id.clone()]);
    }

    #[test]
    fn the_launcher_must_be_the_bridge_the_manifest_names() {
        let b = standard();
        let local = tempfile::tempdir().unwrap();
        let launcher = local.path().join("launch-7.exe");
        fs::copy(b.source.path().join(BRIDGE_EXE_NAME), &launcher).unwrap();
        verify_image(b.source.path(), &launcher).unwrap();
        fs::write(&launcher, b"MZ other").unwrap();
        let error = verify_image(b.source.path(), &launcher).unwrap_err();
        assert!(error.to_string().contains(&b.manifest.bundle_id), "{error}");
        assert!(verify_image(b.source.path(), &local.path().join("missing.exe")).is_err());
    }

    #[test]
    fn tampered_files_are_rejected_and_nothing_is_published() {
        let base = tempfile::tempdir().unwrap();
        let b = standard();
        fs::write(b.source.path().join(BRIDGE_EXE_NAME), b"MZ evil").unwrap();
        assert!(install(b.source.path(), base.path(), "1").is_err());
        assert!(listing(base.path()).is_empty());

        let b = standard();
        fs::write(b.source.path().join(&b.manifest.archive_name), b"zip?").unwrap();
        assert!(install(b.source.path(), base.path(), "2").is_err());
        assert!(listing(base.path()).is_empty());
    }

    #[test]
    fn archives_that_disagree_with_the_manifest_are_rejected() {
        let base = tempfile::tempdir().unwrap();
        // An extra entry the manifest does not list.
        let b = standard();
        let archive = zip_bytes(&[
            ("cua-driver.exe", b"MZ driver"),
            ("sdk.dll", b"sdk"),
            ("extra.dll", b"x"),
        ]);
        let mut manifest = b.manifest.clone();
        manifest.archive_sha256 = hash(&archive);
        fs::write(b.source.path().join(&manifest.archive_name), &archive).unwrap();
        fs::write(
            b.source.path().join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(install(b.source.path(), base.path(), "1").is_err());
        // An entry whose content differs from its manifest hash.
        let b = standard();
        let archive = zip_bytes(&[("cua-driver.exe", b"MZ other"), ("sdk.dll", b"sdk")]);
        let mut manifest = b.manifest.clone();
        manifest.archive_sha256 = hash(&archive);
        fs::write(b.source.path().join(&manifest.archive_name), &archive).unwrap();
        fs::write(
            b.source.path().join(MANIFEST_NAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(install(b.source.path(), base.path(), "2").is_err());
        assert!(listing(base.path()).is_empty());
    }

    #[test]
    fn an_incomplete_installation_is_replaced() {
        let b = standard();
        let base = tempfile::tempdir().unwrap();
        let dir = install(b.source.path(), base.path(), "1").unwrap();
        fs::remove_file(dir.join("sdk.dll")).unwrap();
        let again = install(b.source.path(), base.path(), "2").unwrap();
        assert!(again.join("sdk.dll").is_file());
    }

    #[test]
    fn cleanup_removes_only_the_rdpilot_footprint() {
        let b = standard();
        let profile = tempfile::tempdir().unwrap();
        let local = profile.path().join("Local");
        let temp = profile.path().join("Temp");
        fs::create_dir_all(local.join("Other")).unwrap();
        fs::create_dir_all(temp.join("keep")).unwrap();
        fs::write(profile.path().join("file.txt"), b"x").unwrap();
        let base = local.join("rdpilot");
        let transfer = temp.join(TRANSFER_ROOT_NAME);
        fs::create_dir_all(transfer.join("sub")).unwrap();
        let dir = install(b.source.path(), &base, "1").unwrap();
        let before_other = (listing(&local), listing(&temp));

        // Run from inside the installation: everything but the running image
        // goes now; the image is reported for removal after exit.
        let own = dir.join(BRIDGE_EXE_NAME);
        let leftover = remove_footprint(&base, &transfer, &own).unwrap();
        let own = fs::canonicalize(&own).unwrap();
        assert_eq!(leftover.own_image.as_deref(), Some(own.as_path()));
        assert_eq!(listing(&dir), [BRIDGE_EXE_NAME]);
        assert!(!transfer.exists());

        // Run from elsewhere (for example the served share): all gone.
        let outside = b.source.path().join(BRIDGE_EXE_NAME);
        assert_eq!(
            remove_footprint(&base, &transfer, &outside).unwrap(),
            Leftover::default()
        );
        assert!(!base.exists());
        assert_eq!(listing(&local), ["Other"]);
        assert_eq!(listing(&temp), ["keep"]);
        assert_eq!(before_other.0, ["Other", "rdpilot"]);
        assert_eq!(before_other.1, ["keep", TRANSFER_ROOT_NAME]);
        assert_eq!(listing(profile.path()), ["Local", "Temp", "file.txt"]);
    }
}
