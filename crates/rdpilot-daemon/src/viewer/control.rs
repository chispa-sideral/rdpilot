//! Control routes of the viewer: a tab takes, keeps and releases the
//! control lease of one session, and sends its input.
//!
//! - `POST /api/sessions/{id}/control` with
//!   `{"action":"take"|"release"|"heartbeat"|"blur","lease"?}`.
//! - `POST /api/sessions/{id}/input` with
//!   `{"lease","generation","width","height","events":[...]}` (at most
//!   [`MAX_EVENTS`] events).
//! - Both need the checks of every route plus an exact same-origin
//!   `Origin` ([`super::auth::AuthPolicy::check_control`]), JSON bodies of
//!   at most 8 KiB, and a per-session rate ([`RateLimits`]); they exist only
//!   when the viewer is not read-only.
//! - A request that names a lease which is not current never changes the
//!   controller: it gets 409 with the loss notice data when the lease ended
//!   recently, else 409 without it.
//! - They reach only [`ViewerControl`]: lease operations and input for the
//!   session in the path. Nothing here logs; no lease id, key or coordinate
//!   appears in an error text.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

use hyper::{Response, StatusCode};
use rdpilot_ipc::SessionId;
use serde::Deserialize;

use super::http::{empty, Body};
use super::replay::json;
use crate::control::{HumanEvent, NotHeld, TakeError, BUSY_MESSAGE};
use crate::registry::{ControlRefusal, ViewerControl};

/// Most events per input request.
pub(crate) const MAX_EVENTS: usize = 64;

/// Per-session request rates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RateLimits {
    /// Input events per second.
    pub(crate) events: f64,
    /// Control actions per second.
    pub(crate) actions: f64,
}

impl Default for RateLimits {
    fn default() -> Self {
        RateLimits {
            events: 400.0,
            actions: 20.0,
        }
    }
}

/// A token bucket holding at most one second of its rate.
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Per-session token buckets for input events and control actions.
#[derive(Default)]
pub(crate) struct Rates {
    buckets: Mutex<HashMap<(SessionId, bool), Bucket>>,
}

impl Rates {
    /// Take `cost` tokens from `id`'s bucket; `false` when over the rate.
    fn allow(&self, id: &SessionId, input: bool, rate: f64, cost: f64) -> bool {
        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if buckets.len() > 1024 {
            buckets.clear();
        }
        let now = Instant::now();
        let bucket = buckets.entry((id.clone(), input)).or_insert(Bucket {
            tokens: rate,
            at: now,
        });
        let refill = now.saturating_duration_since(bucket.at).as_secs_f64() * rate;
        bucket.tokens = (bucket.tokens + refill).min(rate);
        bucket.at = now;
        if bucket.tokens >= cost {
            bucket.tokens -= cost;
            true
        } else {
            false
        }
    }
}

/// The loss notice data (`null` for an unknown lease).
fn lost_value(lost: NotHeld) -> serde_json::Value {
    match lost {
        NotHeld::Lost(loss) => serde_json::to_value(loss).unwrap_or_default(),
        NotHeld::Unknown => serde_json::Value::Null,
    }
}

fn not_held(lost: NotHeld) -> Response<Body> {
    json(
        StatusCode::CONFLICT,
        &serde_json::json!({ "held": false, "lost": lost_value(lost) }),
    )
}

fn refusal(refusal: ControlRefusal) -> Response<Body> {
    match refusal {
        ControlRefusal::NoSession => empty(StatusCode::NOT_FOUND),
        ControlRefusal::NotHeld(lost) => not_held(lost),
        ControlRefusal::Take(TakeError::Busy) => json(
            StatusCode::CONFLICT,
            &serde_json::json!({ "held": false, "error": BUSY_MESSAGE }),
        ),
        ControlRefusal::Take(TakeError::Lost(loss)) => not_held(NotHeld::Lost(loss)),
        ControlRefusal::Unavailable => empty(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(Deserialize)]
struct ControlBody {
    action: String,
    #[serde(default)]
    lease: Option<String>,
}

/// `POST /api/sessions/{id}/control`.
pub(crate) async fn post_control(
    control: &ViewerControl,
    rates: &Rates,
    limits: RateLimits,
    id: &SessionId,
    peer: IpAddr,
    body: serde_json::Value,
) -> Response<Body> {
    let Ok(body) = serde_json::from_value::<ControlBody>(body) else {
        return empty(StatusCode::BAD_REQUEST);
    };
    if !rates.allow(id, false, limits.actions, 1.0) {
        return empty(StatusCode::TOO_MANY_REQUESTS);
    }
    let lease = body.lease.as_deref().unwrap_or_default();
    let done = |result: Result<(), ControlRefusal>, held: bool| match result {
        Ok(()) => json(StatusCode::OK, &serde_json::json!({ "held": held })),
        Err(e) => refusal(e),
    };
    match body.action.as_str() {
        "take" => match control.take(id, peer).await {
            Ok((grant, geometry)) => {
                let (width, height) = geometry.unwrap_or_default();
                json(
                    StatusCode::OK,
                    &serde_json::json!({
                        "held": true,
                        "lease": grant.lease,
                        "generation": grant.generation,
                        "width": width,
                        "height": height,
                        "controller": grant.controller,
                    }),
                )
            }
            Err(e) => refusal(e),
        },
        "heartbeat" => done(control.heartbeat(id, lease), true),
        "blur" => done(control.blur(id, lease).await, true),
        "release" => done(control.release(id, lease).await, false),
        _ => empty(StatusCode::BAD_REQUEST),
    }
}

#[derive(Deserialize)]
pub(crate) struct InputBody {
    lease: String,
    generation: u64,
    width: u32,
    height: u32,
    pub(crate) events: Vec<HumanEvent>,
}

/// `POST /api/sessions/{id}/input`.
pub(crate) async fn post_input(
    control: &ViewerControl,
    rates: &Rates,
    limits: RateLimits,
    id: &SessionId,
    body: serde_json::Value,
) -> Response<Body> {
    let Ok(body) = serde_json::from_value::<InputBody>(body) else {
        return empty(StatusCode::BAD_REQUEST);
    };
    if body.events.len() > MAX_EVENTS {
        return empty(StatusCode::PAYLOAD_TOO_LARGE);
    }
    #[allow(clippy::cast_precision_loss)] // At most MAX_EVENTS.
    let cost = body.events.len().max(1) as f64;
    if !rates.allow(id, true, limits.events, cost) {
        return empty(StatusCode::TOO_MANY_REQUESTS);
    }
    match control
        .input(
            id,
            &body.lease,
            body.generation,
            (body.width, body.height),
            body.events,
        )
        .await
    {
        Ok(report) => {
            let mut value = serde_json::json!({
                "applied": report.applied,
                "dropped": report.dropped,
                "held": report.lost.is_none(),
            });
            match report.lost {
                None => json(StatusCode::OK, &value),
                Some(lost) => {
                    value["lost"] = lost_value(lost);
                    json(StatusCode::CONFLICT, &value)
                }
            }
        }
        Err(e) => refusal(e),
    }
}
