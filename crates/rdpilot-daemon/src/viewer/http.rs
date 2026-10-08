//! HTTP/1.1 serving for the live viewer: accept loops, per-connection
//! bounds, and the closed route table.
//!
//! Routes (after [`AuthPolicy::check`]):
//! - GET `/` — the viewer page (token in `?token=`);
//! - GET `/api/sessions` — the session list;
//! - GET `/api/sessions/{id}/frame?after=SEQ` — long-poll for a newer frame;
//! - GET `/api/sessions/{id}/events?after=SEQ` — the session's retained
//!   events with `seq > SEQ`, answered immediately (never held open);
//! - POST `/api/sessions/{id}/recording` — start or stop recording;
//! - POST `/api/sessions/{id}/annotations` — annotate the recording;
//! - GET/HEAD `/api/recordings`, `/api/recordings/{rid}`,
//!   `/api/recordings/{rid}/events`, `/api/recordings/{rid}/segments/{n}` —
//!   replay reads;
//! - POST `/api/recordings/{rid}/keep` — mark or unmark keep;
//! - POST `/api/sessions/{id}/control` — take, keep or release the control
//!   lease;
//! - POST `/api/sessions/{id}/input` — the lease holder's input.
//!
//! A known route with another method is 405; an unknown path is 404 for
//! GET and HEAD and 405 otherwise. In read-only mode every POST route
//! answers 404. No route reaches Cua, transfer, connect or disconnect; the
//! control and input routes reach only human lease operations and input
//! for the session in the path ([`super::control`]). The recording POST
//! routes are in [`super::replay`].
//!
//! Bounds: at most [`Limits::max_connections`] accepted connections across
//! all listeners (excess connections are closed at accept); request headers
//! must arrive within [`Limits::header_read_timeout`] (hyper's timer, which
//! also closes idle keep-alive connections); a response write that makes no
//! progress for [`Limits::write_timeout`] closes the connection
//! ([`WriteDeadline`]); a long-poll answers within [`Limits::long_poll`].

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{header, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rdpilot_ipc::SessionId;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Sleep;

use super::auth::{query_param, AuthPolicy};
use super::control::{self, RateLimits, Rates};
use super::frames::{FrameCache, FrameReply, FRAME_INTERVAL};
use super::replay;
use super::GateGuard;
use crate::registry::{EventsLookup, ViewerControl, ViewerRegistry};

/// The page. `__NONCE__` is replaced per response.
const PAGE: &str = include_str!("page.html");

pub(crate) type Body = Full<Bytes>;

/// Per-connection and per-request bounds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) max_connections: usize,
    pub(crate) header_read_timeout: Duration,
    pub(crate) write_timeout: Duration,
    pub(crate) long_poll: Duration,
    pub(crate) frame_interval: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_connections: 32,
            header_read_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            long_poll: Duration::from_secs(15),
            frame_interval: FRAME_INTERVAL,
        }
    }
}

/// Shared state of one running viewer.
pub(crate) struct ServerState {
    registry: ViewerRegistry,
    auth: AuthPolicy,
    pub(crate) frames: FrameCache,
    limits: Limits,
    pub(crate) permits: Arc<Semaphore>,
    /// Lease operations and input; `None` without the control routes.
    control: Option<ViewerControl>,
    /// No POST route at all.
    read_only: bool,
    idle_timeout: Duration,
    rates: Rates,
    rate_limits: RateLimits,
}

impl ServerState {
    /// A viewer without control routes (recording routes only).
    pub(crate) fn new(registry: ViewerRegistry, auth: AuthPolicy, limits: Limits) -> Self {
        ServerState {
            registry,
            auth,
            frames: FrameCache::new(limits.frame_interval, limits.long_poll),
            permits: Arc::new(Semaphore::new(limits.max_connections)),
            limits,
            control: None,
            read_only: false,
            idle_timeout: crate::control::DEFAULT_IDLE_TIMEOUT,
            rates: Rates::default(),
            rate_limits: RateLimits::default(),
        }
    }

    /// Serve the control and input routes, ending leases after
    /// `idle_timeout` without input.
    #[must_use]
    pub(crate) fn with_control(mut self, control: ViewerControl, idle_timeout: Duration) -> Self {
        self.control = Some(control);
        self.idle_timeout = idle_timeout;
        self
    }

    /// Remove every write route.
    #[must_use]
    pub(crate) fn read_only(mut self) -> Self {
        self.read_only = true;
        self.control = None;
        self
    }

    /// Use other per-session request rates.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_rate_limits(mut self, rate_limits: RateLimits) -> Self {
        self.rate_limits = rate_limits;
        self
    }

    /// Connections currently being served.
    #[cfg(test)]
    pub(crate) fn active_connections(&self) -> usize {
        self.limits.max_connections - self.permits.available_permits()
    }
}

/// A running viewer. Dropping it stops every listener and connection.
pub(crate) struct ViewerHandle {
    tasks: Vec<JoinHandle<()>>,
    _gate: Option<GateGuard>,
}

impl Drop for ViewerHandle {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Serve `listeners` until the returned handle is dropped.
pub(crate) fn serve(
    listeners: Vec<TcpListener>,
    state: Arc<ServerState>,
    gate: Option<GateGuard>,
) -> ViewerHandle {
    let mut tasks: Vec<JoinHandle<()>> = listeners
        .into_iter()
        .map(|listener| tokio::spawn(accept_loop(listener, Arc::clone(&state))))
        .collect();
    if let Some(control) = state.control.clone() {
        // Ends leases whose heartbeat or input stopped.
        let idle_timeout = state.idle_timeout;
        tasks.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                tick.tick().await;
                control.sweep(idle_timeout).await;
            }
        }));
    }
    ViewerHandle { tasks, _gate: gate }
}

/// How often lease timeouts are checked.
const SWEEP_INTERVAL: Duration = Duration::from_millis(500);

async fn accept_loop(listener: TcpListener, state: Arc<ServerState>) {
    // Connection tasks live in this set: aborting the accept task drops the
    // set, which aborts every connection it serves.
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, peer)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Ok(permit) = Arc::clone(&state.permits).try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let state = Arc::clone(&state);
                connections.spawn(async move {
                    serve_connection(stream, peer.ip(), state).await;
                    drop(permit);
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_connection(stream: TcpStream, peer: std::net::IpAddr, state: Arc<ServerState>) {
    let io = TokioIo::new(WriteDeadline::new(stream, state.limits.write_timeout));
    let service_state = Arc::clone(&state);
    let service = service_fn(move |req: Request<Incoming>| {
        let state = Arc::clone(&service_state);
        async move { Ok::<_, Infallible>(handle(req, peer, &state).await) }
    });
    let _ = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(state.limits.header_read_timeout)
        .serve_connection(io, service)
        .await;
}

/// A parsed route.
enum Route {
    Document,
    Sessions,
    Frame(SessionId),
    Events(SessionId),
    Record(SessionId),
    Annotate(SessionId),
    Recordings,
    Recording(String),
    RecordingEvents(String),
    Segment(String, u32),
    Keep(String),
    Control(SessionId),
    Input(SessionId),
    /// A known path whose id does not parse.
    BadId,
}

impl Route {
    /// The methods this route takes.
    fn allows(&self, method: &Method) -> bool {
        match self {
            // Unknown or malformed ids are 404 whatever the method.
            Route::BadId => true,
            Route::Document | Route::Sessions | Route::Frame(_) | Route::Events(_) => {
                method == Method::GET
            }
            Route::Record(_)
            | Route::Annotate(_)
            | Route::Keep(_)
            | Route::Control(_)
            | Route::Input(_) => method == Method::POST,
            Route::Recordings
            | Route::Recording(_)
            | Route::RecordingEvents(_)
            | Route::Segment(..) => method == Method::GET || method == Method::HEAD,
        }
    }
}

fn session_id(raw: &str) -> Option<SessionId> {
    percent_decode(raw)
        .filter(|id| !id.contains('/'))
        .and_then(|id| id.parse::<SessionId>().ok())
}

fn recording_id(raw: &str) -> Option<String> {
    crate::recording::store::valid_id(raw).then(|| raw.to_owned())
}

fn parse_route(path: &str) -> Option<Route> {
    if path == "/" {
        return Some(Route::Document);
    }
    if path == "/api/sessions" {
        return Some(Route::Sessions);
    }
    if path == "/api/recordings" {
        return Some(Route::Recordings);
    }
    if let Some(rest) = path.strip_prefix("/api/sessions/") {
        let (raw, tail) = rest.rsplit_once('/')?;
        let make: fn(SessionId) -> Route = match tail {
            "frame" => Route::Frame,
            "events" => Route::Events,
            "recording" => Route::Record,
            "annotations" => Route::Annotate,
            "control" => Route::Control,
            "input" => Route::Input,
            _ => return None,
        };
        return Some(session_id(raw).map_or(Route::BadId, make));
    }
    if let Some(rest) = path.strip_prefix("/api/recordings/") {
        let mut parts = rest.split('/');
        let raw = parts.next()?;
        let route = match (parts.next(), parts.next(), parts.next()) {
            (None, _, _) => recording_id(raw).map(Route::Recording),
            (Some("events"), None, _) => recording_id(raw).map(Route::RecordingEvents),
            (Some("keep"), None, _) => recording_id(raw).map(Route::Keep),
            (Some("segments"), Some(n), None) => {
                let number = n
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (1..=crate::recording::store::MAX_SEGMENT).contains(n))
                    .filter(|_| n.bytes().all(|b| b.is_ascii_digit()));
                match (recording_id(raw), number) {
                    (Some(id), Some(number)) => Some(Route::Segment(id, number)),
                    _ => None,
                }
            }
            _ => return None,
        };
        return Some(route.unwrap_or(Route::BadId));
    }
    None
}

/// Route one request. Every response carries the security headers.
pub(crate) async fn handle(
    req: Request<Incoming>,
    peer: std::net::IpAddr,
    state: &ServerState,
) -> Response<Body> {
    let (parts, body) = req.into_parts();
    if let Err(status) = state.auth.check(&parts) {
        return empty(status);
    }
    let method = parts.method.clone();
    let Some(route) = parse_route(parts.uri.path()) else {
        return if method == Method::GET || method == Method::HEAD {
            empty(StatusCode::NOT_FOUND)
        } else {
            empty(StatusCode::METHOD_NOT_ALLOWED)
        };
    };
    if !route.allows(&method) {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    // Read-only: no write route exists, the same answer on every address.
    // `route.allows` has passed, so POST means exactly the write routes.
    if method == Method::POST && state.read_only {
        return empty(StatusCode::NOT_FOUND);
    }
    let control = match (&route, &state.control) {
        (Route::Control(_) | Route::Input(_), None) => return empty(StatusCode::NOT_FOUND),
        (Route::Control(_) | Route::Input(_), Some(control)) => {
            if let Err(status) = state.auth.check_control(&parts) {
                return empty(status);
            }
            Some(control)
        }
        _ => None,
    };
    let registry = &state.registry;
    match route {
        Route::Document => document(),
        Route::Sessions => sessions(state),
        Route::BadId => empty(StatusCode::NOT_FOUND),
        Route::Frame(id) => {
            let after = query_param(parts.uri.query(), "after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            frame(state, &id, after).await
        }
        Route::Events(id) => {
            let after = match query_param(parts.uri.query(), "after") {
                None => 0,
                Some(value) => match value.parse::<u64>() {
                    Ok(after) => after,
                    Err(_) => return empty(StatusCode::BAD_REQUEST),
                },
            };
            events(state, &id, after)
        }
        Route::Record(id) => {
            match replay::read_json(&parts, body, state.limits.header_read_timeout).await {
                Ok(json) => replay::post_recording(registry, &id, &json).await,
                Err(status) => empty(status),
            }
        }
        Route::Annotate(id) => {
            match replay::read_json(&parts, body, state.limits.header_read_timeout).await {
                Ok(json) => replay::post_annotation(registry, &id, &json),
                Err(status) => empty(status),
            }
        }
        Route::Control(id) => {
            match replay::read_json(&parts, body, state.limits.header_read_timeout).await {
                Ok(json) => match control {
                    Some(control) => {
                        control::post_control(
                            control,
                            &state.rates,
                            state.rate_limits,
                            &id,
                            peer,
                            json,
                        )
                        .await
                    }
                    None => empty(StatusCode::NOT_FOUND),
                },
                Err(status) => empty(status),
            }
        }
        Route::Input(id) => {
            match replay::read_json(&parts, body, state.limits.header_read_timeout).await {
                Ok(json) => match control {
                    Some(control) => {
                        control::post_input(control, &state.rates, state.rate_limits, &id, json)
                            .await
                    }
                    None => empty(StatusCode::NOT_FOUND),
                },
                Err(status) => empty(status),
            }
        }
        Route::Keep(rid) => {
            match replay::read_json(&parts, body, state.limits.header_read_timeout).await {
                Ok(json) => replay::post_keep(registry, rid, &json).await,
                Err(status) => empty(status),
            }
        }
        Route::Recordings => replay::list(registry).await,
        Route::Recording(rid) => replay::detail(registry, rid).await,
        Route::RecordingEvents(rid) => replay::events(registry, rid).await,
        Route::Segment(rid, n) => {
            let range = parts
                .headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            replay::segment(registry, rid, n, range).await
        }
    }
}

/// The retained events after `after`: 200 with the page, 204 while the
/// session is still connecting (no log yet), 410 once it left the registry.
fn events(state: &ServerState, id: &SessionId, after: u64) -> Response<Body> {
    match state.registry.events(id) {
        EventsLookup::Log(log) => match serde_json::to_vec(&log.after(after)) {
            Ok(body) => response(StatusCode::OK, "application/json", Bytes::from(body)),
            Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
        },
        EventsLookup::Unavailable => empty(StatusCode::NO_CONTENT),
        EventsLookup::Closed => response(
            StatusCode::GONE,
            "application/json",
            Bytes::from_static(br#"{"state":"closed"}"#),
        ),
    }
}

fn with_security_headers(mut response: Response<Body>) -> Response<Body> {
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        header::HeaderValue::from_static("DENY"),
    );
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            header::HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
        );
    }
    response
}

pub(crate) fn response(
    status: StatusCode,
    content_type: &'static str,
    body: Bytes,
) -> Response<Body> {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    with_security_headers(response)
}

pub(crate) fn empty(status: StatusCode) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    with_security_headers(response)
}

fn document() -> Response<Body> {
    let nonce = match super::Token::generate() {
        Ok(token) => token.expose()[..32].to_owned(),
        Err(_) => return empty(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let body = PAGE.replace("__NONCE__", &nonce);
    let mut response = response(
        StatusCode::OK,
        "text/html; charset=utf-8",
        Bytes::from(body),
    );
    let csp = format!(
        "default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; \
         img-src blob:; media-src blob:; connect-src 'self'; frame-ancestors 'none'; \
         base-uri 'none'; \
         form-action 'none'"
    );
    if let Ok(value) = header::HeaderValue::from_str(&csp) {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    response
}

fn sessions(state: &ServerState) -> Response<Body> {
    let sessions = state.registry.sessions();
    let live: Vec<SessionId> = sessions
        .iter()
        .filter_map(|s| s.status.id.parse().ok())
        .collect();
    state.frames.retain(&live);
    let body = serde_json::json!({
        "sessions": sessions,
        "control": state.control.is_some(),
        "writes": !state.read_only,
        "idle_timeout_secs": state.idle_timeout.as_secs(),
    });
    match serde_json::to_vec(&body) {
        Ok(body) => response(StatusCode::OK, "application/json", Bytes::from(body)),
        Err(_) => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn frame(state: &ServerState, id: &SessionId, after: u64) -> Response<Body> {
    match state.frames.next(&state.registry, id, after).await {
        FrameReply::Frame(encoded) => {
            let mut response = response(StatusCode::OK, "image/png", encoded.png.clone());
            let headers = response.headers_mut();
            for (name, value) in [
                ("x-frame-seq", encoded.seq),
                ("x-frame-width", u64::from(encoded.width)),
                ("x-frame-height", u64::from(encoded.height)),
                ("x-frame-encode-ms", encoded.encode_ms),
            ] {
                if let Ok(value) = header::HeaderValue::from_str(&value.to_string()) {
                    headers.insert(name, value);
                }
            }
            response
        }
        FrameReply::NoChange => empty(StatusCode::NO_CONTENT),
        FrameReply::Ended => response(
            StatusCode::GONE,
            "application/json",
            Bytes::from_static(br#"{"state":"ended"}"#),
        ),
        FrameReply::Closed => response(
            StatusCode::GONE,
            "application/json",
            Bytes::from_static(br#"{"state":"closed"}"#),
        ),
        FrameReply::Failed => empty(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// Decode `%XX` escapes (session names may contain any character).
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Closes a connection whose writes make no progress for `timeout`
/// (hyper has no response write timeout of its own).
pub(crate) struct WriteDeadline<S> {
    inner: S,
    timeout: Duration,
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<S> WriteDeadline<S> {
    pub(crate) fn new(inner: S, timeout: Duration) -> Self {
        WriteDeadline {
            inner,
            timeout,
            stalled: None,
        }
    }

    fn pending<T>(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<T>> {
        let timeout = self.timeout;
        let sleep = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "viewer response write made no progress",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WriteDeadline<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WriteDeadline<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(result) => {
                this.stalled = None;
                Poll::Ready(result)
            }
            Poll::Pending => this.pending(cx),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_flush(cx) {
            Poll::Ready(result) => {
                this.stalled = None;
                Poll::Ready(result)
            }
            Poll::Pending => this.pending(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
