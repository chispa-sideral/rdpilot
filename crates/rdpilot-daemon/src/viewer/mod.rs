//! Live viewer: a small HTTP/1.1 server that shows the daemon's own session
//! framebuffers in a browser, controls and annotates their recordings,
//! replays recordings, and lets a tab take the control lease of a session
//! and send it keyboard and mouse input (unless read-only).
//!
//! - Off by default. It starts only on an explicit `ViewerStart` IPC request
//!   (`rdpilot view`) and lives only while that IPC connection stays open.
//!   One viewer at a time.
//! - Binds explicit addresses only: `127.0.0.1`, plus this host's Tailscale
//!   address when the bind set allows it and one is found ([`bind`]).
//! - Every request passes the Host, Origin/`Sec-Fetch-Site` and token
//!   checks in [`auth`] before routing; the route table allows each route
//!   its methods only.
//! - The viewer receives only [`ViewerRegistry`] (session list, passive
//!   frame lookup, recording actions and recording reads) and
//!   [`ViewerControl`] (human lease operations and input). It has no path
//!   to Cua, transfer, connect, disconnect, native agent input or agent
//!   takeover. Only a take waits on a per-session lock, to let an agent
//!   operation in flight finish.
//! - Read-only mode serves no POST route at all.
//! - All tasks run on the multi-thread runtime (`tokio::spawn`), and frame
//!   capture and PNG encoding run in `spawn_blocking`: nothing runs on the
//!   IPC `LocalSet` thread or on an RDP session thread.
//! - The token is never logged. `Token`'s `Debug` is redacted.

mod auth;
mod bind;
mod control;
mod frames;
mod http;
mod replay;

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rdpilot_ipc::WireViewerBind;

use crate::registry::{ViewerControl, ViewerRegistry};

pub(crate) use auth::{AuthPolicy, Token};
#[cfg(test)]
pub(crate) use bind::{
    bind_listeners, is_tailnet_range, select_tailnet_address, Interface, TailnetSelection,
};
#[cfg(test)]
pub(crate) use control::{InputBody, RateLimits};
#[cfg(test)]
pub(crate) use frames::Encoded;
pub(crate) use http::{serve, Limits, ServerState, ViewerHandle};

/// Allows one active viewer per daemon.
#[derive(Clone, Default)]
pub(crate) struct ViewerGate(Arc<AtomicBool>);

/// Released when the viewer stops.
pub(crate) struct GateGuard(Arc<AtomicBool>);

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl ViewerGate {
    pub(crate) fn acquire(&self) -> Option<GateGuard> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| GateGuard(Arc::clone(&self.0)))
    }
}

/// What `rdpilot view` asked for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ViewerParams {
    pub(crate) bind: WireViewerBind,
    pub(crate) tailnet_override: Option<Ipv4Addr>,
    /// No Takeover and no write route.
    pub(crate) read_only: bool,
    /// A human lease ends after this long without input.
    pub(crate) idle_timeout: std::time::Duration,
}

/// A started viewer. Dropping `handle` stops it.
pub(crate) struct Started {
    pub(crate) handle: ViewerHandle,
    /// `http://addr:port/` per bound address, loopback first.
    pub(crate) addresses: Vec<String>,
    pub(crate) token: Token,
    pub(crate) notices: Vec<String>,
}

/// Start the viewer: select and bind addresses, mint a token, and serve.
///
/// # Errors
///
/// A human-readable reason (no secret) when a viewer already runs, no
/// token can be generated, or the loopback address cannot be bound.
pub(crate) async fn start(
    registry: ViewerRegistry,
    control: ViewerControl,
    gate: &ViewerGate,
    params: ViewerParams,
) -> Result<Started, String> {
    let guard = gate
        .acquire()
        .ok_or_else(|| "a viewer is already running for this daemon".to_owned())?;
    let selection = bind::select_tailnet_address(
        &bind::local_interfaces(),
        params.bind,
        params.tailnet_override,
    );
    let mut notices: Vec<String> = selection.notice().into_iter().collect();
    let bound = bind::bind_listeners(selection.address())
        .await
        .map_err(|e| format!("could not bind the viewer on 127.0.0.1: {e}"))?;
    notices.extend(bound.notices.iter().cloned());
    let token = Token::generate().map_err(|e| format!("could not generate a token: {e}"))?;
    let addresses = bound
        .addrs
        .iter()
        .map(|addr| format!("http://{addr}/"))
        .collect();
    let policy = AuthPolicy::new(token.clone(), &bound.addrs);
    let state = ServerState::new(registry, policy, Limits::default());
    let state = Arc::new(if params.read_only {
        state.read_only()
    } else {
        state.with_control(control, params.idle_timeout)
    });
    let handle = serve(bound.listeners, state, Some(guard));
    Ok(Started {
        handle,
        addresses,
        token,
        notices,
    })
}
