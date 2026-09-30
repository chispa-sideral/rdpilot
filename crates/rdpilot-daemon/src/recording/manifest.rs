//! The recording manifest (`manifest.json`, format 1): what was recorded,
//! how, and which segments exist. Rewritten atomically (temporary file,
//! then rename) at start, at every segment close and at stop.

use serde::{Deserialize, Serialize};

/// The manifest format version.
pub(crate) const FORMAT: u32 = 1;

/// The session a recording belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionRef {
    /// The session id.
    pub(crate) id: String,
    /// The caller-supplied session name, when one was given.
    #[serde(default)]
    pub(crate) name: Option<String>,
}

/// The event log this recording persists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LogRef {
    pub(crate) schema: u32,
    pub(crate) log_id: String,
    pub(crate) incarnation: u64,
}

/// Recording settings in effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Settings {
    pub(crate) max_fps: f64,
    pub(crate) quantizer: usize,
    pub(crate) speed: u8,
    pub(crate) tiles: usize,
    pub(crate) encoder_threads: usize,
    pub(crate) budget_bytes: u64,
}

/// One closed segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SegmentEntry {
    /// File name under `segments/`, for example `000001.webm`.
    pub(crate) file: String,
    /// Recording-timeline offset of the segment's first frame.
    pub(crate) start_offset_ms: u64,
    /// Recording-timeline offset where the segment's last frame ends.
    pub(crate) end_offset_ms: u64,
    pub(crate) frames: u64,
    pub(crate) bytes: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Recorder cost figures, written at stop.
///
/// `encode_ms_mean` is the encoder's busy time divided by the frames it
/// encoded (the per-frame cost). The encoder returns a frame's packet only
/// after a few later frames or at segment close, so a single call's time
/// includes work for earlier frames; `encode_call_ms_*` are per call.
/// `capture_to_take_ms_*` is how long a captured frame waited for the
/// encoder (the recorder's lag). The frames the encoder still held when a
/// segment closed and the time of that final flush are reported apart.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Stats {
    pub(crate) frames_captured: u64,
    pub(crate) frames_encoded: u64,
    pub(crate) frames_dropped: u64,
    pub(crate) gap_periods: u64,
    pub(crate) encode_ms_mean: f64,
    pub(crate) encode_call_ms_p50: f64,
    pub(crate) encode_call_ms_p95: f64,
    pub(crate) encode_call_ms_max: f64,
    pub(crate) capture_to_take_ms_p50: f64,
    pub(crate) capture_to_take_ms_p95: f64,
    pub(crate) capture_to_take_ms_max: f64,
    pub(crate) frames_held_at_close_max: u64,
    pub(crate) flush_ms_max: f64,
    pub(crate) events_lost: u64,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) format: u32,
    pub(crate) id: String,
    pub(crate) session: SessionRef,
    /// The address the daemon connected to.
    pub(crate) host: String,
    /// UTC start, milliseconds (`YYYY-MM-DDTHH:MM:SS.sssZ`).
    pub(crate) started_at: String,
    /// The same start as Unix milliseconds (for retention).
    pub(crate) started_unix_ms: u64,
    /// UTC end, once the recording stopped.
    #[serde(default)]
    pub(crate) ended_at: Option<String>,
    /// Why it stopped: `requested`, `session_closed`, `daemon_stopped`,
    /// `write_error`, `encoder_error` or `daemon_lost` (the daemon ended
    /// without stopping it).
    #[serde(default)]
    pub(crate) end_reason: Option<String>,
    /// Length of the recording timeline once stopped.
    #[serde(default)]
    pub(crate) duration_ms: Option<u64>,
    pub(crate) rdpilot_version: String,
    /// How it was started: `config`, `host`, `connect_flag`, `cli` or `viewer`.
    pub(crate) trigger: String,
    pub(crate) settings: Settings,
    pub(crate) codec: String,
    pub(crate) container: String,
    pub(crate) log: LogRef,
    #[serde(default)]
    pub(crate) segments: Vec<SegmentEntry>,
    #[serde(default)]
    pub(crate) stats: Option<Stats>,
}

impl Manifest {
    /// Whether the recording has stopped.
    pub(crate) fn finished(&self) -> bool {
        self.ended_at.is_some()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> Manifest {
        Manifest {
            format: FORMAT,
            id: "20260101T000000Z-0123abcd".into(),
            session: SessionRef {
                id: "web".into(),
                name: Some("web".into()),
            },
            host: "10.0.0.5".into(),
            started_at: "2026-01-01T00:00:00.000Z".into(),
            started_unix_ms: 1_767_225_600_000,
            ended_at: None,
            end_reason: None,
            duration_ms: None,
            rdpilot_version: "0.1.0".into(),
            trigger: "cli".into(),
            settings: Settings {
                max_fps: 4.0,
                quantizer: 130,
                speed: 10,
                tiles: 1,
                encoder_threads: 2,
                budget_bytes: 1024,
            },
            codec: "av1".into(),
            container: "webm".into(),
            log: LogRef {
                schema: 1,
                log_id: "00".repeat(16),
                incarnation: 3,
            },
            segments: vec![],
            stats: None,
        }
    }

    #[test]
    fn round_trips_and_readers_ignore_unknown_fields() {
        let manifest = sample();
        let mut json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["format"], 1);
        assert_eq!(json["codec"], "av1");
        assert_eq!(json["container"], "webm");
        json["future"] = serde_json::json!({"a": 1});
        let back: Manifest = serde_json::from_value(json).unwrap();
        assert_eq!(back, manifest);
        assert!(!back.finished());
    }
}
