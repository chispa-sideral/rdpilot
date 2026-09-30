//! Session recording: a passive consumer of a session's frames and event
//! log that writes an owner-only recording directory (manifest, event log,
//! AV1/WebM video segments), plus retention and the keep mark.
//!
//! - [`RecordingService`] is daemon-wide: the storage settings, the active
//!   recordings by session log, and every recording action (start, stop,
//!   annotate, list, keep). Its callers take only the registry's brief
//!   outer lock, never a per-session lock, never `touch_generation`: a
//!   recording action is not session activity.
//! - Recording file I/O never runs on the IPC `LocalSet` thread: store work
//!   runs in `spawn_blocking`, the rest on the recorder thread.
//! - A recording failure never fails a session or an agent operation.

pub(crate) mod capture;
pub(crate) mod encoder;
pub(crate) mod i420;
pub(crate) mod log;
pub(crate) mod manifest;
pub(crate) mod recorder;
pub(crate) mod store;
pub(crate) mod webm;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime};

use rdpilot_ipc::WireRecording;
use tokio::sync::watch;

use crate::events::{
    CloseReason, EventKind, EventSource, RecordingTrigger, SessionEvents, StopReason,
};
use crate::registry::iso8601_millis_from_system_time;
use crate::seams::ViewFrameSource;

use self::capture::{Mailbox, Timeline};
use self::encoder::{EncoderFactory, Rav1eFactory, ENCODER_THREADS, QUANTIZER, SPEED, TILES};
use self::log::{EventQueue, QueueSink, Waker};
use self::manifest::{LogRef, Manifest, SessionRef, Settings, FORMAT};
use self::recorder::{Recorder, RecorderSetup, RecorderThread, SEGMENT_MS};
use self::store::Store;

/// Longest annotation, in bytes.
pub(crate) const MAX_ANNOTATION_BYTES: usize = 4096;

/// How long daemon shutdown waits for recorders to finish their last segment.
const SHUTDOWN_JOIN: Duration = Duration::from_secs(10);

/// Storage settings in effect for one action.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StorageSettings {
    pub(crate) root: PathBuf,
    pub(crate) max_fps: f64,
    pub(crate) budget_bytes: u64,
}

enum Source {
    /// Recording is not available (tests that do not exercise it).
    Disabled,
    /// Read `[recording]` from `config.toml` and the environment each time.
    Config,
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(StorageSettings),
}

/// What a recording needs from its session.
#[derive(Clone)]
pub(crate) struct Target {
    pub(crate) events: Arc<SessionEvents>,
    pub(crate) frame: Option<Arc<dyn ViewFrameSource>>,
    pub(crate) session: String,
    pub(crate) name: Option<String>,
    pub(crate) host: String,
}

/// The result of a start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Started {
    pub(crate) id: String,
    /// `false` when the session was already recording.
    pub(crate) changed: bool,
}

/// Totals of the recordings directory.
#[derive(Debug, Clone, Default)]
pub(crate) struct Listing {
    pub(crate) recordings: Vec<WireRecording>,
    pub(crate) kept_bytes: u64,
    pub(crate) unkept_bytes: u64,
    pub(crate) budget_bytes: u64,
    pub(crate) kept_over_budget: bool,
}

struct Active {
    id: String,
    events: Weak<SessionEvents>,
    stop_capture: watch::Sender<bool>,
    thread: Option<RecorderThread>,
    started: Instant,
}

impl Active {
    fn alive(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|t| !t.handle.is_finished())
    }
}

#[derive(Default)]
struct State {
    /// By session log id.
    active: HashMap<String, Active>,
    /// Session logs whose recording is being set up.
    starting: HashSet<String>,
    /// Finished recorder threads, joined at shutdown.
    finished: Vec<RecorderThread>,
}

/// The daemon's recording service.
pub(crate) struct RecordingService {
    source: Source,
    factory: Arc<dyn EncoderFactory>,
    segment_ms: u64,
    state: Mutex<State>,
}

impl RecordingService {
    /// A service that refuses every recording action.
    pub(crate) fn disabled() -> Arc<Self> {
        Self::build(Source::Disabled, Arc::new(Rav1eFactory), SEGMENT_MS)
    }

    /// The production service: settings from `config.toml` and the
    /// environment, read at each action.
    pub(crate) fn from_config() -> Arc<Self> {
        Self::build(Source::Config, Arc::new(Rav1eFactory), SEGMENT_MS)
    }

    /// A service with fixed settings and encoder (tests).
    #[cfg(test)]
    pub(crate) fn fixed(
        settings: StorageSettings,
        factory: Arc<dyn EncoderFactory>,
        segment_ms: u64,
    ) -> Arc<Self> {
        Self::build(Source::Fixed(settings), factory, segment_ms)
    }

    fn build(source: Source, factory: Arc<dyn EncoderFactory>, segment_ms: u64) -> Arc<Self> {
        Arc::new(RecordingService {
            source,
            factory,
            segment_ms,
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The storage settings now.
    ///
    /// # Errors
    ///
    /// Recording is disabled, or the configuration is invalid.
    pub(crate) fn settings(&self) -> Result<StorageSettings, String> {
        match &self.source {
            Source::Disabled => Err("recording is not available in this daemon".into()),
            Source::Fixed(settings) => Ok(settings.clone()),
            Source::Config => {
                let cfg = rdpilot_config::resolve_recording()
                    .map_err(|e| format!("recording configuration: {e}"))?;
                Ok(StorageSettings {
                    root: cfg.dir_or_default(),
                    max_fps: cfg.max_fps,
                    budget_bytes: cfg.budget_bytes(),
                })
            }
        }
    }

    /// Recording ids that are being written.
    fn active_ids(&self) -> HashSet<String> {
        self.state().active.values().map(|a| a.id.clone()).collect()
    }

    /// Drop entries whose recorder stopped on its own (a write failure),
    /// detaching their sinks.
    fn reap(&self) {
        let dead: Vec<Active> = {
            let mut state = self.state();
            let keys: Vec<String> = state
                .active
                .iter()
                .filter(|(_, a)| !a.alive())
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .filter_map(|k| state.active.remove(&k))
                .collect()
        };
        for mut active in dead {
            let _ = active.stop_capture.send(true);
            if let Some(events) = active.events.upgrade() {
                events.detach_sink(
                    EventSource::Daemon,
                    EventKind::RecordingStopped {
                        reason: StopReason::WriteError,
                    },
                );
            }
            if let Some(thread) = active.thread.take() {
                self.state().finished.push(thread);
            }
        }
    }

    /// The active recording id of the session log `log_id`.
    pub(crate) fn recording_of(&self, log_id: &str) -> Option<String> {
        self.state()
            .active
            .get(log_id)
            .filter(|a| a.alive())
            .map(|a| a.id.clone())
    }

    /// At daemon start: finish recordings an earlier daemon left open, then
    /// prune. Blocking: run it off the IPC thread.
    pub(crate) fn startup(&self) {
        let Ok(settings) = self.settings() else {
            return;
        };
        let store = Store::new(settings.root);
        let active = self.active_ids();
        store.finalize_leftovers(&active);
        let report = store.prune(
            store::unix_ms(SystemTime::now()),
            settings.budget_bytes,
            &active,
        );
        warn_kept_over_budget(&report);
    }

    /// Start recording `target` now. Already recording: `changed: false`.
    ///
    /// # Errors
    ///
    /// A human-readable reason without secrets; the session is unaffected.
    pub(crate) async fn start(
        self: &Arc<Self>,
        target: Target,
        trigger: RecordingTrigger,
        source: EventSource,
    ) -> Result<Started, String> {
        self.reap();
        let log_id = target.events.header().log_id.clone();
        {
            let mut state = self.state();
            if let Some(active) = state.active.get(&log_id) {
                return Ok(Started {
                    id: active.id.clone(),
                    changed: false,
                });
            }
            if !state.starting.insert(log_id.clone()) {
                return Err("a recording is already starting for this session".into());
            }
        }
        let result = self.start_inner(target, trigger, source).await;
        let mut state = self.state();
        state.starting.remove(&log_id);
        match result {
            Ok(active) => {
                let id = active.id.clone();
                state.active.insert(log_id, active);
                Ok(Started { id, changed: true })
            }
            Err(e) => Err(e),
        }
    }

    async fn start_inner(
        self: &Arc<Self>,
        target: Target,
        trigger: RecordingTrigger,
        source: EventSource,
    ) -> Result<Active, String> {
        let settings = self.settings()?;
        let clock = target.events.clock();
        let start = Instant::now();
        let start_offset = clock.offset_ms(start);
        let started_wall = clock.wall(start);
        let header = target.events.header().clone();
        let manifest = Manifest {
            format: FORMAT,
            id: store::mint_id(started_wall),
            session: SessionRef {
                id: target.session.clone(),
                name: target.name.clone(),
            },
            host: target.host.clone(),
            started_at: iso8601_millis_from_system_time(started_wall),
            started_unix_ms: store::unix_ms(started_wall),
            ended_at: None,
            end_reason: None,
            duration_ms: None,
            rdpilot_version: env!("CARGO_PKG_VERSION").to_owned(),
            trigger: trigger.as_str().to_owned(),
            settings: Settings {
                max_fps: settings.max_fps,
                quantizer: QUANTIZER,
                speed: SPEED,
                tiles: TILES,
                encoder_threads: ENCODER_THREADS,
                budget_bytes: settings.budget_bytes,
            },
            codec: "av1".into(),
            container: "webm".into(),
            log: LogRef {
                schema: header.schema,
                log_id: header.log_id.clone(),
                incarnation: header.incarnation,
            },
            segments: Vec::new(),
            stats: None,
        };
        let timeline = Timeline {
            clock,
            start_offset,
        };
        let active_ids = self.active_ids();
        let factory = Arc::clone(&self.factory);
        let segment_ms = self.segment_ms;
        let storage = settings.clone();
        let recorder = tokio::task::spawn_blocking(move || {
            check_off_ipc_thread();
            let store = Store::new(storage.root.clone());
            store.ensure_root()?;
            store.finalize_leftovers(&active_ids);
            let report = store.prune(
                store::unix_ms(SystemTime::now()),
                storage.budget_bytes,
                &active_ids,
            );
            warn_kept_over_budget(&report);
            let dir = store.create_recording(&manifest.id)?;
            Store::write_manifest(&dir, &manifest)?;
            Recorder::new(RecorderSetup {
                dir,
                manifest,
                timeline,
                budget_bytes: storage.budget_bytes,
                segment_ms,
                factory,
            })
        })
        .await
        .map_err(|_| "recording setup was interrupted".to_owned())?
        .map_err(|e| format!("could not create the recording ({:?})", e.kind()))?;
        let id = recorder_id(&recorder);
        let waker = Arc::new(Waker::default());
        let queue = EventQueue::new(Arc::clone(&waker));
        let mailbox = Mailbox::new(Arc::clone(&waker));
        let thread = recorder::spawn(recorder, Arc::clone(&queue), Arc::clone(&mailbox), &waker)
            .map_err(|e| format!("could not start the recorder ({:?})", e.kind()))?;
        if let Err(sink) = target.events.attach_sink(
            Box::new(QueueSink(queue)),
            source,
            EventKind::RecordingStarted { trigger },
        ) {
            // Another sink is attached: dropping this one ends the recorder.
            drop(sink);
            return Err("the session is already being recorded".into());
        }
        let (stop_capture, stop_rx) = watch::channel(false);
        if let Some(frame) = target.frame {
            tokio::spawn(capture::run(
                frame,
                mailbox,
                timeline,
                settings.max_fps,
                stop_rx,
            ));
        }
        Ok(Active {
            id,
            events: Arc::downgrade(&target.events),
            stop_capture,
            thread: Some(thread),
            started: start,
        })
    }

    /// Stop the recording of `events`, writing `recording_stopped` with
    /// `source` and `reason` as its last event. Returns its id, or `None`
    /// when the session was not recording.
    pub(crate) fn stop(
        &self,
        events: &SessionEvents,
        source: EventSource,
        reason: StopReason,
    ) -> Option<String> {
        self.reap();
        let active = self.state().active.remove(&events.header().log_id)?;
        let _ = active.stop_capture.send(true);
        events.detach_sink(source, EventKind::RecordingStopped { reason });
        let id = active.id.clone();
        if let Some(thread) = active.thread {
            self.state().finished.push(thread);
        }
        Some(id)
    }

    /// The session is closing: write `session_closed` and stop.
    pub(crate) fn session_closed(&self, events: &SessionEvents, reason: CloseReason) {
        if self.state().active.contains_key(&events.header().log_id) {
            events.record_detached(EventSource::Daemon, EventKind::SessionClosed { reason });
            self.stop(events, EventSource::Daemon, StopReason::SessionClosed);
        }
    }

    /// Add an annotation to the active recording of `events`.
    ///
    /// # Errors
    ///
    /// Empty or over 4 KiB, not recording, or the recording is busy; nothing
    /// is written then.
    pub(crate) fn annotate(
        &self,
        events: &SessionEvents,
        source: EventSource,
        text: &str,
    ) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("annotation is empty".into());
        }
        if text.len() > MAX_ANNOTATION_BYTES {
            return Err(format!(
                "annotation is over {MAX_ANNOTATION_BYTES} bytes ({} bytes)",
                text.len()
            ));
        }
        self.reap();
        let Some(id) = self.recording_of(&events.header().log_id) else {
            return Err("session is not recording; start a recording first".into());
        };
        if events.record_detached(
            source,
            EventKind::Annotation {
                text: text.to_owned(),
            },
        ) {
            Ok(id)
        } else {
            Err("recording busy, annotation not written".into())
        }
    }

    /// The recordings on disk with totals. Blocking: run it off the IPC
    /// thread.
    ///
    /// # Errors
    ///
    /// Recording is disabled or its configuration is invalid.
    pub(crate) fn list(&self) -> Result<Listing, String> {
        check_off_ipc_thread();
        let settings = self.settings()?;
        let store = Store::new(settings.root);
        let (active, started): (HashSet<String>, HashMap<String, Instant>) = {
            let state = self.state();
            (
                state
                    .active
                    .values()
                    .filter(|a| a.alive())
                    .map(|a| a.id.clone())
                    .collect(),
                state
                    .active
                    .values()
                    .map(|a| (a.id.clone(), a.started))
                    .collect(),
            )
        };
        let mut listing = Listing {
            budget_bytes: settings.budget_bytes,
            ..Listing::default()
        };
        for scanned in store.scan() {
            let m = scanned.manifest;
            let is_active = active.contains(&m.id);
            if scanned.kept {
                listing.kept_bytes += scanned.bytes;
            } else {
                listing.unkept_bytes += scanned.bytes;
            }
            let duration_ms = if is_active {
                started.get(&m.id).map_or(0, |s| {
                    u64::try_from(s.elapsed().as_millis()).unwrap_or(u64::MAX)
                })
            } else {
                m.duration_ms.unwrap_or(0)
            };
            listing.recordings.push(WireRecording {
                id: m.id,
                session: m.session.id,
                session_name: m.session.name,
                host: m.host,
                started_at: m.started_at,
                duration_ms,
                bytes: scanned.bytes,
                active: is_active,
                kept: scanned.kept,
            });
        }
        listing.kept_over_budget = listing.kept_bytes > settings.budget_bytes;
        Ok(listing)
    }

    /// Mark or unmark recording `id`. Blocking: run it off the IPC thread.
    ///
    /// # Errors
    ///
    /// Unknown id, or recording is disabled.
    pub(crate) fn keep(&self, id: &str, keep: bool) -> Result<bool, String> {
        check_off_ipc_thread();
        let settings = self.settings()?;
        Store::new(settings.root)
            .set_keep(id, keep)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("unknown recording id \"{id}\""),
                kind => format!("could not change the keep mark ({kind:?})"),
            })
    }

    /// Daemon shutdown: stop every recording (`daemon_stopped`) and wait up
    /// to 10 s for the recorders to finish their last segments.
    pub(crate) async fn shutdown(&self) {
        let actives: Vec<Active> = self.state().active.drain().map(|(_, a)| a).collect();
        let mut threads = Vec::new();
        for mut active in actives {
            let _ = active.stop_capture.send(true);
            if let Some(events) = active.events.upgrade() {
                events.detach_sink(
                    EventSource::Daemon,
                    EventKind::RecordingStopped {
                        reason: StopReason::DaemonStopped,
                    },
                );
            }
            threads.extend(active.thread.take());
        }
        threads.append(&mut self.state().finished);
        let join = tokio::task::spawn_blocking(move || {
            for thread in threads {
                let _ = thread.handle.join();
            }
        });
        let _ = tokio::time::timeout(SHUTDOWN_JOIN, join).await;
    }
}

fn recorder_id(recorder: &Recorder) -> String {
    recorder.id().to_owned()
}

fn warn_kept_over_budget(report: &store::PruneReport) {
    if report.kept_over_budget {
        eprintln!(
            "rdpilot-daemon: kept recordings use {} bytes, more than the recording budget; nothing is deleted",
            report.kept_bytes
        );
    }
}

#[cfg(test)]
thread_local! {
    static IPC_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the current thread as the IPC `LocalSet` thread (tests).
#[cfg(test)]
pub(crate) fn mark_ipc_thread() {
    IPC_THREAD.with(|t| t.set(true));
}

/// Recording file I/O must not run on the IPC thread (checked in tests).
fn check_off_ipc_thread() {
    #[cfg(test)]
    IPC_THREAD.with(|t| assert!(!t.get(), "recording file I/O on the IPC thread"));
}
