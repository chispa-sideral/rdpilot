//! The recording's event log: a bounded queue that the session's event log
//! feeds while the recording is attached, and the JSON Lines writer that
//! drains it on the recorder thread.
//!
//! - [`QueueSink`] is the [`EventSink`]: it runs under the session log's
//!   mutex, never blocks and never allocates beyond one queue slot. When
//!   the queue is full it counts the event as lost and refuses it; the
//!   recorder writes `events_lost` with the count at its next pass.
//! - The final event (`recording_stopped`) travels in its own slot through
//!   [`EventSink::on_detach`], so it is never lost.
//! - [`EventLogWriter`] rewrites `seq` (from 1 per recording) and
//!   `offset_ms` (milliseconds on the recording timeline, which is the
//!   video's clock, never decreasing) and keeps `at`, `frame_seq`, `source`
//!   and the kind fields. One line per event, written and flushed at once;
//!   a reader drops a partial last line.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::Thread;

use crate::events::{EventSink, LogHeader, SessionEvent};

use super::store;

/// Events queued for the recorder before new ones are counted as lost.
pub(crate) const QUEUE_CAPACITY: usize = 1024;

/// Wakes the recorder thread. Set once, right after the thread starts.
#[derive(Default)]
pub(crate) struct Waker(OnceLock<Thread>);

impl Waker {
    pub(crate) fn set(&self, thread: Thread) {
        let _ = self.0.set(thread);
    }

    pub(crate) fn wake(&self) {
        if let Some(thread) = self.0.get() {
            thread.unpark();
        }
    }
}

#[derive(Default)]
struct QueueState {
    events: VecDeque<SessionEvent>,
    lost: u64,
    last: Option<SessionEvent>,
    closed: bool,
}

/// What the recorder takes from the queue in one pass.
#[derive(Debug, Default)]
pub(crate) struct Drained {
    pub(crate) events: Vec<SessionEvent>,
    /// Events refused since the previous pass.
    pub(crate) lost: u64,
    /// The final event, once the sink was detached.
    pub(crate) last: Option<SessionEvent>,
    /// The sink is gone: detached, or dropped with its session log.
    pub(crate) closed: bool,
}

/// The queue between the session's event log and the recorder thread.
pub(crate) struct EventQueue {
    state: Mutex<QueueState>,
    waker: Arc<Waker>,
}

impl EventQueue {
    pub(crate) fn new(waker: Arc<Waker>) -> Arc<Self> {
        Arc::new(EventQueue {
            state: Mutex::new(QueueState::default()),
            waker,
        })
    }

    fn state(&self) -> MutexGuard<'_, QueueState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Take everything queued so far.
    pub(crate) fn drain(&self) -> Drained {
        let mut state = self.state();
        Drained {
            events: state.events.drain(..).collect(),
            lost: std::mem::take(&mut state.lost),
            last: state.last.take(),
            closed: state.closed,
        }
    }
}

/// The [`EventSink`] a recording attaches to its session's event log.
pub(crate) struct QueueSink(pub(crate) Arc<EventQueue>);

impl EventSink for QueueSink {
    fn on_event(&self, _header: &LogHeader, event: &SessionEvent) -> bool {
        let accepted = {
            let mut state = self.0.state();
            if state.closed {
                false
            } else if state.events.len() >= QUEUE_CAPACITY {
                state.lost += 1;
                false
            } else {
                state.events.push_back(event.clone());
                true
            }
        };
        self.0.waker.wake();
        accepted
    }

    fn on_detach(&self, _header: &LogHeader, last: &SessionEvent) {
        {
            let mut state = self.0.state();
            state.last = Some(last.clone());
            state.closed = true;
        }
        self.0.waker.wake();
    }
}

impl Drop for QueueSink {
    fn drop(&mut self) {
        // Dropped without a detach (the session log went away): the
        // recorder finishes the recording on its own.
        self.0.state().closed = true;
        self.0.waker.wake();
    }
}

/// Appends events to `events.jsonl`, one line each.
pub(crate) struct EventLogWriter {
    file: File,
    next_seq: u64,
    start_offset: u64,
    last_offset: u64,
    bytes: u64,
}

impl EventLogWriter {
    /// Create `events.jsonl` in `dir`. `start_offset` is the session-clock
    /// offset where the recording timeline starts.
    pub(crate) fn create(dir: &Path, start_offset: u64) -> io::Result<Self> {
        Ok(EventLogWriter {
            file: store::open_private_file(&dir.join(store::EVENTS), true, true)?,
            next_seq: 1,
            start_offset,
            last_offset: 0,
            bytes: 0,
        })
    }

    /// The recording-timeline offset of a session-clock offset.
    pub(crate) fn timeline(&self, session_offset: u64) -> u64 {
        session_offset.saturating_sub(self.start_offset)
    }

    /// Bytes written so far.
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Write one event with the next sequence number and its offset on the
    /// recording timeline (never lower than the previous event's).
    pub(crate) fn write(&mut self, mut event: SessionEvent) -> io::Result<()> {
        event.seq = self.next_seq;
        event.offset_ms = self.timeline(event.offset_ms).max(self.last_offset);
        let mut line = serde_json::to_vec(&event).map_err(io::Error::other)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        self.next_seq += 1;
        self.last_offset = event.offset_ms;
        self.bytes += line.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::events::{EventKind, EventSource, RecordingTrigger, SessionEvents, StopReason};
    use crate::recording::store::tests::temp_root;

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

    fn attached() -> (Arc<SessionEvents>, Arc<EventQueue>, SessionEvent) {
        let log = Arc::new(SessionEvents::new(&"alpha".parse().unwrap(), 1, None));
        let queue = EventQueue::new(Arc::new(Waker::default()));
        let first = log
            .attach_sink(
                Box::new(QueueSink(Arc::clone(&queue))),
                EventSource::Cli,
                started(),
            )
            .ok()
            .unwrap();
        (log, queue, first)
    }

    fn lines(dir: &Path) -> Vec<serde_json::Value> {
        store::read_events(dir)
    }

    /// With the recorder stalled, the queue fills and further events are
    /// counted exactly; recording never waits for the recorder.
    #[test]
    fn a_stalled_recorder_loses_counted_events_without_blocking_the_log() {
        let (log, queue, _) = attached();
        let mut times: Vec<Duration> = (0..(QUEUE_CAPACITY + 1000))
            .map(|_| {
                let t = Instant::now();
                log.record(EventSource::Cli, EventKind::SessionEnded);
                t.elapsed()
            })
            .collect();
        times.sort();
        // Under 10 ms each; the 99th percentile keeps a test thread
        // preempted by a busy host from failing it.
        let p99 = times[times.len() * 99 / 100];
        assert!(p99 < Duration::from_millis(10), "{p99:?}");
        // An annotation is refused while the queue is full: the caller
        // reports "busy" and nothing is written for it.
        assert!(!log.record_detached(EventSource::Cli, EventKind::Annotation { text: "x".into() }));
        log.detach_sink(EventSource::Cli, stopped());
        let drained = queue.drain();
        assert_eq!(drained.events.len(), QUEUE_CAPACITY);
        assert_eq!(drained.events[0].kind, started());
        assert_eq!(drained.lost, 1000 + 1 + 1);
        assert_eq!(drained.last.map(|e| e.kind), Some(stopped()));
        assert!(drained.closed);
        assert!(!drained
            .events
            .iter()
            .any(|e| matches!(e.kind, EventKind::Annotation { .. })));
    }

    #[test]
    fn writer_renumbers_and_moves_offsets_onto_the_recording_timeline() {
        let root = temp_root("log-writer");
        std::fs::create_dir_all(&root).unwrap();
        let (log, queue, first) = attached();
        std::thread::sleep(Duration::from_millis(20));
        log.record(EventSource::Cli, EventKind::SessionEnded);
        log.record_detached(
            EventSource::Viewer,
            EventKind::Annotation {
                text: "hello".into(),
            },
        );
        log.detach_sink(EventSource::Cli, stopped());
        let mut writer = EventLogWriter::create(&root, first.offset_ms).unwrap();
        let drained = queue.drain();
        for event in drained.events {
            writer.write(event).unwrap();
        }
        writer.write(drained.last.unwrap()).unwrap();
        let out = lines(&root);
        assert_eq!(out.len(), 4);
        let seqs: Vec<u64> = out.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, [1, 2, 3, 4]);
        assert_eq!(out[0]["kind"], "recording_started");
        assert_eq!(out[0]["offset_ms"], 0);
        assert!(out[1]["offset_ms"].as_u64().unwrap() >= 20);
        assert_eq!(out[2]["kind"], "annotation");
        assert_eq!(out[2]["source"], "viewer");
        assert_eq!(out[3]["kind"], "recording_stopped");
        assert!(writer.bytes() > 0);
        assert!(out
            .windows(2)
            .all(|w| w[0]["offset_ms"].as_u64() <= w[1]["offset_ms"].as_u64()));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Events from several threads, strip and recording-only alike, reach
    /// the file in one strict order: sequence numbers without gaps, times
    /// never decreasing, each thread's events in the order it made them.
    #[test]
    fn concurrent_producers_give_one_strict_file_order() {
        let root = temp_root("log-order");
        std::fs::create_dir_all(&root).unwrap();
        let (log, queue, first) = attached();
        let mut writer = EventLogWriter::create(&root, first.offset_ms).unwrap();
        let producers: Vec<_> = (0..4_u64)
            .map(|t| {
                let log = Arc::clone(&log);
                std::thread::spawn(move || {
                    for i in 0..200_u64 {
                        if i % 2 == 0 {
                            log.record(
                                EventSource::Cua,
                                EventKind::CuaAttached {
                                    attachment: t * 1000 + i,
                                },
                            );
                        } else {
                            while !log.record_detached(
                                EventSource::Cli,
                                EventKind::EventsLost {
                                    count: t * 1000 + i,
                                },
                            ) {
                                std::thread::yield_now();
                            }
                        }
                    }
                })
            })
            .collect();
        let mut done = false;
        let mut seen_lost = 0;
        while !done {
            done = producers.iter().all(std::thread::JoinHandle::is_finished);
            let drained = queue.drain();
            seen_lost += drained.lost;
            for event in drained.events {
                writer.write(event).unwrap();
            }
        }
        for p in producers {
            p.join().unwrap();
        }
        for event in queue.drain().events {
            writer.write(event).unwrap();
        }
        assert_eq!(seen_lost + queue.drain().lost, 0);
        let out = lines(&root);
        assert_eq!(out.len(), 1 + 800);
        assert!(out
            .iter()
            .enumerate()
            .all(|(i, e)| e["seq"].as_u64() == Some(i as u64 + 1)));
        assert!(out
            .windows(2)
            .all(|w| w[0]["at"].as_str() <= w[1]["at"].as_str()
                && w[0]["offset_ms"].as_u64() <= w[1]["offset_ms"].as_u64()));
        for t in 0..4_u64 {
            let mine: Vec<u64> = out
                .iter()
                .filter_map(|e| e["attachment"].as_u64().or(e["count"].as_u64()))
                .filter(|v| v / 1000 == t)
                .collect();
            assert_eq!(mine.len(), 200);
            assert!(mine.windows(2).all(|w| w[0] < w[1]), "thread {t}");
        }
        // The ring saw only the strip events, numbered without gaps.
        assert_eq!(log.after(0).latest, 400);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_sink_dropped_with_its_log_closes_the_queue() {
        let (log, queue, _) = attached();
        drop(log);
        let drained = queue.drain();
        assert!(drained.closed);
        assert!(drained.last.is_none());
    }
}
