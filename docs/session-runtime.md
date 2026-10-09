# Session runtime contract

This page defines the contract between the RDP session runtime and its
callers. Two changes implement it: the runtime change (a `Send` connect, one
lifecycle owner, operation classes) and the channel-host change (channels
registered at connect, a typed event stream). Both changes cite this page. The
page changes no public surface: the IPC wire values, the CLI, the REST and MCP
adapters and the recording manifest stay as they are.

The `rdpilot` crate API (`Session`, `ManagedSession`) is internal to the
workspace. Both changes may change it without a separate approval.

Code references use the form `crates/<crate>/src/<file>:<line>`. They are
pinned to commit `8f799dd` and describe the code before the changes. The
IronRDP upgrade lands first and can rename the APIs that the references name.
Re-check each reference against the code after the upgrade. The shapes in this
page do not depend on a crate version.

Terms:

- **Session lifecycle**: the one per-session owner of the state `connecting`,
  `live` and `ended`. It is not the daemon module `lifecycle.rs`, which holds
  the idle reaper (`crates/rdpilot-daemon/src/lifecycle.rs:195`).
- **Input epoch**: a counter of input authority within one live session. See
  [Ordered input](#ordered-input).
- **Operation class**: the scheduling class of a command or method. See
  [Loop commands](#loop-commands) and [Operation classes](#operation-classes).
- **Published state**: state that the session loop writes and any reader
  reads without a command.

## Session handle

`Session` is a `Send + Sync` handle. Its fields are already `Send`
(`crates/rdpilot/src/session.rs:33`). Only the connect future is not.

Connect spawns the session thread first. The thread builds a current-thread
runtime and runs the full handshake on it, then runs the session loop. Today
the handshake runs before the thread exists
(`crates/rdpilot/src/session.rs:87-101`), on the caller's runtime
(`crates/rdpilot/src/connect.rs:95`).

- The caller awaits a `Send` future. The future resolves to the live handle or
  to an error.
- Dropping or cancelling the future aborts the handshake on the session thread
  and leaves no session and no thread.
- The channel set is a connect parameter that comes from the configuration.
  The caller does not register a channel after connect.
- The connection configuration is `Clone`
  (`crates/rdpilot/src/config.rs:33`), so it moves to the thread.
- `FrameWatch` and `InputHandle` stay passive `Send` views.

Consequences:

- The daemon `LocalSet` goes (`crates/rdpilot-daemon/src/server.rs:5-19`,
  `crates/rdpilot-daemon/src/server.rs:160-163`).
- The non-`Send` `BoxFuture` goes
  (`crates/rdpilot-daemon/src/seams.rs:36-59`,
  `crates/rdpilot-daemon/src/seams.rs:60`). It also appears in `ManagedCua`
  (`crates/rdpilot-daemon/src/seams.rs:656-660`) and `BundleSource`
  (`crates/rdpilot-daemon/src/seams.rs:628-632`).
- `SessionConnector` and `ManagedSession` methods return `Send` futures.

## Loop commands

The session loop takes commands from one bounded queue. The command enum
replaces `RdpInputEvent`
(`crates/rdpilot/src/session_loop.rs:47-62`). The table maps each current
loop input to its class.

| Current loop input | Source | Class |
| --- | --- | --- |
| `RdpInputEvent::Close` | `crates/rdpilot/src/session.rs:290` | lifecycle |
| `RdpInputEvent::FastPath` | `crates/rdpilot/src/session.rs:201` | ordered input |
| `RdpInputEvent::Request` | `crates/rdpilot/src/bridge.rs:169` | bridge request |
| `bridge.shutdown` notification | `crates/rdpilot/src/session_loop.rs:101` | lifecycle |
| `bridge.retire_ready` notification | `crates/rdpilot/src/session_loop.rs:102` | bridge stream |
| `bridge.data_ready` notification | `crates/rdpilot/src/session_loop.rs:171` | bridge stream |
| bridge liveness tick (10 s) | `crates/rdpilot/src/session_loop.rs:105` | loop-internal timer |
| keepalive tick | `crates/rdpilot/src/session_loop.rs:174` | loop-internal timer |

A bridge stream input carries Cua traffic from a side queue
(`crates/rdpilot/src/bridge.rs:21`). It never passes through the command
queue. A loop-internal timer is not a command.

Classes:

- **Lifecycle**: close and cancel. See
  [Lifecycle commands](#lifecycle-commands).
- **Ordered input**: fast-path batches from the agent and the human through
  one FIFO queue, and resize. See [Ordered input](#ordered-input).
- **Bridge request**: a typed request on the bridge channel, with a
  caller-side timeout.
- **Transfer**: a file or clipboard-content move. The loop queues a transfer
  so that it never blocks a read, an input command or a takeover.
- **Channel command**: one variant shape per channel adapter. The
  channel-host change fills the variants for clipboard, devices and display
  control. See [Channel commands](#channel-commands).

Reads never use the command queue. A read (screenshot, frame, status,
geometry, bootstrap stages) reads published state without a lock that a
command holds.

### Lifecycle commands

A lifecycle command cannot be refused because the input queue is full. Today
`Close` uses `try_send` on a 16-slot queue and is lost when the queue is full
(`crates/rdpilot/src/session.rs:26`, `crates/rdpilot/src/session.rs:290`,
`crates/rdpilot/src/session.rs:372`). The shutdown notification is the prior
art: `stop` notifies first, and the loop handles the notification in its
first arm. The lifecycle path keeps that property.

### Ordered input

Every ordered-input command carries the input epoch. The daemon control
module advances the epoch when a controller takes the lease and when it
releases the lease. The Session only compares epochs. It does not know
principals.

1. The daemon captures the epoch once per admitted operation, at the lease
   check that admits it. An operation is one call of a method such as
   `send_mouse`, or one drag, double click or bootstrap sequence. Every
   batch of the operation carries that one epoch. The operation does not
   re-read the epoch between batches.
2. The loop compares the epoch of a command with the current epoch at
   dequeue.
3. The loop rejects a command with an older epoch and returns a typed stale
   error to the sender. The sender stops the operation at the first stale
   error and does not send the remaining batches.
4. Resize uses the same epoch check.
5. The loop acknowledges an epoch change in command order. The control
   module sends the new epoch to the loop as a command in the same queue as
   ordered input. It grants the takeover to the new controller after the loop
   confirms that command. The confirmation means that the loop has released
   the old input (see below) and that every earlier command is either sent to
   the server or rejected. A command of the new controller never reaches the
   loop before this confirmation.

The name "input epoch" is new. It is none of these existing counters:

- the registry incarnation `generation`
  (`crates/rdpilot-daemon/src/seams.rs:437`,
  `crates/rdpilot-daemon/src/control.rs:233`);
- the bridge `generation` (`crates/rdpilot/src/bridge.rs:41`);
- the Cua `runtime_generation`
  (`crates/rdpilot/src/bridge.rs:280`).

Two invariants keep the check correct.

**Input state follows accepted commands.** The press and release state
(`ironrdp_input::Database`, `crates/rdpilot/src/session.rs:37`) is shared by
the agent and the human. Today the caller applies it before the command is
queued (`crates/rdpilot/src/session.rs:192-198`,
`crates/rdpilot/src/session.rs:224-230`,
`crates/rdpilot/src/session.rs:345-351`). With a check at dequeue, a rejected
command would leave a press in the state that never reached the server. The
session loop therefore owns the state and applies a command at dequeue,
after the epoch check. The input state reflects only the commands that the
loop accepted. The rejected alternative is to compensate a rejected command
in the caller; it keeps two places that must agree.

**A change of epoch releases old input.** A take can cut an agent sequence
in the middle. A drag or a double click is several batches with pauses
(`crates/rdpilot/src/session.rs:180-216`). On an epoch change the loop
releases every key and button that was pressed under the old epoch before it
accepts the first command of the new epoch. This generalizes the discharge
order of the control module
(`crates/rdpilot-daemon/src/control.rs:8-16`), which releases only the
previous human holder's keys today.

### Cua calls

The input epoch does not cover Cua acting calls. A Cua call travels as a
bridge envelope and runs in the guest. The loop cannot recall a forwarded
call. The daemon keeps its Cua admission and its wait for in-flight calls
(`crates/rdpilot-daemon/src/control.rs:547`,
`crates/rdpilot-daemon/src/control.rs:773`).

The epoch covers fast-path input, resize and the lease-gated channel
commands. A take never waits behind a transfer. A take can wait, for a
bounded time, for acting Cua calls that are in flight.

### Channel commands

A channel command that changes the remote clipboard carries the input epoch.
A command that reads the remote clipboard also carries it, because a
clipboard read needs a channel request. The control lease covers all
clipboard operations. In the principal model clipboard operations are the
action class `transfer`, and the lease gates them (see
[`principal-model.md`](principal-model.md), sections "Action classes" and
"Control lease"). Every lease-gated channel command carries the epoch, so
this page and the principal model agree.

## Event stream

The stream types and semantics are fixed here. The channel-host change builds
the whole stream. The runtime change builds no stream.

One typed event enum has these topics:

| Topic | Content |
| --- | --- |
| lifecycle | `connecting`, `live`, `ended` with its cause |
| frame | dirty rectangles and a full snapshot |
| pointer | pointer shape and position |
| geometry | desktop size |
| clipboard | remote clipboard content and state |
| device | redirected device state |
| bridge | bridge milestones, such as the bootstrap stages |

Each topic has a delivery class:

- **Frame**: newest wins. A slow reader skips frames, and a drop counter
  records the skips. This is the behavior of today's `FrameWatch`
  (`crates/rdpilot/src/framebuffer.rs:42-44`).
- **Control topics** (all others): lossless, with a bounded queue. When the
  queue overflows, the subscriber receives an explicit overflow error and
  does not lose events silently.

Each topic has its own monotonic sequence number. A gap in the sequence is
visible to the subscriber.

The subscriber API:

- `subscribe` returns a snapshot first, then deltas. The snapshot carries,
  per topic, the sequence number of the last event that it includes. The
  first delta of a topic has the next sequence number. The stream registers
  the subscriber and takes the snapshot in one step, so no event falls
  between the snapshot and the first delta, and no delta repeats an event of
  the snapshot.
- A subscriber can request a resync. The stream then sends a new snapshot.
- Overflow recovery: when a control-topic queue overflows, the stream ends
  the delivery of that topic to that subscriber with the overflow error. The
  subscriber recovers by a resync, which sends a new snapshot and then
  deltas. The stream keeps no lost events for a subscriber.
  A frame subscriber needs no recovery step, because the frame topic skips
  frames and keeps the newest.
- A slow subscriber does not slow another subscriber or the session loop.
- The recorder and the viewer use the same API.

Today the session drops two kinds of output. Every graphics update copies the
whole image and discards the dirty region
(`crates/rdpilot/src/session_loop.rs:194-202`). The loop ignores pointer
output (`crates/rdpilot/src/session_loop.rs:219-221`). The stream carries
both.

The runtime change exposes the lifecycle state through a `watch` and keeps
`FrameWatch`. The stream later publishes the lifecycle topic as a view of the
same lifecycle owner, so the status has one source.

## Session lifecycle and status

One session lifecycle owner holds the state of a session:

- `connecting`: the handshake runs.
- `live`: the session loop runs.
- `ended(cause)`: the session loop has stopped. The cause is one of: the
  server ended the session, the client closed it, or an error occurred.

Today four views of the status exist, and none derives from an owner:

| View | Reference | Behavior today |
| --- | --- | --- |
| frame `ended` flag | `crates/rdpilot/src/session.rs:116`, `crates/rdpilot/src/session.rs:288`, `crates/rdpilot/src/session.rs:370` | set at thread end, close and drop; it does not separate server end from client close |
| registry `Live` status | `crates/rdpilot-daemon/src/seams.rs:438-445`, `crates/rdpilot-daemon/src/registry.rs:372` | fixed at insert |
| `describe()` | `crates/rdpilot-daemon/src/seams.rs:273-278` | always `Live` |
| `SessionEnded` event | `crates/rdpilot-daemon/src/events.rs:202` | emitted once |

All four become views of the lifecycle owner. The cause exists today only as
text (`crates/rdpilot/src/session.rs:117-120`,
`crates/rdpilot/src/session_loop.rs:216`). The owner keeps it as a value.

Mapping to the public `SessionLifecycle`
(`crates/rdpilot-ipc/src/response.rs:22-36`), with no wire change:

| Lifecycle state | `SessionLifecycle` |
| --- | --- |
| `connecting` | `Connecting` |
| `live` | `Live` |
| `ended` | `Disconnected` (the cause goes to events and logs) |

`Orphaned` stays a registry state that restart reconciliation produces
(`crates/rdpilot-daemon/src/seams.rs:387-406`). The daemon does not produce
`Reconnecting`.

Geometry: `desktop_size` and the bounds check of input return the current
geometry that the session loop publishes. They do not return the connect-time
value (`crates/rdpilot/src/session.rs:90`,
`crates/rdpilot/src/session.rs:138`,
`crates/rdpilot/src/session.rs:166-175`). A reactivation changes the size
inside the loop (`crates/rdpilot/src/session_loop.rs:203-213`), and a resize
publishes it too.

The `ended` transition is the one hook for the cleanup of a session that the
server ended. Today neither the closed record nor the removal of the registry
entry happens on a server end (`crates/rdpilot-daemon/src/registry.rs:467`,
`crates/rdpilot-daemon/src/lifecycle.rs:195`). The runtime change attaches
both to this transition. The time at which the registry removes an orphaned
entry is a retention policy and is outside this page.

## Operation classes

Each method of `Session`, `ManagedSession`, `CuaAttachment` and `ManagedCua`
has an operation class and, where it applies, a principal action class (see
[`principal-model.md`](principal-model.md), section "Action classes"). The
Session does not know principals. The principal policy stays in the principal
model.

| Method | Operation class | Principal class |
| --- | --- | --- |
| `Session::connect` | lifecycle | none (see the note below) |
| `Session::close`, `Drop for Session`, `ManagedSession::close` | lifecycle | `act` (not lease-gated) |
| `Session::screenshot`, `ManagedSession::screenshot` | read | `read` |
| `Session::frame_watch`, `ManagedSession::frame_source` | read | `read` |
| `Session::desktop_size`, `ManagedSession::desktop_size` | read | `read` |
| `Session::bootstrap_stages`, `ManagedSession::bootstrap_stages` | read | `read` |
| `Session::input_handle`, `ManagedSession::human_input` | read (returns a view) | `read` |
| `ManagedSession::describe` | read (a view of the lifecycle owner) | `read` |
| `Session::send_mouse`, `Session::send_key`, `ManagedSession::send_mouse`, `ManagedSession::send_key` | ordered input | `act` |
| `InputHandle::send` | ordered input | `act` |
| `Session::inject_bootstrap` (private) | ordered input | `act` |
| `Session::upload_file`, `Session::download_file`, `ManagedSession::upload_file`, `ManagedSession::download_file` | transfer | `transfer` |
| `Session::deploy_and_launch`, `ManagedSession::deploy_and_launch` | transfer | `transfer` |
| `Session::transfer` (private) | transfer | `transfer` |
| `Session::ping`, `ManagedSession::ping` | bridge request | `read` |
| `Session::attach_cua`, `ManagedSession::attach_cua` | bridge request | `read` (opening the stream changes nothing) |
| `CuaAttachment::send`, `CuaAttachment::recv`, `CuaAttachment::close`, `ManagedCua::identity`, `ManagedCua::send`, `ManagedCua::recv`, `ManagedCua::close` | bridge stream | `read`, `transfer` or `act` per call |

Close and drop are `act` but not lease-gated: any principal that may close a
session closes it, and the lease does not hold the close back, so that a
lifecycle command cannot be refused (see
[Lifecycle commands](#lifecycle-commands)).

`Session::connect` has no principal class. The daemon `Connect` verb carries
the class `credential-command`, because a connect with a password or by saved
host resolves a host credential. The classes `admin` and `credential-command`
have no Session method.

The daemon decides the principal class of a Cua call per call at the Cua gate
(`crates/rdpilot-daemon/src/ipc/cua_gate.rs:34-72`). The classes are `read`,
`transfer` (`clipboard_read`) and `act`. They match the `cua` rows of
`principal-model.toml`. The methods that carry the calls take the class of the call.

Statements:

- A human takeover never waits behind a transfer. The take does not wait for
  the per-session lock that a transfer holds today
  (`crates/rdpilot-daemon/src/registry.rs:554-616`,
  `crates/rdpilot-daemon/src/registry.rs:1035-1050`) and does not use the
  wait bound `TAKE_WAIT` (`crates/rdpilot-daemon/src/control.rs:48`). The
  one exception is the bootstrap at connect; see
  [Bootstrap and takeover](#bootstrap-and-takeover).
- A transfer that leaves the remote session writes to a destination that the
  caller supplies. The per-session staging area is daemon policy and is not
  Session state.
- A take can wait for acting Cua calls (see [Cua calls](#cua-calls)).

## Bootstrap and takeover

`deploy_and_launch` runs when `Connect` is handled. It runs after the registry
entry is `Live` (`crates/rdpilot-daemon/src/dispatch.rs:184-190`) and holds
the per-session lock. It injects the Run dialog keys through ordered input
(`crates/rdpilot/src/session.rs:431-452`). It waits `SESSION_SETTLE` (10 s)
and types at 150 ms per character
(`crates/rdpilot/src/session.rs:28`, `crates/rdpilot/src/session.rs:30`).

Today a take that arrives during the bootstrap waits on the per-session lock
(`crates/rdpilot-daemon/src/registry.rs:1035-1050`). After `TAKE_WAIT` the
daemon undoes the take with the refusal `TakeError::Busy`, and the bootstrap
continues (`crates/rdpilot-daemon/src/control.rs:217-226`). The message of
the refusal already names a long transfer.

With the epoch check, a take during the bootstrap would reject the launch
keys. The contract therefore states this rule:

- While the bootstrap runs, the daemon refuses a human take at once with the
  existing refusal `TakeError::Busy`. The take does not wait.
- `Connect` succeeds as it does today. `list` shows `Live` as it does today.
- The bootstrap keys carry the agent input epoch. The epoch cannot change
  during the bootstrap, because no take is granted during it.

The mechanism is a bootstrap-in-progress mark on the registry entry:

- The mark is set before the entry becomes `Live`. When a bundle is
  configured, the insert sets it. Today the entry is `Live` before
  `deploy_and_launch` starts
  (`crates/rdpilot-daemon/src/dispatch.rs:165-184`), and the take requires
  only `Live` (`crates/rdpilot-daemon/src/registry.rs:685-688`), so a mark
  set later leaves a window in which a take is granted.
- `deploy_and_launch` returning clears the mark, on success and on failure.
  Closing the entry clears it as well.
- The take checks the mark under the same registry guard that it uses to
  check `Live`. A set mark gives `TakeError::Busy` at once.
- The mark gates the take only. Reads, screenshots, the frame watch and
  `list` do not check it.

This rule is the one exception to "a take never waits behind a transfer". The
take does not queue; the refusal comes at once. The rule changes more than
the time of a refusal. Today a take that arrives during the bootstrap
succeeds when the bootstrap ends within `TAKE_WAIT`
(`crates/rdpilot-daemon/src/control.rs:725-749`). Under the rule that take
is refused at once and the caller must retry after the bootstrap. The rule
changes no wire value: the refusal and its message exist today.

Two alternatives need an owner approval, because each changes public
behavior:

- **(a)** The bootstrap counts as `connecting`. `list` then shows
  `Connecting` until the bootstrap ends. This changes the time at which
  `list` and `SessionLifecycle` show `Live`.
- **(b)** A take aborts the bootstrap, and `Connect` fails with a new failure
  cause. This adds a cause to the IPC `Connect` response.

Only the rule in this section depends on the choice.

### Cancellation

- Cancel during the handshake: the caller drops the connect future. The
  handshake stops on the session thread, and no session remains (see
  [Session handle](#session-handle)).
- Cancel during the bootstrap: the daemon closes the entry by its connect
  lease. This is what the daemon does today for a failed bootstrap
  (`crates/rdpilot-daemon/src/dispatch.rs:194-205`,
  `crates/rdpilot-daemon/src/registry.rs:500`). The rule above decides how a
  take interacts with it.

## File ownership and merge order

These files change only in the runtime and channel-host work, on one branch
at a time:

- `crates/rdpilot/src/connect.rs`
- `crates/rdpilot/src/session.rs`
- `crates/rdpilot/src/session_loop.rs`
- `crates/rdpilot/src/bridge.rs` (the sender of the command queue)
- `crates/rdpilot/src/rdpdr_backend.rs` (the channel-host change splits it)

The order is fixed:

1. The IronRDP upgrade lands first.
2. The runtime change is written against the upgraded API and lands next.
3. The channel-host change rebases on the runtime change and lands last.

No two branches edit these files in parallel.

## Requirements trace

Each requirement of the two changes maps to one section of this page.

| Requirement | Change | Section |
| --- | --- | --- |
| `Send` connect | runtime | Session handle |
| `LocalSet` and `BoxFuture` removal | runtime | Session handle |
| One status source | runtime | Session lifecycle and status |
| Operation classes | runtime | Loop commands, Operation classes |
| A take never waits behind a transfer | runtime | Operation classes, Bootstrap and takeover |
| Cancel while connecting, in the handshake and in the bootstrap | runtime | Session handle, Bootstrap and takeover |
| Hook for the retention of orphaned entries | runtime | Session lifecycle and status |
| Channels registered at connect from the configuration | channel host | Session handle, Loop commands |
| RDPDR split | channel host | File ownership and merge order |
| Typed stream with dirty rectangles and pointer | channel host | Event stream |
| Slow-reader isolation | channel host | Event stream |
| Delivery classes | channel host | Event stream |
| Per-topic sequence with visible gaps | channel host | Event stream |
| Snapshot then deltas, and resync | channel host | Event stream |
| Recorder and viewer on one API | channel host | Event stream |
| Resize with epoch, lease and expected geometry | resize work | Ordered input, Session lifecycle and status |

No pair of requirements conflicts:

- The files that both changes edit serialize through the merge order.
- The lifecycle topic of the stream is a view of the lifecycle owner, so the
  runtime change and the stream do not own two statuses.
- The channel registration is a connect parameter, so it does not conflict
  with the `Send` connect.

## Reference index

All references are pinned to commit `8f799dd`.

| Subject | Reference |
| --- | --- |
| Connect before thread spawn | `crates/rdpilot/src/session.rs:87-101` |
| Connect-time desktop size | `crates/rdpilot/src/session.rs:90`, `crates/rdpilot/src/session.rs:138` |
| Frame `ended` set | `crates/rdpilot/src/session.rs:116`, `crates/rdpilot/src/session.rs:288`, `crates/rdpilot/src/session.rs:370` |
| Loop commands | `crates/rdpilot/src/session_loop.rs:47-62` |
| Loop `run` signature | `crates/rdpilot/src/session_loop.rs:71-77` |
| Loop `select!` arms | `crates/rdpilot/src/session_loop.rs:99-180` |
| Bundle check at connect | `crates/rdpilot/src/connect.rs:130` |
| `LocalSet` | `crates/rdpilot-daemon/src/server.rs:5-19`, `crates/rdpilot-daemon/src/server.rs:160-163` |
| `BoxFuture` | `crates/rdpilot-daemon/src/seams.rs:36-59`, `crates/rdpilot-daemon/src/seams.rs:60` |
| `describe()` | `crates/rdpilot-daemon/src/seams.rs:273-278` |
| Registry status | `crates/rdpilot-daemon/src/seams.rs:438-445`, `crates/rdpilot-daemon/src/registry.rs:372` |
| Take wait | `crates/rdpilot-daemon/src/registry.rs:1035-1050`, `crates/rdpilot-daemon/src/control.rs:48`, `crates/rdpilot-daemon/src/control.rs:725-749` |
| Entry `Live` before bootstrap | `crates/rdpilot-daemon/src/dispatch.rs:165-184` |
| Take requires `Live` | `crates/rdpilot-daemon/src/registry.rs:685-688` |
| Viewer reads `ended` | `crates/rdpilot-daemon/src/registry.rs:1118-1122` |
| Session-ended event | `crates/rdpilot-daemon/src/events.rs:202` |
| Public lifecycle enum | `crates/rdpilot-ipc/src/response.rs:22-36` |
| Cua gate | `crates/rdpilot-daemon/src/ipc/cua_gate.rs:34-72` |
