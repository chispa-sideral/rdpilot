# Phase 8: Public SDK API + WorldState - Research

**Researched:** 2026-07-09
**Domain:** Rust SDK API surface hardening (compiler-enforced strictness) + a new composite/aggregating `Session` method (`world_state()`) that sequences three existing perception calls under a single timestamp/span measurement. No new external dependencies, no new wire protocol, no new remote (C#) sensor code.
**Confidence:** HIGH — every claim below was either read directly from the current source tree or verified by actually compiling/running code against the pinned dependency versions in this repo's `Cargo.lock` (not training-data assumption).

## Summary

Phase 8 is ~90% audit + ~10% genuinely new code. API-01 (clean typed `Session` API) is already true today: `crates/rdpilot/src/lib.rs` re-exports only owned types, and a live `cargo clippy --all-targets -D clippy::unwrap_used -D clippy::expect_used -D unsafe_code` run against the library target (excluding the `tests/` integration binary) passes with zero unwrap/expect/unsafe violations — only two pre-existing, unrelated warnings (an unused import in `input.rs`, one `clippy::unnecessary_get_then_check` in `rdpdr_backend.rs`). This phase's real work is: (1) add `#![deny(unsafe_code)]` + clippy `unwrap_used`/`expect_used` gates so this is compiler-enforced, not convention-enforced; (2) add `#[derive(Serialize)]` to the five existing owned types plus the two new ones (`WorldStateOptions`, `WorldState`); (3) build the new `WorldStateOptions`/`WorldState` types and the `Session::world_state()` method that sequences `screenshot()` / `get_window_list()` / `get_uia_tree()` under one measured span.

Two findings materially change how the planner should scope tasks, both empirically verified in this session:

1. **The strict lint gates MUST live as `lib.rs` inner attributes (`#![deny(...)]`), not a `[lints]` table in `Cargo.toml`.** A `[lints]` table applies to every target in the package, including the `tests/live_session.rs` integration binary. Running clippy with the proposed `unwrap_used`/`expect_used` denials across `--all-targets` produces **116 compile errors** in the existing (correct, intentional) live-test suite, which uses `.expect(...)` extensively and legitimately in test code. `lib.rs` inner attributes only gate the library crate compilation unit and leave `tests/*.rs` (a separate crate root) untouched — this is the only approach that satisfies D-8.4 without breaking the existing 27+ gated live tests.
2. **`std::time::Instant` cannot be used for the `WorldState` batch timestamp field if `WorldState` must derive `Serialize` (D-8.4).** Compiling a minimal probe against this repo's exact pinned `serde 1.0.228` confirms `Instant` has no `Serialize` impl (compile error: `the trait Serialize is not implemented for Instant`), while `std::time::SystemTime` and `std::time::Duration` **do** serialize out-of-the-box via serde's built-in impls (`SystemTime` → `{secs_since_epoch, nanos_since_epoch}`, `Duration` → `{secs, nanos}}`) — no `chrono`/`time` crate needed, resolving the CONTEXT's open "no time crate exists" question definitively in favor of stdlib-only. Use `SystemTime` for the batch timestamp and `Duration` (measured via an internal `Instant::now()`/`.elapsed()` pair, never stored) for the capture span.

**Primary recommendation:** Add `#![deny(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]` as `lib.rs` inner attributes (not a `Cargo.toml [lints]` table); add `#[derive(Serialize)]` to `Rect`, `WindowInfo`, `WindowState`, `ProcessInfo`, `UiaElement` with `#[serde(skip)]` on `Screenshot::rgba` (dims-only serialize); build `WorldStateOptions { screenshot: bool, window_list: bool, uia: UiaMode }` / `enum UiaMode { None, Foreground, Hwnd(Vec<u64>), AllTopLevel }` / `WorldState { timestamp: SystemTime, capture_span: Duration, screenshot: Option<Screenshot>, window_list: Option<Vec<WindowInfo>>, uia: Option<Vec<(u64, Vec<UiaElement>)>> }`, where `uia`'s per-window grouping (`Vec<(u64, Vec<UiaElement>)>` rather than a flat `Vec<UiaElement>`) is a new recommendation this research makes to resolve an ambiguity CONTEXT left open (see Architecture Patterns, Pattern 2).

## User Constraints

### Locked Decisions (from CONTEXT.md, verbatim)

- **D-8.1 (a-la-carte composition):** `Session::world_state(WorldStateOptions)` is fully a-la-carte: each component individually selectable. `screenshot` (default ON), `window_list` (default ON), `uia` as a mode — `None` (default) / `Foreground` / `Hwnd(u64)` (one or a set) / `AllTopLevel`. A default `WorldStateOptions` yields screenshot + window list + no UIA (SC#2-compliant).
- **D-8.2 (timestamp + best-effort span):** `world_state()` returns a `WorldState` carrying ONE batch timestamp plus the measured capture span (elapsed across sequenced component fetches). The 500ms bound is BEST-EFFORT — the call still returns and surfaces measured elapsed if exceeded, never hard-fails. SC#2 validated empirically at a live gate. Timestamp type is planner's discretion — no `chrono`/`time` dependency exists today.
- **D-8.3 (single pixel space now):** `WorldState` uses a SINGLE physical virtual-desktop pixel space — the existing shared `crate::Rect` — with no silent scaling. Dual pixel + logical/DPI-scaled coordinates DEFERRED to backlog.
- **D-8.4 (serialization + strict lints):** Add `serde::Serialize` to all owned public types (`Rect`, `WindowInfo`, `WindowState`, `ProcessInfo`, `UiaElement`) and the new `WorldState`, WITHOUT pulling `image`/`ironrdp` types into the public surface. `Screenshot` serializes DIMS-ONLY (`width`/`height`); raw RGBA buffer excluded from serde output. Add `#![deny(unsafe_code)]` plus clippy `unwrap_used`/`expect_used` gates (currently hold by convention + tests only).

### Claude's Discretion (from CONTEXT.md, verbatim)

- Exact `WorldStateOptions` field/enum names and whether the `uia` `Hwnd` variant takes a single `u64` or a `Vec<u64>` — planner's call, as long as the four selection modes are expressible.
- Timestamp type (`Instant` vs `SystemTime`) and whether the capture span is a single `Duration` field or per-component timestamps.
- Exact clippy lint set beyond `unwrap_used`/`expect_used` (e.g. `panic`, `missing_docs`) and whether the gates live in `lib.rs` attributes or a `[lints]` table in `Cargo.toml`.
- Whether `world_state()` sequences the existing public methods (`screenshot`/`get_window_list`/`get_uia_tree`) or funnels the sensor-backed parts through the private `sensor_request()` helper — an implementation detail.
- Exact serde representation of the `WindowState` enum and any `#[serde(rename)]` conventions.

### Deferred Ideas (OUT OF SCOPE, verbatim)

- Dual pixel + logical/DPI-scaled coordinates (API-02 literal) — backlog, revisit for non-96-DPI targets.
- rdpilot CLI / MCP consumer surface — v2, seeded at `.planning/seeds/rdpilot-cli-agent-peer-tool.md`. Informs Phase 8 (a-la-carte + serde) but NOT built here.
- `AllTopLevel` UIA latency risk on many-window desktops — degrades gracefully per D-8.2 (best-effort), a per-window cap is a future refinement if it proves consistently slow.
- `value`/`ValuePattern` on `UiaElement` — still backlog (inherited D-7.1).

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| API-01 | A clean, typed SDK API surface exposes control + perception so a consumer can drive a session programmatically | VERIFIED already ~90% true (lib.rs re-export audit, zero unwrap/expect/unsafe in library-only clippy run below). This research's lint-gate scoping finding (inner attributes, not `[lints]` table) is the concrete mechanism to lock it permanently without breaking the existing live-test suite. |
| API-02 | A coherent `WorldState` correlates screenshot + window list + UIA snapshot in one coordinate space, emitting both pixel and logical/DPI-scaled coordinates | Single-pixel-space part RESOLVED (D-8.3, already unified via shared `crate::Rect`, verified in `perception.rs`/`screenshot.rs`). This research's Pattern 1/2 (below) give the concrete `WorldStateOptions`/`WorldState` shape and the sequencing algorithm, including the Foreground/AllTopLevel hidden-dependency-on-window-list finding. Dual DPI representation explicitly deferred per D-8.3 — not addressed here. |

## Architectural Responsibility Map

This SDK does not have web-app tiers (browser/CDN/DB); the equivalent architectural layers are: the public SDK API (this crate's `lib.rs`/`session.rs` surface), the client-side framebuffer (in-process, no round trip), the DVC transport (`RDPILOT_SENSOR` channel), and the remote sensor helper (the C# process on the Windows target). Phase 8 touches only the first layer.

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Public API surface hardening (API-01: deny gates, no unsafe/unwrap) | SDK Public API (`lib.rs`, `Cargo.toml`) | — | Compile-time gate; no runtime component involved |
| Serialization (`Serialize` derives) | SDK Public API (owned types in `screenshot.rs`/`perception.rs`) | — | Purely additive trait impls on existing owned types; never touches wire (`*Wire`) types or third-party (`image`/`ironrdp`) types (D-09) |
| Screenshot capture (`world_state().screenshot`) | Client-side Framebuffer (`Session::screenshot`, in-process clone of latest `DecodedImage`) | — | No sensor round trip — reused verbatim from Phase 2/6 |
| Window list / UIA tree (`world_state().window_list`/`.uia`) | Remote Sensor Helper (C# process, EnumWindows/UIA COM) | DVC Transport (`RDPILOT_SENSOR` channel, `sensor_request()`) | Structured perception sourced from the target machine; Phase 8 does not change the wire protocol, only how the SDK composes existing calls |
| `world_state()` sequencing + span measurement | SDK Public API (new `Session::world_state()`) | Client-side Framebuffer + Remote Sensor Helper (the components it calls) | The aggregator itself is a pure orchestration layer in the SDK; it owns none of the underlying data acquisition, only the batching/timing contract |

## Standard Stack

No new external dependencies for this phase. Every recommendation below is satisfied by crates already pinned in `Cargo.lock`.

### Core (already present, reused)

| Library | Version (Cargo.lock) | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `serde` | 1.0.228 (`features = ["derive"]`, already in `Cargo.toml`) | `#[derive(Serialize)]` on owned types + `WorldState` | D-8.4 requirement; already a direct dependency, `derive` feature already enabled — no `Cargo.toml` change needed for the derive machinery itself |
| `std::time` | stdlib | `SystemTime` (batch timestamp), `Instant`+`Duration` (span measurement) | VERIFIED (this session, compiled against pinned serde 1.0.228): `SystemTime`/`Duration` implement `Serialize` natively via serde's built-in impls; `Instant` does not. No `chrono`/`time` crate needed or justified. |

### Supporting

None — this phase adds zero supporting libraries. The `WorldStateOptions`/`WorldState`/`UiaMode` types are pure Rust structs/enums built from existing crate-internal types (`crate::Rect`, `crate::Screenshot`, `crate::WindowInfo`, `crate::UiaElement`).

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `SystemTime` for batch timestamp | `chrono::DateTime<Utc>` | Adds a new dependency for a capability serde's stdlib impl already provides; no v1 justification given the "no chrono/time exists today" baseline in CONTEXT — rejected. |
| `Instant`-only span (no wall clock at all) | `SystemTime` timestamp + `Duration` span (recommended) | An `Instant`-only `WorldState` cannot derive `Serialize` (VERIFIED, see Summary) — blocks D-8.4 outright, not just a stylistic choice. |
| `lib.rs` attributes for lint gates | `[lints]` table in `Cargo.toml` | VERIFIED: `[lints]` applies package-wide, breaking `tests/live_session.rs`'s 116 legitimate `.expect()` calls under `-D clippy::expect_used`. `lib.rs` attributes scope correctly to the library crate only. |
| Flat `Vec<UiaElement>` for multi-window UIA modes | `Vec<(u64, Vec<UiaElement>)>` grouped by hwnd (recommended) | A flat `Vec<UiaElement>` loses which window each element came from once `AllTopLevel`/multi-`Hwnd` aggregates more than one window's tree — `UiaElement.parent_id` is only meaningful within a single window's flat tree, so a flat merge silently makes `parent_id` ambiguous across windows. |

**Installation:** none — no new `cargo add` needed. `serde`'s `derive` feature is already enabled in `crates/rdpilot/Cargo.toml` line 50 (`serde = { version = "1", features = ["derive"] }`).

**Version verification:** `serde 1.0.228` confirmed in `/home/marc/dev/rdpilot/Cargo.lock` line 3017 (already resolved, no registry lookup needed — this is an existing pinned dependency, not a new install).

## Package Legitimacy Audit

**Not applicable — this phase introduces zero new external packages.** `serde` (with `derive`) is already a direct dependency (`Cargo.toml` line 50) and already resolved in `Cargo.lock` (`serde 1.0.228`). No `slopcheck`/registry verification is needed for an already-vetted, already-pinned dependency with no version bump required.

## Architecture Patterns

### System Architecture Diagram

```
Consumer code
   │
   │ Session::world_state(WorldStateOptions { screenshot, window_list, uia })
   ▼
┌─────────────────────────────────────────────────────────────┐
│ Session::world_state()  (NEW — session.rs)                  │
│                                                               │
│  let started = Instant::now();          // span measurement  │
│                                                               │
│  if opts.screenshot { screenshot().await? }  ──► Client-side │
│                                                    Framebuffer│
│                                                    (no round  │
│                                                     trip)     │
│                                                               │
│  if opts.window_list || needs_window_list_for(opts.uia) {    │
│      get_window_list().await?           ──► sensor_request() │
│  }                                       ──► RDPILOT_SENSOR   │
│                                               DVC ──► C#      │
│                                               sensor          │
│                                                               │
│  match opts.uia {                                             │
│    None          => None,                                     │
│    Foreground     => pick min-z_order titled window from the  │
│                       window_list just fetched, then           │
│                       get_uia_tree(that hwnd).await?           │
│    Hwnd(hwnds)    => get_uia_tree(h).await? for h in hwnds     │
│    AllTopLevel    => get_uia_tree(h).await? for h in           │
│                       window_list's hwnds                     │
│  }                                        ──► sensor_request() │
│                                            ──► RDPILOT_SENSOR   │
│                                                DVC ──► C# sensor│
│                                                               │
│  let capture_span = started.elapsed();  // best-effort, D-8.2 │
│  WorldState { timestamp: SystemTime::now(), capture_span, .. }│
└─────────────────────────────────────────────────────────────┘
   │
   ▼
WorldState { timestamp, capture_span, screenshot?, window_list?, uia? }
   │
   │ (consumer may serde_json::to_string(&world_state) for the
   │  seeded CLI/MCP peer-tool use case — Screenshot.rgba is
   │  #[serde(skip)]'d, so this payload stays lean, D-8.4)
   ▼
Consumer (local AI agent, scripted harness, etc.)
```

A reader can trace the primary use case end-to-end: a consumer calls `world_state()` with options → the method internally sequences the existing client-side screenshot capture and 0–N sensor round trips over the same `RDPILOT_SENSOR` DVC channel Phases 4–7 already built → returns one correlated, timestamped, serializable snapshot.

### Recommended Project Structure

No new files needed. This phase's code lands in existing modules:

```
crates/rdpilot/src/
├── lib.rs           # add WorldStateOptions/WorldState/UiaMode to the pub use list; add #![deny(unsafe_code)] etc.
├── session.rs       # add Session::world_state() near the other sensor-backed methods (after get_uia_tree)
├── screenshot.rs     # add #[derive(Serialize)] to Rect; add Serialize + #[serde(skip)] to Screenshot
├── perception.rs     # add #[derive(Serialize)] to WindowInfo/WindowState/ProcessInfo/UiaElement
└── error.rs          # no change expected (WorldState composition reuses existing Error variants)
```

A new top-level type home (`worldstate.rs`) is a viable alternative to inlining `WorldStateOptions`/`WorldState`/`UiaMode` into `session.rs`, mirroring how `screenshot.rs`/`perception.rs` already separate owned-type definitions from `session.rs`'s method bodies. Either is consistent with the existing module-per-concern pattern; the planner should pick one and instruct consistently (this research recommends a new `worldstate.rs` module for the two/three new public types, keeping `session.rs` focused on the `Session` struct and its methods, matching how `Rect`/`Screenshot` live in `screenshot.rs` rather than `session.rs`).

### Pattern 1: `WorldStateOptions` / `UiaMode` shape

**What:** A plain-data options struct with a `bool` per independently-toggleable component (`screenshot`, `window_list`) plus a 4-variant enum for the UIA selection mode, implementing `Default` to satisfy D-8.1's "default `WorldStateOptions` yields screenshot + window list + no UIA."

**When to use:** As the sole parameter to `Session::world_state()`.

**Example:**
```rust
// New module, e.g. crates/rdpilot/src/worldstate.rs
use serde::Serialize;
use std::time::{Duration, SystemTime};

/// Selects which UIA trees `Session::world_state()` fetches (D-8.1).
///
/// `Hwnd` takes a `Vec<u64>` rather than a single `u64` so "one or a set"
/// (D-8.1's literal wording) is expressible with exactly one variant: a
/// single-window caller passes a one-element `Vec`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UiaMode {
    /// No UIA tree is fetched (default — matches D-8.1's SC#2-compliant
    /// default `WorldStateOptions`).
    #[default]
    None,
    /// Fetch the UIA tree for the current foreground window only.
    /// `world_state()` determines the foreground hwnd from the window list
    /// it must therefore fetch regardless of `WorldStateOptions::window_list`
    /// (see Pattern 2 — a hidden dependency this research surfaces).
    Foreground,
    /// Fetch the UIA tree for exactly these window handles.
    Hwnd(Vec<u64>),
    /// Fetch the UIA tree for every top-level window in the window list
    /// (D-8.1; latency risk on many-window desktops is a known, accepted
    /// best-effort tradeoff per D-8.2 — see Deferred).
    AllTopLevel,
}

/// A-la-carte selection of which `WorldState` components to fetch (D-8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldStateOptions {
    /// Fetch a full-desktop screenshot (default: `true`).
    pub screenshot: bool,
    /// Fetch the window list (default: `true`).
    pub window_list: bool,
    /// UIA selection mode (default: `UiaMode::None`).
    pub uia: UiaMode,
}

impl Default for WorldStateOptions {
    fn default() -> Self {
        Self { screenshot: true, window_list: true, uia: UiaMode::default() }
    }
}
```

### Pattern 2: `WorldState` shape + the Foreground/AllTopLevel hidden dependency

**What:** The returned snapshot, one field per optional component, plus the batch timestamp and measured span.

**When to use:** As `Session::world_state()`'s return type.

**Example:**
```rust
use crate::{Rect, Screenshot, UiaElement, WindowInfo};

/// A single correlated snapshot of the remote desktop (API-02).
///
/// `uia` groups elements by the window they were fetched from
/// (`(hwnd, Vec<UiaElement>)`) rather than a flat `Vec<UiaElement>` — a
/// flat merge across `AllTopLevel`/multi-`Hwnd` modes would make
/// `UiaElement::parent_id` ambiguous once more than one window's tree is
/// concatenated (each window's tree has its own independent `RuntimeId`
/// namespace). This shape is this research's recommendation resolving an
/// ambiguity CONTEXT left as planner's discretion.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorldState {
    /// Wall-clock time the batch was captured (D-8.2). `SystemTime`, not
    /// `Instant` — `Instant` has no `Serialize` impl (verified against this
    /// repo's pinned serde 1.0.228), which would block deriving `Serialize`
    /// on `WorldState` (D-8.4).
    pub timestamp: SystemTime,
    /// Measured elapsed time across the sequenced component fetches
    /// (D-8.2). Best-effort: exceeding 500ms does not fail the call, it is
    /// simply surfaced here for the caller (and SC#2's live gate) to check.
    pub capture_span: Duration,
    pub screenshot: Option<Screenshot>,
    pub window_list: Option<Vec<WindowInfo>>,
    pub uia: Option<Vec<(u64, Vec<UiaElement>)>>,
}
```

**Sequencing logic** (in `Session::world_state`, pseudocode — exact error-handling/propagation is the planner's task-level design):
```rust
pub async fn world_state(&self, opts: WorldStateOptions) -> Result<WorldState> {
    let started = std::time::Instant::now();

    let screenshot = if opts.screenshot { Some(self.screenshot().await?) } else { None };

    // Foreground/AllTopLevel both need the window list even if
    // opts.window_list is false — fetch it once, share it, only attach it
    // to WorldState.window_list if the caller actually asked for it.
    let needs_list_internally = opts.window_list
        || matches!(opts.uia, UiaMode::Foreground | UiaMode::AllTopLevel);
    let windows = if needs_list_internally {
        Some(self.get_window_list().await?)
    } else {
        None
    };

    let uia = match &opts.uia {
        UiaMode::None => None,
        UiaMode::Hwnd(hwnds) => {
            let mut out = Vec::with_capacity(hwnds.len());
            for &h in hwnds {
                out.push((h, self.get_uia_tree(h).await?));
            }
            Some(out)
        }
        UiaMode::Foreground => {
            let list = windows.as_ref().expect("fetched above when uia == Foreground");
            // Mirrors the Phase 6 live-gate-diagnosed foreground heuristic:
            // minimum z_order among *titled* windows (always-on-top shell
            // chrome legitimately outranks any normal app window).
            if let Some(top) = list.iter()
                .filter(|w| !w.title.is_empty())
                .min_by_key(|w| w.z_order)
            {
                Some(vec![(top.hwnd, self.get_uia_tree(top.hwnd).await?)])
            } else {
                Some(vec![])
            }
        }
        UiaMode::AllTopLevel => {
            let list = windows.as_ref().expect("fetched above when uia == AllTopLevel");
            let mut out = Vec::with_capacity(list.len());
            for w in list {
                out.push((w.hwnd, self.get_uia_tree(w.hwnd).await?));
            }
            Some(out)
        }
    };

    Ok(WorldState {
        timestamp: std::time::SystemTime::now(),
        capture_span: started.elapsed(),
        screenshot,
        window_list: if opts.window_list { windows } else { None },
        uia,
    })
}
```

**Note on error propagation:** every sequenced call (`screenshot()`, `get_window_list()`, `get_uia_tree()`) already returns `Result<_>` and uses `?` in the example above — this means the FIRST failing component aborts the whole `world_state()` call (matching every other `Session` method's existing all-or-nothing error contract). D-8.2's "best-effort" language is about the *timing bound* (500ms), not about individual component failures — a transport failure (`Error::Dvc`) or semantic rejection (`Error::SensorRejected`) on any sequenced call still propagates as an `Err` from `world_state()` exactly as it would from a direct call to that method. This distinction is worth the planner stating explicitly in the `WorldState`/`world_state()` doc comment, since D-8.2's wording ("does NOT hard-fail... on a single transient slow sensor round-trip") could be misread as "swallow errors," which it does not mean.

### Pattern 3: `Screenshot` dims-only serialization

**What:** Derive `Serialize` on `Screenshot` directly with `#[serde(skip)]` on the `rgba: Vec<u8>` field — no manual `impl Serialize` needed.

**Example:**
```rust
#[derive(Clone, Debug, serde::Serialize)]
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub rgba: Vec<u8>,
}
```
`#[serde(skip)]` on a `Serialize`-only derive (no `Deserialize`) requires no `Default` bound — `Default` is only needed when `skip` is combined with `Deserialize` (the deserializer needs a placeholder value for the skipped field). Since D-8.4 only asks for `Serialize` on `Screenshot` (not `Deserialize` — the existing `to_png()` path remains the only way bytes leave the type), this is the minimal-footprint approach. **Do not add `Deserialize` to `Screenshot`** without also adding `#[serde(default)]` or a manual `Default` impl for `rgba`, or the derive will fail to compile.

### Anti-Patterns to Avoid

- **A `[lints]` table in `Cargo.toml` for the unwrap/expect gates:** VERIFIED (this session) to break the existing `tests/live_session.rs` integration suite with 116 compile errors. Use `lib.rs` inner attributes instead (they scope to the library crate compilation unit only).
- **Flat `Vec<UiaElement>` across multiple windows:** loses the window-origin context and makes `parent_id` cross-window-ambiguous. Group by `hwnd` (Pattern 2).
- **Storing an `Instant` anywhere in a type that must derive `Serialize`:** will not compile (VERIFIED). Use `SystemTime` for anything that needs to cross a serde boundary; keep `Instant` purely as a local variable for span measurement.
- **Adding a new DVC message type for "get UIA for all windows" on the sensor side:** unnecessary — `world_state()`'s `AllTopLevel`/multi-`Hwnd` modes are a pure client-side composition of the EXISTING `get_uia_tree(hwnd)` call in a loop. No C#/wire protocol change is needed for this phase (mirrors the "Don't Hand-Roll" entry below).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Wall-clock timestamp serialization | A manual epoch-millis `u64` field + custom `Serialize` impl | `std::time::SystemTime` directly, deriving `Serialize` | serde 1.0.228 (already pinned) serializes `SystemTime` natively as `{secs_since_epoch, nanos_since_epoch}` — VERIFIED by compiling against this repo's exact dependency version. No custom impl, no new crate. |
| Duration serialization | A manual `{millis: u64}` newtype | `std::time::Duration` directly, deriving `Serialize` | Same as above — serde serializes `Duration` natively as `{secs, nanos}`. |
| "Get all UIA trees for every open window" | A new sensor-side `AllWindowsUia` DVC message type in the C# helper | Client-side loop calling the existing `get_uia_tree(hwnd)` once per hwnd from `get_window_list()` | Phase 7 already built and live-verified `get_uia_tree(hwnd)`; `AllTopLevel`/multi-`Hwnd` is purely an SDK-side composition concern, not a new remote capability. Avoids touching the C# sensor / wire contract at all in this phase. |
| Crate-wide "no unwrap in tests either" enforcement | A custom build script or CI grep that fails on any `.expect()` anywhere in the repo | `lib.rs` inner attributes scoped to the library crate only | The existing, correct, already-live-verified test suite legitimately uses `.expect()` — D-8.4's requirement is about library code, not test code. Do not conflate the two. |

**Key insight:** every piece of "new" functionality this phase needs (timestamp/duration serialization, composing existing per-hwnd UIA calls into an "all windows" mode) is already provided by either the stdlib+serde combination already pinned, or by looping over an existing, already-verified public method. There is no case in this phase where a custom protocol, custom serialization format, or new external dependency is justified.

## Common Pitfalls

### Pitfall 1: `[lints]` table breaks the live-test suite
**What goes wrong:** Adding `[lints.clippy] unwrap_used = "deny"` / `expect_used = "deny"` to `Cargo.toml` applies those denials to every target built from this package — including `tests/live_session.rs`, which uses `.expect()` 100+ times as an intentional, idiomatic test-assertion pattern.
**Why it happens:** Cargo's `[lints]` table is package-scoped, not target-scoped; there is no first-class per-target lint override without significant `Cargo.toml` complexity (per-target lint configuration is not a stable Cargo feature as of this writing).
**How to avoid:** Use `#![deny(unsafe_code)]` / `#![deny(clippy::unwrap_used)]` / `#![deny(clippy::expect_used)]` as inner attributes at the top of `lib.rs`. These only apply to the library crate's own compilation unit; `tests/*.rs` files are each their own separate crate root and are unaffected.
**Warning signs:** `cargo clippy --all-targets` (or CI running the same) suddenly fails with dozens/hundreds of new errors after adding the gate — this is the signal the gate was applied at the wrong scope. VERIFIED empirically in this research session: 116 errors reproduced exactly this way.

### Pitfall 2: `Instant` silently blocks `#[derive(Serialize)]`
**What goes wrong:** If a task naively picks `Instant` for the "batch timestamp" (a very natural first instinct — it's already used for span measurement elsewhere in `session.rs`, e.g. `Session::ping`'s `let started = std::time::Instant::now();`), adding `#[derive(Serialize)]` to `WorldState` fails to compile.
**Why it happens:** `Instant` is deliberately opaque (platform-specific, no epoch semantics) and serde provides no impl for it — by design, since an `Instant` value is meaningless outside the process that created it.
**How to avoid:** Use `SystemTime::now()` for the field that ends up in `WorldState`; use a local `Instant::now()`/`.elapsed()` pair (never stored in the struct) purely to measure `capture_span: Duration`.
**Warning signs:** `error[E0277]: the trait bound Instant: Serialize is not satisfied` — VERIFIED reproduced in this research session against this repo's exact serde version.

### Pitfall 3: Foreground/AllTopLevel silently need `get_window_list()` even when `window_list: false`
**What goes wrong:** A naive `world_state()` implementation that only fetches the window list `if opts.window_list` will have no way to resolve `UiaMode::Foreground`'s target hwnd or `UiaMode::AllTopLevel`'s hwnd set when a caller sets `window_list: false, uia: Foreground` (a very plausible a-la-carte combination given D-8.1's explicit rationale — "avoid pulling a bulky screenshot... while keeping the default call SC#2-compliant" implies callers WILL mix-and-match).
**Why it happens:** The a-la-carte design (D-8.1) makes each field look independently togglable, but `Foreground`/`AllTopLevel` have a real data dependency on the window list that is not visible from the type signature alone.
**How to avoid:** Compute `needs_list_internally = opts.window_list || matches!(opts.uia, Foreground | AllTopLevel)` and fetch once; only populate `WorldState.window_list` if the caller actually asked for it (Pattern 2's pseudocode does exactly this).
**Warning signs:** A test with `window_list: false, uia: UiaMode::Foreground` panics/errors on an `Option::unwrap()`/missing-data path, or (worse) silently returns `uia: None` instead of the requested foreground tree.

### Pitfall 4: Conflating "best-effort span" (D-8.2) with "best-effort error handling"
**What goes wrong:** A task author reads "does NOT hard-fail... on a single transient slow sensor round-trip" and implements `world_state()` to swallow/ignore errors from `get_window_list()`/`get_uia_tree()` (e.g. returning `None` for that field on any error), rather than just not treating a >500ms elapsed time as an error.
**Why it happens:** D-8.2's wording is about the TIMING bound specifically, not about the correctness of individual component calls — the two are easy to conflate.
**How to avoid:** Propagate every component call's `Result` with `?` exactly as every other `Session` method does today (`Error::Dvc`/`Error::SensorRejected` still surface as an `Err` from `world_state()`). Only the elapsed-time comparison against 500ms is "soft" (a documented fact on the returned `WorldState.capture_span`, never a hard-fail branch).
**Warning signs:** A live gate test that intentionally disconnects the sensor mid-call still gets `Ok(WorldState { uia: None, .. })` instead of an `Err` — this would be the wrong behavior per this research's reading of D-8.2.

## Code Examples

### Deriving Serialize on the existing owned types (mechanical, Wave 1 candidate)
```rust
// screenshot.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

// perception.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WindowInfo { /* unchanged fields */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]  // mirrors the existing WindowStateWire convention exactly
pub enum WindowState { Normal, Minimized, Maximized }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ProcessInfo { /* unchanged fields */ }

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UiaElement { /* unchanged fields */ }
```
Recommendation: `#[serde(rename_all = "lowercase")]` on `WindowState` mirrors the crate's own existing wire convention (`perception.rs` line 112-117, `WindowStateWire` already uses `rename_all = "lowercase"` for deserialization) — using the identical convention on the owned type's `Serialize` output means a round trip through this crate's own wire format and the new public JSON output look the same, which is a reasonable, low-surprise default absent any other stated constraint.

### Strict lint gates (lib.rs, Wave 1 candidate)
```rust
// Top of lib.rs, before any mod declarations.
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
```
VERIFIED in this session: applying these exact three lints via `cargo clippy -p rdpilot --target x86_64-unknown-linux-gnu -- -D clippy::unwrap_used -D clippy::expect_used -D unsafe_code` (library target only, mirroring what `lib.rs` inner attributes would enforce) produces **zero** new violations against the current codebase — only the two pre-existing unrelated warnings noted in the Summary. No latent unwrap/expect/unsafe cleanup is needed before adding these gates.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| API-01 held "by convention + tests only" (grep-confirmed zero unwrap/expect/unsafe, but not compiler-enforced) | `#![deny(unsafe_code)]` + clippy `unwrap_used`/`expect_used` gates in `lib.rs` | This phase (D-8.4) | SC#4 becomes permanently, structurally true — a future PR that introduces an `.unwrap()` in library code fails to compile/lint-pass rather than silently regressing past a convention. |
| Every `Session` method is single-purpose (`screenshot()`, `get_window_list()`, `get_uia_tree()` called independently by the consumer, who must correlate timing themselves) | `world_state()` — the first composite/aggregating call, sequencing existing methods under one measured span | This phase (API-02) | Consumers no longer hand-roll their own "fetch these three things and hope they're close enough in time" logic; the SDK now owns and measures that correlation contract directly. |
| No owned type is JSON-serializable (only internal `*Wire` structs derive `Deserialize`, for consuming sensor replies) | `Serialize` on all owned public types + `WorldState` | This phase (D-8.4) | Unlocks the seeded v2 CLI/MCP consumer's need for JSON `WorldState` output without adding any new dependency — direct payoff of a decision made now for a documented future need, not speculative. |

**Deprecated/outdated:** none — this phase adds capability, it does not deprecate anything in the existing API surface.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | The Foreground-mode heuristic ("minimum z_order among titled windows") is the correct way to determine the current foreground window from a `WindowInfo` list | Pattern 2 | LOW — this exact heuristic is not an invention of this research; it mirrors the Phase 6 live-gate-diagnosed fix already committed in `session.rs`/tests (STATE.md: "the SC#3 test now confirms focus via the minimum z_order among TITLED windows only... always-on-top shell chrome... legitimately outranks any normal app window regardless of focus"). Re-verify against a live target during Phase 8's own live gate (if one is scoped) since the exact z_order semantics were live-diagnosed once already and could have edge cases (e.g. no titled window exists — `UiaMode::Foreground` then yields an empty `Vec`, which this research's pseudocode handles but should be explicitly tested). |
| A2 | A new `worldstate.rs` module (rather than inlining the two/three new types into `session.rs` or `perception.rs`) is the preferred file location | Architecture Patterns / Recommended Project Structure | LOW — purely organizational; explicitly flagged as "either is consistent," planner's discretion, no functional risk either way. |

**If this table is empty:** N/A — two low-risk organizational/heuristic assumptions are logged above; every load-bearing technical claim (lint scoping, `Instant`/`SystemTime` serde behavior, current API-01 compliance state, `serde` version) was verified by compiling/running code in this session, not assumed from training data.

## Open Questions

1. **Does Phase 8 need its own live gate at all, and if so, how thin should it be?**
   - What we know: SC#1 (no IronRDP/sensor-protocol leak), SC#3 (single coordinate space), and SC#4 (strict lints compile clean) are all fully verifiable OFFLINE — SC#1/SC#4 by static analysis (`grep`/`cargo clippy`, both already done in this research), SC#3 by construction (the shared `crate::Rect` type already unifies the space, verified by reading `perception.rs`). SC#2 (the 500ms best-effort span) is the one criterion that benefits from a real measurement against a real target, exactly like every prior phase's timing constants (D-8.2's own rationale: "matches the project's consistent 'reason offline, live-tune empirically' methodology").
   - What's unclear: whether a live gate is strictly REQUIRED to close Phase 8, given the span is explicitly "best-effort, never hard-fails" (so there is no pass/fail live assertion to make the way Phase 4-7's hard `< 500ms`/`< 1s` bounds required). A live run could simply report the measured span for the record (as Phase 7's SC#3 did — "measured 30.36ms, well under budget") without gating on it.
   - Recommendation: include ONE minimal live check — call `world_state()` with default options (screenshot + window_list, no UIA) plus once more with `UiaMode::Foreground` against a real Notepad-class target (reusing the exact live-test fixture pattern from `tests/live_session.rs`), and report the measured `capture_span` for the record. This is a small, low-risk addition to the existing gated-live-test file, not a new live-test harness. The bulk of Phase 8 (lint gates, Serialize derives, `WorldStateOptions`/`WorldState` types, the `world_state()` sequencing logic and its Foreground/AllTopLevel branching) is fully verifiable offline via the existing `test_session_with_channel`/`test_session_with_sensor` mock-`Session`-construction pattern already used throughout `session.rs`'s `#[cfg(test)] mod tests` (e.g. lines 1029, 1196) — no live target is needed to exercise the sequencing logic itself, only to confirm the real-world span measurement.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain `stable-x86_64-pc-windows-gnu` (repo-pinned, `rust-toolchain.toml`) | The project's canonical build target | ✗ (this research host) | — | Use `RUSTUP_TOOLCHAIN=stable cargo build/test/clippy --target x86_64-unknown-linux-gnu` — VERIFIED working in this session (88/88 lib unit tests pass, `cargo clippy` runs clean modulo 2 pre-existing unrelated warnings). Valid substitution per the established project pattern (STATE.md: "06-01: Offline verification run against the native x86_64-unknown-linux-gnu target... the crate has no cfg(windows) code so this is a safe substitute" — re-confirmed in this session: `grep -rn "cfg(windows)\|cfg(target_os" crates/rdpilot/src/` returns zero matches). |
| `serde`/`serde_json` | `Serialize` derives, `WorldState` JSON output | ✓ | 1.0.228 / 1.0.150(-ish, pinned separately) | — |
| A live disposable Azure Windows VM (`infra/manage-env.ps1 up`) | The optional minimal live check (Open Question 1) | Not probed in this research session (no VM currently up per STATE.md — "VM torn down after the run") | — | If unavailable at execution time, the offline-verifiable 3 of 4 success criteria (SC#1/SC#3/SC#4) still close cleanly; SC#2's live span measurement can be deferred to a follow-up live-gate run using the established `manage-env.ps1 up` infra, exactly as every prior phase did. |

**Missing dependencies with no fallback:** none.

**Missing dependencies with fallback:** the pinned Windows-gnu toolchain (fallback: native Linux target, already proven safe and used by every offline plan since Phase 6); a currently-provisioned live VM (fallback: defer the live check to a dedicated end-of-phase live-gate wave, matching every prior phase's structure).

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust's built-in `#[test]` harness via `cargo test` (no external test framework) |
| Config file | none — `crates/rdpilot/Cargo.toml` `[dev-dependencies]` (`tokio` macros/rt-multi-thread, `serial_test`) is the only test-related config |
| Quick run command | `RUSTUP_TOOLCHAIN=stable cargo test -p rdpilot --target x86_64-unknown-linux-gnu --lib` (offline, ~3s, currently 88/88 passing — VERIFIED this session) |
| Full suite command | Quick run above + `cargo test -p rdpilot --test live_session -- --ignored --test-threads=1` gated behind `RDPILOT_LIVE=1` (requires a provisioned Azure VM per `infra/manage-env.ps1 up`) |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| API-01 | No `unsafe`/`unwrap`/`expect` in library code; no IronRDP/sensor-protocol type leaks in `pub` signatures | static analysis (clippy gate) | `cargo clippy -p rdpilot --lib -- -D clippy::unwrap_used -D clippy::expect_used -D unsafe_code` | ✅ mechanism exists (clippy itself); ❌ the `lib.rs` `#![deny(...)]` attributes that make this automatic on every `cargo clippy`/some CI invocation still need to be added — Wave 0/1 gap |
| API-01 | `WorldStateOptions`/`WorldState`/`UiaMode` construction and `world_state()` sequencing (offline, mocked sensor) | unit | new tests in `session.rs`'s existing `#[cfg(test)] mod tests`, using the established `test_session_with_sensor`/`test_session_with_channel` mock pattern (lines 1029/1196) | ❌ Wave 0 gap — new test file/module additions needed, but the mocking INFRASTRUCTURE these tests need already exists and needs no new fixture work |
| API-01 | Owned types serialize via `serde_json::to_string` without error, `Screenshot` output omits `rgba` | unit | new test asserting `serde_json::to_value(&screenshot)` has no `"rgba"` key and has `"width"`/`"height"` | ❌ Wave 0 gap — straightforward, no new fixtures needed |
| API-02 | `world_state()`'s default options yield `screenshot: Some`, `window_list: Some`, `uia: None` | unit | new test in `session.rs` | ❌ Wave 0 gap |
| API-02 | `Foreground`/`AllTopLevel` fetch the window list internally even when `window_list: false` (Pitfall 3) | unit | new test asserting `WorldState.window_list == None` while `WorldState.uia` is populated, given `WorldStateOptions { window_list: false, uia: UiaMode::Foreground, .. }` against a mocked sensor | ❌ Wave 0 gap — this is the single most important new offline test given Pitfall 3's risk |
| API-02 (SC#2) | Measured `capture_span` is reported (not hard-failed) even when a component is artificially slow | unit (offline, using a mock sensor responder with an injected delay) OR live (real 500ms empirical measurement) | offline: assert the call still returns `Ok` and `capture_span` reflects elapsed time even past 500ms with a mocked slow responder; live: real measurement against a real target, as in every prior phase | ❌ Wave 0 gap (offline variant); live variant per Open Question 1's recommendation |

### Sampling Rate

- **Per task commit:** `RUSTUP_TOOLCHAIN=stable cargo test -p rdpilot --target x86_64-unknown-linux-gnu --lib` (fast, offline, ~3s baseline)
- **Per wave merge:** same command + `cargo clippy -p rdpilot --lib --target x86_64-unknown-linux-gnu -- -D clippy::unwrap_used -D clippy::expect_used -D unsafe_code` (or simply `cargo clippy` once the `lib.rs` gates are in place, since they'll then be enforced automatically)
- **Phase gate:** full lib test suite + clippy gate green offline; the optional minimal live check (Open Question 1) if scoped, before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] New unit tests for `WorldStateOptions::default()` / `UiaMode::default()` — trivial, no new fixtures
- [ ] New unit tests for `Session::world_state()`'s sequencing logic (default options, `window_list: false` + `Foreground`, `AllTopLevel` with 2+ mocked windows) — reuses the EXISTING `test_session_with_sensor` mock pattern (session.rs line 1196), no new test infrastructure needed, just new test functions
- [ ] New unit tests asserting `Screenshot`/`WindowInfo`/`WindowState`/`ProcessInfo`/`UiaElement`/`WorldState` round-trip through `serde_json::to_value` with the expected shape (dims-only `Screenshot`, lowercase `WindowState` strings)
- [ ] `lib.rs` `#![deny(unsafe_code)]` / `#![deny(clippy::unwrap_used)]` / `#![deny(clippy::expect_used)]` attributes themselves are the "test" for API-01/SC#4 — no separate test file, just the attribute addition + a clean `cargo clippy` run
- [ ] Framework install: none — `cargo test`/`cargo clippy` are already fully functional in this repo (VERIFIED this session)

## Security Domain

`security_enforcement: true`, `security_asvs_level: 1` (`.planning/config.json`).

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | Out of scope — Phase 8 adds no new auth surface; NLA/CredSSP auth is unchanged from Phase 2 |
| V3 Session Management | No | `Session` lifecycle is unchanged; `world_state()` is a read-only composite query over an already-established session |
| V4 Access Control | No | No new access-control surface — same single-consumer-process trust model as every prior phase |
| V5 Input Validation | Yes (existing, unchanged) | `world_state()`'s `Hwnd(Vec<u64>)`/`AllTopLevel` modes pass hwnd values into the EXISTING `get_uia_tree(hwnd)` call, which already validates via the sensor's own `SensorRejected`/`Error::Dvc` error paths (D-6.4) — no new validation code is needed, this phase reuses the existing contract verbatim |
| V6 Cryptography | No | No new cryptographic material introduced |

No new ASVS category becomes newly applicable in this phase — Phase 8 is additive composition + compiler-enforced hardening of an already-established, already-audited API surface, not a new attack surface.

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Oversized/unbounded JSON payload if a consumer serializes `WorldState` including raw pixel data | Denial of Service (resource exhaustion in a downstream consumer, e.g. an LLM context window) | Already mitigated by design: `Screenshot::rgba` is `#[serde(skip)]`'d (D-8.4's explicit "context-bloat guard") — the dims-only `Screenshot` in `WorldState`'s JSON output keeps the payload lean regardless of desktop resolution |
| Information disclosure via serialized window titles/process paths/UIA element names reaching an untrusted downstream sink (e.g. a future MCP/CLI consumer forwarding `WorldState` JSON to a remote LLM) | Information Disclosure | Out of scope for this phase (the v2 CLI/MCP consumer that would introduce this data flow is explicitly deferred) — flagged here only as a forward-looking note for whoever plans that v2 phase, not an action item for Phase 8 |

## Sources

### Primary (HIGH confidence — compiled/executed in this session against this repo's exact pinned dependencies)

- `/home/marc/dev/rdpilot/crates/rdpilot/src/lib.rs` — public re-export boundary (D-09), read directly
- `/home/marc/dev/rdpilot/crates/rdpilot/src/session.rs` (1538 lines, read in full) — all public `Session` methods, `sensor_request()` helper (lines 507-566), `ping()` (441-485), `get_window_list()`/`get_process_tree()`/`get_uia_tree()` (583-651), test-mock construction pattern (`test_session_with_channel` line 1029, `test_session_with_sensor` line 1196)
- `/home/marc/dev/rdpilot/crates/rdpilot/src/screenshot.rs` (read in full) — `Screenshot`/`Rect` shapes, `to_png()`/`crop()`
- `/home/marc/dev/rdpilot/crates/rdpilot/src/perception.rs` (read in full) — `WindowInfo`/`WindowState`/`ProcessInfo`/`UiaElement` owned types + `*Wire` split, existing `#[serde(rename_all = "lowercase")]` convention on `WindowStateWire`
- `/home/marc/dev/rdpilot/crates/rdpilot/src/error.rs` (read in full) — `Error` variants, `#[non_exhaustive]`, `thiserror`
- `/home/marc/dev/rdpilot/crates/rdpilot/Cargo.toml` + `/home/marc/dev/rdpilot/Cargo.lock` (grepped) — confirmed `serde 1.0.228` already resolved, `derive` feature already enabled, no `chrono`/`time` crate present, no `[lints]` table present
- `cargo clippy -p rdpilot --target x86_64-unknown-linux-gnu -- -D clippy::unwrap_used -D clippy::expect_used -D unsafe_code` (library target) — run live in this session, 0 violations, 2 pre-existing unrelated warnings
- `cargo clippy -p rdpilot --target x86_64-unknown-linux-gnu --all-targets -- -D clippy::unwrap_used -D clippy::expect_used -D unsafe_code` — run live in this session, 116 errors in `tests/live_session.rs`, proving `[lints]`-table scoping would break the live-test suite
- A minimal standalone probe crate compiled against `serde 1.0.228`/`serde_json` (this session, in scratchpad) proving `SystemTime`/`Duration` derive `Serialize` natively and `Instant` does not (compile error reproduced)
- `cargo test -p rdpilot --target x86_64-unknown-linux-gnu --lib` — run live in this session, 88/88 passing
- `grep -rn "cfg(windows)\|cfg(target_os" crates/rdpilot/src/` — 0 matches, confirming the safe offline-Linux-target substitution claim
- `.planning/phases/08-public-sdk-api-worldstate/08-CONTEXT.md`, `.planning/ROADMAP.md` §Phase 8, `.planning/REQUIREMENTS.md`, `.planning/STATE.md` — read via PerficioRead

### Secondary (MEDIUM confidence)

- None needed — every claim in this research was directly verifiable against the local repo and toolchain; no WebSearch/Context7 lookups were required for this phase's scope (it is entirely stdlib/serde/existing-crate composition, not new third-party API surface).

### Tertiary (LOW confidence)

- None.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — zero new dependencies; `serde`/stdlib behavior directly compiled and verified against this repo's exact pinned versions
- Architecture: HIGH — `WorldStateOptions`/`WorldState`/`UiaMode` shapes are read directly against existing owned types (`WindowInfo`, `UiaElement`, `Rect`) already present in the repo; the Foreground-mode heuristic reuses an already-live-verified Phase 6 pattern (documented in STATE.md, not invented here)
- Pitfalls: HIGH — both headline pitfalls (lint-gate scoping, `Instant` vs `Serialize`) were reproduced by actually running the failing case in this session, not inferred

**Research date:** 2026-07-09
**Valid until:** stable indefinitely for the offline portions (no external API surface involved); the live-check recommendation (Open Question 1) has the same "reason offline, live-tune empirically" validity window as every prior phase's live gate (re-verify against a fresh VM at execution time, per the project's established pattern).
