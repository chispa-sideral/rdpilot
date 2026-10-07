//! The SDK-owned active-session pump (SESS-02, CAP-01, D-04).
//!
//! Mirrors `ironrdp-client/src/rdp.rs::active_session`: split the framed transport
//! into an independent reader/writer, own the only mutating [`DecodedImage`], and
//! `tokio::select!` over three sources:
//!   1. inbound PDUs (`reader.read_pdu()`) → `ActiveStage::process`,
//!   2. control/input events from the [`Session`] (`input_rx.recv()`),
//!   3. the keepalive timer (`keepalive_interval.tick()`).
//!
//! Outputs are dispatched: `ResponseFrame` is written back, `GraphicsUpdate`
//! snapshots `image.data()` into the [`SharedFrame`] (the integrity contract —
//! `screenshot()` reads that snapshot, never the live image, Pitfall 3),
//! `DeactivateAll` runs the reactivation sequence that rebuilds the framebuffer
//! at the new desktop size (Pitfall 2, criterion #4), and `Terminate` exits the
//! loop. `Pointer*` variants are ignored (`enable_server_pointer: false`).
//!
//! The module is named `session_loop` (NOT `loop`, a reserved keyword). No
//! `unwrap`/`expect`/`panic` in non-test code (API-01).

use ironrdp::connector::connection_activation::{
    ConnectionActivationSequence, ConnectionActivationState,
};
use ironrdp::core::WriteBuf;
use ironrdp::graphics::image_processing::PixelFormat;
use ironrdp::session::fast_path;
use ironrdp::session::image::DecodedImage;
use ironrdp::session::{ActiveStage, ActiveStageOutput};
use ironrdp_tokio::{Framed, FramedWrite};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, trace};

use crate::connect::{ConnectedFramed, UpgradedStream};
use crate::error::{Error, Result};
use crate::framebuffer::SharedFrame;
use crate::keepalive::{null_input_event, KEEPALIVE_INTERVAL};

/// Control/input events the [`Session`](crate::Session) sends into the loop.
///
/// `Close` requests a graceful client-side shutdown; the keepalive is emitted
/// from its own `select!` arm (no self-send through the channel). `FastPath`
/// (Phase 3, input injection) carries a pre-built batch of
/// [`FastPathInputEvent`]s — constructed by `Session::send_mouse`/`send_key`
/// via `ironrdp_input::Database::apply` — that this loop only forwards to
/// `process_fastpath_input`; the loop never constructs input events itself and
/// never sleeps for inter-event timing (Pitfall 3).
pub(crate) enum RdpInputEvent {
    /// Begin a graceful client-side shutdown (sent by `Session::close`).
    Close,
    /// A pre-built batch of fast-path input events to forward to the active
    /// stage, unchanged, in the same order.
    FastPath(Vec<ironrdp::pdu::input::fast_path::FastPathInputEvent>),
    /// A proactive typed request on the `RDPILOT_BRIDGE` DVC channel
    /// (BRIDGE-03, SC#2; generalized Phase 6, RESEARCH Pattern 2), carrying
    /// the message type, the correlation `req_id` the caller allocated, and
    /// an optional JSON payload. Unlike `FastPath`, this loop *builds* the
    /// outbound bytes itself (via `ActiveStage::get_dvc` +
    /// `BridgeProcessor::encode_request` +
    /// `ironrdp_dvc::encode_dvc_messages`) rather than merely forwarding a
    /// pre-built payload — `DvcProcessor::start()`/`process()` are reactive
    /// only and cannot originate a send on their own (RESEARCH Q1).
    Request(rdpilot_bridge_protocol::Envelope),
}

/// Run the active-session pump until the connection terminates or the input
/// channel closes.
///
/// Owns `framed` (the TLS-upgraded transport), the input channel receiver, and a
/// write handle to the shared frame. Returns `Ok(())` on a clean terminate /
/// channel-close, or an [`Error`] if a PDU could not be processed or written.
pub(crate) async fn run(
    framed: ConnectedFramed,
    connection_result: ironrdp::connector::ConnectionResult,
    mut input_rx: mpsc::Receiver<RdpInputEvent>,
    frame: SharedFrame,
    bridge: std::sync::Arc<crate::bridge::BridgeShared>,
) -> Result<()> {
    let (mut reader, mut writer) = ironrdp_tokio::split_tokio_framed(framed);

    let mut image = DecodedImage::new(
        PixelFormat::RgbA32,
        connection_result.desktop_size.width,
        connection_result.desktop_size.height,
    );

    let mut active_stage = ActiveStage::new(connection_result);

    let mut keepalive = interval(KEEPALIVE_INTERVAL);
    // If a tick is missed (e.g. the loop was busy), fire once and resync rather
    // than bursting catch-up ticks.
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first tick fires immediately; consume it so the keepalive does not emit
    // an event the instant the session starts.
    keepalive.tick().await;
    let mut bridge_ping = interval(std::time::Duration::from_secs(10));
    bridge_ping.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        let outputs = tokio::select! {
            biased;
            _=bridge.shutdown.notified()=>break,
            _=bridge.retire_ready.notified()=>{
                match bridge.take_retire(){Some(e)=>match build_request_frame(&mut active_stage,e){Ok(b)=>vec![ActiveStageOutput::ResponseFrame(b)],Err(_)=>vec![]},None=>vec![]}
            },
            _=bridge_ping.tick()=>{
                bridge.check_liveness();
                if bridge.ready(){
                    let envelope=rdpilot_bridge_protocol::Envelope::new(bridge.generation,0,rdpilot_bridge_protocol::Message::Ping);
                    match build_request_frame(&mut active_stage,envelope){Ok(bytes)=>vec![ActiveStageOutput::ResponseFrame(bytes)],Err(_)=>vec![]}
                }else{vec![]}
            },
            frame_read = reader.read_pdu() => {
                let (action, payload) = frame_read
                    .map_err(|e| Error::Session(format!("read PDU failed: {e}")))?;
                trace!(?action, len = payload.len(), "frame received");
                active_stage
                    .process(&mut image, action, &payload)
                    .map_err(|e| Error::Session(format!("process PDU failed: {e}")))?
            }
            event = input_rx.recv() => {
                match event {
                    Some(RdpInputEvent::Close) => {
                        debug!("graceful shutdown requested");
                        active_stage
                            .graceful_shutdown()
                            .map_err(|e| Error::Session(format!("graceful shutdown failed: {e}")))?
                    }
                    Some(RdpInputEvent::FastPath(events)) => {
                        // Pre-built by Session::send_mouse/send_key; this loop
                        // only forwards, it never constructs input events or
                        // sleeps between them (Pitfall 3).
                        process_fastpath_input(&mut active_stage, &mut image, &events)?
                    }
                    Some(RdpInputEvent::Request(envelope)) => {
                        // Proactive send: DvcProcessor::start()/process() are
                        // reactive only (RESEARCH Q1), so the outbound request
                        // bytes are built here, from outside the processor,
                        // mirroring IronRDP's own `ActiveStage::encode_resize`
                        // (Display Control DVC) internal pattern.
                        //
                        // NOT propagated via `?`: the DVC may legitimately not
                        // be registered/open yet (the server-side responder
                        // opens it asynchronously, live-verified to take up to
                        // several seconds after the interactive session is
                        // created). That is an expected, per-call-retryable
                        // condition — the caller already bounds each attempt
                        // with its own timeout and leaves the pending oneshot
                        // unresolved on failure here, so the caller's timeout
                        // surfaces a normal `Error::Dvc` it can retry. Ending
                        // the WHOLE session loop thread on a single not-yet-open
                        // request would be fatal to every other in-flight/future
                        // operation (mouse/keyboard/screenshot) too — bug fixed
                        // during the Phase 4 live gate (SC#2/SC#3).
                        match build_request_frame(&mut active_stage, envelope) {
                            Ok(frame_bytes) => vec![ActiveStageOutput::ResponseFrame(frame_bytes)],
                            Err(e) => {
                                debug!(%e, "bridge request dropped (bridge DVC not ready yet); \
                                    the caller's client-side timeout will surface a retryable error");
                                vec![]
                            }
                        }
                    }
                    None => {
                        // All senders dropped without an explicit Close (e.g. the
                        // Session handle was dropped). Exit the loop best-effort.
                        debug!("input channel closed; ending session loop");
                        break;
                    }
                }
            }
            _=bridge.data_ready.notified()=>{
                match bridge.take_data(){Some(e)=>match build_request_frame(&mut active_stage,e){Ok(b)=>vec![ActiveStageOutput::ResponseFrame(b)],Err(_)=>vec![]},None=>vec![]}
            },
            _ = keepalive.tick() => {
                trace!("keepalive tick");
                active_stage
                    .process_fastpath_input(&mut image, &[null_input_event()])
                    .map_err(|e| Error::Session(format!("keepalive input failed: {e}")))?
            }
        };

        let mut terminate = false;
        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(frame_bytes) => {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        writer.write_all(&frame_bytes),
                    )
                    .await
                    .map_err(|_| Error::Session("RDP write deadline exceeded".into()))?
                    .map_err(|e| Error::Session(format!("write response failed: {e}")))?;
                }
                ActiveStageOutput::GraphicsUpdate(_region) => {
                    // Snapshot the whole framebuffer into the shared frame. We
                    // copy the full image rather than the dirty region so a reader
                    // always gets a complete frame (Pitfall 3 — never share the
                    // live DecodedImage).
                    let width = u32::from(image.width());
                    let height = u32::from(image.height());
                    frame.write(width, height, image.data().to_vec());
                }
                ActiveStageOutput::DeactivateAll(mut activation) => {
                    // Server resize / share change: run the reactivation sequence
                    // and rebuild the framebuffer at the new size, or screenshots
                    // go stale / wrong-size (Pitfall 2, criterion #4).
                    tokio::select! {
                        _=bridge.shutdown.notified()=>return Ok(()),
                        outcome=tokio::time::timeout(std::time::Duration::from_secs(10),reactivate(&mut reader,&mut writer,&mut active_stage,&mut image,&mut activation))=>{
                            outcome.map_err(|_|Error::Session("RDP reactivation deadline exceeded".into()))??;
                        }
                    }
                }
                ActiveStageOutput::Terminate(reason) => {
                    debug!(%reason, "session terminated by server");
                    bridge.invalidate(&format!("RDP server ended session: {reason}"));
                    terminate = true;
                }
                // Pointer* variants: ignored — enable_server_pointer is false, so
                // there is no client-side cursor to render (Open Q2).
                _ => {}
            }
        }

        if terminate {
            break;
        }
    }

    Ok(())
}

/// Encode one queued input action as one or more legal fast-path input PDUs.
///
/// This is called synchronously by `run` before it writes the action's outputs
/// and returns to `select!`, so splitting the wire representation cannot let a
/// later queued action interleave with this action's translated event stream.
fn process_fastpath_input(
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    events: &[ironrdp::pdu::input::fast_path::FastPathInputEvent],
) -> Result<Vec<ActiveStageOutput>> {
    let mut outputs = Vec::new();
    for batch in events.chunks(255) {
        let mut batch_outputs = active_stage
            .process_fastpath_input(image, batch)
            .map_err(|e| Error::Session(format!("input injection failed: {e}")))?;
        outputs.append(&mut batch_outputs);
    }
    Ok(outputs)
}

/// Build the outbound DVC frame bytes for a single typed bridge request
/// (RESEARCH Pattern 2: one builder for every `MsgType`, not one per message
/// type).
///
/// Looks up the registered [`crate::bridge::BridgeProcessor`] DVC,
/// requires it to be open (`channel_id()` populated — i.e. the server-side
/// responder has completed `WTSVirtualChannelOpenEx` and the drdynvc Create
/// handshake), encodes the request envelope via
/// [`crate::bridge::BridgeProcessor::encode_request`], and wraps it
/// into a single outbound frame via [`ActiveStage::encode_dvc_messages`].
///
/// Returns [`Error::Dvc`] — deliberately NOT propagated with `?` by the caller
/// — when the channel is not yet registered/open: this is an expected,
/// transient, per-call-retryable condition (the responder opens the channel
/// asynchronously after the interactive session exists), not a fatal session
/// error (see the call site in [`run`]).
fn build_request_frame(
    active_stage: &mut ActiveStage,
    envelope: rdpilot_bridge_protocol::Envelope,
) -> Result<Vec<u8>> {
    let (channel_id, dvc_messages) = {
        let dvc = active_stage
            .get_dvc::<crate::bridge::BridgeProcessor>()
            .ok_or_else(|| Error::dvc("bridge channel not registered"))?;
        let id = dvc
            .channel_id()
            .ok_or_else(|| Error::dvc("bridge channel not open"))?;
        let processor = dvc
            .channel_processor_downcast_ref::<crate::bridge::BridgeProcessor>()
            .ok_or_else(|| Error::dvc("bridge processor unavailable"))?;
        (
            id,
            processor
                .encode_request(envelope)
                .map_err(|e| Error::dvc(e.to_string()))?,
        )
    };
    let svc = ironrdp::dvc::encode_dvc_messages(
        channel_id,
        dvc_messages,
        ironrdp::svc::ChannelFlags::empty(),
    )
    .map_err(|e| Error::dvc(e.to_string()))?;
    active_stage
        .encode_dvc_messages(svc)
        .map_err(|e| Error::dvc(e.to_string()))
}

/// Run the Deactivation-Reactivation sequence and rebuild the framebuffer.
///
/// Drives the [`ConnectionActivationSequence`](ironrdp::connector::ConnectionActivationSequence)
/// to `Finalized`, then replaces `image` with a fresh [`DecodedImage`] at the new
/// desktop size and rebuilds the fast-path processor + share id with the new
/// channel parameters (Pattern 5).
async fn reactivate(
    reader: &mut Framed<ironrdp_tokio::TokioStream<tokio::io::ReadHalf<UpgradedStream>>>,
    writer: &mut Framed<ironrdp_tokio::TokioStream<tokio::io::WriteHalf<UpgradedStream>>>,
    active_stage: &mut ActiveStage,
    image: &mut DecodedImage,
    activation: &mut ConnectionActivationSequence,
) -> Result<()> {
    debug!("deactivate-all received; running reactivation sequence");
    let mut buf = WriteBuf::new();

    loop {
        let written = ironrdp_tokio::single_sequence_step_read(reader, activation, &mut buf)
            .await
            .map_err(|e| Error::Session(format!("reactivation step failed: {e}")))?;

        if written.size().is_some() {
            writer
                .write_all(buf.filled())
                .await
                .map_err(|e| Error::Session(format!("reactivation write failed: {e}")))?;
        }

        if let ConnectionActivationState::Finalized {
            io_channel_id,
            user_channel_id,
            desktop_size,
            share_id,
            enable_server_pointer,
            pointer_software_rendering,
        } = activation.connection_activation_state()
        {
            *image =
                DecodedImage::new(PixelFormat::RgbA32, desktop_size.width, desktop_size.height);

            let processor = fast_path::ProcessorBuilder {
                io_channel_id,
                user_channel_id,
                share_id,
                enable_server_pointer,
                pointer_software_rendering,
                // Reset the bulk-decompression context on reactivation; the
                // server reinitializes compression after a Deactivate-All.
                bulk_decompressor: None,
            }
            .build();

            active_stage.set_fastpath_processor(processor);
            active_stage.set_share_id(share_id);

            debug!(
                width = desktop_size.width,
                height = desktop_size.height,
                "reactivation finalized; framebuffer rebuilt"
            );
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::connector::connection_activation::ConnectionActivationSequence;
    use ironrdp::connector::{Config, ConnectionResult, Credentials, DesktopSize};
    use ironrdp::core::decode;
    use ironrdp::graphics::image_processing::PixelFormat;
    use ironrdp::pdu::gcc::KeyboardType;
    use ironrdp::pdu::input::fast_path::FastPathInput;
    use ironrdp::pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
    use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
    use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
    use ironrdp::session::image::DecodedImage;
    use ironrdp_input::Database;

    fn connection_result_fixture() -> ConnectionResult {
        let config = Config {
            credentials: Credentials::UsernamePassword {
                username: "synthetic".into(),
                password: "synthetic".into(),
            },
            domain: None,
            enable_tls: false,
            enable_credssp: true,
            keyboard_type: KeyboardType::IbmEnhanced,
            keyboard_subtype: 0,
            keyboard_layout: 0,
            keyboard_functional_keys_count: 12,
            ime_file_name: String::new(),
            dig_product_id: String::new(),
            desktop_size: DesktopSize {
                width: 100,
                height: 100,
            },
            bitmap: None,
            client_build: 0,
            client_name: "rdpilot".to_owned(),
            client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
            platform: MajorPlatformType::WINDOWS,
            enable_server_pointer: false,
            request_data: None,
            autologon: false,
            enable_audio_playback: false,
            compression_type: None,
            pointer_software_rendering: true,
            multitransport_flags: None,
            performance_flags: PerformanceFlags::default(),
            desktop_scale_factor: 0,
            hardware_id: None,
            license_cache: None,
            timezone_info: TimezoneInfo::default(),
            alternate_shell: String::new(),
            work_dir: String::new(),
        };
        ConnectionResult {
            io_channel_id: 1003,
            user_channel_id: 1001,
            share_id: 1,
            static_channels: Default::default(),
            desktop_size: config.desktop_size,
            enable_server_pointer: false,
            pointer_software_rendering: false,
            connection_activation: ConnectionActivationSequence::new(config, 1003, 1001),
            compression_type: None,
        }
    }

    fn active_fixture() -> (ActiveStage, DecodedImage) {
        (
            ActiveStage::new(connection_result_fixture()),
            DecodedImage::new(PixelFormat::RgbA32, 100, 100),
        )
    }

    fn translated_type(
        text: &str,
        database: &mut Database,
    ) -> Vec<ironrdp::pdu::input::fast_path::FastPathInputEvent> {
        database
            .apply(crate::input::key_operations(
                &crate::input::KeyAction::Type(text.to_owned()),
            ))
            .into_iter()
            .collect()
    }

    fn dispatch_and_decode(
        stage: &mut ActiveStage,
        image: &mut DecodedImage,
        events: &[ironrdp::pdu::input::fast_path::FastPathInputEvent],
    ) -> Vec<ironrdp::pdu::input::fast_path::FastPathInputEvent> {
        let outputs = process_fastpath_input(stage, image, events)
            .expect("production input dispatcher should encode this action");
        if events.is_empty() {
            assert!(outputs.is_empty(), "empty Type is a no-op");
        }
        let mut decoded = Vec::new();
        for output in outputs {
            if let ActiveStageOutput::ResponseFrame(bytes) = output {
                let pdu: FastPathInput =
                    decode(&bytes).expect("real encoded fast-path PDU decodes");
                assert!((1..=255).contains(&pdu.input_events().len()));
                decoded.extend_from_slice(pdu.input_events());
            }
        }
        assert_eq!(decoded, events);
        decoded
    }

    fn assert_dispatch_conserves(events: &[ironrdp::pdu::input::fast_path::FastPathInputEvent]) {
        let (mut stage, mut image) = active_fixture();
        dispatch_and_decode(&mut stage, &mut image, events);
    }

    /// The keepalive event the loop emits is a well-formed null pointer move
    /// (sanity that the loop's keepalive source is the no-op input, no VM).
    #[test]
    fn keepalive_source_is_null_input() {
        let ev = null_input_event();
        assert!(matches!(
            ev,
            ironrdp::pdu::input::fast_path::FastPathInputEvent::MouseEvent(_)
        ));
    }

    #[test]
    fn production_dispatcher_preserves_translated_type_events_in_legal_pdus() {
        let mut database = Database::new();
        for (label, text) in [
            ("empty", "".to_owned()),
            ("single", "x".to_owned()),
            ("ASCII127", "x".repeat(127)),
            ("ASCII128", "x".repeat(128)),
            ("BMP127", "é".repeat(127)),
            ("BMP128", "é".repeat(128)),
            ("supplementary63", "😀".repeat(63)),
            ("supplementary64", "😀".repeat(64)),
            ("ASCII10000", "x".repeat(10_000)),
            ("mixed-long", "Aé😀🙂".repeat(3_000)),
        ] {
            let events = translated_type(&text, &mut database);
            assert_dispatch_conserves(&events);
            let _ = label;
        }

        let direct = translated_type("x".repeat(128).as_str(), &mut database);
        assert_dispatch_conserves(&direct[..255]);
        assert_dispatch_conserves(&direct[..256]);
    }

    #[test]
    fn stateful_combo_raw_type_mouse_and_keyboard_streams_are_conserved() {
        let (mut stage, mut image) = active_fixture();
        let mut database = Database::new();
        let mut oracle = Database::new();

        let mut apply_and_check = |ops: Vec<ironrdp_input::Operation>| {
            let expected: Vec<_> = oracle.apply(ops.clone()).into_iter().collect();
            let actual: Vec<_> = database.apply(ops).into_iter().collect();
            assert_eq!(actual, expected, "persistent Database state agrees");
            dispatch_and_decode(&mut stage, &mut image, &actual);
        };

        apply_and_check(crate::input::key_operations(
            &crate::input::KeyAction::Combo(vec![crate::input::Key::Ctrl, crate::input::Key::A]),
        ));
        apply_and_check(crate::input::raw_operations(&[crate::RawInput::Key {
            code: 0x1e,
            extended: false,
            down: true,
        }]));
        apply_and_check(crate::input::key_operations(
            &crate::input::KeyAction::Type("é😀".to_owned()),
        ));
        for batch in
            crate::input::mouse_operations(&crate::input::MouseAction::Move { x: 17, y: 23 })
        {
            apply_and_check(batch);
        }
        apply_and_check(crate::input::key_operations(
            &crate::input::KeyAction::Combo(vec![crate::input::Key::B]),
        ));
        apply_and_check(crate::input::raw_operations(&[crate::RawInput::Key {
            code: 0x1e,
            extended: false,
            down: false,
        }]));
    }

    #[tokio::test]
    async fn run_keeps_a_queued_action_after_the_complete_type_stream() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let text = "x".repeat(10_000);
                let type_events = translated_type(&text, &mut Database::new());
                let competitor = FastPathInputEvent::KeyboardEvent(KeyboardFlags::empty(), 0x1e);
                let (input_tx, input_rx) = mpsc::channel(2);
                input_tx
                    .send(RdpInputEvent::FastPath(type_events.clone()))
                    .await
                    .expect("Type action enqueued");

                // A small duplex buffer makes the pump wait for the peer while writing
                // the many Type frames. Enqueue the competitor after observing frame 1.
                let (client, server) = tokio::io::duplex(1024);
                let framed = ironrdp_tokio::TokioFramed::new(
                    Box::new(client) as crate::connect::UpgradedStream
                );
                let mut peer = ironrdp_tokio::TokioFramed::new(server);
                let bridge = std::sync::Arc::new(crate::bridge::BridgeShared::new());
                let shutdown = bridge.clone();
                let pump = tokio::task::spawn_local(run(
                    framed,
                    connection_result_fixture(),
                    input_rx,
                    SharedFrame::new(),
                    bridge,
                ));

                let observed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
                    let mut all_type = Vec::new();
                    let (_, first_frame) =
                        tokio::time::timeout(std::time::Duration::from_secs(5), peer.read_pdu())
                            .await
                            .map_err(|_| "first Type frame deadline".to_owned())?
                            .map_err(|e| format!("first Type frame: {e}"))?;
                    let first: FastPathInput =
                        decode(&first_frame).map_err(|e| format!("first frame decode: {e}"))?;
                    if !(1..=255).contains(&first.input_events().len()) {
                        return Err(format!(
                            "illegal first frame event count: {}",
                            first.input_events().len()
                        ));
                    }
                    all_type.extend_from_slice(first.input_events());

                    tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        input_tx.send(RdpInputEvent::FastPath(vec![competitor.clone()])),
                    )
                    .await
                    .map_err(|_| "competitor enqueue deadline".to_owned())?
                    .map_err(|_| "competitor enqueue failed".to_owned())?;

                    let mut saw_competitor = false;
                    let mut competitor_events = Vec::new();
                    while competitor_events.is_empty() {
                        let (_, frame) = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            peer.read_pdu(),
                        )
                        .await
                        .map_err(|_| "frame read deadline".to_owned())?
                        .map_err(|e| format!("run frame read: {e}"))?;
                        let pdu: FastPathInput =
                            decode(&frame).map_err(|e| format!("frame decode: {e}"))?;
                        if !(1..=255).contains(&pdu.input_events().len()) {
                            return Err(format!(
                                "illegal frame event count: {}",
                                pdu.input_events().len()
                            ));
                        }
                        for event in pdu.input_events() {
                            match event {
                                FastPathInputEvent::UnicodeKeyboardEvent(..) => {
                                    if saw_competitor {
                                        return Err(
                                            "Type resumed after competitor action".to_owned()
                                        );
                                    }
                                    all_type.push(event.clone());
                                }
                                _ => {
                                    saw_competitor = true;
                                    competitor_events.push(event.clone());
                                }
                            }
                        }
                    }
                    if all_type != type_events {
                        return Err(format!(
                            "Type event conservation failed: {} != {}",
                            all_type.len(),
                            type_events.len()
                        ));
                    }
                    if competitor_events != vec![competitor] {
                        return Err(format!("competitor events differ: {competitor_events:?}"));
                    }
                    Ok(())
                })
                .await
                .map_err(|_| "queued action observation deadline".to_owned())
                .and_then(|result| result);

                shutdown.shutdown.notify_one();
                let joined = tokio::time::timeout(std::time::Duration::from_secs(5), pump)
                    .await
                    .expect("pump join deadline")
                    .expect("pump task joins");
                assert!(joined.is_ok(), "run returned an error: {joined:?}");
                observed.expect("queued action contiguity and event conservation");
            })
            .await;
    }
}
