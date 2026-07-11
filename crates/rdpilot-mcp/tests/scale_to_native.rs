//! MCP-04 [BLOCKING] — `scale_to_native` edge/corner precision test.
//!
//! `rdpilot-mcp` is a bin-only crate (no `[lib]` target), so this
//! integration test pulls `computer/scale.rs` in directly via `#[path]`
//! rather than importing it as a library dependency. This is safe because
//! `scale_to_native` is deliberately a PURE function with zero dependency
//! on any other `rdpilot-mcp` module, `rdpilot-ipc`, or `rmcp` type (see
//! that module's own doc comment) — compiling it standalone here exercises
//! the exact same source the binary links.
//!
//! A regression in rounding or clamping fails HERE, by name
//! (`cargo test -p rdpilot-mcp --test scale_to_native`), independent of any
//! live session or daemon.

#[path = "../src/computer/scale.rs"]
mod scale;

use scale::{ADVERTISED_HEIGHT, ADVERTISED_WIDTH, scale_to_native};

/// The zero corner maps to the zero corner regardless of scale factor.
#[test]
fn zero_corner_maps_to_zero_corner() {
    assert_eq!(scale_to_native(0, 0, 1920, 1080), (0, 0));
}

/// Near-corner exact tie: `x = 1279 * 1.5 = 1918.5` is an EXACT `.5` tie —
/// per the corrected MCP-04 spec (commit `4736a3b`), `f64::round()`'s
/// round-half-AWAY-FROM-ZERO tie-break rounds this UP to 1919, not down to
/// 1918 (truncation) and not to the nearest EVEN value (banker's
/// rounding). `y = 799 * 1.35 = 1078.65` rounds to 1079 with no tie
/// involved. Both results land strictly inside `[0, 1920) x [0, 1080)`.
#[test]
fn near_corner_exact_tie_rounds_half_away_from_zero() {
    assert_eq!(scale_to_native(1279, 799, 1920, 1080), (1919, 1079));
}

/// The symmetric near-corner on the opposite diagonal: `x = 0`, `y = 799 *
/// 1.35 = 1078.65 -> 1079` (bottom-left near-corner, no tie on this axis).
#[test]
fn bottom_left_near_corner() {
    assert_eq!(scale_to_native(0, 799, 1920, 1080), (0, 1079));
}

/// The top-right near-corner: `x = 1279 * 1.5 = 1918.5` is the SAME exact
/// tie as the primary near-corner vector — must also round UP to 1919.
/// `y = 0`.
#[test]
fn top_right_near_corner_shares_the_exact_tie() {
    assert_eq!(scale_to_native(1279, 0, 1920, 1080), (1919, 0));
}

/// Center sanity: `640 * 1.5 = 960`, `400 * 1.35 = 540` — no rounding
/// ambiguity at all.
#[test]
fn center_sanity() {
    assert_eq!(scale_to_native(640, 400, 1920, 1080), (960, 540));
}

/// The exclusive advertised-resolution edge (`1280, 800` — one past the
/// last valid advertised coordinate `1279, 799`) that a model may still
/// legally emit MUST clamp strictly inside native bounds, never landing
/// exactly on (or past) `native_w`/`native_h` — `Session::check_bounds`
/// would reject an exact-edge value, and the bridge itself (not that
/// downstream check) is responsible for never emitting one (Pitfall 2).
#[test]
fn advertised_exclusive_edge_clamps_strictly_inside_native_bounds() {
    let (nx, ny) = scale_to_native(1280, 800, 1920, 1080);
    assert!(nx < 1920, "nx must be < native_w, got {nx}");
    assert!(ny < 1080, "ny must be < native_h, got {ny}");
}

/// The same exclusive-edge clamp discipline holds across a spread of
/// native resolutions, not just 1920x1080.
#[test]
fn exclusive_edge_clamps_across_multiple_native_resolutions() {
    for (native_w, native_h) in [(1280u32, 800u32), (1366, 768), (2560, 1440), (3840, 2160)] {
        let (nx, ny) = scale_to_native(ADVERTISED_WIDTH, ADVERTISED_HEIGHT, native_w, native_h);
        assert!(nx < native_w as u16, "nx must be < {native_w}, got {nx}");
        assert!(ny < native_h as u16, "ny must be < {native_h}, got {ny}");
    }
}

/// A square-scale case (native == advertised, 1280x800): scale factors are
/// exactly 1.0, so every interior point maps to itself — the identity
/// mapping, with zero rounding drift.
#[test]
fn square_scale_native_equals_advertised_is_identity_for_interior_points() {
    assert_eq!(scale_to_native(0, 0, ADVERTISED_WIDTH, ADVERTISED_HEIGHT), (0, 0));
    assert_eq!(scale_to_native(640, 400, ADVERTISED_WIDTH, ADVERTISED_HEIGHT), (640, 400));
    assert_eq!(scale_to_native(1279, 799, ADVERTISED_WIDTH, ADVERTISED_HEIGHT), (1279, 799));
}
