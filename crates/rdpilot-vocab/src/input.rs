//! Input vocabulary: mouse buttons, logical keys, mouse and key actions, and
//! the raw events of a human viewer.

use serde::{Deserialize, Serialize};

/// A mouse button used by [`MouseAction::Click`], [`MouseAction::DoubleClick`]
/// and [`MouseAction::Drag`].
///
/// The three buttons of the computer-use vocabulary. The browser X buttons
/// belong to [`PointerButton`]. The IPC spelling is the variant name
/// (`"Left"`); the CLI spelling is in [`Button::CLI_VALUES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    /// The left (primary) mouse button.
    Left,
    /// The right (secondary/context-menu) mouse button.
    Right,
    /// The middle (wheel) mouse button.
    Middle,
}

impl Button {
    /// The CLI spelling of each button: the button, its `--button` value and
    /// the help line for that value, in the order the CLI lists them.
    pub const CLI_VALUES: [(Button, &'static str, &'static str); 3] = [
        (Button::Left, "left", "The left (primary) mouse button"),
        (
            Button::Right,
            "right",
            "The right (secondary/context-menu) mouse button",
        ),
        (Button::Middle, "middle", "The middle (wheel) mouse button"),
    ];

    /// The button whose [`CLI_VALUES`](Button::CLI_VALUES) name is `name`.
    /// The match is exact (case-sensitive).
    #[must_use]
    pub fn from_cli_name(name: &str) -> Option<Button> {
        Button::CLI_VALUES
            .iter()
            .find(|(_, cli_name, _)| *cli_name == name)
            .map(|(button, _, _)| *button)
    }
}

/// A logical key for [`KeyAction::Combo`].
///
/// A named virtual key, not a raw scancode. The set covers the computer-use
/// keys: the three modifiers, `A`-`Z`, `Digit0`-`Digit9`, `F1`-`F12`, common
/// control keys, and the arrow and navigation cluster. The IPC spelling is
/// the variant name; [`parse_key_name`] reads the CLI spelling and
/// [`Key::set1_scancode`] gives the keyboard position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)] // self-explanatory variants naming physical keys
pub enum Key {
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
    /// The left Windows/GUI key. A terminal key, not a modifier.
    Win,
}

/// A Scan Code Set 1 keyboard position, as the RDP keyboard input carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set1Scancode {
    /// The Set-1 make code.
    pub code: u8,
    /// Whether the key is an extended (`E0`-prefixed) key.
    pub extended: bool,
}

impl Key {
    /// The IBM PC/AT Scan Code Set 1 position of this key.
    ///
    /// The table is hand-written plain data: no crate maps a named virtual
    /// key to the outgoing Set-1 byte that RDP injection needs.
    #[must_use]
    pub fn set1_scancode(self) -> Set1Scancode {
        let (extended, code) = match self {
            Key::Ctrl => (false, 0x1D),
            Key::Alt => (false, 0x38),
            Key::Shift => (false, 0x2A),
            Key::A => (false, 0x1E),
            Key::B => (false, 0x30),
            Key::C => (false, 0x2E),
            Key::D => (false, 0x20),
            Key::E => (false, 0x12),
            Key::F => (false, 0x21),
            Key::G => (false, 0x22),
            Key::H => (false, 0x23),
            Key::I => (false, 0x17),
            Key::J => (false, 0x24),
            Key::K => (false, 0x25),
            Key::L => (false, 0x26),
            Key::M => (false, 0x32),
            Key::N => (false, 0x31),
            Key::O => (false, 0x18),
            Key::P => (false, 0x19),
            Key::Q => (false, 0x10),
            Key::R => (false, 0x13),
            Key::S => (false, 0x1F),
            Key::T => (false, 0x14),
            Key::U => (false, 0x16),
            Key::V => (false, 0x2F),
            Key::W => (false, 0x11),
            Key::X => (false, 0x2D),
            Key::Y => (false, 0x15),
            Key::Z => (false, 0x2C),
            Key::Digit0 => (false, 0x0B),
            Key::Digit1 => (false, 0x02),
            Key::Digit2 => (false, 0x03),
            Key::Digit3 => (false, 0x04),
            Key::Digit4 => (false, 0x05),
            Key::Digit5 => (false, 0x06),
            Key::Digit6 => (false, 0x07),
            Key::Digit7 => (false, 0x08),
            Key::Digit8 => (false, 0x09),
            Key::Digit9 => (false, 0x0A),
            Key::F1 => (false, 0x3B),
            Key::F2 => (false, 0x3C),
            Key::F3 => (false, 0x3D),
            Key::F4 => (false, 0x3E),
            Key::F5 => (false, 0x3F),
            Key::F6 => (false, 0x40),
            Key::F7 => (false, 0x41),
            Key::F8 => (false, 0x42),
            Key::F9 => (false, 0x43),
            Key::F10 => (false, 0x44),
            Key::F11 => (false, 0x57),
            Key::F12 => (false, 0x58),
            Key::Enter => (false, 0x1C),
            Key::Esc => (false, 0x01),
            Key::Tab => (false, 0x0F),
            Key::Space => (false, 0x39),
            Key::Backspace => (false, 0x0E),
            Key::Delete => (true, 0x53),
            Key::Up => (true, 0x48),
            Key::Down => (true, 0x50),
            Key::Left => (true, 0x4B),
            Key::Right => (true, 0x4D),
            Key::Home => (true, 0x47),
            Key::End => (true, 0x4F),
            Key::PageUp => (true, 0x49),
            Key::PageDown => (true, 0x51),
            Key::Insert => (true, 0x52),
            // The left Windows/GUI key is the extended byte 0x5B.
            Key::Win => (true, 0x5B),
        };
        Set1Scancode { code, extended }
    }
}

/// An owned mouse action. Coordinates are `u16`, matching the RDP wire width;
/// `dy` is `i16`, with a positive value scrolling up (away from the user), as
/// `WM_MOUSEWHEEL` defines it.
///
/// Horizontal scroll is not part of the vocabulary. The IPC spelling is
/// externally tagged: `{"Click":{"x":1,"y":2,"button":"Left"}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MouseAction {
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
        button: Button,
    },
    /// Two [`Click`](MouseAction::Click)-equivalent sequences at `(x, y)`,
    /// separated by a short inter-click delay that the session applies.
    DoubleClick {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// The button to double-click.
        button: Button,
    },
    /// Move to `(x, y)` then scroll vertically by `dy`: a signed count of
    /// `WHEEL_DELTA` (120-unit) notches, positive = away from the user.
    /// The session splits `|dy| > 255` into several in-range wheel
    /// operations; the value never wraps.
    Scroll {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// Signed scroll amount in `WHEEL_DELTA` (120-unit) notches.
        dy: i16,
    },
    /// Press `button` at `(from_x, from_y)`, move through several
    /// interpolated intermediate points, then release at `(to_x, to_y)`.
    Drag {
        /// Drag start x, in physical virtual-desktop pixels.
        from_x: u16,
        /// Drag start y, in physical virtual-desktop pixels.
        from_y: u16,
        /// Drag end x, in physical virtual-desktop pixels.
        to_x: u16,
        /// Drag end y, in physical virtual-desktop pixels.
        to_y: u16,
        /// The button held during the drag.
        button: Button,
    },
}

impl MouseAction {
    /// Every `(x, y)` pair this action targets, for the caller's coordinate
    /// bounds check.
    ///
    /// `Move`, `Click`, `DoubleClick` and `Scroll` yield exactly one pair;
    /// `Drag` yields its `from` and `to` endpoints.
    #[must_use]
    pub fn coordinates(&self) -> Vec<(u16, u16)> {
        match *self {
            MouseAction::Move { x, y }
            | MouseAction::Click { x, y, .. }
            | MouseAction::DoubleClick { x, y, .. }
            | MouseAction::Scroll { x, y, .. } => vec![(x, y)],
            MouseAction::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                ..
            } => vec![(from_x, from_y), (to_x, to_y)],
        }
    }
}

/// An owned keyboard action. The IPC spelling is externally tagged:
/// `{"Type":"text"}` or `{"Combo":["Ctrl","A"]}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum KeyAction {
    /// Type literal text, one Unicode code point at a time. The text is
    /// layout-independent and correct for non-ASCII input, but it cannot
    /// express held modifiers.
    Type(String),
    /// Press a combination of keys via scancodes, in order, then release them
    /// in reverse order. This is the only path that expresses held modifiers
    /// such as Ctrl+A or Alt+F4.
    Combo(Vec<Key>),
}

/// A pointer button a viewer can press, including the browser X buttons.
///
/// The spelling is snake_case (`"left"`, `"x1"`). The declaration order is
/// the order in which held buttons are released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    /// The left (primary) button.
    Left,
    /// The middle (wheel) button.
    Middle,
    /// The right (secondary) button.
    Right,
    /// The first extra button (typically Back).
    X1,
    /// The second extra button (typically Forward).
    X2,
}

/// One raw input event of a human viewer: a physical key position (Set-1
/// scancode) or a pointer event in framebuffer pixels.
///
/// The viewer spelling is internally tagged and snake_case:
/// `{"type":"move","x":1,"y":2}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RawInput {
    /// Move the pointer to `(x, y)`.
    #[serde(rename = "move")]
    PointerMove {
        /// Target x, in framebuffer pixels.
        x: u16,
        /// Target y, in framebuffer pixels.
        y: u16,
    },
    /// Press or release a pointer button at the current position.
    Button {
        /// The button.
        button: PointerButton,
        /// `true` to press, `false` to release.
        down: bool,
    },
    /// Rotate the wheel by `units` (120 per notch) at the current position.
    Wheel {
        /// `true` for the vertical wheel, `false` for the horizontal one.
        vertical: bool,
        /// Signed wheel units; positive = away from the user.
        units: i16,
    },
    /// Press or release the key with Set-1 scancode `code`.
    Key {
        /// The Set-1 make code.
        code: u8,
        /// Whether the key is an extended (`E0`-prefixed) key.
        extended: bool,
        /// `true` to press, `false` to release.
        down: bool,
    },
}

/// Parse one case-insensitive key name (for example `"ctrl"`, `"ESC"`,
/// `"page-up"` or `"0"`) into a [`Key`].
///
/// This is the one key-name table: the CLI and any other client that reads
/// key names from a user call it instead of holding their own copy. Leading
/// and trailing whitespace is ignored. Aliases: `0`-`9`, `esc`/`escape`,
/// `delete`/`del`, `pageup`/`page-up`, `pagedown`/`page-down`,
/// `insert`/`ins`, `win`/`windows`/`super`.
///
/// # Errors
///
/// Returns a message naming the offending token if `name` is not a known key.
/// The error is a plain `String` so the crate stays free of client error types.
pub fn parse_key_name(name: &str) -> Result<Key, String> {
    let trimmed = name.trim();
    Ok(match trimmed.to_lowercase().as_str() {
        "ctrl" => Key::Ctrl,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        "digit0" | "0" => Key::Digit0,
        "digit1" | "1" => Key::Digit1,
        "digit2" | "2" => Key::Digit2,
        "digit3" | "3" => Key::Digit3,
        "digit4" | "4" => Key::Digit4,
        "digit5" | "5" => Key::Digit5,
        "digit6" | "6" => Key::Digit6,
        "digit7" | "7" => Key::Digit7,
        "digit8" | "8" => Key::Digit8,
        "digit9" | "9" => Key::Digit9,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "enter" => Key::Enter,
        "esc" | "escape" => Key::Esc,
        "tab" => Key::Tab,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page-up" => Key::PageUp,
        "pagedown" | "page-down" => Key::PageDown,
        "insert" | "ins" => Key::Insert,
        "win" | "windows" | "super" => Key::Win,
        other => {
            return Err(format!(
                "unknown key name '{other}' (original token: '{trimmed}')"
            ))
        }
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const ALL_KEYS: [Key; 67] = [
        Key::Ctrl,
        Key::Alt,
        Key::Shift,
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::N,
        Key::O,
        Key::P,
        Key::Q,
        Key::R,
        Key::S,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::X,
        Key::Y,
        Key::Z,
        Key::Digit0,
        Key::Digit1,
        Key::Digit2,
        Key::Digit3,
        Key::Digit4,
        Key::Digit5,
        Key::Digit6,
        Key::Digit7,
        Key::Digit8,
        Key::Digit9,
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::Enter,
        Key::Esc,
        Key::Tab,
        Key::Space,
        Key::Backspace,
        Key::Delete,
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::Insert,
        Key::Win,
    ];

    #[test]
    fn the_key_set_has_67_keys_that_all_have_a_distinct_scancode() {
        let mut seen = Vec::new();
        for key in ALL_KEYS {
            let scancode = key.set1_scancode();
            assert!(
                !seen.contains(&(scancode.code, scancode.extended)),
                "{key:?} repeats a scancode"
            );
            seen.push((scancode.code, scancode.extended));
        }
        assert_eq!(seen.len(), 67);
    }

    #[test]
    fn set1_scancodes_match_the_reference_values() {
        let expect = |key: Key, code: u8, extended: bool| {
            assert_eq!(key.set1_scancode(), Set1Scancode { code, extended });
        };
        expect(Key::Ctrl, 0x1D, false);
        expect(Key::A, 0x1E, false);
        expect(Key::Digit0, 0x0B, false);
        expect(Key::F11, 0x57, false);
        expect(Key::Esc, 0x01, false);
        expect(Key::Delete, 0x53, true);
        expect(Key::Up, 0x48, true);
        expect(Key::PageDown, 0x51, true);
        expect(Key::Win, 0x5B, true);
    }

    #[test]
    fn every_key_serializes_to_its_variant_name_and_parses_from_its_lowercase_name() {
        for key in ALL_KEYS {
            let json = serde_json::to_string(&key).expect("a key serializes");
            let name = json.trim_matches('"');
            assert_eq!(parse_key_name(&name.to_lowercase()), Ok(key), "{name}");
            let back: Key = serde_json::from_str(&json).expect("a key parses");
            assert_eq!(back, key);
        }
    }

    #[test]
    fn parse_key_name_handles_aliases_whitespace_and_a_rejection() {
        assert_eq!(parse_key_name("  CTRL "), Ok(Key::Ctrl));
        assert_eq!(parse_key_name("escape"), Ok(Key::Esc));
        assert_eq!(parse_key_name("super"), Ok(Key::Win));
        assert_eq!(parse_key_name("0"), Ok(Key::Digit0));
        assert_eq!(parse_key_name("page-up"), Ok(Key::PageUp));
        assert_eq!(
            parse_key_name(" Bogus "),
            Err("unknown key name 'bogus' (original token: 'Bogus')".to_owned())
        );
    }

    #[test]
    fn the_cli_button_table_lists_every_button_once_in_help_order() {
        let names: Vec<&str> = Button::CLI_VALUES.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(names, ["left", "right", "middle"]);
        for (button, name, _) in Button::CLI_VALUES {
            assert_eq!(Button::from_cli_name(name), Some(button));
        }
        assert_eq!(Button::from_cli_name("Left"), None);
        assert_eq!(Button::from_cli_name("x1"), None);
    }

    #[test]
    fn raw_input_uses_the_viewer_spelling() {
        let move_json = serde_json::to_string(&RawInput::PointerMove { x: 1, y: 2 });
        assert_eq!(
            move_json.ok().as_deref(),
            Some(r#"{"type":"move","x":1,"y":2}"#)
        );
        let button =
            serde_json::from_str::<RawInput>(r#"{"type":"button","button":"x2","down":true}"#);
        assert_eq!(
            button.ok(),
            Some(RawInput::Button {
                button: PointerButton::X2,
                down: true
            })
        );
        assert!(
            serde_json::from_str::<RawInput>(r#"{"type":"pointer_move","x":1,"y":2}"#).is_err()
        );
    }

    #[test]
    fn pointer_buttons_order_by_declaration() {
        let mut buttons = [
            PointerButton::X2,
            PointerButton::Right,
            PointerButton::Left,
            PointerButton::X1,
            PointerButton::Middle,
        ];
        buttons.sort();
        assert_eq!(
            buttons,
            [
                PointerButton::Left,
                PointerButton::Middle,
                PointerButton::Right,
                PointerButton::X1,
                PointerButton::X2,
            ]
        );
    }

    #[test]
    fn mouse_action_coordinates_cover_every_endpoint() {
        let click = MouseAction::Click {
            x: 1,
            y: 2,
            button: Button::Left,
        };
        assert_eq!(click.coordinates(), [(1, 2)]);
        let drag = MouseAction::Drag {
            from_x: 1,
            from_y: 2,
            to_x: 3,
            to_y: 4,
            button: Button::Middle,
        };
        assert_eq!(drag.coordinates(), [(1, 2), (3, 4)]);
    }
}
