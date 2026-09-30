//! Offline tests for the live viewer.
//!
//! They live outside `src/viewer/` so the fixtures may build a full
//! `Registry`, while the source scan below proves the viewer modules
//! themselves never import it.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rdpilot::ConnectionConfig;
use rdpilot_ipc::{SessionId, SessionLifecycle, WireViewerBind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::lifecycle::{self, LifecycleConfig, ShutdownSignal};
use crate::registry::{Registry, ViewerRegistry};
use crate::seams::{
    BoxFuture, DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector,
    ViewFrameSource,
};
use crate::synthetic_frames::SyntheticFrames;
use crate::viewer::{
    bind_listeners, is_tailnet_range, select_tailnet_address, serve, AuthPolicy, Encoded,
    Interface, Limits, ServerState, TailnetSelection, Token, ViewerGate, ViewerHandle,
    ViewerParams,
};

const TOKEN: &str = "5ec2e7a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8";

// --- Fixtures ------------------------------------------------------------

/// A fake session whose frame source is a [`SyntheticFrames`] the test feeds.
struct FramedSession {
    frames: Arc<SyntheticFrames>,
}

impl ManagedSession for FramedSession {
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>> {
        self.frames.end();
        Box::pin(async { Ok(()) })
    }
    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
    }
    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async { Err(DaemonError::Connect("unused".into())) })
    }
    fn send_mouse(&self, _: rdpilot::MouseAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn send_key(&self, _: rdpilot::KeyAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn upload_file(
        &self,
        _: std::path::PathBuf,
        _: String,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Err(DaemonError::Connect("unused".into())) })
    }
    fn download_file(
        &self,
        _: String,
        _: std::path::PathBuf,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Err(DaemonError::Connect("unused".into())) })
    }
    fn ping(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn desktop_size(&self) -> (u32, u32) {
        (64, 48)
    }
    fn deploy_and_launch(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn frame_source(&self) -> Option<Arc<dyn ViewFrameSource>> {
        Some(Arc::clone(&self.frames) as Arc<dyn ViewFrameSource>)
    }
}

/// Connects [`FramedSession`]s and remembers each one's frames by host.
#[derive(Default)]
struct FramedConnector {
    frames: Mutex<HashMap<String, Arc<SyntheticFrames>>>,
    /// When set, the next connect waits for this before it completes.
    hold: Mutex<Option<Arc<tokio::sync::Notify>>>,
}

impl SessionConnector for FramedConnector {
    fn connect(
        &self,
        cfg: ConnectionConfig,
    ) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        let frames = SyntheticFrames::new();
        self.frames
            .lock()
            .unwrap()
            .insert(cfg.host().to_owned(), Arc::clone(&frames));
        let hold = self.hold.lock().unwrap().take();
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            Ok(Box::new(FramedSession { frames }) as Box<dyn ManagedSession>)
        })
    }
}

struct Fixture {
    registry: Arc<Registry>,
    connector: Arc<FramedConnector>,
}

impl Fixture {
    fn new() -> Self {
        let connector = Arc::new(FramedConnector::default());
        let registry = Arc::new(Registry::new(
            Arc::clone(&connector) as Arc<dyn SessionConnector>,
            Arc::new(NoopReconciliationSink),
        ));
        Fixture {
            registry,
            connector,
        }
    }

    /// Open a session named `name` whose host (the frames key) is `name`.
    async fn open(&self, name: &str) -> (SessionId, Arc<SyntheticFrames>) {
        let id = self
            .registry
            .open(
                Some(name.into()),
                name.into(),
                ConnectionConfig::new(name, "user", "pw"),
            )
            .await
            .unwrap();
        let frames = Arc::clone(&self.connector.frames.lock().unwrap()[name]);
        (id, frames)
    }

    fn viewer_registry(&self) -> ViewerRegistry {
        ViewerRegistry::new(Arc::clone(&self.registry))
    }

    /// Serve on the given listeners with `limits`.
    fn serve(&self, listeners: Vec<TcpListener>, limits: Limits) -> Server {
        let addrs: Vec<SocketAddr> = listeners.iter().map(|l| l.local_addr().unwrap()).collect();
        let policy = AuthPolicy::new(Token::from_test_value(TOKEN), &addrs);
        let state = Arc::new(ServerState::new(self.viewer_registry(), policy, limits));
        let handle = serve(listeners, Arc::clone(&state), None);
        Server {
            addrs,
            state,
            _handle: handle,
        }
    }

    async fn serve_loopback(&self, limits: Limits) -> Server {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        self.serve(vec![listener], limits)
    }
}

struct Server {
    addrs: Vec<SocketAddr>,
    state: Arc<ServerState>,
    _handle: ViewerHandle,
}

impl Server {
    fn addr(&self) -> SocketAddr {
        self.addrs[0]
    }
}

fn fast_limits() -> Limits {
    Limits {
        long_poll: Duration::from_millis(1500),
        ..Limits::default()
    }
}

// --- A minimal HTTP/1.1 client --------------------------------------------

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Send one request (`Connection: close`) and read the whole reply.
/// `host` defaults to `addr`; `extra` headers are added verbatim.
async fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    host: Option<&str>,
    extra: &[(&str, &str)],
) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let host = host.map_or_else(|| addr.to_string(), str::to_owned);
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut raw))
        .await
        .expect("reply within 20 s")
        .unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("complete response head");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

async fn api(addr: SocketAddr, path: &str) -> Reply {
    request(addr, "GET", path, None, &[("Authorization", &bearer())]).await
}

async fn list_ids(addr: SocketAddr) -> Vec<String> {
    let reply = api(addr, "/api/sessions").await;
    assert_eq!(reply.status, 200);
    let json: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    json["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect()
}

fn frame_path(id: &str, after: u64) -> String {
    format!("/api/sessions/{id}/frame?after={after}")
}

// --- Bind set --------------------------------------------------------------

fn iface(name: &str, addr: [u8; 4]) -> Interface {
    Interface {
        name: name.into(),
        addr: IpAddr::V4(Ipv4Addr::from(addr)),
    }
}

#[test]
fn tailnet_selection_table() {
    let ts = iface("tailscale0", [100, 65, 190, 114]);
    let lan = iface("ens18", [192, 168, 122, 168]);
    let docker = iface("docker0", [172, 17, 0, 1]);
    let cgnat_wan = iface("eth0", [100, 72, 1, 9]);
    let lo = iface("lo", [127, 0, 0, 1]);
    let loopback_and_tailnet = WireViewerBind::LoopbackAndTailnet;

    // Present: exactly one Tailscale address.
    assert_eq!(
        select_tailnet_address(
            &[lo.clone(), lan.clone(), docker.clone(), ts.clone()],
            loopback_and_tailnet,
            None
        ),
        TailnetSelection::Selected(Ipv4Addr::new(100, 65, 190, 114))
    );
    // Absent.
    assert_eq!(
        select_tailnet_address(&[lo.clone(), lan.clone()], loopback_and_tailnet, None),
        TailnetSelection::NotFound
    );
    // CGNAT address on a non-Tailscale interface is never a candidate.
    assert_eq!(
        select_tailnet_address(
            &[cgnat_wan.clone(), lan.clone()],
            loopback_and_tailnet,
            None
        ),
        TailnetSelection::NotFound
    );
    // Two candidates: ambiguous, bind none.
    let ts2 = iface("tailscale1", [100, 101, 0, 2]);
    assert!(matches!(
        select_tailnet_address(&[ts.clone(), ts2], loopback_and_tailnet, None),
        TailnetSelection::Ambiguous(c) if c.len() == 2
    ));
    // Windows and macOS interface names.
    assert!(matches!(
        select_tailnet_address(
            &[iface("Tailscale", [100, 64, 0, 1])],
            loopback_and_tailnet,
            None
        ),
        TailnetSelection::Selected(_)
    ));
    assert!(matches!(
        select_tailnet_address(
            &[iface("utun4", [100, 64, 0, 1])],
            loopback_and_tailnet,
            None
        ),
        TailnetSelection::Selected(_)
    ));
    // Config `loopback` disables the tailnet bind.
    assert_eq!(
        select_tailnet_address(std::slice::from_ref(&ts), WireViewerBind::Loopback, None),
        TailnetSelection::Disabled
    );
    // Valid override (present on an interface, in range).
    assert_eq!(
        select_tailnet_address(
            std::slice::from_ref(&cgnat_wan),
            loopback_and_tailnet,
            Some(Ipv4Addr::new(100, 72, 1, 9))
        ),
        TailnetSelection::Selected(Ipv4Addr::new(100, 72, 1, 9))
    );
    // Invalid overrides: out of range, or not on this host.
    assert!(matches!(
        select_tailnet_address(
            std::slice::from_ref(&lan),
            loopback_and_tailnet,
            Some(Ipv4Addr::new(192, 168, 122, 168))
        ),
        TailnetSelection::OverrideRejected(..)
    ));
    assert!(matches!(
        select_tailnet_address(
            std::slice::from_ref(&ts),
            loopback_and_tailnet,
            Some(Ipv4Addr::new(100, 64, 9, 9))
        ),
        TailnetSelection::OverrideRejected(..)
    ));
    assert!(matches!(
        select_tailnet_address(&[ts], loopback_and_tailnet, Some(Ipv4Addr::UNSPECIFIED)),
        TailnetSelection::OverrideRejected(..)
    ));
}

/// Whatever the interface list and settings, the selected address is a
/// tailnet address; with loopback it forms the whole bind set.
#[test]
fn selection_never_yields_an_unspecified_lan_or_public_address() {
    let interfaces = [
        iface("tailscale0", [100, 65, 190, 114]),
        iface("ens18", [192, 168, 122, 168]),
        iface("eth0", [100, 72, 1, 9]),
        iface("eth1", [8, 8, 8, 8]),
        iface("any", [0, 0, 0, 0]),
    ];
    let overrides = [
        None,
        Some(Ipv4Addr::UNSPECIFIED),
        Some(Ipv4Addr::new(8, 8, 8, 8)),
        Some(Ipv4Addr::new(192, 168, 122, 168)),
        Some(Ipv4Addr::new(100, 72, 1, 9)),
    ];
    for bind in [WireViewerBind::Loopback, WireViewerBind::LoopbackAndTailnet] {
        for subset in 0..(1 << interfaces.len()) {
            let list: Vec<Interface> = interfaces
                .iter()
                .enumerate()
                .filter(|(i, _)| subset & (1 << i) != 0)
                .map(|(_, i)| i.clone())
                .collect();
            for o in overrides {
                if let Some(addr) = select_tailnet_address(&list, bind, o).address() {
                    assert!(is_tailnet_range(addr), "{addr} selected");
                    assert!(!addr.is_unspecified() && !addr.is_private());
                }
            }
        }
    }
}

#[tokio::test]
async fn bind_without_tailnet_is_loopback_only() {
    let bound = bind_listeners(None).await.unwrap();
    assert_eq!(bound.addrs.len(), 1);
    assert_eq!(bound.addrs[0].ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(bound.notices.is_empty());
}

/// The tailnet address is selected but its bind fails (not present on
/// this host here): the viewer serves on loopback only and says so.
#[tokio::test]
async fn tailnet_bind_failure_falls_back_to_loopback_with_a_notice() {
    let bound = bind_listeners(Some(Ipv4Addr::new(100, 64, 250, 251)))
        .await
        .unwrap();
    assert_eq!(bound.addrs.len(), 1);
    assert_eq!(bound.addrs[0].ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_eq!(bound.notices.len(), 1);
    assert!(
        bound.notices[0].contains("loopback only"),
        "{:?}",
        bound.notices
    );
    // A non-tailnet address is never bound either.
    let bound = bind_listeners(Some(Ipv4Addr::UNSPECIFIED)).await.unwrap();
    assert_eq!(bound.addrs.len(), 1);
    assert!(bound.addrs[0].ip().is_loopback());
}

/// `start` binds only loopback for `bind = loopback`, one viewer at a time,
/// and the token never appears in `Debug` output.
#[tokio::test]
async fn start_binds_loopback_only_and_allows_one_viewer() {
    let fx = Fixture::new();
    let gate = ViewerGate::default();
    let params = ViewerParams {
        bind: WireViewerBind::Loopback,
        tailnet_override: None,
    };
    let started = crate::viewer::start(fx.viewer_registry(), &gate, params)
        .await
        .unwrap();
    assert_eq!(started.addresses.len(), 1);
    assert!(started.addresses[0].starts_with("http://127.0.0.1:"));
    let token = started.token.expose().to_owned();
    assert_eq!(token.len(), 64);
    assert!(!format!("{:?}", started.token).contains(&token));
    assert!(crate::viewer::start(fx.viewer_registry(), &gate, params)
        .await
        .is_err());
    let addr: SocketAddr = started.addresses[0]
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    drop(started);
    // Stopped: the port closes and the gate reopens.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(TcpStream::connect(addr).await.is_err());
    let again = crate::viewer::start(fx.viewer_registry(), &gate, params).await;
    assert!(again.is_ok());
}

// --- Access checks ---------------------------------------------------------

/// Two bound addresses: 127.0.0.1 and, on Linux, 127.0.0.2 standing in for
/// the tailnet address. Every check applies on every bound address.
async fn two_address_server(fx: &Fixture) -> Server {
    let mut listeners = vec![TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap()];
    if cfg!(target_os = "linux") {
        let port = listeners[0].local_addr().unwrap().port();
        listeners.push(
            TcpListener::bind((Ipv4Addr::new(127, 0, 0, 2), port))
                .await
                .unwrap(),
        );
    }
    fx.serve(listeners, fast_limits())
}

#[tokio::test]
async fn every_bound_address_rejects_bad_token_origin_and_host() {
    /// name, path, Host override, headers, expected status
    type Case<'a> = (
        &'a str,
        String,
        Option<&'a str>,
        Vec<(&'a str, String)>,
        u16,
    );
    let fx = Fixture::new();
    fx.open("alpha").await;
    let server = two_address_server(&fx).await;
    for &addr in &server.addrs {
        let doc_ok = format!("/?token={TOKEN}");
        let own_origin = format!("http://{addr}");
        let cases: Vec<Case> = vec![
            ("document ok", doc_ok.clone(), None, vec![], 200),
            (
                "api ok",
                "/api/sessions".into(),
                None,
                vec![("Authorization", bearer())],
                200,
            ),
            (
                "api own origin",
                "/api/sessions".into(),
                None,
                vec![
                    ("Authorization", bearer()),
                    ("Origin", own_origin.clone()),
                    ("Sec-Fetch-Site", "same-origin".into()),
                ],
                200,
            ),
            ("document no token", "/".into(), None, vec![], 403),
            (
                "document wrong token",
                "/?token=00".into(),
                None,
                vec![],
                403,
            ),
            ("api no token", "/api/sessions".into(), None, vec![], 403),
            (
                "api wrong token",
                "/api/sessions".into(),
                None,
                vec![("Authorization", "Bearer 00".into())],
                403,
            ),
            (
                "api token in query only",
                format!("/api/sessions?token={TOKEN}"),
                None,
                vec![],
                403,
            ),
            ("frame no token", frame_path("alpha", 0), None, vec![], 403),
            (
                "foreign origin",
                "/api/sessions".into(),
                None,
                vec![
                    ("Authorization", bearer()),
                    ("Origin", "http://evil.example".into()),
                ],
                403,
            ),
            (
                "null origin",
                "/api/sessions".into(),
                None,
                vec![("Authorization", bearer()), ("Origin", "null".into())],
                403,
            ),
            (
                "other-port origin",
                "/api/sessions".into(),
                None,
                vec![
                    ("Authorization", bearer()),
                    (
                        "Origin",
                        format!("http://127.0.0.1:{}", addr.port().wrapping_add(1)),
                    ),
                ],
                403,
            ),
            (
                "cross-site api",
                "/api/sessions".into(),
                None,
                vec![
                    ("Authorization", bearer()),
                    ("Sec-Fetch-Site", "cross-site".into()),
                ],
                403,
            ),
            (
                "same-site api",
                "/api/sessions".into(),
                None,
                vec![
                    ("Authorization", bearer()),
                    ("Sec-Fetch-Site", "same-site".into()),
                ],
                403,
            ),
            (
                "rebinding host",
                doc_ok.clone(),
                Some("attacker.example"),
                vec![],
                403,
            ),
            (
                "wrong port host",
                doc_ok.clone(),
                Some("127.0.0.1:1"),
                vec![],
                403,
            ),
            (
                "magicdns host",
                doc_ok.clone(),
                Some("marcdev.tailed30d.ts.net"),
                vec![],
                403,
            ),
        ];
        for (name, path, host, headers, expected) in cases {
            let headers: Vec<(&str, &str)> =
                headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let reply = request(addr, "GET", &path, host, &headers).await;
            assert_eq!(reply.status, expected, "{name} on {addr}");
            if expected == 403 {
                assert!(reply.body.is_empty(), "{name}: rejections carry no detail");
            }
        }
        // `localhost:<port>` is accepted for the loopback address only.
        let localhost = format!("localhost:{}", addr.port());
        let reply = request(
            addr,
            "GET",
            &format!("/?token={TOKEN}"),
            Some(&localhost),
            &[],
        )
        .await;
        assert_eq!(reply.status, 200);
    }
}

/// A top-level navigation to the printed URL from another site (a link
/// in a chat) is allowed; the API stays strict.
#[tokio::test]
async fn cross_site_document_navigation_is_allowed_but_api_is_not() {
    let fx = Fixture::new();
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let doc = format!("/?token={TOKEN}");
    let nav = [
        ("Sec-Fetch-Site", "cross-site"),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "document"),
    ];
    assert_eq!(request(addr, "GET", &doc, None, &nav).await.status, 200);
    // Not a navigation (for example a cross-site fetch or an iframe): refused.
    let fetch = [
        ("Sec-Fetch-Site", "cross-site"),
        ("Sec-Fetch-Mode", "cors"),
        ("Sec-Fetch-Dest", "empty"),
    ];
    assert_eq!(request(addr, "GET", &doc, None, &fetch).await.status, 403);
    let frame = [
        ("Sec-Fetch-Site", "cross-site"),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "iframe"),
    ];
    assert_eq!(request(addr, "GET", &doc, None, &frame).await.status, 403);
    // The navigation exception still needs the token.
    assert_eq!(request(addr, "GET", "/", None, &nav).await.status, 403);
    // The API never accepts a cross-site request.
    let auth = bearer();
    let api_nav = [
        ("Authorization", auth.as_str()),
        ("Sec-Fetch-Site", "cross-site"),
        ("Sec-Fetch-Mode", "navigate"),
        ("Sec-Fetch-Dest", "document"),
    ];
    assert_eq!(
        request(addr, "GET", "/api/sessions", None, &api_nav)
            .await
            .status,
        403
    );
}

/// No route reaches input, Cua, transfer, connect or disconnect; the
/// session routes and the page take GET only.
#[tokio::test]
async fn no_write_or_control_route_exists() {
    let fx = Fixture::new();
    let (id, _frames) = fx.open("alpha").await;
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let auth = bearer();
    let with_auth = [("Authorization", auth.as_str()), ("Content-Length", "0")];
    for path in ["/", "/api/sessions", "/api/sessions/alpha/frame"] {
        for method in ["POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
            let path = if path == "/" {
                format!("/?token={TOKEN}")
            } else {
                path.to_owned()
            };
            let reply = request(addr, method, &path, None, &with_auth).await;
            assert_eq!(reply.status, 405, "{method} {path}");
        }
    }
    for verb in [
        "input",
        "mouse",
        "key",
        "type",
        "click",
        "cua",
        "put",
        "get",
        "upload",
        "download",
        "connect",
        "disconnect",
        "close",
        "ping",
        "screenshot",
        "attach",
    ] {
        for path in [
            format!("/api/sessions/alpha/{verb}"),
            format!("/api/{verb}"),
            format!("/{verb}"),
            format!("/api/sessions/alpha/frame/{verb}"),
        ] {
            let reply = request(addr, "GET", &path, None, &with_auth).await;
            assert_eq!(reply.status, 404, "GET {path}");
            let reply = request(addr, "POST", &path, None, &with_auth).await;
            assert_eq!(reply.status, 405, "POST {path}");
        }
    }
    // The session is untouched.
    assert_eq!(fx.registry.list().len(), 1);
    assert_eq!(fx.registry.list()[0].id, id.as_str());
}

#[tokio::test]
async fn document_is_self_contained_with_a_per_response_nonce() {
    let fx = Fixture::new();
    let server = fx.serve_loopback(fast_limits()).await;
    let reply = request(server.addr(), "GET", &format!("/?token={TOKEN}"), None, &[]).await;
    assert_eq!(reply.status, 200);
    let body = String::from_utf8(reply.body.clone()).unwrap();
    let csp = reply.header("content-security-policy").unwrap().to_owned();
    let nonce = csp
        .split("'nonce-")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .unwrap()
        .to_owned();
    assert_eq!(nonce.len(), 32);
    assert!(body.contains(&format!("<script nonce=\"{nonce}\">")));
    assert!(!body.contains("__NONCE__"));
    assert!(csp.contains("default-src 'none'") && csp.contains("frame-ancestors 'none'"));
    assert!(csp.contains("connect-src 'self'"));
    // Replay plays segments from Blob URLs only; nothing broader.
    assert!(csp.contains("media-src blob:;"));
    assert_eq!(csp.matches("media-src").count(), 1);
    // No external fetch: no absolute resource URL at all.
    assert!(!body.contains("http://") && !body.contains("https://") && !body.contains("src=\"//"));
    // The page does not embed the token.
    assert!(!body.contains(TOKEN));
    assert_eq!(reply.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    let again = request(server.addr(), "GET", &format!("/?token={TOKEN}"), None, &[]).await;
    assert_ne!(again.header("content-security-policy").unwrap(), csp);
}

// --- Frames ---------------------------------------------------------------

#[tokio::test]
async fn frames_reach_the_client_with_resize_ended_and_closed() {
    let fx = Fixture::new();
    let (id, frames) = fx.open("alpha").await;
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();

    // No frame yet: the long-poll answers 204 after its bound.
    let reply = api(addr, &frame_path(id.as_str(), 0)).await;
    assert_eq!(reply.status, 204);

    frames.publish_solid(64, 48, 1);
    let reply = api(addr, &frame_path(id.as_str(), 0)).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("content-type"), Some("image/png"));
    assert_eq!(reply.header("x-frame-seq"), Some("1"));
    assert_eq!(reply.header("x-frame-width"), Some("64"));
    assert_eq!(reply.header("x-frame-height"), Some("48"));
    assert!(reply.header("x-frame-encode-ms").is_some());
    assert_eq!(&reply.body[..4], &[0x89, b'P', b'N', b'G']);

    // A waiting client wakes when the next frame arrives (resize here).
    let waiter = tokio::spawn(async move { api(addr, &frame_path("alpha", 1)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    frames.publish_solid(96, 64, 2);
    let reply = waiter.await.unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("x-frame-seq"), Some("2"));
    assert_eq!(reply.header("x-frame-width"), Some("96"));
    assert_eq!(reply.header("x-frame-height"), Some("64"));

    // Server-side end: 410 ended while the entry still exists.
    frames.end();
    let reply = api(addr, &frame_path(id.as_str(), 2)).await;
    assert_eq!(reply.status, 410);
    assert_eq!(reply.body, br#"{"state":"ended"}"#);
    let list: serde_json::Value =
        serde_json::from_slice(&api(addr, "/api/sessions").await.body).unwrap();
    assert_eq!(list["sessions"][0]["ended"], true);

    // Explicit close: 410 closed and the cache entry is dropped.
    fx.registry.close(&id).await.unwrap();
    let reply = api(addr, &frame_path(id.as_str(), 2)).await;
    assert_eq!(reply.status, 410);
    assert_eq!(reply.body, br#"{"state":"closed"}"#);
    assert_eq!(server.state.frames.cached_sessions(), 0);
    let reply = api(addr, &frame_path("never-existed", 0)).await;
    assert_eq!(reply.status, 410);
}

/// With the publisher at about 1 kHz for 2 s, a client that polls as fast as
/// it can receives at most `2 * 4 + 1` frames (4 fps cap).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frame_rate_cap_holds_under_a_fast_publisher() {
    let fx = Fixture::new();
    let (id, frames) = fx.open("alpha").await;
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let publisher = {
        let frames = Arc::clone(&frames);
        tokio::spawn(async move {
            let mut tick = 0;
            let started = Instant::now();
            while started.elapsed() < Duration::from_millis(2300) {
                frames.publish_solid(32, 32, tick);
                tick += 1;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            tick
        })
    };
    let started = Instant::now();
    let (mut seq, mut received) = (0_u64, 0_u32);
    while started.elapsed() < Duration::from_secs(2) {
        let reply = api(addr, &frame_path(id.as_str(), seq)).await;
        if reply.status == 200 {
            received += 1;
            seq = reply.header("x-frame-seq").unwrap().parse().unwrap();
        }
    }
    let published = publisher.await.unwrap();
    assert!(published > 500, "publisher ran at {published} frames");
    assert!(received >= 2, "received {received}");
    assert!(received <= 9, "rate cap exceeded: {received} frames in 2 s");
}

/// A client that requests a large frame and never reads it: its connection
/// ends within the write deadline, the publisher is never delayed, and
/// other clients keep receiving frames and lists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_client_blocks_neither_the_publisher_nor_other_clients() {
    let fx = Fixture::new();
    let (id, frames) = fx.open("alpha").await;
    let (id_b, frames_b) = fx.open("bravo").await;
    let limits = Limits {
        write_timeout: Duration::from_millis(300),
        ..fast_limits()
    };
    let server = fx.serve_loopback(limits).await;
    let addr = server.addr();

    frames.publish_solid(8, 8, 0);
    server
        .state
        .frames
        .seed(
            &id,
            Encoded {
                seq: 1,
                width: 8,
                height: 8,
                png: Bytes::from(vec![0_u8; 64 << 20]),
                at: Instant::now(),
                encode_ms: 0,
            },
        )
        .await;
    let mut stalled = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "GET {} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {}\r\n\r\n",
        frame_path(id.as_str(), 0),
        bearer()
    );
    stalled.write_all(head.as_bytes()).await.unwrap();
    // Never read from `stalled`.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.state.active_connections(), 1);

    // The publisher is not delayed.
    let started = Instant::now();
    for tick in 0..1000 {
        frames.publish_solid(8, 8, tick);
    }
    assert!(started.elapsed() < Duration::from_secs(2));

    // Another client still gets frames and the list.
    frames_b.publish_solid(16, 16, 1);
    let reply = api(addr, &frame_path(id_b.as_str(), 0)).await;
    assert_eq!(reply.status, 200);
    assert_eq!(list_ids(addr).await.len(), 2);

    // The stalled connection's task ends within the write deadline.
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.state.active_connections() > 0 {
        assert!(Instant::now() < deadline, "stalled connection still held");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(stalled);
}

/// Idle connections are closed after the header timeout, and connections
/// beyond the cap are closed at accept.
#[tokio::test]
async fn connection_cap_and_header_timeout_bound_idle_clients() {
    let fx = Fixture::new();
    let limits = Limits {
        max_connections: 2,
        header_read_timeout: Duration::from_millis(400),
        ..fast_limits()
    };
    let server = fx.serve_loopback(limits).await;
    let addr = server.addr();
    let mut a = TcpStream::connect(addr).await.unwrap();
    let mut b = TcpStream::connect(addr).await.unwrap();
    a.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.state.active_connections(), 2);
    let mut c = TcpStream::connect(addr).await.unwrap();
    let mut buf = [0_u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(2), c.read(&mut buf))
        .await
        .expect("excess connection closed at accept")
        .unwrap_or(0);
    assert_eq!(n, 0);
    for s in [&mut a, &mut b] {
        let _ = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
            .await
            .expect("idle connection closed by the header timeout");
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while server.state.active_connections() > 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

// --- Session list and passive observer -------------------------------------

#[tokio::test]
async fn session_list_reflects_connect_and_close() {
    let fx = Fixture::new();
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    assert!(list_ids(addr).await.is_empty());
    let (a, _) = fx.open("alpha").await;
    fx.open("bravo").await;
    let mut ids = list_ids(addr).await;
    ids.sort();
    assert_eq!(ids, ["alpha", "bravo"]);
    let reply = api(addr, "/api/sessions").await;
    let json: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    let first = &json["sessions"][0];
    for key in [
        "id",
        "name",
        "host",
        "status",
        "connected_since",
        "last_activity",
        "ended",
        "frames",
    ] {
        assert!(first.get(key).is_some(), "missing {key}");
    }
    assert!(!String::from_utf8_lossy(&reply.body).contains("pw"));
    fx.registry.close(&a).await.unwrap();
    assert_eq!(list_ids(addr).await, ["bravo"]);
}

/// Frames flow while an agent operation holds the session's mutex, and
/// viewing never refreshes the session's activity.
#[tokio::test]
async fn viewing_needs_no_session_lock_and_never_touches_activity() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let fx = Fixture::new();
            let (id, frames) = fx.open("alpha").await;
            let server = fx.serve_loopback(fast_limits()).await;
            let addr = server.addr();
            let registry = Arc::clone(&fx.registry);
            let busy_id = id.clone();
            let busy = tokio::task::spawn_local(async move {
                registry
                    .call(&busy_id, |_s| {
                        Box::pin(async {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            Ok(())
                        })
                    })
                    .await
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            let idle_before = fx.registry.live_idle_durations()[0].1;
            let started = Instant::now();
            let mut seq = 0;
            for tick in 0..3 {
                frames.publish_solid(16, 16, tick);
                let reply = api(addr, &frame_path(id.as_str(), seq)).await;
                assert_eq!(reply.status, 200);
                seq = reply.header("x-frame-seq").unwrap().parse().unwrap();
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "frames waited for the session lock"
            );
            let idle_after = fx.registry.live_idle_durations()[0].1;
            assert!(idle_after >= idle_before + started.elapsed() - Duration::from_millis(20));
            busy.await.unwrap().unwrap();
        })
        .await;
}

/// With a viewer long-polling the session, the idle reaper still closes it
/// on schedule, and the empty-registry watcher still fires.
#[tokio::test]
async fn an_open_viewer_changes_neither_idle_reaping_nor_self_shutdown() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let fx = Fixture::new();
            let opened = Instant::now();
            let (id, frames) = fx.open("alpha").await;
            frames.spawn_animation();
            let server = fx.serve_loopback(fast_limits()).await;
            let addr = server.addr();
            let viewer_id = id.clone();
            let viewer = tokio::spawn(async move {
                let mut seq = 0;
                let mut frames_seen = 0;
                loop {
                    let reply = api(addr, &frame_path(viewer_id.as_str(), seq)).await;
                    match reply.status {
                        200 => {
                            frames_seen += 1;
                            seq = reply.header("x-frame-seq").unwrap().parse().unwrap();
                        }
                        204 => {}
                        _ => return (frames_seen, reply.status),
                    }
                }
            });
            let cfg = LifecycleConfig {
                idle_timeout: Duration::from_millis(800),
                empty_grace: Duration::from_millis(300),
                reap_interval: Duration::from_millis(50),
            };
            let shutdown = ShutdownSignal::new();
            let reaper = tokio::task::spawn_local(lifecycle::idle_reaper(
                Arc::clone(&fx.registry),
                cfg,
                shutdown.clone(),
            ));
            let watcher = tokio::task::spawn_local(lifecycle::empty_watcher(
                Arc::clone(&fx.registry),
                cfg,
                shutdown.clone(),
            ));
            while !fx.registry.is_empty() {
                assert!(opened.elapsed() < Duration::from_secs(5), "never reaped");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let reaped_after = opened.elapsed();
            assert!(
                reaped_after >= cfg.idle_timeout,
                "reaped early: {reaped_after:?}"
            );
            assert!(
                reaped_after <= cfg.idle_timeout + Duration::from_millis(400),
                "reaped late: {reaped_after:?}"
            );
            let emptied = Instant::now();
            tokio::time::timeout(Duration::from_secs(3), shutdown.wait())
                .await
                .expect("self-shutdown still fires with a viewer open");
            assert!(emptied.elapsed() <= cfg.empty_grace + Duration::from_millis(400));
            let (frames_seen, final_status) = viewer.await.unwrap();
            assert!(frames_seen >= 2, "viewer saw {frames_seen} frames");
            assert_eq!(final_status, 410);
            let _ = reaper.await;
            let _ = watcher.await;
        })
        .await;
}

// --- Structure --------------------------------------------------------------

/// The viewer modules cannot reach control paths. They import only
/// `ViewerRegistry`, `FrameLookup` and `ViewFrameSource`.
#[test]
fn viewer_modules_never_import_registry_dispatch_or_managed_session() {
    let sources = [
        ("mod.rs", include_str!("viewer/mod.rs")),
        ("bind.rs", include_str!("viewer/bind.rs")),
        ("auth.rs", include_str!("viewer/auth.rs")),
        ("frames.rs", include_str!("viewer/frames.rs")),
        ("http.rs", include_str!("viewer/http.rs")),
        ("replay.rs", include_str!("viewer/replay.rs")),
        ("page.html", include_str!("viewer/page.html")),
    ];
    for (name, source) in sources {
        let code: String = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            "dispatch",
            "ManagedSession",
            "ManagedCua",
            "touch_generation",
            "attach_cua",
            "SessionConnector",
            "Request::",
            "rdpilot::Session",
        ] {
            assert!(!code.contains(forbidden), "{name} mentions {forbidden}");
        }
        // `Registry` only as part of `ViewerRegistry`.
        for (i, _) in code.match_indices("Registry") {
            assert!(
                code[..i].ends_with("Viewer"),
                "{name} names Registry directly"
            );
        }
    }
}

// --- Session event log: server-side end -----------------------------------

fn ended_markers(log: &crate::events::SessionEvents) -> usize {
    log.after(0)
        .events
        .iter()
        .filter(|e| e.kind == crate::events::EventKind::SessionEnded)
        .count()
}

#[tokio::test]
async fn a_server_side_end_records_one_session_ended_marker() {
    let fx = Fixture::new();
    let (id, frames) = fx.open("alpha").await;
    let log = fx.registry.events(&id).unwrap();
    frames.publish_solid(64, 48, 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(ended_markers(&log), 0);
    frames.end();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ended_markers(&log), 1);
    let ended = log.after(0).events.pop().unwrap();
    assert_eq!(ended.source, crate::events::EventSource::Cli);
    // Removing the ended session adds nothing more.
    fx.registry.close(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(ended_markers(&log), 1);
}

#[tokio::test]
async fn closing_a_session_stops_its_watcher_without_a_marker() {
    let fx = Fixture::new();
    let (id, frames) = fx.open("alpha").await;
    let log = fx.registry.events(&id).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    // The watcher holds the frame source while it runs.
    let running = Arc::strong_count(&frames);
    fx.registry.close(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(ended_markers(&log), 0, "our own close is not a server end");
    assert!(Arc::strong_count(&frames) < running, "watcher released");
    assert_eq!(
        Arc::strong_count(&log),
        1,
        "entry and watcher dropped the log"
    );
}

// --- Session event log: the events route -----------------------------------

const MARKER: &str = "SECRET-MARKER-7f3a";

fn events_path(id: &str, after: Option<&str>) -> String {
    match after {
        Some(after) => format!("/api/sessions/{id}/events?after={after}"),
        None => format!("/api/sessions/{id}/events"),
    }
}

fn json(reply: &Reply) -> serde_json::Value {
    serde_json::from_slice(&reply.body).unwrap()
}

fn seqs(page: &serde_json::Value) -> Vec<u64> {
    page["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect()
}

/// name, Host override, headers, expected status
type EventsCase<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, &'a str)>, u16);

#[tokio::test]
async fn events_route_applies_the_same_checks_on_every_bound_address() {
    let fx = Fixture::new();
    fx.open("alpha").await;
    let server = two_address_server(&fx).await;
    let path = events_path("alpha", Some("0"));
    for &addr in &server.addrs {
        let auth = bearer();
        let own_origin = format!("http://{addr}");
        let cases: Vec<EventsCase> = vec![
            ("ok", None, vec![("Authorization", &auth)], 200),
            (
                "own origin",
                None,
                vec![("Authorization", &auth), ("Origin", &own_origin)],
                200,
            ),
            ("no token", None, vec![], 403),
            (
                "wrong token",
                None,
                vec![("Authorization", "Bearer 00")],
                403,
            ),
            (
                "foreign origin",
                None,
                vec![("Authorization", &auth), ("Origin", "http://evil.example")],
                403,
            ),
            (
                "cross-site",
                None,
                vec![("Authorization", &auth), ("Sec-Fetch-Site", "cross-site")],
                403,
            ),
            (
                "rebinding host",
                Some("attacker.example"),
                vec![("Authorization", &auth)],
                403,
            ),
        ];
        for (name, host, headers, expected) in cases {
            let reply = request(addr, "GET", &path, host, &headers).await;
            assert_eq!(reply.status, expected, "{name} on {addr}");
            if expected == 403 {
                assert!(reply.body.is_empty(), "{name}: rejections carry no detail");
            }
        }
        let token_in_query = format!("{path}&token={TOKEN}");
        let reply = request(addr, "GET", &token_in_query, None, &[]).await;
        assert_eq!(reply.status, 403, "token in query only on {addr}");
        let with_body = [("Authorization", auth.as_str()), ("Content-Length", "0")];
        for method in ["POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
            let reply = request(addr, method, &path, None, &with_body).await;
            assert_eq!(reply.status, 405, "{method} on {addr}");
        }
    }
}

#[tokio::test]
async fn events_route_returns_only_the_requested_session_after_seq() {
    use crate::events::{CallOutcome, EventSource};
    let fx = Fixture::new();
    let (alpha, _a) = fx.open("alpha").await;
    let (beta, _b) = fx.open("beta").await;
    let alpha_log = fx.registry.events(&alpha).unwrap();
    let beta_log = fx.registry.events(&beta).unwrap();
    let started = Instant::now();
    let call = alpha_log.call_started(EventSource::Cua, "click");
    alpha_log.call_finished(EventSource::Cua, call, "click", CallOutcome::Ok, started);
    let call = beta_log.call_started(EventSource::Cli, "screenshot");
    beta_log.call_finished(
        EventSource::Cli,
        call,
        "screenshot",
        CallOutcome::Error,
        started,
    );
    beta_log.call_started(EventSource::Cli, "mouse");
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();

    let reply = api(addr, &events_path("alpha", None)).await;
    assert_eq!(reply.status, 200);
    assert!(reply
        .header("content-type")
        .unwrap()
        .starts_with("application/json"));
    let page = json(&reply);
    assert_eq!(seqs(&page), vec![1, 2]);
    assert_eq!(page["latest"], 2);
    assert_eq!(page["header"]["session"], "alpha");
    assert_eq!(page["events"][0]["name"], "click");
    assert!(!String::from_utf8_lossy(&reply.body).contains("screenshot"));

    let page = json(&api(addr, &events_path("beta", Some("1"))).await);
    assert_eq!(seqs(&page), vec![2, 3]);
    assert_eq!(page["events"][0]["outcome"], "error");
    assert_eq!(page["header"]["session"], "beta");

    // Nothing newer: an empty page, answered at once rather than held.
    let begun = Instant::now();
    let reply = api(addr, &events_path("beta", Some("3"))).await;
    assert!(begun.elapsed() < fast_limits().long_poll / 3);
    assert_eq!(reply.status, 200);
    assert_eq!(seqs(&json(&reply)), Vec::<u64>::new());

    for bad in ["x", "-1", "", "1.5", "99999999999999999999999"] {
        let reply = api(addr, &events_path("alpha", Some(bad))).await;
        assert_eq!(reply.status, 400, "after={bad}");
    }
    let reply = api(addr, "/api/sessions/alpha/events/extra").await;
    assert_eq!(reply.status, 404);
    let reply = api(addr, "/api/sessions/nope/events").await;
    assert_eq!(reply.status, 410);
    assert_eq!(json(&reply), serde_json::json!({ "state": "closed" }));
}

#[tokio::test]
async fn events_route_is_204_while_connecting_and_410_after_close() {
    let fx = Fixture::new();
    let hold = Arc::new(tokio::sync::Notify::new());
    *fx.connector.hold.lock().unwrap() = Some(Arc::clone(&hold));
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let opening = fx.registry.open(
        Some("slow".into()),
        "slow".into(),
        ConnectionConfig::new("slow", "user", "pw"),
    );
    let probe = async {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !list_ids(addr).await.contains(&"slow".to_owned()) {
            assert!(Instant::now() < deadline, "session listed while connecting");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let reply = api(addr, &events_path("slow", Some("0"))).await;
        hold.notify_one();
        reply
    };
    let (opened, connecting) = tokio::join!(opening, probe);
    let id = opened.unwrap();
    assert_eq!(connecting.status, 204);
    assert!(connecting.body.is_empty());

    let live = api(addr, &events_path("slow", Some("0"))).await;
    assert_eq!(live.status, 200);
    fx.registry.close(&id).await.unwrap();
    let closed = api(addr, &events_path("slow", Some("0"))).await;
    assert_eq!(closed.status, 410);
    assert_eq!(json(&closed), serde_json::json!({ "state": "closed" }));
}

/// Recorded calls carry names only: nothing a caller typed or passed
/// reaches the events route.
#[tokio::test]
async fn events_route_never_carries_arguments_or_the_token() {
    use crate::events::{CallOutcome, CuaCallTracker};
    let fx = Fixture::new();
    let (id, _frames) = fx.open("alpha").await;
    let log = fx.registry.events(&id).unwrap();
    let mut tracker = CuaCallTracker::new(Arc::clone(&log));
    tracker.observe_request(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": MARKER,
        "method": "tools/call",
        "params": { "name": "type_text", "arguments": { "text": MARKER, "path": "/tmp/x" } },
    }));
    tracker.observe_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": MARKER,
        "result": { "content": [{ "type": "text", "text": MARKER }] },
    }));
    let started = Instant::now();
    let call = log.call_started(crate::events::EventSource::Cli, "put");
    log.call_finished(
        crate::events::EventSource::Cli,
        call,
        "put",
        CallOutcome::Error,
        started,
    );
    let server = fx.serve_loopback(fast_limits()).await;
    let reply = api(server.addr(), &events_path("alpha", None)).await;
    let body = String::from_utf8_lossy(&reply.body);
    assert_eq!(reply.status, 200);
    assert!(body.contains("type_text") && body.contains("put"));
    for secret in [MARKER, TOKEN, "/tmp/x", "arguments"] {
        assert!(!body.contains(secret), "{secret} leaked");
    }
}

// --- Recording routes -------------------------------------------------------

impl Fixture {
    /// A fixture whose registry records into a temporary root.
    fn recording(tag: &str) -> (Self, RecRoot) {
        let root = crate::recording::store::tests::temp_root(tag);
        let service = crate::recording::RecordingService::fixed(
            crate::recording::StorageSettings {
                root: root.clone(),
                max_fps: 4.0,
                budget_bytes: 1 << 20,
            },
            Arc::new(crate::recording::encoder::tests::FakeFactory::default()),
            60_000,
        );
        let connector = Arc::new(FramedConnector::default());
        let registry = Arc::new(Registry::with_recordings(
            Arc::clone(&connector) as Arc<dyn SessionConnector>,
            Arc::new(NoopReconciliationSink),
            service,
        ));
        (
            Fixture {
                registry,
                connector,
            },
            RecRoot(root),
        )
    }
}

/// Removes the recordings root at the end of a test.
struct RecRoot(std::path::PathBuf);

impl Drop for RecRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Send one request with a body (`Connection: close`).
async fn send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Reply {
    let mut extra: Vec<(&str, String)> =
        headers.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
    extra.push(("Content-Length", body.len().to_string()));
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in &extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut raw))
        .await
        .expect("reply within 20 s")
        .unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

async fn post_json(addr: SocketAddr, path: &str, body: serde_json::Value) -> Reply {
    let auth = bearer();
    send(
        addr,
        "POST",
        path,
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        body.to_string().as_bytes(),
    )
    .await
}

fn body_json(reply: &Reply) -> serde_json::Value {
    serde_json::from_slice(&reply.body).unwrap_or_default()
}

/// Record `alpha` from the viewer with a few frames, stop, and return the
/// recording id.
async fn viewer_recording(fx: &Fixture, addr: SocketAddr) -> String {
    let (_id, frames) = fx.open("alpha").await;
    let started = post_json(
        addr,
        "/api/sessions/alpha/recording",
        serde_json::json!({"action":"start"}),
    )
    .await;
    assert_eq!(started.status, 200, "{:?}", body_json(&started));
    let rid = body_json(&started)["id"].as_str().unwrap().to_owned();
    for tick in 0..3 {
        frames.publish_solid(16, 16, tick);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let note = post_json(
        addr,
        "/api/sessions/alpha/annotations",
        serde_json::json!({"text":"from the page"}),
    )
    .await;
    assert_eq!(note.status, 200);
    let stopped = post_json(
        addr,
        "/api/sessions/alpha/recording",
        serde_json::json!({"action":"stop"}),
    )
    .await;
    assert_eq!(stopped.status, 200);
    assert_eq!(body_json(&stopped)["changed"], true);
    tokio::time::sleep(Duration::from_millis(400)).await;
    rid
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recording_routes_apply_every_check_on_every_bound_address() {
    let (fx, _root) = Fixture::recording("viewer-rec-auth");
    fx.open("alpha").await;
    let server = two_address_server(&fx).await;
    let rid = "20260101T000000Z-0123abcd";
    let reads = [
        "/api/recordings".to_owned(),
        format!("/api/recordings/{rid}"),
        format!("/api/recordings/{rid}/events"),
        format!("/api/recordings/{rid}/segments/1"),
    ];
    let writes = [
        "/api/sessions/alpha/recording".to_owned(),
        "/api/sessions/alpha/annotations".to_owned(),
        format!("/api/recordings/{rid}/keep"),
    ];
    for &addr in &server.addrs {
        let auth = bearer();
        let own = format!("http://{addr}");
        for path in reads.iter().chain(&writes) {
            let method = if reads.contains(path) { "GET" } else { "POST" };
            for (name, host, headers) in [
                ("no token", None, vec![("Content-Type", "application/json")]),
                (
                    "wrong token",
                    None,
                    vec![
                        ("Authorization", "Bearer 00"),
                        ("Content-Type", "application/json"),
                    ],
                ),
                (
                    "foreign origin",
                    None,
                    vec![
                        ("Authorization", auth.as_str()),
                        ("Origin", "http://evil.example"),
                        ("Content-Type", "application/json"),
                    ],
                ),
                (
                    "cross-site",
                    None,
                    vec![
                        ("Authorization", auth.as_str()),
                        ("Sec-Fetch-Site", "cross-site"),
                        ("Content-Type", "application/json"),
                    ],
                ),
                (
                    "rebinding host",
                    Some("attacker.example"),
                    vec![
                        ("Authorization", auth.as_str()),
                        ("Content-Type", "application/json"),
                    ],
                ),
            ] {
                let reply = request(
                    addr,
                    method,
                    path,
                    host,
                    &[headers.as_slice(), &[("Content-Length", "0")]].concat(),
                )
                .await;
                assert_eq!(reply.status, 403, "{name}: {method} {path} on {addr}");
                assert!(reply.body.is_empty());
            }
            let reply = request(
                addr,
                method,
                &format!("{path}?token={TOKEN}"),
                None,
                &[("Content-Length", "0")],
            )
            .await;
            assert_eq!(reply.status, 403, "query token: {path}");
            // Own origin passes the checks.
            let ok = request(
                addr,
                method,
                path,
                None,
                &[
                    ("Authorization", auth.as_str()),
                    ("Origin", own.as_str()),
                    ("Content-Length", "0"),
                ],
            )
            .await;
            assert_ne!(ok.status, 403, "{method} {path}");
        }
        // Read routes: GET and HEAD only. Write routes: POST only.
        for path in &reads {
            for method in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS"] {
                let reply = request(
                    addr,
                    method,
                    path,
                    None,
                    &[("Authorization", auth.as_str()), ("Content-Length", "0")],
                )
                .await;
                assert_eq!(reply.status, 405, "{method} {path}");
            }
        }
        for path in &writes {
            for method in ["GET", "HEAD", "PUT", "DELETE", "PATCH", "OPTIONS"] {
                let reply = request(
                    addr,
                    method,
                    path,
                    None,
                    &[("Authorization", auth.as_str()), ("Content-Length", "0")],
                )
                .await;
                assert_eq!(reply.status, 405, "{method} {path}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_routes_bound_the_body_and_its_type() {
    let (fx, _root) = Fixture::recording("viewer-rec-body");
    fx.open("alpha").await;
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let auth = bearer();
    let path = "/api/sessions/alpha/annotations";
    let reply = send(
        addr,
        "POST",
        path,
        &[("Authorization", &auth), ("Content-Type", "text/plain")],
        b"{}",
    )
    .await;
    assert_eq!(reply.status, 415);
    let reply = send(addr, "POST", path, &[("Authorization", &auth)], b"{}").await;
    assert_eq!(reply.status, 415);
    let big = serde_json::json!({ "text": "x".repeat(9000) }).to_string();
    let reply = send(
        addr,
        "POST",
        path,
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        big.as_bytes(),
    )
    .await;
    assert_eq!(reply.status, 413);
    let reply = send(
        addr,
        "POST",
        path,
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/json"),
        ],
        b"{not json",
    )
    .await;
    assert_eq!(reply.status, 400);
    // Not recording: the refusal is shown, nothing is written.
    let reply = post_json(addr, path, serde_json::json!({"text":"hello"})).await;
    assert_eq!(reply.status, 409);
    assert!(body_json(&reply)["error"]
        .as_str()
        .unwrap()
        .contains("not recording"));
    let start = post_json(
        addr,
        "/api/sessions/alpha/recording",
        serde_json::json!({"action":"start"}),
    )
    .await;
    assert_eq!(start.status, 200);
    let over = post_json(addr, path, serde_json::json!({"text": "y".repeat(4097)})).await;
    assert_eq!(over.status, 400);
    assert!(body_json(&over)["error"].as_str().unwrap().contains("4096"));
    let bad = post_json(
        addr,
        "/api/sessions/alpha/recording",
        serde_json::json!({"action":"pause"}),
    )
    .await;
    assert_eq!(bad.status, 400);
    let missing = post_json(
        addr,
        "/api/sessions/ghost/recording",
        serde_json::json!({"action":"start"}),
    )
    .await;
    assert_eq!(missing.status, 404);
}

/// The checks run before any body byte is read, and a slow body is cut
/// off within the header read timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bodies_are_read_only_after_the_checks_and_within_the_timeout() {
    let (fx, _root) = Fixture::recording("viewer-rec-slow");
    fx.open("alpha").await;
    let limits = Limits {
        header_read_timeout: Duration::from_millis(500),
        ..fast_limits()
    };
    let server = fx.serve_loopback(limits).await;
    let addr = server.addr();
    // Unauthenticated, announcing a large body that never comes: 403 at once.
    let started = Instant::now();
    let reply = request(
        addr,
        "POST",
        "/api/sessions/alpha/annotations",
        None,
        &[
            ("Content-Type", "application/json"),
            ("Content-Length", "4000"),
        ],
    )
    .await;
    assert_eq!(reply.status, 403);
    assert!(started.elapsed() < Duration::from_secs(2));
    // Authenticated, the body trickles and stalls: closed within the bound.
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "POST /api/sessions/alpha/annotations HTTP/1.1\r\nHost: {addr}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{{\"te",
        bearer()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let started = Instant::now();
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw)).await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.is_empty() || text.starts_with("HTTP/1.1 408"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_reads_serve_the_recording_with_ranges_and_reject_bad_ids() {
    let (fx, _root) = Fixture::recording("viewer-rec-read");
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let rid = viewer_recording(&fx, addr).await;

    let list = api(addr, "/api/recordings").await;
    assert_eq!(list.status, 200);
    let listed = body_json(&list);
    assert_eq!(listed["recordings"][0]["id"], rid.as_str());
    assert_eq!(listed["recordings"][0]["active"], false);

    let detail = body_json(&api(addr, &format!("/api/recordings/{rid}")).await);
    assert_eq!(detail["manifest"]["codec"], "av1");
    assert_eq!(detail["manifest"]["trigger"], "viewer");
    let segments = detail["manifest"]["segments"].as_array().unwrap().len();
    assert_eq!(segments, 1);

    let events = body_json(&api(addr, &format!("/api/recordings/{rid}/events")).await);
    let events = events.as_array().unwrap();
    assert_eq!(events[0]["kind"], "recording_started");
    assert_eq!(events[0]["source"], "viewer");
    let note = events.iter().find(|e| e["kind"] == "annotation").unwrap();
    assert_eq!(note["source"], "viewer");
    let stop = events.last().unwrap();
    assert_eq!(
        (stop["kind"].as_str(), stop["source"].as_str()),
        (Some("recording_stopped"), Some("viewer"))
    );

    let seg = format!("/api/recordings/{rid}/segments/1");
    let full = api(addr, &seg).await;
    assert_eq!(full.status, 200);
    assert_eq!(full.header("content-type"), Some("video/webm"));
    assert_eq!(full.header("accept-ranges"), Some("bytes"));
    let len = full.body.len();
    assert!(len > 20);
    let auth = bearer();
    let part = request(
        addr,
        "GET",
        &seg,
        None,
        &[("Authorization", &auth), ("Range", "bytes=4-9")],
    )
    .await;
    assert_eq!(part.status, 206);
    assert_eq!(part.body, full.body[4..=9]);
    assert_eq!(
        part.header("content-range"),
        Some(format!("bytes 4-9/{len}").as_str())
    );
    let past = request(
        addr,
        "GET",
        &seg,
        None,
        &[
            ("Authorization", &auth),
            ("Range", &format!("bytes={len}-")),
        ],
    )
    .await;
    assert_eq!(past.status, 416);
    let head = request(addr, "HEAD", &seg, None, &[("Authorization", &auth)]).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(
        head.header("content-length"),
        Some(len.to_string().as_str())
    );

    for bad in [
        "/api/recordings/..".to_owned(),
        "/api/recordings/%2e%2e".to_owned(),
        format!("/api/recordings/{rid}%2f..%2fx"),
        format!("/api/recordings/{rid}0"),
        "/api/recordings/20260101T000000Z-ffffffff".to_owned(),
        format!("/api/recordings/{rid}/segments/0"),
        format!("/api/recordings/{rid}/segments/2"),
        format!("/api/recordings/{rid}/segments/1000000"),
        format!("/api/recordings/{rid}/segments/+1"),
        format!("/api/recordings/{rid}/segments/..%2f..%2fmanifest.json"),
        format!("/api/recordings/{rid}/manifest.json"),
    ] {
        let reply = api(addr, &bad).await;
        assert_eq!(reply.status, 404, "{bad}");
    }
}

/// Viewer actions do not count as activity; keep and unkeep work from the
/// page and the list shows kept, active and over-budget states.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_actions_keep_and_list_without_touching_activity() {
    let (fx, _root) = Fixture::recording("viewer-rec-keep");
    let server = fx.serve_loopback(fast_limits()).await;
    let addr = server.addr();
    let rid = viewer_recording(&fx, addr).await;
    let before = fx.registry.list()[0].last_activity.clone();
    let active = post_json(
        addr,
        "/api/sessions/alpha/recording",
        serde_json::json!({"action":"start"}),
    )
    .await;
    let active_id = body_json(&active)["id"].as_str().unwrap().to_owned();
    let again = post_json(
        addr,
        "/api/sessions/alpha/recording",
        serde_json::json!({"action":"start"}),
    )
    .await;
    assert_eq!(body_json(&again)["changed"], false);
    let kept = post_json(
        addr,
        &format!("/api/recordings/{rid}/keep"),
        serde_json::json!({"keep":true}),
    )
    .await;
    assert_eq!(kept.status, 200);
    assert_eq!(body_json(&kept)["changed"], true);
    let listed = body_json(&api(addr, "/api/recordings").await);
    let rows = listed["recordings"].as_array().unwrap();
    let find = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(find(&rid)["kept"], true);
    assert_eq!(find(&active_id)["active"], true);
    assert_eq!(listed["kept_over_budget"], false);
    assert!(listed["kept_bytes"].as_u64().unwrap() > 0);
    let sessions = body_json(&api(addr, "/api/sessions").await);
    assert_eq!(sessions["sessions"][0]["recording"], active_id.as_str());
    let unknown = post_json(
        addr,
        "/api/recordings/20260101T000000Z-ffffffff/keep",
        serde_json::json!({"keep":true}),
    )
    .await;
    assert_eq!(unknown.status, 404);
    let unkept = post_json(
        addr,
        &format!("/api/recordings/{rid}/keep"),
        serde_json::json!({"keep":false}),
    )
    .await;
    assert_eq!(body_json(&unkept)["changed"], true);
    assert_eq!(fx.registry.list()[0].last_activity, before);
}
