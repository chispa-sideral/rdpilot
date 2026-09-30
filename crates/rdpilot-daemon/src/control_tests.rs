//! State machine tests for the control lease, with a fake input sink that
//! records the exact event sequence.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rdpilot_ipc::WireControllerKind;

use super::*;
use crate::events::{EventKind, SessionEvents};
use crate::seams::{HumanInput, SendFuture, ViewFrameSource};

/// Records every event it is sent, in order.
#[derive(Default)]
pub(crate) struct Sink {
    pub(crate) sent: Mutex<Vec<HumanEvent>>,
}

impl HumanInput for Sink {
    fn send(&self, events: Vec<HumanEvent>) -> SendFuture<'_, Result<(), DaemonError>> {
        self.sent.lock().unwrap().extend(events);
        Box::pin(async { Ok(()) })
    }
}

impl Sink {
    fn take(&self) -> Vec<HumanEvent> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

/// A frame source with a settable geometry.
struct Frames(Mutex<Option<(u32, u32)>>);

impl ViewFrameSource for Frames {
    fn status(&self) -> rdpilot::FrameStatus {
        rdpilot::FrameStatus::default()
    }
    fn changed(&self, _after: u64) -> SendFuture<'_, rdpilot::FrameStatus> {
        Box::pin(std::future::pending())
    }
    fn capture(&self) -> Option<(u64, rdpilot::Screenshot)> {
        None
    }
    fn geometry(&self) -> Option<(u32, u32)> {
        *self.0.lock().unwrap()
    }
}

struct Fixture {
    control: SessionControl,
    sink: Arc<Sink>,
    frames: Arc<Frames>,
    events: Arc<SessionEvents>,
}

fn fixture() -> Fixture {
    let events = Arc::new(SessionEvents::new(&"web".parse().unwrap(), 7, None));
    let sink = Arc::new(Sink::default());
    let frames = Arc::new(Frames(Mutex::new(Some((800, 600)))));
    let control = SessionControl::new(
        "web",
        7,
        Arc::clone(&events),
        Some(Arc::clone(&frames) as Arc<dyn ViewFrameSource>),
        Some(Arc::clone(&sink) as Arc<dyn HumanInput>),
    );
    Fixture {
        control,
        sink,
        frames,
        events,
    }
}

fn tab(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(100, 64, 0, last))
}

async fn take(f: &Fixture, last: u8) -> Grant {
    f.control.take(tab(last), async {}).await.expect("take")
}

fn kinds(events: &SessionEvents) -> Vec<EventKind> {
    events.after(0).events.into_iter().map(|e| e.kind).collect()
}

fn key(code: u8, down: bool) -> HumanEvent {
    HumanEvent::Key {
        code,
        extended: false,
        down,
    }
}

const SHIFT: u8 = 0x2A;

fn human(last: u8) -> ControllerRef {
    ControllerRef::Human {
        address: tab(last).to_string(),
    }
}

#[tokio::test]
async fn the_agent_controls_by_default_and_a_take_moves_control_to_the_tab() {
    let f = fixture();
    assert_eq!(f.control.controller().kind, WireControllerKind::Agent);
    assert!(f.control.check_agent().is_ok());
    let grant = take(&f, 9).await;
    assert_eq!(grant.lease.len(), 32);
    assert_eq!(grant.generation, 7);
    let controller = f.control.controller();
    assert_eq!(controller.kind, WireControllerKind::Human);
    assert_eq!(controller.address.as_deref(), Some("100.64.0.9"));
    let err = f.control.check_agent().unwrap_err().to_string();
    assert!(err.starts_with("session \"web\" is controlled by human viewer 100.64.0.9 since "));
    assert!(err.ends_with("; wait and retry, or take over with: rdpilot takeover --session web"));
    assert_eq!(
        kinds(&f.events),
        vec![EventKind::ControlTaken { by: human(9) }]
    );
}

#[tokio::test]
async fn a_second_tab_moves_the_lease_after_the_first_tabs_releases() {
    let f = fixture();
    let first = take(&f, 1).await;
    f.control
        .input(&first.lease, 7, (800, 600), vec![key(SHIFT, true)])
        .await
        .unwrap();
    f.sink.take();
    let second = take(&f, 2).await;
    assert_eq!(
        f.sink.take(),
        vec![key(SHIFT, false)],
        "release before the move"
    );
    assert_eq!(
        f.control.heartbeat(&first.lease),
        Err(NotHeld::Lost(Loss::by(human(2))))
    );
    assert!(f.control.heartbeat(&second.lease).is_ok());
    assert_eq!(
        kinds(&f.events).last(),
        Some(&EventKind::ControlTakenOver {
            from: human(1),
            by: human(2)
        })
    );
    // The first tab's queued input is refused and never applied.
    let report = f
        .control
        .input(&first.lease, 7, (800, 600), vec![key(0x1E, true)])
        .await
        .unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.dropped.stale, 1);
    assert!(f.sink.take().is_empty());
}

#[tokio::test]
async fn agent_takeover_releases_held_keys_and_is_a_no_op_under_agent_control() {
    let f = fixture();
    let (previous, transition) = f.control.agent_takeover(EventSource::Cli);
    assert_eq!(previous.kind, WireControllerKind::Agent);
    assert!(transition.is_none());
    let grant = take(&f, 3).await;
    f.control
        .input(
            &grant.lease,
            7,
            (800, 600),
            vec![
                HumanEvent::Move { x: 5, y: 5 },
                HumanEvent::Button {
                    button: PointerButton::Left,
                    down: true,
                },
                key(SHIFT, true),
            ],
        )
        .await
        .unwrap();
    f.sink.take();
    let (previous, transition) = f.control.agent_takeover(EventSource::Cli);
    assert_eq!(previous.address.as_deref(), Some("100.64.0.3"));
    let transition = transition.expect("a human held the lease");
    assert_eq!(transition.releases(), 2);
    f.control.discharge(transition).await;
    let sent = f.sink.take();
    assert!(sent.contains(&key(SHIFT, false)));
    assert!(sent.contains(&HumanEvent::Button {
        button: PointerButton::Left,
        down: false
    }));
    assert!(f.control.check_agent().is_ok());
    assert_eq!(
        f.control.heartbeat(&grant.lease),
        Err(NotHeld::Lost(Loss::by(ControllerRef::Agent)))
    );
    assert_eq!(
        kinds(&f.events).last(),
        Some(&EventKind::ControlTakenOver {
            from: human(3),
            by: ControllerRef::Agent
        })
    );
}

#[tokio::test]
async fn every_end_path_returns_control_to_the_agent_with_releases_and_a_reason() {
    for reason in [
        EndReason::ViewerStopped,
        EndReason::SessionEnded,
        EndReason::DaemonStopped,
    ] {
        let f = fixture();
        let grant = take(&f, 4).await;
        f.control
            .input(&grant.lease, 7, (800, 600), vec![key(SHIFT, true)])
            .await
            .unwrap();
        f.sink.take();
        let transition = f.control.end_human(reason).expect("lease ended");
        f.control.discharge(transition).await;
        assert_eq!(f.sink.take(), vec![key(SHIFT, false)]);
        assert!(f.control.check_agent().is_ok());
        assert_eq!(
            f.control.heartbeat(&grant.lease),
            Err(NotHeld::Lost(Loss::reason(reason)))
        );
        assert_eq!(
            kinds(&f.events).last(),
            Some(&EventKind::ControlEnded {
                from: human(4),
                reason
            })
        );
        assert!(f.control.end_human(reason).is_none(), "nothing left to end");
    }
}

#[tokio::test]
async fn release_ends_the_lease_and_blur_releases_without_ending_it() {
    let f = fixture();
    let grant = take(&f, 5).await;
    f.control
        .input(&grant.lease, 7, (800, 600), vec![key(SHIFT, true)])
        .await
        .unwrap();
    f.sink.take();
    let transition = f.control.blur(&grant.lease).unwrap();
    f.control.discharge(transition).await;
    assert_eq!(f.sink.take(), vec![key(SHIFT, false)]);
    assert!(
        f.control.heartbeat(&grant.lease).is_ok(),
        "blur keeps the lease"
    );
    let transition = f.control.release(&grant.lease).unwrap();
    f.control.discharge(transition).await;
    assert!(f.sink.take().is_empty(), "nothing held after blur");
    assert!(f.control.check_agent().is_ok());
    assert_eq!(
        f.control.heartbeat(&grant.lease),
        Err(NotHeld::Lost(Loss::released()))
    );
    assert_eq!(
        kinds(&f.events).last(),
        Some(&EventKind::ControlReleased { from: human(5) })
    );
}

#[tokio::test]
async fn heartbeat_loss_and_idle_timeout_expire_only_granted_leases() {
    let f = fixture();
    let grant = take(&f, 6).await;
    let idle = Duration::from_secs(300);
    assert!(f.control.expire(Instant::now(), idle).is_none());
    // Heartbeats keep the lease, but do not count as input.
    let later = Instant::now() + Duration::from_secs(6);
    let transition = f.control.expire(later, idle).expect("heartbeat lost");
    f.control.discharge(transition).await;
    assert_eq!(
        f.control.heartbeat(&grant.lease),
        Err(NotHeld::Lost(Loss::reason(EndReason::HeartbeatLost)))
    );

    let grant = take(&f, 6).await;
    f.control.heartbeat(&grant.lease).unwrap();
    let transition = f
        .control
        .expire(Instant::now(), Duration::ZERO)
        .expect("idle timeout");
    f.control.discharge(transition).await;
    assert_eq!(
        f.control.heartbeat(&grant.lease),
        Err(NotHeld::Lost(Loss::reason(EndReason::IdleTimeout)))
    );
    assert_eq!(
        kinds(&f.events).last(),
        Some(&EventKind::ControlEnded {
            from: human(6),
            reason: EndReason::IdleTimeout
        })
    );
}

/// A Cua call admitted before a take is counted before the take reads
/// the count; a take marked before admission refuses the call.
#[tokio::test(start_paused = true)]
async fn cua_admission_and_take_interleave_deterministically() {
    let f = Arc::new(fixture());
    // 1. Admitted first: the take waits for it.
    assert!(f.control.admit_cua(1, false).unwrap().is_none());
    assert_eq!(f.control.inflight(), 1);
    let taking = {
        let f = Arc::clone(&f);
        tokio::spawn(async move { f.control.take(tab(7), async {}).await })
    };
    tokio::task::yield_now().await;
    assert!(f.control.human_active(), "the take is marked at once");
    // 2. Marked first: a new acting call is refused, not counted.
    let refusal = f.control.admit_cua(1, false).unwrap_err();
    assert!(refusal
        .mcp_message
        .starts_with("session \"web\" is controlled by human viewer 100.64.0.7 since "));
    assert!(refusal.mcp_message.ends_with(
        "; wait and retry, or repeat this call with \"takeover\": true to take control"
    ));
    assert_eq!(f.control.inflight(), 1);
    // The pending take is never expired by the sweeper.
    assert!(f
        .control
        .expire(Instant::now() + Duration::from_secs(60), Duration::ZERO)
        .is_none());
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!taking.is_finished(), "waits for the call in flight");
    f.control.cua_finished(1);
    let grant = taking.await.unwrap().expect("granted after the answer");
    assert!(f.control.heartbeat(&grant.lease).is_ok());
    // The clocks start at the grant, not at the start of the wait.
    assert!(f
        .control
        .expire(Instant::now(), Duration::from_secs(300))
        .is_none());
}

/// `takeover: true` switches to the agent and counts the call in one
/// step; a pending take is displaced and then waits for nothing.
#[tokio::test(start_paused = true)]
async fn cua_takeover_during_a_pending_take_displaces_it() {
    let f = Arc::new(fixture());
    assert!(f.control.admit_cua(1, false).unwrap().is_none());
    let taking = {
        let f = Arc::clone(&f);
        tokio::spawn(async move { f.control.take(tab(8), async {}).await })
    };
    tokio::task::yield_now().await;
    let transition = f.control.admit_cua(1, true).unwrap();
    assert!(transition.is_some());
    assert_eq!(f.control.inflight(), 2);
    f.control.cua_finished(2);
    assert_eq!(
        taking.await.unwrap().unwrap_err(),
        TakeError::Lost(Loss::by(ControllerRef::Agent))
    );
    assert!(f.control.check_agent().is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_take_gives_up_after_its_bound_and_leaves_the_agent_in_control() {
    let f = fixture();
    assert!(f.control.admit_cua(1, false).unwrap().is_none());
    let result = f
        .control
        .take_within(tab(9), async {}, Duration::from_secs(10))
        .await;
    assert_eq!(result.unwrap_err(), TakeError::Busy);
    assert!(f.control.check_agent().is_ok());
    assert!(!f.control.human_active());
    assert!(
        kinds(&f.events).is_empty(),
        "no event for a take never granted"
    );
}

#[tokio::test(start_paused = true)]
async fn a_take_waits_for_the_per_session_lock() {
    let f = Arc::new(fixture());
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    let held = Arc::clone(&lock).lock_owned().await;
    let taking = {
        let f = Arc::clone(&f);
        let lock = Arc::clone(&lock);
        tokio::spawn(async move {
            f.control
                .take(tab(9), async move {
                    let _ = lock.lock().await;
                })
                .await
        })
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!taking.is_finished());
    drop(held);
    assert!(taking.await.unwrap().is_ok());
}

#[tokio::test(start_paused = true)]
async fn an_abandoned_take_is_undone() {
    let f = Arc::new(fixture());
    assert!(f.control.admit_cua(1, false).unwrap().is_none());
    let taking = {
        let f = Arc::clone(&f);
        tokio::spawn(async move { f.control.take(tab(9), async {}).await })
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(f.control.check_agent().is_err());
    taking.abort();
    let _ = taking.await;
    assert!(f.control.check_agent().is_ok(), "the agent controls again");
}

/// Requests naming a lease that is not current never change control.
#[tokio::test]
async fn non_holder_requests_are_refused_and_change_nothing() {
    let f = fixture();
    let grant = take(&f, 1).await;
    for bogus in ["", "00000000000000000000000000000000", "not-a-lease"] {
        assert_eq!(f.control.heartbeat(bogus), Err(NotHeld::Unknown));
        assert_eq!(f.control.blur(bogus).unwrap_err(), NotHeld::Unknown);
        assert_eq!(f.control.release(bogus).unwrap_err(), NotHeld::Unknown);
        let report = f
            .control
            .input(bogus, 7, (800, 600), vec![key(SHIFT, true)])
            .await
            .unwrap();
        assert_eq!(report.lost, Some(NotHeld::Unknown));
        assert_eq!(report.applied, 0);
    }
    assert!(f.control.heartbeat(&grant.lease).is_ok());
    assert!(f.sink.take().is_empty());
    assert_eq!(kinds(&f.events).len(), 1, "only the take");
}

#[tokio::test]
async fn fencing_drops_old_geometry_and_old_generations_and_coalesces_moves() {
    let f = fixture();
    let grant = take(&f, 2).await;
    let report = f
        .control
        .input(
            &grant.lease,
            7,
            (800, 600),
            vec![
                HumanEvent::Move { x: 1, y: 1 },
                HumanEvent::Move { x: 2, y: 2 },
                HumanEvent::Move { x: 3, y: 3 },
                key(0x1E, true),
                key(0x1E, false),
                HumanEvent::Move { x: 900, y: 3 },
            ],
        )
        .await
        .unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(report.dropped.geometry, 1, "out of the frame");
    assert_eq!(
        f.sink.take(),
        vec![
            HumanEvent::Move { x: 3, y: 3 },
            key(0x1E, true),
            key(0x1E, false)
        ]
    );
    // The frame size changed: pointer events aimed at the old size drop,
    // keys still apply.
    *f.frames.0.lock().unwrap() = Some((1024, 768));
    let report = f
        .control
        .input(
            &grant.lease,
            7,
            (800, 600),
            vec![
                HumanEvent::Move { x: 4, y: 4 },
                HumanEvent::Wheel {
                    vertical: true,
                    units: 120,
                },
                key(0x1E, true),
            ],
        )
        .await
        .unwrap();
    assert_eq!(report.dropped.geometry, 2);
    assert_eq!(report.applied, 1);
    // Another generation: everything drops.
    let report = f
        .control
        .input(&grant.lease, 6, (1024, 768), vec![key(0x1E, false)])
        .await
        .unwrap();
    assert_eq!(report.dropped.generation, 1);
    assert_eq!(report.applied, 0);
    assert_eq!(f.sink.take(), vec![key(0x1E, true)]);
}

#[tokio::test]
async fn at_most_one_controller_and_no_lease_id_in_events_or_status() {
    let f = fixture();
    let a = take(&f, 1).await;
    let b = take(&f, 2).await;
    assert!(f.control.check_lease(&a.lease).is_err());
    assert!(f.control.check_lease(&b.lease).is_ok());
    let log = serde_json::to_string(&f.events.after(0)).unwrap();
    let status = serde_json::to_string(&f.control.controller()).unwrap();
    for lease in [&a.lease, &b.lease] {
        assert!(!log.contains(lease.as_str()));
        assert!(!status.contains(lease.as_str()));
    }
}

#[test]
fn messages_quote_unusual_session_names_and_show_utc_time() {
    assert_eq!(clock_time("2026-01-01T14:02:07Z"), "14:02:07 UTC");
    assert_eq!(shell_arg("notepad"), "notepad");
    assert_eq!(shell_arg("my session"), "'my session'");
    assert_eq!(shell_arg("it's"), "'it'\\''s'");
    assert_eq!(
        cli_refusal("notepad", "100.101.102.103", "2026-01-01T14:02:07Z"),
        "session \"notepad\" is controlled by human viewer 100.101.102.103 since 14:02:07 UTC; \
wait and retry, or take over with: rdpilot takeover --session notepad"
    );
}

/// A registry session whose human input goes to a shared [`Sink`].
struct InputSession(Arc<Sink>);

impl crate::seams::ManagedSession for InputSession {
    fn close(self: Box<Self>) -> crate::seams::BoxFuture<'static, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn describe(&self) -> rdpilot_ipc::SessionLifecycle {
        rdpilot_ipc::SessionLifecycle::Live
    }
    fn screenshot(&self) -> crate::seams::BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async { Ok(rdpilot::Screenshot::from_rgba(1, 1, vec![0; 4])?) })
    }
    fn send_mouse(
        &self,
        _: rdpilot::MouseAction,
    ) -> crate::seams::BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn send_key(
        &self,
        _: rdpilot::KeyAction,
    ) -> crate::seams::BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn upload_file(
        &self,
        _: std::path::PathBuf,
        _: String,
    ) -> crate::seams::BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Err(DaemonError::Io("unused".into())) })
    }
    fn download_file(
        &self,
        _: String,
        _: std::path::PathBuf,
    ) -> crate::seams::BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>> {
        Box::pin(async { Err(DaemonError::Io("unused".into())) })
    }
    fn ping(&self) -> crate::seams::BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn desktop_size(&self) -> (u32, u32) {
        (1, 1)
    }
    fn deploy_and_launch(&self) -> crate::seams::BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn human_input(&self) -> Option<Arc<dyn HumanInput>> {
        Some(Arc::clone(&self.0) as Arc<dyn HumanInput>)
    }
}

struct InputConnector(Arc<Sink>);

impl crate::seams::SessionConnector for InputConnector {
    fn connect(
        &self,
        _: rdpilot::ConnectionConfig,
    ) -> crate::seams::BoxFuture<'static, Result<Box<dyn crate::seams::ManagedSession>, DaemonError>>
    {
        let sink = Arc::clone(&self.0);
        Box::pin(async move {
            Ok(Box::new(InputSession(sink)) as Box<dyn crate::seams::ManagedSession>)
        })
    }
}

/// Closing a session ends its human lease with releases before the
/// session closes, and records `session_ended`.
#[tokio::test]
async fn closing_a_session_ends_its_lease_with_releases() {
    let sink = Arc::new(Sink::default());
    let registry = crate::registry::Registry::new(
        Arc::new(InputConnector(Arc::clone(&sink))),
        Arc::new(crate::seams::NoopReconciliationSink),
    );
    let id = registry
        .open(
            Some("web".into()),
            "h".into(),
            rdpilot::ConnectionConfig::new("h", "u", "p"),
        )
        .await
        .unwrap();
    let control = registry.control(&id).unwrap();
    assert!(control.has_input());
    let grant = control.take(tab(1), async {}).await.unwrap();
    // No frame source: pointer events drop, keys apply.
    control
        .input(
            &grant.lease,
            grant.generation,
            (1, 1),
            vec![key(SHIFT, true)],
        )
        .await
        .unwrap();
    sink.take();
    let events = registry.events(&id).unwrap();
    registry.close(&id).await.unwrap();
    assert_eq!(sink.take(), vec![key(SHIFT, false)]);
    assert_eq!(
        kinds(&events).last(),
        Some(&EventKind::ControlEnded {
            from: human(1),
            reason: EndReason::SessionEnded
        })
    );
    assert_eq!(
        control.heartbeat(&grant.lease),
        Err(NotHeld::Lost(Loss::reason(EndReason::SessionEnded)))
    );
}
