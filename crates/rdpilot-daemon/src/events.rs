//! Per-session event log: what an agent or the CLI did in one session.
//!
//! - One [`SessionEvents`] per live session incarnation, created at connect
//!   and dropped with the registry entry. It keeps the newest
//!   [`RING_CAPACITY`] events in memory; nothing is written to disk.
//! - Records carry names, outcomes and timings only. Argument values, typed
//!   text, file paths, results, images, error text and JSON-RPC ids never
//!   enter a record.
//! - Recording takes only this log's own mutex for a push and a clone. It
//!   never awaits, never takes the per-session mutex and never changes the
//!   session's activity time.
//! - The record types are the stable, additive schema 1: the in-memory ring,
//!   the viewer's HTTP body and any future durable log use them unchanged.
//!   Readers ignore unknown fields and unknown kinds.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rdpilot_ipc::SessionId;
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

/// Receives every event in sequence order, including events later dropped
/// from the in-memory ring. Called under the log's mutex: it must not block
/// (use a bounded `try_send`).
pub trait EventSink: Send + Sync {
    fn on_event(&self, header: &LogHeader, event: &SessionEvent);
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
}

/// One session incarnation's event log.
pub struct SessionEvents {
    header: LogHeader,
    clock: SessionClock,
    frame: Option<Arc<dyn ViewFrameSource>>,
    sink: Option<Arc<dyn EventSink>>,
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
        sink: Option<Arc<dyn EventSink>>,
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
            sink,
            ring: Mutex::new(Ring {
                events: VecDeque::with_capacity(RING_CAPACITY),
                latest: 0,
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

    /// Append an event and return its sequence number.
    pub fn record(&self, source: EventSource, kind: EventKind) -> u64 {
        let frame_seq = self.frame.as_ref().map_or(0, |frame| frame.status().seq);
        let mut ring = self.ring();
        let now = Instant::now();
        ring.latest += 1;
        let event = SessionEvent {
            seq: ring.latest,
            at: iso8601_millis_from_system_time(self.clock.wall(now)),
            offset_ms: self.clock.offset_ms(now),
            frame_seq,
            source,
            kind,
        };
        if ring.events.len() == RING_CAPACITY {
            ring.events.pop_front();
        }
        ring.events.push_back(event);
        if let (Some(sink), Some(event)) = (&self.sink, ring.events.back()) {
            sink.on_event(&self.header, event);
        }
        let seq = ring.latest;
        self.latest.send_replace(seq);
        seq
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
        SessionEvents::new(&"alpha".parse().unwrap(), 7, None, None)
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

    struct TestSink(Mutex<Vec<(LogHeader, SessionEvent)>>);

    impl EventSink for TestSink {
        fn on_event(&self, header: &LogHeader, event: &SessionEvent) {
            self.0.lock().unwrap().push((header.clone(), event.clone()));
        }
    }

    #[test]
    fn sink_receives_every_event_in_order_including_dropped_ones() {
        let sink = Arc::new(TestSink(Mutex::new(Vec::new())));
        let log = SessionEvents::new(
            &"alpha".parse().unwrap(),
            3,
            None,
            Some(Arc::clone(&sink) as Arc<dyn EventSink>),
        );
        let total = RING_CAPACITY as u64 + 25;
        for _ in 0..total {
            log.record(EventSource::Cua, EventKind::SessionEnded);
        }
        let seen = sink.0.lock().unwrap();
        assert_eq!(seen.len() as u64, total);
        assert!(seen.iter().all(|(h, _)| h == log.header()));
        assert!(seen.iter().map(|(_, e)| e.seq).eq(1..=total));
    }
}
