//! Shared latest-frame snapshot (CAP-01).
//!
//! The active-session loop owns the only mutating [`DecodedImage`]. On every
//! `GraphicsUpdate` it copies the RGBA32 bytes + dimensions into a
//! [`FrameSnapshot`] held behind an `Arc<Mutex<…>>`. `Session::screenshot`
//! clones the latest snapshot — it never touches the live image and never holds
//! the lock across an `.await` (Pitfall 3).
//!
//! This snapshot is the integrity point for "current perception": a reader
//! always sees a complete, self-consistent frame (dims and bytes from the same
//! `GraphicsUpdate`), never a half-written buffer.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::Screenshot;

/// An owned, self-consistent copy of the latest decoded framebuffer.
///
/// `rgba` is tightly-packed RGBA32 (`width * height * 4` bytes, row-major, no
/// stride padding) — the native layout of IronRDP's `DecodedImage` on the
/// bitmap/RLE/RDP6/RemoteFX paths. Before the first `GraphicsUpdate` arrives the
/// snapshot is empty (`width == 0 && height == 0 && rgba.is_empty()`).
#[derive(Clone, Debug, Default)]
pub(crate) struct FrameSnapshot {
    /// Frame width, in pixels.
    pub(crate) width: u32,
    /// Frame height, in pixels.
    pub(crate) height: u32,
    /// Tightly-packed RGBA32 pixel data (`width * height * 4` bytes).
    pub(crate) rgba: Vec<u8>,
}

impl FrameSnapshot {
    /// Whether a frame has been captured yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }
}

/// The change signal published after every frame write (and once when the
/// session ends). `watch` keeps only the latest value, so a slow observer
/// skips intermediate frames and no per-observer queue ever grows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStatus {
    /// Monotonic frame sequence number. `0` means no frame has been captured.
    pub seq: u64,
    /// `true` once the RDP session loop has ended or the session was closed.
    pub ended: bool,
}

#[derive(Debug, Default)]
struct Slot {
    frame: FrameSnapshot,
    seq: u64,
}

#[derive(Debug)]
struct Shared {
    slot: Mutex<Slot>,
    signal: watch::Sender<FrameStatus>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        match self.slot.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// A cloneable handle to the shared latest-frame snapshot.
///
/// The loop holds one handle to write; the [`Session`](crate::Session) holds
/// another to read. Cloning the handle is cheap (an `Arc` bump).
#[derive(Clone, Debug)]
pub(crate) struct SharedFrame {
    inner: Arc<Shared>,
}

impl SharedFrame {
    /// Create an empty shared frame (no capture yet).
    pub(crate) fn new() -> Self {
        let (signal, _) = watch::channel(FrameStatus::default());
        Self {
            inner: Arc::new(Shared {
                slot: Mutex::new(Slot::default()),
                signal,
            }),
        }
    }

    /// Overwrite the snapshot with a fresh frame and publish its sequence.
    ///
    /// Called by the loop on each `GraphicsUpdate`. The lock is held only for the
    /// duration of the write (no `.await` inside), so a concurrent `read()` can
    /// never observe a partially-updated frame and the lock is never held across
    /// the network await. The change signal is a non-blocking `watch` update:
    /// it never waits for, or queues data for, an observer.
    ///
    /// A poisoned lock (a panic in another holder, which cannot happen in this
    /// crate's no-panic code) is recovered rather than propagated, so a reader
    /// or writer is never blocked by poisoning.
    pub(crate) fn write(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let seq = {
            let mut guard = self.inner.lock();
            guard.frame.width = width;
            guard.frame.height = height;
            guard.frame.rgba = rgba;
            guard.seq += 1;
            guard.seq
        };
        self.inner.signal.send_if_modified(|status| {
            if seq > status.seq {
                status.seq = seq;
                true
            } else {
                false
            }
        });
    }

    /// Clone the latest snapshot.
    ///
    /// Returns the current [`FrameSnapshot`] by value. The lock is released
    /// before the caller does anything else with the data.
    pub(crate) fn read(&self) -> FrameSnapshot {
        self.inner.lock().frame.clone()
    }

    /// Mark the frame source as ended (session loop returned, or the session
    /// was closed or dropped). Idempotent; wakes every waiting observer once.
    pub(crate) fn mark_ended(&self) {
        self.inner.signal.send_if_modified(|status| {
            if status.ended {
                false
            } else {
                status.ended = true;
                true
            }
        });
    }

    /// A passive, read-only observer of this frame.
    pub(crate) fn watch(&self) -> FrameWatch {
        FrameWatch {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Default for SharedFrame {
    fn default() -> Self {
        Self::new()
    }
}

/// A passive, read-only observer of a session's latest framebuffer.
///
/// It holds only the shared frame, never the [`Session`](crate::Session), so
/// it cannot keep a session, its thread or its connection alive, and it has
/// no input, Cua or transfer capability. Cloning is cheap.
#[derive(Clone)]
pub struct FrameWatch {
    inner: Arc<Shared>,
}

impl std::fmt::Debug for FrameWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameWatch")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl FrameWatch {
    /// The current sequence number and ended flag.
    #[must_use]
    pub fn status(&self) -> FrameStatus {
        *self.inner.signal.borrow()
    }

    /// Wait until a frame newer than `after_seq` exists or the source ends,
    /// and return the status observed at that point. Returns at once when
    /// that is already true. Waiting never blocks the frame writer.
    pub async fn changed(&self, after_seq: u64) -> FrameStatus {
        let mut rx = self.inner.signal.subscribe();
        let observed = match rx
            .wait_for(|status| status.seq > after_seq || status.ended)
            .await
        {
            Ok(status) => *status,
            // The sender lives in `self.inner`, which this watch keeps
            // alive, so the channel cannot close while we wait.
            Err(_) => self.status(),
        };
        observed
    }

    /// Copy the latest frame together with its sequence number, or `None`
    /// before the first frame. The frame and the sequence number always come
    /// from the same write.
    #[must_use]
    pub fn capture(&self) -> Option<(u64, Screenshot)> {
        let (seq, frame) = {
            let guard = self.inner.lock();
            if guard.frame.is_empty() {
                return None;
            }
            (guard.seq, guard.frame.clone())
        };
        Screenshot::from_rgba(frame.width, frame.height, frame.rgba)
            .ok()
            .map(|shot| (seq, shot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_until_first_write() {
        let frame = SharedFrame::new();
        let snap = frame.read();
        assert!(snap.is_empty());
        assert_eq!(snap.width, 0);
        assert_eq!(snap.height, 0);
    }

    /// Round-trip: seeding a snapshot then reading it returns identical dims+rgba
    /// (no VM required).
    #[test]
    fn write_then_read_round_trips_dims_and_bytes() {
        let frame = SharedFrame::new();
        let rgba: Vec<u8> = (0u8..=255).cycle().take(4 * 3 * 4).collect();
        frame.write(4, 3, rgba.clone());

        let snap = frame.read();
        assert!(!snap.is_empty());
        assert_eq!(snap.width, 4);
        assert_eq!(snap.height, 3);
        assert_eq!(snap.rgba, rgba);
        assert_eq!(snap.rgba.len(), 4 * 3 * 4);
    }

    /// A later write fully replaces the earlier frame (latest-wins).
    #[test]
    fn latest_write_wins() {
        let frame = SharedFrame::new();
        frame.write(2, 2, vec![1; 2 * 2 * 4]);
        frame.write(1, 1, vec![9; 4]);

        let snap = frame.read();
        assert_eq!(snap.width, 1);
        assert_eq!(snap.height, 1);
        assert_eq!(snap.rgba, vec![9; 4]);
    }

    /// The handle is cheaply cloneable and both clones see the same data.
    #[test]
    fn clones_share_state() {
        let a = SharedFrame::new();
        let b = a.clone();
        a.write(1, 1, vec![7; 4]);
        assert_eq!(b.read().rgba, vec![7; 4]);
    }

    #[test]
    fn seq_is_monotonic_and_published() {
        let frame = SharedFrame::new();
        let watch = frame.watch();
        assert_eq!(watch.status(), FrameStatus::default());
        frame.write(1, 1, vec![1; 4]);
        assert_eq!(watch.status().seq, 1);
        frame.write(1, 1, vec![2; 4]);
        frame.write(1, 1, vec![3; 4]);
        assert_eq!(watch.status().seq, 3);
        assert!(!watch.status().ended);
    }

    #[tokio::test]
    async fn changed_wakes_on_write_and_on_end() {
        let frame = SharedFrame::new();
        let watch = frame.watch();
        let waiter = tokio::spawn({
            let watch = watch.clone();
            async move { watch.changed(0).await }
        });
        tokio::task::yield_now().await;
        frame.write(1, 1, vec![0; 4]);
        let status = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("changed() must wake on write")
            .expect("join");
        assert_eq!(status.seq, 1);

        let waiter = tokio::spawn({
            let watch = watch.clone();
            async move { watch.changed(1).await }
        });
        tokio::task::yield_now().await;
        frame.mark_ended();
        let status = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("changed() must wake on end")
            .expect("join");
        assert!(status.ended);
        assert_eq!(status.seq, 1);
        // Idempotent.
        frame.mark_ended();
        assert!(watch.status().ended);
    }

    #[tokio::test]
    async fn changed_returns_at_once_when_newer_frame_exists() {
        let frame = SharedFrame::new();
        frame.write(1, 1, vec![0; 4]);
        let status = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            frame.watch().changed(0),
        )
        .await
        .expect("already-newer frame must not wait");
        assert_eq!(status.seq, 1);
    }

    /// A watcher that never reads does not block the writer, and only the
    /// latest frame is retained (bounded memory, stale frames dropped).
    #[test]
    fn idle_watcher_never_blocks_the_writer() {
        let frame = SharedFrame::new();
        let _idle = frame.watch();
        let _idle_rx = frame.inner.signal.subscribe();
        let started = std::time::Instant::now();
        for i in 0..10_000u32 {
            frame.write(1, 1, i.to_le_bytes().to_vec());
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(frame.watch().status().seq, 10_000);
        let (seq, shot) = frame.watch().capture().expect("frame");
        assert_eq!(seq, 10_000);
        assert_eq!(shot.rgba, 9_999u32.to_le_bytes().to_vec());
    }

    #[test]
    fn capture_returns_latest_frame_with_matching_dims_and_seq() {
        let frame = SharedFrame::new();
        assert!(frame.watch().capture().is_none());
        frame.write(2, 1, vec![5; 8]);
        frame.write(1, 2, vec![6; 8]);
        let (seq, shot) = frame.watch().capture().expect("frame");
        assert_eq!(seq, 2);
        assert_eq!((shot.width, shot.height), (1, 2));
        assert_eq!(shot.rgba, vec![6; 8]);
    }

    /// The watch outlives the writer side and keeps reporting `ended`.
    #[test]
    fn watch_stays_valid_after_writer_is_dropped() {
        let frame = SharedFrame::new();
        let watch = frame.watch();
        frame.write(1, 1, vec![1; 4]);
        frame.mark_ended();
        drop(frame);
        assert_eq!(
            watch.status(),
            FrameStatus {
                seq: 1,
                ended: true
            }
        );
        assert!(watch.capture().is_some());
    }
}
