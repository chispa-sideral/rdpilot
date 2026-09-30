//! Local-user-scoped IPC transport — cfg-gated Unix/Windows listener
//! (Plan 12-04; DAEMON-02).
//!
//! The Unix path (`unix.rs`) is fully implemented and offline-testable on
//! this Linux host: `0700` runtime-dir socket + per-connection
//! `peer_cred()` uid check. The Windows path (`windows.rs`, Plan 15-01,
//! closing 12-07's Windows half of DAEMON-02) is authored — explicit
//! owner-only DACL + `first_pipe_instance(true)` — but its
//! `#[cfg(windows)]` gate means this Linux host never compiles it; the
//! real Windows compile+run is confirmed on the pinned Azure VM in
//! Plan 15-05.
//!
//! Socket-path resolution and length-prefixed JSON framing are shared with
//! any thin client via `rdpilot_ipc::transport` (Plan 13-01) — this module
//! only owns the LISTENER-side security primitives that stay daemon-only
//! (bind/accept_and_authorize/authorize_uid, T-13-03).

// `serve_connection` is only wired into the accept loop by `server.rs`
// (Plan 12-06); exercised directly by this wave's own tests until then
// (mirrors `registry.rs`'s identical interface-first rationale).
#![allow(dead_code)]

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use rdpilot_ipc::transport::socket_path;
#[cfg(unix)]
pub use unix::{accept_and_authorize, authorize_uid, bind};

#[cfg(windows)]
pub use windows::{accept_and_authorize, bind, socket_path};

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

mod cua_gate;

use crate::diagnostics::{Diagnostics, Stage};
use crate::dispatch::dispatch_for_ipc;
use crate::events::{CuaCallTracker, EventKind, EventSource};
use crate::registry::{Registry, ViewerControl, ViewerRegistry};
use crate::seams::ManagedCua;
use crate::viewer::{ViewerGate, ViewerParams};
use cua_gate::{CuaGate, Inbound};
use rdpilot_ipc::transport::{read_frame, write_frame};
use rdpilot_ipc::{CuaStreamFrame, Request, ViewerToken, WireResponse, WireViewerBind};

const CONNECT_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Serve one accepted connection: loop `read_frame::<Request>` ->
/// `dispatch` -> `write_frame::<WireResponse>` until the peer closes the
/// stream or a transport error occurs.
///
/// Generic over any `AsyncRead + AsyncWrite` stream — the same loop serves
/// both the Unix `UnixStream` (this plan) and the Windows named pipe
/// (Plan 12-07). Used by Plan 12-06's accept loop.
pub(crate) async fn serve_connection<S>(
    mut stream: S,
    registry: &Registry,
    diagnostics: Option<&Diagnostics>,
    viewer: Option<&ViewerContext>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let req = match read_frame(&mut stream).await {
            Ok(req) => req,
            // Transport closed or malformed frame — end the connection.
            // Deliberately not logged with request content (D-31 applies
            // to the happy path too; nothing here ever had access to a
            // decoded `Request` to begin with on this branch).
            Err(_) => return,
        };
        if let Request::ViewerStart {
            bind,
            tailnet_address,
            read_only,
            idle_timeout_secs,
        } = req
        {
            let options = ViewerOptions {
                bind,
                tailnet_address,
                read_only,
                idle_timeout: idle_timeout_secs
                    .filter(|secs| *secs > 0)
                    .map_or(crate::control::DEFAULT_IDLE_TIMEOUT, Duration::from_secs),
            };
            hold_viewer(&mut stream, viewer, options).await;
            return;
        }
        if let Request::CuaAttach { session } = req {
            match tokio::time::timeout(Duration::from_secs(30), registry.attach_cua(&session)).await
            {
                Ok(Ok(crate::registry::CuaAttach {
                    generation: session_incarnation,
                    events,
                    control,
                    mut attachment,
                })) => {
                    let (bridge_generation, runtime_generation, attachment_id) =
                        attachment.identity();
                    let ack = WireResponse::CuaAttached {
                        session_incarnation,
                        bridge_generation,
                        runtime_generation,
                        attachment_id,
                    };
                    if matches!(
                        tokio::time::timeout(STREAM_WRITE_TIMEOUT, write_frame(&mut stream, &ack))
                            .await,
                        Ok(Ok(()))
                    ) {
                        events.record(
                            EventSource::Cua,
                            EventKind::CuaAttached {
                                attachment: attachment_id,
                            },
                        );
                        let mut tracker = CuaCallTracker::new(Arc::clone(&events));
                        let mut gate = CuaGate::new(control);
                        let reason = forward_cua(
                            &mut stream,
                            attachment.as_mut(),
                            registry,
                            &session,
                            session_incarnation,
                            &mut tracker,
                            &mut gate,
                        )
                        .await;
                        // Unanswered acting calls no longer hold up a take.
                        drop(gate);
                        tracker.close();
                        events.record(
                            EventSource::Cua,
                            EventKind::CuaDetached {
                                attachment: attachment_id,
                                reason: reason.into(),
                            },
                        );
                    }
                    let _ = tokio::time::timeout(STREAM_WRITE_TIMEOUT, attachment.close()).await;
                }
                result => {
                    let message = match result {
                        Ok(Err(e)) => e.to_string(),
                        _ => "Cua attachment timed out".into(),
                    };
                    let response =
                        WireResponse::Error(crate::seams::DaemonError::Connect(message).into());
                    let _ = tokio::time::timeout(
                        STREAM_WRITE_TIMEOUT,
                        write_frame(&mut stream, &response),
                    )
                    .await;
                }
            }
            return;
        }
        let outcome = dispatch_for_ipc(registry, req, diagnostics).await;
        if write_frame(&mut stream, &outcome.response).await.is_err() {
            if let Some(lease) = outcome.connect_lease {
                if let Some(diagnostics) = diagnostics {
                    diagnostics.record(lease.id.as_str(), Stage::IpcPeerClosed);
                }
                if matches!(registry.close_if_generation(&lease).await, Ok(true)) {
                    if let Some(diagnostics) = diagnostics {
                        diagnostics.record(lease.id.as_str(), Stage::RegistryClosed);
                    }
                }
            }
            return;
        }
        if let Some(lease) = outcome.connect_lease {
            if let Some(diagnostics) = diagnostics {
                diagnostics.record(lease.id.as_str(), Stage::IpcResponseWritten);
            }
            if matches!(
                outcome.response,
                WireResponse::Connected {
                    connect_ack_required: true,
                    ..
                }
            ) {
                match tokio::time::timeout(
                    CONNECT_ACK_TIMEOUT,
                    read_frame::<_, Request>(&mut stream),
                )
                .await
                {
                    Ok(Ok(Request::ConnectAck { session })) if session == lease.id => {
                        if write_frame(&mut stream, &WireResponse::Ack).await.is_err() {
                            return;
                        }
                    }
                    _ => {
                        if let Some(diagnostics) = diagnostics {
                            diagnostics.record(lease.id.as_str(), Stage::IpcPeerClosed);
                        }
                        let _ = registry.close_if_generation(&lease).await;
                        return;
                    }
                }
            }
        }
    }
}

const STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// What `serve_connection` needs to start the live viewer.
#[derive(Clone)]
pub(crate) struct ViewerContext {
    pub(crate) registry: ViewerRegistry,
    pub(crate) control: ViewerControl,
    pub(crate) gate: ViewerGate,
}

/// What `rdpilot view` asked for.
struct ViewerOptions {
    bind: WireViewerBind,
    tailnet_address: Option<String>,
    read_only: bool,
    idle_timeout: Duration,
}

/// Start the viewer, answer `ViewerStarted`, then keep it running until the
/// peer closes this connection (or sends anything else). The viewer is off
/// again when this returns. No request content or token is logged.
async fn hold_viewer<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    viewer: Option<&ViewerContext>,
    options: ViewerOptions,
) {
    let ViewerOptions {
        bind,
        tailnet_address,
        read_only,
        idle_timeout,
    } = options;
    let refuse =
        |message: String| WireResponse::Error(crate::seams::DaemonError::Connect(message).into());
    let started = match (viewer, tailnet_address.as_deref().map(str::parse)) {
        (None, _) => Err(refuse("the viewer is not available in this daemon".into())),
        (_, Some(Err(_))) => Err(refuse("tailnet address must be an IPv4 address".into())),
        (Some(ctx), parsed) => {
            let params = ViewerParams {
                bind,
                tailnet_override: parsed.and_then(Result::ok),
                read_only,
                idle_timeout,
            };
            crate::viewer::start(ctx.registry.clone(), ctx.control.clone(), &ctx.gate, params)
                .await
                .map_err(refuse)
        }
    };
    let started = match started {
        Ok(started) => started,
        Err(response) => {
            let _ =
                tokio::time::timeout(STREAM_WRITE_TIMEOUT, write_frame(stream, &response)).await;
            return;
        }
    };
    let response = WireResponse::ViewerStarted {
        addresses: started.addresses.clone(),
        token: ViewerToken(started.token.expose().to_owned()),
        notices: started.notices.clone(),
    };
    if !matches!(
        tokio::time::timeout(STREAM_WRITE_TIMEOUT, write_frame(stream, &response)).await,
        Ok(Ok(()))
    ) {
        return;
    }
    // The viewer lives exactly as long as this connection. `rdpilot view`
    // sends nothing more; EOF (Ctrl-C, terminal closed) or any frame ends it.
    let _ = read_frame::<_, serde_json::Value>(stream).await;
    drop(started.handle);
    // Every human lease ends with the viewer; control returns to the agent.
    if let Some(ctx) = viewer {
        ctx.control.end_all().await;
    }
}

/// Preserve a partially read IPC frame while forwarding spontaneous Cua output.
/// A blocked reader or writer never holds a registry/session lock. No retry or reattach.
async fn forward_cua<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    attachment: &mut dyn ManagedCua,
    registry: &Registry,
    session: &rdpilot_ipc::SessionId,
    incarnation: u64,
    tracker: &mut CuaCallTracker,
    gate: &mut CuaGate,
) -> &'static str {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let reason = 'stream: loop {
        let incoming = read_frame::<_, CuaStreamFrame>(&mut reader);
        tokio::pin!(incoming);
        loop {
            tokio::select! {
                frame = &mut incoming => {
                    match frame {
                        Ok(CuaStreamFrame::Message {message}) => {
                            if !registry.touch_generation(session, incarnation) { break 'stream "target incarnation closed"; }
                            let (inbound, transition) = gate.inbound(message);
                            if let Some(transition) = transition {
                                // Releases reach the session before the call does.
                                gate.control().discharge(transition).await;
                            }
                            match inbound {
                                Inbound::Forward(message) => {
                                    tracker.observe_request(&message);
                                    if !matches!(tokio::time::timeout(STREAM_WRITE_TIMEOUT, attachment.send(message)).await, Ok(Ok(()))) {
                                        break 'stream "Cua input unavailable";
                                    }
                                }
                                Inbound::Answer { request, reply } => {
                                    tracker.observe_request(&request);
                                    tracker.observe_response(&reply);
                                    let frame = CuaStreamFrame::Message { message: reply };
                                    if !matches!(tokio::time::timeout(STREAM_WRITE_TIMEOUT, write_frame(&mut writer, &frame)).await, Ok(Ok(()))) {
                                        break 'stream "caller output unavailable";
                                    }
                                }
                                Inbound::Drop => {}
                            }
                        }
                        Ok(CuaStreamFrame::Closed {..}) | Err(_) => break 'stream "caller closed or malformed frame",
                    }
                    break;
                }
                output = attachment.recv() => {
                    match output {
                        Ok(Some(mut message)) => {
                            if !registry.touch_generation(session, incarnation) { break 'stream "target incarnation closed"; }
                            gate.outbound(&mut message);
                            tracker.observe_response(&message);
                            let frame = CuaStreamFrame::Message { message };
                            if !matches!(tokio::time::timeout(STREAM_WRITE_TIMEOUT, write_frame(&mut writer, &frame)).await, Ok(Ok(()))) {
                                break 'stream "caller output unavailable";
                            }
                        }
                        Ok(None) | Err(_) => break 'stream "Cua attachment closed",
                    }
                }
            }
        }
    };
    let _ = tokio::time::timeout(
        STREAM_WRITE_TIMEOUT,
        write_frame(
            &mut writer,
            &CuaStreamFrame::Closed {
                reason: reason.into(),
            },
        ),
    )
    .await;
    reason
}

#[cfg(test)]
mod cua_tests;
