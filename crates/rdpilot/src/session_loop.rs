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

use ironrdp::connector::connection_activation::{ConnectionActivationSequence, ConnectionActivationState};
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
    /// A proactive ping request on the `RDPILOT_SENSOR` DVC channel (SENSOR-03,
    /// SC#2), carrying the correlation `req_id` `Session::ping()` allocated.
    /// Unlike `FastPath`, this loop *builds* the outbound bytes itself (via
    /// `ActiveStage::get_dvc` + `RdpilotSensorProcessor::encode_ping` +
    /// `ironrdp_dvc::encode_dvc_messages`) rather than merely forwarding a
    /// pre-built payload — `DvcProcessor::start()`/`process()` are reactive
    /// only and cannot originate a send on their own (RESEARCH Q1).
    Ping(u64),
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

    loop {
        let outputs = tokio::select! {
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
                        active_stage
                            .process_fastpath_input(&mut image, &events)
                            .map_err(|e| Error::Session(format!("input injection failed: {e}")))?
                    }
                    Some(RdpInputEvent::Ping(req_id)) => {
                        // Proactive send: DvcProcessor::start()/process() are
                        // reactive only (RESEARCH Q1), so the outbound Ping
                        // bytes are built here, from outside the processor,
                        // mirroring IronRDP's own `ActiveStage::encode_resize`
                        // (Display Control DVC) internal pattern.
                        let (channel_id, dvc_messages) = {
                            // Block-scoped: `get_dvc()` borrows `&mut
                            // active_stage`; that borrow MUST end before the
                            // second `&mut active_stage` call below
                            // (Pitfall 3) — this scope is load-bearing.
                            let dvc = active_stage
                                .get_dvc::<crate::sensor::RdpilotSensorProcessor>()
                                .ok_or_else(|| Error::Dvc("sensor channel not registered".to_owned()))?;
                            let channel_id = dvc
                                .channel_id()
                                .ok_or_else(|| Error::Dvc("sensor channel not yet open".to_owned()))?;
                            let processor = dvc
                                .channel_processor_downcast_ref::<crate::sensor::RdpilotSensorProcessor>()
                                .ok_or_else(|| Error::Dvc("sensor processor downcast failed".to_owned()))?;
                            let dvc_messages = processor
                                .encode_ping(req_id)
                                .map_err(|e| Error::Dvc(e.to_string()))?;
                            (channel_id, dvc_messages)
                        };
                        let svc_messages =
                            ironrdp::dvc::encode_dvc_messages(channel_id, dvc_messages, ironrdp::svc::ChannelFlags::empty())
                                .map_err(|e| Error::Dvc(e.to_string()))?;
                        vec![ActiveStageOutput::ResponseFrame(
                            active_stage
                                .encode_dvc_messages(svc_messages)
                                .map_err(|e| Error::Dvc(e.to_string()))?,
                        )]
                    }
                    None => {
                        // All senders dropped without an explicit Close (e.g. the
                        // Session handle was dropped). Exit the loop best-effort.
                        debug!("input channel closed; ending session loop");
                        break;
                    }
                }
            }
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
                    writer
                        .write_all(&frame_bytes)
                        .await
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
                    reactivate(&mut reader, &mut writer, &mut active_stage, &mut image, &mut activation)
                        .await?;
                }
                ActiveStageOutput::Terminate(reason) => {
                    debug!(%reason, "session terminated by server");
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
            *image = DecodedImage::new(PixelFormat::RgbA32, desktop_size.width, desktop_size.height);

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
}
