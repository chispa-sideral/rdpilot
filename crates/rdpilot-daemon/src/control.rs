//! Control lease: who may send input to one live session.
//!
//! - Each live session has one [`SessionControl`]. The agent (native verbs
//!   and Cua tool calls) controls by default; one live viewer tab may hold
//!   a human lease instead. At most one controller exists at any time.
//! - Every change of controller that displaces a human holder returns a
//!   [`Transition`] that carries the keys and buttons the holder pressed and
//!   did not release. [`SessionControl::discharge`] sends those releases
//!   (awaited, through the session's own input channel) before the next
//!   controller's first action.
//! - Every lease end and change follows one order: the transition is
//!   registered as pending in the same critical section that changes the
//!   lease, and agent admission ([`SessionControl::admit_agent`],
//!   [`SessionControl::wait_released`] before a Cua call is forwarded) and
//!   a human take wait until no transition is pending. So no controller
//!   acts while a key or button of the previous holder is still down.
//! - A human take from the agent marks the lease held at once, so new agent
//!   actions are refused, and then waits (bounded by [`TAKE_WAIT`]) for the
//!   agent operations already in flight: a native operation holding the
//!   per-session lock, and forwarded acting Cua calls without an answer. It
//!   never interrupts them. If the bound expires, the take is undone.
//!   The heartbeat and idle clocks start only when the take completes.
//! - Cua admission ([`SessionControl::admit_cua`]) checks the controller
//!   and counts the call as in flight in one critical section of the state
//!   mutex, so a take cannot slip between the check and the count.
//! - Events carry controller descriptors only. A lease id appears only in
//!   the take response and the holder's own requests: never in an event,
//!   a status, an error text or a log line. Input content (keys, text,
//!   coordinates) is never logged or recorded.
//! - The std mutex is never held across an `.await`.

use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rdpilot_ipc::{WireController, WireControllerKind};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::events::{EventKind, EventSource, SessionEvents};
use crate::registry::iso8601_from_system_time;
use crate::seams::{DaemonError, HumanInput, ViewFrameSource};

/// Longest wait of a human take for agent operations in flight.
pub(crate) const TAKE_WAIT: Duration = Duration::from_secs(10);

/// A human lease ends after this long without a heartbeat from its tab.
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default idle timeout of a human lease (no human input).
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest wait for the session's input channel to take one event.
pub(crate) const INPUT_SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Lease ends remembered per session for loss notices.
const RECENT_ENDS: usize = 16;

/// A mouse button a human can press in the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Left,
    Middle,
    Right,
    X1,
    X2,
}

/// One human input event. Coordinates are framebuffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HumanEvent {
    Move {
        x: u16,
        y: u16,
    },
    Button {
        button: PointerButton,
        down: bool,
    },
    Wheel {
        vertical: bool,
        units: i16,
    },
    Key {
        code: u8,
        extended: bool,
        down: bool,
    },
}

impl HumanEvent {
    /// Pointer events are fenced by frame geometry (except the release of
    /// a held button); key events are not.
    fn is_pointer(&self) -> bool {
        !matches!(self, HumanEvent::Key { .. })
    }
}

/// A key or button pressed by the human holder and not yet released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Held {
    Button(PointerButton),
    Key { code: u8, extended: bool },
}

impl Held {
    fn release(self) -> HumanEvent {
        match self {
            Held::Button(button) => HumanEvent::Button {
                button,
                down: false,
            },
            Held::Key { code, extended } => HumanEvent::Key {
                code,
                extended,
                down: false,
            },
        }
    }
}

/// A controller as events and loss notices describe it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControllerRef {
    Agent,
    Human { address: String },
}

/// Why a human lease ended without a takeover or a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    HeartbeatLost,
    IdleTimeout,
    ViewerStopped,
    SessionEnded,
    DaemonStopped,
}

/// Why a tab no longer holds its lease (the loss notice data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Loss {
    /// Who took over, for a takeover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<ControllerRef>,
    /// Why it ended otherwise (`released` or an [`EndReason`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Loss {
    fn by(controller: ControllerRef) -> Self {
        Loss {
            by: Some(controller),
            reason: None,
        }
    }

    fn reason(reason: EndReason) -> Self {
        Loss {
            by: None,
            reason: Some(end_reason_str(reason).to_owned()),
        }
    }

    fn released() -> Self {
        Loss {
            by: None,
            reason: Some("released".to_owned()),
        }
    }
}

fn end_reason_str(reason: EndReason) -> &'static str {
    match reason {
        EndReason::HeartbeatLost => "heartbeat_lost",
        EndReason::IdleTimeout => "idle_timeout",
        EndReason::ViewerStopped => "viewer_stopped",
        EndReason::SessionEnded => "session_ended",
        EndReason::DaemonStopped => "daemon_stopped",
    }
}

fn source_str(source: EventSource) -> &'static str {
    match source {
        EventSource::Cua => "cua",
        EventSource::Cli => "cli",
        EventSource::Viewer => "viewer",
        EventSource::Daemon => "daemon",
    }
}

fn describe(controller: &ControllerRef) -> String {
    match controller {
        ControllerRef::Agent => "the agent".to_owned(),
        ControllerRef::Human { address } => format!("human viewer {address}"),
    }
}

/// A refused request that names a lease that is not current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotHeld {
    /// The lease ended; the loss notice data.
    Lost(Loss),
    /// No such lease is known (never granted here, or forgotten).
    Unknown,
}

/// Why a human take failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TakeError {
    /// Agent work was still in flight when [`TAKE_WAIT`] expired.
    Busy,
    /// The pending take was displaced before it completed.
    Lost(Loss),
}

/// The message of a take refused with [`TakeError::Busy`].
pub(crate) const BUSY_MESSAGE: &str = "an agent operation is still running (a tool call, native \
input, or a long file transfer or screenshot); it was not interrupted; try again";

/// A granted human lease.
#[derive(Debug, Clone)]
pub(crate) struct Grant {
    /// The lease id (secret to the holder's tab).
    pub(crate) lease: String,
    pub(crate) generation: u64,
    pub(crate) controller: WireController,
}

/// Dropped input counts of one request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Drops {
    /// Events of a lease that is no longer current.
    pub stale: u64,
    /// Pointer events aimed at another frame geometry.
    pub geometry: u64,
    /// Events for another session generation.
    pub generation: u64,
}

/// The result of one input request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputReport {
    pub(crate) applied: u64,
    pub(crate) dropped: Drops,
    /// `None` while the lease is held; the loss otherwise.
    pub(crate) lost: Option<NotHeld>,
}

/// Why an input request could not be applied at all.
#[derive(Debug)]
pub(crate) enum InputError {
    /// The session's input channel did not take an event in time, or the
    /// session has no input path.
    Unavailable,
}

/// An agent Cua call refused under human control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The MCP message (names the holder and the `takeover` argument).
    pub(crate) mcp_message: String,
}

#[derive(Debug, Clone)]
struct Lease {
    id: String,
    address: IpAddr,
    since_wall: SystemTime,
    /// `false` while the take waits for agent work in flight.
    granted: bool,
    last_heartbeat: Instant,
    last_input: Instant,
    held: BTreeSet<Held>,
}

impl Lease {
    fn controller(&self) -> ControllerRef {
        ControllerRef::Human {
            address: self.address.to_string(),
        }
    }
}

#[derive(Default)]
struct State {
    lease: Option<Lease>,
    /// Forwarded acting Cua calls without an answer.
    inflight: usize,
    /// Recently ended leases: id -> loss.
    ended: VecDeque<(String, Loss)>,
    /// The latest human activity (accepted input or a lease end).
    human_activity: Option<(Instant, SystemTime)>,
}

impl State {
    fn remember(&mut self, id: String, loss: Loss) {
        if self.ended.len() >= RECENT_ENDS {
            self.ended.pop_front();
        }
        self.ended.push_back((id, loss));
    }

    fn loss_of(&self, id: &str) -> NotHeld {
        self.ended
            .iter()
            .rev()
            .find(|(ended, _)| ended == id)
            .map_or(NotHeld::Unknown, |(_, loss)| NotHeld::Lost(loss.clone()))
    }

    fn touch_human(&mut self) {
        self.human_activity = Some((Instant::now(), SystemTime::now()));
    }

    /// The granted lease named `id`, or why it is not held.
    fn held(&mut self, id: &str) -> Result<&mut Lease, NotHeld> {
        match &self.lease {
            Some(lease) if lease.id == id && lease.granted => {}
            _ => return Err(self.loss_of(id)),
        }
        self.lease.as_mut().ok_or(NotHeld::Unknown)
    }
}

/// Transitions whose releases are not sent yet.
#[derive(Debug, Default)]
struct Pending {
    count: AtomicUsize,
    /// Signalled whenever `count` drops.
    done: Notify,
}

/// Counts one transition as pending until it is dropped (after
/// [`SessionControl::discharge`], or when a caller is cancelled).
#[derive(Debug)]
struct PendingGuard(Arc<Pending>);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
        self.0.done.notify_waiters();
    }
}

/// The release obligation (and the recorded change) of one control change.
/// Pass it to [`SessionControl::discharge`] before the next controller acts.
/// Until it is discharged (or dropped), agent admission and human takes of
/// the session wait.
#[must_use = "a transition's releases must be discharged"]
#[derive(Debug)]
pub(crate) struct Transition {
    release: Vec<Held>,
    _pending: PendingGuard,
}

impl Transition {
    #[cfg(test)]
    pub(crate) fn releases(&self) -> usize {
        self.release.len()
    }
}

/// The control lease of one live session incarnation.
pub struct SessionControl {
    session: String,
    generation: u64,
    events: Arc<SessionEvents>,
    frame: Option<Arc<dyn ViewFrameSource>>,
    input: Option<Arc<dyn HumanInput>>,
    state: Mutex<State>,
    /// Signalled whenever the in-flight Cua count drops.
    settled: Notify,
    /// Transitions not yet discharged.
    pending: Arc<Pending>,
    /// Serializes human input and releases.
    input_lock: tokio::sync::Mutex<()>,
}

impl SessionControl {
    pub(crate) fn new(
        session: &str,
        generation: u64,
        events: Arc<SessionEvents>,
        frame: Option<Arc<dyn ViewFrameSource>>,
        input: Option<Arc<dyn HumanInput>>,
    ) -> Self {
        SessionControl {
            session: session.to_owned(),
            generation,
            events,
            frame,
            input,
            state: Mutex::new(State::default()),
            settled: Notify::new(),
            pending: Arc::new(Pending::default()),
            input_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// A pending transition that releases `held`. Call it in the same
    /// critical section of the state mutex that changes the lease, so an
    /// admission check under that mutex sees the lease change and the
    /// pending release together.
    fn transition(&self, held: impl IntoIterator<Item = Held>) -> Transition {
        self.pending.count.fetch_add(1, Ordering::AcqRel);
        Transition {
            release: held.into_iter().collect(),
            _pending: PendingGuard(Arc::clone(&self.pending)),
        }
    }

    /// The current frame size (`None` without frames).
    pub(crate) fn geometry(&self) -> Option<(u32, u32)> {
        self.frame.as_ref().and_then(|frame| frame.geometry())
    }

    /// Whether the session can take human input.
    pub(crate) fn has_input(&self) -> bool {
        self.input.is_some()
    }

    /// Whether a human holds (or is taking) the lease.
    pub(crate) fn human_active(&self) -> bool {
        self.state().lease.is_some()
    }

    /// The latest human activity (accepted input or a lease end).
    pub(crate) fn human_activity(&self) -> Option<(Instant, SystemTime)> {
        self.state().human_activity
    }

    /// The current controller for status output.
    pub(crate) fn controller(&self) -> WireController {
        match &self.state().lease {
            None => WireController::agent(),
            Some(lease) => human_wire(lease),
        }
    }

    /// Record a change in the session's event log and the daemon log.
    fn record(&self, source: EventSource, kind: EventKind) {
        let line = match &kind {
            EventKind::ControlTaken { by } => format!("taken by {}", describe(by)),
            EventKind::ControlTakenOver { from, by } => {
                format!("taken over from {} by {}", describe(from), describe(by))
            }
            EventKind::ControlReleased { from } => {
                format!("released by {}; the agent controls", describe(from))
            }
            EventKind::ControlEnded { from, reason } => format!(
                "lease of {} ended ({}); the agent controls",
                describe(from),
                end_reason_str(*reason)
            ),
            _ => String::new(),
        };
        eprintln!(
            "rdpilot-daemon: control of session \"{}\": {line} (source {})",
            self.session,
            source_str(source)
        );
        self.events.record(source, kind);
    }

    /// Refuse agent input while a human holds or is taking the lease.
    ///
    /// # Errors
    ///
    /// [`DaemonError::HumanControl`] naming the holder and the CLI command.
    #[cfg(test)]
    pub(crate) fn check_agent(&self) -> Result<(), DaemonError> {
        match &self.state().lease {
            None => Ok(()),
            Some(lease) => Err(self.human_control_error(lease)),
        }
    }

    /// Admit one native agent action: refused while a human holds or is
    /// taking the lease; otherwise it waits until the releases of every
    /// earlier lease end or change are sent.
    ///
    /// # Errors
    ///
    /// [`DaemonError::HumanControl`] naming the holder and the CLI command.
    pub(crate) async fn admit_agent(&self) -> Result<(), DaemonError> {
        self.until_released(|| {
            let state = self.state();
            match &state.lease {
                Some(lease) => Some(Err(self.human_control_error(lease))),
                None => self.released().then_some(Ok(())),
            }
        })
        .await
    }

    /// Wait until the releases of every earlier lease end or change are
    /// sent (before an admitted Cua call is forwarded).
    pub(crate) async fn wait_released(&self) {
        self.until_released(|| self.released().then_some(())).await;
    }

    /// Whether no transition is pending.
    fn released(&self) -> bool {
        self.pending.count.load(Ordering::Acquire) == 0
    }

    /// Poll `ready` now and after every discharged transition until it
    /// answers.
    async fn until_released<T>(&self, ready: impl FnMut() -> Option<T>) -> T {
        until_signalled(&self.pending.done, ready).await
    }

    fn human_control_error(&self, lease: &Lease) -> DaemonError {
        DaemonError::HumanControl {
            session: self.session.clone(),
            address: lease.address.to_string(),
            since: iso8601_from_system_time(lease.since_wall),
        }
    }

    /// Admit acting Cua calls, of which `count` expect an answer. Under
    /// agent control those are counted in flight. Under a human lease they
    /// are refused unless `takeover`: then the lease ends and they are
    /// counted, in the same critical section, so a take can neither slip
    /// between the check and the count nor take the lease back before they
    /// are forwarded. The returned transition must be discharged before the
    /// calls are forwarded.
    ///
    /// # Errors
    ///
    /// [`Refusal`] with the MCP message while a human holds the lease.
    pub(crate) fn admit_cua(
        &self,
        count: usize,
        takeover: bool,
    ) -> Result<Option<Transition>, Refusal> {
        let mut state = self.state();
        let transition = match &state.lease {
            None => None,
            Some(lease) if !takeover => {
                return Err(Refusal {
                    mcp_message: mcp_refusal(&self.session, lease),
                })
            }
            Some(_) => self.agent_takeover_locked(&mut state, EventSource::Cua),
        };
        state.inflight += count;
        Ok(transition)
    }

    /// `count` admitted acting Cua calls were answered (or will never be).
    pub(crate) fn cua_finished(&self, count: usize) {
        if count == 0 {
            return;
        }
        {
            let mut state = self.state();
            state.inflight = state.inflight.saturating_sub(count);
        }
        self.settled.notify_waiters();
    }

    /// Forwarded acting Cua calls without an answer.
    #[cfg(test)]
    pub(crate) fn inflight(&self) -> usize {
        self.state().inflight
    }

    /// End any human lease in favour of the agent. Returns the previous
    /// controller and the release obligation (`None` when the agent
    /// already controlled).
    pub(crate) fn agent_takeover(
        &self,
        source: EventSource,
    ) -> (WireController, Option<Transition>) {
        let mut state = self.state();
        let previous = state
            .lease
            .as_ref()
            .map_or_else(WireController::agent, human_wire);
        let transition = self.agent_takeover_locked(&mut state, source);
        (previous, transition)
    }

    fn agent_takeover_locked(&self, state: &mut State, source: EventSource) -> Option<Transition> {
        let lease = state.lease.take()?;
        let from = lease.controller();
        state.remember(lease.id.clone(), Loss::by(ControllerRef::Agent));
        state.touch_human();
        self.record(
            source,
            EventKind::ControlTakenOver {
                from,
                by: ControllerRef::Agent,
            },
        );
        Some(self.transition(lease.held))
    }

    /// End a human lease for `reason` (daemon source). `None` when none.
    pub(crate) fn end_human(&self, reason: EndReason) -> Option<Transition> {
        let mut state = self.state();
        let lease = state.lease.take()?;
        state.remember(lease.id.clone(), Loss::reason(reason));
        state.touch_human();
        if lease.granted {
            self.record(
                EventSource::Daemon,
                EventKind::ControlEnded {
                    from: lease.controller(),
                    reason,
                },
            );
        }
        Some(self.transition(lease.held))
    }

    /// Send the releases of `transition`, awaited, through the session's
    /// input channel. Serialized with human input. Its pending count ends
    /// when they are sent.
    pub(crate) async fn discharge(&self, transition: Transition) {
        let _guard = self.input_lock.lock().await;
        self.send_releases(transition.release).await;
    }

    async fn send_releases(&self, release: Vec<Held>) {
        if release.is_empty() {
            return;
        }
        if let Some(input) = &self.input {
            let events = release.into_iter().map(Held::release).collect();
            // Best effort: a closed or stalled session has no stuck key.
            let _ = tokio::time::timeout(INPUT_SEND_TIMEOUT, input.send(events)).await;
        }
    }

    /// Take the lease for a viewer tab at `address`. From the agent, the
    /// take is marked at once and then waits (bounded) for `agent_idle`
    /// (the per-session lock) and for in-flight acting Cua calls. From
    /// another tab, the lease moves at once after that tab's releases.
    ///
    /// # Errors
    ///
    /// [`TakeError::Busy`] when agent work did not finish within
    /// [`TAKE_WAIT`]; [`TakeError::Lost`] when displaced while waiting.
    pub(crate) async fn take<F>(&self, address: IpAddr, agent_idle: F) -> Result<Grant, TakeError>
    where
        F: Future<Output = ()>,
    {
        self.take_within(address, agent_idle, TAKE_WAIT).await
    }

    pub(crate) async fn take_within<F>(
        &self,
        address: IpAddr,
        agent_idle: F,
        bound: Duration,
    ) -> Result<Grant, TakeError>
    where
        F: Future<Output = ()>,
    {
        let now = Instant::now();
        let mut lease = Lease {
            id: new_lease_id(),
            address,
            since_wall: SystemTime::now(),
            granted: false,
            last_heartbeat: now,
            last_input: now,
            held: BTreeSet::new(),
        };
        let id = lease.id.clone();
        let moved = {
            let mut state = self.state();
            match state.lease.take() {
                Some(old) if old.granted => {
                    // From another tab: no agent work can be in flight.
                    let from = old.controller();
                    let by = lease.controller();
                    state.remember(old.id.clone(), Loss::by(by.clone()));
                    lease.granted = true;
                    state.lease = Some(lease.clone());
                    self.record(
                        EventSource::Viewer,
                        EventKind::ControlTakenOver { from, by },
                    );
                    Some(self.transition(old.held))
                }
                pending => {
                    if let Some(pending) = pending {
                        state.remember(pending.id, Loss::by(lease.controller()));
                    }
                    state.lease = Some(lease.clone());
                    None
                }
            }
        };
        if let Some(transition) = moved {
            self.discharge(transition).await;
            self.wait_released().await;
            return Ok(self.grant(&lease));
        }

        // A take abandoned while it waits (the tab went away) is undone.
        let mut pending = PendingTake {
            control: self,
            id: &id,
            armed: true,
        };
        let settled = tokio::time::timeout(bound, async {
            agent_idle.await;
            self.wait_settled().await;
            self.wait_released().await;
        })
        .await;
        pending.armed = false;

        let mut state = self.state();
        let ours = matches!(&state.lease, Some(current) if current.id == id);
        if !ours {
            return Err(match state.loss_of(&id) {
                NotHeld::Lost(loss) => TakeError::Lost(loss),
                NotHeld::Unknown => TakeError::Lost(Loss::by(ControllerRef::Agent)),
            });
        }
        if settled.is_err() {
            state.lease = None;
            return Err(TakeError::Busy);
        }
        let now = Instant::now();
        let Some(current) = state.lease.as_mut() else {
            return Err(TakeError::Busy);
        };
        current.granted = true;
        current.last_heartbeat = now;
        current.last_input = now;
        current.since_wall = SystemTime::now();
        let granted = current.clone();
        drop(state);
        self.record(
            EventSource::Viewer,
            EventKind::ControlTaken {
                by: granted.controller(),
            },
        );
        Ok(self.grant(&granted))
    }

    fn grant(&self, lease: &Lease) -> Grant {
        Grant {
            lease: lease.id.clone(),
            generation: self.generation,
            controller: human_wire(lease),
        }
    }

    /// Wait until no acting Cua call is in flight.
    async fn wait_settled(&self) {
        until_signalled(&self.settled, || (self.state().inflight == 0).then_some(())).await;
    }

    /// Whether `lease` is the granted lease, without changing anything.
    ///
    /// # Errors
    ///
    /// [`NotHeld`] for any other id.
    pub(crate) fn check_lease(&self, lease: &str) -> Result<(), NotHeld> {
        self.state().held(lease).map(|_| ())
    }

    /// The holder's heartbeat. Never counts as input.
    ///
    /// # Errors
    ///
    /// [`NotHeld`] for a lease that is not current.
    pub(crate) fn heartbeat(&self, lease: &str) -> Result<(), NotHeld> {
        let mut state = self.state();
        let lease = state.held(lease)?;
        lease.last_heartbeat = Instant::now();
        Ok(())
    }

    /// The holder's page lost focus or visibility: release what it holds
    /// and keep the lease.
    ///
    /// # Errors
    ///
    /// [`NotHeld`] for a lease that is not current.
    pub(crate) fn blur(&self, lease: &str) -> Result<Transition, NotHeld> {
        let mut state = self.state();
        let lease = state.held(lease)?;
        lease.last_heartbeat = Instant::now();
        let held = std::mem::take(&mut lease.held);
        Ok(self.transition(held))
    }

    /// The holder releases control to the agent.
    ///
    /// # Errors
    ///
    /// [`NotHeld`] for a lease that is not current.
    pub(crate) fn release(&self, lease: &str) -> Result<Transition, NotHeld> {
        let mut state = self.state();
        state.held(lease)?;
        let Some(lease) = state.lease.take() else {
            return Err(NotHeld::Unknown);
        };
        state.remember(lease.id.clone(), Loss::released());
        state.touch_human();
        self.record(
            EventSource::Viewer,
            EventKind::ControlReleased {
                from: lease.controller(),
            },
        );
        Ok(self.transition(lease.held))
    }

    /// End a granted lease whose heartbeat or input stopped. A take that
    /// still waits for agent work is never expired.
    pub(crate) fn expire(&self, now: Instant, idle_timeout: Duration) -> Option<Transition> {
        let reason = {
            let state = self.state();
            let lease = state.lease.as_ref().filter(|lease| lease.granted)?;
            if now.saturating_duration_since(lease.last_heartbeat) > HEARTBEAT_TIMEOUT {
                EndReason::HeartbeatLost
            } else if now.saturating_duration_since(lease.last_input) >= idle_timeout {
                EndReason::IdleTimeout
            } else {
                return None;
            }
        };
        self.end_human(reason)
    }

    /// Apply the holder's input. Each event is checked against the current
    /// lease, and pointer events against the current frame geometry, right
    /// before it is sent; events of an older generation are dropped. The
    /// release of a held button is never fenced by geometry, and a fenced
    /// pointer event releases every held button. Runs of pointer moves are
    /// coalesced to the last one.
    ///
    /// # Errors
    ///
    /// [`InputError::Unavailable`] when the session's input channel does
    /// not take an event within [`INPUT_SEND_TIMEOUT`].
    pub(crate) async fn input(
        &self,
        lease: &str,
        generation: u64,
        geometry: (u32, u32),
        events: Vec<HumanEvent>,
    ) -> Result<InputReport, InputError> {
        let mut report = InputReport {
            applied: 0,
            dropped: Drops::default(),
            lost: None,
        };
        let total = u64::try_from(events.len()).unwrap_or(u64::MAX);
        if generation != self.generation {
            report.dropped.generation = total;
            report.lost = self.check_lease(lease).err();
            return Ok(report);
        }
        let Some(input) = &self.input else {
            return Err(InputError::Unavailable);
        };
        let _guard = self.input_lock.lock().await;
        let events = coalesce(events);
        let mut remaining = u64::try_from(events.len()).unwrap_or(u64::MAX);
        for event in events {
            remaining -= 1;
            let current = self.frame.as_ref().and_then(|frame| frame.geometry());
            let fenced = {
                let mut state = self.state();
                let holder = match state.held(lease) {
                    Ok(holder) => holder,
                    Err(lost) => {
                        report.dropped.stale += remaining + 1;
                        report.lost = Some(lost);
                        return Ok(report);
                    }
                };
                let off_frame = event.is_pointer()
                    && !geometry_matches(&event, geometry, current)
                    && !releases_held_button(&holder.held, &event);
                if off_frame {
                    // The pointer no longer aims at this frame: no button
                    // stays down behind a dropped release.
                    let buttons: Vec<Held> = holder
                        .held
                        .iter()
                        .copied()
                        .filter(|held| matches!(held, Held::Button(_)))
                        .collect();
                    for button in &buttons {
                        holder.held.remove(button);
                    }
                    Some(buttons)
                } else {
                    track(&mut holder.held, &event);
                    let now = Instant::now();
                    holder.last_input = now;
                    holder.last_heartbeat = now;
                    state.touch_human();
                    None
                }
            };
            if let Some(buttons) = fenced {
                report.dropped.geometry += 1;
                self.send_releases(buttons).await;
                continue;
            }
            match tokio::time::timeout(INPUT_SEND_TIMEOUT, input.send(vec![event])).await {
                Ok(Ok(())) => report.applied += 1,
                _ => return Err(InputError::Unavailable),
            }
        }
        Ok(report)
    }
}

/// Undoes a pending take when its future is dropped mid-wait.
struct PendingTake<'a> {
    control: &'a SessionControl,
    id: &'a str,
    armed: bool,
}

impl Drop for PendingTake<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.control.state();
        if matches!(&state.lease, Some(lease) if lease.id == self.id && !lease.granted) {
            state.lease = None;
        }
    }
}

/// The wire form of a human lease holder.
fn human_wire(lease: &Lease) -> WireController {
    WireController {
        kind: WireControllerKind::Human,
        address: Some(lease.address.to_string()),
        since: Some(iso8601_from_system_time(lease.since_wall)),
    }
}

/// `HH:MM:SS UTC` of an ISO-8601 time (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn clock_time(iso: &str) -> String {
    iso.get(11..19)
        .map_or_else(|| iso.to_owned(), |hms| format!("{hms} UTC"))
}

/// The session name as a shell argument: plain names as they are, others
/// in single quotes.
pub(crate) fn shell_arg(name: &str) -> String {
    if !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "'\\''"))
    }
}

/// The CLI/IPC human-control message.
pub(crate) fn cli_refusal(session: &str, address: &str, since_iso: &str) -> String {
    format!(
        "session \"{session}\" is controlled by human viewer {address} since {}; wait and retry, \
or take over with: rdpilot takeover --session {}",
        clock_time(since_iso),
        shell_arg(session)
    )
}

/// The MCP human-control message.
fn mcp_refusal(session: &str, lease: &Lease) -> String {
    format!(
        "session \"{session}\" is controlled by human viewer {} since {}; wait and retry, or \
repeat this call with \"takeover\": true to take control",
        lease.address,
        clock_time(&iso8601_from_system_time(lease.since_wall))
    )
}

fn geometry_matches(event: &HumanEvent, aimed: (u32, u32), current: Option<(u32, u32)>) -> bool {
    if current != Some(aimed) {
        return false;
    }
    match *event {
        HumanEvent::Move { x, y } => u32::from(x) < aimed.0 && u32::from(y) < aimed.1,
        _ => true,
    }
}

/// Poll `ready` now and after every signal of `notify` until it answers.
async fn until_signalled<T>(notify: &Notify, mut ready: impl FnMut() -> Option<T>) -> T {
    loop {
        let signalled = notify.notified();
        tokio::pin!(signalled);
        signalled.as_mut().enable();
        if let Some(answer) = ready() {
            return answer;
        }
        signalled.await;
    }
}

/// Whether `event` releases a button the holder holds.
fn releases_held_button(held: &BTreeSet<Held>, event: &HumanEvent) -> bool {
    matches!(*event, HumanEvent::Button { button, down: false } if held.contains(&Held::Button(button)))
}

fn track(held: &mut BTreeSet<Held>, event: &HumanEvent) {
    match *event {
        HumanEvent::Button { button, down } => {
            if down {
                held.insert(Held::Button(button));
            } else {
                held.remove(&Held::Button(button));
            }
        }
        HumanEvent::Key {
            code,
            extended,
            down,
        } => {
            let key = Held::Key { code, extended };
            if down {
                held.insert(key);
            } else {
                held.remove(&key);
            }
        }
        HumanEvent::Move { .. } | HumanEvent::Wheel { .. } => {}
    }
}

/// Keep only the last of each run of consecutive pointer moves.
fn coalesce(events: Vec<HumanEvent>) -> Vec<HumanEvent> {
    let mut out: Vec<HumanEvent> = Vec::with_capacity(events.len());
    for event in events {
        if let (Some(HumanEvent::Move { .. }), HumanEvent::Move { .. }) = (out.last(), event) {
            out.pop();
        }
        out.push(event);
    }
    out
}

/// 128 random bits as hex. Falls back to time-derived bits if the OS
/// source fails (the token still guards every request).
fn new_lease_id() -> String {
    static FALLBACK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let counter = u128::from(FALLBACK.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        bytes = (nanos ^ (counter << 64) ^ u128::from(std::process::id())).to_be_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
