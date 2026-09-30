//! The recorder: one thread per active recording that writes the event log,
//! encodes captured frames into WebM segments and keeps the manifest up to
//! date. All recording file I/O happens here.
//!
//! [`Recorder`] is the recording's state; its methods take the current time
//! on the recording timeline, so tests drive it with a fake clock.
//! [`spawn`] runs it on a thread with the real clock.
//!
//! - Segments: a segment opens with a new encoder and a key frame at the
//!   first frame taken after the previous close. It closes at a desktop
//!   resize, [`SEGMENT_MS`] after its first frame (even with no further
//!   change: its last frame is extended to the close time), at stop and at
//!   the size cap. Closing flushes the encoder, so every frame it still
//!   held reaches the file. Each packet is matched to its frame through
//!   `input_frameno` and written with that frame's capture time.
//! - An unchanged display adds no frame and no segment.
//! - Frames the capture stage replaced before the encoder took them are
//!   reported as one `frames_dropped` event per backlog period.
//! - Failures never reach the session: an encoder or video write error, or
//!   the size cap, stops the video (`video_stopped`) and events continue; an
//!   event log write error stops the recording. Error messages carry the
//!   error kind only, never event contents.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::capture::{Mailbox, Take, Timeline};
use super::encoder::{EncoderFactory, Packet, VideoEncoder};
use super::i420;
use super::log::{Drained, EventLogWriter, EventQueue, Waker};
use super::manifest::{Manifest, SegmentEntry, Stats};
use super::store::{self, Store};
use super::webm::SegmentWriter;
use crate::events::{
    EventKind, EventSource, SegmentCloseReason, SessionEvent, StopReason, VideoStopReason,
};
use crate::registry::iso8601_millis_from_system_time;

/// Longest segment, on the recording timeline.
pub(crate) const SEGMENT_MS: u64 = 60_000;

/// Most frames an encoder may hold before its output is treated as broken.
const MAX_PENDING: usize = 8;

/// Fixed inputs of one recording.
pub(crate) struct RecorderSetup {
    pub(crate) dir: PathBuf,
    pub(crate) manifest: Manifest,
    pub(crate) timeline: Timeline,
    pub(crate) budget_bytes: u64,
    pub(crate) segment_ms: u64,
    pub(crate) factory: Arc<dyn EncoderFactory>,
}

struct Pending {
    input_frameno: u64,
    offset_ms: u64,
}

struct Segment {
    number: u32,
    encoder: Box<dyn VideoEncoder>,
    writer: SegmentWriter,
    first_offset: u64,
    last_offset: u64,
    width: u32,
    height: u32,
    pending: VecDeque<Pending>,
    next_frameno: u64,
    largest_packet: u64,
}

struct Gap {
    from: u64,
    to: u64,
    count: u64,
}

#[derive(Default)]
struct Figures {
    encode_calls_ms: Vec<f64>,
    busy_ms: f64,
    take_waits_ms: Vec<f64>,
    frames_encoded: u64,
    frames_dropped: u64,
    gap_periods: u64,
    held_at_close_max: u64,
    flush_ms_max: f64,
    events_lost: u64,
}

fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * (sorted.len() - 1) as f64).round();
    sorted[(rank as usize).min(sorted.len() - 1)]
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Lower this thread's scheduling priority (nice 10), so session, IPC and
/// viewer threads win under CPU contention. Threads created afterwards on
/// this thread (the encoder's pool) inherit it.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // Localized: one libc call with plain integer arguments.
pub(crate) fn lower_priority() {
    // SAFETY: setpriority takes integers only; on Linux, PRIO_PROCESS with
    // `who` 0 changes the calling thread's nice value. A failure only
    // leaves the priority unchanged.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// No priority change on other platforms.
#[cfg(not(target_os = "linux"))]
pub(crate) fn lower_priority() {}

/// One active recording.
pub(crate) struct Recorder {
    setup: RecorderSetup,
    manifest: Manifest,
    log: EventLogWriter,
    segment: Option<Segment>,
    next_segment: u32,
    closed_bytes: u64,
    video_stopped: bool,
    last_size: Option<(u32, u32)>,
    last_frame_seq: u64,
    gap: Option<Gap>,
    figures: Figures,
    failed: bool,
}

impl Recorder {
    /// Start writing: the event log is created in `setup.dir`, whose
    /// manifest the caller has written.
    ///
    /// # Errors
    ///
    /// The event log cannot be created.
    pub(crate) fn new(setup: RecorderSetup) -> io::Result<Self> {
        let log = EventLogWriter::create(&setup.dir, setup.timeline.start_offset)?;
        Ok(Recorder {
            manifest: setup.manifest.clone(),
            setup,
            log,
            segment: None,
            next_segment: 1,
            closed_bytes: 0,
            video_stopped: false,
            last_size: None,
            last_frame_seq: 0,
            gap: None,
            figures: Figures::default(),
            failed: false,
        })
    }

    /// The recording id.
    pub(crate) fn id(&self) -> &str {
        &self.manifest.id
    }

    /// Whether the recording could not continue (its event log failed).
    pub(crate) fn failed(&self) -> bool {
        self.failed
    }

    /// An event made by the recorder at `now` (recording timeline).
    fn event(&self, now: u64, kind: EventKind) -> SessionEvent {
        let session_offset = self.setup.timeline.start_offset + now;
        let wall = self.setup.timeline.clock.start_wall + Duration::from_millis(session_offset);
        SessionEvent {
            seq: 0,
            at: iso8601_millis_from_system_time(wall),
            offset_ms: session_offset,
            frame_seq: self.last_frame_seq,
            source: EventSource::Daemon,
            kind,
        }
    }

    fn write(&mut self, event: SessionEvent) {
        if self.failed {
            return;
        }
        if let Err(e) = self.log.write(event) {
            self.failed = true;
            eprintln!(
                "rdpilot-daemon: recording {}: event log write failed ({:?}); recording stopped",
                self.manifest.id,
                e.kind()
            );
        }
    }

    fn write_own(&mut self, now: u64, kind: EventKind) {
        let event = self.event(now, kind);
        self.write(event);
    }

    /// Write queued events, and `events_lost` for refused ones.
    pub(crate) fn handle_events(&mut self, events: Vec<SessionEvent>, lost: u64, now: u64) {
        for event in events {
            self.write(event);
        }
        if lost > 0 {
            self.figures.events_lost += lost;
            self.write_own(now, EventKind::EventsLost { count: lost });
        }
    }

    fn close_gap(&mut self, now: u64) {
        if let Some(gap) = self.gap.take() {
            self.write_own(
                now,
                EventKind::FramesDropped {
                    from_offset_ms: gap.from,
                    to_offset_ms: gap.to,
                    count: gap.count,
                },
            );
        }
    }

    fn stop_video(&mut self, now: u64, reason: VideoStopReason) {
        if self.video_stopped {
            return;
        }
        self.video_stopped = true;
        eprintln!(
            "rdpilot-daemon: recording {}: video stopped ({reason:?}); events continue",
            self.manifest.id
        );
        self.write_own(now, EventKind::VideoStopped { reason });
    }

    /// Bytes of the recording so far, counting frames the encoder holds as
    /// the largest packet seen.
    fn bytes_with_pending(&self) -> u64 {
        let open = self.segment.as_ref().map_or(0, |s| {
            s.writer.bytes() + s.pending.len() as u64 * s.largest_packet
        });
        self.closed_bytes + open + self.log.bytes()
    }

    /// Encode one taken frame at `now`.
    pub(crate) fn handle_frame(&mut self, take: Take, now: u64) {
        let frame = take.frame;
        self.figures
            .take_waits_ms
            .push(frame.captured_at.elapsed().as_secs_f64() * 1000.0);
        if take.dropped > 0 {
            self.figures.frames_dropped += take.dropped;
            let from = take.first_dropped_ms.unwrap_or(frame.offset_ms);
            match &mut self.gap {
                Some(gap) => {
                    gap.to = frame.offset_ms;
                    gap.count += take.dropped;
                }
                None => {
                    self.figures.gap_periods += 1;
                    self.gap = Some(Gap {
                        from,
                        to: frame.offset_ms,
                        count: take.dropped,
                    });
                }
            }
        } else {
            self.close_gap(now);
        }
        self.last_frame_seq = frame.frame_seq;
        let size = (frame.width, frame.height);
        if self.last_size.is_some_and(|s| s != size) {
            if self.segment.is_some() {
                self.close_segment(frame.offset_ms, SegmentCloseReason::Resize);
            }
            self.write_own(
                now,
                EventKind::DesktopResized {
                    width: frame.width,
                    height: frame.height,
                },
            );
        }
        self.last_size = Some(size);
        if self.video_stopped || self.failed {
            return;
        }
        if self.segment.is_none() && !self.open_segment(&frame.offset_ms, size, now) {
            return;
        }
        if self.bytes_with_pending() >= self.setup.budget_bytes {
            self.close_segment(now.max(frame.offset_ms), SegmentCloseReason::SizeCap);
            self.stop_video(now, VideoStopReason::SizeCap);
            return;
        }
        let picture = i420::from_rgba(&frame.rgba, frame.width as usize, frame.height as usize);
        let started = Instant::now();
        let result = self.send(&picture, frame.offset_ms);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        self.figures.encode_calls_ms.push(ms);
        self.figures.busy_ms += ms;
        if let Err(reason) = result {
            self.fail_video(now, reason);
        }
    }

    fn open_segment(&mut self, offset: &u64, (width, height): (u32, u32), now: u64) -> bool {
        lower_priority();
        let number = self.next_segment;
        let encoder = match self.setup.factory.create(width as usize, height as usize) {
            Ok(encoder) => encoder,
            Err(_) => {
                self.stop_video(now, VideoStopReason::EncoderError);
                return false;
            }
        };
        let writer = match SegmentWriter::create(
            &self.setup.dir.join(store::SEGMENTS),
            number,
            width,
            height,
            encoder.codec_config(),
        ) {
            Ok(writer) => writer,
            Err(_) => {
                self.stop_video(now, VideoStopReason::WriteError);
                return false;
            }
        };
        self.next_segment += 1;
        self.segment = Some(Segment {
            number,
            encoder,
            writer,
            first_offset: *offset,
            last_offset: *offset,
            width,
            height,
            pending: VecDeque::new(),
            next_frameno: 0,
            largest_packet: 0,
        });
        self.write_own(
            now,
            EventKind::SegmentStarted {
                segment: number,
                video_offset_ms: *offset,
                width,
                height,
            },
        );
        true
    }

    /// Send one picture and mux whatever packets are ready.
    fn send(&mut self, picture: &i420::I420, offset: u64) -> Result<(), VideoStopReason> {
        let Some(segment) = self.segment.as_mut() else {
            return Ok(());
        };
        let key = segment.next_frameno == 0;
        segment
            .encoder
            .send(picture, key)
            .map_err(|_| VideoStopReason::EncoderError)?;
        segment.pending.push_back(Pending {
            input_frameno: segment.next_frameno,
            offset_ms: offset,
        });
        segment.next_frameno += 1;
        segment.last_offset = offset;
        let packets = segment
            .encoder
            .receive()
            .map_err(|_| VideoStopReason::EncoderError)?;
        let muxed = mux(segment, packets)?;
        self.figures.frames_encoded += muxed;
        if segment.pending.len() > MAX_PENDING {
            return Err(VideoStopReason::EncoderError);
        }
        Ok(())
    }

    fn fail_video(&mut self, now: u64, reason: VideoStopReason) {
        if let Some(segment) = self.segment.take() {
            // An unfinished segment is not playable: remove it.
            let number = segment.number;
            drop(segment);
            let _ = std::fs::remove_file(
                self.setup
                    .dir
                    .join(store::SEGMENTS)
                    .join(format!("{}.part", store::segment_file(number))),
            );
        }
        self.close_gap(now);
        self.stop_video(now, reason);
    }

    /// Close the open segment at `end` (recording timeline).
    fn close_segment(&mut self, end: u64, reason: SegmentCloseReason) {
        let Some(mut segment) = self.segment.take() else {
            return;
        };
        let held = segment.pending.len() as u64;
        self.figures.held_at_close_max = self.figures.held_at_close_max.max(held);
        let started = Instant::now();
        let flushed = segment
            .encoder
            .flush()
            .map_err(|_| VideoStopReason::EncoderError)
            .and_then(|packets| mux(&mut segment, packets));
        let flush_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.figures.flush_ms_max = self.figures.flush_ms_max.max(flush_ms);
        self.figures.busy_ms += flush_ms;
        let muxed = match flushed {
            Ok(n) if segment.pending.is_empty() => n,
            Ok(_) | Err(_) => {
                self.segment = Some(segment);
                self.fail_video(end, VideoStopReason::EncoderError);
                return;
            }
        };
        self.figures.frames_encoded += muxed;
        let end = end.max(segment.last_offset + 1);
        let frames = segment.writer.frames();
        let (number, first, width, height) = (
            segment.number,
            segment.first_offset,
            segment.width,
            segment.height,
        );
        let bytes = match segment.writer.finish(end - first) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.stop_video(end, VideoStopReason::WriteError);
                return;
            }
        };
        self.closed_bytes += bytes;
        if bytes > 0 {
            self.manifest.segments.push(SegmentEntry {
                file: store::segment_file(number),
                start_offset_ms: first,
                end_offset_ms: end,
                frames,
                bytes,
                width,
                height,
            });
            let _ = Store::write_manifest(&self.setup.dir, &self.manifest);
        }
        self.write_own(
            end,
            EventKind::SegmentClosed {
                segment: number,
                frames,
                bytes,
                duration_ms: end - first,
                reason,
            },
        );
    }

    /// When the open segment must close (recording timeline), if one is open.
    pub(crate) fn deadline(&self) -> Option<u64> {
        self.segment
            .as_ref()
            .map(|s| s.first_offset + self.setup.segment_ms)
    }

    /// Close the open segment once its time is up.
    pub(crate) fn tick(&mut self, now: u64) {
        if let Some(deadline) = self.deadline() {
            if now >= deadline {
                self.close_segment(deadline, SegmentCloseReason::Duration);
            }
        }
    }

    /// Stop: write the remaining events, close the open segment, write the
    /// figures and the end into the manifest, then the final event.
    /// `captured` is the number of changed frames the capture stage took.
    pub(crate) fn finish(
        mut self,
        last: Option<SessionEvent>,
        now: u64,
        captured: u64,
    ) -> Manifest {
        self.close_gap(now);
        self.close_segment(now, SegmentCloseReason::Stop);
        let last = last.unwrap_or_else(|| {
            let mut event = self.event(
                now,
                EventKind::RecordingStopped {
                    reason: StopReason::SessionClosed,
                },
            );
            event.source = EventSource::Daemon;
            event
        });
        let reason = match &last.kind {
            EventKind::RecordingStopped { reason } => *reason,
            _ => StopReason::SessionClosed,
        };
        let reason = if self.failed {
            StopReason::WriteError
        } else {
            reason
        };
        let f = &self.figures;
        let encoded = f.frames_encoded.max(1) as f64;
        self.manifest.stats = Some(Stats {
            frames_captured: captured,
            frames_encoded: f.frames_encoded,
            frames_dropped: f.frames_dropped,
            gap_periods: f.gap_periods,
            encode_ms_mean: round2(f.busy_ms / encoded),
            encode_call_ms_p50: round2(percentile(&f.encode_calls_ms, 50.0)),
            encode_call_ms_p95: round2(percentile(&f.encode_calls_ms, 95.0)),
            encode_call_ms_max: round2(percentile(&f.encode_calls_ms, 100.0)),
            capture_to_take_ms_p50: round2(percentile(&f.take_waits_ms, 50.0)),
            capture_to_take_ms_p95: round2(percentile(&f.take_waits_ms, 95.0)),
            capture_to_take_ms_max: round2(percentile(&f.take_waits_ms, 100.0)),
            frames_held_at_close_max: f.held_at_close_max,
            flush_ms_max: round2(f.flush_ms_max),
            events_lost: f.events_lost,
        });
        let end = self.event(now, EventKind::SessionEnded);
        self.manifest.ended_at = Some(end.at);
        self.manifest.end_reason = Some(reason.as_str().to_owned());
        self.manifest.duration_ms = Some(now);
        let _ = Store::write_manifest(&self.setup.dir, &self.manifest);
        self.write(last);
        self.manifest
    }
}

/// Mux `packets` in order, each with its own frame's capture time.
fn mux(segment: &mut Segment, packets: Vec<Packet>) -> Result<u64, VideoStopReason> {
    let mut count = 0;
    for packet in packets {
        let Some(front) = segment.pending.pop_front() else {
            return Err(VideoStopReason::EncoderError);
        };
        if front.input_frameno != packet.input_frameno {
            return Err(VideoStopReason::EncoderError);
        }
        segment.largest_packet = segment.largest_packet.max(packet.data.len() as u64);
        segment
            .writer
            .write_frame(
                front.offset_ms - segment.first_offset,
                &packet.data,
                packet.key,
            )
            .map_err(|_| VideoStopReason::WriteError)?;
        count += 1;
    }
    Ok(count)
}

/// A running recorder thread.
pub(crate) struct RecorderThread {
    pub(crate) handle: JoinHandle<Manifest>,
}

/// Run `recorder` on its own thread until the queue closes. `waker` must be
/// the one `queue` and `mailbox` wake.
///
/// # Errors
///
/// The thread cannot be spawned.
pub(crate) fn spawn(
    recorder: Recorder,
    queue: Arc<EventQueue>,
    mailbox: Arc<Mailbox>,
    waker: &Waker,
) -> io::Result<RecorderThread> {
    let timeline = recorder.setup.timeline;
    let handle = std::thread::Builder::new()
        .name("rdpilot-recorder".into())
        .spawn(move || run(recorder, &queue, &mailbox, timeline))?;
    waker.set(handle.thread().clone());
    Ok(RecorderThread { handle })
}

fn run(
    mut recorder: Recorder,
    queue: &EventQueue,
    mailbox: &Mailbox,
    timeline: Timeline,
) -> Manifest {
    lower_priority();
    loop {
        let now = timeline.offset(Instant::now());
        let Drained {
            events,
            lost,
            last,
            closed,
        } = queue.drain();
        recorder.handle_events(events, lost, now);
        if closed || recorder.failed() {
            // Stopped, or the event log cannot be written: finish what can
            // be finished.
            let now = timeline.offset(Instant::now());
            return recorder.finish(last, now, mailbox.captured());
        }
        if let Some(take) = mailbox.take() {
            recorder.handle_frame(take, now);
        }
        recorder.tick(timeline.offset(Instant::now()));
        let wait = recorder
            .deadline()
            .map_or(Duration::from_secs(1), |deadline| {
                Duration::from_millis(deadline.saturating_sub(timeline.offset(Instant::now())))
                    .min(Duration::from_secs(1))
            });
        std::thread::park_timeout(wait);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::SystemTime;

    use super::*;
    use crate::events::{RecordingTrigger, SessionClock};
    use crate::recording::capture::CapturedFrame;
    use crate::recording::encoder::tests::FakeFactory;
    use crate::recording::encoder::Rav1eFactory;
    use crate::recording::manifest::tests::sample;
    use crate::recording::store::tests::temp_root;
    use crate::recording::webm::tests::parse;

    struct Fixture {
        root: PathBuf,
        dir: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn setup(tag: &str, factory: Arc<dyn EncoderFactory>, budget: u64) -> (Fixture, Recorder) {
        let root = temp_root(tag);
        let store = Store::new(root.clone());
        let id = store::mint_id(SystemTime::now());
        let dir = store.create_recording(&id).unwrap();
        let mut manifest = sample();
        manifest.id = id;
        Store::write_manifest(&dir, &manifest).unwrap();
        let recorder = Recorder::new(RecorderSetup {
            dir: dir.clone(),
            manifest,
            timeline: Timeline {
                clock: SessionClock {
                    start: Instant::now(),
                    start_wall: SystemTime::now(),
                },
                start_offset: 0,
            },
            budget_bytes: budget,
            segment_ms: SEGMENT_MS,
            factory,
        })
        .unwrap();
        (Fixture { root, dir }, recorder)
    }

    fn pixels(w: u32, h: u32, seed: u32) -> Arc<Vec<u8>> {
        Arc::new(
            (0..w * h)
                .flat_map(|i| {
                    let v = ((i * 13 + seed * 57) % 251) as u8;
                    [v, v / 2, 255 - v, 255]
                })
                .collect(),
        )
    }

    fn take(offset_ms: u64, w: u32, h: u32, seed: u32) -> Take {
        Take {
            frame: CapturedFrame {
                offset_ms,
                captured_at: Instant::now(),
                frame_seq: u64::from(seed) + 1,
                width: w,
                height: h,
                rgba: pixels(w, h, seed),
            },
            dropped: 0,
            first_dropped_ms: None,
        }
    }

    fn started(offset_ms: u64) -> SessionEvent {
        SessionEvent {
            seq: 0,
            at: "2026-01-01T00:00:00.000Z".into(),
            offset_ms,
            frame_seq: 0,
            source: EventSource::Cli,
            kind: EventKind::RecordingStarted {
                trigger: RecordingTrigger::Cli,
            },
        }
    }

    fn stopped(offset_ms: u64) -> SessionEvent {
        SessionEvent {
            kind: EventKind::RecordingStopped {
                reason: StopReason::Requested,
            },
            ..started(offset_ms)
        }
    }

    fn events(dir: &std::path::Path) -> Vec<serde_json::Value> {
        store::read_events(dir)
    }

    fn kinds(dir: &std::path::Path) -> Vec<String> {
        events(dir)
            .iter()
            .map(|e| e["kind"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn segment(dir: &std::path::Path, n: u32) -> PathBuf {
        dir.join(store::SEGMENTS).join(store::segment_file(n))
    }

    /// Structure and the encoder lag: with the real encoder, 1, 3 and 10
    /// changed frames followed by an idle display all reach the file at
    /// their exact capture times once the segment closes.
    #[test]
    fn every_frame_reaches_the_segment_at_its_capture_time() {
        for count in [1_u32, 3, 10] {
            let (fx, mut rec) = setup("rec-lag", Arc::new(Rav1eFactory), u64::MAX);
            rec.handle_events(vec![started(0)], 0, 0);
            let offsets: Vec<u64> = (0..count).map(|i| 100 + u64::from(i) * 250).collect();
            for (i, offset) in offsets.iter().enumerate() {
                rec.handle_frame(take(*offset, 64, 48, i as u32), *offset);
            }
            // Idle: nothing arrives; the timer closes the segment.
            rec.tick(30_000);
            assert!(!segment(&fx.dir, 1).exists(), "open until the timer");
            rec.tick(100 + SEGMENT_MS);
            let manifest = rec.finish(Some(stopped(70_000)), 70_000, u64::from(count));
            let p = parse(&segment(&fx.dir, 1));
            assert_eq!(p.doc_type, "webm");
            assert_eq!(p.codec, "V_AV1");
            assert!(p.codec_private.len() > 4, "av1C plus sequence header");
            assert_eq!(p.codec_private[0], 0x81);
            assert_eq!((p.width, p.height), (64, 48));
            assert!(p.has_cues);
            let times: Vec<u64> = p.frames.iter().map(|f| f.0).collect();
            let expected: Vec<u64> = offsets.iter().map(|o| o - 100).collect();
            assert_eq!(times, expected, "count {count}");
            assert!(p.frames[0].1.unwrap_or(true));
            assert!((p.duration_ms - SEGMENT_MS as f64).abs() < 1.0);
            assert_eq!(manifest.segments.len(), 1);
            assert_eq!(manifest.segments[0].frames, u64::from(count));
            assert_eq!(manifest.segments[0].start_offset_ms, 100);
            assert_eq!(manifest.segments[0].end_offset_ms, 100 + SEGMENT_MS);
            let stats = manifest.stats.unwrap();
            assert_eq!(stats.frames_encoded, u64::from(count));
            assert_eq!(stats.frames_captured, u64::from(count));
            assert!(stats.frames_held_at_close_max >= 1);
            let log = events(&fx.dir);
            let closed = log.iter().find(|e| e["kind"] == "segment_closed").unwrap();
            assert_eq!(closed["duration_ms"], SEGMENT_MS);
            assert_eq!(closed["reason"], "duration");
            assert_eq!(closed["frames"], u64::from(count));
            assert_eq!(log.first().unwrap()["kind"], "recording_started");
            assert_eq!(log.last().unwrap()["kind"], "recording_stopped");
            assert!(log
                .windows(2)
                .all(|w| w[0]["offset_ms"].as_u64() <= w[1]["offset_ms"].as_u64()));
            let m = Store::read_manifest(&fx.dir).unwrap();
            assert_eq!(m.end_reason.as_deref(), Some("requested"));
            assert!(m.ended_at.is_some());
        }
    }

    /// A resize while frames are pending flushes them into the old segment;
    /// the new size starts a new segment with a new encoder and a key frame.
    #[test]
    fn a_resize_closes_the_segment_with_its_pending_frames() {
        let fake = FakeFactory {
            lag: 4,
            ..FakeFactory::default()
        };
        let (fx, mut rec) = setup("rec-resize", Arc::new(fake.clone()), u64::MAX);
        rec.handle_frame(take(0, 8, 6, 0), 0);
        rec.handle_frame(take(250, 8, 6, 1), 250);
        rec.handle_frame(take(500, 16, 12, 2), 500);
        rec.handle_frame(take(750, 16, 12, 3), 750);
        let m = rec.finish(Some(stopped(1000)), 1000, 4);
        assert_eq!(fake.created.load(Ordering::SeqCst), 2);
        assert_eq!(*fake.sizes.lock().unwrap(), [(8, 6), (16, 12)]);
        let first = parse(&segment(&fx.dir, 1));
        assert_eq!(
            first.frames.iter().map(|f| f.0).collect::<Vec<_>>(),
            [0, 250]
        );
        assert!((first.duration_ms - 500.0).abs() < 1.0);
        let second = parse(&segment(&fx.dir, 2));
        assert_eq!(
            second.frames.iter().map(|f| f.0).collect::<Vec<_>>(),
            [0, 250]
        );
        assert_eq!(second.frames[0].1, Some(true));
        assert_eq!(m.segments.len(), 2);
        let k = kinds(&fx.dir);
        let at = |name: &str| k.iter().position(|x| x == name).unwrap();
        assert!(at("segment_closed") < at("desktop_resized"));
        assert_eq!(
            k.iter().filter(|x| *x == "segment_started").count(),
            2,
            "{k:?}"
        );
        let closed = &events(&fx.dir)
            .into_iter()
            .find(|e| e["kind"] == "segment_closed")
            .unwrap();
        assert_eq!(closed["reason"], "resize");
    }

    /// Ten minutes without a change add no frame and no segment; the 60 s
    /// timer closes the open one and starts nothing new.
    #[test]
    fn an_idle_display_adds_nothing() {
        let fake = FakeFactory {
            lag: 4,
            ..FakeFactory::default()
        };
        let (fx, mut rec) = setup("rec-idle", Arc::new(fake.clone()), u64::MAX);
        rec.handle_frame(take(0, 8, 6, 0), 0);
        let mut now = 0;
        while now < 600_000 {
            now += 1000;
            rec.tick(now);
        }
        assert_eq!(rec.deadline(), None);
        let m = rec.finish(Some(stopped(now)), now, 1);
        assert_eq!(fake.created.load(Ordering::SeqCst), 1);
        assert_eq!(fake.sends.load(Ordering::SeqCst), 1);
        assert_eq!(m.segments.len(), 1);
        assert_eq!(m.segments[0].end_offset_ms, SEGMENT_MS);
        let k = kinds(&fx.dir);
        assert_eq!(k.iter().filter(|x| *x == "segment_started").count(), 1);
        assert_eq!(k.iter().filter(|x| *x == "segment_closed").count(), 1);
    }

    /// The 60 s bound starts a new segment for the next change.
    #[test]
    fn the_segment_timer_starts_a_new_segment_for_the_next_change() {
        let fake = FakeFactory::default();
        let (fx, mut rec) = setup("rec-timer", Arc::new(fake.clone()), u64::MAX);
        for i in 0..5_u32 {
            let t = u64::from(i) * 20_000;
            rec.tick(t);
            rec.handle_frame(take(t, 8, 6, i), t);
        }
        let m = rec.finish(Some(stopped(90_000)), 90_000, 5);
        assert_eq!(fake.created.load(Ordering::SeqCst), 2);
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.segments[0].frames, 3);
        assert_eq!(m.segments[0].end_offset_ms, 60_000);
        assert_eq!(m.segments[1].start_offset_ms, 60_000);
        assert_eq!(m.segments[1].frames, 2);
        let second = parse(&segment(&fx.dir, 2));
        assert_eq!(second.frames[0].1, Some(true));
    }

    #[test]
    fn packets_out_of_input_order_stop_the_video_and_events_continue() {
        let fake = FakeFactory {
            out_of_order: true,
            lag: 4,
            ..FakeFactory::default()
        };
        let (fx, mut rec) = setup("rec-order", Arc::new(fake), u64::MAX);
        rec.handle_frame(take(0, 8, 6, 0), 0);
        rec.handle_frame(take(250, 8, 6, 1), 250);
        rec.handle_frame(take(500, 8, 6, 2), 500);
        rec.handle_events(vec![started(600)], 0, 600);
        rec.finish(Some(stopped(700)), 700, 3);
        let log = events(&fx.dir);
        let stop = log.iter().find(|e| e["kind"] == "video_stopped").unwrap();
        assert_eq!(stop["reason"], "encoder_error");
        assert!(log.iter().any(|e| e["kind"] == "recording_started"));
        assert_eq!(log.last().unwrap()["kind"], "recording_stopped");
    }

    #[test]
    fn a_failing_encoder_stops_the_video_and_events_continue() {
        let fake = FakeFactory {
            fail_at_send: Some(2),
            ..FakeFactory::default()
        };
        let (fx, mut rec) = setup("rec-fail", Arc::new(fake.clone()), u64::MAX);
        for i in 0..5_u32 {
            rec.handle_frame(take(u64::from(i) * 250, 8, 6, i), u64::from(i) * 250);
        }
        rec.handle_events(vec![started(1300)], 0, 1300);
        rec.finish(Some(stopped(1400)), 1400, 5);
        assert_eq!(
            fake.sends.load(Ordering::SeqCst),
            3,
            "no send after the failure"
        );
        let k = kinds(&fx.dir);
        assert_eq!(k.iter().filter(|x| *x == "video_stopped").count(), 1);
        assert!(k.contains(&"recording_started".to_owned()));
        assert!(!segment(&fx.dir, 1).exists());
        let parts = std::fs::read_dir(fx.dir.join(store::SEGMENTS))
            .unwrap()
            .count();
        assert_eq!(parts, 0, "the unfinished segment is removed");
    }

    /// At the size cap the video stops, no frame is sent afterwards, the
    /// overshoot stays within the frames in flight, and events continue.
    /// A kept recording is capped the same way.
    #[test]
    fn the_size_cap_stops_the_video_within_the_in_flight_bound() {
        for keep in [false, true] {
            let fake = FakeFactory {
                lag: 4,
                packet_bytes: 1000,
                ..FakeFactory::default()
            };
            let budget = 8_000;
            let (fx, mut rec) = setup("rec-cap", Arc::new(fake.clone()), budget);
            if keep {
                std::fs::write(fx.dir.join(store::KEEP), b"").unwrap();
            }
            for i in 0..40_u32 {
                let t = u64::from(i) * 250;
                rec.handle_frame(take(t, 8, 6, i), t);
            }
            let sends = fake.sends.load(Ordering::SeqCst);
            rec.handle_events(vec![started(20_000)], 0, 20_000);
            rec.finish(Some(stopped(20_100)), 20_100, 40);
            assert!(sends < 40);
            assert_eq!(fake.sends.load(Ordering::SeqCst), sends);
            let log = events(&fx.dir);
            let stop = log.iter().find(|e| e["kind"] == "video_stopped").unwrap();
            assert_eq!(stop["reason"], "size_cap");
            assert!(log.iter().any(|e| e["kind"] == "recording_started"));
            let video = std::fs::metadata(segment(&fx.dir, 1)).unwrap().len();
            // Overshoot: at most the frames the encoder held, one packet each.
            assert!(video <= budget + 5 * 1000 + 1000, "{video}");
        }
    }

    /// Stopped abruptly mid-segment (no finish): the closed segments still
    /// parse, the log parses to its last full line, and the open segment's
    /// unfinished file is not a segment.
    #[test]
    fn an_abrupt_stop_leaves_closed_segments_and_the_log_readable() {
        let fake = FakeFactory::default();
        let (fx, mut rec) = setup("rec-crash", Arc::new(fake), u64::MAX);
        rec.handle_events(vec![started(0)], 0, 0);
        rec.handle_frame(take(0, 8, 6, 0), 0);
        rec.tick(SEGMENT_MS);
        rec.handle_frame(take(61_000, 8, 6, 1), 61_000);
        drop(rec);
        assert!(parse(&segment(&fx.dir, 1)).frames.len() == 1);
        assert!(!segment(&fx.dir, 2).exists());
        let mut raw = std::fs::read(fx.dir.join(store::EVENTS)).unwrap();
        raw.extend_from_slice(b"{\"seq\":99,\"kind\":\"annot");
        std::fs::write(fx.dir.join(store::EVENTS), raw).unwrap();
        let log = events(&fx.dir);
        assert_eq!(log.first().unwrap()["kind"], "recording_started");
        assert_ne!(log.last().unwrap()["seq"], 99);
        let store = Store::new(fx.root.clone());
        let listed = store.scan();
        assert_eq!(listed.len(), 1);
        assert!(store.segment_path(&listed[0].manifest.id, 2).is_none());
    }

    /// A directory that became read-only: the next segment cannot be
    /// created, the video stops with a write error, events continue.
    #[cfg(unix)]
    #[test]
    fn a_read_only_directory_stops_the_video_with_a_write_error() {
        use std::os::unix::fs::PermissionsExt;
        let fake = FakeFactory::default();
        let (fx, mut rec) = setup("rec-ro", Arc::new(fake), u64::MAX);
        rec.handle_events(vec![started(0)], 0, 0);
        let segments = fx.dir.join(store::SEGMENTS);
        std::fs::set_permissions(&segments, std::fs::Permissions::from_mode(0o500)).unwrap();
        rec.handle_frame(take(0, 8, 6, 0), 0);
        rec.handle_frame(take(250, 8, 6, 1), 250);
        let m = rec.finish(Some(stopped(300)), 300, 2);
        std::fs::set_permissions(&segments, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(m.segments.is_empty());
        let log = events(&fx.dir);
        let stop: Vec<_> = log
            .iter()
            .filter(|e| e["kind"] == "video_stopped")
            .collect();
        assert_eq!(stop.len(), 1);
        assert_eq!(stop[0]["reason"], "write_error");
        assert_eq!(log.last().unwrap()["kind"], "recording_stopped");
    }

    /// For the same typing-like sequence of changed frames, the video is
    /// smaller than the frames stored as PNG.
    #[test]
    fn video_is_smaller_than_png_for_typing() {
        let (w, h) = (1920_u32, 1080_u32);
        let (fx, mut rec) = setup("rec-size", Arc::new(Rav1eFactory), u64::MAX);
        let mut screen: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                if (200..1500).contains(&x) && (150..800).contains(&y) {
                    [250, 250, 250, 255]
                } else {
                    [0, 90, 190, 255]
                }
            })
            .collect();
        let mut png = 0_u64;
        for n in 0..4_u32 {
            // Type one glyph-sized block of dark pixels.
            let (x0, y0) = (220 + n * 12, 180);
            for y in y0..y0 + 16 {
                for x in x0..x0 + 9 {
                    if (x + y + n) % 3 != 0 {
                        let p = ((y * w + x) * 4) as usize;
                        screen[p..p + 3].copy_from_slice(&[10, 10, 10]);
                    }
                }
            }
            png += rdpilot::Screenshot {
                width: w,
                height: h,
                rgba: screen.clone(),
            }
            .to_png()
            .unwrap()
            .len() as u64;
            let t = u64::from(n) * 250;
            rec.handle_frame(
                Take {
                    frame: CapturedFrame {
                        offset_ms: t,
                        captured_at: Instant::now(),
                        frame_seq: u64::from(n) + 1,
                        width: w,
                        height: h,
                        rgba: Arc::new(screen.clone()),
                    },
                    dropped: 0,
                    first_dropped_ms: None,
                },
                t,
            );
        }
        let m = rec.finish(Some(stopped(4000)), 4000, 4);
        let video: u64 = m.segments.iter().map(|s| s.bytes).sum();
        assert_eq!(m.segments[0].frames, 4);
        assert!(video < png, "video {video} vs png {png}");
        let _ = fx;
    }

    /// A slow encoder under a steady change source: the capture stage keeps
    /// its pace, the mailbox holds one frame, replaced frames become one
    /// `frames_dropped` event per backlog period with exact counts, every
    /// encoded frame keeps its capture time, and recording an event stays
    /// fast while the encoder is busy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_encoder_drops_frames_with_gap_events() {
        use crate::events::SessionEvents;
        use crate::recording::capture::tests::TestFrames;
        use crate::recording::capture::{self, Mailbox};
        use crate::recording::log::{EventQueue, QueueSink, Waker};

        let fake = FakeFactory {
            delay: Duration::from_millis(1000),
            ..FakeFactory::default()
        };
        let root = temp_root("rec-backlog");
        let store = Store::new(root.clone());
        let id = store::mint_id(SystemTime::now());
        let dir = store.create_recording(&id).unwrap();
        let mut manifest = sample();
        manifest.id = id;
        Store::write_manifest(&dir, &manifest).unwrap();
        let frames = TestFrames::new(8, 6);
        let log = Arc::new(SessionEvents::new(&"alpha".parse().unwrap(), 1, None));
        let timeline = Timeline {
            clock: log.clock(),
            start_offset: log.clock().offset_ms(Instant::now()),
        };
        let recorder = Recorder::new(RecorderSetup {
            dir: dir.clone(),
            manifest,
            timeline,
            budget_bytes: u64::MAX,
            segment_ms: SEGMENT_MS,
            factory: Arc::new(fake.clone()),
        })
        .unwrap();
        let waker = Arc::new(Waker::default());
        let queue = EventQueue::new(Arc::clone(&waker));
        let mailbox = Mailbox::new(Arc::clone(&waker));
        let thread = spawn(recorder, Arc::clone(&queue), Arc::clone(&mailbox), &waker).unwrap();
        log.attach_sink(
            Box::new(QueueSink(Arc::clone(&queue))),
            EventSource::Cli,
            EventKind::RecordingStarted {
                trigger: RecordingTrigger::Cli,
            },
        )
        .ok()
        .unwrap();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let capture = tokio::spawn(capture::run(
            frames.clone(),
            Arc::clone(&mailbox),
            timeline,
            4.0,
            stop_rx,
        ));
        let mut slowest = Duration::ZERO;
        for v in 0..40_u8 {
            frames.paint(v.wrapping_mul(5).wrapping_add(1));
            let t = Instant::now();
            log.record(EventSource::Cli, EventKind::SessionEnded);
            slowest = slowest.max(t.elapsed());
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        stop_tx.send_replace(true);
        capture.await.unwrap();
        log.detach_sink(
            EventSource::Cli,
            EventKind::RecordingStopped {
                reason: StopReason::Requested,
            },
        );
        let manifest = tokio::task::spawn_blocking(move || thread.handle.join().unwrap())
            .await
            .unwrap();
        assert!(slowest < Duration::from_millis(10), "{slowest:?}");
        let stats = manifest.stats.clone().unwrap();
        let captured = mailbox.captured();
        assert!(captured >= 12, "4 s at 4 fps: {captured}");
        assert!(stats.frames_encoded <= 7, "{stats:?}");
        assert!(stats.frames_dropped > 0);
        assert!(
            captured - stats.frames_encoded - stats.frames_dropped <= 1,
            "{captured} {stats:?}"
        );
        let log_lines = store::read_events(&dir);
        let gaps: Vec<&serde_json::Value> = log_lines
            .iter()
            .filter(|e| e["kind"] == "frames_dropped")
            .collect();
        assert!(!gaps.is_empty());
        assert_eq!(
            gaps.iter()
                .map(|g| g["count"].as_u64().unwrap())
                .sum::<u64>(),
            stats.frames_dropped
        );
        assert!(gaps.len() as u64 <= stats.gap_periods);
        for g in &gaps {
            assert!(g["from_offset_ms"].as_u64() < g["to_offset_ms"].as_u64());
        }
        // Encoded frames are one pacing interval apart or more, at the
        // capture times the segment records.
        let p = parse(&segment(&dir, 1));
        let times: Vec<u64> = p.frames.iter().map(|f| f.0).collect();
        assert_eq!(times.len() as u64, stats.frames_encoded);
        assert!(times.windows(2).all(|w| w[1] - w[0] >= 240), "{times:?}");
        // Newest wins: a frame waits for the encoder at most until the next
        // capture replaces it, one pacing interval.
        assert!(stats.capture_to_take_ms_max <= 250.0 + 100.0, "{stats:?}");
        let _ = std::fs::remove_dir_all(root);
    }
}
