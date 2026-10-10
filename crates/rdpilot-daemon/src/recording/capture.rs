//! The capture stage: takes changed frames from the session's passive frame
//! source at most `max_fps` times per second and hands them to the recorder
//! through a one-slot, newest-wins mailbox.
//!
//! - Runs as a task on the multi-thread runtime; the frame copy and pixel comparison run on a blocking thread.
//! - `FrameWatch` raises its sequence number even for identical pixels, so
//!   every capture is compared with the previous one; an unchanged display
//!   adds nothing.
//! - Each frame is stamped with its capture time on the recording timeline.
//! - It never waits for the encoder: a frame still in the mailbox is
//!   replaced by a newer one and counted as dropped, so neither the frame
//!   writer nor the viewer is ever held up by the recorder.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::log::Waker;
use crate::events::SessionClock;
use crate::seams::ViewFrameSource;

/// One captured, changed frame.
#[derive(Debug, Clone)]
pub(crate) struct CapturedFrame {
    /// Capture time on the recording timeline (ms).
    pub(crate) offset_ms: u64,
    /// Capture time on the monotonic clock (for lag figures).
    pub(crate) captured_at: Instant,
    pub(crate) frame_seq: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Arc<Vec<u8>>,
}

/// What the recorder takes from the mailbox.
#[derive(Debug)]
pub(crate) struct Take {
    pub(crate) frame: CapturedFrame,
    /// Frames replaced in the mailbox since the previous take.
    pub(crate) dropped: u64,
    /// Capture offset of the first of them.
    pub(crate) first_dropped_ms: Option<u64>,
}

#[derive(Default)]
struct Slot {
    frame: Option<CapturedFrame>,
    dropped: u64,
    first_dropped_ms: Option<u64>,
}

/// The one-slot, newest-wins handover from the capture stage to the
/// recorder thread.
pub(crate) struct Mailbox {
    slot: Mutex<Slot>,
    waker: Arc<Waker>,
    captured: AtomicU64,
}

impl Mailbox {
    pub(crate) fn new(waker: Arc<Waker>) -> Arc<Self> {
        Arc::new(Mailbox {
            slot: Mutex::new(Slot::default()),
            waker,
            captured: AtomicU64::new(0),
        })
    }

    fn slot(&self) -> MutexGuard<'_, Slot> {
        match self.slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Hand over `frame`; an untaken older frame is dropped and counted.
    pub(crate) fn put(&self, frame: CapturedFrame) {
        self.captured.fetch_add(1, Ordering::Relaxed);
        {
            let mut slot = self.slot();
            if let Some(old) = slot.frame.take() {
                slot.dropped += 1;
                slot.first_dropped_ms.get_or_insert(old.offset_ms);
            }
            slot.frame = Some(frame);
        }
        self.waker.wake();
    }

    /// Take the newest frame, with the drops since the previous take.
    pub(crate) fn take(&self) -> Option<Take> {
        let mut slot = self.slot();
        let frame = slot.frame.take()?;
        Some(Take {
            frame,
            dropped: std::mem::take(&mut slot.dropped),
            first_dropped_ms: slot.first_dropped_ms.take(),
        })
    }

    /// Changed frames captured so far.
    pub(crate) fn captured(&self) -> u64 {
        self.captured.load(Ordering::Relaxed)
    }
}

/// Where and how the capture stage stamps frames.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeline {
    pub(crate) clock: SessionClock,
    /// Session-clock offset where the recording timeline starts.
    pub(crate) start_offset: u64,
}

impl Timeline {
    pub(crate) fn offset(&self, at: Instant) -> u64 {
        self.clock.offset_ms(at).saturating_sub(self.start_offset)
    }
}

/// The capture stage. Returns when `stop` turns `true` (or its sender is
/// dropped) or the frame source ends.
pub(crate) async fn run(
    source: Arc<dyn ViewFrameSource>,
    mailbox: Arc<Mailbox>,
    timeline: Timeline,
    max_fps: f64,
    mut stop: watch::Receiver<bool>,
) {
    let interval = Duration::from_secs_f64(1.0 / max_fps.clamp(0.5, 8.0));
    let mut after = 0_u64;
    let mut last_capture: Option<Instant> = None;
    let mut reference: Option<(u32, u32, Arc<Vec<u8>>)> = None;
    loop {
        if *stop.borrow() {
            return;
        }
        let status = tokio::select! {
            status = source.changed(after) => status,
            _ = stop.changed() => return,
        };
        if status.ended {
            return;
        }
        if status.seq <= after {
            // A source that answers without progress: do not spin.
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(250)) => {}
                _ = stop.changed() => return,
            }
            continue;
        }
        if let Some(due) = last_capture.map(|last| last + interval) {
            let due = tokio::time::Instant::from_std(due);
            tokio::select! {
                () = tokio::time::sleep_until(due) => {}
                _ = stop.changed() => return,
            }
        }
        let source_for_capture = Arc::clone(&source);
        let previous = reference.take();
        let captured = tokio::task::spawn_blocking(move || {
            let captured_at = Instant::now();
            let Some((seq, shot)) = source_for_capture.capture() else {
                return (previous, None);
            };
            let same = previous.as_ref().is_some_and(|(w, h, rgba)| {
                *w == shot.width && *h == shot.height && rgba.as_slice() == shot.rgba.as_slice()
            });
            if same {
                return (previous, Some((seq, captured_at, None)));
            }
            let rgba = Arc::new(shot.rgba);
            let current = (shot.width, shot.height, Arc::clone(&rgba));
            (Some(current), Some((seq, captured_at, Some(rgba))))
        })
        .await;
        let Ok((kept, result)) = captured else {
            return;
        };
        reference = kept;
        let Some((seq, captured_at, changed)) = result else {
            // No frame yet: wait for the next change.
            after = status.seq;
            continue;
        };
        last_capture = Some(captured_at);
        after = seq.max(status.seq);
        if let (Some(rgba), Some((width, height, _))) = (changed, reference.as_ref()) {
            mailbox.put(CapturedFrame {
                offset_ms: timeline.offset(captured_at),
                captured_at,
                frame_seq: seq,
                width: *width,
                height: *height,
                rgba,
            });
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::seams::BoxFuture;

    /// A frame source whose pixels and sequence number tests set directly.
    pub(crate) struct TestFrames {
        state: watch::Sender<rdpilot::FrameStatus>,
        pixels: Mutex<(u32, u32, Vec<u8>)>,
        pub(crate) captures: AtomicU64,
    }

    impl TestFrames {
        pub(crate) fn new(width: u32, height: u32) -> Arc<Self> {
            Arc::new(TestFrames {
                state: watch::Sender::new(rdpilot::FrameStatus::default()),
                pixels: Mutex::new((width, height, vec![0; (width * height * 4) as usize])),
                captures: AtomicU64::new(0),
            })
        }

        /// Paint every pixel with `value` and publish a new frame.
        pub(crate) fn paint(&self, value: u8) {
            {
                let mut p = self.pixels.lock().unwrap();
                let len = p.2.len();
                p.2 = (0..len)
                    .map(|i| {
                        if i % 4 == 3 {
                            255
                        } else {
                            value.wrapping_add((i / 4 % 7) as u8)
                        }
                    })
                    .collect();
            }
            self.state.send_modify(|s| s.seq += 1);
        }

        /// Publish a new sequence number with the same pixels.
        pub(crate) fn bump(&self) {
            self.state.send_modify(|s| s.seq += 1);
        }

        pub(crate) fn end(&self) {
            self.state.send_modify(|s| s.ended = true);
        }
    }

    impl ViewFrameSource for TestFrames {
        fn status(&self) -> rdpilot::FrameStatus {
            *self.state.borrow()
        }
        fn changed(&self, after_seq: u64) -> BoxFuture<'_, rdpilot::FrameStatus> {
            let mut rx = self.state.subscribe();
            Box::pin(async move {
                rx.wait_for(|s| s.seq > after_seq || s.ended)
                    .await
                    .map(|s| *s)
                    .unwrap_or(rdpilot::FrameStatus {
                        seq: after_seq,
                        ended: true,
                    })
            })
        }
        fn capture(&self) -> Option<(u64, rdpilot::Screenshot)> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            let seq = self.state.borrow().seq;
            if seq == 0 {
                return None;
            }
            let p = self.pixels.lock().unwrap();
            Some((
                seq,
                rdpilot::Screenshot {
                    width: p.0,
                    height: p.1,
                    rgba: p.2.clone(),
                },
            ))
        }
    }

    fn timeline() -> Timeline {
        Timeline {
            clock: SessionClock {
                start: Instant::now(),
                start_wall: std::time::SystemTime::now(),
            },
            start_offset: 0,
        }
    }

    /// Drains the mailbox like a recorder would, recording what it got.
    fn collector(
        mailbox: Arc<Mailbox>,
        done: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<Vec<Take>> {
        std::thread::spawn(move || {
            let mut got = Vec::new();
            while !done.load(Ordering::SeqCst) {
                if let Some(t) = mailbox.take() {
                    got.push(t);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            got.extend(mailbox.take());
            got
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn identical_pixels_add_no_frame() {
        let frames = TestFrames::new(8, 6);
        let mailbox = Mailbox::new(Arc::new(Waker::default()));
        let (stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run(
            frames.clone(),
            Arc::clone(&mailbox),
            timeline(),
            8.0,
            stop,
        ));
        frames.paint(1);
        tokio::time::sleep(Duration::from_millis(300)).await;
        for _ in 0..10 {
            frames.bump();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(mailbox.captured(), 1);
        assert!(frames.captures.load(Ordering::SeqCst) > 1, "it did compare");
        stop_tx.send_replace(true);
        task.await.unwrap();
    }

    /// Under a 100 fps change source, at most `max_fps` frames per second
    /// (plus the first) are captured, and each frame's stamp is within one
    /// interval of the change it shows.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_cap_holds_and_stamps_follow_the_changes() {
        let frames = TestFrames::new(8, 6);
        let mailbox = Mailbox::new(Arc::new(Waker::default()));
        let tl = timeline();
        let (stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run(frames.clone(), Arc::clone(&mailbox), tl, 4.0, stop));
        let done = Arc::new(AtomicBool::new(false));
        let collect = collector(Arc::clone(&mailbox), Arc::clone(&done));
        let started = Instant::now();
        let mut changes = Vec::new();
        let mut value = 0_u8;
        while started.elapsed() < Duration::from_secs(2) {
            value = value.wrapping_add(1);
            frames.paint(value);
            changes.push(tl.offset(Instant::now()));
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        stop_tx.send_replace(true);
        task.await.unwrap();
        done.store(true, Ordering::SeqCst);
        let got = collect.join().unwrap();
        let captured = mailbox.captured();
        assert!(captured <= 2 * 4 + 2, "captured {captured}");
        assert!(captured >= 4, "captured {captured}");
        for take in &got {
            let at = take.frame.offset_ms;
            // The newest change at or before the capture is at most one
            // interval (250 ms) older, plus scheduling slack.
            let change = changes
                .iter()
                .rev()
                .find(|c| **c <= at + 1)
                .copied()
                .unwrap();
            assert!(
                at.saturating_sub(change) <= 250 + 60,
                "stamp {at} vs change {change}"
            );
        }
        let offsets: Vec<u64> = got.iter().map(|t| t.frame.offset_ms).collect();
        assert!(
            offsets.windows(2).all(|w| w[1] - w[0] >= 240),
            "{offsets:?}"
        );
    }

    /// Nobody takes: the capture stage keeps going, the slot holds one
    /// frame and every replaced frame is counted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_capture_stage_never_waits_for_the_recorder() {
        let frames = TestFrames::new(4, 4);
        let mailbox = Mailbox::new(Arc::new(Waker::default()));
        let (stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run(
            frames.clone(),
            Arc::clone(&mailbox),
            timeline(),
            8.0,
            stop,
        ));
        for v in 0..10_u8 {
            frames.paint(v * 3 + 1);
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let captured = mailbox.captured();
        let take = mailbox.take().unwrap();
        assert!(captured >= 9, "{captured}");
        assert_eq!(take.dropped, captured - 1);
        assert!(take.first_dropped_ms.unwrap() < take.frame.offset_ms);
        assert!(mailbox.take().is_none());
        stop_tx.send_replace(true);
        task.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn it_returns_when_the_source_ends_or_on_stop() {
        let frames = TestFrames::new(4, 4);
        let mailbox = Mailbox::new(Arc::new(Waker::default()));
        let (_stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run(
            frames.clone(),
            Arc::clone(&mailbox),
            timeline(),
            4.0,
            stop,
        ));
        frames.end();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        let (stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run(frames.clone(), mailbox, timeline(), 4.0, stop));
        drop(stop_tx);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}
