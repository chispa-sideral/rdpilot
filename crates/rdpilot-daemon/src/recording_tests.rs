//! Daemon-level recording tests: requests through `dispatch`, a registry
//! with a recording service over a temporary root, fake sessions with test
//! frames and a scripted encoder.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rdpilot::ConnectionConfig;
use rdpilot_ipc::{
    Request, SessionId, SessionLifecycle, WireErrorCode, WireRecordingState, WireResponse,
};
use rdpilot_vocab::RecordingTrigger;

use crate::dispatch::dispatch;
use crate::events::CuaCallTracker;
use crate::recording::capture::tests::TestFrames;
use crate::recording::encoder::tests::FakeFactory;
use crate::recording::{mark_ipc_thread, RecordingService, StorageSettings};
use crate::registry::Registry;
use crate::seams::{
    BoxFuture, DaemonError, ManagedSession, NoopReconciliationSink, SessionConnector,
    ViewFrameSource,
};

const MARKER: &str = "RECMARKER-5d1e";
const PASSWORD: &str = "PASSWORD-9c2b";

struct Session {
    frames: Option<Arc<TestFrames>>,
}

impl ManagedSession for Session {
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>> {
        if let Some(f) = &self.frames {
            f.end();
        }
        Box::pin(async { Ok(()) })
    }
    fn describe(&self) -> SessionLifecycle {
        SessionLifecycle::Live
    }
    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async {
            Ok(rdpilot::Screenshot {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            })
        })
    }
    fn send_mouse(&self, _: rdpilot::MouseAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn send_key(&self, _: rdpilot::KeyAction) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn upload_file(
        &self,
        _: PathBuf,
        _: String,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Err(DaemonError::Connect("no".into())) })
    }
    fn download_file(
        &self,
        _: String,
        _: PathBuf,
    ) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Err(DaemonError::Connect("no".into())) })
    }
    fn ping(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn desktop_size(&self) -> (u32, u32) {
        (16, 12)
    }
    fn deploy_and_launch(&self) -> BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn frame_source(&self) -> Option<Arc<dyn ViewFrameSource>> {
        self.frames
            .as_ref()
            .map(|f| Arc::clone(f) as Arc<dyn ViewFrameSource>)
    }
}

/// Sessions to host `noframes` have no frame source.
#[derive(Default)]
struct Connector {
    frames: Mutex<Vec<Arc<TestFrames>>>,
}

impl SessionConnector for Connector {
    fn connect(
        &self,
        cfg: ConnectionConfig,
    ) -> BoxFuture<'static, Result<Box<dyn ManagedSession>, DaemonError>> {
        let frames = (cfg.host() != "noframes").then(|| {
            let f = TestFrames::new(16, 12);
            self.frames.lock().unwrap().push(Arc::clone(&f));
            f
        });
        Box::pin(async move { Ok(Box::new(Session { frames }) as Box<dyn ManagedSession>) })
    }
}

struct Fx {
    root: PathBuf,
    registry: Arc<Registry>,
    connector: Arc<Connector>,
    service: Arc<RecordingService>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fx(tag: &str) -> Fx {
    fx_with(tag, FakeFactory::default())
}

fn fx_with(tag: &str, factory: FakeFactory) -> Fx {
    let root = crate::recording::store::tests::temp_root(tag);
    let service = RecordingService::fixed(
        StorageSettings {
            root: root.join("recordings"),
            max_fps: 8.0,
            budget_bytes: 1 << 30,
        },
        Arc::new(factory),
        60_000,
    );
    let connector = Arc::new(Connector::default());
    let registry = Arc::new(Registry::with_recordings(
        Arc::clone(&connector) as Arc<dyn SessionConnector>,
        Arc::new(NoopReconciliationSink),
        Arc::clone(&service),
    ));
    Fx {
        root,
        registry,
        connector,
        service,
    }
}

impl Fx {
    fn recordings_root(&self) -> PathBuf {
        self.root.join("recordings")
    }

    async fn connect(
        &self,
        name: &str,
        host: &str,
        record: Option<RecordingTrigger>,
    ) -> WireResponse {
        dispatch(
            &self.registry,
            Request::Connect {
                name: Some(name.into()),
                host: host.into(),
                port: None,
                username: "user".into(),
                password: PASSWORD.into(),
                domain: None,
                accept_invalid_certs: false,
                cua_enabled: false,
                cua_version: "latest-dev".into(),
                cua_auto_download: true,
                connect_ack: false,
                record,
            },
        )
        .await
    }

    async fn run(&self, request: Request) -> WireResponse {
        dispatch(&self.registry, request).await
    }

    fn recording_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(self.recordings_root())
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        dirs.sort();
        dirs
    }

    /// Wait until every recorder thread finished writing.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn sid(name: &str) -> SessionId {
    name.parse().unwrap()
}

fn events(dir: &Path) -> Vec<serde_json::Value> {
    crate::recording::store::read_events(dir)
}

fn kinds(dir: &Path) -> Vec<String> {
    events(dir)
        .iter()
        .map(|e| e["kind"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn changed(response: &WireResponse) -> (Option<String>, bool) {
    match response {
        WireResponse::RecordingChanged { id, changed, .. } => (id.clone(), *changed),
        other => panic!("expected RecordingChanged, got {other:?}"),
    }
}

fn recording_error(response: &WireResponse) -> String {
    match response {
        WireResponse::Error(e) if e.code == WireErrorCode::Recording => e.message.clone(),
        other => panic!("expected a recording error, got {other:?}"),
    }
}

fn all_bytes(dir: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                out.extend(all_bytes(&path));
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.extend(bytes);
            }
        }
    }
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[tokio::test]
async fn a_session_with_recording_off_writes_nothing() {
    let fx = fx("rec-off");
    assert!(matches!(
        fx.connect("web", "h", None).await,
        WireResponse::Connected {
            recording: WireRecordingState::Off,
            ..
        }
    ));
    fx.run(Request::Screenshot {
        session: sid("web"),
    })
    .await;
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    assert!(
        !fx.recordings_root().exists(),
        "no directory or file at all"
    );
}

#[tokio::test]
async fn connect_with_a_trigger_records_from_connect() {
    let fx = fx("rec-connect");
    let response = fx.connect("web", "h", Some(RecordingTrigger::Host)).await;
    let WireResponse::Connected {
        recording: WireRecordingState::On { id },
        ..
    } = response
    else {
        panic!("expected a recording: {response:?}");
    };
    assert_eq!(
        fx.registry.list()[0].recording.as_deref(),
        Some(id.as_str())
    );
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    fx.settle().await;
    let dir = fx.recordings_root().join(&id);
    let manifest = crate::recording::store::Store::read_manifest(&dir).unwrap();
    assert_eq!(manifest.trigger, "host");
    assert_eq!(manifest.host, "h");
    assert_eq!(manifest.session.name.as_deref(), Some("web"));
    assert_eq!(manifest.end_reason.as_deref(), Some("session_closed"));
    let k = kinds(&dir);
    assert_eq!(k.first().map(String::as_str), Some("recording_started"));
    let log = events(&dir);
    let closed = log.iter().find(|e| e["kind"] == "session_closed").unwrap();
    assert_eq!(closed["reason"], "disconnect");
    let last = log.last().unwrap();
    assert_eq!(last["kind"], "recording_stopped");
    assert_eq!(last["reason"], "session_closed");
    assert_eq!(last["source"], "daemon");
}

/// Start on a running session records from that moment; stop writes the
/// source and the session goes on; start, stop, start gives two
/// recordings; redundant start and stop change nothing and say so.
#[tokio::test]
async fn start_and_stop_on_a_running_session() {
    let fx = fx("rec-running");
    fx.connect("web", "h", None).await;
    fx.run(Request::Screenshot {
        session: sid("web"),
    })
    .await; // before: not recorded
    let (first, was) = changed(
        &fx.run(Request::RecordStart {
            session: sid("web"),
        })
        .await,
    );
    assert!(was);
    let first = first.unwrap();
    let (again, was) = changed(
        &fx.run(Request::RecordStart {
            session: sid("web"),
        })
        .await,
    );
    assert_eq!((again.as_deref(), was), (Some(first.as_str()), false));
    fx.run(Request::DesktopSize {
        session: sid("web"),
    })
    .await;
    let (stopped, was) = changed(
        &fx.run(Request::RecordStop {
            session: sid("web"),
        })
        .await,
    );
    assert_eq!((stopped.as_deref(), was), (Some(first.as_str()), true));
    let (none, was) = changed(
        &fx.run(Request::RecordStop {
            session: sid("web"),
        })
        .await,
    );
    assert_eq!((none, was), (None, false));
    // The session continues.
    assert!(matches!(
        fx.run(Request::Screenshot {
            session: sid("web")
        })
        .await,
        WireResponse::Screenshot { .. }
    ));
    let (second, _) = changed(
        &fx.run(Request::RecordStart {
            session: sid("web"),
        })
        .await,
    );
    fx.run(Request::RecordStop {
        session: sid("web"),
    })
    .await;
    fx.settle().await;
    assert_ne!(second.as_deref(), Some(first.as_str()));
    assert_eq!(fx.recording_dirs().len(), 2);
    let dir = fx.recordings_root().join(&first);
    let log = events(&dir);
    let names: Vec<&str> = log.iter().filter_map(|e| e["name"].as_str()).collect();
    assert_eq!(
        names,
        ["desktop_size", "desktop_size"],
        "only calls after start"
    );
    assert_eq!(log[0]["kind"], "recording_started");
    assert_eq!(log[0]["trigger"], "cli");
    let last = log.last().unwrap();
    assert_eq!(
        (last["kind"].as_str(), last["source"].as_str()),
        (Some("recording_stopped"), Some("cli"))
    );
    assert_eq!(last["reason"], "requested");
    // Stopped recordings are no longer listed as the session's recording.
    assert_eq!(fx.registry.list()[0].recording, None);
}

#[tokio::test]
async fn annotations_are_written_with_source_or_refused_without_writing() {
    let fx = fx("rec-annotate");
    fx.connect("web", "h", None).await;
    let refused = fx
        .run(Request::Annotate {
            session: sid("web"),
            text: "early".into(),
        })
        .await;
    assert!(recording_error(&refused).contains("not recording"));
    let (id, _) = changed(
        &fx.run(Request::RecordStart {
            session: sid("web"),
        })
        .await,
    );
    let ok = fx
        .run(Request::Annotate {
            session: sid("web"),
            text: "look here".into(),
        })
        .await;
    assert!(changed(&ok).1);
    for bad in [String::new(), "   ".into(), "x".repeat(4097)] {
        recording_error(
            &fx.run(Request::Annotate {
                session: sid("web"),
                text: bad,
            })
            .await,
        );
    }
    assert!(
        changed(
            &fx.run(Request::Annotate {
                session: sid("web"),
                text: "y".repeat(4096),
            })
            .await
        )
        .1
    );
    fx.run(Request::RecordStop {
        session: sid("web"),
    })
    .await;
    fx.settle().await;
    let log = events(&fx.recordings_root().join(id.unwrap()));
    let notes: Vec<&serde_json::Value> = log.iter().filter(|e| e["kind"] == "annotation").collect();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0]["text"], "look here");
    assert_eq!(notes[0]["source"], "cli");
    assert!(notes[0]["at"].as_str().unwrap().ends_with('Z'));
    assert!(notes[0]["offset_ms"].as_u64().is_some());
    assert!(!log.iter().any(|e| e["text"] == "early"));
}

#[tokio::test]
async fn keep_list_and_totals_survive_a_fresh_service() {
    let fx = fx("rec-keep");
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    fx.settle().await;
    let WireResponse::Recordings {
        recordings,
        kept_bytes,
        unkept_bytes,
        budget_bytes,
        kept_over_budget,
    } = fx.run(Request::RecordingList {}).await
    else {
        panic!("expected Recordings");
    };
    assert_eq!(recordings.len(), 1);
    let id = recordings[0].id.clone();
    assert!(!recordings[0].kept && !recordings[0].active);
    assert_eq!(recordings[0].session_name.as_deref(), Some("web"));
    assert_eq!(
        (kept_bytes, budget_bytes, kept_over_budget),
        (0, 1 << 30, false)
    );
    assert!(unkept_bytes > 0);
    assert!(
        changed(
            &fx.run(Request::RecordingKeep {
                id: id.clone(),
                keep: true
            })
            .await
        )
        .1
    );
    assert!(
        !changed(
            &fx.run(Request::RecordingKeep {
                id: id.clone(),
                keep: true
            })
            .await
        )
        .1
    );
    // A fresh service over the same root (a daemon restart) sees the mark.
    let fresh = RecordingService::fixed(
        StorageSettings {
            root: fx.recordings_root(),
            max_fps: 4.0,
            budget_bytes: 1,
        },
        Arc::new(FakeFactory::default()),
        60_000,
    );
    let listing = tokio::task::spawn_blocking(move || fresh.list().unwrap())
        .await
        .unwrap();
    assert!(listing.recordings[0].kept);
    assert!(listing.kept_over_budget);
    assert_eq!(listing.unkept_bytes, 0);
    assert!(
        changed(
            &fx.run(Request::RecordingKeep {
                id: id.clone(),
                keep: false
            })
            .await
        )
        .1
    );
    let unknown = format!("{}-ffffffff", &id[..16]);
    assert!(recording_error(
        &fx.run(Request::RecordingKeep {
            id: unknown,
            keep: true
        })
        .await
    )
    .contains("unknown"));
    assert!(recording_error(
        &fx.run(Request::RecordingKeep {
            id: "../etc".into(),
            keep: true
        })
        .await
    )
    .contains("unknown"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_reap_and_aborted_connects_close_with_their_reason() {
    let fx = fx("rec-reap");
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    fx.registry.close_idle(&sid("web")).await.unwrap();
    fx.settle().await;
    let dir = fx.recording_dirs().pop().unwrap();
    let log = events(&dir);
    let closed = log.iter().find(|e| e["kind"] == "session_closed").unwrap();
    assert_eq!(closed["reason"], "idle_reap");
}

/// Strip events (native verbs, Cua calls ok, error and no reply) appear in
/// the recording with the same fields as in the live log, apart from the
/// sequence number and the offset; no argument name or value is stored.
#[tokio::test]
async fn strip_events_are_persisted_with_the_live_fields_and_no_arguments() {
    let fx = fx("rec-strip");
    fx.connect("web", "h", None).await;
    let (id, _) = changed(
        &fx.run(Request::RecordStart {
            session: sid("web"),
        })
        .await,
    );
    fx.run(Request::Key {
        session: sid("web"),
        action: rdpilot::KeyAction::Type(MARKER.into()),
    })
    .await;
    let log = fx.registry.events(&sid("web")).unwrap();
    {
        let mut tracker = CuaCallTracker::new(Arc::clone(&log));
        let call = |id: u64, name: &str| serde_json::json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":{MARKER: MARKER}}});
        tracker.observe_request(&call(1, "type_text"));
        tracker.observe_response(&serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":MARKER}]}}));
        tracker.observe_request(&call(2, "click"));
        tracker.observe_response(
            &serde_json::json!({"jsonrpc":"2.0","id":2,"error":{"code":1,"message":MARKER}}),
        );
        tracker.observe_request(&call(3, "hold"));
    } // dropped: call 3 finishes as no_reply
    let live = log.after(0).events;
    fx.run(Request::RecordStop {
        session: sid("web"),
    })
    .await;
    fx.settle().await;
    let dir = fx.recordings_root().join(id.unwrap());
    let recorded: Vec<serde_json::Value> = events(&dir)
        .into_iter()
        .filter(|e| matches!(e["kind"].as_str(), Some("call_started" | "call_finished")))
        .collect();
    assert_eq!(recorded.len(), live.len());
    for (rec, live) in recorded.iter().zip(&live) {
        let mut live = serde_json::to_value(live).unwrap();
        let mut rec = rec.clone();
        for v in [&mut live, &mut rec] {
            let o = v.as_object_mut().unwrap();
            o.remove("seq");
            o.remove("offset_ms");
        }
        assert_eq!(rec, live);
    }
    let outcomes: Vec<&str> = recorded
        .iter()
        .filter_map(|e| e["outcome"].as_str())
        .collect();
    assert_eq!(outcomes, ["ok", "ok", "error", "no_reply"]);
    let bytes = all_bytes(&fx.recordings_root());
    assert!(
        !contains(&bytes, MARKER),
        "no argument name or value, no typed text"
    );
    assert!(!contains(&bytes, PASSWORD));
}

/// Recording actions do not change `last_activity` or idle time.
#[tokio::test]
async fn recording_actions_are_not_session_activity() {
    let fx = fx("rec-passive");
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    let before = fx.registry.list()[0].last_activity.clone();
    tokio::time::sleep(Duration::from_millis(60)).await;
    fx.run(Request::Annotate {
        session: sid("web"),
        text: "n".into(),
    })
    .await;
    fx.run(Request::RecordStop {
        session: sid("web"),
    })
    .await;
    fx.run(Request::RecordStart {
        session: sid("web"),
    })
    .await;
    fx.run(Request::RecordingList {}).await;
    let idle = fx.registry.live_idle_durations();
    assert!(idle[0].1 >= Duration::from_millis(60));
    assert_eq!(fx.registry.list()[0].last_activity, before);
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    assert!(
        fx.registry.is_empty(),
        "self-shutdown sees an empty registry"
    );
}

/// With test frames and a real capture stage: frames become a segment; a
/// session without a frame source records events only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_become_segments_and_frameless_sessions_record_events_only() {
    let fx = fx("rec-frames");
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    let frames = Arc::clone(&fx.connector.frames.lock().unwrap()[0]);
    for v in 0..4_u8 {
        frames.paint(v * 40 + 1);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    fx.connect("bare", "noframes", Some(RecordingTrigger::ConnectFlag))
        .await;
    fx.run(Request::DesktopSize {
        session: sid("bare"),
    })
    .await;
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    fx.run(Request::Disconnect {
        session: sid("bare"),
    })
    .await;
    fx.settle().await;
    let mut with_video = 0;
    for dir in fx.recording_dirs() {
        let m = crate::recording::store::Store::read_manifest(&dir).unwrap();
        if m.session.id == "web" {
            assert_eq!(m.segments.len(), 1, "{m:?}");
            assert!(m.segments[0].frames >= 2);
            assert!(dir.join("segments").join("000001.webm").is_file());
            with_video += 1;
        } else {
            assert!(m.segments.is_empty());
            assert!(kinds(&dir).contains(&"call_finished".to_owned()));
        }
    }
    assert_eq!(with_video, 1);
}

#[cfg(unix)]
#[tokio::test]
async fn everything_created_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fx("rec-perms");
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    let (id, _) = changed(
        &fx.run(Request::RecordingKeep {
            id: fx.recording_dirs()[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            keep: true,
        })
        .await,
    );
    assert!(id.is_some());
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    fx.settle().await;
    fn walk(path: &Path) {
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        if path.is_dir() {
            assert_eq!(mode, 0o700, "{}", path.display());
            for entry in std::fs::read_dir(path).unwrap() {
                walk(&entry.unwrap().path());
            }
        } else {
            assert_eq!(mode, 0o600, "{}", path.display());
        }
    }
    walk(&fx.recordings_root());
}

/// Recording file I/O never runs on the IPC thread: this test's thread
/// stands for it, and the store checks where it runs.
#[tokio::test]
async fn no_recording_file_io_on_the_ipc_thread() {
    let fx = fx("rec-thread");
    mark_ipc_thread();
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    fx.run(Request::RecordStop {
        session: sid("web"),
    })
    .await;
    fx.run(Request::RecordStart {
        session: sid("web"),
    })
    .await;
    fx.run(Request::RecordingList {}).await;
    let id = fx.recording_dirs()[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fx.run(Request::RecordingKeep { id, keep: true }).await;
    fx.run(Request::Disconnect {
        session: sid("web"),
    })
    .await;
    let _ = &fx.service;
}

/// A daemon restart finishes what the previous one left open.
#[tokio::test]
async fn startup_finalizes_leftovers() {
    let fx = fx("rec-startup");
    fx.connect("web", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    let dir = fx.recording_dirs()[0].clone();
    // Another service over the same root (a new daemon) with no active
    // recordings sees this one as left open.
    let next = RecordingService::fixed(
        StorageSettings {
            root: fx.recordings_root(),
            max_fps: 4.0,
            budget_bytes: 1 << 30,
        },
        Arc::new(FakeFactory::default()),
        60_000,
    );
    tokio::task::spawn_blocking(move || next.startup())
        .await
        .unwrap();
    let m = crate::recording::store::Store::read_manifest(&dir).unwrap();
    assert_eq!(m.end_reason.as_deref(), Some("daemon_lost"));
}

/// The manifest of `dir` once its recorder finished it (at most 10 s).
async fn finished_manifest(dir: &Path) -> crate::recording::manifest::Manifest {
    for _ in 0..100 {
        let manifest = crate::recording::store::Store::read_manifest(dir).unwrap();
        if manifest.finished() {
            return manifest;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("recording {} did not finish", dir.display());
}

fn listed(response: WireResponse, id: &str) -> rdpilot_ipc::WireRecording {
    let WireResponse::Recordings { recordings, .. } = response else {
        panic!("expected Recordings");
    };
    recordings
        .into_iter()
        .find(|r| r.id == id)
        .expect("recording is listed")
}

/// A stopped recording whose recorder is still closing its last segment
/// stays active: another start does not treat it as left open by an
/// earlier daemon, and the list shows it as active with its duration.
#[tokio::test]
async fn a_start_during_another_recordings_close_keeps_its_segment() {
    let fx = fx_with(
        "rec-close-race",
        FakeFactory {
            delay: Duration::from_millis(1500),
            ..FakeFactory::default()
        },
    );
    fx.connect("one", "h", Some(RecordingTrigger::ConnectFlag))
        .await;
    fx.connect("two", "h", None).await;
    let frames = Arc::clone(&fx.connector.frames.lock().unwrap()[0]);
    frames.paint(41);
    tokio::time::sleep(Duration::from_millis(400)).await;
    frames.paint(81);
    tokio::time::sleep(Duration::from_millis(400)).await;
    // The slow encoder is still busy with the first frame.
    let (first, _) = changed(
        &fx.run(Request::RecordStop {
            session: sid("one"),
        })
        .await,
    );
    let first = first.unwrap();
    let closing = listed(fx.run(Request::RecordingList {}).await, &first);
    assert!(closing.active, "closing recording listed {closing:?}");
    assert!(
        closing.duration_ms > 0,
        "closing recording listed {closing:?}"
    );
    let (second, started) = changed(
        &fx.run(Request::RecordStart {
            session: sid("two"),
        })
        .await,
    );
    assert!(started && second.is_some());
    let dir = fx.recordings_root().join(&first);
    let manifest = finished_manifest(&dir).await;
    assert_eq!(manifest.end_reason.as_deref(), Some("requested"));
    assert_eq!(manifest.segments.len(), 1, "kinds {:?}", kinds(&dir));
    assert!(dir
        .join("segments")
        .join(&manifest.segments[0].file)
        .is_file());
    let kinds = kinds(&dir);
    assert!(kinds.iter().any(|k| k == "segment_closed"), "{kinds:?}");
    assert!(!kinds.iter().any(|k| k == "video_stopped"), "{kinds:?}");
    let closed = listed(fx.run(Request::RecordingList {}).await, &first);
    assert!(!closed.active);
    assert_eq!(closed.duration_ms, manifest.duration_ms.unwrap());
    assert!(closed.bytes > manifest.segments[0].bytes);
    fx.run(Request::RecordStop {
        session: sid("two"),
    })
    .await;
    finished_manifest(&fx.recordings_root().join(second.unwrap())).await;
}
