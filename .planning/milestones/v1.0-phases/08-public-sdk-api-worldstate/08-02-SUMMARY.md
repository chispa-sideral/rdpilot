---
phase: 08-public-sdk-api-worldstate
plan: 02
subsystem: sdk-public-api
tags: [worldstate, session, serde, offline-tests]
dependency graph:
  requires: [08-01]
  provides: [WorldStateOptions, UiaMode, WorldState, Session::world_state]
  affects: [09-scripted-proof-harness]
tech-stack:
  added: []
  patterns:
    - "a-la-carte options struct (bool flags + mode enum) with an SC#2-compliant manual Default impl"
    - "SystemTime + Duration for a serializable batch timestamp/span (Instant deliberately avoided -- no Serialize impl)"
    - "grouped Vec<(hwnd, Vec<UiaElement>)> to keep per-window RuntimeId namespaces unambiguous"
    - "single internal window-list fetch shared by window_list surfacing and Foreground/AllTopLevel UIA resolution"
key-files:
  created:
    - crates/rdpilot/src/worldstate.rs
  modified:
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/lib.rs
decisions:
  - "UiaMode::Hwnd carries Vec<u64> (not a single u64) to express D-8.1's 'one or a set' as one variant"
  - "Foreground selection is titled-only then minimum z_order, reusing the Phase 6 live-diagnosed heuristic verbatim"
  - "capture_span is never compared against 500ms in code -- the bound is documented on the field and checked at the Plan 03 live gate, per D-8.2"
metrics:
  duration: "~20 min"
  completed: 2026-07-09
---

# Phase 8 Plan 2: WorldState composite snapshot Summary

Added the crate's first composite/aggregating `Session` method — `world_state()` — which
sequences the existing client-side screenshot capture and 0–N sensor round trips (window list,
optional per-window UIA trees) into one correlated, timestamped, serializable `WorldState`
snapshot, with a-la-carte `WorldStateOptions`/`UiaMode` selection (D-8.1) and only owned SDK
types crossing the public boundary (SC#1).

## What Was Built

**Task 1 — `crates/rdpilot/src/worldstate.rs` (new module)**
- `UiaMode` enum: `None` (`#[default]`), `Foreground`, `Hwnd(Vec<u64>)`, `AllTopLevel`.
- `WorldStateOptions { screenshot: bool, window_list: bool, uia: UiaMode }` with a manual
  `Default` impl returning `screenshot: true, window_list: true, uia: UiaMode::None` — SC#2-
  compliant out of the box.
- `WorldState { timestamp: SystemTime, capture_span: Duration, screenshot: Option<Screenshot>,
  window_list: Option<Vec<WindowInfo>>, uia: Option<Vec<(u64, Vec<UiaElement>)>> }`, deriving
  `Debug, Clone, serde::Serialize`. `SystemTime`/`Duration` were used (not `Instant`, which has
  no `Serialize` impl — RESEARCH Pitfall 2); `uia` is grouped by hwnd (not a flat merge) so each
  window's independent `RuntimeId` namespace never collides.
- `lib.rs`: added `mod worldstate;` and `pub use worldstate::{UiaMode, WorldState,
  WorldStateOptions};`; extended the D-09 boundary comment to list the three new owned types and
  documented that `worldstate` stays a private `mod` (matching the existing `screenshot` pattern).
- No new dependency — `Cargo.toml` untouched.

**Task 2 — `Session::world_state()` in `session.rs`**
- `pub async fn world_state(&self, opts: WorldStateOptions) -> Result<WorldState>`, placed
  immediately after `get_uia_tree`.
- Sequencing: local `std::time::Instant::now()` span start (never stored in `WorldState` itself)
  → optional `self.screenshot().await?` → `needs_list_internally = opts.window_list ||
  matches!(opts.uia, UiaMode::Foreground | UiaMode::AllTopLevel)` → at most one
  `self.get_window_list().await?` fetch when needed → `uia` resolved per mode (`Hwnd` loops one
  `get_uia_tree` call per handle; `Foreground` filters to non-empty-title windows then picks the
  minimum `z_order`, falling back to an empty group if none are titled; `AllTopLevel` walks every
  window in list order) → `WorldState` constructed with `timestamp: SystemTime::now()`,
  `capture_span: started.elapsed()`, and `window_list` populated **only** when `opts.window_list`
  was true (even though the list may have been fetched internally for `Foreground`/`AllTopLevel`,
  RESEARCH Pitfall 3).
- Every sequenced component call (`screenshot`, `get_window_list`, `get_uia_tree`) is propagated
  with `?` — no error is ever downgraded to a `None` field (RESEARCH Pitfall 4: the 500ms bound is
  a soft, best-effort timing signal on the returned `capture_span`, checked only at the Plan 03
  live gate; it is never compared in code and never causes a hard failure by itself).
- The two `windows.as_ref()` matches for `Foreground`/`AllTopLevel` handle the (unreachable in
  practice, since `needs_list_internally` guarantees a fetch) `None` case by returning an empty
  group rather than `unwrap`/`expect`, honoring the crate's `#![deny(clippy::unwrap_used,
  clippy::expect_used)]` gates.
- Five offline unit tests added to the existing `#[cfg(test)] mod tests`, reusing
  `test_session_with_sensor` and a new small `drive_one_request`/`canned_window_list_reply`/
  `canned_uia_reply` helper trio to keep the multi-request tests readable:
  1. `world_state_default_options_returns_screenshot_and_window_list_no_uia` — default options
     yield `screenshot: Some`, `window_list: Some`, `uia: None`.
  2. `world_state_foreground_uia_fetches_list_internally_but_hides_it` (Pitfall 3) —
     `{screenshot:false, window_list:false, uia:Foreground}` against an untitled topmost window
     plus two titled windows: `window_list` stays `None`, the `Uia` request targets the titled
     window with the minimum `z_order` (222), proving both the internal-fetch and the
     titled-only-then-min-z_order selection.
  3. `world_state_all_top_level_uia_covers_every_window_in_order` — `AllTopLevel` with 2 windows
     issues one `Uia` request per window in list order and reuses the single internally-fetched
     list to also populate `window_list` (requested in this test).
  4. `world_state_hwnd_uia_fetches_group_per_handle_without_list` — `Hwnd(vec![111, 222])` issues
     no `WindowList` request at all, one `Uia` request per handle in order, `window_list: None`.
  5. `world_state_error_propagates_as_err_not_none` (Pitfall 4) — a `success:false` reply on the
     internally-fetched `WindowList` request under default options makes `world_state()` return
     `Err(Error::SensorRejected)`, not `Ok` with a `None` field.

## Verification

- `RUSTUP_TOOLCHAIN=stable cargo build -p rdpilot --target x86_64-unknown-linux-gnu` — clean
  (one pre-existing, out-of-scope warning: unused `crate::error::Error` import in `input.rs`,
  Phase 5).
- `RUSTUP_TOOLCHAIN=stable cargo test -p rdpilot --target x86_64-unknown-linux-gnu --lib` —
  **99/99 tests pass**, including all 5 new `world_state` tests.
- `RUSTUP_TOOLCHAIN=stable cargo clippy -p rdpilot --lib --target x86_64-unknown-linux-gnu` —
  **exits 0** under the Plan 01 deny gates (`unsafe_code`, `clippy::unwrap_used`,
  `clippy::expect_used`). Two remaining warnings are pre-existing and unrelated to this plan's
  files (`input.rs` unused import from Phase 5; `rdpdr_backend.rs` `unnecessary_get_then_check`)
  — confirmed via `grep` that neither warning references `worldstate.rs` or the new `session.rs`
  code.
- SC#1: `world_state()`/`WorldState` expose only owned types (`Screenshot`, `WindowInfo`,
  `UiaElement`, `SystemTime`, `Duration`) — no `ironrdp`/`image`/sensor-wire type in the signature.
- SC#2: default-options offline test confirms screenshot + window list + no UIA, with a measured
  `capture_span`; the real "within 500ms" measurement is deferred to the Plan 03 live gate, per
  plan `<verification>`.
- SC#3: by construction — `WorldState` reuses `crate::Rect`/`Screenshot`/`WindowInfo`/`UiaElement`
  verbatim; no scale factor introduced anywhere in this plan.
- Pitfall 3 and Pitfall 4 are both directly exercised by dedicated tests (see list above).

## Deviations from Plan

None — plan executed exactly as written. The two `windows.as_ref() => None` fallback arms (empty
group rather than `unwrap`/`expect`) are a direct, in-scope consequence of the crate's existing
`#![deny(clippy::unwrap_used, clippy::expect_used)]` gates (Plan 01) applied to this plan's new
code — not a deviation from the plan's own instructions, which explicitly called for avoiding
panics.

## Known Stubs

None. Every field `world_state()` can produce is wired to a real (mocked-in-tests) sensor/
screenshot code path; no hardcoded empty/placeholder value flows to a public field.

## Threat Flags

None. This plan composes existing, already-threat-modeled `Session` methods
(`screenshot`/`get_window_list`/`get_uia_tree`) client-side; it introduces no new network
endpoint, auth path, file access pattern, or schema at a trust boundary beyond what the plan's own
`<threat_model>` (T-08-01/T-08-02/T-08-04, all `accept`/`mitigate`-by-inheritance) already covers.

## Self-Check: PASSED

- FOUND: crates/rdpilot/src/worldstate.rs
- FOUND: crates/rdpilot/src/session.rs (world_state method + 5 new tests)
- FOUND: crates/rdpilot/src/lib.rs (mod worldstate + pub use)
- FOUND commit fba4473 (Task 1: worldstate.rs + lib.rs)
- FOUND commit 5b6861c (Task 2: Session::world_state + tests)
