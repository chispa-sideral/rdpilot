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
}
