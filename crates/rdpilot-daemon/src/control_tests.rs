//! State machine tests for the control lease, with a fake input sink that
//! records the exact event sequence.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rdpilot_ipc::WireControllerKind;

use super::*;
use crate::events::{EventKind, SessionEvents};
use crate::seams::{BoxFuture, HumanInput, ViewFrameSource};

/// Records every event the session loop would send, in order (the releases
/// of an epoch change included), and every epoch change it is sent.
#[derive(Default)]
pub(crate) struct Sink {
    pub(crate) sent: Mutex<Vec<RawInput>>,
    model: Mutex<crate::seams::LoopInputModel>,
    epochs: Mutex<Vec<InputEpoch>>,
    /// The epochs of the agent's native actions.
    agent: Mutex<Vec<InputEpoch>>,
}

impl HumanInput for Sink {
    fn send(
        &self,
        events: Vec<RawInput>,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        let sent = self.model.lock().unwrap().send(epoch, events);
        Box::pin(async move {
            self.sent.lock().unwrap().extend(sent?);
            Ok(())
        })
    }

    fn set_epoch(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.epochs.lock().unwrap().push(epoch);
        let releases = self.model.lock().unwrap().set_epoch(epoch);
        self.sent.lock().unwrap().extend(releases);
        Box::pin(async { Ok(()) })
    }
}

impl Sink {
    fn take(&self) -> Vec<RawInput> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    /// The epoch changes sent so far, emptied.
    fn epochs(&self) -> Vec<InputEpoch> {
        std::mem::take(&mut *self.epochs.lock().unwrap())
    }
}

/// A frame source with a settable geometry.
struct Frames(Mutex<Option<(u32, u32)>>);

impl ViewFrameSource for Frames {
    fn status(&self) -> rdpilot::FrameStatus {
        rdpilot::FrameStatus::default()
    }
    fn changed(&self, _after: u64) -> BoxFuture<'_, rdpilot::FrameStatus> {
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
        EPOCH_CONFIRM_WAIT,
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

fn key(code: u8, down: bool) -> RawInput {
    RawInput::Key {
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
                RawInput::PointerMove { x: 5, y: 5 },
                RawInput::Button {
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
    assert_eq!(transition.epoch(), Some(InputEpoch::INITIAL.next().next()));
    assert!(f.control.discharge(transition).await);
    let sent = f.sink.take();
    assert!(sent.contains(&key(SHIFT, false)));
    assert!(sent.contains(&RawInput::Button {
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
                RawInput::PointerMove { x: 1, y: 1 },
                RawInput::PointerMove { x: 2, y: 2 },
                RawInput::PointerMove { x: 3, y: 3 },
                key(0x1E, true),
                key(0x1E, false),
                RawInput::PointerMove { x: 900, y: 3 },
            ],
        )
        .await
        .unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(report.dropped.geometry, 1, "out of the frame");
    assert_eq!(
        f.sink.take(),
        vec![
            RawInput::PointerMove { x: 3, y: 3 },
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
                RawInput::PointerMove { x: 4, y: 4 },
                RawInput::Wheel {
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

fn left(down: bool) -> RawInput {
    RawInput::Button {
        button: PointerButton::Left,
        down,
    }
}

/// Geometry fencing never leaves a button down: the release of a held
/// button passes after a resize, and a fenced pointer event releases every
/// held button.
#[tokio::test]
async fn geometry_fencing_never_leaves_a_button_down() {
    let f = fixture();
    let grant = take(&f, 3).await;
    let press = vec![RawInput::PointerMove { x: 5, y: 5 }, left(true)];
    let report = f
        .control
        .input(&grant.lease, 7, (800, 600), press.clone())
        .await
        .unwrap();
    assert_eq!(report.applied, 2);
    f.sink.take();

    // 1. Resize, then the page's button-up aimed at the old size: applied.
    *f.frames.0.lock().unwrap() = Some((1024, 768));
    let report = f
        .control
        .input(&grant.lease, 7, (800, 600), vec![left(false)])
        .await
        .unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.dropped.geometry, 0);
    assert_eq!(f.sink.take(), vec![left(false)]);
    // A button-up for a button not held is fenced as before.
    let report = f
        .control
        .input(&grant.lease, 7, (800, 600), vec![left(false)])
        .await
        .unwrap();
    assert_eq!(report.dropped.geometry, 1);
    assert!(f.sink.take().is_empty());

    // 2. Press at the new size, resize again, then a move aimed at the
    // old size: dropped, and the held button is released.
    let report = f
        .control
        .input(&grant.lease, 7, (1024, 768), press)
        .await
        .unwrap();
    assert_eq!(report.applied, 2);
    f.sink.take();
    *f.frames.0.lock().unwrap() = Some((800, 600));
    let report = f
        .control
        .input(
            &grant.lease,
            7,
            (1024, 768),
            vec![RawInput::PointerMove { x: 6, y: 6 }, left(false)],
        )
        .await
        .unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(
        report.dropped.geometry, 2,
        "the move and the late button-up"
    );
    assert_eq!(f.sink.take(), vec![left(false)], "released once");
    // Nothing is left to release.
    let transition = f.control.release(&grant.lease).unwrap();
    assert_eq!(transition.releases(), 0);
    f.control.discharge(transition).await;
    assert!(f.sink.take().is_empty());
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

/// Human input that also hears when the agent's native action of `epoch`
/// reaches an [`InputSession`], and checks its epoch as the loop does.
trait AgentLog: HumanInput {
    fn agent_acted(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>>;
}

impl AgentLog for Sink {
    fn agent_acted(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.agent.lock().unwrap().push(epoch);
        self.send(Vec::new(), epoch)
    }
}

/// A registry session whose human input goes to a shared sink.
struct InputSession<S>(Arc<S>);

impl<S: AgentLog> crate::seams::ManagedSession for InputSession<S> {
    fn close(self: Box<Self>) -> crate::seams::BoxFuture<'static, Result<(), DaemonError>> {
        Box::pin(async { Ok(()) })
    }
    fn screenshot(&self) -> crate::seams::BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async { Ok(rdpilot::Screenshot::from_rgba(1, 1, vec![0; 4])?) })
    }
    fn send_mouse(
        &self,
        _: rdpilot::MouseAction,
        epoch: rdpilot::InputEpoch,
    ) -> crate::seams::BoxFuture<'_, Result<(), DaemonError>> {
        self.0.agent_acted(epoch)
    }
    fn send_key(
        &self,
        _: rdpilot::KeyAction,
        epoch: rdpilot::InputEpoch,
    ) -> crate::seams::BoxFuture<'_, Result<(), DaemonError>> {
        self.0.agent_acted(epoch)
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
    fn deploy_and_launch(
        &self,
        _epoch: rdpilot::InputEpoch,
    ) -> crate::seams::BoxFuture<'_, Result<Duration, DaemonError>> {
        Box::pin(async { Ok(Duration::ZERO) })
    }
    fn human_input(&self) -> Option<Arc<dyn HumanInput>> {
        Some(Arc::clone(&self.0) as Arc<dyn HumanInput>)
    }
}

struct InputConnector<S>(Arc<S>);

impl<S: AgentLog> crate::seams::SessionConnector for InputConnector<S> {
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

/// One entry of the exact sequence a gated session sees.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Rec {
    /// A human event (or a release) reached the session's input channel.
    Human(RawInput),
    /// The agent's native action reached the session.
    Agent,
}

/// A session input that, while shut, holds every send, epoch change and
/// agent action at a FIFO gate (the order of the session loop's queue), and
/// records human events, releases and agent actions in one sequence, as the
/// loop would send them. It starts open.
struct GatedSink {
    log: Mutex<Vec<Rec>>,
    gate: tokio::sync::Semaphore,
    model: Mutex<crate::seams::LoopInputModel>,
    /// The epochs of the agent's actions.
    agent_epochs: Mutex<Vec<InputEpoch>>,
}

impl GatedSink {
    fn new() -> Self {
        GatedSink {
            log: Mutex::new(Vec::new()),
            gate: tokio::sync::Semaphore::new(1),
            model: Mutex::new(crate::seams::LoopInputModel::default()),
            agent_epochs: Mutex::new(Vec::new()),
        }
    }

    fn shut(&self) {
        self.gate.try_acquire().unwrap().forget();
    }

    fn open(&self) {
        self.gate.add_permits(1);
    }

    fn log(&self) -> Vec<Rec> {
        self.log.lock().unwrap().clone()
    }

    /// Pass the gate, then apply `events` of `epoch` and log what is sent.
    async fn pass(
        &self,
        epoch: InputEpoch,
        events: Vec<RawInput>,
        agent: bool,
    ) -> Result<(), DaemonError> {
        drop(self.gate.acquire().await.unwrap());
        let sent = self.model.lock().unwrap().send(epoch, events)?;
        let mut log = self.log.lock().unwrap();
        log.extend(sent.into_iter().map(Rec::Human));
        if agent {
            log.push(Rec::Agent);
            self.agent_epochs.lock().unwrap().push(epoch);
        }
        Ok(())
    }
}

impl HumanInput for GatedSink {
    fn send(
        &self,
        events: Vec<RawInput>,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(self.pass(epoch, events, false))
    }

    fn set_epoch(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async move {
            drop(self.gate.acquire().await.unwrap());
            let releases = self.model.lock().unwrap().set_epoch(epoch);
            self.log
                .lock()
                .unwrap()
                .extend(releases.into_iter().map(Rec::Human));
            Ok(())
        })
    }
}

impl AgentLog for GatedSink {
    fn agent_acted(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(self.pass(epoch, Vec::new(), true))
    }
}

/// A registry with one session whose human input goes to a new
/// [`GatedSink`], and a viewer over it.
struct Gated {
    sink: Arc<GatedSink>,
    registry: Arc<crate::registry::Registry>,
    viewer: crate::registry::ViewerControl,
    id: rdpilot_ipc::SessionId,
}

async fn gated() -> Gated {
    let sink = Arc::new(GatedSink::new());
    let registry = Arc::new(crate::registry::Registry::new(
        Arc::new(InputConnector(Arc::clone(&sink))),
        Arc::new(crate::seams::NoopReconciliationSink),
    ));
    let id = registry
        .open(
            Some("web".into()),
            "h".into(),
            rdpilot::ConnectionConfig::new("h", "u", "p"),
        )
        .await
        .unwrap();
    let viewer = crate::registry::ViewerControl::new(Arc::clone(&registry));
    Gated {
        sink,
        registry,
        viewer,
        id,
    }
}

/// The lease end or change under test.
#[derive(Debug, Clone, Copy)]
enum EndPath {
    Release,
    HeartbeatLost,
    IdleTimeout,
    ViewerStopped,
    DaemonStopped,
    CliTakeover,
}

/// Run `path` while the holder's key press is still waiting for the
/// session's input channel, then a native agent key action. The gate
/// opens last. The agent's action must reach the session only after the
/// press and its release.
async fn native_action_after(path: EndPath) -> Vec<Rec> {
    let Gated {
        sink,
        registry,
        viewer,
        id,
    } = gated().await;
    let (grant, _) = viewer.take(&id, tab(1)).await.unwrap();
    sink.shut();
    let control = registry.control(&id).unwrap();

    // 1. The holder presses Shift; the send waits at the gate.
    let human = control.input(
        &grant.lease,
        grant.generation,
        (1, 1),
        vec![key(SHIFT, true)],
    );
    // 2. The lease ends or changes.
    let end = async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        match path {
            EndPath::Release => viewer.release(&id, &grant.lease).await.unwrap(),
            EndPath::HeartbeatLost => {
                let later = Instant::now() + HEARTBEAT_TIMEOUT + Duration::from_secs(1);
                let transition = control.expire(later, DEFAULT_IDLE_TIMEOUT).unwrap();
                control.discharge(transition).await;
            }
            EndPath::IdleTimeout => viewer.sweep(Duration::ZERO).await,
            EndPath::ViewerStopped => viewer.end_all().await,
            EndPath::DaemonStopped => registry.end_all_leases(EndReason::DaemonStopped).await,
            EndPath::CliTakeover => {
                let (_, changed) = registry.takeover(&id, EventSource::Cli).await.unwrap();
                assert!(changed);
            }
        }
    };
    // 3. Another agent client acts (native Key).
    let agent = async {
        tokio::time::sleep(Duration::from_millis(2)).await;
        registry
            .call_acting(&id, |s, epoch| {
                s.send_key(rdpilot::KeyAction::Type("a".into()), epoch)
            })
            .await
    };
    // 4. The input channel takes events again.
    let open = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        sink.open();
    };
    let (human, (), agent, ()) = tokio::join!(human, end, agent, open);
    assert_eq!(human.unwrap().applied, 1, "{path:?}");
    agent.unwrap();
    assert!(control.check_agent().is_ok(), "{path:?}");
    sink.log()
}

/// The sequence every lease end and change must produce: the press, its
/// release, and only then the agent's action.
fn released_then_agent() -> Vec<Rec> {
    vec![
        Rec::Human(key(SHIFT, true)),
        Rec::Human(key(SHIFT, false)),
        Rec::Agent,
    ]
}

#[tokio::test(start_paused = true)]
async fn release_sends_releases_before_the_next_agent_action() {
    assert_eq!(
        native_action_after(EndPath::Release).await,
        released_then_agent()
    );
}

#[tokio::test(start_paused = true)]
async fn heartbeat_loss_sends_releases_before_the_next_agent_action() {
    assert_eq!(
        native_action_after(EndPath::HeartbeatLost).await,
        released_then_agent()
    );
}

#[tokio::test(start_paused = true)]
async fn idle_timeout_sends_releases_before_the_next_agent_action() {
    assert_eq!(
        native_action_after(EndPath::IdleTimeout).await,
        released_then_agent()
    );
}

#[tokio::test(start_paused = true)]
async fn viewer_stop_sends_releases_before_the_next_agent_action() {
    assert_eq!(
        native_action_after(EndPath::ViewerStopped).await,
        released_then_agent()
    );
}

#[tokio::test(start_paused = true)]
async fn daemon_exit_sends_releases_before_the_next_agent_action() {
    assert_eq!(
        native_action_after(EndPath::DaemonStopped).await,
        released_then_agent()
    );
}

/// `rdpilot takeover` from one client, then a native action from another.
#[tokio::test(start_paused = true)]
async fn cli_takeover_sends_releases_before_another_agent_action() {
    assert_eq!(
        native_action_after(EndPath::CliTakeover).await,
        released_then_agent()
    );
}

/// A take by another tab sends the first tab's releases before the new
/// holder's first event, even while the first tab's input is in flight.
#[tokio::test(start_paused = true)]
async fn another_tab_acts_only_after_the_first_tabs_releases() {
    let Gated {
        sink, viewer, id, ..
    } = gated().await;
    let (first, _) = viewer.take(&id, tab(1)).await.unwrap();
    sink.shut();
    let first_input = viewer.input(
        &id,
        &first.lease,
        first.generation,
        (1, 1),
        vec![key(SHIFT, true)],
    );
    let second = async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        let (second, _) = viewer.take(&id, tab(2)).await.unwrap();
        viewer
            .input(
                &id,
                &second.lease,
                second.generation,
                (1, 1),
                vec![key(0x1E, true), key(0x1E, false)],
            )
            .await
            .unwrap()
    };
    let open = async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        sink.open();
    };
    let (first_input, second, ()) = tokio::join!(first_input, second, open);
    assert_eq!(first_input.unwrap().applied, 1);
    assert_eq!(second.applied, 2);
    assert_eq!(
        sink.log(),
        vec![
            Rec::Human(key(SHIFT, true)),
            Rec::Human(key(SHIFT, false)),
            Rec::Human(key(0x1E, true)),
            Rec::Human(key(0x1E, false)),
        ]
    );
}

/// Advance `epoch` by one: that epoch change is the only one the sink saw
/// since the last check, and it is the control's epoch now.
fn one_change(f: &Fixture, epoch: &mut InputEpoch) {
    *epoch = epoch.next();
    assert_eq!(f.sink.epochs(), vec![*epoch]);
    assert_eq!(f.control.input_epoch(), *epoch);
}

fn no_change(f: &Fixture, epoch: InputEpoch) {
    assert_eq!(f.sink.epochs(), Vec::<InputEpoch>::new());
    assert_eq!(f.control.input_epoch(), epoch);
}

/// Every controller change (a granted lease that ends or moves) sends one
/// epoch change before the next controller acts; a blur, a geometry fence,
/// an ended session and Cua acting calls under agent control send none.
#[tokio::test]
async fn every_controller_change_advances_the_epoch_once_and_blur_does_not() {
    let f = fixture();
    let mut epoch = InputEpoch::INITIAL;
    // Take from the agent.
    let a = take(&f, 1).await;
    one_change(&f, &mut epoch);
    // Blur and a geometry fence release without a change.
    f.control
        .input(&a.lease, 7, (800, 600), vec![left(true), key(SHIFT, true)])
        .await
        .unwrap();
    let transition = f.control.blur(&a.lease).unwrap();
    assert!(f.control.discharge(transition).await);
    f.control
        .input(&a.lease, 7, (800, 600), vec![left(true)])
        .await
        .unwrap();
    let report = f
        .control
        .input(
            &a.lease,
            7,
            (800, 600),
            vec![RawInput::PointerMove { x: 900, y: 1 }],
        )
        .await
        .unwrap();
    assert_eq!(report.dropped.geometry, 1);
    no_change(&f, epoch);
    // Take from another tab, then release.
    let b = take(&f, 2).await;
    one_change(&f, &mut epoch);
    let transition = f.control.release(&b.lease).unwrap();
    assert!(f.control.discharge(transition).await);
    one_change(&f, &mut epoch);
    // Heartbeat loss and idle timeout.
    for (later, idle) in [
        (
            Instant::now() + Duration::from_secs(6),
            DEFAULT_IDLE_TIMEOUT,
        ),
        (Instant::now(), Duration::ZERO),
    ] {
        take(&f, 3).await;
        one_change(&f, &mut epoch);
        let transition = f.control.expire(later, idle).unwrap();
        assert!(f.control.discharge(transition).await);
        one_change(&f, &mut epoch);
    }
    // Viewer stop, daemon stop, close of a live session.
    for reason in [
        EndReason::ViewerStopped,
        EndReason::DaemonStopped,
        EndReason::SessionEnded,
    ] {
        take(&f, 4).await;
        one_change(&f, &mut epoch);
        let transition = f.control.end_human(reason).unwrap();
        assert!(f.control.discharge(transition).await);
        one_change(&f, &mut epoch);
    }
    // The takeover verb and a Cua takeover.
    take(&f, 5).await;
    one_change(&f, &mut epoch);
    let (_, transition) = f.control.agent_takeover(EventSource::Cli);
    assert!(f.control.discharge(transition.unwrap()).await);
    one_change(&f, &mut epoch);
    take(&f, 5).await;
    one_change(&f, &mut epoch);
    let transition = f.control.admit_cua(1, true).unwrap().unwrap();
    assert!(f.control.discharge(transition).await);
    one_change(&f, &mut epoch);
    f.control.cua_finished(1);
    // Cua acting calls under agent control.
    assert!(f.control.admit_cua(2, false).unwrap().is_none());
    f.control.cua_finished(2);
    no_change(&f, epoch);
    // The session ended: nothing is sent or waited for.
    take(&f, 6).await;
    one_change(&f, &mut epoch);
    let transition = f.control.end_human_ended().unwrap();
    assert_eq!(transition.epoch(), None);
    assert!(f.control.discharge(transition).await);
    no_change(&f, epoch);
    assert_eq!(f.control.admit_agent().await.unwrap(), epoch);
}

/// Ending or taking over a take from the agent that is still pending is no
/// controller change: the agent keeps its epoch, so its native operation in
/// flight completes.
#[tokio::test(start_paused = true)]
async fn ending_a_pending_take_keeps_the_agents_epoch() {
    for takeover in [false, true] {
        let f = Arc::new(fixture());
        let epoch = f.control.admit_agent().await.unwrap();
        // The agent presses a button (a drag in flight holds the lock).
        f.sink.send(vec![left(true)], epoch).await.unwrap();
        let taking = {
            let f = Arc::clone(&f);
            tokio::spawn(async move { f.control.take(tab(9), std::future::pending::<()>()).await })
        };
        tokio::task::yield_now().await;
        assert!(f.control.human_active());
        let transition = if takeover {
            f.control.agent_takeover(EventSource::Cli).1.unwrap()
        } else {
            f.control.end_human(EndReason::ViewerStopped).unwrap()
        };
        assert_eq!(transition.epoch(), None);
        assert!(f.control.discharge(transition).await);
        assert!(taking.await.unwrap().is_err());
        no_change(&f, epoch);
        // The drag completes at its epoch: nothing was released.
        f.sink.send(vec![left(false)], epoch).await.unwrap();
        assert_eq!(f.sink.take(), vec![left(true), left(false)]);
    }
}

/// A session input whose first send waits at a barrier before the epoch
/// check: the window between a viewer's lease check and the session loop.
struct BarrierSink {
    inner: Sink,
    armed: std::sync::atomic::AtomicBool,
    barrier: tokio::sync::Semaphore,
}

impl BarrierSink {
    fn new() -> Self {
        BarrierSink {
            inner: Sink::default(),
            armed: std::sync::atomic::AtomicBool::new(false),
            barrier: tokio::sync::Semaphore::new(0),
        }
    }

    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn open(&self) {
        self.barrier.add_permits(1);
    }
}

impl HumanInput for BarrierSink {
    fn send(
        &self,
        events: Vec<RawInput>,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async move {
            if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                drop(self.barrier.acquire().await.unwrap());
            }
            self.inner.send(events, epoch).await
        })
    }

    fn set_epoch(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.set_epoch(epoch)
    }
}

impl AgentLog for BarrierSink {
    fn agent_acted(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.agent_acted(epoch)
    }
}

/// Tab A's event passes its lease check and waits before the session loop;
/// tab B takes the lease meanwhile. The take does not wait for A's request:
/// the loop refuses A's event as stale, nothing of it is sent, and B's first
/// event follows the epoch change.
#[tokio::test(start_paused = true)]
async fn the_old_tabs_input_after_a_take_is_rejected_stale() {
    let sink = Arc::new(BarrierSink::new());
    let events = Arc::new(SessionEvents::new(&"web".parse().unwrap(), 7, None));
    let control = Arc::new(SessionControl::new(
        "web",
        7,
        events,
        None,
        Some(Arc::clone(&sink) as Arc<dyn HumanInput>),
        EPOCH_CONFIRM_WAIT,
    ));
    let a = control.take(tab(1), async {}).await.unwrap();
    sink.arm();
    let a_input = {
        let control = Arc::clone(&control);
        let lease = a.lease.clone();
        tokio::spawn(async move {
            control
                .input(&lease, 7, (1, 1), vec![key(SHIFT, true)])
                .await
        })
    };
    tokio::task::yield_now().await;
    let started = tokio::time::Instant::now();
    let b = control.take(tab(2), async {}).await.unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO, "no wait for tab A");
    sink.open();
    let report = a_input.await.unwrap().unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.dropped.stale, 1);
    assert_eq!(report.lost, Some(NotHeld::Lost(Loss::by(human(2)))));
    assert!(sink.inner.take().is_empty(), "nothing of tab A was sent");
    control
        .input(&b.lease, 7, (1, 1), vec![key(0x1E, true)])
        .await
        .unwrap();
    assert_eq!(sink.inner.take(), vec![key(0x1E, true)]);
    assert_eq!(control.input_epoch(), InputEpoch::INITIAL.next().next());
}

/// A human's event waits before the session loop while the agent takes
/// over with `rdpilot takeover`: it is refused as stale, nothing stays
/// held, and the agent's next native action carries the new epoch.
#[tokio::test(start_paused = true)]
async fn a_humans_input_after_the_takeover_verb_is_rejected_stale() {
    let sink = Arc::new(BarrierSink::new());
    let registry = Arc::new(crate::registry::Registry::new(
        Arc::new(InputConnector(Arc::clone(&sink))),
        Arc::new(crate::seams::NoopReconciliationSink),
    ));
    let id = registry
        .open(
            Some("web".into()),
            "h".into(),
            rdpilot::ConnectionConfig::new("h", "u", "p"),
        )
        .await
        .unwrap();
    let viewer = crate::registry::ViewerControl::new(Arc::clone(&registry));
    let (grant, _) = viewer.take(&id, tab(1)).await.unwrap();
    assert_eq!(sink.inner.epochs(), vec![InputEpoch::INITIAL.next()]);
    sink.arm();
    let human_input = {
        let viewer = viewer.clone();
        let id = id.clone();
        let lease = grant.lease.clone();
        tokio::spawn(async move {
            viewer
                .input(
                    &id,
                    &lease,
                    grant.generation,
                    (1, 1),
                    vec![key(SHIFT, true)],
                )
                .await
        })
    };
    tokio::task::yield_now().await;
    let (_, changed) = registry.takeover(&id, EventSource::Cli).await.unwrap();
    assert!(changed);
    let current = InputEpoch::INITIAL.next().next();
    assert_eq!(sink.inner.epochs(), vec![current]);
    registry
        .call_acting(&id, |s, epoch| {
            s.send_key(rdpilot::KeyAction::Type("a".into()), epoch)
        })
        .await
        .unwrap();
    assert_eq!(*sink.inner.agent.lock().unwrap(), vec![current]);
    sink.open();
    let report = human_input.await.unwrap().unwrap();
    assert_eq!(report.dropped.stale, 1);
    assert_eq!(
        report.lost,
        Some(NotHeld::Lost(Loss::by(ControllerRef::Agent)))
    );
    assert!(sink.inner.take().is_empty());
    assert_eq!(sink.inner.model.lock().unwrap().held(), 0);
}

/// A session input whose epoch changes can be held unanswered.
#[derive(Default)]
struct StallingSink {
    inner: Sink,
    stall: std::sync::atomic::AtomicBool,
}

impl HumanInput for StallingSink {
    fn send(
        &self,
        events: Vec<RawInput>,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.send(events, epoch)
    }

    fn set_epoch(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        if self.stall.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(std::future::pending());
        }
        self.inner.set_epoch(epoch)
    }
}

/// An epoch change the session loop does not confirm within the confirm
/// wait refuses a take as busy (from the agent and from another tab) and
/// still ends a released lease; the epoch is never rolled back.
#[tokio::test(start_paused = true)]
async fn an_unconfirmed_epoch_change_refuses_the_take_and_still_ends_the_lease() {
    let wait = Duration::from_secs(1);
    let sink = Arc::new(StallingSink::default());
    let events = Arc::new(SessionEvents::new(&"web".parse().unwrap(), 7, None));
    let control = SessionControl::new(
        "web",
        7,
        events,
        None,
        Some(Arc::clone(&sink) as Arc<dyn HumanInput>),
        wait,
    );
    let n = InputEpoch::INITIAL;
    // From the agent.
    sink.stall.store(true, std::sync::atomic::Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    assert_eq!(
        control.take(tab(1), async {}).await.unwrap_err(),
        TakeError::Busy
    );
    assert_eq!(started.elapsed(), wait);
    assert!(!control.human_active());
    assert_eq!(control.admit_agent().await.unwrap(), n.next());
    // From another tab.
    sink.stall.store(false, std::sync::atomic::Ordering::SeqCst);
    let first = control.take(tab(1), async {}).await.unwrap();
    sink.stall.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        control.take(tab(2), async {}).await.unwrap_err(),
        TakeError::Busy
    );
    assert!(!control.human_active());
    assert_eq!(
        control.heartbeat(&first.lease),
        Err(NotHeld::Lost(Loss::by(human(2))))
    );
    assert_eq!(control.admit_agent().await.unwrap(), n.next().next().next());
    // A release.
    sink.stall.store(false, std::sync::atomic::Ordering::SeqCst);
    let held = control.take(tab(3), async {}).await.unwrap();
    sink.stall.store(true, std::sync::atomic::Ordering::SeqCst);
    let transition = control.release(&held.lease).unwrap();
    let started = tokio::time::Instant::now();
    assert!(!control.discharge(transition).await);
    assert_eq!(started.elapsed(), wait);
    assert!(!control.human_active());
    assert_eq!(
        control.admit_agent().await.unwrap(),
        n.next().next().next().next().next()
    );
}

/// A session input whose agent actions wait at `gate` and are then refused
/// as stale; human input and epoch changes go to `inner`.
struct StaleAgent {
    inner: Sink,
    gate: tokio::sync::Semaphore,
}

impl HumanInput for StaleAgent {
    fn send(
        &self,
        events: Vec<RawInput>,
        epoch: InputEpoch,
    ) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.send(events, epoch)
    }

    fn set_epoch(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        self.inner.set_epoch(epoch)
    }
}

impl AgentLog for StaleAgent {
    fn agent_acted(&self, epoch: InputEpoch) -> BoxFuture<'_, Result<(), DaemonError>> {
        Box::pin(async move {
            drop(self.gate.acquire().await.unwrap());
            Err(DaemonError::Sdk(rdpilot::Error::StaleInput {
                sent: epoch,
                current: epoch.next(),
            }))
        })
    }
}

#[tokio::test(start_paused = true)]
async fn stale_agent_input_maps_to_human_control_only_with_a_current_lease() {
    use rdpilot_ipc::{Request, WireErrorCode, WireResponse};
    let sink = Arc::new(StaleAgent {
        inner: Sink::default(),
        gate: tokio::sync::Semaphore::new(0),
    });
    let registry = Arc::new(crate::registry::Registry::new(
        Arc::new(InputConnector(Arc::clone(&sink))),
        Arc::new(crate::seams::NoopReconciliationSink),
    ));
    let id = registry
        .open(
            Some("web".into()),
            "h".into(),
            rdpilot::ConnectionConfig::new("h", "u", "p"),
        )
        .await
        .unwrap();
    let key_request = || Request::Key {
        session: id.clone(),
        action: rdpilot::KeyAction::Type("a".into()),
    };
    // A take starts while the agent's key is in flight: the take is
    // pending (a lease exists) when the stale answer arrives.
    let agent = {
        let registry = Arc::clone(&registry);
        let request = key_request();
        tokio::spawn(async move { crate::dispatch::dispatch(&registry, request).await })
    };
    tokio::task::yield_now().await;
    let viewer = crate::registry::ViewerControl::new(Arc::clone(&registry));
    let taking = {
        let viewer = viewer.clone();
        let id = id.clone();
        tokio::spawn(async move { viewer.take(&id, tab(1)).await })
    };
    tokio::task::yield_now().await;
    assert!(registry.control(&id).unwrap().human_active());
    sink.gate.add_permits(1);
    match agent.await.unwrap() {
        WireResponse::Error(error) => {
            assert_eq!(error.code, WireErrorCode::HumanControl);
            assert_eq!(
                error.controller.and_then(|c| c.address).as_deref(),
                Some("100.64.0.1")
            );
        }
        other => panic!("{other:?}"),
    }
    let (grant, _) = taking.await.unwrap().unwrap();
    viewer.release(&id, &grant.lease).await.unwrap();
    // No lease: the stale answer is internal.
    sink.gate.add_permits(1);
    match crate::dispatch::dispatch(&registry, key_request()).await {
        WireResponse::Error(error) => assert_eq!(error.code, WireErrorCode::Internal),
        other => panic!("{other:?}"),
    }
}
