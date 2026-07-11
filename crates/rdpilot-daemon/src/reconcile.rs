//! Disk-persisted crash-restart reconciliation state: written on every
//! connect/disconnect/reap, read once at daemon startup to surface
//! possibly-still-live orphaned remote sessions (Plan 12-05; DAEMON-04).
//!
//! [`ReconciliationRecord`] is the minimal on-disk shape (id/host/
//! connected_since — never a credential, D-31). [`JsonReconciliationSink`]
//! implements [`crate::seams::ReconciliationSink`] by rewriting the whole
//! `Vec<ReconciliationRecord>` file atomically (temp-write + `rename`) on
//! every `record_open`/`record_closed` call, so a `kill -9` mid-write
//! leaves either the old or the new complete file, never a torn one
//! (T-12-15). [`scan_orphans`] is the startup-time read: any record still
//! present was never cleanly removed by its owning daemon process, so it
//! is a candidate orphan (Pitfall 9) — a missing or corrupt file yields an
//! empty scan rather than blocking daemon startup (T-12-16), and
//! [`seed_into`] is the bridge that feeds those candidates into the
//! registry as `Orphaned` entries (T-12-14), never auto-torn-down
//! (T-12-17, D-31) — only an explicit `Registry::close` reconciles one.
//!
//! `record_open`/`record_closed` are synchronous and infallible in
//! signature ([`crate::seams::ReconciliationSink`]'s contract): a failed
//! disk write is swallowed-and-logged here rather than panicking or
//! propagating, so a transient I/O failure degrades to best-effort
//! (a missed orphan surface — a visibility gap) rather than crashing the
//! daemon over its own crash-recovery bookkeeping.

// This crate's ONLY production caller of this module's public surface is
// Plan 12-06's `server.rs` `run()` (the startup scan + seed), which has
// not landed yet — mirrors `registry.rs`'s own module-level `dead_code`
// allowance for the exact same interface-first reason (this plan's own
// inline tests below exercise every item; production wiring is Plan
// 12-06's job).
#![allow(dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use rdpilot_ipc::SessionId;
use serde::{Deserialize, Serialize};

use crate::registry::Registry;
use crate::seams::ReconciliationSink;

/// A minimal per-session reconciliation record (DAEMON-04): just enough to
/// identify a possibly-still-live remote session on restart. Never a
/// credential-shaped field (D-31) — `host` is non-secret target
/// addressing, matching `SessionStatus`'s own field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciliationRecord {
    /// `SessionId::as_str()`.
    pub id: String,
    /// The target host (non-secret target addressing, D-31).
    pub host: String,
    /// ISO-8601 timestamp of when the session originally connected.
    pub connected_since: String,
}

/// A [`ReconciliationSink`] backed by a single JSON file on disk, rewritten
/// wholesale (atomically) on every state transition.
#[derive(Debug, Clone)]
pub struct JsonReconciliationSink {
    path: PathBuf,
}

impl JsonReconciliationSink {
    /// Resolve the platform-conventional reconciliation-state file path
    /// under `directories::BaseDirs::cache_dir()` (mirroring
    /// `rdpilot-config::paths::config_file_path`'s `BaseDirs` reuse
    /// pattern) and construct a sink at it.
    ///
    /// Returns `None` only when `BaseDirs::new()` itself cannot resolve a
    /// home directory on this platform — an environment-level condition,
    /// not a reconciliation-content error.
    #[must_use]
    pub fn new() -> Option<Self> {
        let base = directories::BaseDirs::new()?;
        let path = base.cache_dir().join("rdpilot").join("sessions.json");
        Some(JsonReconciliationSink::at(path))
    }

    /// Construct a sink at an explicit path (test injection point — Plan
    /// 12-05's own inline tests and the `tests/crash_restart_reconcile.rs`
    /// integration test use a unique temp path per run so they never
    /// clobber a real state file or collide with each other).
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        JsonReconciliationSink { path }
    }

    /// The path this sink reads/writes (test/diagnostic helper).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Vec<ReconciliationRecord> {
        load_records(&self.path)
    }

    fn save(&self, records: &[ReconciliationRecord]) {
        if let Err(err) = save_records(&self.path, records) {
            // Best-effort (see module doc): a failed reconciliation write
            // must not crash the daemon. The worst case is a missed
            // orphan surface on a future crash — a visibility gap, not a
            // data-destroying action.
            eprintln!("rdpilot-daemon: failed to write reconciliation file {:?}: {err}", self.path);
        }
    }
}

impl ReconciliationSink for JsonReconciliationSink {
    fn record_open(&self, id: &SessionId, host: &str, connected_since: &str) {
        let mut records = self.load();
        match records.iter_mut().find(|r| r.id == id.as_str()) {
            Some(existing) => {
                existing.host = host.to_owned();
                existing.connected_since = connected_since.to_owned();
            }
            None => records.push(ReconciliationRecord {
                id: id.as_str().to_owned(),
                host: host.to_owned(),
                connected_since: connected_since.to_owned(),
            }),
        }
        self.save(&records);
    }

    fn record_closed(&self, id: &SessionId) {
        let mut records = self.load();
        records.retain(|r| r.id != id.as_str());
        self.save(&records);
    }
}

/// Read the reconciliation-state file at `path` and return every record
/// still present — the candidate orphans left by a crashed predecessor
/// daemon that never got to remove them via a clean `record_closed`.
///
/// Returns `Vec::new()` for an absent, empty, or corrupt/unparseable file
/// (logged, not panicked) — a garbage state file must never block daemon
/// startup (T-12-16).
#[must_use]
pub fn scan_orphans(path: &Path) -> Vec<ReconciliationRecord> {
    load_records(path)
}

fn load_records(path: &Path) -> Vec<ReconciliationRecord> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            eprintln!("rdpilot-daemon: could not read reconciliation file {path:?}: {err}");
            return Vec::new();
        }
    };
    match serde_json::from_slice::<Vec<ReconciliationRecord>>(&bytes) {
        Ok(records) => records,
        Err(err) => {
            eprintln!("rdpilot-daemon: reconciliation file {path:?} is corrupt ({err}); treating as empty");
            Vec::new()
        }
    }
}

/// Monotonic per-process counter mixed into [`tmp_sibling_path`] so two
/// overlapping writes to the same target path never collide on the same
/// temp file name.
static TMP_SUFFIX_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A sibling temp path next to `path` (same directory, so the subsequent
/// `rename` is same-filesystem and therefore atomic).
fn tmp_sibling_path(path: &Path) -> PathBuf {
    let suffix = TMP_SUFFIX_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("sessions.json"));
    name.push(format!(".tmp-{}-{suffix}", std::process::id()));
    path.with_file_name(name)
}

/// Serialize `records` and rewrite the whole file at `path` ATOMICALLY:
/// write a sibling temp file, then `rename` it over `path` (rename is
/// atomic on the same filesystem, T-12-15) — a crash mid-write leaves
/// either the old or the new complete file, never a torn one.
fn save_records(path: &Path, records: &[ReconciliationRecord]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(records).map_err(io::Error::other)?;
    let tmp_path = tmp_sibling_path(path);
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Seed every leftover reconciliation record into `registry` as an
/// `Orphaned` entry (DAEMON-04) — the bridge Plan 12-06's `run()` calls
/// once at startup, after [`scan_orphans`]. Never auto-tears-down a
/// session (D-31/T-12-17); this only makes each candidate orphan visible
/// via `Registry::list`.
pub fn seed_into(records: Vec<ReconciliationRecord>, registry: &Registry) {
    for record in records {
        match SessionId::from_str(&record.id) {
            Ok(id) => registry.seed_orphan(id, record.host, record.connected_since),
            Err(err) => {
                eprintln!("rdpilot-daemon: skipping malformed reconciliation record id {:?}: {err}", record.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::seams::{NoopReconciliationSink, SessionConnector};

    /// A unique temp-file path per test invocation (no `tempfile` crate
    /// dependency — a monotonic counter mixed into the process id keeps
    /// concurrent test threads from colliding, mirroring
    /// `registry.rs::generate_auto_id`'s own no-new-dependency pattern).
    static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);
    fn unique_test_path(label: &str) -> PathBuf {
        let n = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("rdpilot-daemon-reconcile-test-{}-{label}-{n}.json", std::process::id()))
    }

    fn test_id(s: &str) -> SessionId {
        SessionId::from_str(s).expect("non-empty literal")
    }

    #[test]
    fn json_reconciliation_sink_implements_reconciliation_sink() {
        fn assert_impl(_s: &dyn ReconciliationSink) {}
        let sink = JsonReconciliationSink::at(unique_test_path("impl-check"));
        assert_impl(&sink);
    }

    #[test]
    fn record_open_then_scan_orphans_returns_the_record() {
        let path = unique_test_path("open-then-scan");
        let sink = JsonReconciliationSink::at(path.clone());
        sink.record_open(&test_id("payroll"), "10.0.0.9", "2026-01-01T00:00:00Z");

        let scanned = scan_orphans(&path);
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].id, "payroll");
        assert_eq!(scanned[0].host, "10.0.0.9");
        assert_eq!(scanned[0].connected_since, "2026-01-01T00:00:00Z");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn record_open_then_record_closed_then_scan_orphans_returns_empty() {
        let path = unique_test_path("open-then-close-then-scan");
        let sink = JsonReconciliationSink::at(path.clone());
        sink.record_open(&test_id("payroll"), "10.0.0.9", "2026-01-01T00:00:00Z");
        sink.record_closed(&test_id("payroll"));

        let scanned = scan_orphans(&path);
        assert!(scanned.is_empty(), "expected no records after a clean close, got {scanned:?}");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn scan_orphans_on_an_absent_file_returns_empty_without_panicking() {
        let path = unique_test_path("absent");
        // Deliberately never written.
        let scanned = scan_orphans(&path);
        assert!(scanned.is_empty());
    }

    #[test]
    fn scan_orphans_on_a_truncated_garbage_file_returns_empty_without_panicking() {
        let path = unique_test_path("garbage");
        fs::write(&path, b"{ not valid json at all [[[").expect("scratch write should succeed");

        let scanned = scan_orphans(&path);
        assert!(scanned.is_empty(), "a corrupt file must yield an empty scan, not a panic");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn concurrent_ish_upsert_of_two_ids_yields_both() {
        let path = unique_test_path("two-ids");
        let sink = JsonReconciliationSink::at(path.clone());
        sink.record_open(&test_id("payroll"), "10.0.0.9", "2026-01-01T00:00:00Z");
        sink.record_open(&test_id("web"), "10.0.0.5", "2026-01-01T00:01:00Z");

        let mut scanned = scan_orphans(&path);
        scanned.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].id, "payroll");
        assert_eq!(scanned[1].id, "web");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn record_open_upserts_rather_than_duplicating_the_same_id() {
        let path = unique_test_path("upsert");
        let sink = JsonReconciliationSink::at(path.clone());
        sink.record_open(&test_id("payroll"), "10.0.0.9", "2026-01-01T00:00:00Z");
        sink.record_open(&test_id("payroll"), "10.0.0.10", "2026-01-01T00:05:00Z");

        let scanned = scan_orphans(&path);
        assert_eq!(scanned.len(), 1, "re-opening the same id must upsert, not append a duplicate");
        assert_eq!(scanned[0].host, "10.0.0.10");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn writes_go_through_a_temp_file_rename_not_a_direct_write() {
        // Structural guard for T-12-15: after a save, no leftover
        // `.tmp-` sibling remains (rename consumed it) and the target
        // file itself parses cleanly.
        let path = unique_test_path("atomic-write");
        let sink = JsonReconciliationSink::at(path.clone());
        sink.record_open(&test_id("payroll"), "10.0.0.9", "2026-01-01T00:00:00Z");

        let dir = path.parent().expect("temp path has a parent").to_path_buf();
        let stem = path.file_name().expect("temp path has a file name").to_string_lossy().into_owned();
        let leftover_tmp = fs::read_dir(&dir)
            .expect("temp dir should be readable")
            .filter_map(Result::ok)
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with(&stem) && name.contains(".tmp-")
            });
        assert!(!leftover_tmp, "no .tmp- sibling should survive a completed atomic write");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn seed_into_populates_the_registry_with_orphaned_entries() {
        let registry = Registry::new(Arc::new(NoopSessionConnector), Arc::new(NoopReconciliationSink));
        let records = vec![
            ReconciliationRecord { id: "payroll".to_owned(), host: "10.0.0.9".to_owned(), connected_since: "2026-01-01T00:00:00Z".to_owned() },
        ];
        seed_into(records, &registry);

        let statuses = registry.list();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "payroll");
        assert_eq!(statuses[0].host, "10.0.0.9");
        assert_eq!(statuses[0].status, rdpilot_ipc::SessionLifecycle::Orphaned);
    }

    #[test]
    fn seed_into_skips_malformed_ids_without_panicking() {
        let registry = Registry::new(Arc::new(NoopSessionConnector), Arc::new(NoopReconciliationSink));
        let records = vec![
            ReconciliationRecord { id: String::new(), host: "10.0.0.9".to_owned(), connected_since: "2026-01-01T00:00:00Z".to_owned() },
        ];
        seed_into(records, &registry);

        assert!(registry.list().is_empty(), "a malformed id must be skipped, not seeded");
    }

    /// A `SessionConnector` never actually invoked by these tests
    /// (`seed_into` never calls `connect`) — only present to satisfy
    /// `Registry::new`'s constructor.
    struct NoopSessionConnector;
    impl SessionConnector for NoopSessionConnector {
        fn connect(
            &self,
            _cfg: rdpilot::ConnectionConfig,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Box<dyn crate::seams::ManagedSession>, crate::seams::DaemonError>>>> {
            Box::pin(async { Err(crate::seams::DaemonError::Connect("unused".to_owned())) })
        }
    }
}
