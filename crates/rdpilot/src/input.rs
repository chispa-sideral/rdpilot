//! The input vocabulary of the SDK and its translation into IronRDP input.
//!
//! The types (`MouseAction`, `KeyAction`, `Button`, `Key`, `PointerButton`,
//! `RawInput`) live in `rdpilot-vocab`, which has no IronRDP dependency, and
//! are re-exported here as the SDK API. No `ironrdp` type (`Operation`,
//! `Scancode`, `FastPathInputEvent`) appears in a public field or signature.
//!
//! The pure translation from those types into `ironrdp_input::Operation`
//! batches is the only code in the crate that constructs `Operation`s;
//! `Session` calls it and forwards the result to
//! `ironrdp_input::Database::apply`.

#[cfg(test)]
use crate::error::Error;
use ironrdp_input::{MouseButton, MousePosition, Operation, Scancode, WheelRotations};

pub use rdpilot_vocab::{Button, Key, KeyAction, MouseAction, PointerButton, RawInput};

/// Whether `key` is a held modifier (Ctrl/Alt/Shift) rather than a terminal
/// key. Drives [`KeyAction::Combo`]'s press/release ordering:
/// [`combo_operations`] partitions a `Combo` on this predicate. `Win` is a
/// terminal key.
fn is_modifier(key: Key) -> bool {
    matches!(key, Key::Ctrl | Key::Alt | Key::Shift)
}

/// Map a [`Key`] to its IBM PC/AT Scan Code Set 1 byte.
///
/// The table lives in `rdpilot-vocab` ([`Key::set1_scancode`]); this wraps
/// it in the IronRDP scancode type. Extended (`E0`-prefixed) keys pass
/// `extended = true`.
pub(crate) fn scancode(key: Key) -> Scancode {
    let set1 = key.set1_scancode();
    Scancode::from_u8(set1.extended, set1.code)
}

/// Map an owned [`Button`] to `ironrdp_input::MouseButton`. `Middle` shares
/// the wheel-button wire flag inside `ironrdp-input` itself (already handled
/// correctly there); no separate mapping is needed here.
fn mouse_button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Right => MouseButton::Right,
        Button::Middle => MouseButton::Middle,
    }
}

/// The largest wheel-rotation magnitude that safely survives
/// `MousePdu::encode`'s `as u8` wire cast (Pitfall 1, T-03-01).
/// `MousePdu::encode` packs the wheel magnitude into the low 8 bits of the
/// same 16-bit wire word that carries the sign/flag bits, so a single PDU is
/// hard-ceilinged at 255; this SDK caps each split operation at 240 (2 full
/// `WHEEL_DELTA` notches, D-3.3) for a clean multiple-of-120 remainder.
const MAX_WHEEL_MAGNITUDE_PER_OPERATION: u16 = 240;

/// Split a signed wheel-rotation magnitude into one or more in-range
/// `Operation::WheelRotations`, preserving sign, so nothing above
/// [`MAX_WHEEL_MAGNITUDE_PER_OPERATION`] ever reaches `MousePdu::encode`'s
/// truncating `as u8` cast (Pitfall 1). `dy == 0` still yields exactly one
/// zero-magnitude operation (a harmless no-op scroll) rather than an empty
/// batch, so a `Scroll` action always emits at least one `WheelRotations`.
fn wheel_rotation_operations(dy: i16) -> Vec<Operation> {
    let sign: i16 = if dy < 0 { -1 } else { 1 };
    let mut remaining = dy.unsigned_abs();
    let mut ops = Vec::new();
    while remaining > 0 {
        let chunk = remaining.min(MAX_WHEEL_MAGNITUDE_PER_OPERATION);
        // `chunk` is always <= 240, so this conversion never actually
        // saturates; `unwrap_or` is a defensive fallback, not a silent-wrap
        // risk (API-01: no unwrap/expect/panic in library code).
        let rotation_units = sign * i16::try_from(chunk).unwrap_or(i16::MAX);
        ops.push(Operation::WheelRotations(WheelRotations {
            is_vertical: true,
            rotation_units,
        }));
        remaining -= chunk;
    }
    if ops.is_empty() {
        ops.push(Operation::WheelRotations(WheelRotations {
            is_vertical: true,
            rotation_units: 0,
        }));
    }
    ops
}

/// Number of interpolated intermediate `MouseMove`s a [`MouseAction::Drag`]
/// emits between its press and release batches (D-3.8). `SM_CXDRAG`/
/// `SM_CYDRAG` are per-move distance thresholds, not event-count thresholds
/// (03-RESEARCH.md §4) — this offline-safe starting value is empirically
/// tuned against a live target in a later plan.
const DRAG_INTERPOLATION_STEPS: u16 = 5;

/// Translate a [`MouseAction`] into one or more ordered `Operation` batches.
///
/// The outer `Vec` is the sequence of **timed** batches: `Session::send_mouse`
/// (a later plan) applies each inner batch to `ironrdp_input::Database` in
/// one `apply()` call and sends the resulting PDUs together, sleeping
/// between batches only where D-3.7/D-3.8 require an inter-batch delay
/// (`DoubleClick`, `Drag`). `Move`/`Click`/`Scroll` are always exactly one
/// batch.
pub(crate) fn mouse_operations(action: &MouseAction) -> Vec<Vec<Operation>> {
    match *action {
        MouseAction::Move { x, y } => {
            vec![vec![Operation::MouseMove(MousePosition { x, y })]]
        }
        MouseAction::Click { x, y, button } => vec![click_batch(x, y, button)],
        MouseAction::DoubleClick { x, y, button } => {
            vec![click_batch(x, y, button), click_batch(x, y, button)]
        }
        MouseAction::Scroll { x, y, dy } => {
            // MouseMove must precede WheelRotations in the SAME batch/apply()
            // call (Pitfall 5, T-03-03): `Operation::WheelRotations` has no
            // x/y field of its own — it scrolls at Database's last-tracked
            // mouse position, which would otherwise be stale/wrong.
            let mut batch = vec![Operation::MouseMove(MousePosition { x, y })];
            batch.extend(wheel_rotation_operations(dy));
            vec![batch]
        }
        MouseAction::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
            button,
        } => drag_batches(from_x, from_y, to_x, to_y, button),
    }
}

/// A single click: move to `(x, y)`, press `button`, release `button`
/// (`MouseMove` -> `MouseButtonPressed` -> `MouseButtonReleased` ordering).
fn click_batch(x: u16, y: u16, button: Button) -> Vec<Operation> {
    let btn = mouse_button(button);
    vec![
        Operation::MouseMove(MousePosition { x, y }),
        Operation::MouseButtonPressed(btn),
        Operation::MouseButtonReleased(btn),
    ]
}

/// Move to `from` and press `button`, then [`DRAG_INTERPOLATION_STEPS`]
/// interpolated moves ending exactly at `to`, then release (D-3.8). Every
/// interpolated move is its own batch so `Session::send_mouse` can space
/// them with a real `tokio::time::sleep` between `input_tx.send()` calls.
fn drag_batches(
    from_x: u16,
    from_y: u16,
    to_x: u16,
    to_y: u16,
    button: Button,
) -> Vec<Vec<Operation>> {
    let btn = mouse_button(button);
    let mut batches = Vec::with_capacity(usize::from(DRAG_INTERPOLATION_STEPS) + 2);
    batches.push(vec![
        Operation::MouseMove(MousePosition {
            x: from_x,
            y: from_y,
        }),
        Operation::MouseButtonPressed(btn),
    ]);
    for step in 1..=DRAG_INTERPOLATION_STEPS {
        let (x, y) = interpolate(from_x, from_y, to_x, to_y, step, DRAG_INTERPOLATION_STEPS);
        batches.push(vec![Operation::MouseMove(MousePosition { x, y })]);
    }
    batches.push(vec![Operation::MouseButtonReleased(btn)]);
    batches
}

/// Linear interpolation between `(from_x, from_y)` and `(to_x, to_y)` at
/// `step / total`. Widened to `i32` so the subtraction can go negative
/// without wrapping; the result always lies within `[from, to]` (both
/// originally `u16`), so the final cast back to `u16` never truncates in
/// practice — the `unwrap_or` clamp is defensive only (API-01).
fn interpolate(
    from_x: u16,
    from_y: u16,
    to_x: u16,
    to_y: u16,
    step: u16,
    total: u16,
) -> (u16, u16) {
    let lerp = |from: u16, to: u16| -> u16 {
        let from = i32::from(from);
        let to = i32::from(to);
        let step = i32::from(step);
        let total = i32::from(total);
        let value = from + (to - from) * step / total;
        u16::try_from(value).unwrap_or(if value < 0 { 0 } else { u16::MAX })
    };
    (lerp(from_x, to_x), lerp(from_y, to_y))
}

/// Translate a [`KeyAction`] into an ordered `Operation` sequence (D-3.5).
pub(crate) fn key_operations(action: &KeyAction) -> Vec<Operation> {
    match action {
        KeyAction::Type(text) => {
            let mut ops = Vec::with_capacity(text.chars().count() * 2);
            for c in text.chars() {
                ops.push(Operation::UnicodeKeyPressed(c));
                ops.push(Operation::UnicodeKeyReleased(c));
            }
            ops
        }
        KeyAction::Combo(keys) => combo_operations(keys),
    }
}

/// Press held modifiers first (in call order), then the remaining terminal
/// key(s) (in call order); release in the exact reverse order (D-3.5:
/// press-mods -> press-key -> release-key -> release-mods). Handles the
/// degenerate empty/all-modifier/modifier-free partitions without ever
/// indexing out of range (API-01, T-03-04) — an empty `Combo` yields an
/// empty (but well-formed) sequence, never a panic.
fn combo_operations(keys: &[Key]) -> Vec<Operation> {
    let (modifiers, others): (Vec<Key>, Vec<Key>) =
        keys.iter().copied().partition(|k| is_modifier(*k));
    let mut ops = Vec::with_capacity((modifiers.len() + others.len()) * 2);
    for &k in &modifiers {
        ops.push(Operation::KeyPressed(scancode(k)));
    }
    for &k in &others {
        ops.push(Operation::KeyPressed(scancode(k)));
    }
    for &k in others.iter().rev() {
        ops.push(Operation::KeyReleased(scancode(k)));
    }
    for &k in modifiers.iter().rev() {
        ops.push(Operation::KeyReleased(scancode(k)));
    }
    ops
}

/// Translate raw events into `ironrdp_input` operations, in order.
pub(crate) fn raw_operations(events: &[RawInput]) -> Vec<Operation> {
    let mut ops = Vec::with_capacity(events.len());
    for event in events {
        match *event {
            RawInput::PointerMove { x, y } => {
                ops.push(Operation::MouseMove(MousePosition { x, y }))
            }
            RawInput::Button { button, down } => {
                let button = match button {
                    PointerButton::Left => MouseButton::Left,
                    PointerButton::Middle => MouseButton::Middle,
                    PointerButton::Right => MouseButton::Right,
                    PointerButton::X1 => MouseButton::X1,
                    PointerButton::X2 => MouseButton::X2,
                };
                ops.push(if down {
                    Operation::MouseButtonPressed(button)
                } else {
                    Operation::MouseButtonReleased(button)
                });
            }
            RawInput::Wheel { vertical, units } => {
                // Split like `Scroll` so no operation exceeds the wire cap.
                for op in wheel_rotation_operations(units) {
                    if let Operation::WheelRotations(mut w) = op {
                        w.is_vertical = vertical;
                        ops.push(Operation::WheelRotations(w));
                    }
                }
            }
            RawInput::Key {
                code,
                extended,
                down,
            } => {
                let scancode = Scancode::from_u8(extended, code);
                ops.push(if down {
                    Operation::KeyPressed(scancode)
                } else {
                    Operation::KeyReleased(scancode)
                });
            }
        }
    }
    ops
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

        let scroll = MouseAction::Scroll {
            x: 3,
            y: 4,
            dy: 120,
        };
        assert_eq!(
            scroll,
            MouseAction::Scroll {
                x: 3,
                y: 4,
                dy: 120
            }
        );

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

    /// `Key::Win` maps to the Set-1 extended left-Windows/GUI scancode 0x5B
    /// (05-02 RESEARCH Pitfall 3) -- required to express D-5.1's Win+R
    /// launch sequence.
    #[test]
    fn win_key_maps_to_extended_0x5b() {
        assert_eq!(scancode(Key::Win).as_u8(), (true, 0x5B));
    }

    /// `Key::Win` is a terminal key, not a modifier (`is_modifier(Win)` stays
    /// false): a `Combo([Win, R])` presses both keys in call order and
    /// releases in reverse order, yielding a non-empty ordered sequence --
    /// no modifier-partition surprise (D-5.1's Win+R launch sequence).
    #[test]
    fn combo_win_r_yields_ordered_non_empty_sequence() {
        let ops = key_operations(&KeyAction::Combo(vec![Key::Win, Key::R]));
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
                scancode(Key::Win).as_u8(),
                scancode(Key::R).as_u8(),
                scancode(Key::R).as_u8(),
                scancode(Key::Win).as_u8(),
            ]
        );
    }

    // --- Task 2: mouse translation (D-3.3, D-3.7, D-3.8, Pitfall 1, Pitfall 5) ---

    #[test]
    fn scroll_always_emits_move_before_wheel_rotations() {
        let batches = mouse_operations(&MouseAction::Scroll {
            x: 10,
            y: 20,
            dy: 120,
        });
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(op_kind(&batch[0]), "MouseMove");
        assert_eq!(op_kind(&batch[1]), "WheelRotations");
    }

    #[test]
    fn scroll_single_notch_produces_exactly_one_wheel_rotation() {
        let batches = mouse_operations(&MouseAction::Scroll {
            x: 0,
            y: 0,
            dy: 120,
        });
        let wheel_ops = batches[0]
            .iter()
            .filter(|op| op_kind(op) == "WheelRotations")
            .count();
        assert_eq!(wheel_ops, 1);
    }

    #[test]
    fn scroll_over_255_magnitude_splits_into_in_range_operations() {
        let batches = mouse_operations(&MouseAction::Scroll {
            x: 0,
            y: 0,
            dy: 600,
        });
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
        let batches = mouse_operations(&MouseAction::Scroll {
            x: 0,
            y: 0,
            dy: -600,
        });
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
        assert!(
            batches.len() >= 3,
            "expected >= 3 batches, got {}",
            batches.len()
        );
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

    #[test]
    fn raw_input_maps_to_operations_in_order() {
        let ops = raw_operations(&[
            RawInput::PointerMove { x: 3, y: 4 },
            RawInput::Button {
                button: PointerButton::X2,
                down: true,
            },
            RawInput::Button {
                button: PointerButton::X2,
                down: false,
            },
            RawInput::Wheel {
                vertical: false,
                units: -360,
            },
            RawInput::Key {
                code: 0x2A,
                extended: false,
                down: true,
            },
            RawInput::Key {
                code: 0x48,
                extended: true,
                down: false,
            },
        ]);
        assert_eq!(
            format!("{ops:?}"),
            format!(
                "{:?}",
                vec![
                    Operation::MouseMove(MousePosition { x: 3, y: 4 }),
                    Operation::MouseButtonPressed(MouseButton::X2),
                    Operation::MouseButtonReleased(MouseButton::X2),
                    Operation::WheelRotations(WheelRotations {
                        is_vertical: false,
                        rotation_units: -240,
                    }),
                    Operation::WheelRotations(WheelRotations {
                        is_vertical: false,
                        rotation_units: -120,
                    }),
                    Operation::KeyPressed(Scancode::from_u8(false, 0x2A)),
                    Operation::KeyReleased(Scancode::from_u8(true, 0x48)),
                ]
            )
        );
    }
}
