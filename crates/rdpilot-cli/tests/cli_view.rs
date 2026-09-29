//! Offline real-binary proof of `rdpilot view` (Ticket 560): the compiled
//! `rdpilot` CLI against the compiled `rdpilot-daemon` with the env-gated
//! fake connector and synthetic frames (`RDPILOT_DAEMON_TEST_CONNECTOR=1`,
//! `RDPILOT_DAEMON_TEST_FRAMES=1`). No RDP target is needed.
//!
//! Same isolation and stdio rules as `cli_lifecycle.rs`: a fresh
//! `XDG_RUNTIME_DIR` per test, and CLI output captured to files (the
//! auto-started daemon inherits the first CLI call's stdio). `rdpilot view`
//! never starts a daemon, so its stdout can be a pipe.
//!
//! Requires `cargo build --workspace` first (the daemon binary is located
//! next to the CLI binary, as in production).

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Env {
    root: PathBuf,
    xdg: PathBuf,
    capture: PathBuf,
    sink: PathBuf,
    idle_ms: u64,
    grace_ms: u64,
    calls: usize,
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Env {
    fn new(tag: &str, idle_ms: u64, grace_ms: u64) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "rdpilot-cli-view-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let xdg = root.join("xdg-runtime");
        let capture = root.join("capture");
        std::fs::create_dir_all(&xdg).unwrap();
        std::fs::create_dir_all(&capture).unwrap();
        Env {
            sink: root.join("sessions.json"),
            root,
            xdg,
            capture,
            idle_ms,
            grace_ms,
            calls: 0,
        }
    }

    fn socket(&self) -> PathBuf {
        self.xdg.join("rdpilot").join("daemon.sock")
    }

    fn bin() -> PathBuf {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
        assert!(
            bin.with_file_name("rdpilot-daemon").exists(),
            "run `cargo build --workspace` first: the rdpilot-daemon binary must sit next to rdpilot"
        );
        bin
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(Self::bin());
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.xdg)
            .env("RDPILOT_DAEMON_SINK_PATH", &self.sink)
            .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
            .env("RDPILOT_DAEMON_TEST_FRAMES", "1")
            .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", self.idle_ms.to_string())
            .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", self.grace_ms.to_string())
            .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50")
            // Never pick up the developer's own config or viewer settings.
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("HOME", &self.root)
            .env_remove("RDPILOT_VIEWER__BIND")
            .env_remove("RDPILOT_VIEWER__TAILNET_ADDRESS");
        command
    }

    /// Run a short CLI call with stdout/stderr captured to files.
    fn run(&mut self, args: &[&str]) -> Run {
        self.calls += 1;
        let out = self.capture.join(format!("call-{}-stdout.log", self.calls));
        let err = self.capture.join(format!("call-{}-stderr.log", self.calls));
        let status = self
            .command(args)
            .stdout(Stdio::from(std::fs::File::create(&out).unwrap()))
            .stderr(Stdio::from(std::fs::File::create(&err).unwrap()))
            .status()
            .unwrap();
        Run {
            status,
            stdout: std::fs::read_to_string(&out).unwrap_or_default(),
            stderr: std::fs::read_to_string(&err).unwrap_or_default(),
        }
    }

    fn connect(&mut self, name: &str) {
        let run = self.run(&[
            "connect",
            "--name",
            name,
            "--host",
            "10.0.0.5",
            "--username",
            "u",
            "--password",
            "p",
        ]);
        assert!(run.status.success(), "connect {name}: {}", run.stderr);
    }

    fn list(&mut self) -> Vec<serde_json::Value> {
        let run = self.run(&["list", "--json"]);
        assert!(run.status.success(), "list: {}", run.stderr);
        serde_json::from_str::<Vec<serde_json::Value>>(&run.stdout).unwrap()
    }

    /// Every captured CLI and daemon log (the daemon inherits call 1's stdio).
    fn captured_logs(&self) -> String {
        let mut all = String::new();
        for entry in std::fs::read_dir(&self.capture).unwrap() {
            all.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default());
        }
        all
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A running `rdpilot view`.
struct Viewer {
    child: Child,
    urls: Vec<String>,
    stdout_rest: mpsc::Receiver<String>,
    stderr_path: PathBuf,
}

impl Viewer {
    fn start(env: &Env, args: &[&str]) -> Viewer {
        let stderr_path = env.capture.join("view-stderr.log");
        let mut child = env
            .command(&[&["view"], args].concat())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(std::fs::File::create(&stderr_path).unwrap()))
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let mut urls = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let line = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|_| {
                    panic!(
                        "view printed no URL: {}",
                        std::fs::read_to_string(&stderr_path).unwrap_or_default()
                    )
                });
            if let Some(url) = line.strip_prefix("  http://") {
                urls.push(format!("http://{url}"));
            }
            if line.starts_with("Anyone with one of these URLs") {
                break;
            }
        }
        Viewer {
            child,
            urls,
            stdout_rest: rx,
            stderr_path,
        }
    }

    fn addr(&self) -> SocketAddr {
        self.urls[0]
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn token(&self) -> String {
        self.urls[0].split("token=").nth(1).unwrap().to_owned()
    }

    fn wait_exit(&mut self, within: Duration) -> ExitStatus {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "rdpilot view did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Reply {
    status: u16,
    head: String,
    body: Vec<u8>,
}

fn http_get(addr: SocketAddr, path: &str, bearer: Option<&str>) -> Reply {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(25)))
        .unwrap();
    let auth = bearer.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Reply {
        status,
        head,
        body: raw[split + 4..].to_vec(),
    }
}

fn session_ids(addr: SocketAddr, token: &str) -> Vec<String> {
    let reply = http_get(addr, "/api/sessions", Some(token));
    assert_eq!(reply.status, 200);
    let json: serde_json::Value = serde_json::from_slice(&reply.body).unwrap();
    let mut ids: Vec<String> = json["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

fn wait_for(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn sigint(child: &Child) {
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

fn socket_gone(path: &Path) -> bool {
    !path.exists()
}

#[test]
fn view_without_a_daemon_fails_and_starts_nothing() {
    let mut env = Env::new("nodaemon", 60_000, 1_000);
    let run = env.run(&["view", "--bind", "loopback"]);
    assert!(!run.status.success());
    assert!(
        run.stderr.contains("no rdpilot daemon is running"),
        "{}",
        run.stderr
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(socket_gone(&env.socket()), "view must never start a daemon");
}

#[test]
fn view_serves_two_sessions_tracks_close_and_stops_on_ctrl_c() {
    let mut env = Env::new("sessions", 60_000, 1_000);
    env.connect("alpha");
    env.connect("bravo");
    let mut viewer = Viewer::start(&env, &["--bind", "loopback"]);
    assert_eq!(viewer.urls.len(), 1, "loopback only: {:?}", viewer.urls);
    assert!(viewer.urls[0].starts_with("http://127.0.0.1:"));
    let token = viewer.token();
    assert_eq!(token.len(), 64);
    let addr = viewer.addr();

    // The printed URL opens the page; the list shows both sessions.
    let page = http_get(addr, &format!("/?token={token}"), None);
    assert_eq!(page.status, 200);
    assert_eq!(http_get(addr, "/", None).status, 403);
    assert_eq!(session_ids(addr, &token), ["alpha", "bravo"]);

    // Frames of both sessions, concurrently.
    let fetchers: Vec<_> = ["alpha", "bravo"]
        .into_iter()
        .map(|id| {
            let token = token.clone();
            std::thread::spawn(move || {
                let mut seq = 0_u64;
                let mut sizes = Vec::new();
                for _ in 0..3 {
                    let reply = http_get(
                        addr,
                        &format!("/api/sessions/{id}/frame?after={seq}"),
                        Some(&token),
                    );
                    assert_eq!(reply.status, 200, "{id}: {}", reply.head);
                    assert_eq!(&reply.body[..4], &[0x89, b'P', b'N', b'G']);
                    let header = |name: &str| -> u64 {
                        reply
                            .head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case(name)
                                    .then(|| v.trim().parse().ok())?
                            })
                            .unwrap()
                    };
                    let next = header("x-frame-seq");
                    assert!(next > seq);
                    seq = next;
                    sizes.push((header("x-frame-width"), header("x-frame-height")));
                }
                sizes
            })
        })
        .collect();
    for fetcher in fetchers {
        let sizes = fetcher.join().unwrap();
        assert!(sizes.iter().all(|s| *s == (64, 48) || *s == (96, 64)));
    }

    // A second viewer is refused while this one runs.
    let second = env.run(&["view", "--bind", "loopback"]);
    assert!(!second.status.success());
    assert!(
        second.stderr.contains("already running"),
        "{}",
        second.stderr
    );

    // Closing a session removes it from the list; its frames report closed.
    let run = env.run(&["disconnect", "--session", "alpha"]);
    assert!(run.status.success());
    assert_eq!(session_ids(addr, &token), ["bravo"]);
    let closed = http_get(addr, "/api/sessions/alpha/frame?after=0", Some(&token));
    assert_eq!(closed.status, 410);
    assert_eq!(closed.body, br#"{"state":"closed"}"#);

    // The viewer never changed the sessions' activity.
    let bravo = env.list().into_iter().find(|s| s["id"] == "bravo").unwrap();
    assert_eq!(bravo["last_activity"], bravo["connected_since"]);

    // Ctrl-C stops the viewer; the port closes.
    sigint(&viewer.child);
    let status = viewer.wait_exit(Duration::from_secs(5));
    assert!(status.success(), "{}", viewer.stderr());
    wait_for("viewer port to close", Duration::from_secs(3), || {
        TcpStream::connect(addr).is_err()
    });
    while viewer.stdout_rest.try_recv().is_ok() {}

    // The token appears in no CLI or daemon log.
    assert!(!env.captured_logs().contains(&token));

    let run = env.run(&["disconnect", "--session", "bravo"]);
    assert!(run.status.success());
    // Let the daemon exit before the temp root is removed.
    wait_for("daemon self-shutdown", Duration::from_secs(5), || {
        socket_gone(&env.socket())
    });
}

/// With `rdpilot view` running and a client long-polling one session's
/// frames, that session is reaped at the same idle deadline as an unwatched
/// one, and after the registry empties the daemon exits within the grace
/// period and `rdpilot view` exits with the "daemon exited" message.
#[test]
fn an_open_viewer_changes_neither_idle_reaping_nor_self_shutdown() {
    const IDLE_MS: u64 = 3_000;
    const GRACE_MS: u64 = 700;
    let mut env = Env::new("lifecycle", IDLE_MS, GRACE_MS);
    let watched_at = Instant::now();
    env.connect("watched");
    let unwatched_at = Instant::now();
    env.connect("unwatched");
    let mut viewer = Viewer::start(&env, &["--bind", "loopback"]);
    let (addr, token) = (viewer.addr(), viewer.token());

    let poller = std::thread::spawn(move || {
        let mut seq = 0_u64;
        let mut frames = 0_u32;
        loop {
            let reply = http_get(
                addr,
                &format!("/api/sessions/watched/frame?after={seq}"),
                Some(&token),
            );
            match reply.status {
                200 => {
                    frames += 1;
                    seq = reply
                        .head
                        .lines()
                        .find_map(|l| l.strip_prefix("x-frame-seq: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                }
                204 => {}
                other => return (frames, other),
            }
        }
    });

    let mut reaped: [Option<Duration>; 2] = [None, None];
    let deadline = Instant::now() + Duration::from_secs(IDLE_MS / 1000 + 5);
    while reaped.iter().any(Option::is_none) {
        assert!(Instant::now() < deadline, "sessions were not reaped");
        let sessions = env.list();
        for (i, (name, since)) in [("watched", watched_at), ("unwatched", unwatched_at)]
            .into_iter()
            .enumerate()
        {
            match sessions.iter().find(|s| s["id"] == name) {
                Some(s) => assert_eq!(
                    s["last_activity"], s["connected_since"],
                    "viewing must not refresh activity"
                ),
                None if reaped[i].is_none() => reaped[i] = Some(since.elapsed()),
                None => {}
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let emptied = Instant::now();
    let idle = Duration::from_millis(IDLE_MS);
    let (watched, unwatched) = (reaped[0].unwrap(), reaped[1].unwrap());
    assert!(watched >= idle, "watched reaped early: {watched:?}");
    assert!(
        watched <= idle + Duration::from_millis(1_500),
        "watched reaped late: {watched:?}"
    );
    let skew = watched.abs_diff(unwatched);
    assert!(
        skew <= Duration::from_millis(600),
        "watched {watched:?} vs unwatched {unwatched:?}"
    );

    // The daemon exits after the grace period although the viewer's IPC
    // connection and HTTP clients are still open.
    wait_for(
        "daemon self-shutdown",
        Duration::from_millis(GRACE_MS + 3_000),
        || socket_gone(&env.socket()),
    );
    assert!(emptied.elapsed() <= Duration::from_millis(GRACE_MS + 2_000));
    let status = viewer.wait_exit(Duration::from_secs(5));
    assert!(!status.success());
    assert!(
        viewer.stderr().contains("daemon exited; viewer stopped"),
        "{}",
        viewer.stderr()
    );
    let (frames, final_status) = poller.join().unwrap();
    assert!(frames >= 2, "the watcher saw {frames} frames");
    assert_eq!(final_status, 410, "the watched session reported closed");
}
