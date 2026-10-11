//! Control lease: who may send input to one live session.
//!
//! - Each live session has one [`SessionControl`]. The agent (native verbs
//!   and Cua tool calls) controls by default; one live viewer tab may hold
//!   a human lease instead. At most one controller exists at any time.
//! - Every controller change (a granted human lease that ends or moves)
//!   advances the session's input epoch in the critical section that changes
//!   the lease, and returns a [`Transition`] that carries the new epoch.
//!   [`SessionControl::discharge`] sends it to the session loop, which
//!   releases every key and button still pressed before it confirms. Input
//!   of the old controller that reaches the loop after the change is
//!   refused there as stale, so it can neither act nor leave a press behind.
//!   A pending take that is never granted is not a controller change.
//! - Every operation captures the epoch once when it is admitted: an agent
//!   action in [`SessionControl::admit_agent`], a viewer request at its
//!   first lease check, the bootstrap from [`SessionControl::input_epoch`].
//! - A blur or a geometry fence releases the holder's keys or buttons
//!   without a controller change: such a [`Transition`] carries the
//!   releases, sent with the epoch current when it was created.
//! - Every lease end and change follows one order: the transition is
//!   registered as pending in the same critical section that changes the
//!   lease, and agent admission ([`SessionControl::admit_agent`],
//!   [`SessionControl::wait_released`] before a Cua call is forwarded) and
//!   a human take wait until no transition is pending. So no controller
//!   acts before the previous holder's keys and buttons are released.
//! - A human take is granted only after the session loop confirmed the
//!   epoch change, within [`TAKE_BOUND`] and at most the confirm wait; an
//!   unconfirmed change refuses the take as busy. The epoch is never rolled
//!   back: the agent then acts at the new epoch.
//! - A human take from the agent marks the lease held at once, so new agent
//!   actions are refused, and then waits, within the one [`TAKE_BOUND`],
//!   only for forwarded acting Cua calls without an answer and for the
//!   epoch confirmation. It never interrupts a Cua call; if the bound
//!   expires while one is in flight, the take is undone. Agent native input
//!   in flight is not waited for: the epoch change cuts it at its next
//!   batch, and the session loop releases what it pressed. Transfers and
//!   reads never delay a take. The heartbeat and idle clocks start only
//!   when the take completes.
//! - Cua admission ([`SessionControl::admit_cua`]) checks the controller
//!   and counts the call as in flight in one critical section of the state
//!   mutex, so a take cannot slip between the check and the count.
//! - Events carry controller descriptors only. A lease id appears only in
//!   the take response and the holder's own requests: never in an event,
//!   a status, an error text or a log line. Input content (keys, text,
//!   coordinates) is never logged or recorded.
//! - The std mutex is never held across an `.await`.

use std::collections::{BTreeSet, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rdpilot::InputEpoch;
use rdpilot_ipc::{WireController, WireControllerKind};
use rdpilot_vocab::{PointerButton, RawInput};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::events::{EventKind, EventSource, SessionEvents};
use crate::registry::iso8601_from_system_time;
use crate::seams::{DaemonError, HumanInput, ViewFrameSource};

/// The one bound of a human take: in-flight acting Cua calls, the pending
/// release ordering and the epoch confirmation. `EPOCH_CONFIRM_WAIT` is spent
/// inside it.
pub(crate) const TAKE_BOUND: Duration = Duration::from_secs(10);

/// A human lease ends after this long without a heartbeat from its tab.
pub(crate) const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default idle timeout of a human lease (no human input).
pub(crate) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Longest wait for the session's input channel to take one event.
pub(crate) const INPUT_SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest wait for the session loop to confirm an input epoch change: above
/// its 5 s write deadline plus the drain of its 16 queued commands.
pub(crate) const EPOCH_CONFIRM_WAIT: Duration = Duration::from_secs(6);

/// Override of [`EPOCH_CONFIRM_WAIT`] in milliseconds, for tests. It is
/// undocumented and has no CLI flag or config key.
pub(crate) const EPOCH_CONFIRM_ENV: &str = "RDPILOT_DAEMON_EPOCH_CONFIRM_MS";

/// The confirm wait for the override value `value` (the content of
/// [`EPOCH_CONFIRM_ENV`], if set): a millisecond count above zero, or
/// [`EPOCH_CONFIRM_WAIT`] when absent, unparsable or zero.
pub(crate) fn epoch_confirm_wait(value: Option<&str>) -> Duration {
    value
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(EPOCH_CONFIRM_WAIT, Duration::from_millis)
}

/// Lease ends remembered per session for loss notices.
const RECENT_ENDS: usize = 16;

/// Pointer events are fenced by frame geometry (except the release of a held
/// button); key events are not.
fn is_pointer(event: &RawInput) -> bool {
    !matches!(event, RawInput::Key { .. })
}

/// A key or button pressed by the human holder and not yet released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Held {
    Button(PointerButton),
    Key { code: u8, extended: bool },
}

impl Held {
    pub(crate) fn release(self) -> RawInput {
        match self {
            Held::Button(button) => RawInput::Button {
                button,
                down: false,
            },
            Held::Key { code, extended } => RawInput::Key {
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
    /// An acting Cua call was still in flight, or the epoch change was not
    /// confirmed, when [`TAKE_BOUND`] expired.
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

struct State {
    lease: Option<Lease>,
    /// The input epoch of the session. It advances at every controller
    /// change and never goes back.
    epoch: InputEpoch,
    /// Forwarded acting Cua calls without an answer.
    inflight: usize,
    /// Recently ended leases: id -> loss.
    ended: VecDeque<(String, Loss)>,
    /// The latest human activity (accepted input or a lease end).
    human_activity: Option<(Instant, SystemTime)>,
}

impl Default for State {
    fn default() -> Self {
        State {
            lease: None,
            epoch: InputEpoch::INITIAL,
            inflight: 0,
            ended: VecDeque::new(),
            human_activity: None,
        }
    }
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

/// The release obligation of one control change: a new input epoch for a
/// controller change, or the releases of a blur or a lease end without one.
/// Pass it to [`SessionControl::discharge`] before the next controller acts.
/// Until it is discharged (or dropped), agent admission and human takes of
/// the session wait.
#[must_use = "a transition's releases must be discharged"]
#[derive(Debug)]
pub(crate) struct Transition {
    release: Vec<Held>,
    /// The epoch `release` is sent with: the epoch when it was created.
    at: InputEpoch,
    /// The new input epoch of a controller change.
    epoch: Option<InputEpoch>,
    _pending: PendingGuard,
}

impl Transition {
    #[cfg(test)]
    pub(crate) fn releases(&self) -> usize {
        self.release.len()
    }

    #[cfg(test)]
    pub(crate) fn epoch(&self) -> Option<InputEpoch> {
        self.epoch
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
    /// Longest wait for the session loop to confirm an epoch change.
    confirm_wait: Duration,
}

impl SessionControl {
    pub(crate) fn new(
        session: &str,
        generation: u64,
        events: Arc<SessionEvents>,
        frame: Option<Arc<dyn ViewFrameSource>>,
        input: Option<Arc<dyn HumanInput>>,
        confirm_wait: Duration,
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
            confirm_wait,
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// A pending transition that releases `held` at the current epoch. Call
    /// it in the same critical section of the state mutex that changes the
    /// lease, so an admission check under that mutex sees the lease change
    /// and the pending release together.
    fn transition(&self, state: &State, held: impl IntoIterator<Item = Held>) -> Transition {
        self.pending.count.fetch_add(1, Ordering::AcqRel);
        Transition {
            release: held.into_iter().collect(),
            at: state.epoch,
            epoch: None,
            _pending: PendingGuard(Arc::clone(&self.pending)),
        }
    }

    /// A controller change: advance the input epoch and return the pending
    /// transition that sends the new epoch. The session loop releases every
    /// key and button of the old epoch. Call it in the critical section that
    /// changes the lease, like [`SessionControl::transition`].
    fn advance(&self, state: &mut State) -> Transition {
        state.epoch = state.epoch.next();
        let mut transition = self.transition(state, []);
        transition.epoch = Some(state.epoch);
        transition
    }

    /// The current input epoch, for an operation admitted without
    /// [`SessionControl::admit_agent`] (the bootstrap).
    pub(crate) fn input_epoch(&self) -> InputEpoch {
        self.state().epoch
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
    /// earlier lease end or change are sent, and returns the input epoch the
    /// action sends with.
    ///
    /// # Errors
    ///
    /// [`DaemonError::HumanControl`] naming the holder and the CLI command.
    pub(crate) async fn admit_agent(&self) -> Result<InputEpoch, DaemonError> {
        self.until_released(|| {
            let state = self.state();
            match &state.lease {
                Some(lease) => Some(Err(self.human_control_error(lease))),
                None => self.released().then_some(Ok(state.epoch)),
            }
        })
        .await
    }

    /// The error of an admitted agent action: input refused as stale while
    /// a human holds or takes the lease is that human's control; any other
    /// error is unchanged.
    pub(crate) fn stale_error(&self, error: DaemonError) -> DaemonError {
        if !matches!(error, DaemonError::Sdk(rdpilot::Error::StaleInput { .. })) {
            return error;
        }
        match &self.state().lease {
            Some(lease) => self.human_control_error(lease),
            None => error,
        }
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
        Some(self.end_lease(state, &lease))
    }

    /// The transition of the end of `lease`: a controller change when it
    /// was granted. A pending take holds nothing and the agent still
    /// controls, so its end changes nothing.
    fn end_lease(&self, state: &mut State, lease: &Lease) -> Transition {
        if lease.granted {
            self.advance(state)
        } else {
            self.transition(state, [])
        }
    }

    /// Take the human lease out of `state` for `reason` (daemon source),
    /// remember its loss and record its end when it was granted. `None` when
    /// no lease exists.
    fn take_human(&self, state: &mut State, reason: EndReason) -> Option<Lease> {
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
        Some(lease)
    }

    /// End a human lease for `reason` (daemon source). `None` when none.
    pub(crate) fn end_human(&self, reason: EndReason) -> Option<Transition> {
        let mut state = self.state();
        let lease = self.take_human(&mut state, reason)?;
        Some(self.end_lease(&mut state, &lease))
    }

    /// End a human lease because the session ended. The session loop is gone,
    /// so nothing is sent and no epoch change is waited for: the transition
    /// carries neither releases nor an epoch. `None` when no lease exists.
    pub(crate) fn end_human_ended(&self) -> Option<Transition> {
        let mut state = self.state();
        self.take_human(&mut state, EndReason::SessionEnded)?;
        Some(self.transition(&state, []))
    }

    /// Discharge `transition` within the confirm wait. Returns whether the
    /// session loop confirmed its epoch change; see
    /// [`SessionControl::discharge_within`].
    pub(crate) async fn discharge(&self, transition: Transition) -> bool {
        self.discharge_within(transition, self.confirm_wait).await
    }

    /// Send `transition` to the session. An epoch change is sent without the
    /// input lock (the session loop orders it after every earlier input) and
    /// waited for at most `wait`; it is confirmed when the loop has written
    /// its releases, and at once when the session has no input path. A closed
    /// session answers not confirmed at once. Releases without an epoch
    /// change are sent best effort, serialized with human input, and count as
    /// confirmed. The pending count of the transition ends when this returns.
    pub(crate) async fn discharge_within(&self, transition: Transition, wait: Duration) -> bool {
        if let Some(epoch) = transition.epoch {
            let Some(input) = &self.input else {
                return true;
            };
            return matches!(
                tokio::time::timeout(wait, input.set_epoch(epoch)).await,
                Ok(Ok(()))
            );
        }
        let _guard = self.input_lock.lock().await;
        self.send_releases(transition.release, transition.at).await;
        true
    }

    async fn send_releases(&self, release: Vec<Held>, epoch: InputEpoch) {
        if release.is_empty() {
            return;
        }
        if let Some(input) = &self.input {
            let events = release.into_iter().map(Held::release).collect();
            // Best effort: a closed or stalled session has no stuck key, and
            // releases of an older epoch are refused as stale (the epoch
            // change already released them).
            let _ = tokio::time::timeout(INPUT_SEND_TIMEOUT, input.send(events, epoch)).await;
        }
    }

    /// Take the lease for a viewer tab at `address`. From the agent, the
    /// take is marked at once and then waits for in-flight acting Cua calls
    /// and pending releases. From another tab, the old lease ends at once.
    /// Either way the take is granted only after the session loop confirmed
    /// the epoch change. Every wait shares one bound, [`TAKE_BOUND`].
    ///
    /// # Errors
    ///
    /// [`TakeError::Busy`] when an acting Cua call did not finish, or the
    /// epoch change was not confirmed, within [`TAKE_BOUND`];
    /// [`TakeError::Lost`] when displaced while waiting.
    pub(crate) async fn take(&self, address: IpAddr) -> Result<Grant, TakeError> {
        self.take_within(address, TAKE_BOUND).await
    }

    pub(crate) async fn take_within(
        &self,
        address: IpAddr,
        bound: Duration,
    ) -> Result<Grant, TakeError> {
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + bound;
        let lease = Lease {
            id: new_lease_id(),
            address,
            since_wall: SystemTime::now(),
            granted: false,
            last_heartbeat: started,
            last_input: started,
            held: BTreeSet::new(),
        };
        let id = lease.id.clone();
        let by = lease.controller();
        // The new lease is pending (never expired, displaceable) until the
        // epoch change is confirmed.
        let moved = {
            let mut state = self.state();
            match state.lease.replace(lease) {
                Some(old) if old.granted => {
                    // From another tab: no agent work can be in flight.
                    state.remember(old.id.clone(), Loss::by(by.clone()));
                    self.record(
                        EventSource::Viewer,
                        EventKind::ControlTakenOver {
                            from: old.controller(),
                            by,
                        },
                    );
                    Some(self.advance(&mut state))
                }
                pending => {
                    if let Some(pending) = pending {
                        state.remember(pending.id, Loss::by(by));
                    }
                    None
                }
            }
        };

        // A take abandoned while it waits (the tab went away) is undone.
        let mut pending = PendingTake {
            control: self,
            id: &id,
            armed: true,
        };
        let from_agent = moved.is_none();
        let confirmed = if let Some(transition) = moved {
            let confirmed = self
                .discharge_within(transition, self.confirm_wait.min(bound))
                .await;
            // Releases of the old holder still pending (a blur, a fence)
            // are sent before the new holder acts. Not sent within the bound:
            // busy, like an unconfirmed change.
            confirmed
                && tokio::time::timeout_at(deadline, self.wait_released())
                    .await
                    .is_ok()
        } else {
            let settled = tokio::time::timeout_at(deadline, async {
                self.wait_settled().await;
                self.wait_released().await;
            })
            .await;
            let transition = {
                let mut state = self.state();
                if !matches!(&state.lease, Some(current) if current.id == id) {
                    pending.armed = false;
                    return Err(lost_take(&state, &id));
                }
                if settled.is_err() {
                    pending.armed = false;
                    state.lease = None;
                    return Err(TakeError::Busy);
                }
                self.advance(&mut state)
            };
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            self.discharge_within(transition, self.confirm_wait.min(remaining))
                .await
        };
        pending.armed = false;

        let mut state = self.state();
        if !matches!(&state.lease, Some(current) if current.id == id) {
            return Err(lost_take(&state, &id));
        }
        if !confirmed {
            // The epoch stays advanced: the agent acts at the new epoch.
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
        if from_agent {
            self.record(
                EventSource::Viewer,
                EventKind::ControlTaken {
                    by: granted.controller(),
                },
            );
        }
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
        Ok(self.transition(&state, held))
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
        Ok(self.advance(&mut state))
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
    /// request sends with the input epoch of its first lease check; events
    /// the session loop refuses as stale (the lease ended after the check)
    /// are dropped as stale. The release of a held button is never fenced by
    /// geometry, and a fenced pointer event releases every held button. Runs
    /// of pointer moves are coalesced to the last one.
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
        events: Vec<RawInput>,
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
        let mut epoch = None;
        for event in events {
            remaining -= 1;
            let current = self.frame.as_ref().and_then(|frame| frame.geometry());
            let (fenced, at) = {
                let mut state = self.state();
                let at = *epoch.get_or_insert(state.epoch);
                let holder = match state.held(lease) {
                    Ok(holder) => holder,
                    Err(lost) => {
                        report.dropped.stale += remaining + 1;
                        report.lost = Some(lost);
                        return Ok(report);
                    }
                };
                let off_frame = is_pointer(&event)
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
                    (Some(buttons), at)
                } else {
                    track(&mut holder.held, &event);
                    let now = Instant::now();
                    holder.last_input = now;
                    holder.last_heartbeat = now;
                    state.touch_human();
                    (None, at)
                }
            };
            if let Some(buttons) = fenced {
                report.dropped.geometry += 1;
                self.send_releases(buttons, at).await;
                continue;
            }
            match tokio::time::timeout(INPUT_SEND_TIMEOUT, input.send(vec![event], at)).await {
                Ok(Ok(())) => report.applied += 1,
                Ok(Err(DaemonError::Sdk(rdpilot::Error::StaleInput { .. }))) => {
                    // The lease ended after its check: the loop refused
                    // this event and released what the old epoch held.
                    report.dropped.stale += remaining + 1;
                    report.lost = self.check_lease(lease).err();
                    return Ok(report);
                }
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

/// Why the pending take `id` is no longer the lease.
fn lost_take(state: &State, id: &str) -> TakeError {
    match state.loss_of(id) {
        NotHeld::Lost(loss) => TakeError::Lost(loss),
        NotHeld::Unknown => TakeError::Lost(Loss::by(ControllerRef::Agent)),
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

fn geometry_matches(event: &RawInput, aimed: (u32, u32), current: Option<(u32, u32)>) -> bool {
    if current != Some(aimed) {
        return false;
    }
    match *event {
        RawInput::PointerMove { x, y } => u32::from(x) < aimed.0 && u32::from(y) < aimed.1,
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
fn releases_held_button(held: &BTreeSet<Held>, event: &RawInput) -> bool {
    matches!(*event, RawInput::Button { button, down: false } if held.contains(&Held::Button(button)))
}

/// Track the keys and buttons `event` presses or releases.
pub(crate) fn track(held: &mut BTreeSet<Held>, event: &RawInput) {
    match *event {
        RawInput::Button { button, down } => {
            if down {
                held.insert(Held::Button(button));
            } else {
                held.remove(&Held::Button(button));
            }
        }
        RawInput::Key {
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
        RawInput::PointerMove { .. } | RawInput::Wheel { .. } => {}
    }
}

/// Keep only the last of each run of consecutive pointer moves.
fn coalesce(events: Vec<RawInput>) -> Vec<RawInput> {
    let mut out: Vec<RawInput> = Vec::with_capacity(events.len());
    for event in events {
        if let (Some(RawInput::PointerMove { .. }), RawInput::PointerMove { .. }) =
            (out.last(), event)
        {
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
