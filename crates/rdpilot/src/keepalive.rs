//! Automatic keepalive (SESS-02, D-06, Pitfall M1).
//!
//! While a [`Session`](crate::Session) is alive, the active-session loop pushes a
//! benign synthetic input roughly every [`KEEPALIVE_INTERVAL`] so the server's
//! idle timer never fires. The event is a **zero-delta pointer move** — never a
//! keystroke (a keystroke could leak characters into a focused remote field,
//! Pitfall 4). There is no caller opt-in (D-06).
//!
//! We build the `FastPathInputEvent::MouseEvent` directly rather than via
//! `ironrdp_input::Database`, because `Database` deduplicates a move to the same
//! position (it emits nothing when the cursor has not moved) — which would make a
//! repeated zero-delta keepalive a no-op. Emitting the well-formed `MOVE` PDU
//! directly guarantees a real (but harmless) PDU on every tick.
//!
//! Assumption A1: if a zero-delta move proves insufficient to reset the server
//! idle timer in the live idle test (Plan 03), the documented fallback is an
//! alternating ±1px move. This is noted for Plan 03; Phase 2 ships the zero-delta
//! form.

use core::time::Duration;

use ironrdp::pdu::input::mouse::PointerFlags;
use ironrdp::pdu::input::fast_path::FastPathInputEvent;
use ironrdp::pdu::input::MousePdu;

/// How often the keepalive fires. 60 s is shorter than any realistic RDP idle
/// timeout (Pitfall M1) while keeping wire traffic negligible.
pub(crate) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// Build a single zero-delta pointer-move event to the origin.
///
/// `PointerFlags::MOVE` with `(0, 0)` and no wheel rotation is a well-formed,
/// side-effect-free mouse-move PDU: it does not press a button, type a key, or
/// scroll. It is the safest null input for keeping a session alive.
pub(crate) fn null_input_event() -> FastPathInputEvent {
    FastPathInputEvent::MouseEvent(MousePdu {
        flags: PointerFlags::MOVE,
        number_of_wheel_rotation_units: 0,
        x_position: 0,
        y_position: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keepalive event is a pointer MOVE, not a keystroke (Pitfall 4).
    #[test]
    fn keepalive_is_a_zero_delta_pointer_move() {
        match null_input_event() {
            FastPathInputEvent::MouseEvent(pdu) => {
                assert!(pdu.flags.contains(PointerFlags::MOVE));
                assert_eq!(pdu.x_position, 0);
                assert_eq!(pdu.y_position, 0);
                assert_eq!(pdu.number_of_wheel_rotation_units, 0);
            }
            other => panic!("keepalive must be a MouseEvent move, got {other:?}"),
        }
    }

    /// Defense-in-depth: the keepalive must never be a keyboard event.
    #[test]
    fn keepalive_is_never_a_keystroke() {
        assert!(!matches!(
            null_input_event(),
            FastPathInputEvent::KeyboardEvent(..) | FastPathInputEvent::UnicodeKeyboardEvent(..)
        ));
    }

    #[test]
    fn interval_is_sixty_seconds() {
        assert_eq!(KEEPALIVE_INTERVAL, Duration::from_secs(60));
    }
}
