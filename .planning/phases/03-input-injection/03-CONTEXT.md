# Phase 3: Input Injection - Context

**Gathered:** 2026-07-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Mouse and keyboard actions are delivered to the remote session at the correct
coordinates, and the DPI coordinate contract is defined and enforced for all
future phases. Phase 2's `Session` handle gains input-injection capability;
no new crate is introduced.

**Requirements:** INPUT-01 (mouse actions: move, click variants, scroll,
drag), INPUT-02 (keyboard: type text and key combinations/modifiers).

**Success Criteria** (what must be TRUE):
1. A mouse click sent to a known remote coordinate activates the target
   element (e.g. clicking a Notepad menu opens it).
2. All mouse action types work: move, left/right/middle click, double-click,
   scroll, and drag.
3. Typed text and key combinations (e.g. Ctrl+A, Alt+F4) are received by the
   remote application.
4. The coordinate contract is documented and enforced: remote session is
   forced to 96 DPI (100%), and all coordinate values are in physical
   virtual-desktop pixels.

**Explicitly NOT in this phase** (belongs elsewhere — redirect any scope
creep):
- Setting/forcing 96 DPI on the VM — already done at the VM level by Phase 1.
  Phase 3 documents and *enforces* (bounds-checks) the contract in the SDK; it
  does not configure DPI.
- Computer-use vocabulary mapping (Anthropic/OpenAI action schema) — INPUT-01
  ultimately needs this, but this phase delivers the underlying
  `send_mouse`/`send_key` primitives; the vocabulary wrapper is a thin layer
  that can be added later on top without re-opening this phase's API.
- Horizontal scroll — deferred (see Deferred Ideas).
- Window enumeration / real window geometry → Phase 6.
- DVC sensor / structured perception → Phase 4+.

</domain>

<decisions>
## Implementation Decisions

### Public API shape
- **D-3.1:** Two owned enums + two methods on `Session`:
  `Session::send_mouse(MouseAction) -> Result<()>` and
  `Session::send_key(KeyAction) -> Result<()>`. Owned SDK types only
  (consistent with Phase 2 D-09) — `ironrdp`/`FastPathInputEvent` types never
  appear in the public API. Example call sites:
  ```rust
  session.send_mouse(MouseAction::Click { x, y, button: Button::Left }).await?;
  session.send_mouse(MouseAction::Scroll { x, y, dy }).await?;
  session.send_key(KeyAction::Type("hello".into())).await?;
  session.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A])).await?;
  ```
  A computer-use vocabulary wrapper (INPUT-01) maps onto these methods later
  — not built in this phase.

### Coordinate contract enforcement
- **D-3.2:** Add a public `Session::desktop_size() -> (u32, u32)` accessor
  that surfaces the negotiated desktop size (currently tracked internally in
  `connect.rs`/`session_loop.rs` but not exposed). Every mouse/click
  coordinate passed to `send_mouse` must be bounds-checked against this size
  and rejected with a typed `Error` **before** any PDU is constructed or
  sent — mirroring the `screenshot.rs` `crop_out_of_bounds` precedent
  (`Error::crop_out_of_bounds`). This satisfies success criterion #4
  ("enforced", not just documented). 96 DPI itself is already forced at the
  VM level by Phase 1; Phase 3 does not set DPI, it documents and enforces
  the resulting coordinate contract in the SDK.

### Scroll scope
- **D-3.3:** Vertical scroll only for v1. Scroll amount is expressed as a
  number of 120-unit notches (`WHEEL_DELTA`), matching Windows' standard
  wheel-delta convention. Horizontal scroll is deferred (add only if research
  finds it trivial to include). Matches ROADMAP success criterion #2 wording
  and research `FEATURES.md` TS-5.

### Live verification strategy
- **D-3.4:** Screenshot-diff verification. For each success criterion that
  can be observed visually (menu opens, dropdown appears, text renders),
  take a screenshot before and after the input action and assert a
  meaningful region changed. Extends Phase 2's frame-comparison helpers in
  `crates/rdpilot/tests/live_session.rs`. No dependency on Phase 6 (window
  list) or Phase 7 (UIA) — those don't exist yet. Reuse the gated live-test
  harness verbatim: `tests/common::load_config`, the `RDPILOT_LIVE` env-var
  gate, and `#[ignore]` so offline `cargo test` stays green.

### Keyboard PDU path (recommended default — confirm during research/plan)
- **D-3.5:** Split keyboard path by `KeyAction` variant:
  - `KeyAction::Type(text)` → `UnicodeKeyboardEvent` per character
    (layout-independent, correct for arbitrary/non-ASCII text).
  - `KeyAction::Combo(keys)` → scancode `KeyboardEvent` down/up sequences
    with correct modifier ordering: press modifiers → press key → release
    key → release modifiers (reverse order).
  - Rationale: the Unicode path cannot express held modifiers; the scancode
    path is required for combos like Ctrl+A / Alt+F4.

### Internal PDU construction strategy (recommended default)
- **D-3.6:** Hybrid approach — use `ironrdp-input`'s `Database`/`Operation`
  API for stateful real input (button-state and modifier-state tracking,
  prevents invalid PDU sequences, de-dups redundant moves), but continue
  hand-building `FastPathInputEvent` directly where `keepalive.rs` already
  established that pattern (keepalive's zero-delta move deliberately bypasses
  `Database` deduplication — see Reusable Assets). `ironrdp-input = "0.6"` is
  already pinned in `Cargo.toml` but currently unused.

### Double-click synthesis (recommended default)
- **D-3.7:** Synthesize double-click as two single-click sequences separated
  by a conservative hardcoded ~100ms inter-click delay. No configurability
  exposed for v1 (YAGNI) — RDP has no native double-click PDU.

### Drag synthesis (recommended default)
- **D-3.8:** `MouseAction` drag sends: button-down at `from` → ~5
  interpolated intermediate MOVE events → button-up at `to`. Many Win32/UWP
  controls require movement past a threshold to register a drag. Fixed small
  step count for v1 (no configurability).

### Claude's Discretion
The following recommended defaults (D-3.5 through D-3.8) were accepted by
the user as provisional, not individually debated — confirm exact shapes
during research/planning:
- Exact `ironrdp-pdu`/`ironrdp::pdu::input` enum field names and whether
  modifiers embed in the PDU or must be hand-sequenced (open unknown, see
  below).
- Exact inter-click delay tuning and drag step count/spacing, provided the
  live test criteria (menu opens, drag registers) pass.
- Internal error-type additions for coordinate-bounds and other input
  failures (style should mirror existing `Error` enum in `error.rs`).

</decisions>

<specifics>
## Specific Ideas

- Public API shape the user signed off on (verbatim intent):
  ```rust
  session.send_mouse(MouseAction::Click { x, y, button: Button::Left }).await?;
  session.send_mouse(MouseAction::Scroll { x, y, dy }).await?;
  session.send_key(KeyAction::Type("hello".into())).await?;
  session.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A])).await?;
  ```
- Clicking a Notepad menu (e.g. File menu) is the canonical live-verification
  target for success criterion #1 — matches Phase 1's provisioned test VM
  and Phase 2's precedent of using visible remote-only apps for sanity
  checks.

</specifics>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope & requirements
- `.planning/ROADMAP.md` — Phase 3 goal, 4 success criteria, requirement
  mapping (INPUT-01, INPUT-02).
- `.planning/REQUIREMENTS.md` — full text of INPUT-01 (mouse actions mapped
  to Anthropic/OpenAI computer-use vocabulary) and INPUT-02 (keyboard
  type/combo injection).
- `.planning/PROJECT.md` — locked stack decisions and project boundaries.
- `.planning/STATE.md` — accumulated decisions and current position.

### Research (locked tech & pitfalls)
- `.planning/research/ARCHITECTURE.md` §"Component 3: Input Injector" —
  absolute-only mouse coordinate space, virtual-desktop pixel contract.
- `.planning/research/ARCHITECTURE.md` §"Component 5: Public SDK API" —
  overall public-surface shape this phase extends.
- `.planning/research/FEATURES.md` TS-5 ("Mouse Input — Move, Click, Scroll,
  Drag") and TS-6 ("Keyboard Input — Type Text + Key Combos").
- `.planning/research/PITFALLS.md` Pitfall C4 (UIPI blocks input injection
  and UIA at higher integrity levels — relevant if the target app runs
  elevated) and Pitfall M1 (keepalive interval, already handled by Phase 2
  but shares the input-PDU code path).
- `.planning/research/STACK.md` — `ironrdp-input` 0.6.x crate purpose
  (`Database`/`Operation` stateful input construction).

### Phase 2 (session/API this phase extends)
- `.planning/phases/02-rdp-session-framebuffer-core/02-CONTEXT.md` — D-04
  (SDK owns session loop), D-09 (owned-SDK-types-only public API rule this
  phase must follow), D-13/D-14 (config/credential boundaries, unaffected by
  this phase).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/rdpilot/src/session_loop.rs` — the `tokio::select!` pump already
  has an `input_rx` arm; `RdpInputEvent` currently only has a `Close`
  variant, with an inline comment marking where the
  `FastPath(Vec<FastPathInputEvent>)` variant goes. Dispatch already flows
  through `active_stage.process_fastpath_input(&mut image, &events)`.
- `crates/rdpilot/src/session.rs` — `Session` owns
  `input_tx: mpsc::Sender<RdpInputEvent>` (capacity 16,
  `INPUT_CHANNEL_CAPACITY`). New `send_mouse`/`send_key` methods construct
  events and `.send()` on this same channel — no new plumbing required.
- `crates/rdpilot/src/keepalive.rs` — working precedent for hand-building
  `FastPathInputEvent::MouseEvent(MousePdu { .. })` directly; also documents
  that `ironrdp_input::Database` de-dups identical-position moves (a known
  gotcha to work around for real input, not just keepalive).
- `crates/rdpilot/src/screenshot.rs` — `crop()` returns
  `Result<Screenshot>` and never panics on out-of-bounds input
  (`Error::crop_out_of_bounds`); this is the direct precedent for D-3.2's
  coordinate bounds-checking.
- `crates/rdpilot/src/lib.rs` — the public surface is exactly `Session`,
  `ConnectionConfig`, `Screenshot`, `Rect`, `Error`, `Result` (D-09, Phase 2).
  New `MouseAction`/`KeyAction`/`Button`/`Key` types must be added to this
  `pub use` list; `ironrdp`/`image` internals must not leak.
- `crates/rdpilot/tests/common/mod.rs` and
  `crates/rdpilot/tests/live_session.rs` — the gated live-test harness
  (`RDPILOT_LIVE` env gate, `#[ignore]`) to extend for input-injection live
  verification (D-3.4).

### Established Patterns
- No `unwrap`/`expect`/`panic!` in library code (API-01, enforced since
  Phase 2) — coordinate-bounds and other input errors must return `Error`.
- Owned-SDK-types-only public API (D-09) — applies to all new input types.
- Live verification runs locally against the Phase-1 VM, gated so a default
  `cargo test` never hard-fails without a live target (D-17/D-18, Phase 2).

### Integration Points
- All new input methods hang off the existing `Session` handle and the
  existing `input_tx`/`input_rx` channel — no new channel, no new background
  task.
- `desktop_size()` (D-3.2) needs a value already computed during connect
  (negotiated desktop size); locate where `connect.rs`/`session_loop.rs`
  currently store this and surface it via a new accessor rather than
  recomputing it.

</code_context>

<deferred>
## Deferred Ideas

- **Horizontal scroll** — add only if research finds it trivial; otherwise
  stays out of scope beyond v1.
- **Computer-use action-vocabulary wrapper** (Anthropic/OpenAI schema
  mapping) — INPUT-01 eventually needs this, but it is a thin layer on top
  of `send_mouse`/`send_key` and is not required to satisfy this phase's
  4 success criteria.
- **Configurable double-click delay / drag step count** — hardcoded for v1
  (YAGNI); revisit if live testing shows the fixed values are unreliable.
- **Window enumeration / real window geometry** → Phase 6.
- **DVC sensor / structured perception** → Phase 4+.

</deferred>

---

*Phase: 03-input-injection*
*Context gathered: 2026-07-08*