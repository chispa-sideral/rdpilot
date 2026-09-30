//! Recording routes of the viewer: the recording controls of the live
//! panel (POST) and the replay reads (GET and HEAD).
//!
//! - Write routes accept `Content-Type: application/json` only (else 415)
//!   and a body of at most [`MAX_BODY`] bytes (else 413), read only after
//!   the request passed every check and bounded in time by the header read
//!   timeout. Annotation text over 4 KiB is refused with 400.
//! - Read routes take a recording id that must have the exact id shape and
//!   a segment number in 1..=999999; paths are joined from these validated
//!   parts only. Unknown ids are 404. Only closed segments are served, with
//!   single-range requests (206, or 416 when unsatisfiable).
//! - File reads run on blocking threads.
//! - Actions here are not session activity and take no per-session lock.

use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::http::request::Parts;
use hyper::{header, Response, StatusCode};
use rdpilot_ipc::SessionId;

use super::http::{empty, response, Body};
use crate::recording::store::{self, Store};
use crate::registry::{Refusal, ViewerRegistry};

/// Largest accepted request body.
pub(crate) const MAX_BODY: usize = 8 * 1024;

pub(crate) fn json(status: StatusCode, value: &serde_json::Value) -> Response<Body> {
    match serde_json::to_vec(value) {
        Ok(body) => response(status, "application/json", Bytes::from(body)),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

fn refusal(refusal: Refusal) -> Response<Body> {
    let (status, message) = match refusal {
        Refusal::NoSession(m) => (StatusCode::NOT_FOUND, m),
        Refusal::Rejected(m) => {
            let status = if m.contains("bytes") || m.contains("empty") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::CONFLICT
            };
            (status, m)
        }
        Refusal::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
    };
    json(status, &serde_json::json!({ "error": message }))
}

fn changed(id: Option<String>, changed: bool, message: String) -> Response<Body> {
    json(
        StatusCode::OK,
        &serde_json::json!({ "id": id, "changed": changed, "message": message }),
    )
}

/// Read a small JSON body: after the checks, within `timeout`.
pub(crate) async fn read_json(
    parts: &Parts,
    body: Incoming,
    timeout: Duration,
) -> Result<serde_json::Value, Response<Body>> {
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let media = content_type.split(';').next().unwrap_or_default().trim();
    if !media.eq_ignore_ascii_case("application/json") {
        return Err(empty(StatusCode::UNSUPPORTED_MEDIA_TYPE));
    }
    let declared = parts
        .headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > MAX_BODY) {
        return Err(empty(StatusCode::PAYLOAD_TOO_LARGE));
    }
    let collected = tokio::time::timeout(timeout, Limited::new(body, MAX_BODY).collect()).await;
    let bytes = match collected {
        Err(_) => return Err(empty(StatusCode::REQUEST_TIMEOUT)),
        Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => {
            return Err(empty(StatusCode::PAYLOAD_TOO_LARGE))
        }
        Ok(Err(_)) => return Err(empty(StatusCode::BAD_REQUEST)),
        Ok(Ok(collected)) => collected.to_bytes(),
    };
    serde_json::from_slice(&bytes).map_err(|_| empty(StatusCode::BAD_REQUEST))
}

/// `POST /api/sessions/{id}/recording` `{"action":"start"|"stop"}`.
pub(crate) async fn post_recording(
    registry: &ViewerRegistry,
    id: &SessionId,
    body: &serde_json::Value,
) -> Response<Body> {
    match body.get("action").and_then(serde_json::Value::as_str) {
        Some("start") => match registry.record_start(id).await {
            Ok((rid, true)) => changed(
                Some(rid.clone()),
                true,
                format!("recording started ({rid})"),
            ),
            Ok((rid, false)) => changed(
                Some(rid.clone()),
                false,
                format!("already recording ({rid}); nothing changed"),
            ),
            Err(e) => refusal(e),
        },
        Some("stop") => match registry.record_stop(id) {
            Ok(Some(rid)) => changed(
                Some(rid.clone()),
                true,
                format!("recording stopped ({rid})"),
            ),
            Ok(None) => changed(
                None,
                false,
                "session is not recording; nothing changed".into(),
            ),
            Err(e) => refusal(e),
        },
        _ => empty(StatusCode::BAD_REQUEST),
    }
}

/// `POST /api/sessions/{id}/annotations` `{"text":"..."}`.
pub(crate) fn post_annotation(
    registry: &ViewerRegistry,
    id: &SessionId,
    body: &serde_json::Value,
) -> Response<Body> {
    let Some(text) = body.get("text").and_then(serde_json::Value::as_str) else {
        return empty(StatusCode::BAD_REQUEST);
    };
    match registry.annotate(id, text) {
        Ok(rid) => changed(
            Some(rid.clone()),
            true,
            format!("annotation added to {rid}"),
        ),
        Err(e) => refusal(e),
    }
}

/// `POST /api/recordings/{rid}/keep` `{"keep":true|false}`.
pub(crate) async fn post_keep(
    registry: &ViewerRegistry,
    rid: String,
    body: &serde_json::Value,
) -> Response<Body> {
    let Some(keep) = body.get("keep").and_then(serde_json::Value::as_bool) else {
        return empty(StatusCode::BAD_REQUEST);
    };
    let recordings = registry.recordings();
    let for_task = rid.clone();
    match tokio::task::spawn_blocking(move || recordings.keep(&for_task, keep)).await {
        Ok(Ok(was_changed)) => changed(
            Some(rid.clone()),
            was_changed,
            if keep {
                format!("{rid} kept")
            } else {
                format!("{rid} no longer kept")
            },
        ),
        Ok(Err(message)) if message.starts_with("unknown") => json(
            StatusCode::NOT_FOUND,
            &serde_json::json!({ "error": message }),
        ),
        Ok(Err(message)) => json(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({ "error": message }),
        ),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `GET /api/recordings`: the list with totals.
pub(crate) async fn list(registry: &ViewerRegistry) -> Response<Body> {
    let recordings = registry.recordings();
    match tokio::task::spawn_blocking(move || recordings.list()).await {
        Ok(Ok(listing)) => json(
            StatusCode::OK,
            &serde_json::json!({
                "recordings": listing.recordings,
                "kept_bytes": listing.kept_bytes,
                "unkept_bytes": listing.unkept_bytes,
                "budget_bytes": listing.budget_bytes,
                "kept_over_budget": listing.kept_over_budget,
            }),
        ),
        Ok(Err(message)) => json(
            StatusCode::SERVICE_UNAVAILABLE,
            &serde_json::json!({ "error": message }),
        ),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// The store, or `None` when recording is unavailable (503).
fn store_of(registry: &ViewerRegistry) -> Option<Store> {
    registry.recordings().root().map(Store::new)
}

/// `GET /api/recordings/{rid}`: the manifest with the keep mark and whether
/// it is still being written.
pub(crate) async fn detail(registry: &ViewerRegistry, rid: String) -> Response<Body> {
    let Some(store) = store_of(registry) else {
        return empty(StatusCode::SERVICE_UNAVAILABLE);
    };
    let recordings = registry.recordings();
    let read = tokio::task::spawn_blocking(move || {
        let dir = store.dir(&rid)?;
        let manifest = Store::read_manifest(&dir).ok()?;
        let active = recordings
            .list()
            .ok()
            .and_then(|l| l.recordings.into_iter().find(|r| r.id == rid))
            .is_some_and(|r| r.active);
        Some(serde_json::json!({
            "manifest": manifest,
            "kept": dir.join(store::KEEP).is_file(),
            "active": active,
        }))
    })
    .await;
    match read {
        Ok(Some(value)) => json(StatusCode::OK, &value),
        Ok(None) => empty(StatusCode::NOT_FOUND),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// `GET /api/recordings/{rid}/events`: the complete event lines as an array.
pub(crate) async fn events(registry: &ViewerRegistry, rid: String) -> Response<Body> {
    let Some(store) = store_of(registry) else {
        return empty(StatusCode::SERVICE_UNAVAILABLE);
    };
    let read = tokio::task::spawn_blocking(move || {
        let dir = store
            .dir(&rid)
            .filter(|d| d.join(store::MANIFEST).is_file())?;
        Some(store::read_events(&dir))
    })
    .await;
    match read {
        Ok(Some(events)) => json(StatusCode::OK, &serde_json::Value::Array(events)),
        Ok(None) => empty(StatusCode::NOT_FOUND),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// A parsed single `Range: bytes=...` header.
fn parse_range(value: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // several ranges: serve the whole file
    }
    let (start, end) = spec.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    let range = if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        if suffix == 0 || len == 0 {
            return Some(Err(()));
        }
        (len.saturating_sub(suffix), len - 1)
    } else {
        let start: u64 = start.parse().ok()?;
        let end = if end.is_empty() {
            len.saturating_sub(1)
        } else {
            end.parse::<u64>().ok()?.min(len.saturating_sub(1))
        };
        if start >= len || start > end {
            return Some(Err(()));
        }
        (start, end)
    };
    Some(Ok(range))
}

/// `GET /api/recordings/{rid}/segments/{n}`: a closed segment, `video/webm`.
pub(crate) async fn segment(
    registry: &ViewerRegistry,
    rid: String,
    number: u32,
    range: Option<String>,
) -> Response<Body> {
    let Some(store) = store_of(registry) else {
        return empty(StatusCode::SERVICE_UNAVAILABLE);
    };
    let read = tokio::task::spawn_blocking(move || {
        let path = store.segment_path(&rid, number)?;
        std::fs::read(path).ok()
    })
    .await;
    let bytes = match read {
        Ok(Some(bytes)) => Bytes::from(bytes),
        Ok(None) => return empty(StatusCode::NOT_FOUND),
        Err(_) => return empty(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let len = bytes.len() as u64;
    let mut reply = match range.as_deref().and_then(|r| parse_range(r, len)) {
        None => response(StatusCode::OK, "video/webm", bytes),
        Some(Err(())) => {
            let mut r = empty(StatusCode::RANGE_NOT_SATISFIABLE);
            if let Ok(v) = header::HeaderValue::from_str(&format!("bytes */{len}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return r;
        }
        Some(Ok((start, end))) => {
            let (s, e) = (
                usize::try_from(start).unwrap_or(0),
                usize::try_from(end).unwrap_or(0),
            );
            let mut r = response(
                StatusCode::PARTIAL_CONTENT,
                "video/webm",
                bytes.slice(s..=e),
            );
            if let Ok(v) = header::HeaderValue::from_str(&format!("bytes {start}-{end}/{len}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            r
        }
    };
    reply.headers_mut().insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some(Ok((0, 9))));
        assert_eq!(parse_range("bytes=90-", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=-10", 100), Some(Ok((90, 99))));
        assert_eq!(parse_range("bytes=50-500", 100), Some(Ok((50, 99))));
        assert_eq!(parse_range("bytes=100-", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=9-3", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=-0", 100), Some(Err(())));
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=a-b", 100), None);
    }
}
