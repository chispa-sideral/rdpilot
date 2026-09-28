//! Offline tests for the live viewer (Ticket 560, acceptance 563).
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
        Box::pin(async move { Ok(Box::new(FramedSession { frames }) as Box<dyn ManagedSession>) })
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
        select_tailnet_address(&[ts.clone()], WireViewerBind::Loopback, None),
        TailnetSelection::Disabled
    );
    // Valid override (present on an interface, in range).
    assert_eq!(
        select_tailnet_address(
            &[cgnat_wan.clone()],
            loopback_and_tailnet,
            Some(Ipv4Addr::new(100, 72, 1, 9))
        ),
        TailnetSelection::Selected(Ipv4Addr::new(100, 72, 1, 9))
    );
    // Invalid overrides: out of range, or not on this host.
    assert!(matches!(
        select_tailnet_address(
            &[lan.clone()],
            loopback_and_tailnet,
            Some(Ipv4Addr::new(192, 168, 122, 168))
        ),
        TailnetSelection::OverrideRejected(..)
    ));
    assert!(matches!(
        select_tailnet_address(
            &[ts.clone()],
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

/// R9: the tailnet address is selected but its bind fails (not present on
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
    let fx = Fixture::new();
    fx.open("alpha").await;
    let server = two_address_server(&fx).await;
    for &addr in &server.addrs {
        let doc_ok = format!("/?token={TOKEN}");
        let own_origin = format!("http://{addr}");
        let cases: Vec<(&str, String, Option<&str>, Vec<(&str, String)>, u16)> = vec![
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

/// R2: a top-level navigation to the printed URL from another site (a link
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

/// No route reaches input, Cua, transfer, connect or disconnect, and no
/// method other than GET is served.
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

/// R1: the viewer modules cannot reach control paths. They import only
/// `ViewerRegistry`, `FrameLookup` and `ViewFrameSource`.
#[test]
fn viewer_modules_never_import_registry_dispatch_or_managed_session() {
    let sources = [
        ("mod.rs", include_str!("viewer/mod.rs")),
        ("bind.rs", include_str!("viewer/bind.rs")),
        ("auth.rs", include_str!("viewer/auth.rs")),
        ("frames.rs", include_str!("viewer/frames.rs")),
        ("http.rs", include_str!("viewer/http.rs")),
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
