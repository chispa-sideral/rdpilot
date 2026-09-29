//! HTTP/1.1 serving for the live viewer: accept loops, per-connection
//! bounds, and the closed, read-only route table.
//!
//! Routes (GET only, after [`AuthPolicy::check`]):
//! - `/` — the viewer page (token in `?token=`);
//! - `/api/sessions` — the session list;
//! - `/api/sessions/{id}/frame?after=SEQ` — long-poll for a newer frame.
//!
//! Everything else is 404. No route accepts a body or reaches input, Cua,
//! transfer, connect or disconnect.
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
use hyper::{header, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rdpilot_ipc::SessionId;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Sleep;

use super::auth::{query_param, AuthPolicy};
use super::frames::{FrameCache, FrameReply, FRAME_INTERVAL};
use super::GateGuard;
use crate::registry::ViewerRegistry;

/// The page. `__NONCE__` is replaced per response.
const PAGE: &str = include_str!("page.html");

type Body = Full<Bytes>;

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
}

impl ServerState {
    pub(crate) fn new(registry: ViewerRegistry, auth: AuthPolicy, limits: Limits) -> Self {
        ServerState {
            registry,
            auth,
            frames: FrameCache::new(limits.frame_interval, limits.long_poll),
            permits: Arc::new(Semaphore::new(limits.max_connections)),
            limits,
        }
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
    let tasks = listeners
        .into_iter()
        .map(|listener| tokio::spawn(accept_loop(listener, Arc::clone(&state))))
        .collect();
    ViewerHandle { tasks, _gate: gate }
}

async fn accept_loop(listener: TcpListener, state: Arc<ServerState>) {
    // Connection tasks live in this set: aborting the accept task drops the
    // set, which aborts every connection it serves.
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Ok(permit) = Arc::clone(&state.permits).try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let state = Arc::clone(&state);
                connections.spawn(async move {
                    serve_connection(stream, state).await;
                    drop(permit);
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn serve_connection(stream: TcpStream, state: Arc<ServerState>) {
    let io = TokioIo::new(WriteDeadline::new(stream, state.limits.write_timeout));
    let service_state = Arc::clone(&state);
    let service = service_fn(move |req: Request<Incoming>| {
        let state = Arc::clone(&service_state);
        async move { Ok::<_, Infallible>(handle(req, &state).await) }
    });
    let _ = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(state.limits.header_read_timeout)
        .serve_connection(io, service)
        .await;
}

/// Route one request. Every response carries the security headers.
pub(crate) async fn handle(req: Request<Incoming>, state: &ServerState) -> Response<Body> {
    let (parts, _body) = req.into_parts();
    if let Err(status) = state.auth.check(&parts) {
        return empty(status);
    }
    let path = parts.uri.path();
    if path == "/" {
        return document();
    }
    if path == "/api/sessions" {
        return sessions(state);
    }
    if let Some(id) = path
        .strip_prefix("/api/sessions/")
        .and_then(|rest| rest.strip_suffix("/frame"))
    {
        let Some(id) = percent_decode(id)
            .filter(|id| !id.contains('/'))
            .and_then(|id| id.parse::<SessionId>().ok())
        else {
            return empty(StatusCode::NOT_FOUND);
        };
        let after = query_param(parts.uri.query(), "after")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        return frame(state, &id, after).await;
    }
    empty(StatusCode::NOT_FOUND)
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

fn response(status: StatusCode, content_type: &'static str, body: Bytes) -> Response<Body> {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    with_security_headers(response)
}

fn empty(status: StatusCode) -> Response<Body> {
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
         img-src blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; \
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
    match serde_json::to_vec(&serde_json::json!({ "sessions": sessions })) {
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
