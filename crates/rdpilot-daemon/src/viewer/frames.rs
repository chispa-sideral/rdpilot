//! Per-session encoded-frame cache, rate cap and long-poll for the viewer.
//!
//! - At most one encoded PNG per session is kept, and all clients of that
//!   session share it, so encode cost does not grow with the number of tabs.
//! - At most one capture+encode per session per [`FRAME_INTERVAL`] (4 fps).
//! - Capture and encode run in `spawn_blocking`. The session loop is never
//!   involved: its only coupling is the frame source's non-blocking signal.
//! - Stale frames are dropped: a slow client receives the newest frame when
//!   it asks next.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rdpilot_ipc::SessionId;

use crate::registry::{FrameLookup, ViewerRegistry};
use crate::seams::ViewFrameSource;

/// Minimum time between two encodes of one session (a 4 fps cap).
pub(crate) const FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// How often a long-poll re-checks that its session still exists.
const RECHECK: Duration = Duration::from_millis(500);

/// One encoded frame.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub(crate) seq: u64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) png: Bytes,
    /// When the frame was captured (rate-cap reference).
    pub(crate) at: Instant,
}

/// The answer to one long-poll.
#[derive(Debug)]
pub(crate) enum FrameReply {
    /// A frame newer than the client's.
    Frame(Arc<Encoded>),
    /// Nothing newer before the long-poll deadline.
    NoChange,
    /// The RDP session ended (server side); the entry may still exist.
    Ended,
    /// The session is not in the registry.
    Closed,
    /// Capture or encode failed.
    Failed,
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<Encoded>>>>;

/// The per-session encoded-frame cache.
pub(crate) struct FrameCache {
    slots: Mutex<HashMap<SessionId, Slot>>,
    interval: Duration,
    long_poll: Duration,
}

impl FrameCache {
    pub(crate) fn new(interval: Duration, long_poll: Duration) -> Self {
        FrameCache {
            slots: Mutex::new(HashMap::new()),
            interval,
            long_poll,
        }
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, Slot>> {
        match self.slots.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn slot(&self, id: &SessionId) -> Slot {
        Arc::clone(self.slots().entry(id.clone()).or_default())
    }

    /// Drop cached frames of sessions that are no longer listed.
    pub(crate) fn retain(&self, live: &[SessionId]) {
        self.slots().retain(|id, _| live.contains(id));
    }

    #[cfg(test)]
    pub(crate) fn cached_sessions(&self) -> usize {
        self.slots().len()
    }

    /// Seed the cache (tests only).
    #[cfg(test)]
    pub(crate) async fn seed(&self, id: &SessionId, encoded: Encoded) {
        *self.slot(id).lock().await = Some(Arc::new(encoded));
    }

    /// Wait (bounded by the long-poll time) for a frame newer than `after`.
    pub(crate) async fn next(
        &self,
        registry: &ViewerRegistry,
        id: &SessionId,
        after: u64,
    ) -> FrameReply {
        let deadline = Instant::now() + self.long_poll;
        loop {
            let source = match registry.frame_source(id) {
                FrameLookup::Source(source) => source,
                FrameLookup::Closed => {
                    self.slots().remove(id);
                    return FrameReply::Closed;
                }
                FrameLookup::Unavailable => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return FrameReply::NoChange;
                    }
                    tokio::time::sleep(remaining.min(RECHECK)).await;
                    continue;
                }
            };
            let status = source.status();
            if status.seq > after {
                return self.encoded_after(id, source, after).await;
            }
            if status.ended {
                return FrameReply::Ended;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return FrameReply::NoChange;
            }
            // Re-check the registry now and then so a closed session is
            // reported even if its source never signals.
            let _ = tokio::time::timeout(remaining.min(RECHECK), source.changed(after)).await;
        }
    }

    async fn encoded_after(
        &self,
        id: &SessionId,
        source: Arc<dyn ViewFrameSource>,
        after: u64,
    ) -> FrameReply {
        let slot = self.slot(id);
        // One encode per session at a time; other clients wait here and
        // then share the result.
        let mut cached = slot.lock().await;
        if let Some(encoded) = cached.as_ref() {
            let current = source.status().seq;
            let fresh = encoded.seq >= current || encoded.at.elapsed() < self.interval;
            if encoded.seq > after && fresh {
                return FrameReply::Frame(Arc::clone(encoded));
            }
            let wait = self.interval.saturating_sub(encoded.at.elapsed());
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        let capture_source = Arc::clone(&source);
        let encoded = tokio::task::spawn_blocking(move || {
            let at = Instant::now();
            let (seq, shot) = capture_source.capture()?;
            let png = shot.to_png().ok()?;
            Some(Encoded {
                seq,
                width: shot.width,
                height: shot.height,
                png: Bytes::from(png),
                at,
            })
        })
        .await;
        match encoded {
            Ok(Some(encoded)) => {
                let encoded = Arc::new(encoded);
                *cached = Some(Arc::clone(&encoded));
                if encoded.seq > after {
                    FrameReply::Frame(encoded)
                } else {
                    FrameReply::NoChange
                }
            }
            _ => FrameReply::Failed,
        }
    }
}
