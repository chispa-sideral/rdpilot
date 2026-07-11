//! The `computer` mega-tool (D-21/D-14.2, MCP-02/MCP-04): a single
//! Anthropic `computer_20250124`-compatible schema-discriminated action
//! enum, the pure [`scale::scale_to_native`] coordinate bridge, and the
//! action -> `Request` dispatch table ([`dispatch`]).
//!
//! [`ComputerAction`] mirrors the Anthropic `computer_20250124` action
//! set's field names VERBATIM (Pitfall 3) so a computer-use-trained
//! model's learned tool-call conventions transfer directly. This server
//! defines its OWN JSON schema for the tool (it is not literally
//! Anthropic's built-in `computer` tool type), so it is free to — and
//! does, per D-29/Pitfall 4 — add an extra required `session` field
//! ([`ComputerArgs`]) the Anthropic reference schema has no concept of.

pub mod dispatch;
pub mod scale;

// ADVERTISED_HEIGHT/ADVERTISED_WIDTH are re-exported for API completeness
// (this module's own doc comment; the artifact surface this plan commits
// to) even though nothing inside this crate currently reads them by this
// path — `computer/dispatch.rs` reaches `scale_to_native` the same way,
// and the BLOCKING `tests/scale_to_native.rs` integration test exercises
// `computer/scale.rs`'s own constants directly (bin-only crate, no `[lib]`
// target — see that test file's own doc comment).
#[allow(unused_imports)]
pub use scale::{ADVERTISED_HEIGHT, ADVERTISED_WIDTH, scale_to_native};

use schemars::JsonSchema;
use serde::Deserialize;

/// One `computer_20250124` action. Internally tagged on `action`
/// (`#[serde(tag = "action")]`), snake_case discriminants
/// (`rename_all = "snake_case"`) — an unrecognized `action` value is a
/// hard `serde` deserialize error, never a silent fallback.
///
/// Field names mirror Anthropic's `computer_20250124` action schema
/// verbatim (Pitfall 3): `coordinate`, `start_coordinate`, `text`,
/// `scroll_direction`, `scroll_amount`, `duration`.
// Several variants below carry a `text`/`coordinate` field that this
// plan's dispatch deliberately does not read (modifier-key `text` on
// clicks is accepted for Anthropic field-name parity but not yet wired,
// research Open Questions; `LeftMouseDown`/`LeftMouseUp`'s `coordinate` is
// captured for schema completeness even though the action itself is an
// explicit rejection, T-14-10) — never dead in the sense of "unreachable",
// just not consumed by THIS plan's mapping. Mirrors `connect.rs`'s
// "declared now, not yet consumed" `#[allow(dead_code)]` convention.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ComputerAction {
    /// Capture a screenshot of the remote desktop.
    Screenshot,
    /// Move the pointer to `coordinate` with no button state change.
    MouseMove {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
    },
    /// Move to `coordinate` then click the left (primary) button.
    LeftClick {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
        /// Optional modifier-key combo held during the click (e.g.
        /// `"shift"`). Accepted by the schema for field-name parity with
        /// Anthropic's reference (Pitfall 3/A2); NOT currently wired to a
        /// `WireMouseAction` equivalent (research Open Questions — no
        /// direct SDK primitive), so it is presently a no-op.
        text: Option<String>,
    },
    /// Move to `coordinate` then click the right (secondary) button.
    RightClick {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
        /// See [`ComputerAction::LeftClick::text`] — presently a no-op.
        text: Option<String>,
    },
    /// Move to `coordinate` then click the middle (wheel) button.
    MiddleClick {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
        /// See [`ComputerAction::LeftClick::text`] — presently a no-op.
        text: Option<String>,
    },
    /// Move to `coordinate` then double-click the left button.
    DoubleClick {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
        /// See [`ComputerAction::LeftClick::text`] — presently a no-op.
        text: Option<String>,
    },
    /// Move to `coordinate` then click the left button three times.
    ///
    /// No atomic wire primitive: dispatched as THREE sequential `Click`
    /// round trips, ~100ms apart (research mapping table).
    TripleClick {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
        /// See [`ComputerAction::LeftClick::text`] — presently a no-op.
        text: Option<String>,
    },
    /// Press the left button at `start_coordinate`, move to `coordinate`,
    /// then release — a `WireMouseAction::Drag`. Both endpoints are scaled
    /// through `scale_to_native` independently.
    LeftClickDrag {
        /// The drag's origin, `[x, y]` in the advertised 1280x800 space.
        start_coordinate: [u32; 2],
        /// The drag's destination, `[x, y]` in the advertised 1280x800
        /// space.
        coordinate: [u32; 2],
    },
    /// Press (but do not release) the left button at `coordinate`.
    ///
    /// **UNSUPPORTED — GAP:** `WireMouseAction` has no press-only variant.
    /// Explicitly rejected (never a silent no-op); [`LeftClickDrag`] is the
    /// documented alternative.
    ///
    /// [`LeftClickDrag`]: ComputerAction::LeftClickDrag
    LeftMouseDown {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
    },
    /// Release a previously pressed left button at `coordinate`.
    ///
    /// **UNSUPPORTED — GAP:** see [`LeftMouseDown`].
    ///
    /// [`LeftMouseDown`]: ComputerAction::LeftMouseDown
    LeftMouseUp {
        /// `[x, y]` in the advertised 1280x800 space.
        coordinate: [u32; 2],
    },
    /// Press a key combination (e.g. `"ctrl+s"`, `"alt+Tab"`, `"Return"`).
    Key {
        /// A `"+"`-separated key-name combo, parsed via
        /// `rdpilot_ipc::parse_wire_key`.
        text: String,
    },
    /// Type literal text, one Unicode code point at a time.
    Type {
        /// The literal text to type.
        text: String,
    },
    /// Scroll at `coordinate` in `scroll_direction` by `scroll_amount`
    /// notches. `up`/`down` map to `WireMouseAction::Scroll`; `left`/
    /// `right` is an **UNSUPPORTED — GAP** (rdpilot's `Scroll` wire verb
    /// carries only vertical `dy`, D-3.3).
    Scroll {
        /// `[x, y]` in the advertised 1280x800 space. Required in
        /// practice by this server's dispatch (no cursor-position
        /// tracking exists to fall back on — see [`CursorPosition`]'s own
        /// gap), even though the schema marks it optional for field-name
        /// parity with Anthropic's reference.
        ///
        /// [`CursorPosition`]: ComputerAction::CursorPosition
        coordinate: Option<[u32; 2]>,
        /// Which direction to scroll.
        scroll_direction: ScrollDirection,
        /// How many wheel notches to scroll.
        scroll_amount: u32,
        /// Optional modifier-key combo — see
        /// [`ComputerAction::LeftClick::text`], presently a no-op.
        text: Option<String>,
    },
    /// Press-and-hold a key combination for `duration` seconds.
    ///
    /// No atomic SDK "hold for N seconds" primitive exists (research A3):
    /// approximated as an atomic `WireKeyAction::Combo` press+release — a
    /// documented limitation, NOT a rejection. `duration` is capped at
    /// 100s (Pitfall 5, matching Anthropic's own reference).
    HoldKey {
        /// A `"+"`-separated key-name combo, parsed via
        /// `rdpilot_ipc::parse_wire_key`.
        text: String,
        /// How long to (approximate) holding the combo, in seconds. Capped
        /// at 100s.
        duration: f64,
    },
    /// Sleep for `duration` seconds with no daemon round trip. Capped at
    /// 100s (Pitfall 5).
    Wait {
        /// How long to wait, in seconds. Capped at 100s.
        duration: f64,
    },
    /// Report the pointer's current position.
    ///
    /// **UNSUPPORTED — GAP:** no SDK pointer-position getter exists.
    /// Explicitly rejected; take a `screenshot` to observe the pointer
    /// instead.
    CursorPosition,
}

/// `computer_20250124`'s `scroll_direction` vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScrollDirection {
    /// Scroll away from the user (page content moves down).
    Up,
    /// Scroll toward the user (page content moves up).
    Down,
    /// **UNSUPPORTED — GAP** (D-3.3: rdpilot's `Scroll` wire verb is
    /// vertical-only).
    Left,
    /// **UNSUPPORTED — GAP** (D-3.3: rdpilot's `Scroll` wire verb is
    /// vertical-only).
    Right,
}

/// The `computer` tool's top-level input: `session` (D-29, required on
/// every `rdpilot_*`/`computer` tool call — an extra field beyond
/// Anthropic's own reference schema, legal because this server defines its
/// own schema, Pitfall 4) plus the flattened [`ComputerAction`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ComputerArgs {
    /// The target session id (D-29). Required on every call.
    pub session: String,
    /// The `computer_20250124` action to perform.
    #[serde(flatten)]
    pub action: ComputerAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_deserializes_with_no_extra_fields() {
        let action: ComputerAction = serde_json::from_str(r#"{"action":"screenshot"}"#).unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(action, ComputerAction::Screenshot));
    }

    #[test]
    fn left_click_deserializes_coordinate_and_defaults_text_to_none() {
        let action: ComputerAction = serde_json::from_str(r#"{"action":"left_click","coordinate":[10,20]}"#)
            .unwrap_or_else(|e| panic!("{e}"));
        match action {
            ComputerAction::LeftClick { coordinate, text } => {
                assert_eq!(coordinate, [10, 20]);
                assert_eq!(text, None);
            }
            other => panic!("expected LeftClick, got {other:?}"),
        }
    }

    #[test]
    fn scroll_deserializes_scroll_direction_down() {
        let action: ComputerAction = serde_json::from_str(
            r#"{"action":"scroll","coordinate":[1,2],"scroll_direction":"down","scroll_amount":3}"#,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        match action {
            ComputerAction::Scroll { scroll_direction, .. } => {
                assert_eq!(scroll_direction, ScrollDirection::Down);
            }
            other => panic!("expected Scroll, got {other:?}"),
        }
    }

    #[test]
    fn unknown_action_tag_is_a_hard_deserialize_error() {
        let result: Result<ComputerAction, _> = serde_json::from_str(r#"{"action":"teleport"}"#);
        assert!(result.is_err(), "unknown action tag must hard-reject, never silently fall back");
    }

    #[test]
    fn computer_args_requires_session_alongside_the_flattened_action() {
        let args: ComputerArgs =
            serde_json::from_str(r#"{"session":"brave-otter","action":"screenshot"}"#).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(args.session, "brave-otter");
        assert!(matches!(args.action, ComputerAction::Screenshot));

        let missing_session: Result<ComputerArgs, _> = serde_json::from_str(r#"{"action":"screenshot"}"#);
        assert!(missing_session.is_err(), "session must be required (D-29)");
    }
}
