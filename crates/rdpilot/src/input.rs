//! Owned input vocabulary (`MouseAction`, `KeyAction`, `Button`, `Key`, D-3.1).
//!
//! Every public type here is an owned SDK type (D-09): no `ironrdp`/
//! `FastPathInputEvent`/`Scancode` type ever appears in a public field or
//! signature. A later commit in this same plan adds the crate-internal pure
//! translation from these types into `ironrdp_input::Operation` batches; that
//! translation is the only code in the crate allowed to construct
//! `ironrdp_input::Operation`s (`Session`, a later plan, just calls it and
//! forwards the result to `ironrdp_input::Database::apply`).

use crate::error::Error;

/// A mouse button used by [`MouseAction::Click`], [`MouseAction::DoubleClick`],
/// and [`MouseAction::Drag`] (D-3.1).
///
/// Only the three buttons the v1 computer-use vocabulary needs; X1/X2
/// (browser back/forward) are out of scope this phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// The left (primary) mouse button.
    Left,
    /// The right (secondary/context-menu) mouse button.
    Right,
    /// The middle (wheel) mouse button.
    Middle,
}

/// A logical key for [`KeyAction::Combo`] (D-3.1).
///
/// Named virtual keys, not raw scancodes — [`Key`] never leaks an
/// `ironrdp_input::Scancode` (D-09). Covers the computer-use key set: the
/// three modifiers, `A`-`Z`, `Digit0`-`Digit9`, `F1`-`F12`, common control
/// keys, and the arrow/navigation cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

impl Key {
    /// Whether this key is a held modifier (Ctrl/Alt/Shift) rather than a
    /// terminal key. Drives [`KeyAction::Combo`]'s press/release ordering
    /// (D-3.5): a later commit in this plan partitions a `Combo` on this
    /// predicate.
    #[allow(dead_code)] // consumed by the translation added later in this plan
    fn is_modifier(self) -> bool {
        matches!(self, Key::Ctrl | Key::Alt | Key::Shift)
    }
}

/// An owned mouse action (D-3.1). Coordinates are `u16`, matching the native
/// RDP wire width (`ironrdp_input::MousePosition`); `dy` is `i16`, matching
/// `ironrdp_input::WheelRotations::rotation_units` (positive = scroll up /
/// away from the user, per `WM_MOUSEWHEEL`'s documented sign convention).
///
/// Horizontal scroll is deliberately absent (Deferred, D-3.3).
#[derive(Clone, Debug, PartialEq)]
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
    /// separated by a short inter-click delay (D-3.7; the delay itself is
    /// applied by `Session::send_mouse` in a later plan, not here).
    DoubleClick {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// The button to double-click.
        button: Button,
    },
    /// Move to `(x, y)` then scroll vertically by `dy` (D-3.3): a signed
    /// count of `WHEEL_DELTA` (120-unit) notches, positive = away from the
    /// user. `|dy| > 255` is transparently split into multiple in-range wheel
    /// operations by the translation added later in this plan (Pitfall 1) —
    /// never silently wrapped.
    Scroll {
        /// Target x, in physical virtual-desktop pixels.
        x: u16,
        /// Target y, in physical virtual-desktop pixels.
        y: u16,
        /// Signed scroll amount in `WHEEL_DELTA` (120-unit) notches.
        dy: i16,
    },
    /// Press `button` at `(from_x, from_y)`, move through several
    /// interpolated intermediate points, then release at `(to_x, to_y)`
    /// (D-3.8).
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
    /// bounds check (`Session::send_mouse`, a later plan; SC#4).
    ///
    /// `Move`/`Click`/`DoubleClick`/`Scroll` yield exactly one pair; `Drag`
    /// yields both its `from` and `to` endpoints.
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

/// An owned keyboard action (D-3.1).
#[derive(Clone, Debug, PartialEq)]
pub enum KeyAction {
    /// Type literal text, one Unicode code point at a time
    /// (`Operation::UnicodeKeyPressed`/`Released` — layout-independent,
    /// correct for arbitrary/non-ASCII text; cannot express held modifiers).
    Type(String),
    /// Press a combination of keys via scancodes, in order, then release them
    /// in reverse order (D-3.5) — the only path that can express held
    /// modifiers like Ctrl+A or Alt+F4.
    Combo(Vec<Key>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_input::Operation;

    /// The four D-3.1-locked call sites compile and construct verbatim.
    #[test]
    fn locked_call_sites_compile_and_construct() {
        let click = MouseAction::Click {
            x: 1,
            y: 2,
            button: Button::Left,
        };
        assert_eq!(
            click,
            MouseAction::Click {
                x: 1,
                y: 2,
                button: Button::Left,
            }
        );

        let scroll = MouseAction::Scroll { x: 3, y: 4, dy: 120 };
        assert_eq!(scroll, MouseAction::Scroll { x: 3, y: 4, dy: 120 });

        let typed = KeyAction::Type("h".into());
        assert_eq!(typed, KeyAction::Type("h".to_owned()));

        let combo = KeyAction::Combo(vec![Key::Ctrl, Key::A]);
        assert_eq!(combo, KeyAction::Combo(vec![Key::Ctrl, Key::A]));
    }

    #[test]
    fn coordinates_yields_expected_pairs() {
        assert_eq!(MouseAction::Move { x: 1, y: 2 }.coordinates(), vec![(1, 2)]);
        assert_eq!(
            MouseAction::Click {
                x: 1,
                y: 2,
                button: Button::Left,
            }
            .coordinates(),
            vec![(1, 2)]
        );
        assert_eq!(
            MouseAction::DoubleClick {
                x: 5,
                y: 6,
                button: Button::Right,
            }
            .coordinates(),
            vec![(5, 6)]
        );
        assert_eq!(
            MouseAction::Scroll { x: 7, y: 8, dy: 10 }.coordinates(),
            vec![(7, 8)]
        );
        assert_eq!(
            MouseAction::Drag {
                from_x: 1,
                from_y: 2,
                to_x: 3,
                to_y: 4,
                button: Button::Left,
            }
            .coordinates(),
            vec![(1, 2), (3, 4)]
        );
    }

    /// `Error::CoordinateOutOfBounds` (added to `error.rs` in this same task)
    /// renders a message naming the offending coordinate and desktop size,
    /// and reports the expected stable category (D-3.2, SC#4).
    #[test]
    fn coordinate_out_of_bounds_renders_message_and_category() {
        let err = Error::coordinate_out_of_bounds(1920, 100, 1920, 1080);
        assert_eq!(err.category(), "coordinate_out_of_bounds");
        let message = err.to_string();
        assert!(message.contains("1920"));
        assert!(message.contains("100"));
        assert!(message.contains("1080"));
    }

    // --- Task 2: scancode table (D-3.1) ---

    /// Tags an `Operation` by variant name only, for order-of-operations
    /// assertions (`Operation` itself derives only `Debug`/`Clone`, not
    /// `PartialEq` — this is a test-only, ironrdp-input-source-verified
    /// discriminant helper, not a public API).
    fn op_kind(op: &Operation) -> &'static str {
        match op {
            Operation::MouseButtonPressed(_) => "MouseButtonPressed",
            Operation::MouseButtonReleased(_) => "MouseButtonReleased",
            Operation::MouseMove(_) => "MouseMove",
            Operation::WheelRotations(_) => "WheelRotations",
            Operation::KeyPressed(_) => "KeyPressed",
            Operation::KeyReleased(_) => "KeyReleased",
            Operation::UnicodeKeyPressed(_) => "UnicodeKeyPressed",
            Operation::UnicodeKeyReleased(_) => "UnicodeKeyReleased",
        }
    }

    #[test]
    fn scancode_matches_set1_reference_bytes() {
        assert_eq!(scancode(Key::Ctrl).as_u8(), (false, 0x1D));
        assert_eq!(scancode(Key::A).as_u8(), (false, 0x1E));
        assert_eq!(scancode(Key::F4).as_u8(), (false, 0x3E));
    }

    #[test]
    fn arrow_keys_are_extended() {
        let (extended, _) = scancode(Key::Up).as_u8();
        assert!(extended);
    }

    // --- Task 2: mouse translation (D-3.3, D-3.7, D-3.8, Pitfall 1, Pitfall 5) ---

    #[test]
    fn scroll_always_emits_move_before_wheel_rotations() {
        let batches = mouse_operations(&MouseAction::Scroll { x: 10, y: 20, dy: 120 });
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(op_kind(&batch[0]), "MouseMove");
        assert_eq!(op_kind(&batch[1]), "WheelRotations");
    }

    #[test]
    fn scroll_single_notch_produces_exactly_one_wheel_rotation() {
        let batches = mouse_operations(&MouseAction::Scroll { x: 0, y: 0, dy: 120 });
        let wheel_ops = batches[0]
            .iter()
            .filter(|op| op_kind(op) == "WheelRotations")
            .count();
        assert_eq!(wheel_ops, 1);
    }

    #[test]
    fn scroll_over_255_magnitude_splits_into_in_range_operations() {
        let batches = mouse_operations(&MouseAction::Scroll { x: 0, y: 0, dy: 600 });
        let wheel_ops: Vec<i16> = batches[0]
            .iter()
            .filter_map(|op| match op {
                Operation::WheelRotations(w) => Some(w.rotation_units),
                _ => None,
            })
            .collect();
        assert!(wheel_ops.len() > 1, "expected a split, got {wheel_ops:?}");
        for units in &wheel_ops {
            assert!(
                units.unsigned_abs() <= 255,
                "PDU-unsafe magnitude reached translation output: {units}"
            );
            assert!(*units > 0, "sign must be preserved");
        }
        let total: i32 = wheel_ops.iter().map(|u| i32::from(*u)).sum();
        assert_eq!(total, 600);
    }

    #[test]
    fn scroll_negative_magnitude_preserves_sign_when_split() {
        let batches = mouse_operations(&MouseAction::Scroll { x: 0, y: 0, dy: -600 });
        let wheel_ops: Vec<i16> = batches[0]
            .iter()
            .filter_map(|op| match op {
                Operation::WheelRotations(w) => Some(w.rotation_units),
                _ => None,
            })
            .collect();
        assert!(!wheel_ops.is_empty());
        for units in &wheel_ops {
            assert!(*units < 0);
        }
    }

    #[test]
    fn double_click_produces_two_batches() {
        let batches = mouse_operations(&MouseAction::DoubleClick {
            x: 1,
            y: 1,
            button: Button::Left,
        });
        assert_eq!(batches.len(), 2);
        for batch in &batches {
            assert_eq!(op_kind(&batch[0]), "MouseMove");
            assert_eq!(op_kind(&batch[1]), "MouseButtonPressed");
            assert_eq!(op_kind(&batch[2]), "MouseButtonReleased");
        }
    }

    #[test]
    fn drag_has_leading_press_and_trailing_release_with_interpolated_moves() {
        let batches = mouse_operations(&MouseAction::Drag {
            from_x: 0,
            from_y: 0,
            to_x: 100,
            to_y: 50,
            button: Button::Left,
        });
        assert!(batches.len() >= 3, "expected >= 3 batches, got {}", batches.len());
        assert!(batches
            .first()
            .expect("non-empty")
            .iter()
            .any(|op| op_kind(op) == "MouseButtonPressed"));
        assert!(batches
            .last()
            .expect("non-empty")
            .iter()
            .any(|op| op_kind(op) == "MouseButtonReleased"));
        for batch in &batches[1..batches.len() - 1] {
            assert!(batch.iter().all(|op| op_kind(op) == "MouseMove"));
        }
    }

    // --- Task 2: keyboard translation (D-3.5, Pitfall 2, T-03-02, T-03-04) ---

    #[test]
    fn type_yields_unicode_press_release_pairs_per_char() {
        let ops = key_operations(&KeyAction::Type("hi".into()));
        let kinds: Vec<_> = ops.iter().map(op_kind).collect();
        assert_eq!(
            kinds,
            vec![
                "UnicodeKeyPressed",
                "UnicodeKeyReleased",
                "UnicodeKeyPressed",
                "UnicodeKeyReleased",
            ]
        );
        let chars: Vec<char> = ops
            .iter()
            .map(|op| match op {
                Operation::UnicodeKeyPressed(c) | Operation::UnicodeKeyReleased(c) => *c,
                other => panic!("expected a unicode key operation, got {other:?}"),
            })
            .collect();
        assert_eq!(chars, vec!['h', 'h', 'i', 'i']);
    }

    #[test]
    fn combo_ctrl_a_presses_mods_first_and_releases_in_reverse() {
        let ops = key_operations(&KeyAction::Combo(vec![Key::Ctrl, Key::A]));
        let kinds: Vec<_> = ops.iter().map(op_kind).collect();
        assert_eq!(
            kinds,
            vec!["KeyPressed", "KeyPressed", "KeyReleased", "KeyReleased"]
        );
        let scancodes: Vec<(bool, u8)> = ops
            .iter()
            .map(|op| match op {
                Operation::KeyPressed(s) | Operation::KeyReleased(s) => s.as_u8(),
                other => panic!("expected a key operation, got {other:?}"),
            })
            .collect();
        assert_eq!(
            scancodes,
            vec![
                scancode(Key::Ctrl).as_u8(),
                scancode(Key::A).as_u8(),
                scancode(Key::A).as_u8(),
                scancode(Key::Ctrl).as_u8(),
            ]
        );
    }

    #[test]
    fn combo_alt_f4_presses_mods_first_and_releases_in_reverse() {
        let ops = key_operations(&KeyAction::Combo(vec![Key::Alt, Key::F4]));
        let kinds: Vec<_> = ops.iter().map(op_kind).collect();
        assert_eq!(
            kinds,
            vec!["KeyPressed", "KeyPressed", "KeyReleased", "KeyReleased"]
        );
    }

    #[test]
    fn combo_degenerate_cases_never_panic() {
        assert!(key_operations(&KeyAction::Combo(vec![])).is_empty());

        let all_mods = key_operations(&KeyAction::Combo(vec![Key::Ctrl, Key::Shift]));
        assert_eq!(all_mods.len(), 4);

        let mod_free = key_operations(&KeyAction::Combo(vec![Key::A]));
        assert_eq!(mod_free.len(), 2);
    }
}
