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

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ironrdp::pdu::input::fast_path::FastPathInputEvent;
use ironrdp_input::Database;
use tokio::sync::mpsc;

use crate::config::ConnectionConfig;
use crate::connect;
use crate::error::{Error, Result};
use crate::framebuffer::SharedFrame;
use crate::input::{Key, KeyAction, MouseAction};
use crate::screenshot::Screenshot;
use crate::sensor::SensorShared;
use crate::session_loop::{self, RdpInputEvent};

/// Inter-click delay for [`MouseAction::DoubleClick`] (D-3.7). RDP has no
/// native double-click PDU, so a double-click is synthesized as two single
/// clicks separated by this delay, applied on the caller's async context
/// (never inside the session-loop `select!`, Pitfall 3). Windows'
/// `GetDoubleClickTime()` defaults to 500ms; 100ms sits comfortably inside
/// that window. LIVE-VERIFY tuning target confirmed against the real VM in
/// Plan 04 (RESEARCH §4 — this value is empirical, not derivable statically).
const DOUBLE_CLICK_GAP: Duration = Duration::from_millis(100);

/// Spacing between a [`MouseAction::Drag`]'s interpolated `MouseMove`
/// batches (D-3.8), applied on the caller's async context (Pitfall 3), so
/// consecutive moves are observable as distinct wall-clock events past the
/// `SM_CXDRAG`/`SM_CYDRAG` (~4px) distance threshold rather than arriving as
/// one instantaneous burst. LIVE-VERIFY tuning target confirmed against the
/// real VM in Plan 04 (RESEARCH §4 — empirical, not derivable statically).
const DRAG_STEP_GAP: Duration = Duration::from_millis(15);

/// Bound on the input/control channel. Keepalive + close are low-frequency; a
/// small buffer is ample and bounds memory if the loop briefly lags.
const INPUT_CHANNEL_CAPACITY: usize = 16;

/// Number of Win+R launch-injection attempts [`Session::deploy_and_launch`]
/// makes before giving up (D-5.2). Each attempt polls for a pong across
/// [`PINGS_PER_LAUNCH_ATTEMPT`] `ping()` calls before re-injecting; three
/// attempts gives resilience against a one-off missed keystroke/focus issue
/// without an unbounded retry.
const LAUNCH_ATTEMPTS: u32 = 3;

/// Number of `ping()` polls attempted within a single launch attempt before
/// [`Session::deploy_and_launch`] re-injects the launch sequence (D-5.2).
/// Each `ping()` call is itself bounded at its own 500ms hard timeout
/// (`Session::ping`, unchanged) — 20 polls gives ~10s per attempt, chosen to
/// comfortably exceed the ~10s `WTSVirtualChannelOpenEx` open-retry window
/// the sensor's own DVC-open race is expected to need (Phase 4 RESEARCH
/// Pitfall 1 / 04-03-SUMMARY.md, carried forward to the C# sensor by D-5.7).
/// Three attempts at ~10s each gives a ~30s total outer budget.
const PINGS_PER_LAUNCH_ATTEMPT: u32 = 20;

/// Settle delay between opening the Run dialog (Win+R) and typing the launch
/// command, giving the remote shell time to render the dialog before
/// keystrokes are injected (D-5.1). Applied on the caller's async context —
/// never inside the session-loop `select!` (Pitfall 3).
///
/// Live-tuned (Plan 04 live gate): widened from the offline-reasoned 300ms to
/// 800ms after the live VM showed a rendered-but-not-yet-input-ready Run
/// dialog at 300-500ms post-Win+R (see [`SESSION_SETTLE`]'s doc comment for
/// the related, larger finding this pairs with).
const RUN_DIALOG_SETTLE: Duration = Duration::from_millis(800);

/// One-time settle delay before the FIRST launch attempt only, applied once
/// at the top of [`Session::deploy_and_launch`] before any input is injected.
///
/// Live-tuned (Plan 04 live gate, empirical bug fix): live diagnostics showed
/// that immediately after `Session::connect` returns, the remote interactive
/// session can still be mid-transition (observed: the Windows lock-screen
/// wallpaper still on screen, or a Run dialog that visually renders correctly
/// but silently drops injected Unicode keyboard input) even though the first
/// framebuffer has already arrived and `screenshot()` succeeds. Injecting
/// Win+R + a typed command in this window produced a completely empty Run
/// dialog textbox on every one of 3 retry attempts (all within the DVC-open
/// retry budget, so this is NOT the D-5.7 sensor-side race — it reproduced
/// even for pure `Type()` calls with no sensor involved). Waiting this long
/// once, before the first attempt, before injecting ANY input, reliably
/// (3/3 in live testing) let the desktop settle enough for typed input to
/// register. Subsequent re-injections (attempts 2/3) do not repeat this
/// delay — by then the session has had ample time to settle from the natural
/// spacing of the ping-poll budget.
const SESSION_SETTLE: Duration = Duration::from_secs(3);

/// Number of characters typed per [`KeyAction::Type`] call when injecting
/// the launch command (live-tuned bug fix, see
/// [`Session::inject_launch_sequence`]'s doc comment for the full root
/// cause). Small enough that the Run dialog's ComboBox autocomplete never
/// falls behind a single burst; matches the chunk sizes that reproduced
/// cleanly during live bisection.
const TYPE_CHUNK_LEN: usize = 16;

/// Settle delay between successive typed chunks (see [`TYPE_CHUNK_LEN`]).
const TYPE_CHUNK_GAP: Duration = Duration::from_millis(150);

/// Split `s` into `max_len`-character (not byte) chunks, preserving order.
/// Pure and offline-testable. Never panics on empty input, non-ASCII input,
/// or `max_len == 0` (treated as "one char per chunk" to avoid an infinite
/// empty-chunk loop, API-01).
fn chunk_str(s: &str, max_len: usize) -> Vec<String> {
    let max_len = max_len.max(1);
    let chars: Vec<char> = s.chars().collect();
    chars
        .chunks(max_len)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

/// Build the Win+R run-command string that copies the sensor exe from the
/// RDPDR-redirected `RDPILOT` drive to `%TEMP%` and starts it (D-5.1).
///
/// Pure and offline-testable (no `Session`/IO dependency). The served
/// filename is [`crate::connect::SENSOR_EXE_NAME`] — the exact constant
/// `connect::connect`'s RDPDR registration (Task 1) announces the drive
/// backend under — so the announced name and this command can never drift
/// apart.
fn launch_command() -> String {
    let name = crate::connect::SENSOR_EXE_NAME;
    format!("cmd /c copy \\\\tsclient\\RDPILOT\\{name} %TEMP%\\{name} && start \"\" %TEMP%\\{name}")
}

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
    /// Correlation state shared with the `RdpilotSensorProcessor` registered
    /// in `connect.rs` (SENSOR-03, RESEARCH Q1): the pending `req_id`-keyed
    /// oneshot map and the version-handshake outcome. `Session::ping()`
    /// reads/mutates this from the caller's async context; the processor
    /// mutates it from the dedicated session-loop OS thread.
    sensor: Arc<SensorShared>,
    /// Monotonic correlation-id counter for `Session::ping()` requests.
    /// Starts at 1 — `req_id` 0 is reserved for the version handshake
    /// (RESEARCH Q2, D-4.3).
    next_req_id: AtomicU64,
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
        let (connection_result, framed, sensor) = connect::connect(cfg).await?;

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
            sensor,
            next_req_id: AtomicU64::new(1),
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

    /// Send a mouse action to the remote session (D-3.1, INPUT-01, SC#2).
    ///
    /// Every action's coordinate(s) are bounds-checked against
    /// [`Session::desktop_size`] and rejected with
    /// [`Error::CoordinateOutOfBounds`] **before** any
    /// `ironrdp_input::Operation`/PDU is built (D-3.2, SC#4). The action is
    /// then translated into one or more ordered `Operation` batches
    /// ([`crate::input::mouse_operations`]); each batch is applied to the
    /// stateful [`Database`] under a brief lock (never held across an
    /// `.await`) and the resulting fast-path events are sent into the
    /// session loop. `DoubleClick`/`Drag` space their batches with a real
    /// `tokio::time::sleep` **on this caller's async context** — the
    /// session-loop `select!` never sleeps (Pitfall 3).
    ///
    /// # Errors
    ///
    /// Returns [`Error::CoordinateOutOfBounds`] if any coordinate `action`
    /// touches falls outside the negotiated desktop size (nothing is sent in
    /// that case), or [`Error::Session`] if the input channel is closed
    /// (the session loop already exited) or the internal input-state lock is
    /// poisoned.
    pub async fn send_mouse(&self, action: MouseAction) -> Result<()> {
        self.check_bounds(&action)?;

        // Only DoubleClick/Drag need an inter-batch delay (D-3.7/D-3.8); all
        // other actions translate to exactly one batch, so no gap is used.
        let gap = match action {
            MouseAction::DoubleClick { .. } => Some(DOUBLE_CLICK_GAP),
            MouseAction::Drag { .. } => Some(DRAG_STEP_GAP),
            MouseAction::Move { .. } | MouseAction::Click { .. } | MouseAction::Scroll { .. } => None,
        };

        let batches = crate::input::mouse_operations(&action);
        let last_index = batches.len().saturating_sub(1);

        for (i, batch) in batches.into_iter().enumerate() {
            let events: Vec<FastPathInputEvent> = {
                let mut db = self
                    .input_db
                    .lock()
                    .map_err(|_| Error::Session("input state lock poisoned".to_owned()))?;
                db.apply(batch).into_iter().collect()
            }; // guard dropped here — never held across the .await below

            self.input_tx
                .send(RdpInputEvent::FastPath(events))
                .await
                .map_err(|_| Error::Session("input channel closed".to_owned()))?;

            if let Some(gap) = gap {
                if i != last_index {
                    tokio::time::sleep(gap).await;
                }
            }
        }

        Ok(())
    }

    /// Send a keyboard action to the remote session (D-3.1, INPUT-02, SC#3).
    ///
    /// `KeyAction::Type(text)` is translated into per-character Unicode
    /// key-press/release operations (layout-independent); `KeyAction::Combo(keys)`
    /// is translated into scancode down/up operations with modifiers pressed
    /// first and released last, in reverse order (D-3.5), so `Ctrl+A` and
    /// `Alt+F4` are expressible. Both variants translate to exactly one
    /// `Operation` batch — a Combo/Type is a single `apply()` call, unlike
    /// `send_mouse`'s multi-batch DoubleClick/Drag, so no inter-event timing is
    /// needed here (keyboard also carries no coordinates, so there is no bounds
    /// check). Translation is entirely delegated to
    /// [`crate::input::key_operations`], which is the only code in the crate
    /// allowed to construct `ironrdp_input::Operation`s for keyboard input
    /// (Pitfall 2) — this method never hand-builds a `KeyboardEvent`/
    /// `UnicodeKeyboardEvent`. The typed text / key list is never logged
    /// (Security V5, D-14 redaction parity).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Session`] if the input channel is closed (the session
    /// loop already exited) or the internal input-state lock is poisoned.
    pub async fn send_key(&self, action: KeyAction) -> Result<()> {
        let ops = crate::input::key_operations(&action);

        let events: Vec<FastPathInputEvent> = {
            let mut db = self
                .input_db
                .lock()
                .map_err(|_| Error::Session("input state lock poisoned".to_owned()))?;
            db.apply(ops).into_iter().collect()
        }; // guard dropped here — never held across the .await below

        self.input_tx
            .send(RdpInputEvent::FastPath(events))
            .await
            .map_err(|_| Error::Session("input channel closed".to_owned()))?;

        Ok(())
    }

    /// Round-trip a ping over the `RDPILOT_SENSOR` DVC channel (SENSOR-03,
    /// SC#2/SC#3).
    ///
    /// Fails fast with [`Error::Dvc`] — without sending anything — if the
    /// version handshake previously detected a mismatch (SC#3: a clear
    /// caller-visible error, never silent corruption). Otherwise allocates a
    /// correlation id, registers a `oneshot` reply slot in the shared
    /// [`SensorShared::pending`] map, sends [`RdpInputEvent::Request`] into the
    /// session loop (which builds and writes the outbound bytes via
    /// `ActiveStage::encode_dvc_messages`), and bounds the whole round trip
    /// at 500 ms (SC#2). A timeout removes the now-leaked pending entry so
    /// it cannot be fulfilled later by a stale reply.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dvc`] if the handshake previously mismatched, the
    /// input channel is closed, the session loop closed the reply channel
    /// before responding, or no pong arrives within 500 ms.
    pub async fn ping(&self) -> Result<Duration> {
        {
            let handshake = match self.sensor.handshake.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let crate::sensor::HandshakeState::Mismatched { local, remote } = &*handshake {
                return Err(Error::dvc(format!(
                    "sensor version mismatch: local v{local} vs remote v{remote}"
                )));
            }
        } // guard dropped here — never held across the .await below

        let req_id = self.next_req_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut pending = match self.sensor.pending.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            pending.insert(req_id, tx);
        } // guard dropped here — never held across the .await below

        let started = std::time::Instant::now();

        self.input_tx
            .send(RdpInputEvent::Request(crate::sensor::MsgType::Ping, req_id, None))
            .await
            .map_err(|_| Error::dvc("input channel closed"))?;

        match tokio::time::timeout(Duration::from_millis(500), rx).await {
            // A Pong reply's payload is always empty (Value::Null); ping()
            // only cares that the round trip completed, not the payload.
            Ok(Ok(_payload)) => Ok(started.elapsed()),
            Ok(Err(_recv)) => Err(Error::dvc("sensor channel closed before replying")),
            Err(_elapsed) => {
                let mut pending = match self.sensor.pending.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                pending.remove(&req_id);
                Err(Error::dvc("ping timed out after 500ms"))
            }
        }
    }

    /// Deploy and launch the sensor exe in-band over RDPDR + injected input
    /// (D-5.1/D-5.2, SENSOR-02).
    ///
    /// Opens the Run dialog (Win+R), settles, types the copy-and-start
    /// command referencing the RDPDR-redirected `RDPILOT` drive
    /// ([`launch_command`]), presses Enter, then poll-retries
    /// [`Session::ping`]. On the FIRST successful pong, returns its
    /// round-trip elapsed — this is the SC4 measurement, taken from a
    /// successful launch, **not** from the first keystroke (D-5.2). If no
    /// pong arrives within a launch attempt's poll budget
    /// ([`PINGS_PER_LAUNCH_ATTEMPT`] polls), the Win+R sequence is
    /// re-injected, up to [`LAUNCH_ATTEMPTS`] times total, before giving up.
    /// Runs entirely on the caller's async context (never inside the
    /// session-loop `select!`, Pitfall 3).
    ///
    /// Requires the connect-time `ConnectionConfig::sensor_binary_path` to
    /// have been set (so the RDPDR channel was registered, Task 1) —
    /// callers that skip that configuration will simply never receive a
    /// pong and this call exhausts its retry budget.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Bootstrap`] (T-05-08 — never hangs, never panics) if
    /// no pong arrives after [`LAUNCH_ATTEMPTS`] re-injections, or propagates
    /// [`Error::Session`]/[`Error::Dvc`] immediately if injecting input fails
    /// for a reason unrelated to launch timing (input channel closed, input
    /// lock poisoned).
    pub async fn deploy_and_launch(&self) -> Result<Duration> {
        // One-time settle before the FIRST attempt only (SESSION_SETTLE doc
        // comment has the live-diagnosed root cause: the session can still
        // be mid-transition immediately after connect, silently dropping
        // injected input even though the first framebuffer already arrived).
        tokio::time::sleep(SESSION_SETTLE).await;

        for _attempt in 1..=LAUNCH_ATTEMPTS {
            self.inject_launch_sequence().await?;

            for _poll in 0..PINGS_PER_LAUNCH_ATTEMPT {
                match self.ping().await {
                    Ok(elapsed) => return Ok(elapsed),
                    // A DVC timeout/transient error is expected while the
                    // sensor process is still starting or opening its own
                    // DVC handle (D-5.7) — keep polling within this attempt.
                    Err(Error::Dvc(_)) => continue,
                    // Anything else (channel closed, lock poisoned) is not a
                    // launch-timing condition — surface it immediately.
                    Err(other) => return Err(other),
                }
            }
        }

        Err(Error::bootstrap(format!(
            "sensor did not respond after {LAUNCH_ATTEMPTS} launch attempt(s),              {PINGS_PER_LAUNCH_ATTEMPT} ping polls each — check RDPDR drive              redirection is enabled on the target, AV/EDR is not blocking the              copied exe, and the launch is reaching the correct interactive              session"
        )))
    }

    /// Inject the Win+R launch sequence (D-5.1): open Run, settle, type the
    /// copy-and-start command in small chunks, press Enter. Applied via the
    /// same [`Session::send_key`] path real user input uses — no separate
    /// PDU-building code, no direct `ironrdp_input` construction here.
    ///
    /// Live-tuned (Plan 04 live gate, empirical bug fix): typing the entire
    /// ~113-character command in ONE `send_key(Type(..))` call — one atomic
    /// batch of ~226 Unicode key events delivered in a single FastPath PDU
    /// with no inter-character delay — corrupted the text the Run dialog's
    /// ComboBox autocomplete/MRU-suggestion logic actually submitted (live
    /// evidence: `Get-CimInstance Win32_Process` showed the launched
    /// `cmd.exe`'s actual command line as a spliced/truncated string, e.g.
    /// `cmd /nsor.exet-sensor.exe && start ...`, NOT the intended text —
    /// the visible Run dialog textbox looked correct in a screenshot taken
    /// right after typing, but the autocomplete engine had silently
    /// corrupted the underlying submission). Splitting the string into
    /// small chunks with a short settle between each — mirroring the
    /// manually-bisected sequence that reproduced cleanly during live
    /// diagnosis — gives the ComboBox's autocomplete state machine time to
    /// resolve between bursts and reliably produces the exact intended
    /// command line.
    async fn inject_launch_sequence(&self) -> Result<()> {
        self.send_key(KeyAction::Combo(vec![Key::Win, Key::R])).await?;
        tokio::time::sleep(RUN_DIALOG_SETTLE).await;
        for chunk in chunk_str(&launch_command(), TYPE_CHUNK_LEN) {
            self.send_key(KeyAction::Type(chunk)).await?;
            tokio::time::sleep(TYPE_CHUNK_GAP).await;
        }
        self.send_key(KeyAction::Combo(vec![Key::Enter])).await?;
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
    use crate::input::{Button, Key, KeyAction};

    /// A default desktop size for offline tests that don't care about the
    /// exact value.
    const TEST_DESKTOP_SIZE: (u32, u32) = (1920, 1080);

    fn test_input_db() -> Mutex<Database> {
        Mutex::new(Database::new())
    }

    /// A fresh, unstarted `SensorShared` for offline test `Session` literals
    /// (no VM — the handshake/pending map is never driven by a real
    /// `RdpilotSensorProcessor` here).
    fn test_sensor() -> Arc<SensorShared> {
        Arc::new(SensorShared::new())
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
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
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
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
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
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
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
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
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
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
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

    /// Build a `Session` wired to a real (undrained) channel, so tests can
    /// drain `input_rx` and assert on what `send_mouse` actually sent.
    fn test_session_with_channel(desktop_size: (u32, u32)) -> (Session, mpsc::Receiver<RdpInputEvent>) {
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size,
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
        };
        (session, input_rx)
    }

    /// An in-range `Click` sends exactly one non-empty `FastPath` batch
    /// (SC#2).
    #[tokio::test]
    async fn send_mouse_click_sends_one_nonempty_fastpath_batch() {
        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_mouse(MouseAction::Click {
                x: 100,
                y: 100,
                button: Button::Left,
            })
            .await
            .expect("in-range click succeeds");

        let RdpInputEvent::FastPath(events) = input_rx.try_recv().expect("a FastPath message was sent") else {
            panic!("expected a FastPath event");
        };
        assert!(!events.is_empty());
        assert!(input_rx.try_recv().is_err(), "Click must send exactly one batch");
    }

    /// `Scroll` sends a `FastPath` batch containing a wheel event (D-3.3).
    #[tokio::test]
    async fn send_mouse_scroll_sends_wheel_event() {
        use ironrdp::pdu::input::mouse::PointerFlags;

        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_mouse(MouseAction::Scroll { x: 100, y: 100, dy: 120 })
            .await
            .expect("in-range scroll succeeds");

        let RdpInputEvent::FastPath(events) = input_rx.try_recv().expect("a FastPath message was sent") else {
            panic!("expected a FastPath event");
        };
        assert!(events.iter().any(|e| match e {
            FastPathInputEvent::MouseEvent(pdu) => pdu.flags.contains(PointerFlags::VERTICAL_WHEEL),
            _ => false,
        }));
    }

    /// An out-of-bounds coordinate is rejected before anything is sent on
    /// the channel (D-3.2, SC#4 — "enforced").
    #[tokio::test]
    async fn send_mouse_rejects_out_of_range_coordinate_before_sending() {
        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        let err = session
            .send_mouse(MouseAction::Click {
                x: 9000,
                y: 100,
                button: Button::Left,
            })
            .await;
        assert!(matches!(err, Err(Error::CoordinateOutOfBounds { .. })));
        assert!(input_rx.try_recv().is_err(), "nothing should have been sent");
    }

    /// `DoubleClick` synthesizes two single-click sequences, sent as two
    /// separate `FastPath` messages (D-3.7).
    #[tokio::test]
    async fn send_mouse_double_click_emits_two_fastpath_messages() {
        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_mouse(MouseAction::DoubleClick {
                x: 100,
                y: 100,
                button: Button::Left,
            })
            .await
            .expect("in-range double-click succeeds");

        let mut count = 0;
        while input_rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 2, "DoubleClick must synthesize exactly two batches");
    }

    // --- Task 1: send_key -- Type (Unicode) + Combo (scancode) wiring (INPUT-02, SC#3) ---

    /// `send_key(KeyAction::Type(..))` sends a single non-empty `FastPath`
    /// batch carrying the per-character Unicode operations (D-3.5).
    #[tokio::test]
    async fn send_key_type_sends_nonempty_fastpath_batch() {
        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_key(KeyAction::Type("hi".into()))
            .await
            .expect("Type send succeeds");

        let RdpInputEvent::FastPath(events) = input_rx.try_recv().expect("a FastPath message was sent") else {
            panic!("expected a FastPath event");
        };
        assert!(!events.is_empty());
        assert!(input_rx.try_recv().is_err(), "Type must send exactly one batch");
    }

    /// `send_key(KeyAction::Combo([Ctrl, A]))` sends a single non-empty
    /// `FastPath` batch (scancode down/up, modifier ordering -- D-3.5).
    #[tokio::test]
    async fn send_key_combo_ctrl_a_sends_nonempty_fastpath_batch() {
        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A]))
            .await
            .expect("Ctrl+A combo send succeeds");

        let RdpInputEvent::FastPath(events) = input_rx.try_recv().expect("a FastPath message was sent") else {
            panic!("expected a FastPath event");
        };
        assert!(!events.is_empty());
    }

    /// `send_key(KeyAction::Combo([Alt, F4]))` likewise sends a non-empty
    /// `FastPath` batch (SC#3 -- Alt+F4 expressible).
    #[tokio::test]
    async fn send_key_combo_alt_f4_sends_nonempty_fastpath_batch() {
        let (session, mut input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_key(KeyAction::Combo(vec![Key::Alt, Key::F4]))
            .await
            .expect("Alt+F4 combo send succeeds");

        let RdpInputEvent::FastPath(events) = input_rx.try_recv().expect("a FastPath message was sent") else {
            panic!("expected a FastPath event");
        };
        assert!(!events.is_empty());
    }

    /// A degenerate empty `Combo` returns `Ok` without panicking (API-01,
    /// T-03-11); it may send an empty/no-op batch.
    #[tokio::test]
    async fn send_key_empty_combo_returns_ok_without_panic() {
        let (session, _input_rx) = test_session_with_channel((1920, 1080));

        session
            .send_key(KeyAction::Combo(vec![]))
            .await
            .expect("empty combo must not error");
    }

    // --- Task 2: Session::ping() -- handshake fast-fail + 500ms timeout (SC#2/SC#3) ---

    /// Build a `Session` wired to a real (undrained) channel with a
    /// caller-supplied `SensorShared`, so `ping()` tests can control the
    /// handshake state (no VM — no real `RdpilotSensorProcessor` involved).
    fn test_session_with_sensor(sensor: Arc<SensorShared>) -> (Session, mpsc::Receiver<RdpInputEvent>) {
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
            sensor,
            next_req_id: AtomicU64::new(1),
        };
        (session, input_rx)
    }

    /// A `Mismatched` handshake makes `ping()` fail fast with `Error::Dvc`
    /// and never sends a `Ping` on the input channel (SC#3 — the caller
    /// never even reaches the session loop on a version skew).
    #[tokio::test]
    async fn ping_fast_fails_on_mismatched_handshake_without_sending() {
        let sensor = Arc::new(SensorShared::new());
        {
            let mut handshake = sensor.handshake.lock().expect("lock");
            *handshake = crate::sensor::HandshakeState::Mismatched { local: 1, remote: 2 };
        }
        let (session, mut input_rx) = test_session_with_sensor(sensor);

        let err = session.ping().await;
        assert!(matches!(err, Err(Error::Dvc(_))));
        assert!(
            input_rx.try_recv().is_err(),
            "a Mismatched handshake must fast-fail without sending a Ping"
        );
    }

    /// With no session loop draining the channel (so no pong ever arrives),
    /// `ping()` bounds the wait at 500ms and returns a timeout `Error::Dvc`
    /// (SC#2 bound), rather than hanging forever.
    #[tokio::test]
    async fn ping_times_out_after_500ms_when_no_pong_arrives() {
        let sensor = Arc::new(SensorShared::new());
        let (session, _input_rx) = test_session_with_sensor(sensor);

        let err = session.ping().await;
        match err {
            Err(Error::Dvc(msg)) => assert!(
                msg.contains("500ms") || msg.contains("timed out"),
                "expected a timeout-flavored message, got: {msg}"
            ),
            other => panic!("expected Err(Error::Dvc(_)) mentioning the timeout, got {other:?}"),
        }
    }

    // --- Task 2: deploy_and_launch -- launch-command helper + Error::Bootstrap (D-5.1/D-5.2) ---

    /// The launch command copies the RDPDR-announced sensor exe from the
    /// redirected `RDPILOT` drive to `%TEMP%` and starts it (D-5.1) — pure,
    /// offline-testable, no VM.
    #[test]
    fn launch_command_references_redirected_drive_temp_dest_and_start() {
        let cmd = launch_command();
        assert!(
            cmd.contains(r"\tsclient\RDPILOT"),
            "must reference the RDPDR-redirected drive: {cmd}"
        );
        assert!(cmd.contains("%TEMP%"), "must copy to %TEMP%: {cmd}");
        assert!(cmd.contains("start"), "must start the copied exe: {cmd}");
        assert!(
            cmd.contains(crate::connect::SENSOR_EXE_NAME),
            "must reference the RDPDR-announced sensor filename (Task 1) so the two can never drift apart: {cmd}"
        );
    }

    /// `chunk_str` splits on character boundaries, preserves order and total
    /// content, and never panics on edge cases (empty string, `max_len == 0`,
    /// multi-byte chars) -- the live-diagnosed autocomplete-corruption fix
    /// (see `inject_launch_sequence`'s doc comment) depends on this never
    /// dropping or reordering characters.
    #[test]
    fn chunk_str_preserves_order_and_content() {
        let chunks = chunk_str("abcdefghij", 3);
        assert_eq!(chunks, vec!["abc", "def", "ghi", "j"]);
        assert_eq!(chunks.concat(), "abcdefghij");

        assert!(chunk_str("", 5).is_empty());

        // max_len == 0 must not infinite-loop or panic -- treated as 1.
        let single = chunk_str("xy", 0);
        assert_eq!(single, vec!["x", "y"]);

        // Multi-byte chars: chunk on char boundaries, never split mid-codepoint.
        let unicode = chunk_str("é€", 1);
        assert_eq!(unicode, vec!["é", "€"]);
    }

    /// `deploy_and_launch` propagates a closed-input-channel error from the
    /// Win+R injection step immediately, without hanging or panicking
    /// (T-05-08) — fast (does not wait out the ping poll budget); the full
    /// live inject-then-poll round trip is exercised in Plan 04.
    #[tokio::test]
    async fn deploy_and_launch_propagates_closed_channel_error_without_hanging() {
        let (input_tx, input_rx) = mpsc::channel(1);
        drop(input_rx); // closed channel -- send_key fails immediately, before any ping poll
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
        };

        let err = session.deploy_and_launch().await;
        assert!(matches!(err, Err(Error::Session(_))));
    }
}
