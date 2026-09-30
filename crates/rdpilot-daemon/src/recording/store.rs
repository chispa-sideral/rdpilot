//! Owner-only storage of recordings and their retention.
//!
//! Layout under the root (format 1):
//!
//! ```text
//! <root>/<id>/manifest.json      atomic (temporary file + rename)
//! <root>/<id>/events.jsonl       append-only, one event per line
//! <root>/<id>/keep               empty marker: never pruned
//! <root>/<id>/segments/NNNNNN.webm
//! ```
//!
//! `<id>` is `YYYYMMDDTHHMMSSZ-` plus 8 lowercase hex digits. Every path is
//! joined from a validated id and a validated segment number only, so no
//! request can reach outside the root.
//!
//! Directories are created owner-only (0700 on Unix; a protected DACL for
//! the current user on Windows) and files 0600 on Unix (inheriting the
//! directory's DACL on Windows). Modes are set at creation, never changed
//! afterwards. Everything here does blocking file I/O: callers run it on a
//! blocking thread or the recorder thread.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::manifest::Manifest;

/// Unkept finished recordings older than this are deleted.
pub(crate) const MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Length of a recording id.
const ID_LEN: usize = 25;

/// Highest segment number.
pub(crate) const MAX_SEGMENT: u32 = 999_999;

pub(crate) const MANIFEST: &str = "manifest.json";
pub(crate) const EVENTS: &str = "events.jsonl";
pub(crate) const KEEP: &str = "keep";
pub(crate) const SEGMENTS: &str = "segments";

/// Mint a recording id for `now`.
pub(crate) fn mint_id(now: SystemTime) -> String {
    let stamp = crate::registry::iso8601_from_system_time(now).replace(['-', ':'], "");
    let mut bytes = [0_u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = now
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        bytes = (nanos ^ std::process::id()).to_be_bytes();
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{stamp}-{hex}")
}

/// Whether `id` has the exact id shape.
pub(crate) fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == ID_LEN
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z'
        && b[16] == b'-'
        && b[17..]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
}

/// The file name of segment `n` (1-based).
pub(crate) fn segment_file(n: u32) -> String {
    format!("{n:06}.webm")
}

/// Create one owner-only directory (not its parents). Fails if it exists.
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(windows)]
    {
        crate::win_acl::create_private_dir(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::create_dir(path)
    }
}

/// Open an owner-only file for writing: `create_new` fails if it exists,
/// otherwise it is created or truncated. With `append`, writes go to the end.
pub(crate) fn open_private_file(path: &Path, create_new: bool, append: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true);
    if append {
        options.append(true);
    }
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
        if !append {
            options.truncate(true);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Write `bytes` to `path` atomically: an owner-only temporary file next to
/// it, flushed to disk, then renamed over `path`.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut file = open_private_file(&tmp, false, false)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// One recording found on disk.
#[derive(Debug, Clone)]
pub(crate) struct Scanned {
    pub(crate) manifest: Manifest,
    /// Bytes of every file in the recording directory.
    pub(crate) bytes: u64,
    pub(crate) kept: bool,
}

/// What a pruning pass found and did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PruneReport {
    pub(crate) deleted: Vec<String>,
    pub(crate) kept_bytes: u64,
    pub(crate) unkept_bytes: u64,
    pub(crate) kept_over_budget: bool,
}

/// The recordings root.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    root: PathBuf,
}

impl Store {
    pub(crate) fn new(root: PathBuf) -> Self {
        Store { root }
    }

    /// Create the root (owner-only) and its parents when missing. An
    /// existing root is used as it is.
    pub(crate) fn ensure_root(&self) -> io::Result<()> {
        if self.root.is_dir() {
            return Ok(());
        }
        if let Some(parent) = self.root.parent() {
            fs::create_dir_all(parent)?;
        }
        match create_private_dir(&self.root) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            other => other,
        }
    }

    /// The directory of recording `id`, or `None` for a malformed id.
    pub(crate) fn dir(&self, id: &str) -> Option<PathBuf> {
        valid_id(id).then(|| self.root.join(id))
    }

    /// The path of closed segment `n` of recording `id`, when it exists.
    pub(crate) fn segment_path(&self, id: &str, n: u32) -> Option<PathBuf> {
        if !(1..=MAX_SEGMENT).contains(&n) {
            return None;
        }
        let path = self.dir(id)?.join(SEGMENTS).join(segment_file(n));
        path.is_file().then_some(path)
    }

    /// Create the directory of a new recording with its `segments/` folder.
    pub(crate) fn create_recording(&self, id: &str) -> io::Result<PathBuf> {
        let dir = self
            .dir(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad recording id"))?;
        self.ensure_root()?;
        create_private_dir(&dir)?;
        create_private_dir(&dir.join(SEGMENTS))?;
        Ok(dir)
    }

    pub(crate) fn write_manifest(dir: &Path, manifest: &Manifest) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
        write_atomic(&dir.join(MANIFEST), &bytes)
    }

    pub(crate) fn read_manifest(dir: &Path) -> io::Result<Manifest> {
        let bytes = fs::read(dir.join(MANIFEST))?;
        serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Mark or unmark `id` as kept. Returns whether the mark changed.
    ///
    /// # Errors
    ///
    /// `NotFound` for an unknown or malformed id.
    pub(crate) fn set_keep(&self, id: &str, keep: bool) -> io::Result<bool> {
        let dir = self
            .dir(id)
            .filter(|d| d.join(MANIFEST).is_file())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown recording id"))?;
        let mark = dir.join(KEEP);
        let present = mark.is_file();
        if keep == present {
            return Ok(false);
        }
        if keep {
            write_atomic(&mark, b"")?;
        } else {
            fs::remove_file(&mark)?;
        }
        Ok(true)
    }

    /// Every readable recording, oldest first.
    pub(crate) fn scan(&self) -> Vec<Scanned> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut found: Vec<Scanned> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_str().is_some_and(valid_id))
            .filter_map(|e| {
                let dir = e.path();
                let manifest = Self::read_manifest(&dir).ok()?;
                Some(Scanned {
                    bytes: dir_bytes(&dir),
                    kept: dir.join(KEEP).is_file(),
                    manifest,
                })
            })
            .collect();
        found.sort_by(|a, b| {
            (a.manifest.started_unix_ms, &a.manifest.id)
                .cmp(&(b.manifest.started_unix_ms, &b.manifest.id))
        });
        found
    }

    /// Finish recordings left open by an earlier daemon (not in `active`):
    /// the end comes from the last complete event, unfinished segment files
    /// are deleted and the end reason is `daemon_lost`. Returns their ids.
    pub(crate) fn finalize_leftovers(&self, active: &HashSet<String>) -> Vec<String> {
        let mut done = Vec::new();
        for scanned in self.scan() {
            let manifest = scanned.manifest;
            if manifest.finished() || active.contains(&manifest.id) {
                continue;
            }
            let Some(dir) = self.dir(&manifest.id) else {
                continue;
            };
            if let Ok(entries) = fs::read_dir(dir.join(SEGMENTS)) {
                for entry in entries.filter_map(Result::ok) {
                    if entry.path().extension().is_some_and(|e| e == "part") {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
            let events = read_events(&dir);
            let last = events.last();
            let mut manifest = manifest;
            manifest.ended_at = Some(
                last.and_then(|e| e.get("at"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&manifest.started_at)
                    .to_owned(),
            );
            manifest.duration_ms = Some(
                last.and_then(|e| e.get("offset_ms"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
            );
            manifest.end_reason = Some("daemon_lost".into());
            if Self::write_manifest(&dir, &manifest).is_ok() {
                done.push(manifest.id);
            }
        }
        done
    }

    /// Delete unkept finished recordings that are not in `active`: first
    /// those older than [`MAX_AGE_MS`], then the oldest while the unkept
    /// total (active recordings included) is over `budget`. Kept recordings
    /// are never deleted and not counted in the unkept total.
    pub(crate) fn prune(
        &self,
        now_unix_ms: u64,
        budget: u64,
        active: &HashSet<String>,
    ) -> PruneReport {
        let mut report = PruneReport::default();
        let mut candidates = Vec::new();
        for scanned in self.scan() {
            let id = scanned.manifest.id.clone();
            let is_active = active.contains(&id) || !scanned.manifest.finished();
            if scanned.kept {
                report.kept_bytes += scanned.bytes;
                continue;
            }
            let age = now_unix_ms.saturating_sub(scanned.manifest.started_unix_ms);
            if !is_active && age > MAX_AGE_MS && self.delete(&id) {
                report.deleted.push(id);
                continue;
            }
            report.unkept_bytes += scanned.bytes;
            if !is_active {
                candidates.push((id, scanned.bytes));
            }
        }
        for (id, bytes) in candidates {
            if report.unkept_bytes <= budget {
                break;
            }
            if self.delete(&id) {
                report.unkept_bytes -= bytes;
                report.deleted.push(id);
            }
        }
        report.kept_over_budget = report.kept_bytes > budget;
        report
    }

    fn delete(&self, id: &str) -> bool {
        self.dir(id)
            .is_some_and(|dir| fs::remove_dir_all(dir).is_ok())
    }

    /// Totals without deleting anything.
    #[cfg(test)]
    pub(crate) fn totals(&self, budget: u64) -> PruneReport {
        let mut report = PruneReport::default();
        for scanned in self.scan() {
            if scanned.kept {
                report.kept_bytes += scanned.bytes;
            } else {
                report.unkept_bytes += scanned.bytes;
            }
        }
        report.kept_over_budget = report.kept_bytes > budget;
        report
    }
}

/// Bytes of every regular file under `dir`.
pub(crate) fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// The complete event lines of a recording, parsed. A partial last line (a
/// write cut short by a crash) and unparsable lines are skipped.
pub(crate) fn read_events(dir: &Path) -> Vec<serde_json::Value> {
    let Ok(file) = File::open(dir.join(EVENTS)) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if line.last() != Some(&b'\n') {
                    break;
                }
                if let Ok(value) = serde_json::from_slice(&line) {
                    events.push(value);
                }
            }
        }
    }
    events
}

/// Unix milliseconds of `t`.
pub(crate) fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::recording::manifest::tests::sample;

    pub(crate) fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rdpilot-rec-{tag}-{}-{}",
            std::process::id(),
            mint_id(SystemTime::now())
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn add(store: &Store, id: &str, started_unix_ms: u64, finished: bool, bytes: usize) {
        let dir = store.create_recording(id).unwrap();
        let mut m = sample();
        m.id = id.into();
        m.started_unix_ms = started_unix_ms;
        if finished {
            m.ended_at = Some("2026-01-01T00:01:00.000Z".into());
        }
        Store::write_manifest(&dir, &m).unwrap();
        let mut f =
            open_private_file(&dir.join(SEGMENTS).join(segment_file(1)), true, false).unwrap();
        f.write_all(&vec![0_u8; bytes]).unwrap();
    }

    fn ids(store: &Store) -> Vec<String> {
        store.scan().into_iter().map(|s| s.manifest.id).collect()
    }

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn ids_have_a_strict_shape() {
        let id = mint_id(UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600));
        assert!(id.starts_with("20260101T000000Z-"), "{id}");
        assert!(valid_id(&id));
        for bad in [
            "",
            "..",
            "20260101T000000Z-0123abc",
            "20260101T000000Z-0123abcde",
            "20260101T000000Z-0123ABCD",
            "20260101T000000Z/0123abcd",
            "20260101T000000Z-0123ab/d",
            "20260101T000000Z-0123ab\\d",
            "20260101T000000Z-0123ab\0d",
            "2026010AT000000Z-0123abcd",
            "../../etc/passwd/aaaaaaaa",
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
        let store = Store::new(PathBuf::from("/nonexistent"));
        assert!(store.dir("..").is_none());
        assert!(store.segment_path(&id, 0).is_none());
        assert!(store.segment_path(&id, MAX_SEGMENT + 1).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn directories_are_0700_and_files_0600() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("modes");
        let store = Store::new(root.clone());
        let id = mint_id(SystemTime::now());
        let dir = store.create_recording(&id).unwrap();
        Store::write_manifest(&dir, &sample()).unwrap();
        store.set_keep(&id, true).unwrap();
        drop(open_private_file(&dir.join(EVENTS), true, true).unwrap());
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(SEGMENTS)), 0o700);
        assert_eq!(mode(&dir.join(MANIFEST)), 0o600);
        assert_eq!(mode(&dir.join(KEEP)), 0o600);
        assert_eq!(mode(&dir.join(EVENTS)), 0o600);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_writes_are_atomic_and_leave_no_temporary_file() {
        let root = temp_root("atomic");
        let store = Store::new(root.clone());
        let id = mint_id(SystemTime::now());
        let dir = store.create_recording(&id).unwrap();
        let mut m = sample();
        Store::write_manifest(&dir, &m).unwrap();
        m.segments.push(crate::recording::manifest::SegmentEntry {
            file: segment_file(1),
            start_offset_ms: 0,
            end_offset_ms: 10,
            frames: 1,
            bytes: 5,
            width: 2,
            height: 2,
        });
        Store::write_manifest(&dir, &m).unwrap();
        assert_eq!(Store::read_manifest(&dir).unwrap(), m);
        assert!(!dir.join("manifest.json.tmp").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn keep_and_unkeep_survive_a_fresh_store() {
        let root = temp_root("keep");
        let id = mint_id(SystemTime::now());
        add(&Store::new(root.clone()), &id, 0, true, 1);
        assert!(Store::new(root.clone()).set_keep(&id, true).unwrap());
        assert!(!Store::new(root.clone()).set_keep(&id, true).unwrap());
        assert!(Store::new(root.clone()).scan()[0].kept);
        assert!(Store::new(root.clone()).set_keep(&id, false).unwrap());
        assert!(!Store::new(root.clone()).scan()[0].kept);
        let unknown = format!("{}-ffffffff", &id[..16]);
        assert_eq!(
            Store::new(root.clone())
                .set_keep(&unknown, true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(Store::new(root.clone()).set_keep("..", true).is_err());
        let _ = fs::remove_dir_all(root);
    }

    fn id_n(n: u32) -> String {
        format!("20260101T000000Z-{n:08x}")
    }

    #[test]
    fn prune_deletes_old_unkept_then_the_oldest_over_budget() {
        let root = temp_root("prune");
        let store = Store::new(root.clone());
        let now = 100 * DAY;
        add(&store, &id_n(1), now - 8 * DAY, true, 100); // too old
        add(&store, &id_n(2), now - 6 * DAY, true, 400);
        add(&store, &id_n(3), now - 5 * DAY, true, 400);
        add(&store, &id_n(4), now - 4 * DAY, true, 400);
        let sizes: Vec<u64> = store.scan().iter().map(|s| s.bytes).collect();
        let budget = sizes[2] + sizes[3] + 10;
        let report = store.prune(now, budget, &HashSet::new());
        assert_eq!(report.deleted, vec![id_n(1), id_n(2)]);
        assert_eq!(ids(&store), vec![id_n(3), id_n(4)]);
        assert!(report.unkept_bytes <= budget);
        assert!(!report.kept_over_budget);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn kept_recordings_are_never_deleted_or_counted() {
        let root = temp_root("kept");
        let store = Store::new(root.clone());
        let now = 100 * DAY;
        add(&store, &id_n(1), now - 30 * DAY, true, 5000);
        add(&store, &id_n(2), now - DAY, true, 10);
        store.set_keep(&id_n(1), true).unwrap();
        let report = store.prune(now, 1000, &HashSet::new());
        assert!(report.deleted.is_empty(), "{report:?}");
        assert!(report.kept_over_budget);
        assert!(report.kept_bytes > 5000);
        assert!(report.unkept_bytes < 1000);
        assert_eq!(store.totals(1000).kept_bytes, report.kept_bytes);

        // Unmarked, it is subject to the next pass.
        store.set_keep(&id_n(1), false).unwrap();
        let report = store.prune(now, 1000, &HashSet::new());
        assert_eq!(report.deleted, vec![id_n(1)]);
        assert!(!report.kept_over_budget);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn active_recordings_are_never_pruned_but_count_toward_the_budget() {
        let root = temp_root("active");
        let store = Store::new(root.clone());
        let now = 100 * DAY;
        add(&store, &id_n(1), now - 9 * DAY, false, 3000); // unfinished: active
        add(&store, &id_n(2), now - 2 * DAY, true, 10);
        let active: HashSet<String> = [id_n(1)].into_iter().collect();
        let report = store.prune(now, 100, &active);
        assert_eq!(report.deleted, vec![id_n(2)]);
        assert_eq!(ids(&store), vec![id_n(1)]);
        assert!(report.unkept_bytes > 3000);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn crash_leftovers_are_finalized_and_part_files_removed() {
        let root = temp_root("leftover");
        let store = Store::new(root.clone());
        let id = id_n(7);
        add(&store, &id, 0, false, 10);
        let dir = store.dir(&id).unwrap();
        let part = dir.join(SEGMENTS).join("000002.webm.part");
        fs::write(&part, b"partial").unwrap();
        fs::write(
            dir.join(EVENTS),
            b"{\"seq\":1,\"at\":\"2026-01-01T00:00:00.000Z\",\"offset_ms\":0}\n\
              {\"seq\":2,\"at\":\"2026-01-01T00:00:05.250Z\",\"offset_ms\":5250}\n\
              {\"seq\":3,\"at\":\"2026-01-0",
        )
        .unwrap();
        assert_eq!(store.finalize_leftovers(&HashSet::new()), vec![id.clone()]);
        let m = Store::read_manifest(&dir).unwrap();
        assert_eq!(m.ended_at.as_deref(), Some("2026-01-01T00:00:05.250Z"));
        assert_eq!(m.duration_ms, Some(5250));
        assert_eq!(m.end_reason.as_deref(), Some("daemon_lost"));
        assert!(!part.exists());
        assert!(dir.join(SEGMENTS).join(segment_file(1)).exists());
        // Finished recordings and active ones are left alone.
        assert!(store.finalize_leftovers(&HashSet::new()).is_empty());
        let other = id_n(8);
        add(&store, &other, 0, false, 1);
        let active: HashSet<String> = [other.clone()].into_iter().collect();
        assert!(store.finalize_leftovers(&active).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn event_reader_stops_at_a_partial_last_line() {
        let root = temp_root("events");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(EVENTS), b"{\"seq\":1}\n{\"seq\":2}\n{\"seq\":").unwrap();
        let events = read_events(&root);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["seq"], 2);
        let _ = fs::remove_dir_all(root);
    }
}
