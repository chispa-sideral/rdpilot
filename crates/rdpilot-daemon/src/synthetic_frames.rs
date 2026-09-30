//! A synthetic [`ViewFrameSource`] for offline tests: the env-gated fake
//! connector (`RDPILOT_DAEMON_TEST_FRAMES`) and the viewer's unit tests.
//! It has the same latest-wins, never-blocking semantics as
//! `rdpilot::FrameWatch`. The production connector never uses it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdpilot::{FrameStatus, Screenshot};
use tokio::sync::watch;

use crate::seams::{SendFuture, ViewFrameSource};

#[derive(Default)]
struct Slot {
    frame: Option<(u32, u32, Vec<u8>)>,
    seq: u64,
}

/// A frame source fed by [`SyntheticFrames::publish`].
pub(crate) struct SyntheticFrames {
    slot: Mutex<Slot>,
    signal: watch::Sender<FrameStatus>,
}

impl SyntheticFrames {
    pub(crate) fn new() -> Arc<Self> {
        let (signal, _) = watch::channel(FrameStatus::default());
        Arc::new(SyntheticFrames {
            slot: Mutex::new(Slot::default()),
            signal,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        match self.slot.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Publish a new frame. Never blocks on observers.
    pub(crate) fn publish(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let seq = {
            let mut slot = self.lock();
            slot.seq += 1;
            slot.frame = Some((width, height, rgba));
            slot.seq
        };
        self.signal.send_if_modified(|status| {
            let newer = seq > status.seq;
            if newer {
                status.seq = seq;
            }
            newer
        });
    }

    /// Mark the source ended.
    pub(crate) fn end(&self) {
        self.signal.send_if_modified(|status| {
            let changed = !status.ended;
            status.ended = true;
            changed
        });
    }

    /// Publish a solid-colour frame; the colour is derived from `tick`.
    pub(crate) fn publish_solid(&self, width: u32, height: u32, tick: u64) {
        let colour = [
            (tick.wrapping_mul(37) % 256) as u8,
            (tick.wrapping_mul(91) % 256) as u8,
            (tick.wrapping_mul(53) % 256) as u8,
            255,
        ];
        let pixels = (width as usize) * (height as usize);
        let mut rgba = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            rgba.extend_from_slice(&colour);
        }
        self.publish(width, height, rgba);
    }

    /// Feed a changing frame every 100 ms. The size switches between
    /// 64x48 and 96x64 every 3 s so resize handling can be observed. The
    /// task ends when the returned source is ended or dropped elsewhere.
    pub(crate) fn spawn_animation(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = 0_u64;
            loop {
                let Some(source) = weak.upgrade() else { return };
                if source.signal.borrow().ended {
                    return;
                }
                let (w, h) = if (tick / 30) % 2 == 0 {
                    (64, 48)
                } else {
                    (96, 64)
                };
                source.publish_solid(w, h, tick);
                drop(source);
                tick += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }
}

impl ViewFrameSource for SyntheticFrames {
    fn status(&self) -> FrameStatus {
        *self.signal.borrow()
    }

    fn changed(&self, after_seq: u64) -> SendFuture<'_, FrameStatus> {
        let mut rx = self.signal.subscribe();
        Box::pin(async move {
            let observed = match rx.wait_for(|s| s.seq > after_seq || s.ended).await {
                Ok(status) => *status,
                Err(_) => FrameStatus {
                    seq: after_seq,
                    ended: true,
                },
            };
            observed
        })
    }

    fn geometry(&self) -> Option<(u32, u32)> {
        self.lock().frame.as_ref().map(|(w, h, _)| (*w, *h))
    }

    fn capture(&self) -> Option<(u64, Screenshot)> {
        let slot = self.lock();
        let (w, h, rgba) = slot.frame.clone()?;
        let seq = slot.seq;
        drop(slot);
        Screenshot::from_rgba(w, h, rgba).ok().map(|s| (seq, s))
    }
}
