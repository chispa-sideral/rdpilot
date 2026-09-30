//! The verified download cache.
//!
//! Layout under the cache root:
//!
//! ```text
//! <component>/<version>/<arch>/<file>          verified download
//! <component>/<version>/<arch>/<file>.sha256   its SHA-256
//! bundles/<bundle_id>/                          bridge, Cua archive, manifest
//! resolved.json                                 channel -> version, with time
//! ```
//!
//! Every directory is written as a hidden `.tmp-*` sibling and published
//! with one `rename`, so a reader sees either nothing or a complete entry.
//! When two connects publish the same entry, the second rename fails and
//! the loser verifies and uses the winner's entry. Every file is hashed
//! again before use; a damaged entry is removed and treated as missing.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::http::hex;
use super::Arch;

const RESOLVED_FILE: &str = "resolved.json";
pub(crate) const BUNDLES_DIR: &str = "bundles";
const TMP_PREFIX: &str = ".tmp-";

/// Lower-case hex SHA-256 of a file.
pub(crate) fn sha256_file(path: &Path) -> io::Result<String> {
    Ok(sha256_reader(fs::File::open(path)?)?.0)
}

/// Lower-case hex SHA-256 and length of everything `reader` yields.
pub(crate) fn sha256_reader(mut reader: impl io::Read) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        hasher.update(&buffer[..n]);
        total += n as u64;
    }
    Ok((hex(&hasher.finalize()), total))
}

/// A unique hidden sibling name.
pub(crate) fn tmp_name() -> String {
    let mut random = [0u8; 8];
    let _ = getrandom::fill(&mut random);
    format!("{TMP_PREFIX}{}-{}", std::process::id(), hex(&random))
}

#[derive(Debug, Clone)]
pub(crate) struct Cache {
    root: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Resolved {
    #[serde(default)]
    entries: BTreeMap<String, ResolvedEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolvedEntry {
    value: String,
    at: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Cache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    fn entry_dir(&self, component: &str, version: &str, arch: Arch) -> PathBuf {
        self.root.join(component).join(version).join(arch.as_str())
    }

    /// A verified cached file, or `None`. A damaged entry is removed.
    pub(crate) fn lookup(
        &self,
        component: &str,
        version: &str,
        arch: Arch,
        file: &str,
    ) -> Option<(PathBuf, String)> {
        let dir = self.entry_dir(component, version, arch);
        let path = dir.join(file);
        let expected = fs::read_to_string(dir.join(format!("{file}.sha256"))).ok()?;
        let expected = expected.trim();
        match sha256_file(&path) {
            Ok(actual) if actual == expected => Some((path, actual)),
            Ok(_) | Err(_) if dir.exists() => {
                let _ = fs::remove_dir_all(&dir);
                None
            }
            _ => None,
        }
    }

    /// Move a verified download into the cache and return its final path.
    pub(crate) fn publish(
        &self,
        component: &str,
        version: &str,
        arch: Arch,
        file: &str,
        download: &Path,
        sha256: &str,
    ) -> io::Result<PathBuf> {
        let target = self.entry_dir(component, version, arch);
        let parent = target
            .parent()
            .ok_or_else(|| io::Error::other("cache entry has no parent"))?;
        fs::create_dir_all(parent)?;
        let tmp = parent.join(tmp_name());
        fs::create_dir(&tmp)?;
        let staged = (|| {
            fs::rename(download, tmp.join(file))?;
            fs::write(tmp.join(format!("{file}.sha256")), format!("{sha256}\n"))
        })();
        if let Err(e) = staged {
            let _ = fs::remove_dir_all(&tmp);
            return Err(e);
        }
        if fs::rename(&tmp, &target).is_err() {
            // Another connect published first (or a damaged entry is in the
            // way): keep a valid winner, otherwise replace the damaged entry.
            if let Some((path, _)) = self.lookup(component, version, arch, file) {
                let _ = fs::remove_dir_all(&tmp);
                return Ok(path);
            }
            if let Err(e) = fs::rename(&tmp, &target) {
                let _ = fs::remove_dir_all(&tmp);
                return match self.lookup(component, version, arch, file) {
                    Some((path, _)) => Ok(path),
                    None => Err(e),
                };
            }
        }
        Ok(target.join(file))
    }

    /// Cached versions of `component` for `arch`, unverified.
    pub(crate) fn versions(&self, component: &str, arch: Arch) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root.join(component)) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .filter(|name| self.entry_dir(component, name, arch).is_dir())
            .collect()
    }

    /// A download location inside the cache (same filesystem as entries).
    pub(crate) fn download_path(&self) -> io::Result<PathBuf> {
        let dir = self.root.join(".downloads");
        fs::create_dir_all(&dir)?;
        Ok(dir.join(tmp_name()))
    }

    /// The directory of a bundle.
    pub(crate) fn bundle_dir(&self, bundle_id: &str) -> PathBuf {
        self.root.join(BUNDLES_DIR).join(bundle_id)
    }

    /// A resolved channel value younger than `ttl`.
    pub(crate) fn resolved(&self, key: &str, ttl: Duration) -> Option<String> {
        let entry = self.read_resolved().entries.remove(key)?;
        let age = now_secs().saturating_sub(entry.at);
        (age < ttl.as_secs()).then_some(entry.value)
    }

    /// Remember a resolved channel value now.
    pub(crate) fn remember(&self, key: &str, value: &str) {
        let mut resolved = self.read_resolved();
        resolved.entries.insert(
            key.to_owned(),
            ResolvedEntry {
                value: value.to_owned(),
                at: now_secs(),
            },
        );
        let Ok(json) = serde_json::to_vec_pretty(&resolved) else {
            return;
        };
        if fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let tmp = self.root.join(tmp_name());
        if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, self.root.join(RESOLVED_FILE)).is_ok()
        {
            return;
        }
        let _ = fs::remove_file(&tmp);
    }

    fn read_resolved(&self) -> Resolved {
        fs::read(self.root.join(RESOLVED_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Remove leftovers of interrupted writes. Called when the daemon
    /// starts, before any connect.
    pub(crate) fn sweep(&self) {
        let _ = fs::remove_dir_all(self.root.join(".downloads"));
        let mut dirs = vec![self.root.clone(), self.root.join(BUNDLES_DIR)];
        for component in [super::CUA_COMPONENT, super::BRIDGE_COMPONENT] {
            if let Ok(versions) = fs::read_dir(self.root.join(component)) {
                dirs.extend(versions.filter_map(Result::ok).map(|e| e.path()));
            }
        }
        for dir in dirs {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if entry.file_name().to_string_lossy().starts_with(TMP_PREFIX) {
                    let path = entry.path();
                    let _ = fs::remove_dir_all(&path).or_else(|_| fs::remove_file(&path));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(cache: &Cache, bytes: &[u8]) -> (PathBuf, String) {
        let path = cache.download_path().unwrap();
        fs::write(&path, bytes).unwrap();
        let sha = sha256_file(&path).unwrap();
        (path, sha)
    }

    #[test]
    fn publish_lookup_and_damage() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        let (path, sha) = download(&cache, b"one");
        let published = cache
            .publish("c", "1.0.0", Arch::X86_64, "f.bin", &path, &sha)
            .unwrap();
        assert_eq!(fs::read(&published).unwrap(), b"one");
        assert_eq!(
            cache.lookup("c", "1.0.0", Arch::X86_64, "f.bin"),
            Some((published.clone(), sha))
        );
        assert_eq!(cache.versions("c", Arch::X86_64), vec!["1.0.0".to_owned()]);
        fs::write(&published, b"tampered").unwrap();
        assert_eq!(cache.lookup("c", "1.0.0", Arch::X86_64, "f.bin"), None);
        assert!(
            cache.versions("c", Arch::X86_64).is_empty(),
            "entry removed"
        );
    }

    #[test]
    fn second_publisher_uses_the_winner() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        let (a, sha) = download(&cache, b"same");
        let (b, _) = download(&cache, b"same");
        let first = cache
            .publish("c", "1.0.0", Arch::X86_64, "f", &a, &sha)
            .unwrap();
        let second = cache
            .publish("c", "1.0.0", Arch::X86_64, "f", &b, &sha)
            .unwrap();
        assert_eq!(first, second);
        cache.sweep();
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("c").join("1.0.0"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("x86_64")]);
    }

    #[test]
    fn resolved_values_expire() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        assert_eq!(cache.resolved("k", Duration::from_secs(60)), None);
        cache.remember("k", "v1");
        assert_eq!(
            cache.resolved("k", Duration::from_secs(60)).as_deref(),
            Some("v1")
        );
        assert_eq!(cache.resolved("k", Duration::ZERO), None);
    }

    #[test]
    fn sweep_removes_interrupted_writes() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        let stale = dir.path().join(BUNDLES_DIR).join(tmp_name());
        fs::create_dir_all(&stale).unwrap();
        let (download, _) = download(&cache, b"x");
        cache.sweep();
        assert!(!stale.exists());
        assert!(!download.exists());
    }
}
