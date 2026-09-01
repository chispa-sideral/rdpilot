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

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ironrdp::pdu::input::fast_path::FastPathInputEvent;
use ironrdp_input::Database;
use tokio::sync::mpsc;

use crate::config::ConnectionConfig;
use crate::BootstrapStage;
use crate::connect;
use crate::error::{Error, Result};
use crate::framebuffer::SharedFrame;
use crate::input::{Key, KeyAction, MouseAction};
use crate::perception::{ProcessInfo, UiaElement, UiaScope, WindowInfo};
use crate::screenshot::Screenshot;
use crate::sensor::SensorShared;
use crate::session_loop::{self, RdpInputEvent};
use crate::worldstate::{UiaMode, WorldState, WorldStateOptions};

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

/// Timeout for the two enumeration requests ([`Session::get_window_list`],
/// [`Session::get_process_tree`]) — wider than [`ACTION_TIMEOUT_MS`] because
/// real remote enumeration work (`EnumWindows` over every top-level window,
/// a full process-snapshot walk) plus a larger JSON reply payload can
/// legitimately take longer than [`Session::ping`]'s trivial no-payload
/// round trip. LIVE-VERIFY tuning target for the Wave 4 gate (06-RESEARCH
/// Pattern 3, Open Question 1) — mirrors how [`RUN_DIALOG_SETTLE`]/
/// [`SESSION_SETTLE`] were empirically tuned in Phase 5.
const ENUMERATION_TIMEOUT_MS: u64 = 2000;

/// Timeout for the two near-instant action requests
/// ([`Session::set_foreground_window`], [`Session::launch_process`]).
/// Mirrors [`Session::ping`]'s existing 500ms bound — also a LIVE-VERIFY
/// tuning target at the Wave 4 gate, kept distinct from
/// [`ENUMERATION_TIMEOUT_MS`] in case live testing shows the two categories
/// need different bounds.
const ACTION_TIMEOUT_MS: u64 = 500;

/// Timeout for [`Session::upload_file`]/[`Session::download_file`]'s
/// `FileTransfer` round trip (D-10.4). Deliberately much wider than
/// [`ENUMERATION_TIMEOUT_MS`]/[`ACTION_TIMEOUT_MS`] because the sensor does
/// not reply until its OWN `FileStream` copy loop (a blocking read/write
/// pass against a potentially multi-MB file, 10-03-SUMMARY.md) has fully
/// completed -- unlike every other sensor-mediated call, whose reply is
/// near-instant relative to the round trip itself.
///
/// **UNVALIDATED initial guess (30s), NOT yet live-tuned** -- this plan
/// (10-04) is offline-only; the live gate is Plan 10-05. This value MUST be
/// exercised against a real multi-MB transfer's actual latency at the
/// 10-05 live gate and adjusted if it proves too tight (a large file over a
/// slow RDP link) or unnecessarily wide -- do NOT treat this constant as
/// settled just because it compiles and offline tests pass, exactly
/// mirroring how [`ENUMERATION_TIMEOUT_MS`]/[`ACTION_TIMEOUT_MS`] were
/// themselves flagged as LIVE-VERIFY tuning targets.
const TRANSFER_TIMEOUT_MS: u64 = 30_000;

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
///
/// LIVE-DIAGNOSED BUG FIX (06-05 live gate): a Windows interactive RDP
/// session is REUSED (reconnected, not recreated) across separate client
/// connections to the same account unless the prior session was explicitly
/// logged off — confirmed live via `quser`/`qwinsta` showing the same
/// session id in state `Disc` after `Session::close()`. If a prior
/// `deploy_and_launch` call already launched a sensor process that is still
/// running when a NEW connection calls `deploy_and_launch` again, the plain
/// `copy` here silently fails (the destination exe is locked by the still-
/// running process) and the chained `&&` short-circuits `start`, so NO new
/// process ever launches — while the OLD process can no longer answer the
/// NEW connection's dynamic virtual channel (its channel handle belongs to
/// the prior, now-closed connection). The result is a hang that exhausts
/// the full launch-attempt/ping-poll retry budget with no pong ever
/// arriving. Fix: unconditionally `taskkill` any already-running instance
/// first (`>nul 2>&1` — a "not found" exit code is expected and harmless on
/// the very first launch in a session) and give the OS a moment to release
/// the file handle before copying, so every call is idempotent regardless
/// of prior launches in the same reused session.
fn launch_command() -> String {
    let name = crate::connect::SENSOR_EXE_NAME;
    format!(
        "cmd /c taskkill /F /IM {name} >nul 2>&1 & timeout /t 1 >nul & \
         cd /d \"%TEMP%\" && copy /Y \"\\\\tsclient\\RDPILOT\\{name}\" \"{name}\" >nul && start \"\" \"{name}\""
    )
}

/// Pure crop-mapping helper behind [`Session::screenshot_window`] (D-6.1),
/// factored out of the async method so it is unit-testable with a synthetic
/// [`Screenshot`] and no live session/loop involved. `window.rect` is
/// already in physical virtual-desktop pixels (the framebuffer coordinate
/// space, guaranteed by the Phase 6 wire contract), so no coordinate remap
/// happens here — this is exactly [`Screenshot::crop`], named for its call
/// site.
fn crop_to_window(shot: Screenshot, window: &WindowInfo) -> Result<Screenshot> {
    shot.crop(window.rect)
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
    /// The configured local file-transfer share root
    /// ([`ConnectionConfig::share_root`], D-10.1), captured once at connect
    /// time — mirrors `desktop_size`'s static-capture rationale. This is
    /// the SAME directory `RdpilotDriveBackend` serves as the
    /// RDPDR-redirected `RDPILOT` drive (`connect.rs`), so
    /// [`Session::upload_file`]/[`Session::download_file`] can stage/
    /// retrieve transfer bytes with a plain `std::fs` call on THIS machine
    /// — no RDPDR IRP round trip is needed for the SDK's own side of the
    /// staging directory, only for the remote Windows machine's side.
    /// `None` when the caller never configured a share root, in which case
    /// `upload_file`/`download_file` fail fast with [`Error::Config`]
    /// before sending anything (Plan 10-04).
    share_root: Option<PathBuf>,
}

/// The outcome of a completed [`Session::upload_file`] or
/// [`Session::download_file`] call (D-10.4).
///
/// An owned, credential-free public type (D-09, D-10.4/T-10-08): no
/// `ironrdp-rdpdr` or other third-party type ever appears here, and nothing
/// beyond the byte count and the VERIFIED checksum is exposed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TransferOutcome {
    /// The number of bytes transferred, as reported by the sensor.
    pub bytes_transferred: u64,
    /// The SHA-256 digest (lowercase hex, no separators), independently
    /// recomputed on the Rust side and CONFIRMED to match the
    /// sensor-reported digest (D-10.5) — never the sensor's raw claim
    /// alone. A divergence never reaches this struct; it surfaces as
    /// [`Error::ChecksumMismatch`] instead.
    pub checksum: String,
}

/// Streams `path` through SHA-256 with a bounded 64KiB buffer (never loads
/// the whole file into memory, T-10-07/T-05-05 discipline) and returns the
/// lowercase hex digest, no separators — matching the C# sensor's own
/// `Convert.ToHexString(...).ToLowerInvariant()` output casing exactly
/// (10-03-SUMMARY.md), so the two independently-computed digests (D-10.5 —
/// neither side trusts the other's reported hash) can be compared
/// byte-for-byte.
///
/// Never a custom hash loop (10-RESEARCH "Don't Hand-Roll") — uses
/// `sha2::Sha256`, the RustCrypto reference implementation already a
/// direct dependency (Plan 10-01).
fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = fs::File::open(path)
        .map_err(|e| Error::dvc(format!("failed to open {} for checksum: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| Error::dvc(format!("failed to read {} while hashing: {e}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
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
            share_root: cfg.get_share_root().map(Path::to_path_buf),
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

    /// Round-trip a generic sensor request, returning the reply's `data` on
    /// `success:true` — the shared plumbing behind
    /// [`Session::get_window_list`], [`Session::get_process_tree`],
    /// [`Session::set_foreground_window`], and [`Session::launch_process`]
    /// (RESEARCH Pattern 3).
    ///
    /// Mirrors [`Session::ping`]'s five-step shape exactly (handshake
    /// fast-fail check, allocate a `req_id`, register a `oneshot` reply slot
    /// in [`SensorShared::pending`], send [`RdpInputEvent::Request`], await
    /// with a `timeout_ms` bound — removing the pending entry on timeout so
    /// a stale late reply cannot resurrect it), and adds the D-6.4
    /// semantic-vs-transport error branch that `ping()` itself does not
    /// need (a `Pong`'s payload carries no `{success,data|error}` envelope).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dvc`] for a handshake mismatch, a closed
    /// input/reply channel, or a timeout (transport failures), and
    /// [`Error::SensorRejected`] when the sensor answered but its reply's
    /// `success` field was `false` (D-6.4).
    async fn sensor_request(
        &self,
        msg_type: crate::sensor::MsgType,
        payload: Option<serde_json::Value>,
        timeout_ms: u64,
    ) -> Result<serde_json::Value> {
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

        self.input_tx
            .send(RdpInputEvent::Request(msg_type, req_id, payload))
            .await
            .map_err(|_| Error::dvc("input channel closed"))?;

        match tokio::time::timeout(Duration::from_millis(timeout_ms), rx).await {
            Ok(Ok(value)) => {
                let success = value
                    .get("success")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                if success {
                    Ok(value.get("data").cloned().unwrap_or(serde_json::Value::Null))
                } else {
                    let reason = value
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown");
                    // D-10.4 BLOCKER fix: a structured `error_kind` sentinel
                    // of "path_traversal" (the sole authoritative producer
                    // is the C# sensor's own `ValidateRemotePath`
                    // rejection, 10-03-SUMMARY.md) surfaces as the distinct
                    // `Error::PathTraversal`, NOT the generic
                    // `Error::SensorRejected` every other semantic
                    // rejection maps to. Other callers (WindowList/Uia/
                    // LaunchProcess) never set `error_kind` on their
                    // replies, so this branch is inert for them — the
                    // fall-through below is byte-for-byte unchanged.
                    let error_kind = value.get("error_kind").and_then(serde_json::Value::as_str);
                    if error_kind == Some("path_traversal") {
                        Err(Error::path_traversal(reason))
                    } else {
                        Err(Error::sensor_rejected(reason))
                    }
                }
            }
            Ok(Err(_recv)) => Err(Error::dvc("sensor channel closed before replying")),
            Err(_elapsed) => {
                let mut pending = match self.sensor.pending.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                pending.remove(&req_id);
                Err(Error::dvc(format!("request timed out after {timeout_ms}ms")))
            }
        }
    }

    /// List all top-level windows on the remote desktop (PERC-02,
    /// SENSOR-backed).
    ///
    /// Round-trips a `WindowList` request over the `RDPILOT_SENSOR` DVC
    /// channel (via [`Session::sensor_request`], mirroring
    /// [`Session::ping`]'s shape) bounded at [`ENUMERATION_TIMEOUT_MS`], and
    /// deserializes the reply's `data` array into owned [`WindowInfo`]
    /// values — the crate-internal `*Wire` structs never leave this crate
    /// (D-09).
    ///
    /// # Errors
    ///
    /// Returns [`Error::SensorRejected`] if the sensor answered but rejected
    /// the request (D-6.4), or [`Error::Dvc`] for a handshake mismatch, a
    /// closed channel, a malformed reply, or a timeout.
    pub async fn get_window_list(&self) -> Result<Vec<WindowInfo>> {
        let data = self
            .sensor_request(crate::sensor::MsgType::WindowList, None, ENUMERATION_TIMEOUT_MS)
            .await?;
        let wires: Vec<crate::perception::WindowInfoWire> =
            serde_json::from_value(data).map_err(|e| Error::dvc(format!("malformed WindowList reply: {e}")))?;
        Ok(wires
            .into_iter()
            .map(crate::perception::WindowInfoWire::into_owned)
            .collect())
    }

    /// List the remote machine's process tree (PROC-01, SENSOR-backed).
    ///
    /// Round-trips a `ProcessTree` request bounded at
    /// [`ENUMERATION_TIMEOUT_MS`] and deserializes the reply's `data` array
    /// into owned [`ProcessInfo`] values (D-09). See
    /// [`Session::get_window_list`] for the shared round-trip/error-branching
    /// shape.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SensorRejected`] if the sensor answered but rejected
    /// the request (D-6.4), or [`Error::Dvc`] for a handshake mismatch, a
    /// closed channel, a malformed reply, or a timeout.
    pub async fn get_process_tree(&self) -> Result<Vec<ProcessInfo>> {
        let data = self
            .sensor_request(crate::sensor::MsgType::ProcessTree, None, ENUMERATION_TIMEOUT_MS)
            .await?;
        let wires: Vec<crate::perception::ProcessInfoWire> =
            serde_json::from_value(data).map_err(|e| Error::dvc(format!("malformed ProcessTree reply: {e}")))?;
        Ok(wires
            .into_iter()
            .map(crate::perception::ProcessInfoWire::into_owned)
            .collect())
    }

    /// Retrieve a flat UI Automation tree for the given window, at a
    /// caller-configurable [`UiaScope`] (PERC-03, D-9.1, SENSOR-backed).
    ///
    /// `scope` maps to the wire `max_depth` field: [`UiaScope::Children`]
    /// sends `max_depth: 1` (the original D-7.4-locked `TreeScope_Children`
    /// walk — unchanged behavior/latency for every existing caller);
    /// [`UiaScope::Subtree { max_depth }`](UiaScope::Subtree) sends that
    /// `max_depth` for the D-9.1 HUMAN-APPROVED flagged deeper walk, which
    /// the C# sensor performs as a bounded, level-by-level
    /// `TreeScope_Children` walk (never an uncapped `TreeScope_Subtree`),
    /// clamped sensor-side to a safety cap to protect the Phase 7 SC#3
    /// 500ms sensor-side walk budget. This supersedes D-7.4's children-only
    /// lock as an explicit, caller-opt-in capability — not a silent
    /// redesign of the UIA subsystem.
    ///
    /// Round-trips a `Uia` request bounded at [`ENUMERATION_TIMEOUT_MS`]
    /// (the transport timeout — distinct from SC#3's 500ms sensor-side walk
    /// budget, which the C# handler must hit and which is verified live,
    /// not by tightening this transport bound) and deserializes the
    /// reply's `data` array into owned [`UiaElement`] values — the
    /// crate-internal `UiaElementWire` never leaves this crate (D-09). See
    /// [`Session::get_window_list`] for the shared round-trip/error-
    /// branching shape.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SensorRejected`] if the sensor answered but rejected
    /// the request (D-6.4), or [`Error::Dvc`] for a handshake mismatch, a
    /// closed channel, a malformed reply, or a timeout.
    pub async fn get_uia_tree(&self, hwnd: u64, scope: UiaScope) -> Result<Vec<UiaElement>> {
        let max_depth: u32 = match scope {
            UiaScope::Children => 1,
            UiaScope::Subtree { max_depth } => max_depth,
        };
        let data = self
            .sensor_request(
                crate::sensor::MsgType::Uia,
                Some(serde_json::json!({ "hwnd": hwnd, "max_depth": max_depth })),
                ENUMERATION_TIMEOUT_MS,
            )
            .await?;
        let wires: Vec<crate::perception::UiaElementWire> =
            serde_json::from_value(data).map_err(|e| Error::dvc(format!("malformed Uia reply: {e}")))?;
        Ok(wires
            .into_iter()
            .map(crate::perception::UiaElementWire::into_owned)
            .collect())
    }

    /// Capture a correlated, timestamped snapshot of the remote desktop
    /// (API-02, SC#2) — the first composite/aggregating `Session` method:
    /// sequences the existing client-side [`Session::screenshot`] and 0-N
    /// sensor round trips ([`Session::get_window_list`],
    /// [`Session::get_uia_tree`]) under one batch timestamp and one
    /// measured capture span.
    ///
    /// - SC#1: only owned SDK types appear in this signature and in
    ///   [`WorldState`] — no `ironrdp`/`image`/sensor-wire type leaks.
    /// - SC#2: the returned [`WorldState`] carries one [`SystemTime`]
    ///   batch timestamp and one measured [`Duration`] `capture_span`
    ///   spanning every sequenced fetch below.
    /// - SC#3: every coordinate ([`Screenshot`] dims, [`WindowInfo::rect`],
    ///   [`UiaElement::bbox`]) is the single shared `crate::Rect` physical
    ///   pixel space — no scaling is introduced here.
    /// - D-8.2/Pitfall 4: `capture_span` exceeding 500ms is a soft,
    ///   best-effort signal surfaced on the returned value — it is NEVER
    ///   compared against 500ms here and never causes a hard failure. A
    ///   genuine transport/semantic error on any sequenced component call
    ///   ([`Error::Dvc`]/[`Error::SensorRejected`]) is a DIFFERENT, hard
    ///   condition: every such call is propagated with `?` and surfaces as
    ///   `Err`, never silently downgraded to a `None` field.
    /// - Pitfall 3: [`UiaMode::Foreground`]/[`UiaMode::AllTopLevel`] both
    ///   need the top-level window list to resolve which window(s) to walk
    ///   — that list is fetched at most once internally whenever needed,
    ///   even if `opts.window_list` is `false`, but [`WorldState::window_list`]
    ///   is only populated when the caller actually asked for it.
    ///
    /// # Errors
    ///
    /// Propagates [`Error::Session`]/[`Error::Decode`] from
    /// [`Session::screenshot`], or [`Error::SensorRejected`]/[`Error::Dvc`]
    /// from any sequenced [`Session::get_window_list`]/
    /// [`Session::get_uia_tree`] call.
    pub async fn world_state(&self, opts: WorldStateOptions) -> Result<WorldState> {
        // Local-only span measurement -- NEVER stored in `WorldState`
        // itself (Pitfall 2: `Instant` has no `Serialize` impl).
        let started = std::time::Instant::now();

        let screenshot = if opts.screenshot {
            Some(self.screenshot().await?)
        } else {
            None
        };

        // Pitfall 3: `Foreground`/`AllTopLevel` both need the window list
        // to resolve which window(s) to walk, even if the caller did not
        // ask for `window_list` itself. Fetch it at most once.
        let needs_list_internally =
            opts.window_list || matches!(opts.uia, UiaMode::Foreground | UiaMode::AllTopLevel);
        let windows = if needs_list_internally {
            Some(self.get_window_list().await?)
        } else {
            None
        };

        let uia = match &opts.uia {
            UiaMode::None => None,
            UiaMode::Hwnd(hwnds) => {
                let mut groups = Vec::with_capacity(hwnds.len());
                for &hwnd in hwnds {
                    groups.push((hwnd, self.get_uia_tree(hwnd, UiaScope::Children).await?));
                }
                Some(groups)
            }
            UiaMode::Foreground => match windows.as_ref() {
                Some(list) => {
                    // The Phase 6 live-diagnosed foreground heuristic:
                    // titled-only (always-on-top untitled shell chrome
                    // otherwise outranks real app windows in raw z-order),
                    // then minimum `z_order` among those.
                    let foreground = list.iter().filter(|w| !w.title.is_empty()).min_by_key(|w| w.z_order);
                    match foreground {
                        Some(w) => Some(vec![(w.hwnd, self.get_uia_tree(w.hwnd, UiaScope::Children).await?)]),
                        None => Some(vec![]),
                    }
                }
                // Unreachable in practice -- `needs_list_internally` always
                // fetches the list for this mode -- but handled without a
                // panic (API-01, SC#4) rather than via unwrap/expect.
                None => Some(vec![]),
            },
            UiaMode::AllTopLevel => match windows.as_ref() {
                Some(list) => {
                    let mut groups = Vec::with_capacity(list.len());
                    for w in list {
                        groups.push((w.hwnd, self.get_uia_tree(w.hwnd, UiaScope::Children).await?));
                    }
                    Some(groups)
                }
                None => Some(vec![]),
            },
        };

        Ok(WorldState {
            timestamp: std::time::SystemTime::now(),
            capture_span: started.elapsed(),
            screenshot,
            // Populated ONLY when the caller asked for it, even though
            // `windows` may have been fetched internally above (Pitfall 3).
            window_list: if opts.window_list { windows } else { None },
            uia,
        })
    }

    /// Bring a remote window to the foreground (PERC-04, SENSOR-backed).
    ///
    /// Round-trips a `SetForegroundWindow` request bounded at
    /// [`ACTION_TIMEOUT_MS`]; the sensor reports `success:true` only if the
    /// underlying `SetForegroundWindow` Win32 call itself succeeded (its
    /// return value was nonzero) — it does not itself verify the visual
    /// outcome (06-RESEARCH). The caller is responsible for confirming the
    /// focus change with a follow-up [`Session::get_window_list`] call,
    /// exactly as ROADMAP Phase 6 SC#3 specifies.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SensorRejected`] if the sensor rejected the request
    /// — e.g. a closed/invalid `hwnd` (D-6.4) — or [`Error::Dvc`] for a
    /// handshake mismatch, a closed channel, or a timeout.
    pub async fn set_foreground_window(&self, hwnd: u64) -> Result<()> {
        self.sensor_request(
            crate::sensor::MsgType::SetForegroundWindow,
            Some(serde_json::json!({ "hwnd": hwnd })),
            ACTION_TIMEOUT_MS,
        )
        .await?;
        Ok(())
    }

    /// Launch a process on the remote machine (PROC-01, D-6.2,
    /// SENSOR-backed).
    ///
    /// Round-trips a `LaunchProcess` request bounded at
    /// [`ACTION_TIMEOUT_MS`] and returns the new process's PID read from the
    /// reply — fire-and-forget (D-6.2): this call does NOT poll for the
    /// process to appear in a later process tree; a caller that wants that
    /// confirmation issues its own follow-up [`Session::get_process_tree`]
    /// call. The exe path/arguments/working directory are operational, not
    /// secret, but this call does not itself add any additional logging
    /// beyond what the caller already holds (ASVS V5).
    ///
    /// # Errors
    ///
    /// Returns [`Error::SensorRejected`] if the sensor rejected the request
    /// — e.g. the exe could not start (D-6.4) — or [`Error::Dvc`] for a
    /// handshake mismatch, a closed channel, a malformed reply, or a
    /// timeout.
    pub async fn launch_process(&self, exe: &str, args: Option<&str>, cwd: Option<&str>) -> Result<u32> {
        let data = self
            .sensor_request(
                crate::sensor::MsgType::LaunchProcess,
                Some(serde_json::json!({ "exe": exe, "args": args, "cwd": cwd })),
                ACTION_TIMEOUT_MS,
            )
            .await?;
        let pid = data
            .get("pid")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| Error::dvc("LaunchProcess reply missing/invalid \"pid\" field"))?;
        u32::try_from(pid).map_err(|_| Error::dvc("LaunchProcess reply \"pid\" exceeds u32 range"))
    }

    /// Upload a local file to a named destination under the sensor's fixed
    /// remote transfer root (FILE-01, D-10.4).
    ///
    /// `remote_name` is relative to the sensor-owned, FIXED
    /// `%TEMP%/rdpilot-transfer-root` directory on the REMOTE machine
    /// (10-03-SUMMARY.md) — it is NEVER an arbitrary absolute path
    /// elsewhere on the remote disk; the sensor's own `ValidateRemotePath`
    /// rejects any candidate that looks rooted before touching
    /// `System.IO` (D-10.2). Passing a value that escapes this transfer
    /// root surfaces as [`Error::PathTraversal`] (the distinct end-to-end
    /// producer this plan wires, D-10.4/BLOCKER fix), never
    /// [`Error::SensorRejected`].
    ///
    /// `local` is copied into the configured
    /// [`ConnectionConfig::share_root`] under a fresh SDK-generated staging
    /// name (never `remote_name` itself, to avoid any collision with a
    /// caller-chosen name) so the RDPDR-redirected `RDPILOT` drive can
    /// serve it to the sensor at `\\tsclient\RDPILOT\<staging-name>`; the
    /// sensor then copies those bytes to the validated `remote_name`
    /// destination (10-RESEARCH architecture diagram).
    ///
    /// Rides the existing async [`Session::sensor_request`] extension point
    /// exactly like [`Session::launch_process`] — no `tokio::spawn`, no
    /// change to the dedicated-OS-thread + current-thread-Tokio session
    /// loop (threading model UNTOUCHABLE).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] if no [`ConnectionConfig::share_root`] was
    /// configured at connect time (nothing is sent in that case);
    /// [`Error::PathTraversal`] if the sensor rejected `remote_name` as
    /// escaping its transfer root; [`Error::SensorRejected`] for any other
    /// sensor-side rejection; [`Error::Dvc`] for a handshake mismatch, a
    /// closed channel, a malformed reply, a timeout, or a local staging I/O
    /// failure; and [`Error::ChecksumMismatch`] if the sensor-reported
    /// SHA-256 of the written remote file does not match the independently
    /// computed digest of `local` (D-10.5).
    pub async fn upload_file(&self, local: &Path, remote_name: &str) -> Result<TransferOutcome> {
        let share_root = self.share_root.as_deref().ok_or_else(|| {
            Error::Config("upload_file requires ConnectionConfig::share_root to be configured".to_owned())
        })?;

        let local_hash = sha256_file(local)?;

        let share_name = self.unique_share_name();
        let staged = share_root.join(&share_name);
        fs::copy(local, &staged).map_err(|e| {
            Error::dvc(format!(
                "failed to stage {} into the share root for upload: {e}",
                local.display()
            ))
        })?;

        let payload = serde_json::json!({
            "op": "Upload",
            "remote_path": remote_name,
            "share_name": share_name,
        });
        let result = self
            .sensor_request(crate::sensor::MsgType::FileTransfer, Some(payload), TRANSFER_TIMEOUT_MS)
            .await;

        // Best-effort cleanup regardless of outcome — never leave the
        // staged copy behind under the share root.
        let _ = fs::remove_file(&staged);

        let data = result?;
        let bytes_transferred = data
            .get("bytes_transferred")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| Error::dvc("FileTransfer reply missing/invalid \"bytes_transferred\" field"))?;
        let sensor_sha256 = data
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::dvc("FileTransfer reply missing/invalid \"sha256\" field"))?
            .to_owned();

        if !local_hash.eq_ignore_ascii_case(&sensor_sha256) {
            return Err(Error::checksum_mismatch(sensor_sha256, local_hash));
        }

        Ok(TransferOutcome {
            bytes_transferred,
            checksum: local_hash,
        })
    }

    /// Download a file from the sensor's fixed remote transfer root to a
    /// local destination (FILE-02, D-10.4).
    ///
    /// `remote_name` is relative to the sensor-owned, FIXED
    /// `%TEMP%/rdpilot-transfer-root` directory on the REMOTE machine
    /// (10-03-SUMMARY.md) — see [`Session::upload_file`]'s doc comment for
    /// the full constraint and the [`Error::PathTraversal`] producer this
    /// shares.
    ///
    /// The sensor copies the validated `remote_name` file to a fresh
    /// SDK-generated staging name under the RDPDR-redirected `RDPILOT`
    /// drive (`\\tsclient\RDPILOT\<staging-name>`), which lands at
    /// `<share_root>/<staging-name>` on THIS machine via the staged-write +
    /// atomic-rename path (Plan 10-02); once the sensor's DVC reply
    /// confirms success, that staged file is moved to the caller-requested
    /// `local` destination with a plain `std::fs` call (no further RDPDR
    /// round trip needed for the SDK's own side).
    ///
    /// Rides the existing async [`Session::sensor_request`] extension point
    /// exactly like [`Session::launch_process`] — no `tokio::spawn`, no
    /// change to the session loop's threading model (UNTOUCHABLE).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] if no [`ConnectionConfig::share_root`] was
    /// configured at connect time (nothing is sent in that case);
    /// [`Error::PathTraversal`] if the sensor rejected `remote_name` as
    /// escaping its transfer root; [`Error::SensorRejected`] for any other
    /// sensor-side rejection; [`Error::Dvc`] for a handshake mismatch, a
    /// closed channel, a malformed reply, a timeout, or a local move/I/O
    /// failure; and [`Error::ChecksumMismatch`] if the sensor-reported
    /// SHA-256 does not match the independently computed digest of the
    /// downloaded local file (D-10.5).
    pub async fn download_file(&self, remote_name: &str, local: &Path) -> Result<TransferOutcome> {
        let share_root = self.share_root.as_deref().ok_or_else(|| {
            Error::Config("download_file requires ConnectionConfig::share_root to be configured".to_owned())
        })?;

        let share_name = self.unique_share_name();
        let staged = share_root.join(&share_name);

        let payload = serde_json::json!({
            "op": "Download",
            "remote_path": remote_name,
            "share_name": share_name,
        });
        let data = self
            .sensor_request(crate::sensor::MsgType::FileTransfer, Some(payload), TRANSFER_TIMEOUT_MS)
            .await?;

        let bytes_transferred = data
            .get("bytes_transferred")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| Error::dvc("FileTransfer reply missing/invalid \"bytes_transferred\" field"))?;
        let sensor_sha256 = data
            .get("sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::dvc("FileTransfer reply missing/invalid \"sha256\" field"))?
            .to_owned();

        // The sensor's own DVC reply only confirms ITS side of the copy
        // completed; the bytes it wrote landed at <share_root>/<share_name>
        // via the staged-write + atomic-rename path (Plan 10-02), not at
        // `local` yet — move them there now with a plain std::fs call.
        if fs::rename(&staged, local).is_err() {
            // Cross-device destinations can't be renamed (EXDEV) — fall
            // back to copy+remove, same net effect.
            fs::copy(&staged, local).map_err(|e| {
                Error::dvc(format!("failed to move the downloaded file to {}: {e}", local.display()))
            })?;
            let _ = fs::remove_file(&staged);
        }

        let local_hash = sha256_file(local)?;
        if !local_hash.eq_ignore_ascii_case(&sensor_sha256) {
            return Err(Error::checksum_mismatch(sensor_sha256, local_hash));
        }

        Ok(TransferOutcome {
            bytes_transferred,
            checksum: local_hash,
        })
    }

    /// Generate a fresh, collision-free filename for staging file-transfer
    /// bytes under the configured [`ConnectionConfig::share_root`] (mirrors
    /// `rdpdr_backend.rs`'s own `allocate_staging_path` naming scheme —
    /// nanosecond timestamp + process id + a monotonic counter, no new
    /// crate dependency). Deliberately distinct from any caller-supplied
    /// `remote_name`/`local` name so two concurrent transfers on the same
    /// `Session` can never collide under the share root.
    fn unique_share_name(&self) -> String {
        let counter = self.next_req_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("rdpilot-transfer-{nanos}-{}-{counter}.tmp", std::process::id())
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
            "sensor did not respond after {LAUNCH_ATTEMPTS} launch attempt(s), {PINGS_PER_LAUNCH_ATTEMPT} ping polls each; stages={}",
            self.bootstrap_summary()
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
        self.sensor.bootstrap.record(BootstrapStage::LaunchInputAttempted);
        self.send_key(KeyAction::Combo(vec![Key::Win, Key::R])).await?;
        tokio::time::sleep(RUN_DIALOG_SETTLE).await;
        for chunk in chunk_str(&launch_command(), TYPE_CHUNK_LEN) {
            self.send_key(KeyAction::Type(chunk)).await?;
            tokio::time::sleep(TYPE_CHUNK_GAP).await;
        }
        self.send_key(KeyAction::Combo(vec![Key::Enter])).await?;
        self.sensor.bootstrap.record(BootstrapStage::LaunchInputSent);
        Ok(())
    }

    /// Snapshot the fixed, redacted bootstrap stages accumulated so far.
    #[must_use]
    pub fn bootstrap_stages(&self) -> Vec<BootstrapStage> {
        self.sensor.bootstrap.snapshot()
    }

    fn bootstrap_summary(&self) -> String {
        let stages = self.bootstrap_stages();
        if stages.is_empty() {
            "none".to_owned()
        } else {
            stages.into_iter().map(BootstrapStage::as_str).collect::<Vec<_>>().join(",")
        }
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

    /// Capture a single window as a cropped screenshot (CAP-02, D-6.1).
    ///
    /// A pure client-side crop of the already-captured desktop framebuffer
    /// (via [`Session::screenshot`]) to `window.rect` — **no sensor round
    /// trip**. `WindowInfo.rect` is already in physical virtual-desktop
    /// pixels (the framebuffer coordinate space, guaranteed by the Phase 6
    /// wire contract), so no coordinate remap happens here.
    ///
    /// **Known limitation (D-6.1):** an occluded or minimized window yields
    /// a clipped, stale, or blank crop, because only the RDP-rendered
    /// desktop frame is available to crop — there is no sensor-side
    /// `PrintWindow`/`BitBlt` capture of the window's own contents.
    /// Sensor-side capture for occluded/minimized windows is deliberately
    /// deferred to the backlog (06-RESEARCH Pattern 3 / D-6.1 locked
    /// decision).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Session`] if no framebuffer has been captured yet
    /// (propagated from [`Session::screenshot`]), or
    /// [`Error::CropOutOfBounds`] if `window.rect` falls outside the
    /// captured framebuffer (an off-screen or stale-geometry window) —
    /// never panics.
    pub async fn screenshot_window(&self, window: &WindowInfo) -> Result<Screenshot> {
        let shot = self.screenshot().await?;
        crop_to_window(shot, window)
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
            share_root: None,
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
            share_root: None,
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
            share_root: None,
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
            share_root: None,
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
            share_root: None,
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
            share_root: None,
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
            share_root: None,
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

    // --- Task 1 (06-02): sensor-backed Session methods -- D-6.4 error branching ---

    /// `get_window_list` on a `success:true` reply deserializes the
    /// wire-contract `data` array into owned `WindowInfo` values with all
    /// fields intact -- exercised through the full `Session` round trip
    /// (`sensor_request` -> deserialize), not just `perception.rs`'s own
    /// wire-shape test.
    #[tokio::test]
    async fn get_window_list_success_returns_owned_windows() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle = tokio::spawn(async move { session.get_window_list().await });

        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert!(matches!(msg_type, crate::sensor::MsgType::WindowList));
        assert!(payload.is_none(), "get_window_list sends no payload");

        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({
            "success": true,
            "data": [{
                "hwnd": 65536,
                "title": "Untitled - Notepad",
                "rect": {"x": 10, "y": 20, "w": 800, "h": 600},
                "z_order": 0,
                "state": "normal",
                "class_name": "Notepad",
                "pid": 4242
            }]
        }))
        .expect("reply delivered before the receiver was dropped");

        let windows = handle
            .await
            .expect("task did not panic")
            .expect("get_window_list succeeds on a success:true reply");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].hwnd, 65536);
        assert_eq!(windows[0].title, "Untitled - Notepad");
        assert_eq!(windows[0].pid, 4242);
    }

    /// `get_window_list` on a `success:false` reply returns
    /// `Error::SensorRejected` whose `Display` carries the sensor's reason
    /// (D-6.4).
    #[tokio::test]
    async fn get_window_list_semantic_failure_returns_sensor_rejected() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle = tokio::spawn(async move { session.get_window_list().await });

        let RdpInputEvent::Request(_msg_type, req_id, _payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({ "success": false, "error": "enum failed" }))
            .expect("reply delivered before the receiver was dropped");

        let err = handle.await.expect("task did not panic");
        match err {
            Err(Error::SensorRejected(msg)) => assert!(msg.contains("enum failed")),
            other => panic!("expected Err(Error::SensorRejected(_)) mentioning the reason, got {other:?}"),
        }
    }

    /// `get_uia_tree(hwnd, UiaScope::Children)` sends `max_depth: 1` on the
    /// wire (the D-7.4 default, unchanged behavior for every existing
    /// caller) -- Plan 09-02 Task 1's payload-shape coverage for the
    /// children-scope mapping.
    #[tokio::test]
    async fn get_uia_tree_children_scope_sends_max_depth_one() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle = tokio::spawn(async move { session.get_uia_tree(65536, UiaScope::Children).await });

        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert!(matches!(msg_type, crate::sensor::MsgType::Uia));
        let payload = payload.expect("get_uia_tree sends a payload");
        assert_eq!(payload["hwnd"], 65536);
        assert_eq!(payload["max_depth"], 1);

        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(canned_uia_reply())
            .expect("reply delivered before the receiver was dropped");

        handle
            .await
            .expect("task did not panic")
            .expect("get_uia_tree succeeds on a success:true reply");
    }

    /// `get_uia_tree(hwnd, UiaScope::Subtree { max_depth: 3 })` sends
    /// `max_depth: 3` on the wire alongside `hwnd` -- Plan 09-02's D-9.1
    /// flagged deeper-walk payload-shape coverage (mirrors the existing
    /// `session.rs` payload-shape test pattern, e.g.
    /// `launch_process_success_returns_pid`'s `payload["exe"]` assertion).
    #[tokio::test]
    async fn get_uia_tree_subtree_scope_sends_requested_max_depth() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle =
            tokio::spawn(async move { session.get_uia_tree(65536, UiaScope::Subtree { max_depth: 3 }).await });

        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert!(matches!(msg_type, crate::sensor::MsgType::Uia));
        let payload = payload.expect("get_uia_tree sends a payload");
        assert_eq!(payload["hwnd"], 65536);
        assert_eq!(payload["max_depth"], 3);

        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(canned_uia_reply())
            .expect("reply delivered before the receiver was dropped");

        handle
            .await
            .expect("task did not panic")
            .expect("get_uia_tree succeeds on a success:true reply");
    }

    /// `launch_process` on a `success:true` reply returns the PID read from
    /// `data.pid` (D-6.2 -- fire-and-forget, no follow-up polling here).
    #[tokio::test]
    async fn launch_process_success_returns_pid() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle = tokio::spawn(async move { session.launch_process("notepad.exe", None, None).await });

        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert!(matches!(msg_type, crate::sensor::MsgType::LaunchProcess));
        assert_eq!(payload.expect("launch_process sends a payload")["exe"], "notepad.exe");

        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({ "success": true, "data": { "pid": 4321 } }))
            .expect("reply delivered before the receiver was dropped");

        let pid = handle
            .await
            .expect("task did not panic")
            .expect("launch_process succeeds on a success:true reply");
        assert_eq!(pid, 4321);
    }

    /// A reply that never arrives bounds the wait at the method's timeout
    /// and returns a transport `Error::Dvc` -- never hangs -- and removes
    /// the pending entry so a stale late reply cannot resurrect it (mirrors
    /// `ping_times_out_after_500ms_when_no_pong_arrives`, extended with the
    /// pending-map-removal assertion). Uses `set_foreground_window`'s
    /// `ACTION_TIMEOUT_MS` (500ms) rather than the wider enumeration bound
    /// to keep the test fast -- the round-trip/timeout plumbing lives in the
    /// single shared `sensor_request` helper, so this exercises the
    /// mechanism common to all four methods.
    #[tokio::test]
    async fn set_foreground_window_times_out_and_removes_pending_entry() {
        let sensor = Arc::new(SensorShared::new());
        let (session, _input_rx) = test_session_with_sensor(sensor.clone());

        let err = session.set_foreground_window(1).await;
        assert!(matches!(err, Err(Error::Dvc(_))));

        let pending = sensor.pending.lock().expect("lock");
        assert!(
            pending.is_empty(),
            "the pending entry must be removed on timeout, not leaked"
        );
    }

    // --- Task 1/2 (10-04): upload_file/download_file -- FileTransfer wiring,
    // error_kind mapping, SHA-256 verification (D-10.4/D-10.5) ---

    /// `sensor_request`'s success:false branch maps a reply carrying
    /// `error_kind:"path_traversal"` to the DISTINCT `Error::PathTraversal`
    /// (D-10.4 BLOCKER fix), never the generic `Error::SensorRejected` --
    /// the direct offline proof of the end-to-end producer this plan wires
    /// (C# `ValidateRemotePath` rejection -> `error_kind` -> here).
    #[tokio::test]
    async fn sensor_request_maps_path_traversal_error_kind_to_distinct_error() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle =
            tokio::spawn(async move { session.sensor_request(crate::sensor::MsgType::FileTransfer, None, TRANSFER_TIMEOUT_MS).await });

        let RdpInputEvent::Request(_, req_id, _) = input_rx.recv().await.expect("a Request was sent") else {
            panic!("expected a Request event");
        };
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({
            "success": false,
            "error": "remote_path failed ancestry validation under the sensor transfer root",
            "error_kind": "path_traversal"
        }))
        .expect("reply delivered before the receiver was dropped");

        let err = handle.await.expect("task did not panic");
        assert!(
            matches!(err, Err(Error::PathTraversal(_))),
            "expected Error::PathTraversal, got {err:?}"
        );
    }

    /// A `success:false` reply with NO `error_kind` (or any value other
    /// than `"path_traversal"`) still maps to `Error::SensorRejected` --
    /// proves the new branch is ADDITIVE, not a blanket reclassification
    /// (WindowList/Uia/LaunchProcess never set `error_kind` and must be
    /// completely unaffected).
    #[tokio::test]
    async fn sensor_request_failure_without_path_traversal_error_kind_still_maps_to_sensor_rejected() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let handle =
            tokio::spawn(async move { session.sensor_request(crate::sensor::MsgType::FileTransfer, None, TRANSFER_TIMEOUT_MS).await });

        let RdpInputEvent::Request(_, req_id, _) = input_rx.recv().await.expect("a Request was sent") else {
            panic!("expected a Request event");
        };
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({ "success": false, "error": "copy failed", "error_kind": "io" }))
            .expect("reply delivered before the receiver was dropped");

        let err = handle.await.expect("task did not panic");
        assert!(
            matches!(err, Err(Error::SensorRejected(_))),
            "expected Error::SensorRejected for a non-path_traversal error_kind, got {err:?}"
        );
    }

    /// Creates a fresh, empty temp directory to use as a file-transfer
    /// `share_root` in offline tests (mirrors `rdpdr_backend.rs`'s own
    /// `nanos + pid` temp-dir naming scheme; no new crate dependency).
    /// Callers are responsible for `fs::remove_dir_all` cleanup.
    fn test_share_root_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut dir = std::env::temp_dir();
        dir.push(format!("rdpilot-session-test-share-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create test share root");
        dir
    }

    /// Build a `Session` wired to a real (undrained) channel with a
    /// caller-supplied `SensorShared` AND a configured `share_root` --
    /// extends `test_session_with_sensor` for the `upload_file`/
    /// `download_file` tests, which need real local `std::fs` I/O against a
    /// share root (no VM involved -- the RDPDR IRP plane is not exercised
    /// by these offline tests, only the DVC control-plane round trip plus
    /// the SDK's own local staging step).
    fn test_session_with_sensor_and_share_root(
        sensor: Arc<SensorShared>,
        share_root: PathBuf,
    ) -> (Session, mpsc::Receiver<RdpInputEvent>) {
        let (input_tx, input_rx) = mpsc::channel(INPUT_CHANNEL_CAPACITY);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
            sensor,
            next_req_id: AtomicU64::new(1),
            share_root: Some(share_root),
        };
        (session, input_rx)
    }

    /// `sha256_file` matches known SHA-256 test vectors (empty input and
    /// `b"abc"`) -- never a custom hash loop (10-RESEARCH "Don't
    /// Hand-Roll").
    #[test]
    fn sha256_file_matches_known_vectors() {
        let dir = test_share_root_dir();

        let empty_path = dir.join("empty.bin");
        fs::write(&empty_path, b"").expect("write empty vector file");
        assert_eq!(
            sha256_file(&empty_path).expect("hash empty file"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let abc_path = dir.join("abc.bin");
        fs::write(&abc_path, b"abc").expect("write abc vector file");
        assert_eq!(
            sha256_file(&abc_path).expect("hash abc file"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// `upload_file` fails fast with `Error::Config` -- and sends nothing --
    /// when no `ConnectionConfig::share_root` was configured at connect
    /// time (D-10.4).
    #[tokio::test]
    async fn upload_file_without_configured_share_root_returns_config_error_without_sending() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor);

        let err = session.upload_file(Path::new("/does/not/matter"), "dest.txt").await;
        assert!(matches!(err, Err(Error::Config(_))));
        assert!(
            input_rx.try_recv().is_err(),
            "must not send a Request without a configured share root"
        );
    }

    /// `download_file` fails fast with `Error::Config` -- and sends nothing
    /// -- when no `ConnectionConfig::share_root` was configured at connect
    /// time (D-10.4), mirroring `upload_file`'s same-shaped precondition
    /// check.
    #[tokio::test]
    async fn download_file_without_configured_share_root_returns_config_error_without_sending() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor);

        let err = session.download_file("remote.txt", Path::new("/does/not/matter")).await;
        assert!(matches!(err, Err(Error::Config(_))));
        assert!(
            input_rx.try_recv().is_err(),
            "must not send a Request without a configured share root"
        );
    }

    /// `upload_file` sends a `FileTransfer` Request with `op:"Upload"`, the
    /// caller's `remote_name` as `remote_path`, and a non-empty generated
    /// `share_name`; on a matching-checksum success reply it returns a
    /// `TransferOutcome` carrying the independently-verified digest
    /// (D-10.4/D-10.5).
    #[tokio::test]
    async fn upload_file_sends_filetransfer_request_and_verifies_checksum_on_success() {
        let sensor = Arc::new(SensorShared::new());
        let share_root = test_share_root_dir();
        let local = share_root.join("local-source.txt");
        fs::write(&local, b"hello upload").expect("write local source");

        let (session, mut input_rx) = test_session_with_sensor_and_share_root(sensor.clone(), share_root.clone());
        let local_for_task = local.clone();

        let handle = tokio::spawn(async move { session.upload_file(&local_for_task, "nested/dest.txt").await });

        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert!(matches!(msg_type, crate::sensor::MsgType::FileTransfer));
        let payload = payload.expect("upload_file sends a payload");
        assert_eq!(payload["op"], "Upload");
        assert_eq!(payload["remote_path"], "nested/dest.txt");
        let share_name = payload["share_name"]
            .as_str()
            .expect("share_name present")
            .to_owned();
        assert!(!share_name.is_empty());
        assert_ne!(share_name, "nested/dest.txt", "the staging name must never reuse the caller's remote_name");

        let expected_hash = sha256_file(&local).expect("hash local source");
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({
            "success": true,
            "data": { "bytes_transferred": 12, "sha256": expected_hash }
        }))
        .expect("reply delivered before the receiver was dropped");

        let outcome = handle
            .await
            .expect("task did not panic")
            .expect("upload_file succeeds on a matching-checksum success reply");
        assert_eq!(outcome.bytes_transferred, 12);
        assert_eq!(outcome.checksum, expected_hash);

        // The staged copy under share_root must be cleaned up, not left behind.
        assert!(!share_root.join(&share_name).exists());

        let _ = fs::remove_dir_all(&share_root);
    }

    /// `upload_file` returns the distinct `Error::ChecksumMismatch` (never
    /// `Error::SensorRejected`/`Error::Dvc`) when the sensor-reported
    /// SHA-256 does not match the independently-computed digest of the
    /// local source file (D-10.5).
    #[tokio::test]
    async fn upload_file_checksum_mismatch_returns_distinct_error() {
        let sensor = Arc::new(SensorShared::new());
        let share_root = test_share_root_dir();
        let local = share_root.join("local-source.txt");
        fs::write(&local, b"hello upload").expect("write local source");

        let (session, mut input_rx) = test_session_with_sensor_and_share_root(sensor.clone(), share_root.clone());
        let local_for_task = local.clone();

        let handle = tokio::spawn(async move { session.upload_file(&local_for_task, "dest.txt").await });

        let RdpInputEvent::Request(_, req_id, _) = input_rx.recv().await.expect("a Request was sent") else {
            panic!("expected a Request event");
        };
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({
            "success": true,
            "data": { "bytes_transferred": 12, "sha256": "0".repeat(64) }
        }))
        .expect("reply delivered before the receiver was dropped");

        let err = handle.await.expect("task did not panic");
        assert!(
            matches!(err, Err(Error::ChecksumMismatch { .. })),
            "expected Error::ChecksumMismatch, got {err:?}"
        );

        let _ = fs::remove_dir_all(&share_root);
    }

    /// A `FileTransfer` success reply missing `bytes_transferred`/`sha256`
    /// surfaces as `Error::Dvc` with a clear message -- mirrors
    /// `launch_process`'s missing-`"pid"` handling.
    #[tokio::test]
    async fn upload_file_malformed_reply_missing_fields_returns_dvc_error() {
        let sensor = Arc::new(SensorShared::new());
        let share_root = test_share_root_dir();
        let local = share_root.join("local-source.txt");
        fs::write(&local, b"hello upload").expect("write local source");

        let (session, mut input_rx) = test_session_with_sensor_and_share_root(sensor.clone(), share_root.clone());
        let local_for_task = local.clone();

        let handle = tokio::spawn(async move { session.upload_file(&local_for_task, "dest.txt").await });

        let RdpInputEvent::Request(_, req_id, _) = input_rx.recv().await.expect("a Request was sent") else {
            panic!("expected a Request event");
        };
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({ "success": true, "data": {} }))
            .expect("reply delivered before the receiver was dropped");

        let err = handle.await.expect("task did not panic");
        assert!(matches!(err, Err(Error::Dvc(_))), "expected Error::Dvc, got {err:?}");

        let _ = fs::remove_dir_all(&share_root);
    }

    /// `download_file` sends a `FileTransfer` Request with `op:"Download"`
    /// and the caller's `remote_name` as `remote_path`; once the sensor's
    /// reply confirms success, the bytes already staged (by Plan 10-02's
    /// staged-write path, simulated here) at
    /// `<share_root>/<share_name>` are moved to the caller-requested
    /// destination and the independently-recomputed checksum matches
    /// (D-10.4/D-10.5).
    #[tokio::test]
    async fn download_file_sends_filetransfer_request_and_moves_staged_file_to_destination() {
        let sensor = Arc::new(SensorShared::new());
        let share_root = test_share_root_dir();
        let dest_dir = test_share_root_dir(); // an independent temp dir standing in for an arbitrary caller destination
        let dest = dest_dir.join("downloaded.txt");

        let (session, mut input_rx) = test_session_with_sensor_and_share_root(sensor.clone(), share_root.clone());
        let dest_for_task = dest.clone();

        let handle = tokio::spawn(async move { session.download_file("remote/file.txt", &dest_for_task).await });

        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert!(matches!(msg_type, crate::sensor::MsgType::FileTransfer));
        let payload = payload.expect("download_file sends a payload");
        assert_eq!(payload["op"], "Download");
        assert_eq!(payload["remote_path"], "remote/file.txt");
        let share_name = payload["share_name"]
            .as_str()
            .expect("share_name present")
            .to_owned();

        // Simulate the RDPDR staged-write path (Plan 10-02) having already
        // deposited the downloaded bytes at <share_root>/<share_name> by
        // the time the sensor's DVC reply arrives.
        let staged_path = share_root.join(&share_name);
        fs::write(&staged_path, b"downloaded content").expect("simulate staged write");
        let expected_hash = sha256_file(&staged_path).expect("hash staged file");

        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({
            "success": true,
            "data": { "bytes_transferred": 19, "sha256": expected_hash }
        }))
        .expect("reply delivered before the receiver was dropped");

        let outcome = handle
            .await
            .expect("task did not panic")
            .expect("download_file succeeds on a matching-checksum success reply");
        assert_eq!(outcome.bytes_transferred, 19);
        assert_eq!(outcome.checksum, expected_hash);
        assert_eq!(
            fs::read(&dest).expect("destination file was written"),
            b"downloaded content"
        );
        assert!(
            !staged_path.exists(),
            "the staged file must be moved to the destination, not left behind"
        );

        let _ = fs::remove_dir_all(&share_root);
        let _ = fs::remove_dir_all(&dest_dir);
    }

    /// `download_file` returns the distinct `Error::ChecksumMismatch` when
    /// the sensor-reported SHA-256 does not match the independently
    /// computed digest of the downloaded local file (D-10.5).
    #[tokio::test]
    async fn download_file_checksum_mismatch_returns_distinct_error() {
        let sensor = Arc::new(SensorShared::new());
        let share_root = test_share_root_dir();
        let dest_dir = test_share_root_dir();
        let dest = dest_dir.join("downloaded.txt");

        let (session, mut input_rx) = test_session_with_sensor_and_share_root(sensor.clone(), share_root.clone());
        let dest_for_task = dest.clone();

        let handle = tokio::spawn(async move { session.download_file("remote/file.txt", &dest_for_task).await });

        let RdpInputEvent::Request(_, req_id, payload) = input_rx.recv().await.expect("a Request was sent") else {
            panic!("expected a Request event");
        };
        let share_name = payload.expect("download_file sends a payload")["share_name"]
            .as_str()
            .expect("share_name present")
            .to_owned();
        let staged_path = share_root.join(&share_name);
        fs::write(&staged_path, b"downloaded content").expect("simulate staged write");

        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(serde_json::json!({
            "success": true,
            "data": { "bytes_transferred": 19, "sha256": "0".repeat(64) }
        }))
        .expect("reply delivered before the receiver was dropped");

        let err = handle.await.expect("task did not panic");
        assert!(
            matches!(err, Err(Error::ChecksumMismatch { .. })),
            "expected Error::ChecksumMismatch, got {err:?}"
        );

        let _ = fs::remove_dir_all(&share_root);
        let _ = fs::remove_dir_all(&dest_dir);
    }

    /// `upload_file`'s round trip is bounded at `TRANSFER_TIMEOUT_MS`
    /// (30s) -- mirrors `set_foreground_window_times_out_and_removes_pending_entry`,
    /// but uses Tokio's paused/mocked clock (`start_paused = true`) instead
    /// of a real 30-second wall-clock wait, so this offline test stays fast
    /// while still proving `upload_file` passes `TRANSFER_TIMEOUT_MS` (not
    /// some other bound) into the shared `sensor_request` helper -- a
    /// deliberately UNVALIDATED initial guess (see `TRANSFER_TIMEOUT_MS`'s
    /// doc comment) that must be live-tuned at the 10-05 gate.
    #[tokio::test(start_paused = true)]
    async fn upload_file_times_out_after_transfer_timeout_and_removes_pending_entry() {
        let sensor = Arc::new(SensorShared::new());
        let share_root = test_share_root_dir();
        let local = share_root.join("local-source.txt");
        fs::write(&local, b"hello").expect("write local source");

        let (session, _input_rx) = test_session_with_sensor_and_share_root(sensor.clone(), share_root.clone());

        let err = session.upload_file(&local, "dest.txt").await;
        assert!(matches!(err, Err(Error::Dvc(_))), "expected a timeout Error::Dvc, got {err:?}");

        let pending = sensor.pending.lock().expect("lock");
        assert!(pending.is_empty(), "the pending entry must be removed on timeout, not leaked");
        drop(pending);

        let _ = fs::remove_dir_all(&share_root);
    }

    // --- Task 2 (08-02): world_state() -- composite snapshot aggregation ---

    /// Drain the next `RdpInputEvent::Request` off `input_rx`, assert its
    /// `msg_type`, and fulfil it with `reply` via the sensor's pending
    /// oneshot map -- the shared drive-one-round-trip step every
    /// `world_state()` test below repeats once per sequenced sensor call
    /// (mirrors `get_window_list_success_returns_owned_windows`'s manual
    /// steps, factored out to keep multi-request tests readable).
    async fn drive_one_request(
        sensor: &Arc<SensorShared>,
        input_rx: &mut mpsc::Receiver<RdpInputEvent>,
        expected: crate::sensor::MsgType,
        reply: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let RdpInputEvent::Request(msg_type, req_id, payload) =
            input_rx.recv().await.expect("a Request was sent")
        else {
            panic!("expected a Request event");
        };
        assert_eq!(msg_type, expected);
        let tx = {
            let mut pending = sensor.pending.lock().expect("lock");
            pending.remove(&req_id).expect("req_id registered in pending map")
        };
        tx.send(reply).expect("reply delivered before the receiver was dropped");
        payload
    }

    /// A canned `WindowList` success reply with the given `(hwnd, title,
    /// z_order)` triples, used across the `world_state()` tests below.
    fn canned_window_list_reply(windows: &[(u64, &str, u32)]) -> serde_json::Value {
        let data: Vec<serde_json::Value> = windows
            .iter()
            .map(|(hwnd, title, z_order)| {
                serde_json::json!({
                    "hwnd": hwnd,
                    "title": title,
                    "rect": {"x": 0, "y": 0, "w": 100, "h": 100},
                    "z_order": z_order,
                    "state": "normal",
                    "class_name": "Some",
                    "pid": 1
                })
            })
            .collect();
        serde_json::json!({ "success": true, "data": data })
    }

    /// A canned `Uia` success reply with a single element, used across the
    /// `world_state()` tests below (contents are not asserted in detail --
    /// only that a group was produced per hwnd).
    fn canned_uia_reply() -> serde_json::Value {
        serde_json::json!({
            "success": true,
            "data": [{
                "runtime_id": [1],
                "control_type": 50000,
                "name": "OK",
                "bbox": {"x": 0, "y": 0, "w": 10, "h": 10},
                "enabled": true,
                "visible": true,
                "focusable": true,
                "focused": false,
                "depth": 0,
                "parent_runtime_id": []
            }]
        })
    }

    /// `world_state(WorldStateOptions::default())` yields
    /// `screenshot: Some`, `window_list: Some`, `uia: None`, with a
    /// populated `capture_span` (SC#2 default is SC#2-compliant, D-8.1).
    #[tokio::test]
    async fn world_state_default_options_returns_screenshot_and_window_list_no_uia() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());
        session.frame.write(2, 2, vec![255u8; 2 * 2 * 4]);

        let handle = tokio::spawn(async move { session.world_state(WorldStateOptions::default()).await });

        drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::WindowList,
            canned_window_list_reply(&[(111, "Notepad", 0)]),
        )
        .await;

        let world = handle
            .await
            .expect("task did not panic")
            .expect("world_state succeeds with default options");

        let shot = world.screenshot.expect("default options request a screenshot");
        assert_eq!((shot.width, shot.height), (2, 2));
        let windows = world.window_list.expect("default options request the window list");
        assert_eq!(windows.len(), 1);
        assert!(world.uia.is_none(), "default options request no UIA");
        assert!(world.capture_span >= Duration::ZERO);
    }

    /// Pitfall 3: `{screenshot:false, window_list:false, uia:Foreground}`
    /// against a mocked window list containing an untitled top window and
    /// two titled windows returns `window_list: None` (not requested) yet
    /// `uia: Some(vec![(hwnd, elements)])` for the titled window with the
    /// minimum `z_order` -- proving the list was fetched internally (one
    /// `WindowList` request observed) but never surfaced, and that the
    /// untitled always-on-top window is excluded from foreground selection.
    #[tokio::test]
    async fn world_state_foreground_uia_fetches_list_internally_but_hides_it() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let opts = WorldStateOptions {
            screenshot: false,
            window_list: false,
            uia: UiaMode::Foreground,
        };
        let handle = tokio::spawn(async move { session.world_state(opts).await });

        drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::WindowList,
            canned_window_list_reply(&[
                (999, "", 0),                // untitled, topmost by raw z_order -- must be excluded
                (111, "Background Window", 5),
                (222, "Foreground Window", 2), // minimum z_order among titled windows
            ]),
        )
        .await;

        let payload = drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::Uia,
            canned_uia_reply(),
        )
        .await;
        assert_eq!(
            payload.expect("get_uia_tree sends a payload")["hwnd"],
            222,
            "the foreground selection must pick the titled window with the minimum z_order"
        );

        let world = handle.await.expect("task did not panic").expect("world_state succeeds");
        assert!(
            world.window_list.is_none(),
            "window_list was not requested, so it must stay None even though fetched internally"
        );
        let uia = world.uia.expect("Foreground mode requests a UIA group");
        assert_eq!(uia.len(), 1);
        assert_eq!(uia[0].0, 222);
        assert_eq!(uia[0].1.len(), 1);
    }

    /// `AllTopLevel` with 2+ mocked windows: `uia` contains one
    /// `(hwnd, Vec<UiaElement>)` entry per window in the list, in list
    /// order; `window_list` is populated because it was also requested,
    /// reusing the single internally-fetched list for both purposes.
    #[tokio::test]
    async fn world_state_all_top_level_uia_covers_every_window_in_order() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let opts = WorldStateOptions {
            screenshot: false,
            window_list: true,
            uia: UiaMode::AllTopLevel,
        };
        let handle = tokio::spawn(async move { session.world_state(opts).await });

        drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::WindowList,
            canned_window_list_reply(&[(111, "First", 0), (222, "Second", 1)]),
        )
        .await;

        let payload_1 = drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::Uia,
            canned_uia_reply(),
        )
        .await;
        assert_eq!(payload_1.expect("payload present")["hwnd"], 111);

        let payload_2 = drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::Uia,
            canned_uia_reply(),
        )
        .await;
        assert_eq!(payload_2.expect("payload present")["hwnd"], 222);

        let world = handle.await.expect("task did not panic").expect("world_state succeeds");
        assert_eq!(world.window_list.expect("window_list was requested").len(), 2);
        let uia = world.uia.expect("AllTopLevel requests a UIA group per window");
        assert_eq!(uia.len(), 2);
        assert_eq!(uia[0].0, 111);
        assert_eq!(uia[1].0, 222);
    }

    /// `UiaMode::Hwnd(vec![h1, h2])` fetches a UIA group per hwnd, in
    /// order, WITHOUT ever fetching the window list (no `WindowList`
    /// request observed); `window_list` stays `None` because it was not
    /// requested.
    #[tokio::test]
    async fn world_state_hwnd_uia_fetches_group_per_handle_without_list() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());

        let opts = WorldStateOptions {
            screenshot: false,
            window_list: false,
            uia: UiaMode::Hwnd(vec![111, 222]),
        };
        let handle = tokio::spawn(async move { session.world_state(opts).await });

        let payload_1 = drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::Uia,
            canned_uia_reply(),
        )
        .await;
        assert_eq!(payload_1.expect("payload present")["hwnd"], 111);

        let payload_2 = drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::Uia,
            canned_uia_reply(),
        )
        .await;
        assert_eq!(payload_2.expect("payload present")["hwnd"], 222);

        let world = handle.await.expect("task did not panic").expect("world_state succeeds");
        assert!(world.window_list.is_none(), "Hwnd mode never fetches the window list");
        let uia = world.uia.expect("Hwnd mode requests a UIA group per handle");
        assert_eq!(uia.len(), 2);
        assert_eq!(uia[0].0, 111);
        assert_eq!(uia[1].0, 222);
    }

    /// Pitfall 4: a mocked `success:false` reply on a sequenced component
    /// call (here, the internally-fetched `WindowList` under default
    /// options) makes `world_state()` return `Err(Error::SensorRejected)`,
    /// NOT `Ok` with a `None` field -- the hard-error/soft-timing
    /// distinction (D-8.2).
    #[tokio::test]
    async fn world_state_error_propagates_as_err_not_none() {
        let sensor = Arc::new(SensorShared::new());
        let (session, mut input_rx) = test_session_with_sensor(sensor.clone());
        session.frame.write(2, 2, vec![255u8; 2 * 2 * 4]);

        let handle = tokio::spawn(async move { session.world_state(WorldStateOptions::default()).await });

        drive_one_request(
            &sensor,
            &mut input_rx,
            crate::sensor::MsgType::WindowList,
            serde_json::json!({ "success": false, "error": "enum failed" }),
        )
        .await;

        let err = handle.await.expect("task did not panic");
        match err {
            Err(Error::SensorRejected(msg)) => assert!(msg.contains("enum failed")),
            other => panic!("expected Err(Error::SensorRejected(_)) mentioning the reason, got {other:?}"),
        }
    }

    // --- Task 2 (06-02): screenshot_window -- D-6.1 client-side per-window crop ---

    /// Build a `w x h` synthetic screenshot whose pixel (x,y) encodes
    /// R=x, G=y, B=x+y, A=255 -- mirrors `screenshot.rs`'s own `checkerboard`
    /// helper so subpixel assertions are exact.
    fn checkerboard(w: u32, h: u32) -> Screenshot {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.push(x as u8);
                rgba.push(y as u8);
                rgba.push((x + y) as u8);
                rgba.push(255);
            }
        }
        Screenshot::from_rgba(w, h, rgba).expect("valid buffer")
    }

    /// A `WindowInfo` used only for its `rect` -- the other fields are
    /// irrelevant to `crop_to_window`/`screenshot_window`, filled with cheap
    /// placeholders.
    fn test_window(rect: crate::Rect) -> WindowInfo {
        WindowInfo {
            hwnd: 1,
            title: String::new(),
            rect,
            z_order: 0,
            state: crate::WindowState::Normal,
            class_name: String::new(),
            pid: 1,
        }
    }

    /// An in-bounds window rect crops to the exact dims and origin subpixel
    /// (D-6.1) -- exercises the pure `crop_to_window` helper with no live
    /// session/loop.
    #[test]
    fn crop_to_window_in_bounds_matches_rect_and_origin_pixel() {
        let shot = checkerboard(10, 8);
        let window = test_window(crate::Rect { x: 2, y: 3, w: 4, h: 2 });

        let cropped = crop_to_window(shot, &window).expect("in-bounds crop succeeds");
        assert_eq!(cropped.width, 4);
        assert_eq!(cropped.height, 2);
        // Origin pixel of the crop is source (x=2, y=3): R=2, G=3, B=5, A=255.
        assert_eq!(&cropped.rgba[0..4], &[2, 3, 5, 255]);
    }

    /// A window rect that exceeds the framebuffer returns
    /// `Error::CropOutOfBounds`, never panics -- the documented D-6.1
    /// occlusion/off-screen limitation surfaces as a typed error, not a
    /// crash.
    #[test]
    fn crop_to_window_out_of_bounds_returns_typed_error() {
        let shot = checkerboard(10, 8);
        let window = test_window(crate::Rect { x: 8, y: 0, w: 10, h: 1 });

        let err = crop_to_window(shot, &window);
        assert!(matches!(err, Err(Error::CropOutOfBounds { .. })));
    }

    /// `screenshot_window` wires `Session::screenshot` into `crop_to_window`
    /// end-to-end: a session with a seeded frame and an in-bounds window
    /// rect returns the correctly-sized crop (D-6.1) and never touches the
    /// sensor DVC channel (no round trip).
    #[tokio::test]
    async fn screenshot_window_crops_the_captured_framebuffer_without_a_sensor_round_trip() {
        let (input_tx, mut input_rx) = mpsc::channel(1);
        let frame = SharedFrame::new();
        frame.write(4, 4, vec![7u8; 4 * 4 * 4]);
        let session = Session {
            thread: None,
            input_tx,
            frame,
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
            sensor: test_sensor(),
            next_req_id: AtomicU64::new(1),
            share_root: None,
        };
        let window = test_window(crate::Rect { x: 1, y: 1, w: 2, h: 2 });

        let shot = session
            .screenshot_window(&window)
            .await
            .expect("in-bounds window crop succeeds");
        assert_eq!(shot.width, 2);
        assert_eq!(shot.height, 2);
        assert!(
            input_rx.try_recv().is_err(),
            "screenshot_window must not touch the sensor DVC channel (D-6.1)"
        );
    }

    // --- Task 2: deploy_and_launch -- launch-command helper + Error::Bootstrap (D-5.1/D-5.2) ---

    /// The launch command changes to `%TEMP%`, copies the RDPDR-announced
    /// sensor exe from the redirected `RDPILOT` drive, and starts it (D-5.1)
    /// — pure, offline-testable, no VM.
    #[test]
    fn launch_command_references_redirected_drive_temp_dest_and_start() {
        let cmd = launch_command();
        assert!(
            cmd.contains(r"\tsclient\RDPILOT"),
            "must reference the RDPDR-redirected drive: {cmd}"
        );
        assert!(cmd.contains("%TEMP%"), "must copy to %TEMP%: {cmd}");
        assert!(cmd.contains(r#"cd /d "%TEMP%""#), "TEMP must be quoted: {cmd}");
        assert!(
            cmd.contains(r#""\\tsclient\RDPILOT\rdpilot-sensor.exe" "rdpilot-sensor.exe""#),
            "redirected source and destination must be quoted: {cmd}"
        );
        assert!(cmd.contains(r#"start "" "rdpilot-sensor.exe""#), "started executable must be quoted: {cmd}");
        assert!(cmd.contains("start"), "must start the copied exe: {cmd}");
        assert!(
            cmd.contains(crate::connect::SENSOR_EXE_NAME),
            "must reference the RDPDR-announced sensor filename (Task 1) so the two can never drift apart: {cmd}"
        );
    }

    /// The Run dialog/ShellExecute path has a MAX_PATH-sized command buffer.
    /// Keep this below that limit after expanding a long Crabbox-style TEMP
    /// path, so its trailing executable name cannot be truncated.
    #[test]
    fn launch_command_fits_run_dialog_limit_with_long_temp_path() {
        const WINDOWS_RUN_COMMAND_LIMIT: usize = 260;
        const LONG_CRABBOX_TEMP: &str =
            r"C:\Users\crabbox.very-long-machine-name\AppData\Local\Temp\2";

        let expanded = launch_command().replace("%TEMP%", LONG_CRABBOX_TEMP);

        assert!(
            expanded.len() < WINDOWS_RUN_COMMAND_LIMIT,
            "expanded launch command is {} bytes, but must stay below the {WINDOWS_RUN_COMMAND_LIMIT}-byte Run/ShellExecute limit: {expanded}",
            expanded.len(),
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
            share_root: None,
        };

        let err = session.deploy_and_launch().await;
        assert!(matches!(err, Err(Error::Session(_))));
        assert_eq!(session.bootstrap_stages(), vec![BootstrapStage::LaunchInputAttempted]);
    }

    #[test]
    fn bootstrap_summary_is_a_compact_redacted_stage_vector() {
        let sensor = test_sensor();
        sensor.bootstrap.record(BootstrapStage::RdpdrFileAccess);
        sensor.bootstrap.record(BootstrapStage::LaunchInputSent);
        let (input_tx, _input_rx) = mpsc::channel(1);
        let session = Session {
            thread: None,
            input_tx,
            frame: SharedFrame::new(),
            input_db: test_input_db(),
            desktop_size: TEST_DESKTOP_SIZE,
            sensor,
            next_req_id: AtomicU64::new(1),
            share_root: None,
        };
        assert_eq!(session.bootstrap_summary(), "rdpdr_file_access,launch_input_sent");
    }
}
