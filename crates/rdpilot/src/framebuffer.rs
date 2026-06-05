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

/// A cloneable handle to the shared latest-frame snapshot.
///
/// The loop holds one handle to write; the [`Session`](crate::Session) holds
/// another to read. Cloning the handle is cheap (an `Arc` bump).
#[derive(Clone, Debug)]
pub(crate) struct SharedFrame {
    inner: Arc<Mutex<FrameSnapshot>>,
}

impl SharedFrame {
    /// Create an empty shared frame (no capture yet).
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FrameSnapshot::default())),
        }
    }

    /// Overwrite the snapshot with a fresh frame.
    ///
    /// Called by the loop on each `GraphicsUpdate`. The lock is held only for the
    /// duration of the write (no `.await` inside), so a concurrent `read()` can
    /// never observe a partially-updated frame and the lock is never held across
    /// the network await.
    ///
    /// A poisoned lock (a panic in another holder, which cannot happen in this
    /// crate's no-panic code) is recovered rather than propagated, so a reader
    /// or writer is never blocked by poisoning.
    pub(crate) fn write(&self, width: u32, height: u32, rgba: Vec<u8>) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.width = width;
        guard.height = height;
        guard.rgba = rgba;
    }

    /// Clone the latest snapshot.
    ///
    /// Returns the current [`FrameSnapshot`] by value. The lock is released
    /// before the caller does anything else with the data.
    pub(crate) fn read(&self) -> FrameSnapshot {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }
}

impl Default for SharedFrame {
    fn default() -> Self {
        Self::new()
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
}
