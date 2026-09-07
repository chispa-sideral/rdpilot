//! [`WorldStateOptions`] / [`UiaMode`] / [`WorldState`] — the composite,
//! a-la-carte snapshot type produced by [`crate::Session::world_state`]
//! (API-02, D-8.1).
//!
//! `WorldState` correlates the client-side desktop screenshot with 0-N
//! sensor round trips (window list, optional UIA trees) under a single
//! batch timestamp and one measured capture span (SC#2), reusing the same
//! owned SDK types (`crate::Screenshot`, `crate::WindowInfo`,
//! `crate::UiaElement`) that already share one physical virtual-desktop
//! pixel space via `crate::Rect` — no new coordinate system, no silent
//! scaling (SC#3, D-8.3). Only owned SDK types appear here — no `ironrdp`,
//! `image`, or sensor-wire type ever crosses this module's public surface
//! (SC#1, D-09).

use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::{Screenshot, UiaElement, WindowInfo};

/// Which UI Automation tree(s), if any, [`crate::Session::world_state`]
/// should fetch (D-8.1).
///
/// `Foreground` and `AllTopLevel` both require the top-level window list —
/// `world_state()` fetches it internally for these two modes even when the
/// caller did not also request `WorldStateOptions::window_list`, but never
/// surfaces it in the returned [`WorldState`] unless the caller asked for it
/// (RESEARCH Pitfall 3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UiaMode {
    /// Fetch no UIA tree at all. The default — a plain `WorldStateOptions`
    /// stays SC#2-compliant (screenshot + window list, no UIA) without the
    /// caller paying for any UIA round trip.
    #[default]
    None,
    /// Fetch the UIA tree of only the current foreground window, selected
    /// as the minimum-`z_order` window among windows with a non-empty
    /// title (the Phase 6 live-diagnosed foreground heuristic: titled-only,
    /// since always-on-top shell chrome with no title outranks real app
    /// windows in raw z-order). If no titled window exists, yields an empty
    /// group list.
    Foreground,
    /// Fetch the UIA tree for each of the given window handles, one sensor
    /// round trip per handle, in the given order. A `Vec<u64>` (rather than
    /// a single `u64`) expresses D-8.1's "one or a set" as a single
    /// variant.
    Hwnd(Vec<u64>),
    /// Fetch the UIA tree for every top-level window currently listed, in
    /// list order. May be slow on a desktop with many windows (each window
    /// is its own sensor round trip) — degrades gracefully per D-8.2
    /// (best-effort timing, never a hard failure).
    AllTopLevel,
}

/// A-la-carte selection of which components
/// [`crate::Session::world_state`] should capture (D-8.1). Lets a consumer
/// request only what it needs — e.g. skipping the screenshot to keep a
/// structured-only payload lean for a downstream agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldStateOptions {
    /// Whether to capture a full-desktop screenshot (client-side, no
    /// sensor round trip — see [`crate::Session::screenshot`]).
    pub screenshot: bool,
    /// Whether to include the top-level window list in the returned
    /// [`WorldState`]. Note: the list may still be fetched internally even
    /// when this is `false`, if `uia` is [`UiaMode::Foreground`] or
    /// [`UiaMode::AllTopLevel`] (RESEARCH Pitfall 3) — it just will not be
    /// surfaced on the result unless this flag is `true`.
    pub window_list: bool,
    /// Which UIA tree(s), if any, to fetch.
    pub uia: UiaMode,
    /// Whether to fetch and surface `elevation_active` (a session-scoped
    /// UAC/elevation consent-prompt detection) — opt-in, a-la-carte:
    /// `world_state()` does not fetch the process tree
    /// at all unless this is `true`, preserving the existing SC#2 default
    /// cheapness for every caller that does not ask for it.
    pub elevation_check: bool,
}

impl Default for WorldStateOptions {
    /// `screenshot: true, window_list: true, uia: UiaMode::None,
    /// elevation_check: false` — SC#2 compliant out of the box: a plain
    /// `WorldStateOptions::default()` yields a screenshot and a window
    /// list with no UIA and no extra process-tree round trip (D-8.1).
    fn default() -> Self {
        Self {
            screenshot: true,
            window_list: true,
            uia: UiaMode::default(),
            elevation_check: false,
        }
    }
}

/// A correlated, timestamped snapshot of the remote desktop (API-02, SC#2):
/// a screenshot, a window list, and/or grouped UIA trees captured under one
/// batch [`WorldState::timestamp`] and one measured
/// [`WorldState::capture_span`].
///
/// # Coordinate space (SC#3, D-8.3)
///
/// Every coordinate carried here — [`Screenshot`] dimensions, each
/// [`WindowInfo::rect`], each [`UiaElement::bbox`] — is the single shared
/// `crate::Rect` physical virtual-desktop pixel space already used
/// end-to-end by the rest of the SDK. There is no silent scaling and no
/// second (logical/DPI) coordinate system here.
///
/// # Timing (D-8.2)
///
/// [`WorldState::capture_span`] is the measured wall-clock elapsed across
/// the sequenced component fetches. SC#2's "within 500 ms of each other" is
/// a BEST-EFFORT bound: exceeding it surfaces the measured elapsed value
/// but does NOT make `world_state()` return `Err` — only a genuine
/// transport/semantic error on a sequenced component call
/// ([`crate::Error::Dvc`] / [`crate::Error::SensorRejected`]) does that
/// (RESEARCH Pitfall 4). Timing is soft; correctness errors are hard.
#[derive(Debug, Clone, Serialize)]
pub struct WorldState {
    /// The batch timestamp taken once, after all sequenced component
    /// fetches complete (wall-clock, `SystemTime` — chosen over `Instant`
    /// because `Instant` has no `Serialize` impl and would block this
    /// derive; RESEARCH Pitfall 2).
    pub timestamp: SystemTime,
    /// The measured wall-clock elapsed across the sequenced component
    /// fetches (D-8.2) — best-effort against the SC#2 500 ms bound, never a
    /// hard-fail condition by itself.
    pub capture_span: Duration,
    /// The full-desktop screenshot, if [`WorldStateOptions::screenshot`]
    /// was requested.
    pub screenshot: Option<Screenshot>,
    /// The top-level window list, if [`WorldStateOptions::window_list`]
    /// was requested. May be `None` even though the list was fetched
    /// internally to resolve [`UiaMode::Foreground`]/[`UiaMode::AllTopLevel`]
    /// (RESEARCH Pitfall 3) — this field reflects only what the caller
    /// asked to see.
    pub window_list: Option<Vec<WindowInfo>>,
    /// UIA trees grouped by originating window handle, if
    /// [`WorldStateOptions::uia`] requested any. Grouped as
    /// `Vec<(hwnd, Vec<UiaElement>)>` rather than a flat
    /// `Vec<UiaElement>`: each window's `UiaElement::parent_id` `RuntimeId`
    /// namespace is independent, so a flat merge across
    /// [`UiaMode::AllTopLevel`]/multi-[`UiaMode::Hwnd`] would make
    /// `parent_id` ambiguous (RESEARCH Pattern 2 / Alternatives).
    pub uia: Option<Vec<(u64, Vec<UiaElement>)>>,
    /// Whether a UAC/elevation consent prompt is active in this session
    /// (session-scoped, structural detection — see
    /// [`crate::perception::elevation_prompt_active`]), if
    /// [`WorldStateOptions::elevation_check`] was requested. `None` when
    /// not requested, OR when the underlying `get_process_tree()` fetch
    /// itself failed with [`crate::Error::Dvc`] — a stuck sensor degrades
    /// this one field rather than failing an otherwise-successful
    /// screenshot/window-list/UIA snapshot (fail-open, the same safety-net
    /// principle applied to `world_state` too). Any OTHER error from that
    /// fetch still propagates via `?` like every other component.
    pub elevation_active: Option<bool>,
}
