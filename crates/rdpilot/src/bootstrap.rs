//! Redacted progress for one sensor bootstrap attempt.
//!
//! The stages are a closed vocabulary. They intentionally carry no target,
//! command, path, payload, or error text, so callers can persist them in an
//! owner-only diagnostic without turning it into a secret-bearing trace.

use std::sync::Mutex;

/// A fixed, redacted milestone in the in-band sensor bootstrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapStage {
    RdpdrFileAccess,
    RdpdrFileRead,
    LaunchInputAttempted,
    LaunchInputSent,
    DvcChannelCreated,
    DvcChannelOpen,
    VersionReceived,
    PingSent,
    PongReceived,
}

impl BootstrapStage {
    /// Stable, safe-to-persist diagnostic label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RdpdrFileAccess => "rdpdr_file_access",
            Self::RdpdrFileRead => "rdpdr_file_read",
            Self::LaunchInputAttempted => "launch_input_attempted",
            Self::LaunchInputSent => "launch_input_sent",
            Self::DvcChannelCreated => "dvc_channel_created",
            Self::DvcChannelOpen => "dvc_channel_open",
            Self::VersionReceived => "version_received",
            Self::PingSent => "ping_sent",
            Self::PongReceived => "pong_received",
        }
    }
}

/// Thread-safe, idempotent recorder shared by the RDPDR backend, interactive
/// input path, and DVC processor for the lifetime of one session.
#[derive(Debug)]
pub(crate) struct BootstrapProgress {
    stages: Mutex<Vec<BootstrapStage>>,
}

impl BootstrapProgress {
    pub(crate) fn new() -> Self {
        Self {
            stages: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn record(&self, stage: BootstrapStage) {
        let mut stages = match self.stages.lock() {
            Ok(stages) => stages,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !stages.contains(&stage) {
            stages.push(stage);
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<BootstrapStage> {
        match self.stages.lock() {
            Ok(stages) => stages.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_ordered_deduplicated_and_redacted() {
        let progress = BootstrapProgress::new();
        progress.record(BootstrapStage::RdpdrFileAccess);
        progress.record(BootstrapStage::RdpdrFileAccess);
        progress.record(BootstrapStage::PongReceived);

        let stages = progress.snapshot();
        assert_eq!(
            stages,
            vec![
                BootstrapStage::RdpdrFileAccess,
                BootstrapStage::PongReceived,
            ]
        );
        assert_eq!(stages.iter().map(|stage| stage.as_str()).collect::<Vec<_>>(), vec!["rdpdr_file_access", "pong_received"]);
    }
}
