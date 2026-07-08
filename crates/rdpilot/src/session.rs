//! [`Session`] — the public handle to a managed RDP session (SESS-02, D-04/D-07).
//!
//! `Session::connect` runs the internal connect path, drives the SDK-owned
//! active-session loop on a dedicated background thread (current-thread Tokio
//! runtime — see the `thread` field for why not `tokio::spawn`), and returns a
//! handle. The consumer never drives the RDP state machine (D-04): the background
//! loop pumps PDUs, maintains the latest framebuffer snapshot, and emits the
//! automatic keepalive.
//!
//! - [`Session::screenshot`] clones the latest [`FrameSnapshot`](crate::framebuffer)
//!   into an owned [`Screenshot`] — never the live `DecodedImage` (Pitfall 3).
//! - [`Session::close`] requests a graceful client-side shutdown and awaits the
//!   loop's exit.
//! - [`Drop`] closes the channel (signalling the loop to exit) if `close()` was
//!   not called (best-effort, cannot `await`; D-07).
//!
//! Only owned SDK types appear in the public signatures — no `ironrdp`, `image`,
//! `rustls`, or `tokio` type leaks (D-09). No `unwrap`/`expect`/`panic` (API-01).

use std::sync::Mutex;
use std::thread::JoinHandle;

use ironrdp_input::Database;
use tokio::sync::mpsc;

use crate::config::ConnectionConfig;
use crate::connect;
use crate::error::{Error, Result};
use crate::framebuffer::SharedFrame;
use crate::input::MouseAction;
use crate::screenshot::Screenshot;
use crate::session_loop::{self, RdpInputEvent};

/// Bound on the input/control channel. Keepalive + close are low-frequency; a
/// small buffer is ample and bounds memory if the loop briefly lags.
const INPUT_CHANNEL_CAPACITY: usize = 16;

/// A live, managed RDP session.
///
/// Obtain one with [`Session::connect`]. The SDK owns the session loop on a
/// background task; you read the latest framebuffer with [`Session::screenshot`]
/// and tear down with [`Session::close`]. Dropping the handle without calling
/// `close()` aborts the background task as a best-effort fallback (D-07).
pub struct Session {
    /// Join handle of the OS thread running the active-session loop.
    ///
    /// The loop is driven on a dedicated current-thread Tokio runtime rather than
    /// via `tokio::spawn`: the reactivation step in the loop holds a
    /// `Sequence::next_pdu_hint() -> Option<&dyn PduHint>` borrow across an
    /// `.await`, which trips a known higher-ranked-lifetime limitation in
    /// `tokio::spawn`'s auto-`Send` inference. A current-thread runtime imposes no
    /// `Send` bound on its futures, sidestepping the limitation cleanly without
    /// leaking it to consumers.
    thread: Option<JoinHandle<Result<()>>>,
    /// Sender for input/control events into the loop.
    input_tx: mpsc::Sender<RdpInputEvent>,
    /// Read handle to the shared latest-frame snapshot.
    frame: SharedFrame,
    /// Stateful input tracking (button/modifier state, move dedup) for real
    /// user input (D-3.6). Locked only for the synchronous
    /// `Database::apply()` call — never held across an `.await` (Pitfall 3,
    /// T-03-07).
    input_db: Mutex<Database>,
    /// The negotiated desktop size, in physical virtual-desktop pixels
    /// `(width, height)`, captured once at connect time (D-3.2). See
    /// `Session::connect`'s doc comment for why this is a deliberate v1
    /// static capture, not live-updated on a server-driven reactivation
    /// resize (RESEARCH Open Q1 / Assumption A1).
    desktop_size: (u32, u32),
}

impl Session {
    /// Connect to and authenticate an RDP session, returning a managed handle.
    ///
    /// Runs the full connect sequence (TCP → TLS/CredSSP → active session), spawns
    /// the SDK-owned loop, and returns once the session is active. The consumer
    /// never drives the state machine afterward (D-04).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Connect`] / [`Error::Tls`] if the connection or
    /// authentication fails.
    pub async fn connect(cfg: &ConnectionConfig) -> Result<Session> {
        let (connection_result, framed) = connect::connect(cfg).await?;

        // Deliberate v1 static capture (D-3.2; RESEARCH Open Q1 / Assumption
        // A1): copy the negotiated desktop size out here, BEFORE
        // `connection_result` moves into the session-loop thread below. This
        // value is never live-updated on a server-driven reactivation resize
        // (`session_loop::reactivate` rebuilds the framebuffer at a new size,
        // but does not write back here). Phase 1 fixes the VM's resolution
        // and this phase's live tests do not exercise a mid-session resize,
        // so a static connect-time capture is sufficient for this phase's
        // success criteria; a future phase can promote this field to an
        // `Arc<(AtomicU32, AtomicU32)>` written from `reactivate()` if live
        // resize-correctness is ever needed. The widening u16 -> u32 cast is
        // always lossless.
        let desktop_size = (
            u32::from(connection_result.desktop_size.width),
            u32::from(connection_result.desktop_size.height),
        );

        let frame = SharedFrame::new();
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);

        let loop_frame = frame.clone();
        // Drive the loop on a dedicated OS thread with a current-thread runtime
        // (see the `thread` field docs for why `tokio::spawn` is unsuitable).
        let thread = std::thread::Builder::new()
            .name("rdpilot-session".to_owned())
            .spawn(move || -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| Error::Session(format!("could not start session runtime: {e}")))?;
                runtime.block_on(session_loop::run(framed, connection_result, input_rx, loop_frame))
            })
            .map_err(|e| Error::Session(format!("could not spawn session thread: {e}")))?;

        Ok(Session {
            thread: Some(thread),
            input_tx,
            frame,
            input_db: Mutex::new(Database::new()),
            desktop_size,
        })
    }

    /// The negotiated desktop size, in physical virtual-desktop pixels
    /// `(width, height)` (D-3.2, SC#4).
    ///
    /// Captured once at connect time — see the doc comment on
    /// `Session::connect` for why this is a deliberate v1 simplification
    /// (not live-updated on a server-driven reactivation resize).
    #[must_use]
    pub fn desktop_size(&self) -> (u32, u32) {
        self.desktop_size
    }

    /// Reject any coordinate `action` touches that falls outside
    /// `self.desktop_size`, before any `ironrdp_input::Operation`/PDU is
    /// built (D-3.2, SC#4 — "enforced", not merely documented; T-03-05).
    fn check_bounds(&self, action: &MouseAction) -> Result<()> {
        let (w, h) = self.desktop_size;
        for (x, y) in action.coordinates() {
            let (x, y) = (u32::from(x), u32::from(y));
            if x >= w || y >= h {
                return Err(Error::coordinate_out_of_bounds(x, y, w, h));
            }
        }
        Ok(())
    }

    /// Capture the latest full-desktop framebuffer as an owned [`Screenshot`].
    ///
    /// Clones the most recent snapshot maintained by the loop — it never touches
    /// the live image (Pitfall 3). Call [`Screenshot::to_png`] to encode, or
    /// [`Screenshot::crop`] to extract a region.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Session`] if no frame has been received yet (the server
    /// has not sent an initial graphics update), or [`Error::Decode`] if the
    /// captured buffer is internally inconsistent.
    pub async fn screenshot(&self) -> Result<Screenshot> {
        let snap = self.frame.read();
        if snap.is_empty() {
            return Err(Error::Session(
                "no framebuffer captured yet (awaiting the first graphics update)".to_owned(),
            ));
        }
        Screenshot::from_rgba(snap.width, snap.height, snap.rgba)
    }

    /// Gracefully close the session and wait for the background task to finish.
    ///
    /// Sends a graceful client-side shutdown request to the loop, then awaits the
    /// task's exit. The transport socket is dropped when the task ends.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Session`] if the loop returned an error or the task
    /// panicked. A closed channel (loop already gone) is treated as already-closed
    /// and is not an error.
    pub async fn close(mut self) -> Result<()> {
        // Best-effort graceful shutdown request; if the loop already ended, the
        // channel is closed and there is nothing to shut down.
        let _ = self.input_tx.send(RdpInputEvent::Close).await;

        // Drop the sender so the loop's `input_rx.recv()` resolves to `None` and
        // the loop exits even if the graceful path did not produce a Terminate.
        // (Re-create a dummy sender to keep the field valid until `self` drops.)
        let (dummy_tx, _dummy_rx) = mpsc::channel(1);
        let _ = std::mem::replace(&mut self.input_tx, dummy_tx);

        if let Some(thread) = self.thread.take() {
            // Joining the OS thread is blocking; do it on the blocking pool so we
            // do not stall the caller's async runtime.
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .map_err(|e| Error::Session(format!("join task failed: {e}")))?
                .map_err(|_| Error::Session("session thread panicked".to_owned()))?
        } else {
            Ok(())
        }
    }
}

/// Best-effort teardown if the handle is dropped without [`Session::close`]
/// (D-07). Cannot `await`, so it does not join the thread; dropping `input_tx`
/// closes the channel, which signals the loop to exit, and the OS reaps the
/// socket as the loop's transport drops. The dedicated thread finishes on its
/// own shortly after.
impl Drop for Session {
    fn drop(&mut self) {
        // `input_tx` is dropped with `self`, closing the channel — the loop sees
        // `recv() == None` and breaks. We deliberately do not join the thread
        // here (Drop cannot block on async teardown); the detached thread exits
        // on its own. `close()` is the clean, awaited path.
        self.thread.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Button;

    /// A default desktop size for offline tests that don't care about the
    /// exact value.
    const TEST_DESKTOP_SIZE: (u32, u32) = (1920, 1080);

    fn test_input_db() -> Mutex<Database> {
        Mutex::new(Database::new())
    }

    /// `screenshot()` on a session with no captured frame returns a typed error,
    /// never a panic (no VM — drives only the snapshot read path).
    #[tokio::test]
    async fn screenshot_without_frame_returns_session_error() {
        // Build a Session without connecting: a closed channel + empty frame.
        let (input_tx, _input_rx) = mpsc::channel(1);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
        };

        let err = session.screenshot().await;
        assert!(matches!(err, Err(Error::Session(_))));
    }

    /// Once a frame is seeded, `screenshot()` returns an owned Screenshot with the
    /// snapshot's dims+bytes (no VM).
    #[tokio::test]
    async fn screenshot_returns_owned_snapshot() {
        let (input_tx, _input_rx) = mpsc::channel(1);
        let frame = SharedFrame::new();
        frame.write(2, 2, vec![255u8; 2 * 2 * 4]);

        let session = Session {
            thread: None,
            input_tx,
            frame,
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
        };

        let shot = session.screenshot().await.expect("frame present");
        assert_eq!(shot.width, 2);
        assert_eq!(shot.height, 2);
        assert_eq!(shot.rgba.len(), 2 * 2 * 4);
    }

    /// Dropping a Session with no task is a clean no-op (Drop guard path).
    #[tokio::test]
    async fn drop_without_close_is_safe() {
        let (input_tx, _input_rx) = mpsc::channel(1);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
        };
        drop(session); // must not panic
    }

    /// `desktop_size()` returns exactly the value set at construction (D-3.2).
    #[test]
    fn desktop_size_returns_construction_value() {
        let (input_tx, _input_rx) = mpsc::channel(1);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: (1920, 1080),
        };
        assert_eq!(session.desktop_size(), (1920, 1080));
    }

    /// `check_bounds` rejects a coordinate at/past the desktop edge and
    /// accepts an in-range one, without ever building a PDU (D-3.2, SC#4;
    /// T-03-05).
    #[test]
    fn check_bounds_rejects_out_of_range_and_accepts_in_range() {
        let (input_tx, _input_rx) = mpsc::channel(1);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: (1920, 1080),
        };

        let out_of_range = MouseAction::Click {
            x: 5000,
            y: 100,
            button: Button::Left,
        };
        assert!(matches!(
            session.check_bounds(&out_of_range),
            Err(Error::CoordinateOutOfBounds { .. })
        ));

        let in_range = MouseAction::Click {
            x: 100,
            y: 100,
            button: Button::Left,
        };
        assert!(session.check_bounds(&in_range).is_ok());
    }
}
