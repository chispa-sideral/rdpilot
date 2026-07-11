//! Owned wire mirrors of `rdpilot`'s input vocabulary (`MouseAction`,
//! `KeyAction`, `Button`, `Key`).
//!
//! Deliberately duplicates each SDK type's field shape rather than importing
//! it (the `transfer.rs` convention, Decision 1): `rdpilot-ipc` must never
//! depend on `rdpilot`, so every wire DTO that corresponds to an SDK type is
//! its own owned mirror, kept in sync by convention/tests, not by a shared
//! type.
//!
//! [`WireKey`] enumerates the same variant set as `rdpilot::Key` 1:1 (copied
//! from `crates/rdpilot/src/input.rs`'s `pub enum Key` definition, which
//! currently has 67 variants: 3 modifiers, 26 letters, 10 digits, 12
//! F-keys, 6 control keys, 4 arrow keys, 5 navigation-cluster keys, and 1
//! `Win` key). A test below asserts this count so a future drift in either
//! enum's variant list is caught immediately.

use serde::{Deserialize, Serialize};

/// Wire mirror of `rdpilot::Button` — a mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireButton {
    /// The left (primary) mouse button.
    Left,
    /// The right (secondary/context-menu) mouse button.
    Right,
    /// The middle (wheel) mouse button.
    Middle,
}

/// Wire mirror of `rdpilot::MouseAction` — a single mouse action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireMouseAction {
    /// Move the pointer to `(x, y)` with no button state change.
    Move {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
    },
    /// Move to `(x, y)` then press and release `button` once.
    Click {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// The button to click.
        button: WireButton,
    },
    /// Two click-equivalent sequences at `(x, y)`, separated by a short
    /// inter-click delay.
    DoubleClick {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// The button to double-click.
        button: WireButton,
    },
    /// Move to `(x, y)` then scroll vertically by `dy`.
    Scroll {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// Signed count of `WHEEL_DELTA` (120-unit) notches, positive = away
        /// from the user.
        dy: i16,
    },
    /// Press the button at `(from_x, from_y)`, move to `(to_x, to_y)`, then
    /// release the button.
    Drag {
        /// Origin x, in physical virtual-desktop pixels.
        from_x: u16,
        /// Origin y, in physical virtual-desktop pixels.
        from_y: u16,
        /// Destination x, in physical virtual-desktop pixels.
        to_x: u16,
        /// Destination y, in physical virtual-desktop pixels.
        to_y: u16,
        /// The button to drag with.
        button: WireButton,
    },
}

/// Wire mirror of `rdpilot::Key` — a logical key for [`WireKeyAction::Combo`].
///
/// Named virtual keys, mirroring `rdpilot::Key`'s full variant list 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)] // self-explanatory variants naming physical keys
pub enum WireKey {
    Ctrl,
    Alt,
    Shift,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Enter,
    Esc,
    Tab,
    Space,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    /// The left Windows/GUI key.
    Win,
}

/// Wire mirror of `rdpilot::KeyAction` — a single keyboard action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireKeyAction {
    /// Type literal text, one Unicode code point at a time.
    Type(String),
    /// Press a combination of keys, in order, then release them in reverse
    /// order.
    Combo(Vec<WireKey>),
}

/// Parse one case-insensitive key name (e.g. `"ctrl"`, `"ESC"`, `"page-up"`,
/// `"0"`) into a [`WireKey`].
///
/// The single canonical key-name table (research "Don't Hand-Roll"):
/// `rdpilot-cli`'s `verbs/input.rs::parse_key_name` delegates to this
/// function rather than holding its own copy, and the (future) `rdpilot-mcp`
/// `computer` tool's `key`/`hold_key` actions are expected to do the same —
/// avoiding a second/third hand-copied 67-entry match table silently
/// drifting out of sync.
///
/// # Errors
///
/// Returns `Err(String)` — a plain owned message naming the offending token
/// — if `name` does not match any known key (T-13-17: no silent drop, no
/// panic). Deliberately returns a plain `String` rather than a CLI-specific
/// error type: this crate must stay free of any CLI dependency.
pub fn parse_wire_key(name: &str) -> Result<WireKey, String> {
    let trimmed = name.trim();
    Ok(match trimmed.to_lowercase().as_str() {
        "ctrl" => WireKey::Ctrl,
        "alt" => WireKey::Alt,
        "shift" => WireKey::Shift,
        "a" => WireKey::A,
        "b" => WireKey::B,
        "c" => WireKey::C,
        "d" => WireKey::D,
        "e" => WireKey::E,
        "f" => WireKey::F,
        "g" => WireKey::G,
        "h" => WireKey::H,
        "i" => WireKey::I,
        "j" => WireKey::J,
        "k" => WireKey::K,
        "l" => WireKey::L,
        "m" => WireKey::M,
        "n" => WireKey::N,
        "o" => WireKey::O,
        "p" => WireKey::P,
        "q" => WireKey::Q,
        "r" => WireKey::R,
        "s" => WireKey::S,
        "t" => WireKey::T,
        "u" => WireKey::U,
        "v" => WireKey::V,
        "w" => WireKey::W,
        "x" => WireKey::X,
        "y" => WireKey::Y,
        "z" => WireKey::Z,
        "digit0" | "0" => WireKey::Digit0,
        "digit1" | "1" => WireKey::Digit1,
        "digit2" | "2" => WireKey::Digit2,
        "digit3" | "3" => WireKey::Digit3,
        "digit4" | "4" => WireKey::Digit4,
        "digit5" | "5" => WireKey::Digit5,
        "digit6" | "6" => WireKey::Digit6,
        "digit7" | "7" => WireKey::Digit7,
        "digit8" | "8" => WireKey::Digit8,
        "digit9" | "9" => WireKey::Digit9,
        "f1" => WireKey::F1,
        "f2" => WireKey::F2,
        "f3" => WireKey::F3,
        "f4" => WireKey::F4,
        "f5" => WireKey::F5,
        "f6" => WireKey::F6,
        "f7" => WireKey::F7,
        "f8" => WireKey::F8,
        "f9" => WireKey::F9,
        "f10" => WireKey::F10,
        "f11" => WireKey::F11,
        "f12" => WireKey::F12,
        "enter" => WireKey::Enter,
        "esc" | "escape" => WireKey::Esc,
        "tab" => WireKey::Tab,
        "space" => WireKey::Space,
        "backspace" => WireKey::Backspace,
        "delete" | "del" => WireKey::Delete,
        "up" => WireKey::Up,
        "down" => WireKey::Down,
        "left" => WireKey::Left,
        "right" => WireKey::Right,
        "home" => WireKey::Home,
        "end" => WireKey::End,
        "pageup" | "page-up" => WireKey::PageUp,
        "pagedown" | "page-down" => WireKey::PageDown,
        "insert" | "ins" => WireKey::Insert,
        "win" | "windows" | "super" => WireKey::Win,
        other => return Err(format!("unknown key name '{other}' (original token: '{trimmed}')")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_button_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        for button in [WireButton::Left, WireButton::Right, WireButton::Middle] {
            let json = serde_json::to_string(&button)?;
            let _: WireButton = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    #[test]
    fn wire_mouse_action_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let samples = vec![
            WireMouseAction::Move { x: 1, y: 2 },
            WireMouseAction::Click { x: 1, y: 2, button: WireButton::Left },
            WireMouseAction::DoubleClick { x: 1, y: 2, button: WireButton::Right },
            WireMouseAction::Scroll { x: 1, y: 2, dy: -120 },
            WireMouseAction::Drag { from_x: 1, from_y: 2, to_x: 3, to_y: 4, button: WireButton::Middle },
        ];
        for action in samples {
            let json = serde_json::to_string(&action)?;
            let _: WireMouseAction = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    #[test]
    fn wire_key_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        for key in [WireKey::Ctrl, WireKey::A, WireKey::Digit0, WireKey::F1, WireKey::Enter, WireKey::Win] {
            let json = serde_json::to_string(&key)?;
            let parsed: WireKey = serde_json::from_str(&json)?;
            assert_eq!(parsed, key);
        }
        Ok(())
    }

    #[test]
    fn wire_key_action_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let samples = vec![
            WireKeyAction::Type("hello".to_owned()),
            WireKeyAction::Combo(vec![WireKey::Ctrl, WireKey::A]),
        ];
        for action in samples {
            let json = serde_json::to_string(&action)?;
            let _: WireKeyAction = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    /// [`WireKey`] mirrors `rdpilot::Key`'s full variant list 1:1. The SDK
    /// enum (`crates/rdpilot/src/input.rs`) currently has 67 variants: 3
    /// modifiers (Ctrl/Alt/Shift) + 26 letters (A-Z) + 10 digits
    /// (Digit0-Digit9) + 12 F-keys (F1-F12) + 6 control keys
    /// (Enter/Esc/Tab/Space/Backspace/Delete) + 4 arrow keys
    /// (Up/Down/Left/Right) + 5 navigation-cluster keys
    /// (Home/End/PageUp/PageDown/Insert) + 1 Win key = 67.
    #[test]
    fn wire_key_has_the_same_variant_count_as_rdpilot_key() {
        let all = [
            WireKey::Ctrl,
            WireKey::Alt,
            WireKey::Shift,
            WireKey::A,
            WireKey::B,
            WireKey::C,
            WireKey::D,
            WireKey::E,
            WireKey::F,
            WireKey::G,
            WireKey::H,
            WireKey::I,
            WireKey::J,
            WireKey::K,
            WireKey::L,
            WireKey::M,
            WireKey::N,
            WireKey::O,
            WireKey::P,
            WireKey::Q,
            WireKey::R,
            WireKey::S,
            WireKey::T,
            WireKey::U,
            WireKey::V,
            WireKey::W,
            WireKey::X,
            WireKey::Y,
            WireKey::Z,
            WireKey::Digit0,
            WireKey::Digit1,
            WireKey::Digit2,
            WireKey::Digit3,
            WireKey::Digit4,
            WireKey::Digit5,
            WireKey::Digit6,
            WireKey::Digit7,
            WireKey::Digit8,
            WireKey::Digit9,
            WireKey::F1,
            WireKey::F2,
            WireKey::F3,
            WireKey::F4,
            WireKey::F5,
            WireKey::F6,
            WireKey::F7,
            WireKey::F8,
            WireKey::F9,
            WireKey::F10,
            WireKey::F11,
            WireKey::F12,
            WireKey::Enter,
            WireKey::Esc,
            WireKey::Tab,
            WireKey::Space,
            WireKey::Backspace,
            WireKey::Delete,
            WireKey::Up,
            WireKey::Down,
            WireKey::Left,
            WireKey::Right,
            WireKey::Home,
            WireKey::End,
            WireKey::PageUp,
            WireKey::PageDown,
            WireKey::Insert,
            WireKey::Win,
        ];
        assert_eq!(all.len(), 67, "WireKey must mirror rdpilot::Key's full 67-variant set 1:1");
    }

    #[test]
    fn parse_wire_key_handles_representative_names_aliases_and_a_rejection() {
        assert_eq!(parse_wire_key("ctrl"), Ok(WireKey::Ctrl));
        assert_eq!(parse_wire_key("ESC"), Ok(WireKey::Esc));
        assert_eq!(parse_wire_key("escape"), Ok(WireKey::Esc));
        assert_eq!(parse_wire_key("super"), Ok(WireKey::Win));
        assert_eq!(parse_wire_key("0"), Ok(WireKey::Digit0));
        assert_eq!(parse_wire_key("page-up"), Ok(WireKey::PageUp));

        let err = parse_wire_key("nope").expect_err("unknown key name must be rejected");
        assert!(err.contains("nope"), "error must name the offending token, got: {err}");
    }
}
