//! Per-session event log: what an agent or the CLI did in one session.
//!
//! - One [`SessionEvents`] per live session incarnation, created at connect
//!   and dropped with the registry entry. It keeps the newest
//!   [`RING_CAPACITY`] events in memory. The log itself writes nothing; an
//!   attached recorder ([`SessionEvents::attach_sink`]) persists it.
//! - Records carry names, outcomes and timings only. Argument values, typed
//!   text, file paths, results, images, error text and JSON-RPC ids never
//!   enter a record.
//! - Recording takes only this log's own mutex for a push and a clone. It
//!   never awaits, never takes the per-session mutex and never changes the
//!   session's activity time.
//! - The record types are the stable, additive schema 1: the in-memory ring,
//!   the viewer's HTTP body and the recording's event log use them
//!   unchanged. Readers ignore unknown fields and unknown kinds.
//! - Recording-only kinds (lifecycle, annotations) go to the attached sink
//!   only ([`SessionEvents::record_detached`]): they never enter the ring,
//!   never take a ring sequence number and never change `latest`, so the
//!   live strip and `/events` are unchanged by recording.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rdpilot_ipc::SessionId;
/// What started a recording; its serde and manifest spellings are declared
/// with the type in `rdpilot-vocab`.
pub(crate) use rdpilot_vocab::RecordingTrigger;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::registry::iso8601_millis_from_system_time;
use crate::seams::ViewFrameSource;

/// Events kept per session; older events are dropped.
pub const RING_CAPACITY: usize = 200;

/// The record schema version.
pub const SCHEMA: u32 = 1;

/// Identifies one log (one session incarnation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogHeader {
    /// Record schema version ([`SCHEMA`]).
    pub schema: u32,
    /// 128-bit random hex id, unique across daemon restarts.
    pub log_id: String,
    /// The session id.
    pub session: String,
    /// The registry's incarnation number for this session.
    pub incarnation: u64,
    /// When the log started (UTC, milliseconds).
    pub started_at: String,
}

/// Who caused an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// A Cua tool call through an attached Cua stream.
    Cua,
    /// A native rdpilot verb (CLI or MCP native tools).
    Cli,
    /// The live viewer page.
    Viewer,
    /// The daemon itself (recording lifecycle).
    Daemon,
}

/// Why a recording stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// `rdpilot record stop` or the viewer's Stop control.
    Requested,
    /// The session was closed.
    SessionClosed,
    /// The daemon shut down.
    DaemonStopped,
    /// A recording file could not be written.
    WriteError,
    /// The video encoder failed.
    EncoderError,
}

impl StopReason {
    /// The manifest spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Requested => "requested",
            StopReason::SessionClosed => "session_closed",
            StopReason::DaemonStopped => "daemon_stopped",
            StopReason::WriteError => "write_error",
            StopReason::EncoderError => "encoder_error",
        }
    }
}

/// How a session was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    /// `rdpilot disconnect`.
    Disconnect,
    /// The idle reaper closed it.
    IdleReap,
    /// The client that asked for the connect went away before it completed.
    ConnectAborted,
}

/// Why a video segment was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentCloseReason {
    /// The segment reached its maximum length.
    Duration,
    /// The desktop size changed.
    Resize,
    /// The recording stopped.
    Stop,
    /// The recording reached its size cap.
    SizeCap,
    /// The encoder or a write failed.
    Error,
}

/// Why the video of a recording stopped while events continue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoStopReason {
    /// The recording reached the size cap.
    SizeCap,
    /// The video encoder failed.
    EncoderError,
    /// A video file could not be written.
    WriteError,
}

/// How a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallOutcome {
    Ok,
    Error,
    /// The stream closed before an answer arrived.
    NoReply,
}

/// What happened. Internally tagged by `kind`; new kinds are additive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    CallStarted {
        /// Per-log call number (never the client's request id).
        call: u64,
        /// Tool or verb name.
        name: String,
    },
    CallFinished {
        call: u64,
        name: String,
        outcome: CallOutcome,
        duration_ms: u64,
    },
    CuaAttached {
        attachment: u64,
    },
    CuaDetached {
        attachment: u64,
        reason: String,
    },
    /// The remote session ended (server side).
    SessionEnded,
    /// Recording only: a recording started.
    RecordingStarted {
        trigger: RecordingTrigger,
    },
    /// Recording only: the recording stopped (always its last event).
    RecordingStopped {
        reason: StopReason,
    },
    /// Recording only: the daemon closed the session.
    SessionClosed {
        reason: CloseReason,
    },
    /// Recording only: the desktop size changed.
    DesktopResized {
        width: u32,
        height: u32,
    },
    /// Recording only: a video segment started with its first frame.
    SegmentStarted {
        segment: u32,
        video_offset_ms: u64,
        width: u32,
        height: u32,
    },
    /// Recording only: a video segment was closed and is playable.
    SegmentClosed {
        segment: u32,
        frames: u64,
        bytes: u64,
        duration_ms: u64,
        reason: SegmentCloseReason,
    },
    /// Recording only: the encoder fell behind and skipped display states.
    FramesDropped {
        from_offset_ms: u64,
        to_offset_ms: u64,
        count: u64,
    },
    /// Recording only: events that could not be queued for the recording.
    EventsLost {
        count: u64,
    },
    /// Recording only: video writing stopped; events continue.
    VideoStopped {
        reason: VideoStopReason,
    },
    /// Recording only: a note added from the CLI or the viewer.
    Annotation {
        text: String,
    },
    /// A human viewer took control from the agent.
    ControlTaken {
        by: crate::control::ControllerRef,
    },
    /// Control moved from one controller to another (agent takeover, or
    /// another viewer tab).
    ControlTakenOver {
        from: crate::control::ControllerRef,
        by: crate::control::ControllerRef,
    },
    /// The human holder released control; the agent controls.
    ControlReleased {
        from: crate::control::ControllerRef,
    },
    /// A human lease ended without a takeover; the agent controls.
    ControlEnded {
        from: crate::control::ControllerRef,
        reason: crate::control::EndReason,
    },
    /// A kind this reader does not know (read side only).
    #[serde(other)]
    Unknown,
}

/// One event record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Per-log sequence number, from 1, without gaps.
    pub seq: u64,
    /// Wall-clock time (UTC, milliseconds), for display.
    pub at: String,
    /// Milliseconds since the log started on the monotonic clock.
    pub offset_ms: u64,
    /// The session's frame sequence number at record time (0 without frames).
    pub frame_seq: u64,
    pub source: EventSource,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// The log's clock: a monotonic start plus its wall-clock reading, so
/// other recorders (for example frame capture) can stamp on the same clock.
#[derive(Debug, Clone, Copy)]
pub struct SessionClock {
    pub start: Instant,
    pub start_wall: SystemTime,
}

impl SessionClock {
    fn now() -> Self {
        SessionClock {
            start: Instant::now(),
            start_wall: SystemTime::now(),
        }
    }

    /// Milliseconds since the start.
    #[must_use]
    pub fn offset_ms(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.start).as_millis()).unwrap_or(u64::MAX)
    }

    /// The wall-clock time of `at`, derived from the monotonic clock.
    #[must_use]
    pub fn wall(&self, at: Instant) -> SystemTime {
        self.start_wall + at.saturating_duration_since(self.start)
    }
}

/// Receives every event in order while attached, including events later
/// dropped from the in-memory ring and the recording-only events. Called
/// under the log's mutex: it must not block (use a bounded queue).
pub trait EventSink: Send + Sync {
    /// Offer one event; `false` when the sink could not take it.
    fn on_event(&self, header: &LogHeader, event: &SessionEvent) -> bool;
    /// The sink is detached; `last` is its final event and must not be lost.
    fn on_detach(&self, header: &LogHeader, last: &SessionEvent);
}

/// A page of events after a given sequence number.
#[derive(Debug, Clone, Serialize)]
pub struct EventsPage {
    pub header: LogHeader,
    /// The oldest retained sequence number (`latest + 1` when none is
    /// retained). `oldest > after + 1` means events were dropped.
    pub oldest: u64,
    /// The newest recorded sequence number (0 before the first event).
    pub latest: u64,
    pub events: Vec<SessionEvent>,
}

struct Ring {
    events: VecDeque<SessionEvent>,
    latest: u64,
    sink: Option<Box<dyn EventSink>>,
}

/// One session incarnation's event log.
pub struct SessionEvents {
    header: LogHeader,
    clock: SessionClock,
    frame: Option<Arc<dyn ViewFrameSource>>,
    ring: Mutex<Ring>,
    latest: watch::Sender<u64>,
    next_call: AtomicU64,
}

impl SessionEvents {
    /// A new, empty log.
    pub(crate) fn new(
        session: &SessionId,
        incarnation: u64,
        frame: Option<Arc<dyn ViewFrameSource>>,
    ) -> Self {
        let clock = SessionClock::now();
        SessionEvents {
            header: LogHeader {
                schema: SCHEMA,
                log_id: new_log_id(),
                session: session.as_str().to_owned(),
                incarnation,
                started_at: iso8601_millis_from_system_time(clock.start_wall),
            },
            clock,
            frame,
            ring: Mutex::new(Ring {
                events: VecDeque::with_capacity(RING_CAPACITY),
                latest: 0,
                sink: None,
            }),
            latest: watch::Sender::new(0),
            next_call: AtomicU64::new(1),
        }
    }

    fn ring(&self) -> MutexGuard<'_, Ring> {
        match self.ring.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[must_use]
    pub fn header(&self) -> &LogHeader {
        &self.header
    }

    #[must_use]
    pub fn clock(&self) -> SessionClock {
        self.clock
    }

    /// Allocate a per-log call number.
    pub fn next_call(&self) -> u64 {
        self.next_call.fetch_add(1, Ordering::Relaxed)
    }

    fn frame_seq(&self) -> u64 {
        self.frame.as_ref().map_or(0, |frame| frame.status().seq)
    }

    /// Stamp an event with the current time on the log's clock.
    fn stamp(
        &self,
        seq: u64,
        frame_seq: u64,
        source: EventSource,
        kind: EventKind,
    ) -> SessionEvent {
        let now = Instant::now();
        SessionEvent {
            seq,
            at: iso8601_millis_from_system_time(self.clock.wall(now)),
            offset_ms: self.clock.offset_ms(now),
            frame_seq,
            source,
            kind,
        }
    }

    /// Append an event and return its sequence number.
    pub fn record(&self, source: EventSource, kind: EventKind) -> u64 {
        let frame_seq = self.frame_seq();
        let mut guard = self.ring();
        let ring = &mut *guard;
        ring.latest += 1;
        let event = self.stamp(ring.latest, frame_seq, source, kind);
        if ring.events.len() == RING_CAPACITY {
            ring.events.pop_front();
        }
        if let Some(sink) = &ring.sink {
            sink.on_event(&self.header, &event);
        }
        ring.events.push_back(event);
        let seq = ring.latest;
        self.latest.send_replace(seq);
        seq
    }

    /// Attach `sink`, first handing it the event `(source, kind)` stamped
    /// now. From then on it receives every event in order. Returns the
    /// stamped first event, or the sink back when one is already attached.
    ///
    /// # Errors
    ///
    /// The unchanged `sink` when a sink is already attached.
    pub(crate) fn attach_sink(
        &self,
        sink: Box<dyn EventSink>,
        source: EventSource,
        kind: EventKind,
    ) -> Result<SessionEvent, Box<dyn EventSink>> {
        let frame_seq = self.frame_seq();
        let mut ring = self.ring();
        if ring.sink.is_some() {
            return Err(sink);
        }
        let first = self.stamp(0, frame_seq, source, kind);
        sink.on_event(&self.header, &first);
        ring.sink = Some(sink);
        Ok(first)
    }

    /// Detach the sink, handing it `(source, kind)` stamped now as its final
    /// event (never lost). Returns that event, or `None` without a sink.
    pub(crate) fn detach_sink(&self, source: EventSource, kind: EventKind) -> Option<SessionEvent> {
        let frame_seq = self.frame_seq();
        let mut ring = self.ring();
        let sink = ring.sink.take()?;
        let last = self.stamp(0, frame_seq, source, kind);
        sink.on_detach(&self.header, &last);
        Some(last)
    }

    /// Give a recording-only event to the attached sink only: it does not
    /// enter the ring, takes no sequence number and leaves `latest`
    /// unchanged. `false` when no sink is attached or it could not take it.
    pub(crate) fn record_detached(&self, source: EventSource, kind: EventKind) -> bool {
        let frame_seq = self.frame_seq();
        let ring = self.ring();
        let Some(sink) = &ring.sink else {
            return false;
        };
        let event = self.stamp(0, frame_seq, source, kind);
        sink.on_event(&self.header, &event)
    }

    /// Whether a sink is attached.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_sink(&self) -> bool {
        self.ring().sink.is_some()
    }

    /// Record a call start and return its call number.
    pub fn call_started(&self, source: EventSource, name: &str) -> u64 {
        let call = self.next_call();
        self.record(
            source,
            EventKind::CallStarted {
                call,
                name: name.to_owned(),
            },
        );
        call
    }

    /// Record a call end.
    pub fn call_finished(
        &self,
        source: EventSource,
        call: u64,
        name: &str,
        outcome: CallOutcome,
        started: Instant,
    ) {
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.record(
            source,
            EventKind::CallFinished {
                call,
                name: name.to_owned(),
                outcome,
                duration_ms,
            },
        );
    }

    /// The retained events with `seq > after`.
    #[must_use]
    pub fn after(&self, after: u64) -> EventsPage {
        let ring = self.ring();
        let latest = ring.latest;
        let oldest = ring.events.front().map_or(latest + 1, |e| e.seq);
        let events = ring
            .events
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect();
        EventsPage {
            header: self.header.clone(),
            oldest,
            latest,
            events,
        }
    }

    /// Watch the newest sequence number.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.latest.subscribe()
    }
}

/// A spawned watcher task, aborted when dropped.
pub struct WatchTask(tokio::task::AbortHandle);

impl Drop for WatchTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Record `session_ended` once when `frame` reports that the session loop
/// ended. Runs on the multi-thread runtime (`tokio::spawn`); the returned
/// handle aborts it.
pub(crate) fn watch_session_end(
    frame: Arc<dyn ViewFrameSource>,
    events: Arc<SessionEvents>,
    control: Arc<crate::control::SessionControl>,
) -> WatchTask {
    let task = tokio::spawn(async move {
        let mut after = 0;
        loop {
            let status = frame.changed(after).await;
            if status.ended {
                events.record(EventSource::Cli, EventKind::SessionEnded);
                crate::registry::end_lease(&control, crate::control::EndReason::SessionEnded).await;
                return;
            }
            if status.seq <= after {
                // A source that answers without progress: do not spin.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            after = status.seq.max(after);
        }
    });
    WatchTask(task.abort_handle())
}

/// A native verb in flight. `finish` records its outcome; dropping it
/// unfinished (the caller went away) records `no_reply`.
pub(crate) struct CliCall {
    events: Arc<SessionEvents>,
    call: u64,
    name: &'static str,
    started: Instant,
    finished: bool,
}

impl CliCall {
    pub(crate) fn start(events: Arc<SessionEvents>, name: &'static str) -> Self {
        let started = Instant::now();
        let call = events.call_started(EventSource::Cli, name);
        Self {
            events,
            call,
            name,
            started,
            finished: false,
        }
    }

    pub(crate) fn finish(mut self, ok: bool) {
        self.end(if ok {
            CallOutcome::Ok
        } else {
            CallOutcome::Error
        });
    }

    fn end(&mut self, outcome: CallOutcome) {
        if !self.finished {
            self.finished = true;
            self.events.call_finished(
                EventSource::Cli,
                self.call,
                self.name,
                outcome,
                self.started,
            );
        }
    }
}

impl Drop for CliCall {
    fn drop(&mut self) {
        self.end(CallOutcome::NoReply);
    }
}

/// Most unanswered Cua tool calls tracked per attachment. When full, the
/// oldest is finished as `no_reply`.
pub(crate) const CUA_PENDING_CAP: usize = 64;

/// Recorded in place of a tool name that is not a short plain identifier.
pub(crate) const INVALID_NAME: &str = "(invalid name)";

struct PendingCall {
    id: serde_json::Value,
    call: u64,
    name: String,
    started: Instant,
}

/// Pairs Cua `tools/call` requests with their responses for one
/// attachment. Only the tool name, outcome and timing are recorded; the
/// JSON-RPC id is used for matching and never stored in the log. Messages
/// are only read, never changed.
pub(crate) struct CuaCallTracker {
    events: Arc<SessionEvents>,
    pending: VecDeque<PendingCall>,
}

impl CuaCallTracker {
    pub(crate) fn new(events: Arc<SessionEvents>) -> Self {
        Self {
            events,
            pending: VecDeque::new(),
        }
    }

    /// A message from the caller towards Cua.
    pub(crate) fn observe_request(&mut self, message: &serde_json::Value) {
        let Some(obj) = message.as_object() else {
            return; // batches and non-objects are not recorded
        };
        if obj.get("method").and_then(serde_json::Value::as_str) != Some("tools/call") {
            return;
        }
        let Some(id) = obj.get("id").filter(|id| id.is_string() || id.is_number()) else {
            return; // a notification: no answer will come
        };
        let name = obj
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(serde_json::Value::as_str)
            .filter(|n| valid_tool_name(n))
            .unwrap_or(INVALID_NAME)
            .to_owned();
        if let Some(pos) = self.pending.iter().position(|p| p.id == *id) {
            if let Some(old) = self.pending.remove(pos) {
                self.finish(old, CallOutcome::NoReply);
            }
        }
        if self.pending.len() >= CUA_PENDING_CAP {
            if let Some(old) = self.pending.pop_front() {
                self.finish(old, CallOutcome::NoReply);
            }
        }
        let started = Instant::now();
        let call = self.events.call_started(EventSource::Cua, &name);
        self.pending.push_back(PendingCall {
            id: id.clone(),
            call,
            name,
            started,
        });
    }

    /// A message from Cua towards the caller.
    pub(crate) fn observe_response(&mut self, message: &serde_json::Value) {
        let Some(obj) = message.as_object() else {
            return;
        };
        if obj.contains_key("method") {
            return;
        }
        let Some(id) = obj.get("id") else {
            return;
        };
        let is_error = obj.contains_key("error");
        let Some(result) = obj.get("result").or(obj.get("error")) else {
            return;
        };
        let Some(pos) = self.pending.iter().position(|p| p.id == *id) else {
            return;
        };
        let Some(pending) = self.pending.remove(pos) else {
            return;
        };
        let outcome = if is_error
            || result.get("isError").and_then(serde_json::Value::as_bool) == Some(true)
        {
            CallOutcome::Error
        } else {
            CallOutcome::Ok
        };
        self.finish(pending, outcome);
    }

    /// The attachment ended: every unanswered call is finished as `no_reply`.
    pub(crate) fn close(&mut self) {
        while let Some(pending) = self.pending.pop_front() {
            self.finish(pending, CallOutcome::NoReply);
        }
    }

    fn finish(&self, pending: PendingCall, outcome: CallOutcome) {
        self.events.call_finished(
            EventSource::Cua,
            pending.call,
            &pending.name,
            outcome,
            pending.started,
        );
    }
}

impl Drop for CuaCallTracker {
    fn drop(&mut self) {
        self.close();
    }
}

/// 1..=64 characters of `[A-Za-z0-9_.-]`.
fn valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// 128 random bits as hex. Falls back to time-derived bits if the OS
/// source fails, so a log can always be created.
fn new_log_id() -> String {
    static FALLBACK: AtomicU64 = AtomicU64::new(0);
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let counter = u128::from(FALLBACK.fetch_add(1, Ordering::Relaxed));
        bytes = (nanos ^ (counter << 64) ^ u128::from(std::process::id())).to_be_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn log() -> SessionEvents {
        SessionEvents::new(&"alpha".parse().unwrap(), 7, None)
    }

    #[test]
    fn ring_keeps_the_newest_events_with_oldest_and_latest() {
        let log = log();
        for _ in 0..(RING_CAPACITY + 50) {
            log.record(EventSource::Cli, EventKind::SessionEnded);
        }
        let page = log.after(0);
        assert_eq!(page.events.len(), RING_CAPACITY);
        assert_eq!(page.latest, (RING_CAPACITY + 50) as u64);
        assert_eq!(page.oldest, 51);
        assert_eq!(page.events.first().unwrap().seq, 51);
        assert_eq!(page.events.last().unwrap().seq, page.latest);
    }

    #[test]
    fn empty_log_reports_nothing_dropped() {
        let page = log().after(0);
        assert_eq!((page.oldest, page.latest), (1, 0));
        assert!(page.events.is_empty());
    }

    #[test]
    fn seq_is_monotonic_and_after_filters() {
        let log = log();
        let seqs: Vec<u64> = (0..5)
            .map(|_| log.record(EventSource::Cua, EventKind::SessionEnded))
            .collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
        let page = log.after(3);
        assert_eq!(
            page.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert!(log.after(5).events.is_empty());
        assert_eq!(*log.subscribe().borrow(), 5);
    }

    #[test]
    fn offsets_do_not_decrease_and_calls_are_numbered() {
        let log = log();
        let a = log.call_started(EventSource::Cli, "screenshot");
        let b = log.call_started(EventSource::Cli, "mouse");
        assert_eq!((a, b), (1, 2));
        log.call_finished(
            EventSource::Cli,
            a,
            "screenshot",
            CallOutcome::Ok,
            Instant::now(),
        );
        let events = log.after(0).events;
        assert!(events.windows(2).all(|w| w[0].offset_ms <= w[1].offset_ms));
        assert!(matches!(
            &events[2].kind,
            EventKind::CallFinished { call: 1, name, outcome: CallOutcome::Ok, .. } if name == "screenshot"
        ));
    }

    #[test]
    fn millisecond_timestamps_have_a_fixed_shape() {
        let t = UNIX_EPOCH + Duration::from_millis(1_704_067_200_007);
        assert_eq!(
            iso8601_millis_from_system_time(t),
            "2024-01-01T00:00:00.007Z"
        );
        let log = log();
        log.record(EventSource::Cli, EventKind::SessionEnded);
        let at = &log.after(0).events[0].at;
        assert_eq!(at.len(), "2024-01-01T00:00:00.000Z".len());
        assert!(at.ends_with('Z') && at.as_bytes()[19] == b'.');
        assert_eq!(log.header().started_at.len(), at.len());
    }

    #[test]
    fn log_ids_are_random_128_bit_hex() {
        let a = log().header().log_id.clone();
        let b = log().header().log_id.clone();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    /// Pins schema 1: header and one event of each kind.
    #[test]
    fn schema_1_golden_json() {
        let header = LogHeader {
            schema: 1,
            log_id: "00112233445566778899aabbccddeeff".into(),
            session: "alpha".into(),
            incarnation: 7,
            started_at: "2024-01-01T00:00:00.000Z".into(),
        };
        let event = |seq, source, kind| SessionEvent {
            seq,
            at: "2024-01-01T00:00:01.250Z".into(),
            offset_ms: 1250,
            frame_seq: 3,
            source,
            kind,
        };
        let events = vec![
            event(
                1,
                EventSource::Cua,
                EventKind::CuaAttached { attachment: 303 },
            ),
            event(
                2,
                EventSource::Cua,
                EventKind::CallStarted {
                    call: 1,
                    name: "click".into(),
                },
            ),
            event(
                3,
                EventSource::Cua,
                EventKind::CallFinished {
                    call: 1,
                    name: "click".into(),
                    outcome: CallOutcome::NoReply,
                    duration_ms: 12,
                },
            ),
            event(
                4,
                EventSource::Cua,
                EventKind::CuaDetached {
                    attachment: 303,
                    reason: "Cua attachment closed".into(),
                },
            ),
            event(5, EventSource::Cli, EventKind::SessionEnded),
        ];
        let json = serde_json::to_value(serde_json::json!({
            "header": header,
            "events": events,
        }))
        .unwrap();
        let common = r#""at":"2024-01-01T00:00:01.250Z","offset_ms":1250,"frame_seq":3"#;
        let expected: serde_json::Value = serde_json::from_str(&format!(
            r#"{{
            "header": {{"schema":1,"log_id":"00112233445566778899aabbccddeeff","session":"alpha","incarnation":7,"started_at":"2024-01-01T00:00:00.000Z"}},
            "events": [
              {{"seq":1,{common},"source":"cua","kind":"cua_attached","attachment":303}},
              {{"seq":2,{common},"source":"cua","kind":"call_started","call":1,"name":"click"}},
              {{"seq":3,{common},"source":"cua","kind":"call_finished","call":1,"name":"click","outcome":"no_reply","duration_ms":12}},
              {{"seq":4,{common},"source":"cua","kind":"cua_detached","attachment":303,"reason":"Cua attachment closed"}},
              {{"seq":5,{common},"source":"cli","kind":"session_ended"}}
            ]}}"#
        ))
        .unwrap();
        assert_eq!(json, expected);
        // Round trip.
        let back: Vec<SessionEvent> = serde_json::from_value(json["events"].clone()).unwrap();
        assert_eq!(back, events);
        let back: LogHeader = serde_json::from_value(json["header"].clone()).unwrap();
        assert_eq!(back, header);
    }

    /// Readers accept unknown fields and unknown kinds (additive schema).
    #[test]
    fn schema_1_tolerates_unknown_fields_and_kinds() {
        let event: SessionEvent = serde_json::from_str(
            r#"{"seq":9,"at":"x","offset_ms":1,"frame_seq":0,"source":"cli","kind":"call_started","call":2,"name":"key","future_field":true}"#,
        )
        .unwrap();
        assert_eq!(
            event.kind,
            EventKind::CallStarted {
                call: 2,
                name: "key".into()
            }
        );
        let event: SessionEvent = serde_json::from_str(
            r#"{"seq":10,"at":"x","offset_ms":1,"frame_seq":0,"source":"cua","kind":"frame_marked","extra":{"a":1}}"#,
        )
        .unwrap();
        assert_eq!(event.kind, EventKind::Unknown);
        let header: LogHeader = serde_json::from_str(
            r#"{"schema":1,"log_id":"ab","session":"s","incarnation":1,"started_at":"x","host_hint":"later"}"#,
        )
        .unwrap();
        assert_eq!(header.incarnation, 1);
    }

    /// Collects what it is given; refuses events once `capacity` is reached.
    struct TestSink {
        seen: Arc<Mutex<Vec<(LogHeader, SessionEvent)>>>,
        last: Arc<Mutex<Option<SessionEvent>>>,
        capacity: usize,
    }

    impl TestSink {
        #[allow(clippy::type_complexity, clippy::new_ret_no_self)]
        fn new(
            capacity: usize,
        ) -> (
            Box<dyn EventSink>,
            Arc<Mutex<Vec<(LogHeader, SessionEvent)>>>,
            Arc<Mutex<Option<SessionEvent>>>,
        ) {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let last = Arc::new(Mutex::new(None));
            (
                Box::new(TestSink {
                    seen: Arc::clone(&seen),
                    last: Arc::clone(&last),
                    capacity,
                }),
                seen,
                last,
            )
        }
    }

    impl EventSink for TestSink {
        fn on_event(&self, header: &LogHeader, event: &SessionEvent) -> bool {
            let mut seen = self.seen.lock().unwrap();
            if seen.len() >= self.capacity {
                return false;
            }
            seen.push((header.clone(), event.clone()));
            true
        }
        fn on_detach(&self, _header: &LogHeader, last: &SessionEvent) {
            *self.last.lock().unwrap() = Some(last.clone());
        }
    }

    fn started() -> EventKind {
        EventKind::RecordingStarted {
            trigger: RecordingTrigger::Cli,
        }
    }

    fn stopped() -> EventKind {
        EventKind::RecordingStopped {
            reason: StopReason::Requested,
        }
    }

    #[test]
    fn sink_receives_every_event_in_order_including_dropped_ones() {
        let (sink, seen, _) = TestSink::new(usize::MAX);
        let log = SessionEvents::new(&"alpha".parse().unwrap(), 3, None);
        log.attach_sink(sink, EventSource::Cli, started())
            .ok()
            .unwrap();
        let total = RING_CAPACITY as u64 + 25;
        for _ in 0..total {
            log.record(EventSource::Cua, EventKind::SessionEnded);
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len() as u64, total + 1);
        assert!(seen.iter().all(|(h, _)| h == log.header()));
        assert_eq!(seen[0].1.kind, started());
        assert!(seen[1..].iter().map(|(_, e)| e.seq).eq(1..=total));
    }

    /// Recording-only events reach the sink but never the ring, a ring
    /// sequence number or `latest`; ring sequence numbers stay contiguous.
    #[test]
    fn detached_events_never_enter_the_ring_or_move_latest() {
        let (sink, seen, last) = TestSink::new(usize::MAX);
        let log = SessionEvents::new(&"alpha".parse().unwrap(), 3, None);
        assert!(!log.record_detached(EventSource::Cli, started()));
        let watch = log.subscribe();
        let first = log
            .attach_sink(sink, EventSource::Cli, started())
            .ok()
            .unwrap();
        assert_eq!(first.seq, 0);
        assert!(log.has_sink());
        assert!(log.record_detached(
            EventSource::Viewer,
            EventKind::Annotation {
                text: "note".into()
            }
        ));
        assert_eq!(log.after(0).latest, 0);
        assert!(log.after(0).events.is_empty());
        assert_eq!(*watch.borrow(), 0);
        assert_eq!(log.record(EventSource::Cli, EventKind::SessionEnded), 1);
        assert!(log.record_detached(
            EventSource::Cli,
            EventKind::Annotation { text: "two".into() }
        ));
        assert_eq!(log.record(EventSource::Cli, EventKind::SessionEnded), 2);
        let page = log.after(0);
        assert_eq!(
            page.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(page
            .events
            .iter()
            .all(|e| e.kind == EventKind::SessionEnded));

        let stop = log.detach_sink(EventSource::Cli, stopped()).unwrap();
        assert!(!log.has_sink());
        assert!(log.detach_sink(EventSource::Cli, stopped()).is_none());
        assert!(!log.record_detached(EventSource::Cli, started()));
        let seen: Vec<EventKind> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.kind.clone())
            .collect();
        assert_eq!(seen.first(), Some(&started()));
        assert_eq!(seen.len(), 5);
        assert_eq!(
            last.lock().unwrap().as_ref().map(|e| &e.kind),
            Some(&stop.kind)
        );
    }

    #[test]
    fn only_one_sink_at_a_time() {
        let (a, _, _) = TestSink::new(usize::MAX);
        let (b, _, _) = TestSink::new(usize::MAX);
        let log = SessionEvents::new(&"alpha".parse().unwrap(), 3, None);
        assert!(log.attach_sink(a, EventSource::Cli, started()).is_ok());
        assert!(log.attach_sink(b, EventSource::Cli, started()).is_err());
    }

    /// A full sink refuses: the caller learns it (an annotation reports
    /// "busy"), and the final event still arrives through `on_detach`.
    #[test]
    fn a_full_sink_refuses_but_the_final_event_still_arrives() {
        let (sink, seen, last) = TestSink::new(2);
        let log = SessionEvents::new(&"alpha".parse().unwrap(), 3, None);
        log.attach_sink(sink, EventSource::Cli, started())
            .ok()
            .unwrap();
        log.record(EventSource::Cli, EventKind::SessionEnded);
        assert!(!log.record_detached(EventSource::Cli, EventKind::Annotation { text: "x".into() }));
        log.detach_sink(EventSource::Cli, stopped());
        assert_eq!(seen.lock().unwrap().len(), 2);
        assert_eq!(
            last.lock().unwrap().as_ref().map(|e| e.kind.clone()),
            Some(stopped())
        );
    }

    /// Pins the recording-only kinds and the new sources.
    #[test]
    fn recording_kinds_golden_json() {
        let cases: Vec<(EventSource, EventKind, &str)> = vec![
            (
                EventSource::Cli,
                EventKind::RecordingStarted {
                    trigger: RecordingTrigger::ConnectFlag,
                },
                r#""source":"cli","kind":"recording_started","trigger":"connect_flag""#,
            ),
            (
                EventSource::Viewer,
                EventKind::RecordingStopped {
                    reason: StopReason::Requested,
                },
                r#""source":"viewer","kind":"recording_stopped","reason":"requested""#,
            ),
            (
                EventSource::Daemon,
                EventKind::SessionClosed {
                    reason: CloseReason::IdleReap,
                },
                r#""source":"daemon","kind":"session_closed","reason":"idle_reap""#,
            ),
            (
                EventSource::Daemon,
                EventKind::DesktopResized {
                    width: 1280,
                    height: 720,
                },
                r#""source":"daemon","kind":"desktop_resized","width":1280,"height":720"#,
            ),
            (
                EventSource::Daemon,
                EventKind::SegmentStarted {
                    segment: 2,
                    video_offset_ms: 61000,
                    width: 8,
                    height: 6,
                },
                r#""source":"daemon","kind":"segment_started","segment":2,"video_offset_ms":61000,"width":8,"height":6"#,
            ),
            (
                EventSource::Daemon,
                EventKind::SegmentClosed {
                    segment: 2,
                    frames: 3,
                    bytes: 900,
                    duration_ms: 60000,
                    reason: SegmentCloseReason::Duration,
                },
                r#""source":"daemon","kind":"segment_closed","segment":2,"frames":3,"bytes":900,"duration_ms":60000,"reason":"duration""#,
            ),
            (
                EventSource::Daemon,
                EventKind::FramesDropped {
                    from_offset_ms: 10,
                    to_offset_ms: 900,
                    count: 3,
                },
                r#""source":"daemon","kind":"frames_dropped","from_offset_ms":10,"to_offset_ms":900,"count":3"#,
            ),
            (
                EventSource::Daemon,
                EventKind::EventsLost { count: 4 },
                r#""source":"daemon","kind":"events_lost","count":4"#,
            ),
            (
                EventSource::Daemon,
                EventKind::VideoStopped {
                    reason: VideoStopReason::SizeCap,
                },
                r#""source":"daemon","kind":"video_stopped","reason":"size_cap""#,
            ),
            (
                EventSource::Cli,
                EventKind::Annotation {
                    text: "look".into(),
                },
                r#""source":"cli","kind":"annotation","text":"look""#,
            ),
        ];
        for (source, kind, fields) in cases {
            let event = SessionEvent {
                seq: 1,
                at: "2024-01-01T00:00:01.250Z".into(),
                offset_ms: 1250,
                frame_seq: 3,
                source,
                kind,
            };
            let expected: serde_json::Value = serde_json::from_str(&format!(
                r#"{{"seq":1,"at":"2024-01-01T00:00:01.250Z","offset_ms":1250,"frame_seq":3,{fields}}}"#
            ))
            .unwrap();
            assert_eq!(serde_json::to_value(&event).unwrap(), expected);
            let back: SessionEvent = serde_json::from_value(expected).unwrap();
            assert_eq!(back, event);
        }
    }

    fn kinds(log: &SessionEvents) -> Vec<EventKind> {
        log.after(0).events.into_iter().map(|e| e.kind).collect()
    }

    fn finished(log: &SessionEvents) -> Vec<(u64, String, CallOutcome)> {
        kinds(log)
            .into_iter()
            .filter_map(|k| match k {
                EventKind::CallFinished {
                    call,
                    name,
                    outcome,
                    ..
                } => Some((call, name, outcome)),
                _ => None,
            })
            .collect()
    }

    fn call(id: serde_json::Value, name: &str) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":{}}})
    }

    #[test]
    fn tracker_pairs_calls_with_ok_and_error_answers() {
        let log = Arc::new(log());
        let mut t = CuaCallTracker::new(Arc::clone(&log));
        t.observe_request(&call(1.into(), "click"));
        t.observe_request(&call("b".into(), "type_text"));
        t.observe_request(&call(3.into(), "screenshot"));
        // Out of order; an isError result and a JSON-RPC error.
        t.observe_response(
            &serde_json::json!({"jsonrpc":"2.0","id":"b","result":{"isError":true,"content":[]}}),
        );
        t.observe_response(
            &serde_json::json!({"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"no"}}),
        );
        t.observe_response(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"content":[]}}));
        // Unmatched and repeated answers are ignored.
        t.observe_response(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}}));
        t.observe_response(&serde_json::json!({"jsonrpc":"2.0","id":99,"result":{}}));
        assert_eq!(
            finished(&log),
            vec![
                (2, "type_text".into(), CallOutcome::Error),
                (3, "screenshot".into(), CallOutcome::Error),
                (1, "click".into(), CallOutcome::Ok),
            ]
        );
        t.close();
        assert_eq!(finished(&log).len(), 3);
    }

    #[test]
    fn tracker_ignores_everything_but_client_tool_calls() {
        let log = Arc::new(log());
        let mut t = CuaCallTracker::new(Arc::clone(&log));
        for message in [
            serde_json::json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"click"}}),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            serde_json::json!([call(2.into(), "click")]),
            serde_json::json!("tools/call"),
            serde_json::json!({"jsonrpc":"2.0","id":null,"method":"tools/call","params":{"name":"click"}}),
        ] {
            t.observe_request(&message);
        }
        // A server-to-client request is not an answer.
        t.observe_response(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sampling/createMessage","result":{}}));
        t.observe_response(&serde_json::json!([{"jsonrpc":"2.0","id":1,"result":{}}]));
        t.close();
        assert!(kinds(&log).is_empty());
    }

    #[test]
    fn tracker_replaces_bad_names() {
        let log = Arc::new(log());
        let mut t = CuaCallTracker::new(Arc::clone(&log));
        let long = "a".repeat(65);
        t.observe_request(&call(1.into(), "ok.name-1_x"));
        t.observe_request(&call(2.into(), "has space"));
        t.observe_request(&call(3.into(), ""));
        t.observe_request(&call(4.into(), &long));
        t.observe_request(&call(5.into(), "caf\u{e9}"));
        t.observe_request(&serde_json::json!({"id":6,"method":"tools/call","params":{"name":7}}));
        t.observe_request(&serde_json::json!({"id":7,"method":"tools/call"}));
        let names: Vec<String> = kinds(&log)
            .into_iter()
            .filter_map(|k| match k {
                EventKind::CallStarted { name, .. } => Some(name),
                _ => None,
            })
            .collect();
        let mut expected = vec!["ok.name-1_x".to_owned()];
        expected.extend(std::iter::repeat_n(INVALID_NAME.to_owned(), 6));
        assert_eq!(names, expected);
        assert!(valid_tool_name(&"a".repeat(64)));
    }

    #[test]
    fn tracker_finishes_duplicates_overflow_and_leftovers_as_no_reply() {
        let log = Arc::new(log());
        let mut t = CuaCallTracker::new(Arc::clone(&log));
        t.observe_request(&call(1.into(), "first"));
        t.observe_request(&call(1.into(), "again"));
        assert_eq!(
            finished(&log),
            vec![(1, "first".into(), CallOutcome::NoReply)]
        );
        t.observe_response(&serde_json::json!({"id":1,"result":{}}));
        assert_eq!(finished(&log)[1], (2, "again".into(), CallOutcome::Ok));

        for id in 0..CUA_PENDING_CAP as u64 {
            t.observe_request(&call((100 + id).into(), "fill"));
        }
        assert_eq!(finished(&log).len(), 2);
        t.observe_request(&call(1000.into(), "over"));
        let done = finished(&log);
        assert_eq!(done.len(), 3);
        assert_eq!(done[2], (3, "fill".into(), CallOutcome::NoReply));
        // The evicted id no longer matches.
        t.observe_response(&serde_json::json!({"id":100,"result":{}}));
        assert_eq!(finished(&log).len(), 3);

        drop(t);
        let done = finished(&log);
        assert_eq!(done.len(), 3 + CUA_PENDING_CAP);
        assert!(done[3..]
            .iter()
            .all(|(_, _, outcome)| *outcome == CallOutcome::NoReply));
    }

    #[test]
    fn cli_call_records_outcome_or_no_reply_when_dropped() {
        let log = Arc::new(log());
        CliCall::start(Arc::clone(&log), "screenshot").finish(true);
        CliCall::start(Arc::clone(&log), "put").finish(false);
        drop(CliCall::start(Arc::clone(&log), "key"));
        assert_eq!(
            finished(&log),
            vec![
                (1, "screenshot".into(), CallOutcome::Ok),
                (2, "put".into(), CallOutcome::Error),
                (3, "key".into(), CallOutcome::NoReply),
            ]
        );
        assert!(log
            .after(0)
            .events
            .iter()
            .all(|e| e.source == EventSource::Cli));
    }

    struct EndingFrames(watch::Sender<rdpilot::FrameStatus>);

    impl ViewFrameSource for EndingFrames {
        fn status(&self) -> rdpilot::FrameStatus {
            *self.0.borrow()
        }
        fn changed(&self, after_seq: u64) -> crate::seams::BoxFuture<'_, rdpilot::FrameStatus> {
            let mut rx = self.0.subscribe();
            Box::pin(async move {
                let status = rx
                    .wait_for(|s| s.seq > after_seq || s.ended)
                    .await
                    .map(|s| *s);
                status.unwrap_or(rdpilot::FrameStatus {
                    seq: after_seq,
                    ended: true,
                })
            })
        }
        fn capture(&self) -> Option<(u64, rdpilot::Screenshot)> {
            None
        }
    }

    fn ended_count(log: &SessionEvents) -> usize {
        kinds(log)
            .iter()
            .filter(|k| **k == EventKind::SessionEnded)
            .count()
    }

    #[tokio::test]
    async fn watcher_records_session_ended_once() {
        let (tx, _) = watch::channel(rdpilot::FrameStatus::default());
        let frames = Arc::new(EndingFrames(tx));
        let log = Arc::new(SessionEvents::new(
            &"alpha".parse().unwrap(),
            1,
            Some(Arc::clone(&frames) as Arc<dyn ViewFrameSource>),
        ));
        let control = Arc::new(crate::control::SessionControl::new(
            "alpha",
            1,
            Arc::clone(&log),
            None,
            None,
        ));
        let _watch = watch_session_end(frames.clone(), Arc::clone(&log), control);
        frames.0.send_modify(|s| s.seq = 1);
        frames.0.send_modify(|s| s.seq = 2);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(ended_count(&log), 0);
        frames.0.send_modify(|s| s.ended = true);
        tokio::time::sleep(Duration::from_millis(30)).await;
        frames.0.send_modify(|s| s.seq = 3);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(ended_count(&log), 1);
        let ended = log.after(0).events.pop().unwrap();
        assert_eq!(ended.source, EventSource::Cli);
        assert_eq!(ended.frame_seq, 2);
    }

    #[tokio::test]
    async fn dropped_watcher_records_nothing() {
        let (tx, _) = watch::channel(rdpilot::FrameStatus::default());
        let frames = Arc::new(EndingFrames(tx));
        let log = Arc::new(log());
        let control = Arc::new(crate::control::SessionControl::new(
            "alpha",
            1,
            Arc::clone(&log),
            None,
            None,
        ));
        let watch = watch_session_end(frames.clone(), Arc::clone(&log), control);
        tokio::task::yield_now().await;
        drop(watch);
        tokio::time::sleep(Duration::from_millis(30)).await;
        frames.0.send_modify(|s| s.ended = true);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(ended_count(&log), 0);
        // The task released its references.
        assert_eq!(Arc::strong_count(&log), 1);
        assert_eq!(Arc::strong_count(&frames), 1);
    }

    /// Pins the control kinds of schema 1 (additive): controller
    /// descriptors and reasons only.
    #[test]
    fn control_kinds_golden_json() {
        use crate::control::{ControllerRef, EndReason};
        let human = || ControllerRef::Human {
            address: "100.64.0.9".into(),
        };
        let event = |seq, source, kind| SessionEvent {
            seq,
            at: "2024-01-01T00:00:01.250Z".into(),
            offset_ms: 1250,
            frame_seq: 3,
            source,
            kind,
        };
        let events = vec![
            event(
                1,
                EventSource::Viewer,
                EventKind::ControlTaken { by: human() },
            ),
            event(
                2,
                EventSource::Cua,
                EventKind::ControlTakenOver {
                    from: human(),
                    by: ControllerRef::Agent,
                },
            ),
            event(
                3,
                EventSource::Viewer,
                EventKind::ControlReleased { from: human() },
            ),
            event(
                4,
                EventSource::Daemon,
                EventKind::ControlEnded {
                    from: human(),
                    reason: EndReason::IdleTimeout,
                },
            ),
        ];
        let json = serde_json::to_value(&events).unwrap();
        let common = r#""at":"2024-01-01T00:00:01.250Z","offset_ms":1250,"frame_seq":3"#;
        let h = r#"{"kind":"human","address":"100.64.0.9"}"#;
        let expected: serde_json::Value = serde_json::from_str(&format!(
            r#"[
              {{"seq":1,{common},"source":"viewer","kind":"control_taken","by":{h}}},
              {{"seq":2,{common},"source":"cua","kind":"control_taken_over","from":{h},"by":{{"kind":"agent"}}}},
              {{"seq":3,{common},"source":"viewer","kind":"control_released","from":{h}}},
              {{"seq":4,{common},"source":"daemon","kind":"control_ended","from":{h},"reason":"idle_timeout"}}
            ]"#
        ))
        .unwrap();
        assert_eq!(json, expected);
        let back: Vec<SessionEvent> = serde_json::from_value(json).unwrap();
        assert_eq!(back, events);
        for reason in [
            EndReason::HeartbeatLost,
            EndReason::ViewerStopped,
            EndReason::SessionEnded,
            EndReason::DaemonStopped,
        ] {
            let text = serde_json::to_string(&reason).unwrap();
            assert!(text
                .chars()
                .all(|c| c == '"' || c == '_' || c.is_ascii_lowercase()));
        }
    }
}
