//! [`scale_to_native`] — the MCP-04 BLOCKING pure coordinate bridge.
//!
//! Every model-supplied coordinate lives in the FIXED advertised
//! [`ADVERTISED_WIDTH`]x[`ADVERTISED_HEIGHT`] (1280x800, D-14.2 LOCKED)
//! space. `scale_to_native` bridges that space to a session's actual
//! native 96-DPI physical-pixel desktop dimensions (sourced from the real
//! `Request::DesktopSize` wire verb, never PNG-sniffed).
//!
//! Deliberately a single, pure, five-argument function with NO dependency
//! on a live session, the handler, or any `rdpilot-ipc`/`rmcp` type — this
//! is what makes it directly unit-testable (this module's own tests, plus
//! the BLOCKING `tests/scale_to_native.rs` integration test) without a
//! running daemon or MCP server.
//!
//! Rounding discipline (Pitfall 2): floating-point scale factors, then
//! [`f64::round`] (round-to-nearest, **half away from zero** — Rust's
//! `f64::round` tie-break, NOT truncation and NOT banker's/round-half-to-
//! even), then `.clamp(0.0, native_dim - 1.0)` as the LAST step before the
//! `u16` cast. The clamp — not `Session::check_bounds`'s own rejection —
//! is the bridge's own safety net: an advertised-edge input that rounds up
//! to exactly `native_dim` (e.g. `1280 * 1.5 = 1920.0` against a native
//! width of 1920) lands one pixel inside bounds instead of exactly on the
//! exclusive edge `Session::check_bounds` would reject.

/// The FIXED advertised logical width the `computer` tool exposes to a
/// driving model (D-14.2 LOCKED, WXGA). Never dynamic/per-session.
pub const ADVERTISED_WIDTH: u32 = 1280;

/// The FIXED advertised logical height the `computer` tool exposes to a
/// driving model (D-14.2 LOCKED, WXGA). Never dynamic/per-session.
pub const ADVERTISED_HEIGHT: u32 = 800;

/// Bridge one model-supplied `(x, y)` coordinate in the
/// [`ADVERTISED_WIDTH`]x[`ADVERTISED_HEIGHT`] space to native
/// `(native_w, native_h)` physical pixels.
///
/// Pure: no I/O, no session, no daemon round trip. Always returns a value
/// strictly inside `[0, native_w) x [0, native_h)` — never a coordinate
/// `Session::check_bounds` would reject, and never relies on that
/// downstream check as a safety net (Pitfall 2).
#[must_use]
pub fn scale_to_native(x: u32, y: u32, native_w: u32, native_h: u32) -> (u16, u16) {
    let scale_x = f64::from(native_w) / f64::from(ADVERTISED_WIDTH);
    let scale_y = f64::from(native_h) / f64::from(ADVERTISED_HEIGHT);

    // saturating_sub guards the degenerate native_dim == 0 case: clamp's
    // own assertion requires min <= max, so a native dimension of 0 must
    // clamp to [0.0, 0.0], not [0.0, -1.0] (which would panic).
    let max_x = f64::from(native_w.saturating_sub(1));
    let max_y = f64::from(native_h.saturating_sub(1));

    // f64::round: "round half away from zero" — the required tie-break
    // (an exact .5 tie, e.g. 1279 * 1.5 = 1918.5, MUST round UP).
    let nx = (f64::from(x) * scale_x).round().clamp(0.0, max_x);
    let ny = (f64::from(y) * scale_y).round().clamp(0.0, max_y);

    // Safe: clamp already bounded nx/ny to [0, native_dim - 1] <= u16::MAX
    // for any RDP-realistic desktop dimension (native dims are u16 on the
    // wire, DesktopSize's own width/height fields).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (nx as u16, ny as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_maps_to_origin() {
        assert_eq!(scale_to_native(0, 0, 1920, 1080), (0, 0));
    }

    #[test]
    fn near_corner_exact_tie_rounds_half_away_from_zero() {
        // x = 1279 * 1.5 = 1918.5 -- an EXACT tie, must round UP to 1919.
        // y = 799 * 1.35 = 1078.65 -> 1079.
        assert_eq!(scale_to_native(1279, 799, 1920, 1080), (1919, 1079));
    }

    #[test]
    fn center_sanity() {
        assert_eq!(scale_to_native(640, 400, 1920, 1080), (960, 540));
    }

    #[test]
    fn advertised_exclusive_edge_clamps_strictly_inside_native_bounds() {
        let (nx, ny) = scale_to_native(1280, 800, 1920, 1080);
        assert!(nx < 1920 && ny < 1080, "exclusive-edge input must clamp inside native bounds, got ({nx}, {ny})");
    }

    #[test]
    fn square_scale_is_identity_for_interior_points() {
        // native == advertised (1280x800): scale factors are 1.0.
        assert_eq!(scale_to_native(100, 150, ADVERTISED_WIDTH, ADVERTISED_HEIGHT), (100, 150));
        assert_eq!(scale_to_native(0, 0, ADVERTISED_WIDTH, ADVERTISED_HEIGHT), (0, 0));
    }

    #[test]
    fn degenerate_zero_native_dims_do_not_panic() {
        assert_eq!(scale_to_native(0, 0, 0, 0), (0, 0));
    }
}
