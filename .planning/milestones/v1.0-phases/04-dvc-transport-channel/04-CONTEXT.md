# Phase 4: DVC Transport Channel - Context

**Gathered:** 2026-07-08
**Status:** Ready for planning

<domain>
## Phase Boundary

The `RDPILOT_SENSOR` dynamic virtual channel is open and bidirectional by the
time the RDP session is fully established, confirmed by a ping/heartbeat
round-trip — before any sensor modules exist.

**Requirement:** SENSOR-03 ("A DVC request/response transport channel carries
structured-perception data between the SDK and the sensor").

**Success Criteria** (what must be TRUE):
1. `DvcProcessor` is registered with `DrdynvcClient` before
   `connector.connect()` completes (IronRDP hard constraint — no channel can
   be added after the session is established).
2. A ping sent over `RDPILOT_SENSOR` returns a pong from the server-side
   endpoint within 500 ms.
3. A version handshake is the first message on the channel; a mismatch
   closes the channel with a clear error (not silent corruption).

**Explicitly NOT in this phase** (belongs elsewhere — redirect scope creep):
- Real sensor modules (window list, process tree, UIA) — Phases 6-7.
- The sensor's implementation language (C# NativeAOT vs. all-Rust) — stays
  DEFERRED per PROJECT.md; not decided here.
- Real sensor bootstrap/deployment mechanism (RDPDR drive redirection,
  WinRM, blob+SAS) — Phase 5 (SENSOR-01/SENSOR-02).
- Anything beyond Ping/Pong/Version message types — the envelope is designed
  to be durable/extensible, but WindowList/ProcessTree/Uia payloads are not
  implemented until Phases 6-7.

</domain>

<decisions>
## Implementation Decisions

### Ping-validation strategy
- **D-4.1:** A minimal, disposable server-side responder is deployed to the
  live Azure VM (from Phase 1). It opens `RDPILOT_SENSOR` server-side,
  answers pong, and participates in the version handshake. It is discarded
  before Phase 5's real sensor exists. This is the real end-to-end live gate
  for success criteria 2 and 3.
  - Rejected: local `ironrdp-dvc-pipe-proxy` loopback — does not prove
    real-RDP transport.
  - Rejected: deferring proof to Phase 5 — leaves SENSOR-03 unproven on its
    own merits.

### Sensor language (unchanged — not decided here)
- **D-4.2:** The C# .NET 8 NativeAOT vs. all-Rust sensor decision remains
  DEFERRED per PROJECT.md. This phase's throwaway responder is written in
  whatever is cheapest and must NOT prejudice the Phase 5 choice.
  Recommended: PowerShell, since WinRM + `pwsh` are already present on the
  target from Phase 1.

### Protocol scope
- **D-4.3:** Design a durable request/response envelope now —
  `{ version, req_id, type, payload }` (`req_id` is a correlation id
  reserved for later concurrent queries) — but implement ONLY the Version,
  Ping, and Pong message types in this phase. Phases 6-7 add
  WindowList/ProcessTree/Uia message types without reworking framing. Reuse
  `ironrdp-dvc`'s chunking/reassembly (`encode_dvc_messages`,
  DataFirst/Data PDUs) rather than hand-rolling framing.

### Encoding
- **D-4.4:** JSON via `serde`/`serde_json`. Promote `serde` + `serde_json`
  from dev-dependencies to runtime `[dependencies]` and add the `derive`
  feature. Payloads are JSON — human-debuggable and trivially extensible
  for later structured UIA/window data.
  - Rejected: compact binary encoding — smaller frames but more
    hand-written codec work for structured payloads later.

### Sensor delivery direction (spans Phase 4 and Phase 5 / SENSOR-02)
- **D-4.5:**
  - **This phase (throwaway responder):** WinRM (`Copy-Item -ToSession` or
    inline base64 write) + `Start-Process`. Already reachable from Phase 1;
    zero new Rust crates. The DVC channel is the thing under test here, so
    delivery must not also exercise unproven RDPDR plumbing.
  - **Phase 5 (real sensor, SENSOR-02) — recorded for that phase's
    planning, not decided here:** RDPDR client-side drive redirection is
    PRIMARY (`ironrdp-rdpdr` 0.6.0 — client-side filesystem device
    redirection; the `-native` backend being *nix-only is a non-issue
    because our client runs on Linux). Bootstrap pattern proven by prior
    art agent-rdp: map a drive, drop the binary, launch via injected Win+R
    keystroke against `\\tsclient\...`. WinRM = fallback. Blob+SAS
    in-guest download = tertiary (reuses `manage-env.ps1` mechanism; note
    it is the ONLY delivery path that sets Mark-of-the-Web / Internet
    zone — relevant because a future Azure Trusted Signing Authenticode
    signature neutralizes SmartScreen "unknown publisher" friction exactly
    there).
  - CLIPRDR clipboard file-copy is DROPPED for Phase 5 delivery.
    Technically supported by `ironrdp-cliprdr` 0.6.0
    (`FileGroupDescriptorW`) but strictly worse: requires driving a
    fragile remote-side GUI paste vs. RDPDR's stable path reference, with
    no signing/MOTW advantage.
  - Azure Trusted Signing note (for Phase 5): Authenticode signing affects
    AV/SmartScreen identically regardless of transport, EXCEPT the
    blob/SAS path (which sets MOTW) — carry this forward to Phase 5
    planning.

### Claude's Discretion
- Exact throwaway-responder script structure/naming (as long as it opens
  the DVC channel server-side and answers pong + version handshake).
- Internal `Error::Dvc(String)` message wording, provided it mirrors the
  existing `Error` enum's style (see Existing Code Insights).
- Exact reply-channel design for the outbound ping (see Open Questions —
  this is an open research item, not a locked decision).

</decisions>

<specifics>
## Specific Ideas

- No specific UX/behavioral references beyond the success criteria above —
  this is an infrastructure/transport phase with no user-facing surface.
- The version-handshake requirement is explicit: "first message on the
  channel", and a mismatch must produce "a clear error (not silent data
  corruption)" — this framing (from ROADMAP.md success criterion 3) should
  drive the error-handling design directly.

</specifics>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope & requirements
- `.planning/ROADMAP.md` §"Phase 4: DVC Transport Channel" — goal, 3 success
  criteria, requirement mapping (SENSOR-03).
- `.planning/REQUIREMENTS.md` — full text of SENSOR-03.
- `.planning/PROJECT.md` — locked stack decisions; sensor-language decision
  explicitly deferred here.
- `.planning/STATE.md` — accumulated decisions and current position.

### Research (locked tech & pitfalls)
- `.planning/research/ARCHITECTURE.md` §"Component 4: Sensor Client
  (local)" and §"Transport: Recommended — RDP Dynamic Virtual Channel
  (DVC)" — DVC mechanics, `DrdynvcClient`/`DvcProcessor` registration
  constraint (must happen before `connector.connect()` completes — HIGH
  confidence, hard IronRDP constraint), request/response protocol shape.
- `.planning/research/ARCHITECTURE.md` §"Anti-Pattern 2: Per-Window DVC
  Channels" — one channel, request-ID-tagged multiplexing (informs the
  `req_id` field in D-4.3's envelope).
- `.planning/research/PITFALLS.md` Pitfall M4 ("RDP Virtual Channel
  Bootstrap — Trust and Version Skew") — version handshake as first
  message is the documented mitigation; directly drives D-4.3 and success
  criterion 3.
- `.planning/research/PITFALLS.md` Pitfall m3 ("Credentials in Process
  Memory and Channel Leakage") — DVC is unauthenticated transport by
  default; relevant context for the throwaway responder even though
  app-layer auth is out of scope for this phase.
- `.planning/research/STACK.md` §"Virtual Channel / Sensor Transport" —
  `ironrdp-dvc` 0.14.x / (pinned `0.6` in this repo's `Cargo.toml`) crate
  purpose, `DvcProcessor` trait shape (`channel_name()`, `start()`,
  `process()`, `close()`), `ironrdp-dvc-pipe-proxy` (confirmed NOT needed
  for this phase per D-4.1).
- `.planning/research/FEATURES.md` TS-10 ("Sensor Helper Bootstrap +
  Transport") — DVC preferred in-band, WinRM as fallback; matches D-4.5.

### Phase 2 (session/API this phase extends)
- `.planning/phases/02-rdp-session-framebuffer-core/02-CONTEXT.md` — D-04
  (SDK owns session loop, extended here with the new outbound-ping
  dispatch arm), D-09 (owned-SDK-types-only public API rule this phase's
  `Session::ping()` must follow).

### Phase 3 (immediate predecessor — patterns to mirror)
- `.planning/phases/03-input-injection/03-CONTEXT.md` — precedent for
  extending `Session`/`session_loop.rs` with new capability without new
  plumbing; precedent for `Error` enum additions (D-3.2's
  `coordinate_out_of_bounds` style); precedent for gated live-test
  additions to `tests/live_session.rs` (D-3.4).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/rdpilot/src/connect.rs:87-96` — the empty DVC registration seam:
  `DrdynvcClient::new()` at :95, `.with_static_channel(...)` at :96, and a
  placeholder `let _ = RDPILOT_SENSOR;` at :94, with the exact recipe
  already written as a comment at :87-93. Must become
  `DrdynvcClient::new().with_dynamic_channel(RdpilotSensorProcessor::new())`
  before `connect_begin` (:98). Reserved channel-name const at
  `connect.rs:44` (`RDPILOT_SENSOR`).
- `crates/rdpilot/src/error.rs` — `Error::CoordinateOutOfBounds` (added in
  Phase 3) is the direct precedent for adding `Error::Dvc(String)`: variant
  + `#[error(...)]` message + `pub(crate)` constructor (:101-133) + a new
  exhaustive `category()` arm (:135-149). Third-party errors are
  source-erased to `String` (established pattern).
- `crates/rdpilot/src/session.rs:172-249` — `send_mouse`/`send_key` shape
  (lock/apply/drop-guard-before-await) is the pattern `Session::ping()`
  should mirror, extended with a response-await path.
- `crates/rdpilot/src/session_loop.rs:44-51,78-124` — the `tokio::select!`
  pump and `RdpInputEvent::{Close,FastPath}` enum. Currently fire-and-forget
  with no ack path (confirmed by `tests/live_session.rs:299`). A new
  `RdpInputEvent::Ping`-style variant plus a new DVC-send dispatch arm in
  the `select!` is required — see Open Questions for the reply-channel
  design.
- `crates/rdpilot/tests/live_session.rs` — gated live-test harness
  (`require_target!`, `RDPILOT_LIVE` env gate, `#[ignore]`, `block_on`
  bridge) used by the Phase 3 screenshot-diff tests; the pattern to mirror
  for a new `sensor_ping_pong_under_500ms` test.

### Established Patterns
- No `unwrap`/`expect`/`panic!` in library code — `Error::Dvc` must be
  returned, not panicked, on version mismatch or timeout.
- Owned-SDK-types-only public API (D-09, Phase 2) — `Session::ping()`
  return type must not leak `ironrdp`/`ironrdp-dvc` types.
- Gated live verification (`RDPILOT_LIVE`, `#[ignore]`) so default
  `cargo test` stays green without a live target.

### Integration Points
- `RdpilotSensorProcessor` (new, implements `ironrdp-dvc`'s `DvcProcessor`)
  is registered in `connect.rs` and owns `start()`/`process()`/`close()`.
  `start()` must send the version handshake as the very first outbound
  message.
- `Cargo.toml:44` — `serde`/`serde_json` currently dev-only; promote to
  runtime `[dependencies]` per D-4.4. `ironrdp-dvc = "0.6"` already
  present; `ironrdp-dvc-pipe-proxy` is NOT needed (per D-4.1, throwaway-VM
  choice instead of local loopback proxy).

</code_context>

<deferred>
## Deferred Ideas

- **Sensor implementation language** (C# .NET 8 NativeAOT vs. all-Rust) —
  Phase 5. This phase's throwaway responder must not prejudice that choice.
- **Real sensor bootstrap/deployment** (RDPDR drive redirection primary,
  WinRM fallback, blob+SAS tertiary) — Phase 5 (SENSOR-01/SENSOR-02).
- **CLIPRDR clipboard file-copy delivery** — dropped entirely, not just
  deferred; recorded so it isn't re-proposed in Phase 5 planning.
- **Azure Trusted Signing / Authenticode** — Phase 5 concern; the MOTW
  interaction with the blob/SAS path is noted here only so Phase 5
  planning has the context.
- **WindowList / ProcessTree / Uia message types** — Phases 6-7. The
  envelope (D-4.3) is designed to accommodate them without reworking
  framing, but they are not implemented here.
- **Application-layer channel authentication** (per PITFALLS.md Pitfall
  m3) — not addressed in this phase; DVC remains unauthenticated transport
  for now.

</deferred>

---

## Settled — do not re-litigate

- **DVC registration mechanism & site** (before `connect_begin`, via
  `.with_dynamic_channel(...)`) — locked in Phase 2; this phase implements
  the seam already reserved there.
- **RDP library = IronRDP; `ironrdp-dvc` is the DVC crate** — locked in
  Phase 2 / PROJECT.md.

## Open Questions (for researcher/planner — NOT resolved here)

1. **Outbound reply-channel design for `Session::ping()`.** No
   caller→DVC send path currently exists (the session-loop mpsc is
   fire-and-forget). Candidate designs: (a) a `oneshot` sender carried
   inside a new `RdpInputEvent::Ping(oneshot::Sender<Pong>)` variant, or
   (b) a shared `Notify`/state object the `DvcProcessor` updates on
   `process()` and the caller awaits separately. Wrap the round-trip in
   `tokio::time::timeout` for the 500 ms bound (success criterion 2).
2. **Version-handshake wire format specifics.** The envelope's `version`
   field is a protocol-version integer; a mismatch must map to
   `Error::Dvc` and close the channel. Exact serialization details (which
   side sends first, exact JSON shape) are for research/planning to pin
   down.
3. **How the throwaway responder opens the server-side DVC channel.**
   `WTSVirtualChannelOpenEx` must run in-session. Confirm whether
   PowerShell can open a DVC via `Add-Type`/P-Invoke (agent-rdp does DVC
   from a PowerShell agent, so it appears feasible) or whether a tiny
   compiled helper is required instead. This is the top research question
   for the throwaway responder (D-4.1/D-4.2).

---

*Phase: 04-dvc-transport-channel*
*Context gathered: 2026-07-08*
