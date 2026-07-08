# Phase 3: Input Injection - Research

**Researched:** 2026-07-08
**Domain:** RDP fast-path input PDU construction (mouse + keyboard) via IronRDP, coordinate-contract enforcement, stateful input dedup
**Confidence:** HIGH (exact enum shapes, bitflag values, and encode/decode logic verified against the raw GitHub source at the precise pinned Cargo.lock tags — not against latest/master)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Public API shape**
- **D-3.1:** Two owned enums + two methods on `Session`: `Session::send_mouse(MouseAction) -> Result<()>` and `Session::send_key(KeyAction) -> Result<()>`. Owned SDK types only (consistent with Phase 2 D-09) — `ironrdp`/`FastPathInputEvent` types never appear in the public API. Example call sites:
  ```rust
  session.send_mouse(MouseAction::Click { x, y, button: Button::Left }).await?;
  session.send_mouse(MouseAction::Scroll { x, y, dy }).await?;
  session.send_key(KeyAction::Type("hello".into())).await?;
  session.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A])).await?;
  ```
  A computer-use vocabulary wrapper (INPUT-01) maps onto these methods later — not built in this phase.

**Coordinate contract enforcement**
- **D-3.2:** Add a public `Session::desktop_size() -> (u32, u32)` accessor that surfaces the negotiated desktop size (currently tracked internally in `connect.rs`/`session_loop.rs` but not exposed). Every mouse/click coordinate passed to `send_mouse` must be bounds-checked against this size and rejected with a typed `Error` **before** any PDU is constructed or sent — mirroring the `screenshot.rs` `crop_out_of_bounds` precedent (`Error::crop_out_of_bounds`). This satisfies success criterion #4 ("enforced", not just documented). 96 DPI itself is already forced at the VM level by Phase 1; Phase 3 does not set DPI, it documents and enforces the resulting coordinate contract in the SDK.

**Scroll scope**
- **D-3.3:** Vertical scroll only for v1. Scroll amount is expressed as a number of 120-unit notches (`WHEEL_DELTA`), matching Windows' standard wheel-delta convention. Horizontal scroll is deferred (add only if research finds it trivial to include). Matches ROADMAP success criterion #2 wording and research `FEATURES.md` TS-5.

**Live verification strategy**
- **D-3.4:** Screenshot-diff verification. For each success criterion that can be observed visually (menu opens, dropdown appears, text renders), take a screenshot before and after the input action and assert a meaningful region changed. Extends Phase 2's frame-comparison helpers in `crates/rdpilot/tests/live_session.rs`. No dependency on Phase 6 (window list) or Phase 7 (UIA) — those don't exist yet. Reuse the gated live-test harness verbatim: `tests/common::load_config`, the `RDPILOT_LIVE` env-var gate, and `#[ignore]` so offline `cargo test` stays green.

**Keyboard PDU path (recommended default — confirmed during research)**
- **D-3.5:** Split keyboard path by `KeyAction` variant:
  - `KeyAction::Type(text)` → `UnicodeKeyboardEvent` per character (layout-independent, correct for arbitrary/non-ASCII text).
  - `KeyAction::Combo(keys)` → scancode `KeyboardEvent` down/up sequences with correct modifier ordering: press modifiers → press key → release key → release modifiers (reverse order).
  - Rationale: the Unicode path cannot express held modifiers; the scancode path is required for combos like Ctrl+A / Alt+F4.
  - **Confirmed by research:** `ironrdp-input::Database::apply()` implements exactly this split already (see Code Examples) — `Operation::UnicodeKeyPressed(char)`/`UnicodeKeyReleased(char)` for the Unicode path, `Operation::KeyPressed(Scancode)`/`KeyReleased(Scancode)` for the scancode path. No hand-rolled PDU construction needed for either.

**Internal PDU construction strategy (recommended default — confirmed)**
- **D-3.6:** Hybrid approach — use `ironrdp-input`'s `Database`/`Operation` API for stateful real input (button-state and modifier-state tracking, prevents invalid PDU sequences, de-dups redundant moves), but continue hand-building `FastPathInputEvent` directly where `keepalive.rs` already established that pattern (keepalive's zero-delta move deliberately bypasses `Database` deduplication — see Reusable Assets). `ironrdp-input = "0.6"` is already pinned in `Cargo.toml` but currently unused. **Confirmed:** `Database::apply` skips emitting a `MouseMove` event when the position is unchanged (source-verified, see Code Examples) — this is exactly why `keepalive.rs` bypasses `Database` for its zero-delta move.

**Double-click synthesis (recommended default)**
- **D-3.7:** Synthesize double-click as two single-click sequences separated by a conservative hardcoded ~100ms inter-click delay. No configurability exposed for v1 (YAGNI) — RDP has no native double-click PDU.

**Drag synthesis (recommended default)**
- **D-3.8:** `MouseAction` drag sends: button-down at `from` → ~5 interpolated intermediate MOVE events → button-up at `to`. Many Win32/UWP controls require movement past a threshold to register a drag. Fixed small step count for v1 (no configurability).

### Claude's Discretion
The following recommended defaults (D-3.5 through D-3.8) were accepted by the user as provisional, not individually debated — confirmed during research:
- Exact `ironrdp-pdu`/`ironrdp::pdu::input` enum field names and whether modifiers embed in the PDU or must be hand-sequenced — **fully resolved below, see "Resolved Open Unknowns."**
- Exact inter-click delay tuning and drag step count/spacing, provided the live test criteria (menu opens, drag registers) pass — **the 100ms/500ms and 5-step values are within safe Windows-documented bounds (see Common Pitfalls), but final tuning is a live-verify item.**
- Internal error-type additions for coordinate-bounds and other input failures (style should mirror existing `Error` enum in `error.rs`) — **a concrete variant shape is proposed in Code Examples.**

### Deferred Ideas (OUT OF SCOPE)
- **Horizontal scroll** — add only if research finds it trivial; otherwise stays out of scope beyond v1. (Research finding: trivial to *support* via `Operation::WheelRotations { is_vertical: false, .. }`, but out of scope per explicit D-3.3 lock — do not add without a scope change.)
- **Computer-use action-vocabulary wrapper** (Anthropic/OpenAI schema mapping) — INPUT-01 eventually needs this, but it is a thin layer on top of `send_mouse`/`send_key` and is not required to satisfy this phase's 4 success criteria.
- **Configurable double-click delay / drag step count** — hardcoded for v1 (YAGNI); revisit if live testing shows the fixed values are unreliable.
- **Window enumeration / real window geometry** → Phase 6.
- **DVC sensor / structured perception** → Phase 4+.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| INPUT-01 | User can inject mouse actions (move, click variants, scroll, drag) at remote coordinates, mapped to the Anthropic/OpenAI computer-use action vocabulary | `ironrdp-input::Database`/`Operation` fully covers move/click/drag/scroll at the PDU-construction level (verified source, see Code Examples); vocabulary mapping is explicitly deferred to a later thin wrapper (not required for this phase's success criteria) |
| INPUT-02 | User can inject keyboard input (type text and key combinations/modifiers) | `Operation::UnicodeKeyPressed`/`KeyPressed`+`Scancode` fully covers both paths (verified source); Common Pitfalls documents the `KeyboardFlags` type collision and the scancode table this phase must hand-build |
</phase_requirements>

## Summary

Everything this phase needs already exists in the dependency graph — no `Cargo.toml` change is required. `ironrdp-input = "0.6.0"` is pinned but unused (Phase 2 built the seam, not the consumer), and `ironrdp`'s `"input"` feature is already enabled, re-exporting `ironrdp_pdu::input` at `ironrdp::pdu::input` (proven working today by `keepalive.rs`). The exact pinned versions were verified by fetching the raw GitHub source at the precise tags matching `Cargo.lock` (`ironrdp-pdu-v0.8.0`, `ironrdp-input-v0.6.0`) — not against latest/master, which can and does drift (training-data recollection of these APIs would have been describing a plausibly-different current-master shape).

The central architectural finding, not spelled out in CONTEXT.md, is **where the stateful `ironrdp_input::Database` must live**. It cannot live inside `session_loop.rs`'s `tokio::select!` pump: that loop must never sleep (the drag-interpolation and double-click delays would starve PDU reads and the keepalive tick for the delay duration). It must live in `Session`, guarded by a plain `std::sync::Mutex` locked only for the synchronous `apply()` call (never held across an `.await`). `Session::send_mouse`/`send_key` builds `Operation`s, locks the mutex, calls `Database::apply()` to get a `Vec<FastPathInputEvent>`, drops the guard, and sends the batch into the *already-existing* `input_tx` channel using the *already-anticipated* `RdpInputEvent::FastPath(Vec<FastPathInputEvent>)` variant (the exact variant name and shape session_loop.rs's own doc comment predicted in Phase 2). Any inter-event timing (the double-click gap, the drag interpolation spacing) happens as multiple discrete `input_tx.send()` calls interleaved with `tokio::time::sleep` **on the caller's async context**, never inside the loop thread.

The single most concrete, previously-undocumented bug risk this research surfaces: `MousePdu::encode()` casts `number_of_wheel_rotation_units` to `u8` with `as u8` — a **silent truncating wraparound**, not a clamp. A caller-supplied scroll magnitude of `360` (3 Windows notches) does not error and does not clamp to 255 — it wraps to `104` on the wire, producing a materially wrong (and wrong-feeling) scroll amount with no error surfaced anywhere. `send_mouse` must reject (or internally split into multiple PDUs) any single `Scroll` magnitude whose absolute value exceeds `255` before construction.

**Primary recommendation:** Add `Session::desktop_size()` and bounds-checking in `Session::send_mouse`/`send_key` (new `Error::CoordinateOutOfBounds` variant), thread a `Mutex<ironrdp_input::Database>` into `Session`, add the `RdpInputEvent::FastPath` variant to `session_loop.rs` exactly as its own comment anticipates, and build `MouseAction`/`KeyAction`/`Button`/`Key` as new owned types in a new `src/input.rs` module — reusing `ironrdp-input::Operation` end-to-end rather than hand-building `FastPathInputEvent`s (that hand-built path stays reserved for `keepalive.rs`'s deliberate `Database`-bypass).

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Mouse/keyboard action API (`send_mouse`/`send_key`) | API / Backend (local SDK) | — | Public SDK surface; no browser/frontend tier exists in this project — the "backend" here is the local Rust SDK process |
| Coordinate bounds enforcement | API / Backend (local SDK) | — | Must happen in the caller's async context (`Session::send_mouse`), before any PDU touches the wire, per D-3.2 |
| Stateful input tracking (`ironrdp_input::Database`) | API / Backend (local SDK) | — | Lives in `Session` (Mutex-guarded), not in the session loop — see Summary |
| PDU dispatch / wire write | Database / Storage analogue → here: the session-loop transport thread | — | `session_loop.rs`'s existing `tokio::select!` pump; receives pre-built `FastPathInputEvent`s and forwards to `active_stage.process_fastpath_input`, no construction logic of its own |
| Remote rendering / target application response | Remote Windows target (out of this SDK's tiers) | — | Verified only via screenshot-diff live tests (D-3.4); not directly observable from the SDK's own state |

*(This project has no browser/CDN tier — it is a local Rust SDK controlling a remote desktop over RDP, so the standard web-app tier table collapses to "local SDK" vs. "remote target.")*

## Resolved Open Unknowns

Everything CONTEXT.md flagged as an open unknown for research is resolved here, cited against the exact pinned-version source.

### 1. `FastPathInputEvent` exact enum shape (ironrdp-pdu 0.8.0)

`[VERIFIED: github.com/Devolutions/IronRDP @ tag ironrdp-pdu-v0.8.0, crates/ironrdp-pdu/src/input/fast_path.rs]`

```rust
pub enum FastPathInputEvent {
    KeyboardEvent(KeyboardFlags, u8),        // scancode, 1 byte
    UnicodeKeyboardEvent(KeyboardFlags, u16), // UTF-16 code unit
    MouseEvent(MousePdu),
    MouseEventEx(MouseXPdu),   // extra mouse buttons (X1/X2) — not needed this phase
    MouseEventRel(MouseRelPdu), // relative mode — NOT used (RDP mouse is absolute-only)
    QoeEvent(u32),
    SyncEvent(SynchronizeFlags), // lock-key sync (caps/num/scroll/kana) — not required for INPUT-01/02
}

pub struct MousePdu {
    pub flags: PointerFlags,
    pub number_of_wheel_rotation_units: i16,
    pub x_position: u16,
    pub y_position: u16,
}
```

**Critical disambiguation — two different `KeyboardFlags` types exist in `ironrdp-pdu`:**
- `ironrdp_pdu::input::fast_path::KeyboardFlags` (8-bit; used by `FastPathInputEvent` — **this is the one this phase must import**):
  ```rust
  pub struct KeyboardFlags: u8 {
      const RELEASE = 0x01;
      const EXTENDED = 0x02;
      const EXTENDED1 = 0x04;
  }
  ```
  No explicit "DOWN" flag exists — **absence of `RELEASE` means key-down**, presence means key-up.
- `ironrdp_pdu::input::scan_code::KeyboardFlags` (16-bit; belongs to the *slow-path* `ScanCodePdu`/`InputEventPdu` — **not used anywhere in this SDK**, which only ever uses the fast-path input channel). Importing the wrong one is a plausible copy-paste mistake (both are named identically) that will either fail to compile against `FastPathInputEvent::KeyboardEvent` or, worse, compile against a different fast-path-adjacent call site with silently wrong flag values. Flagged explicitly in Common Pitfalls.

`PointerFlags` (`ironrdp_pdu::input::mouse`, already imported by `keepalive.rs`):
```rust
pub struct PointerFlags: u16 {
    const WHEEL_NEGATIVE = 0x0100;
    const VERTICAL_WHEEL = 0x0200;
    const HORIZONTAL_WHEEL = 0x0400;
    const MOVE = 0x0800;
    const LEFT_BUTTON = 0x1000;
    const RIGHT_BUTTON = 0x2000;
    const MIDDLE_BUTTON_OR_WHEEL = 0x4000;
    const DOWN = 0x8000;
}
```
Note: there is no separate `MIDDLE_BUTTON` constant — the middle button and the wheel-button share `MIDDLE_BUTTON_OR_WHEEL`; `ironrdp-input`'s own `MouseButton::Middle → PointerFlags::MIDDLE_BUTTON_OR_WHEEL` mapping (see below) already handles this correctly.

### 2. `ironrdp-input::Database`/`Operation` API (0.6.0) — full state-tracking behavior

`[VERIFIED: github.com/Devolutions/IronRDP @ tag ironrdp-input-v0.6.0, crates/ironrdp-input/src/lib.rs — full source read]`

```rust
pub enum MouseButton { Left = 0, Middle = 1, Right = 2, X1 = 3, X2 = 4 }
pub struct Scancode { /* opaque; code: u8, extended: bool */ }
impl Scancode {
    pub const fn from_u8(extended: bool, code: u8) -> Self;
    pub const fn from_u16(scancode: u16) -> Self; // extended auto-detected from 0xE000 mask
}
pub struct MousePosition { pub x: u16, pub y: u16 }
pub struct WheelRotations { pub is_vertical: bool, pub rotation_units: i16 }

pub enum Operation {
    MouseButtonPressed(MouseButton),
    MouseButtonReleased(MouseButton),
    MouseMove(MousePosition),
    WheelRotations(WheelRotations),
    KeyPressed(Scancode),
    KeyReleased(Scancode),
    UnicodeKeyPressed(char),
    UnicodeKeyReleased(char),
}

pub struct Database { /* tracks: 512-bit keyboard bitmap, 5-bit mouse-button bitmap, last mouse position, pressed-unicode-char set */ }
impl Database {
    pub fn new() -> Self;
    pub fn apply(&mut self, ops: impl IntoIterator<Item = Operation>) -> SmallVec<[FastPathInputEvent; 2]>;
    pub fn release_all(&mut self) -> SmallVec<[FastPathInputEvent; 2]>; // releases every tracked key/button — useful safety net, not required by D-3.1..8 but cheap to wire into `Session::close`/`Drop` if desired (discretionary)
}
```

**Confirmed state-tracking behavior (answers the CONTEXT.md open unknown directly):**
- `MouseMove`: **de-dups** — no event emitted if the position is unchanged from the tracked state. This is exactly why `keepalive.rs` bypasses `Database` for its zero-delta keepalive move (its own doc comment already says this; now source-confirmed to be `Database`'s real behavior, not folklore).
- `MouseButtonPressed`/`Released`: also state-gated — pressing an already-pressed button, or releasing an already-released one, emits nothing.
- `KeyPressed`: if the scancode is *already* tracked as pressed, `Database` auto-synthesizes a `RELEASE` event before re-emitting the press (auto key-repeat handling) — the caller does not need to manually release before re-pressing the same key.
- `KeyReleased`: only emits if the scancode was tracked as pressed; releasing an untracked key is a no-op.
- Modifiers embed **as separate down/up PDU sequences**, not as bits inside a single PDU — confirming D-3.5's "press modifiers → press key → release key → release modifiers" design is the *only* correct way to express a combo; there is no single-PDU "modified key" shape.
- `WheelRotations` unconditionally emits (no dedup) — every scroll `Operation` becomes exactly one `MouseEvent(MousePdu{flags: VERTICAL_WHEEL|HORIZONTAL_WHEEL, number_of_wheel_rotation_units: rotation_units, ..})`, at the *last tracked* mouse position (not a caller-supplied `x, y` at the PDU level — see Common Pitfalls for what this means for `MouseAction::Scroll { x, y, dy }`).

### 3. Scroll sign convention and WHEEL_DELTA=120 mapping

`[VERIFIED: github.com/Devolutions/IronRDP @ ironrdp-pdu-v0.8.0, crates/ironrdp-pdu/src/input/mouse.rs — full encode()/decode() read]`

```rust
// encode():
let wheel_negative_bit = if self.number_of_wheel_rotation_units < 0 {
    PointerFlags::WHEEL_NEGATIVE.bits()
} else { PointerFlags::empty().bits() };
let truncated_wheel_rotation_units = self.number_of_wheel_rotation_units as u8; // ← SILENT WRAP, see Pitfalls
let wheel_rotations_bits = u16::from(truncated_wheel_rotation_units);
let flags = self.flags.bits() | wheel_negative_bit | wheel_rotations_bits;
```

- **Sign is fully automatic** — the caller sets a signed `i16` (`number_of_wheel_rotation_units` on `MousePdu`, or `rotation_units` on `ironrdp_input::WheelRotations`); the `WHEEL_NEGATIVE` flag bit is derived from the sign at encode time. The caller must NOT set `WHEEL_NEGATIVE` manually.
- **Convention:** positive = away from user (scroll up / content moves down), matching `[CITED: learn.microsoft.com/windows/win32/inputdev/wm-mousewheel]` `WM_MOUSEWHEEL`'s documented sign (positive = forward/away from the user). `WHEEL_DELTA = 120` is Microsoft's documented constant `[CITED: learn.microsoft.com, devblogs.microsoft.com/oldnewthing/20130123-00]` — one notch = ±120, matching D-3.3 exactly.
- **Magnitude is packed into the low 8 bits of the same 16-bit wire word as the flags** — see Common Pitfalls for the resulting hard ceiling of ±255 per single PDU.

### 4. Live-verify-only items (cannot be resolved statically — schedule into the gated live suite, not static code assumptions)

- **Windows double-click time threshold on the actual VM.** `GetDoubleClickTime()`'s system default is **500 ms** `[CITED: learn.microsoft.com/windows/win32/api/winuser/nf-winuser-getdoubleclicktime]`, and D-3.7's hardcoded ~100ms inter-click delay sits comfortably inside that default window — but the *actual* value on the Phase-1-provisioned VM has never been queried or overridden, and some hardened/accessibility images change it. **This must be verified empirically during live execution** (does a 100ms-spaced double-click reliably register in the target app, e.g. Notepad or Explorer double-clicking to open/select?), not assumed from the documented default.
- **Whether the target app needs N intermediate drag MOVE events, and what spacing.** D-3.8's "~5 interpolated intermediate MOVE events" is a reasonable, commonly-cited starting point for drag-threshold detection in Win32/UWP controls, but no authoritative spec value exists — different controls (list-box multi-select drag vs. a slider thumb vs. a window resize handle) have different internal drag-start thresholds (Windows' `SM_CXDRAG`/`SM_CYDRAG`, default 4px, is a *distance* threshold, not an event-count threshold — meaning what matters is that consecutive MOVE deltas exceed ~4px each, not the total event count). **This must be validated empirically against whatever drag-capable control the live suite actually exercises** — recommend the live test log a decision point: if 5 steps at even spacing across the drag distance doesn't visibly register, increase step count before tuning delay.
- Both items are D-3.7/D-3.8's own "Claude's Discretion" carve-out ("provided the live test criteria... pass") — this research confirms there is no static answer to derive; the planner should schedule a dedicated live-verify task/checkpoint for each rather than hardcoding a value and calling it done from static reasoning alone.

## Standard Stack

### Core (no new dependencies — all already pinned in `Cargo.toml`, confirmed against `Cargo.lock`)

| Library | Locked Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ironrdp-input` | 0.6.0 `[VERIFIED: Cargo.lock]` | `Database`/`Operation` stateful input construction | Official IronRDP crate, purpose-built for exactly this problem; already pinned since Phase 1 planning, unused until now |
| `ironrdp-pdu` | 0.8.0 `[VERIFIED: Cargo.lock]` | `FastPathInputEvent`, `MousePdu`, `PointerFlags`, `KeyboardFlags` (fast-path variant) | Already a direct dependency; `keepalive.rs` already imports from it |
| `ironrdp` (umbrella) | 0.15.0 `[VERIFIED: Cargo.lock]`, `"input"` feature already enabled | Re-exports `ironrdp_pdu` at `ironrdp::pdu` | Already proven working (`keepalive.rs`, `session_loop.rs`) |

**Installation:** none required — no `Cargo.toml` edit for this phase.

**Version verification note:** Context7's indexed docs for `ironrdp-input` describe the *current* API (matches what was found at the pinned 0.6.0 tag in every particular checked: `Database`, `Operation`, `MouseButton`, `MousePosition`, `Scancode::from_u8`, `WheelRotations`, `synchronize_event`). All API details in this document were independently re-verified by fetching the raw source from the exact `Cargo.lock`-matching git tags (`ironrdp-pdu-v0.8.0`, `ironrdp-input-v0.6.0`) rather than relying on Context7/docs.rs summaries alone, because docs.rs index pages do not always render full struct/enum bodies and a 0.14-vs-0.6 version-drift trap already exists in this repo's own `research/STACK.md` (see State of the Art below).

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `ironrdp-input::Database` for stateful tracking | Hand-build every `FastPathInputEvent` directly (as `keepalive.rs` does) | Loses automatic dedup, button/modifier-state tracking, and auto key-repeat handling; `keepalive.rs` deliberately bypasses `Database` for one specific, narrow reason (dedup would suppress its zero-delta move) — that reason does not apply to real user-driven input, which should always go through `Database` |
| Hand-built scancode table | `pc-keyboard`, `ps2`, or `scancode` crates (found via search) | **Rejected** — these crates decode *incoming* PS/2 hardware scancode streams into logical keys (the opposite direction), built for `no_std` embedded OS kernels reading a physical keyboard controller. None map "named virtual key → outgoing Set-1 scancode byte for RDP injection." A hand-built `match`/table over the ~40–60 keys needed for computer-use (letters, digits, modifiers, function keys, navigation, Enter/Esc/Tab/Space/Backspace/Delete, arrows) against the public, decades-stable IBM PC/AT Scan Code Set 1 standard is the correct, low-risk choice — see Don't Hand-Roll |

## Package Legitimacy Audit

No new external packages are introduced by this phase — `ironrdp-input` was already vetted and pinned during Phase 1/2 planning (see `.planning/phases/02-.../02-RESEARCH.md`), and no new crate is added to `Cargo.toml`. Package legitimacy re-audit is not required; the existing `Cargo.lock` pin (`ironrdp-input 0.6.0`, `ironrdp-pdu 0.8.0`) is reused as-is.

| Package | Registry | Disposition |
|---------|----------|-------------|
| `ironrdp-input` | crates.io | Already approved (Phase 2 audit); no change this phase |
| `ironrdp-pdu` | crates.io | Already approved (Phase 2 audit); no change this phase |

## Architecture Patterns

### System Architecture Diagram

```
Session::send_mouse(MouseAction) / send_key(KeyAction)     [caller's async context, e.g. test/example]
        │
        ▼
1. bounds-check (x,y) against self.desktop_size()  ──fail──▶ Err(Error::CoordinateOutOfBounds) [no PDU built]
        │ pass
        ▼
2. build Vec<ironrdp_input::Operation> for the action
   (Click: [MouseMove, ButtonPressed, ButtonReleased])
   (DoubleClick: two Click sequences, tokio::time::sleep(~100ms) between the two `input_tx.send()` calls)
   (Drag: [ButtonPressed(from)] → N × [MouseMove(interpolated)] → [ButtonReleased(to)], one input_tx.send() per step)
   (Scroll: [WheelRotations{ is_vertical: true, rotation_units: dy }])
   (Type(text): per-char [UnicodeKeyPressed(c), UnicodeKeyReleased(c)])
   (Combo(keys): [KeyPressed(mod1), KeyPressed(mod2), KeyPressed(key), KeyReleased(key), KeyReleased(mod2), KeyReleased(mod1)])
        │
        ▼
3. lock self.input_db: Mutex<ironrdp_input::Database>  (brief lock, never held across .await)
   let events: Vec<FastPathInputEvent> = input_db.apply(ops);
   drop(guard)
        │
        ▼
4. input_tx.send(RdpInputEvent::FastPath(events)).await   ── existing mpsc channel, existing capacity-16 backpressure
        │
        ▼  (crosses to the dedicated session-loop OS thread — session.rs / session_loop.rs boundary)
tokio::select! loop (session_loop.rs) — event = input_rx.recv()
        │
        ▼
5. active_stage.process_fastpath_input(&mut image, &events)   ── existing dispatch, ALREADY WIRED for RdpInputEvent::Close;
        │                                                         only the match arm + enum variant are new
        ▼
6. ActiveStageOutput::ResponseFrame(bytes) → writer.write_all(bytes)   ── PDU hits the wire, unchanged existing path
        │
        ▼
[Remote Windows target] reacts (menu opens / text appears / drag registers)
        │
        ▼
7. Live test: session.screenshot() before/after → diff assertion (D-3.4)   ── the ONLY observable proof; no ack path exists
```

**Load-bearing property this diagram makes explicit:** step 2's timing (`tokio::time::sleep`) happens **before** step 4, on the caller's runtime — never inside the `tokio::select!` loop at step 5. See Common Pitfalls for why this matters.

### Recommended Project Structure

```
crates/rdpilot/src/
├── input.rs         # NEW — MouseAction, KeyAction, Button, Key enums (owned SDK types);
│                     #       scancode table (Key → ironrdp_input::Scancode);
│                     #       pure functions: MouseAction -> Vec<Operation>, KeyAction -> Vec<Operation>
├── session.rs        # MODIFIED — add `input_db: std::sync::Mutex<ironrdp_input::Database>` field,
│                     #             `desktop_size: (u32, u32)` field, `send_mouse`/`send_key`/`desktop_size()` methods
├── session_loop.rs    # MODIFIED — add `RdpInputEvent::FastPath(Vec<FastPathInputEvent>)` variant + one select! arm
├── connect.rs         # unaffected — desktop_size already available on ConnectionResult before it moves into the loop thread
├── error.rs           # MODIFIED — add `Error::CoordinateOutOfBounds { .. }` (mirrors `CropOutOfBounds` shape exactly)
├── lib.rs             # MODIFIED — `pub use input::{MouseAction, KeyAction, Button, Key};`
```

### Pattern 1: Extracting `desktop_size` before it moves into the session-loop thread

**What:** `Session::connect` currently does `let (connection_result, framed) = connect::connect(cfg).await?;` and then moves `connection_result` wholesale into the spawned thread's closure (`session_loop::run(framed, connection_result, ...)`). `connection_result.desktop_size` (a `DesktopSize { width: u16, height: u16 }`, confirmed by its use in `session_loop.rs`'s `DecodedImage::new(.., connection_result.desktop_size.width, connection_result.desktop_size.height)`) must be copied out into a local **before** that move, since `Session` needs it after the move has happened.

**When to use:** Exactly once, in `Session::connect`, right after the `connect::connect` call.

**Example:**
```rust
// crates/rdpilot/src/session.rs — inside Session::connect, before the thread::Builder::spawn call:
let (connection_result, framed) = connect::connect(cfg).await?;
let desktop_size = (
    u32::from(connection_result.desktop_size.width),
    u32::from(connection_result.desktop_size.height),
); // widening cast, always lossless (u16 -> u32)
// ... existing thread spawn, moving `connection_result` as today ...
Ok(Session { thread: Some(thread), input_tx, frame, input_db: Mutex::new(Database::new()), desktop_size })
```

**Open design question (flag for planner, not a blocker):** `session_loop.rs::reactivate()` rebuilds `image` at a *new* `desktop_size` on a server-driven `DeactivateAll`/resize event, but the value captured at connect time above would go stale if that happens mid-session. Phase 1 fixed the VM's resolution and Phase 3's live tests do not exercise a mid-session resize, so a static connect-time capture is *sufficient for this phase's success criteria*, but is a known latent staleness gap. If the planner wants correctness-robustness now rather than deferring, the fix is to replace the plain `(u32,u32)` field with a small `Arc<(AtomicU32, AtomicU32)>` written both at connect time and inside `reactivate()`. Recommend documenting the static-capture choice explicitly in code comments either way, so it is a deliberate decision, not an oversight.

### Pattern 2: `Session::send_mouse` — bounds-check, translate, apply, dispatch

**Example (Click):**
```rust
// crates/rdpilot/src/session.rs
pub async fn send_mouse(&self, action: MouseAction) -> Result<()> {
    for (x, y) in action.coordinates() { // MouseAction exposes every (x,y) pair it touches
        let (w, h) = self.desktop_size;
        if u32::from(x) >= w || u32::from(y) >= h {
            return Err(Error::coordinate_out_of_bounds(x, y, w, h));
        }
    }
    for ops_batch in action.into_operation_batches() { // Vec<Vec<Operation>>; >1 batch only for DoubleClick/Drag (needs inter-batch delay)
        let events = {
            let mut db = self.input_db.lock().expect("input_db mutex poisoned");
            db.apply(ops_batch)
        }; // guard dropped here — never held across the next .await
        self.input_tx.send(RdpInputEvent::FastPath(events.into_vec()))
            .await
            .map_err(|_| Error::Session("input channel closed".to_owned()))?;
        // MouseAction's batching iterator is responsible for the D-3.7/D-3.8 sleep
        // between batches — see Pattern 3.
    }
    Ok(())
}
```

**When to use:** All mouse actions route through this one method; only `action.into_operation_batches()` differs per variant (single batch for Move/Click/Scroll, two batches with a sleep for DoubleClick, N batches with per-step interpolation for Drag).

### Pattern 3: Timing lives on the caller's context, never inside `session_loop`'s `select!`

**What:** `session_loop.rs::run`'s `tokio::select!` loop is also responsible for reading inbound PDUs and firing the keepalive tick. Any `tokio::time::sleep` placed inside a `select!` arm blocks *only that arm's future*, not the other arms — but constructing the double-click delay as "one `RdpInputEvent::FastPath` message, then `sleep(100ms)` inside the loop before processing the second half" would still be architecturally wrong, because it serializes the delay onto the single-threaded loop's only worker, delaying every other `select!` branch's fairness for that duration and adding avoidable jitter to keepalive timing.

**When to use:** Always — this is a hard architectural rule for this phase, not a stylistic preference.

**Anti-pattern to avoid:** Sending one `RdpInputEvent::FastPath` containing *all* of a double-click's or drag's events pre-computed with no gap, hoping the remote target infers timing from PDU arrival order alone. RDP fast-path PDUs carry no client-side timestamp the server trusts for double-click detection — the server's `GetDoubleClickTime()` measurement is wall-clock, driven by when each PDU is *received*. Batching everything into one message sent instantaneously would make the "double" click arrive as two clicks with ~0ms gap, which Windows may or may not still recognize as a double-click depending on driver/session timing jitter — sending the two halves as separate `input_tx.send()` calls with a real `tokio::time::sleep` between them on the caller's context is the only reliable way to honor D-3.7's stated 100ms gap.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Mouse button/keyboard modifier state tracking, redundant-move dedup | A custom `HashMap<Button,bool>` / bitset tracker | `ironrdp_input::Database` | Already implements exactly this, source-verified (dedup, auto key-repeat release, tracked state accessors); reinventing it risks missing the same edge cases IronRDP already handles |
| Wire-format PDU construction for mouse/keyboard events | Hand-built `FastPathInputEvent`/`MousePdu`/`KeyboardFlags` literals for every real input action | `Database::apply(operations)` | `keepalive.rs`'s hand-built pattern exists for one narrow, deliberate reason (bypassing dedup for a zero-delta move) — that reason does not generalize to real click/type/drag input, where dedup and state tracking are exactly what's wanted |
| Scan Code Set 1 mapping for logical keys → RDP scancodes | Reaching for `pc-keyboard`/`ps2`/`scancode` crates | A small hand-written `match`/table (this is the *correct* place to hand-roll — no available crate solves this direction of the problem) | Those crates decode *incoming* hardware scancode streams for embedded OS kernels — the opposite direction from "encode an outgoing scancode for a named key." No maintained crate provides "key name → Set 1 byte for RDP injection"; the mapping itself is a decades-stable public standard, not complex logic, so hand-writing it carries none of the "don't hand-roll" risk (no edge cases to get wrong beyond typo-checking the table against a reference) |
| Coordinate bounds checking | A `panic!`/`unwrap()` on out-of-range coordinates, or a silent clamp | `Error::CoordinateOutOfBounds` returned before PDU construction, mirroring `Error::CropOutOfBounds` | Matches the established Phase 2 precedent exactly (API-01: no `unwrap`/`expect`/`panic!` in library code); a silent clamp would violate D-3.2's "enforced" wording by masking caller bugs |

**Key insight:** The entire mouse/keyboard PDU-construction problem is already solved by `ironrdp-input`, pinned but unused since Phase 1/2. The only genuinely new code this phase writes is: (1) the owned `MouseAction`/`KeyAction` → `Vec<Operation>` translation, (2) the scancode table for named keys, (3) the bounds-check, and (4) the `Mutex<Database>`-in-`Session` + `RdpInputEvent::FastPath` wiring. Everything else is composition of already-vetted, already-pinned library code.

## Common Pitfalls

### Pitfall 1: Wheel-rotation magnitude silently wraps past ±255 (not clamped, not errored)

**What goes wrong:** `MousePdu::encode()` does `let truncated_wheel_rotation_units = self.number_of_wheel_rotation_units as u8;` — a Rust `as` numeric cast, which truncates/wraps rather than saturating. A caller passing `dy = 360` (3 Windows notches, a perfectly reasonable "scroll 3 clicks" request under D-3.3's 120-unit-notch convention) produces `360 as u8 == 104` on the wire — a materially different, wrong scroll amount, with **no error, no panic, no log line** anywhere in the call chain.

**Why it happens:** The wire format packs the wheel magnitude into the low 8 bits of the same 16-bit word that carries the `PointerFlags`, so only 8 bits of magnitude exist on the wire regardless of the ergonomic `i16` Rust-level field. `[VERIFIED: raw source of MousePdu::encode/decode, ironrdp-pdu-v0.8.0]`

**How to avoid:** In `send_mouse`'s `Scroll` handling, reject (return `Err`) or internally split any `|dy| > 255` into multiple sequential `WheelRotations` operations/PDUs, each within range. Given D-3.3's "notches" framing (120-unit multiples), the simplest safe rule is: **cap or split at 2 notches (240) per PDU** — a caller who wants a 5-notch scroll gets 3 PDUs (240 + 240... no — 5×120=600; split as e.g. 240+240+120), not one wrapped PDU.

**Warning signs:** Live test observes a scroll landing in the wrong direction or wrong distance specifically for multi-notch scroll amounts, while single-notch (120) scrolls work fine.

### Pitfall 2: Two identically-named `KeyboardFlags` types — importing the wrong one

**What goes wrong:** `ironrdp_pdu::input::scan_code::KeyboardFlags` (16-bit, slow-path) and `ironrdp_pdu::input::fast_path::KeyboardFlags` (8-bit, fast-path) are two distinct types with the same name in different modules. This SDK only ever uses the fast-path input channel (matching `session_loop.rs`'s `process_fastpath_input` dispatch) — only `fast_path::KeyboardFlags` is ever correct here. `ironrdp_input::Database` already imports the correct one internally (confirmed: `use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};`), so as long as this phase's code goes *through* `Database`/`Operation` rather than hand-building `KeyboardEvent`s directly, this pitfall is structurally avoided. It only resurfaces if someone hand-builds a `FastPathInputEvent::KeyboardEvent` directly (as `keepalive.rs` does for mouse, but this phase should not do for keyboard) and imports from the wrong module path.

**How to avoid:** Route all keyboard construction through `ironrdp_input::Operation`/`Database::apply`, never hand-build `KeyboardEvent`/`UnicodeKeyboardEvent` directly.

### Pitfall 3: Sleeping inside `session_loop.rs`'s `tokio::select!` loop

See Architecture Pattern 3 above — this is the single most important non-obvious architectural constraint this research surfaces. Any implementation that threads the double-click/drag timing delay into the session-loop thread (rather than the caller's context) will work in a simple manual test but degrade keepalive timing and PDU-read latency under load, and is a much harder bug to notice than a compile error.

### Pitfall 4: UIPI blocks input injection into elevated processes (Pitfall C4, project research)

`[CITED: .planning/research/PITFALLS.md Pitfall C4]` — if the live-verify target app (or anything else in focus) runs at a higher Windows integrity level than the automation session user, `SendInput`-equivalent injection (which is what the RDP fast-path input channel ultimately drives, server-side) silently fails with no client-visible error — the PDU is accepted and the wire write succeeds, but the target does not react. For this phase's live verification, scope the target strictly to a standard-integrity app (Notepad, as CONTEXT.md's `<specifics>` already names as the canonical target) — do not test against anything requiring elevation or a UAC-secured desktop.

### Pitfall 5: `MouseAction::Scroll`'s caller-supplied `(x, y)` does not become the wheel PDU's position parameter for free

**What goes wrong:** `Operation::WheelRotations` (confirmed source) builds its `MousePdu` using `Database`'s *internally tracked* last mouse position (`self.mouse_position.x/y`), not any position passed alongside the wheel operation itself — there is no `x`/`y` field on `WheelRotations`. The public API `MouseAction::Scroll { x, y, dy }` (per the CONTEXT.md-locked call-site shape) therefore needs an *explicit* `Operation::MouseMove(MousePosition{x,y})` **before** the `Operation::WheelRotations` in the same batch, or the scroll will apply wherever the mouse last was (possibly stale/wrong) rather than the caller's intended `(x,y)`.

**How to avoid:** `MouseAction::Scroll`'s translation to `Vec<Operation>` must always be `[MouseMove{x,y}, WheelRotations{..}]`, two ops in one `apply()` call — not just the wheel op alone. (Note `MouseMove` itself is dedup-gated by `Database` — if the position is already correct, the move op is silently absorbed with no PDU emitted, which is fine and correct.)

## Code Examples

### `ironrdp-input` transaction pattern (verified against pinned 0.6.0 source and cross-checked with Context7's indexed llms.txt)

```rust
// Source: raw.githubusercontent.com/Devolutions/IronRDP/ironrdp-input-v0.6.0/crates/ironrdp-input/src/lib.rs
use ironrdp_input::{Database, Operation, MouseButton, MousePosition, Scancode, WheelRotations};
use ironrdp::pdu::input::fast_path::FastPathInputEvent;

let mut db = Database::new();

// Click at (500, 300):
let events: smallvec::SmallVec<[FastPathInputEvent; 2]> = db.apply(vec![
    Operation::MouseMove(MousePosition { x: 500, y: 300 }),
    Operation::MouseButtonPressed(MouseButton::Left),
    Operation::MouseButtonReleased(MouseButton::Left),
]);

// Ctrl+A combo (D-3.5 modifier ordering):
let ctrl = Scancode::from_u8(false, 0x1D); // left Ctrl, Set 1
let a = Scancode::from_u8(false, 0x1E);    // 'A', Set 1
let events = db.apply(vec![
    Operation::KeyPressed(ctrl),
    Operation::KeyPressed(a),
    Operation::KeyReleased(a),
    Operation::KeyReleased(ctrl),
]);

// Type "hi" (Unicode path, no modifiers possible):
let events = db.apply(vec![
    Operation::UnicodeKeyPressed('h'), Operation::UnicodeKeyReleased('h'),
    Operation::UnicodeKeyPressed('i'), Operation::UnicodeKeyReleased('i'),
]);

// Vertical scroll, 1 notch up, at last-tracked position — see Pitfall 5 for why
// a MouseMove must precede this if a specific (x,y) is required:
let events = db.apply(vec![
    Operation::WheelRotations(WheelRotations { is_vertical: true, rotation_units: 120 }),
]);
```

### Proposed `Error` variant (mirrors `CropOutOfBounds` exactly)

```rust
// crates/rdpilot/src/error.rs — new variant, same shape/style as CropOutOfBounds
#[error(
    "coordinate ({x},{y}) is out of bounds for a {desktop_w}x{desktop_h} desktop"
)]
CoordinateOutOfBounds { x: u32, y: u32, desktop_w: u32, desktop_h: u32 },
```

## State of the Art

| Old Approach (this repo's own `research/STACK.md`, dated 2026-06-04) | Current / Actual (as pinned) | When Changed | Impact |
|--------------------------------------------------------------------|-------------------------------|---------------|--------|
| `research/STACK.md` lists `ironrdp-input = "0.14"` | `Cargo.toml`/`Cargo.lock` actually pin `ironrdp-input = "0.6"` | Resolved during Phase 1/2 planning (per `Cargo.toml`'s own comment: "member crates at their independent versions... 0.9/0.8/0.6/0.2 — uniform 0.14 pins will NOT resolve") | This is *expected*, not a regression — the umbrella `ironrdp` facade is 0.15/0.14-ish, but member crates version independently and much lower. All API details in this research were checked against the actually-pinned 0.6.0/0.8.0, not the stale 0.14 figure still sitting in the older research doc. Any future research pass should treat `Cargo.lock`, not `research/STACK.md`, as the version source of truth. |

No other deprecated/outdated findings apply — `ironrdp-input`'s `Database`/`Operation` API and `ironrdp-pdu`'s fast-path input enums are both stable, actively-used-in-production (Devolutions Gateway) shapes with no announced breaking changes pending.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | A static connect-time `desktop_size` capture (no live update on server-driven resize) is sufficient for this phase's 4 success criteria | Pattern 1 | If the live-test VM unexpectedly resizes mid-session (Phase 1 fixed the resolution, so unlikely), `desktop_size()` would report a stale value and bounds-checking could incorrectly reject valid post-resize coordinates, or accept now-invalid pre-resize ones |
| A2 | 5 interpolated MOVE events at even spacing is enough to register a drag on whatever control the live suite exercises | Resolved Open Unknowns §4 | If wrong, the live drag test fails and the planner needs a follow-up tuning pass — explicitly scoped as a live-verify item, not a hard blocker |
| A3 | The Phase-1-provisioned VM's `GetDoubleClickTime()` has not been changed from the Windows default (500ms) | Resolved Open Unknowns §4 | If the VM's threshold is lower than assumed, D-3.7's 100ms delay is still safely inside it (100 < 500 regardless), so this specific assumption has low practical risk — flagged for completeness only |

## Open Questions

1. **Should `desktop_size()` be updated live on server-driven resize (reactivation), or is a static connect-time value acceptable for v1?**
   - What we know: Phase 1 fixes the VM's resolution; Phase 3's live tests do not exercise a mid-session resize; a static capture satisfies all 4 stated success criteria.
   - What's unclear: whether a future phase (or an unexpected server-side event) will hit the staleness gap.
   - Recommendation: Ship the static capture for v1 (documented explicitly in a code comment as a deliberate v1 simplification, not an oversight), and file it as a candidate backlog item if a later phase needs live-updating desktop size.

2. **Exact drag step count/spacing and double-click delay tuning** — see Resolved Open Unknowns §4; genuinely requires live-VM validation, not resolvable from static research.

3. **Should `Database::release_all()` be wired into `Session::close()`/`Drop`?** Not required by any of D-3.1–D-3.8 or the 4 success criteria, but it's a one-line addition that guards against a session closing mid-drag (button still logically "pressed" in `Database`'s state, though the connection is gone anyway so it's likely moot for v1). Low-priority discretionary addition; recommend the planner decide, not blocking.

## Environment Availability

No new external tools/services are introduced by this phase. It reuses exactly the same live-target infrastructure Phase 2 already established and gated:

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Azure test VM + `.secrets/connection.json` | D-3.4 live screenshot-diff verification | Provisioned on-demand via Phase 1's `manage-env.ps1 up` (not currently running by default) | n/a | Offline `cargo test` stays green without it (`RDPILOT_LIVE` gate, D-18 precedent) |
| `ironrdp-input`/`ironrdp-pdu` (already in `Cargo.lock`) | All of INPUT-01/02 | Yes — resolved at 0.6.0/0.8.0 | 0.6.0 / 0.8.0 | n/a |

**Missing dependencies with no fallback:** none.
**Missing dependencies with fallback:** none beyond the already-established live-VM gating pattern.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `cargo test` (`#[test]` / `#[tokio::test]`), no external test framework — same as Phase 2 |
| Config file | None — `Cargo.toml`'s `[dev-dependencies]` only (`tokio` macros/rt-multi-thread, `serde_json`, `serial_test`) |
| Quick run command | `cargo test -p rdpilot` (offline unit tests only; live suite `#[ignore]`'d by default) |
| Full suite command | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` (requires a provisioned VM + `.secrets/connection.json`, per `tests/common/mod.rs`) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| INPUT-02 (criterion #4 half) | `send_mouse`/bounds-check rejects out-of-range coords before any PDU is built (no VM needed) | unit | `cargo test -p rdpilot coordinate_out_of_bounds -x` | ❌ Wave: types+seam (new `#[cfg(test)]` in `input.rs`/`session.rs`) |
| — | `desktop_size()` returns the connect-time value, widened correctly (no VM needed) | unit | `cargo test -p rdpilot desktop_size -x` | ❌ Wave: types+seam |
| — | Scroll magnitude >255 is rejected/split, not silently wrapped (Pitfall 1 regression guard, no VM needed) | unit | `cargo test -p rdpilot scroll_magnitude -x` | ❌ Wave: mouse |
| — | `MouseAction::Scroll` translation always emits a `MouseMove` before `WheelRotations` (Pitfall 5 regression guard, no VM needed) | unit | `cargo test -p rdpilot scroll_translation -x` | ❌ Wave: mouse |
| INPUT-01 (criterion #1, #2) | Click activates a Notepad menu; move/click-variants/scroll/drag all observably affect the remote desktop | live (screenshot-diff) | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored mouse` | ❌ Wave: gated live suite (extend `tests/live_session.rs`) |
| INPUT-02 (criterion #3) | Typed text and key combos (Ctrl+A, Alt+F4) received by the remote app | live (screenshot-diff) | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored keyboard` | ❌ Wave: gated live suite |

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot` (offline unit tests)
- **Per wave merge:** offline unit suite + `RDPILOT_LIVE=1 ... --include-ignored` where a live VM is available (developer's local gated run, same D-18 pattern as Phase 2)
- **Phase gate:** Full suite green (offline + one canonical live run against a freshly-provisioned VM) before `/gsd-verify-work`, matching Phase 2's precedent of a canonical live pass before phase completion

### Wave 0 Gaps
- No new test framework or config needed — `tests/common/mod.rs` and `tests/live_session.rs` are reused as-is (D-3.4 explicitly says "extend," not replace).
- New unit test modules needed inline in `input.rs`/`session.rs`/`error.rs` (standard `#[cfg(test)] mod tests` pattern already used throughout this crate — no new file, no new fixture).

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | Unaffected by this phase (Phase 2's NLA/CredSSP path is untouched) |
| V3 Session Management | no | Unaffected |
| V4 Access Control | no | No new access-control surface |
| V5 Input Validation | yes | Rust's type system (owned enums, no raw `ironrdp` types in the public API) + explicit `Error::CoordinateOutOfBounds` bounds check before any PDU construction (never a silent clamp/truncate — directly informed by Pitfall 1's discovery that the *underlying library itself* silently truncates, making SDK-level validation the only correctness backstop) |
| V6 Cryptography | no | No new cryptography; input PDUs ride the existing TLS/CredSSP-secured channel from Phase 2 |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Out-of-range/malformed coordinates reaching PDU construction | Tampering | `Error::CoordinateOutOfBounds`, checked before any `Operation`/PDU is built (D-3.2) |
| Wheel-rotation magnitude silent wraparound (library-level bug surfaced by this research) | Tampering (data integrity of the constructed PDU) | SDK-level pre-check/split of any `|dy| > 255` before it ever reaches `ironrdp-input`/`ironrdp-pdu` |
| Typed text (`KeyAction::Type`) or key combos logged via `tracing` | Information Disclosure | Never log the contents of `KeyAction::Type(text)` or `Combo(keys)` at any `tracing` level above a redacted summary (e.g. log only the *variant name* and character count, never the string) — mirrors the existing `ConnectionConfig` password-redaction pattern (D-14) |
| UIPI blocking input into an elevated-integrity target (Pitfall C4) | Denial of Service (input silently swallowed, no error surfaced) | Documented constraint only for v1 — scope live verification to standard-integrity targets (Notepad); no code-level mitigation exists within this SDK's boundary |

## Sources

### Primary (HIGH confidence — raw source at exact pinned-version git tags, cross-checked against `Cargo.lock`)
- `github.com/Devolutions/IronRDP` @ tag `ironrdp-pdu-v0.8.0` — `crates/ironrdp-pdu/src/input/fast_path.rs` (full file read), `crates/ironrdp-pdu/src/input/mouse.rs` (full encode/decode read), `crates/ironrdp-pdu/src/input/mod.rs` (full file read)
- `github.com/Devolutions/IronRDP` @ tag `ironrdp-input-v0.6.0` — `crates/ironrdp-input/src/lib.rs` (full file read, 453 lines)
- `Cargo.lock` (this repo) — confirms `ironrdp 0.15.0`, `ironrdp-pdu 0.8.0`, `ironrdp-input 0.6.0`, `ironrdp-tokio 0.9.0` are each resolved to exactly one instance (no version-split type-mismatch risk)
- Context7 `/devolutions/ironrdp` (`query-docs`) — `ironrdp-input` `Database`/`Operation` transaction pattern and `synchronize_event`, cross-checked against and matching the raw 0.6.0 source
- `learn.microsoft.com/windows/win32/inputdev/wm-mousewheel` — `WHEEL_DELTA=120`, wheel-rotation sign convention
- `learn.microsoft.com/windows/win32/api/winuser/nf-winuser-getdoubleclicktime` — default double-click threshold (500ms)
- This repo: `crates/rdpilot/src/{session.rs,session_loop.rs,keepalive.rs,screenshot.rs,connect.rs,error.rs,config.rs,lib.rs}`, `tests/{common/mod.rs,live_session.rs}`, `Cargo.toml`

### Secondary (MEDIUM confidence)
- `devblogs.microsoft.com/oldnewthing/20130123-00` — historical rationale for `WHEEL_DELTA=120`'s specific value (context only, not load-bearing)
- WebSearch for existing scancode-table crates (`pc-keyboard`, `ps2`, `scancode`) — confirmed via each crate's own description that they solve the opposite direction of the problem

### Tertiary (LOW confidence)
- None — every load-bearing claim in this document was verified against either raw pinned-version source or official Microsoft documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new dependencies; existing pins confirmed via `Cargo.lock` and raw source at matching tags
- Architecture (Mutex<Database>-in-Session, timing-on-caller-context): HIGH — directly derived from reading the actual `session_loop.rs`/`session.rs`/`keepalive.rs` source and the constraints those files already impose (single-threaded dedicated-thread runtime, existing channel shape, existing doc-comment anticipating `RdpInputEvent::FastPath`)
- Pitfalls (wheel truncation, KeyboardFlags collision): HIGH — verified by reading the actual `encode()`/`decode()` implementations, not inferred
- Live-verify-only items (double-click timing, drag step count): correctly LOW/unresolvable by design — flagged as such, not force-fit into a false-confidence answer

**Research date:** 2026-07-08
**Valid until:** 30 days (stable, production-used upstream crates; pinned versions in this repo are the actual constraint, not upstream churn)
