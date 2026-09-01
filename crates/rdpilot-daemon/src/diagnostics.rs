//! Opt-in, redacted Connect lifecycle diagnostics.
//!
//! Its fixed stage enum and numeric attempt correlation prevent the diagnostic
//! sink from becoming an accidental path for RDP target details, credentials,
//! command lines, session names, sensor payloads, or error strings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};

const DIAGNOSTICS_PATH_ENV: &str = "RDPILOT_DAEMON_DIAGNOSTICS_PATH";
const MAX_EVENTS: usize = 100;
static TMP_SUFFIX: AtomicU64 = AtomicU64::new(0);

/// Every permitted diagnostic stage. The enum is intentionally closed so a
/// future caller cannot put a free-form error string into the evidence file.
#[derive(Clone, Copy)]
pub(crate) enum Stage {
    RegistryOpened,
    SensorBootstrapStarted,
    SensorBootstrapFinished,
    IpcResponseWritten,
    IpcPeerClosed,
    RegistryClosed,
    Bootstrap(rdpilot::BootstrapStage),
}

impl Stage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RegistryOpened => "registry_opened",
            Self::SensorBootstrapStarted => "sensor_bootstrap_started",
            Self::SensorBootstrapFinished => "sensor_bootstrap_finished",
            Self::IpcResponseWritten => "ipc_response_written",
            Self::IpcPeerClosed => "ipc_peer_closed",
            Self::RegistryClosed => "registry_closed",
            Self::Bootstrap(stage) => stage.as_str(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Event {
    schema_version: u8,
    daemon_pid: u32,
    daemon_generation: u64,
    attempt: u64,
    elapsed_ms: u128,
    stage: String,
}

/// Owner-only diagnostics sink. Construction is opt-in and fails closed: on
/// a platform where owner-only storage is not implemented, no file is made.
pub(crate) struct Diagnostics {
    path: PathBuf,
    started: Instant,
    generation: u64,
    write_lock: Mutex<()>,
    attempts: Mutex<HashMap<String, u64>>,
    next_attempt: AtomicU64,
}

impl Diagnostics {
    /// Use an explicit path as the opt-in switch. Unix establishes 0700/0600
    /// storage; non-Unix targets return `None` until an equivalent verified
    /// owner-only DACL implementation exists, rather than writing a weaker
    /// file.
    #[must_use]
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os(DIAGNOSTICS_PATH_ENV).map(PathBuf::from)?;
        Self::at_owner_only_path(path)
    }

    #[cfg(unix)]
    fn at_owner_only_path(path: PathBuf) -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;

        let parent = path.parent()?;
        std::fs::create_dir_all(parent).ok()?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).ok()?;
        Some(Self {
            path,
            started: Instant::now(),
            generation: TMP_SUFFIX.fetch_add(1, Ordering::Relaxed),
            write_lock: Mutex::new(()),
            attempts: Mutex::new(HashMap::new()),
            next_attempt: AtomicU64::new(1),
        })
    }

    #[cfg(not(unix))]
    fn at_owner_only_path(_path: PathBuf) -> Option<Self> {
        // The named-pipe DACL is not a filesystem-file DACL. Until this
        // project has a reusable verified file-DACL helper, diagnostics
        // deliberately remain unavailable on non-Unix platforms.
        None
    }

    /// Append a fixed-schema event by atomically replacing a bounded JSON
    /// vector. Any storage failure is fail-closed and produces no fallback
    /// output or log line.
    pub(crate) fn record(&self, session_id: &str, stage: Stage) {
        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };
        let mut events = load_events(&self.path).unwrap_or_default();
        let attempt = {
            let mut attempts = match self.attempts.lock() {
                Ok(attempts) => attempts,
                Err(poisoned) => poisoned.into_inner(),
            };
            *attempts
                .entry(session_id.to_owned())
                .or_insert_with(|| self.next_attempt.fetch_add(1, Ordering::Relaxed))
        };
        events.push(Event {
            schema_version: 1,
            daemon_pid: std::process::id(),
            daemon_generation: self.generation,
            attempt,
            elapsed_ms: self.started.elapsed().as_millis(),
            stage: stage.as_str().to_owned(),
        });
        let first = events.len().saturating_sub(MAX_EVENTS);
        let events = &events[first..];
        let _ = save_owner_only(&self.path, events);
    }

    /// Persist a fixed bootstrap vector. Each element is an enum value owned
    /// by `rdpilot`, never caller-provided text.
    pub(crate) fn record_bootstrap_stages(
        &self,
        session_id: &str,
        stages: &[rdpilot::BootstrapStage],
    ) {
        for stage in stages {
            self.record(session_id, Stage::Bootstrap(*stage));
        }
    }
}

#[cfg(unix)]
fn load_events(path: &Path) -> Option<Vec<Event>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    serde_json::from_slice(&bytes).ok()
}

#[cfg(not(unix))]
fn load_events(_path: &Path) -> Option<Vec<Event>> {
    None
}

#[cfg(unix)]
fn save_owner_only(path: &Path, events: &[Event]) -> std::io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let json = serde_json::to_vec(events).map_err(std::io::Error::other)?;
    let suffix = TMP_SUFFIX.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!(
        ".rdpilot-connect-diagnostics-{}-{suffix}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&json)?;
    file.sync_all()?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, path)
}

#[cfg(not(unix))]
fn save_owner_only(_path: &Path, _events: &[Event]) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "owner-only diagnostics unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bootstrap_vector_is_fixed_and_does_not_persist_free_form_text() {
        let parent = std::env::temp_dir().join(format!(
            "rdpilot-daemon-diagnostics-test-{}-{}",
            std::process::id(),
            TMP_SUFFIX.fetch_add(1, Ordering::Relaxed)
        ));
        let path = parent.join("diagnostics.json");
        let diagnostics = Diagnostics::at_owner_only_path(path.clone()).expect("owner-only diagnostics");
        let stages = [
            rdpilot::BootstrapStage::RdpdrFileAccess,
            rdpilot::BootstrapStage::RdpdrFileRead,
            rdpilot::BootstrapStage::LaunchInputAttempted,
            rdpilot::BootstrapStage::LaunchInputSent,
            rdpilot::BootstrapStage::DvcChannelCreated,
            rdpilot::BootstrapStage::DvcChannelOpen,
            rdpilot::BootstrapStage::VersionReceived,
            rdpilot::BootstrapStage::PingSent,
            rdpilot::BootstrapStage::PongReceived,
        ];
        diagnostics.record_bootstrap_stages("attempt-1", &stages);

        let json = std::fs::read_to_string(path).expect("diagnostics written");
        let events: Vec<Event> = serde_json::from_str(&json).expect("fixed event schema");
        assert_eq!(
            events.iter().map(|event| event.stage.as_str()).collect::<Vec<_>>(),
            stages.iter().map(|stage| stage.as_str()).collect::<Vec<_>>()
        );
        assert!(!json.contains("attempt-1"));
        assert!(!json.contains("password"));
        assert!(!json.contains("command"));
        assert!(!json.contains("path"));
        std::fs::remove_dir_all(parent).expect("temporary diagnostics removed");
    }
}
