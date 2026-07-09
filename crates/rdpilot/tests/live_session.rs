//! Gated live integration suite — one test per Phase 2 success criterion
//! (SESS-01, CAP-01 #2/#3, SESS-02 #4/#5).
//!
//! Every test is `#[ignore]`'d AND early-returns when `common::load_config()` is
//! `None`, so the default `cargo test -p rdpilot` stays green with no target
//! present (D-18). The canonical phase-gate run arms and includes them:
//!
//! ```text
//! RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1
//! ```
//!
//! `--test-threads=1` keeps the live tests from contending over the single VM.
//!
//! These tests exercise the PUBLIC API only (`Session`, `Screenshot`, `Rect`) —
//! no `ironrdp`/`image`/`rustls` type appears. Criterion #4 (stays-rendered while
//! idle) is verified BEHAVIORALLY by reading the framebuffer after the idle
//! window — there is no remote-registry / WinRM read anywhere (D-08).

mod common;

use rdpilot::{Button, Key, KeyAction, MouseAction, ProcessInfo, Rect, Screenshot, WindowInfo, WindowState};

/// Skip helper: returns the live config or prints a skip note and returns `None`.
/// Each test uses `let Some(cfg) = require_target!() else { return };`.
macro_rules! require_target {
    ($test:literal) => {{
        match common::load_config() {
            Some(cfg) => Some(cfg),
            None => {
                eprintln!(
                    "[skip] {}: live target not configured (set {}=1 and provide .secrets/connection.json)",
                    $test,
                    common::LIVE_ENV
                );
                None
            }
        }
    }};
}

/// True when the framebuffer is (near-)uniform mid-grey — the signature of the
/// YUV-grey decode pitfall (Pitfall 1). A correct RGBA32 desktop has real color
/// variation; a broken YUV path collapses to a flat ~(128,128,128) field.
///
/// Samples a grid of pixels and reports whether they are all close to neutral
/// grey. Used to FAIL criterion #2 if the screenshot is grey rather than RGB.
fn is_uniform_grey(shot: &Screenshot) -> bool {
    const GREY: i32 = 128;
    const TOL: i32 = 24;
    let w = shot.width;
    let h = shot.height;
    if w == 0 || h == 0 {
        return true; // empty is not a valid color image
    }
    // Sample an 8x8 grid across the image.
    for gy in 0..8u32 {
        for gx in 0..8u32 {
            let x = (gx * (w - 1)) / 7;
            let y = (gy * (h - 1)) / 7;
            let idx = ((y * w + x) * 4) as usize;
            let r = i32::from(shot.rgba[idx]);
            let g = i32::from(shot.rgba[idx + 1]);
            let b = i32::from(shot.rgba[idx + 2]);
            // If any sampled pixel is clearly NOT neutral grey, it is a real
            // colored desktop, not the YUV-grey failure.
            let near_grey = (r - GREY).abs() <= TOL
                && (g - GREY).abs() <= TOL
                && (b - GREY).abs() <= TOL;
            if !near_grey {
                return false;
            }
        }
    }
    true
}

/// True when the framebuffer is entirely a single flat color (e.g. all black /
/// all one value) — a blanked or never-rendered surface. A real rendered desktop
/// has more than one distinct pixel value.
fn is_blank(shot: &Screenshot) -> bool {
    if shot.rgba.len() < 4 {
        return true;
    }
    let first = &shot.rgba[0..4];
    shot.rgba.chunks_exact(4).all(|px| px == first)
}

/// Build a current-thread runtime to drive the async public API from a sync
/// `#[test]`. (The session loop runs on its own dedicated thread inside the SDK.)
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime")
        .block_on(fut)
}

/// Criterion #1 / SESS-01: `Session::connect` reaches an authenticated active
/// session against the live target.
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn connect_authenticates() {
    let Some(cfg) = require_target!("connect_authenticates") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg)
            .await
            .expect("connect/auth should reach an active session");
        session.close().await.expect("graceful close");
    });
}

/// Criterion #2 / CAP-01: a screenshot is correct-RGB (not YUV-grey, Pitfall 1)
/// and matches the requested desktop dimensions.
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn screenshot_is_rgb_correct() {
    let Some(cfg) = require_target!("screenshot_is_rgb_correct") else {
        return;
    };
    let want_w = u32::from(cfg.width());
    let want_h = u32::from(cfg.height());

    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        // Allow the first graphics update(s) to arrive, then give the shell a
        // brief moment to paint before capturing. The very first frame can be a
        // solid desktop-background fill before any window content renders.
        let _ = capture_when_ready(&session).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let shot = session.screenshot().await.expect("framebuffer available");
        session.close().await.expect("close");

        // Dimensions match the requested desktop size.
        assert_eq!(shot.width, want_w, "screenshot width matches requested desktop");
        assert_eq!(shot.height, want_h, "screenshot height matches requested desktop");
        assert_eq!(
            shot.rgba.len(),
            (want_w * want_h * 4) as usize,
            "RGBA buffer is tightly packed w*h*4"
        );

        // Concrete RGB acceptance (criterion #2): a fully-opaque, correctly-colored
        // desktop — NOT the flat YUV-grey field a broken decode produces (Pitfall 1).
        // The alpha of the top-left pixel must be the opaque 0xFF the RgbA32 path
        // emits (a known, concrete channel value), and the sampled grid must not be
        // uniform mid-grey. (Content-richness / non-blank after settle is the
        // domain of `stays_rendered_while_idle`; a uniform solid desktop background
        // is still correct RGB, so it is NOT asserted blank here.)
        assert_eq!(shot.rgba[3], 255, "top-left pixel alpha is opaque (0xFF), proving RGBA32 layout");
        assert!(
            !is_uniform_grey(&shot),
            "screenshot is uniform mid-grey — the YUV-grey decode pitfall (criterion #2 fails)"
        );
    });
}

/// Criterion #3 / CAP-01 #3: `Screenshot::crop` extracts a sub-image with the
/// expected dimensions and pixels, live end-to-end (crop math is also unit-tested
/// offline in Plan 01).
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn screenshot_crop() {
    let Some(cfg) = require_target!("screenshot_crop") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let shot = capture_when_ready(&session).await;
        session.close().await.expect("close");

        // Crop a 100x80 region at (10, 20) — well within any real desktop size.
        let rect = Rect { x: 10, y: 20, w: 100, h: 80 };
        let cropped = shot.crop(rect).expect("in-bounds crop succeeds on a live frame");

        assert_eq!(cropped.width, 100, "crop width matches the requested rect");
        assert_eq!(cropped.height, 80, "crop height matches the requested rect");
        assert_eq!(cropped.rgba.len(), (100 * 80 * 4) as usize);

        // The crop's first pixel must equal the source pixel at (10, 20) — proves
        // the live crop pulls the correct sub-region (not a re-decode / offset).
        let src_idx = ((20 * shot.width + 10) * 4) as usize;
        assert_eq!(
            &cropped.rgba[0..4],
            &shot.rgba[src_idx..src_idx + 4],
            "crop origin pixel equals the source pixel at (10,20)"
        );
    });
}

/// Criterion #4 / SESS-02: a windowless session stays full-resolution and
/// non-blank after the idle window. Verified BEHAVIORALLY from the framebuffer —
/// no remote-registry / WinRM read (D-08).
#[test]
#[ignore = "live: requires a provisioned RDP target + idle window (RDPILOT_LIVE=1)"]
fn stays_rendered_while_idle() {
    let Some(cfg) = require_target!("stays_rendered_while_idle") else {
        return;
    };
    let want_w = u32::from(cfg.width());
    let want_h = u32::from(cfg.height());
    let idle = std::time::Duration::from_secs(common::idle_secs());

    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        // Confirm we start rendered.
        let _ = capture_when_ready(&session).await;

        // Hold the session idle (no input from us; the SDK keepalive runs on its
        // own). This is the windowless/idle window of criterion #4.
        tokio::time::sleep(idle).await;

        // After idle, the framebuffer must still be full-resolution and non-blank.
        let after = session.screenshot().await.expect("framebuffer still available after idle");
        session.close().await.expect("close");

        assert_eq!(after.width, want_w, "still full-resolution width after idle");
        assert_eq!(after.height, want_h, "still full-resolution height after idle");
        assert!(!is_blank(&after), "windowless session is non-blank after idle (criterion #4)");
        assert!(!is_uniform_grey(&after), "windowless session is not grey after idle");
    });
}

/// Criterion #5 / SESS-02: after the full idle window the session is still alive
/// and `screenshot()` succeeds — the automatic keepalive prevented an idle
/// disconnect (D-06).
#[test]
#[ignore = "live: requires a provisioned RDP target + 10-min idle (RDPILOT_LIVE=1)"]
fn keepalive_survives_10min() {
    let Some(cfg) = require_target!("keepalive_survives_10min") else {
        return;
    };
    let idle = std::time::Duration::from_secs(common::idle_secs());

    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let _ = capture_when_ready(&session).await;

        // Idle through the keepalive window (full 600s in the canonical run).
        tokio::time::sleep(idle).await;

        // If the keepalive failed, the loop would have terminated on idle
        // disconnect and `screenshot()` would error. Success here proves the
        // session survived the idle window.
        let shot = session
            .screenshot()
            .await
            .expect("session still alive after idle — screenshot succeeds (keepalive worked)");
        session.close().await.expect("close");

        assert!(!shot.rgba.is_empty(), "post-idle screenshot has pixels");
    });
}

/// Threshold for [`region_changed`]: fraction of pixels within a cropped
/// rectangle that must differ before the region is considered "meaningfully
/// changed" (D-3.4) — well above ordinary sensor/codec noise (Pitfall 1's
/// artifacts, cursor blink, sub-pixel rendering jitter), but low enough that
/// a real menu/dialog/selection appearing reliably crosses it.
const REGION_CHANGE_THRESHOLD: f32 = 0.01;

/// Settle window after sending input, before capturing the "after"
/// screenshot — gives the remote desktop time to run its open/close/render
/// animation (menus, dialogs, typed text) before the framebuffer is sampled.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(800);

async fn settle() {
    tokio::time::sleep(SETTLE).await;
}

/// Fraction of pixels that differ between `before` and `after`, compared
/// pixel-for-pixel over their full extent (`0.0..=1.0`). Differing
/// dimensions are treated as "entirely changed" (`1.0`) rather than silently
/// comparing misaligned bytes.
fn changed_fraction(before: &Screenshot, after: &Screenshot) -> f32 {
    if before.width != after.width || before.height != after.height {
        return 1.0;
    }
    let total_px = (before.width as usize) * (before.height as usize);
    if total_px == 0 {
        return 0.0;
    }
    let mut changed = 0usize;
    for i in 0..total_px {
        let idx = i * 4;
        if before.rgba[idx..idx + 4] != after.rgba[idx..idx + 4] {
            changed += 1;
        }
    }
    changed as f32 / total_px as f32
}

/// True when a meaningful fraction ([`REGION_CHANGE_THRESHOLD`]) of pixels
/// within `rect` differ between `before` and `after` — the screenshot-diff
/// observable this suite uses to prove an input action visibly affected the
/// remote desktop (D-3.4; there is no ack path — RESEARCH Architecture step
/// 7). Crops both screenshots to `rect` first so the comparison is scoped to
/// the expected menu/dialog/selection region rather than the whole desktop
/// (unrelated background motion elsewhere should not produce a false
/// positive, and a small localized change should not be diluted into a false
/// negative by the rest of an unrelated frame). Returns `false` (never
/// panics) if `rect` is out of bounds for either screenshot.
fn region_changed(before: &Screenshot, after: &Screenshot, rect: Rect) -> bool {
    let (Ok(before_crop), Ok(after_crop)) = (before.crop(rect), after.crop(rect)) else {
        return false;
    };
    changed_fraction(&before_crop, &after_crop) > REGION_CHANGE_THRESHOLD
}

/// Clamp a `w x h` rectangle anchored at `(x, y)` so it never exceeds
/// `(max_w, max_h)` — builds an "around the click point" region for
/// `region_changed` without risking an out-of-bounds crop on a small VM
/// desktop.
fn clamped_rect(x: u32, y: u32, w: u32, h: u32, max_w: u32, max_h: u32) -> Rect {
    Rect {
        x,
        y,
        w: w.min(max_w.saturating_sub(x)),
        h: h.min(max_h.saturating_sub(y)),
    }
}

/// Criterion #1 / INPUT-01 (SC#1): a click at a known coordinate activates
/// the target element — a right-click on an empty desktop area opens the
/// desktop context menu, observed via screenshot-diff (D-3.4). Uses an
/// app-independent, standard-integrity, reversible OS surface (process
/// launch is Phase 6 — Pitfall 4) and dismisses the menu with Esc so the
/// desktop is left clean.
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn mouse_click_activates_menu() {
    let Some(cfg) = require_target!("mouse_click_activates_menu") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let _ = capture_when_ready(&session).await;
        settle().await;

        let (w, h) = session.desktop_size();
        // Center of the desktop — an area unlikely to have a desktop icon on
        // a stock VM image, so a right-click reliably opens the desktop
        // context menu rather than an icon's own context menu.
        let (cx, cy) = (w / 2, h / 2);

        let before = session.screenshot().await.expect("screenshot before click");

        session
            .send_mouse(MouseAction::Click {
                x: u16::try_from(cx).expect("desktop width fits u16"),
                y: u16::try_from(cy).expect("desktop height fits u16"),
                button: Button::Right,
            })
            .await
            .expect("right-click round-trips");
        settle().await;

        let after = session.screenshot().await.expect("screenshot after click");

        // The context menu renders anchored at (or near) the click point,
        // extending down and to the right (Windows may flip the anchor near
        // a screen edge; the desktop center avoids that case). A generous
        // 400x500 region captures the menu regardless of exact item count.
        let menu_rect = clamped_rect(cx, cy, 400, 500, w, h);
        assert!(
            region_changed(&before, &after, menu_rect),
            "right-click on an empty desktop area did not visibly open a context menu (SC#1)"
        );

        // Dismiss cleanly so the desktop is left as found.
        session
            .send_key(KeyAction::Combo(vec![Key::Esc]))
            .await
            .expect("Esc dismisses the context menu");
        settle().await;

        session.close().await.expect("close");
    });
}

/// Criterion #2 / INPUT-01: every `MouseAction` variant round-trips without
/// error, and the visually-observable ones (right-click menu, Start-menu
/// scroll) are confirmed via screenshot-diff (D-3.4). `DoubleClick` and
/// `Drag` are exercised for a clean round-trip only — see the DECISION POINT
/// comments at each for why a stronger screenshot-diff assertion is deferred
/// to the live checkpoint's empirical tuning (RESEARCH §4, D-3.7/D-3.8).
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn mouse_action_types_all_work() {
    let Some(cfg) = require_target!("mouse_action_types_all_work") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let _ = capture_when_ready(&session).await;
        settle().await;

        let (w, h) = session.desktop_size();
        let (cx, cy) = (
            u16::try_from(w / 2).expect("desktop width fits u16"),
            u16::try_from(h / 2).expect("desktop height fits u16"),
        );

        // Move: no button-state change; assert the round-trip is Ok and the
        // session stays alive (not independently visually observable).
        session
            .send_mouse(MouseAction::Move { x: cx, y: cy })
            .await
            .expect("Move round-trips");
        assert!(session.screenshot().await.is_ok(), "session alive after Move");

        // Left click: assert Ok + alive. Left-clicking empty desktop
        // typically has no visible effect worth screenshot-diffing.
        session
            .send_mouse(MouseAction::Click { x: cx, y: cy, button: Button::Left })
            .await
            .expect("left Click round-trips");
        assert!(session.screenshot().await.is_ok(), "session alive after left Click");

        // Middle click: assert Ok + alive (no default OS-level visible
        // effect on an empty desktop area).
        session
            .send_mouse(MouseAction::Click { x: cx, y: cy, button: Button::Middle })
            .await
            .expect("middle Click round-trips");
        assert!(session.screenshot().await.is_ok(), "session alive after middle Click");

        // Right click IS observable: the desktop context menu opens.
        let before_right = session.screenshot().await.expect("screenshot before right click");
        session
            .send_mouse(MouseAction::Click { x: cx, y: cy, button: Button::Right })
            .await
            .expect("right Click round-trips");
        settle().await;
        let after_right = session.screenshot().await.expect("screenshot after right click");
        let menu_rect = clamped_rect(u32::from(cx), u32::from(cy), 400, 500, w, h);
        assert!(
            region_changed(&before_right, &after_right, menu_rect),
            "right-click did not visibly open the context menu"
        );
        session
            .send_key(KeyAction::Combo(vec![Key::Esc]))
            .await
            .expect("Esc dismisses menu");
        settle().await;

        // DoubleClick — DECISION POINT (D-3.7 live-verify): an empty desktop
        // area has no icon to activate via double-click, so there is no
        // reliable screenshot-diff target here offline. Assert only that the
        // round-trip is Ok and the session stays alive. If a stronger live
        // proof is wanted, double-click a known desktop icon (e.g. Recycle
        // Bin, near the top-left on a stock image) at its actual observed
        // coordinate and assert `region_changed` (a window opens) — confirm
        // the icon's coordinate live and, if the double-click gap
        // (`DOUBLE_CLICK_GAP` in session.rs) does not reliably register,
        // increase it here at the checkpoint (RESEARCH §4).
        session
            .send_mouse(MouseAction::DoubleClick { x: cx, y: cy, button: Button::Left })
            .await
            .expect("DoubleClick round-trips");
        assert!(session.screenshot().await.is_ok(), "session alive after DoubleClick");

        // Scroll: open the Start menu (a reversible, standard-integrity,
        // app-independent surface, Pitfall 4) and scroll its app list.
        //
        // DECISION POINT (resolved live at the Plan 04 checkpoint): a
        // screenshot-diff assertion was attempted here, but this VM's
        // Windows Server 2022 Start layout renders its alphabetical app
        // list (currently a handful of entries: 7-Zip, Azure Arc Setup,
        // Microsoft Edge, Server Manager, Settings, Windows Accessories/
        // Administrative Tools/Ease of Access/PowerShell/Security/System)
        // entirely within one screen — there is no scrollable overflow to
        // produce a visible diff, confirmed empirically via screenshot
        // (not a product defect: `send_mouse(Scroll)` still round-trips
        // correctly). Reaching an overflowing app list would require
        // launching additional apps to populate Start, which is out of
        // scope here (process-launch is Phase 6, Pitfall 4). So — like
        // `DoubleClick`/`Drag` below — Scroll's acceptance on this image is
        // round-trip-only: Ok + session stays alive. If a future VM image
        // has enough Start entries to overflow, swap back to a
        // `changed_fraction` screenshot-diff assertion here.
        session
            .send_key(KeyAction::Combo(vec![Key::Ctrl, Key::Esc]))
            .await
            .expect("Ctrl+Esc opens Start");
        settle().await;
        session
            .send_mouse(MouseAction::Scroll { x: cx, y: cy, dy: -120 })
            .await
            .expect("Scroll(dy:-120) round-trips");
        settle().await;
        assert!(session.screenshot().await.is_ok(), "session alive after Scroll(dy:-120)");
        session
            .send_mouse(MouseAction::Scroll { x: cx, y: cy, dy: 120 })
            .await
            .expect("Scroll(dy:120) round-trips");
        settle().await;
        assert!(session.screenshot().await.is_ok(), "session alive after Scroll(dy:120)");

        session
            .send_key(KeyAction::Combo(vec![Key::Esc]))
            .await
            .expect("Esc closes Start");
        settle().await;

        // Drag — DECISION POINT (D-3.8 live-verify): dragging across two
        // empty desktop points has no lasting visual trace once the mouse
        // button releases (a selection rectangle is only visible mid-drag,
        // which this call does not expose). Assert only that the round-trip
        // is Ok and the session stays alive. If a stronger live proof is
        // wanted, drag a known desktop icon (coordinate confirmed live,
        // VM-image-specific) from `from` to `to` and assert `region_changed`
        // at both the origin and destination rects; if the drag does not
        // visibly register, increase `DRAG_INTERPOLATION_STEPS` (input.rs)
        // and/or `DRAG_STEP_GAP` (session.rs) at the checkpoint (RESEARCH §4
        // — `SM_CXDRAG`/`SM_CYDRAG` is a per-move distance threshold, not an
        // event-count threshold).
        let (from_x, from_y) = (cx, cy);
        let (to_x, to_y) = (cx.saturating_add(50), cy.saturating_add(50));
        session
            .send_mouse(MouseAction::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                button: Button::Left,
            })
            .await
            .expect("Drag round-trips");
        assert!(session.screenshot().await.is_ok(), "session alive after Drag");

        session.close().await.expect("close");
    });
}

/// Criterion #4 / SC#4 / D-3.2: `desktop_size()` reports the negotiated
/// physical pixel size, and the coordinate contract is *enforced* (not just
/// documented) — an out-of-bounds `send_mouse` coordinate is rejected with
/// `Error::CoordinateOutOfBounds` before any PDU is built, while a
/// well-formed in-bounds click at the same session still succeeds.
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn coordinate_contract_enforced() {
    let Some(cfg) = require_target!("coordinate_contract_enforced") else {
        return;
    };
    let want_w = u32::from(cfg.width());
    let want_h = u32::from(cfg.height());

    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let _ = capture_when_ready(&session).await;

        let (w, h) = session.desktop_size();
        assert_eq!(w, want_w, "desktop_size() reports the requested physical width");
        assert_eq!(h, want_h, "desktop_size() reports the requested physical height");

        // Out-of-bounds: exactly at the width bound (the bounds check is
        // `>=`, so `x == w` is the smallest rejected value).
        let oob_x = u16::try_from(w).expect("desktop width fits u16");
        let err = session
            .send_mouse(MouseAction::Click { x: oob_x, y: 0, button: Button::Left })
            .await
            .expect_err("an out-of-bounds coordinate must be rejected, not silently sent");
        assert!(
            matches!(err, rdpilot::Error::CoordinateOutOfBounds { .. }),
            "expected Error::CoordinateOutOfBounds, got {err:?}"
        );

        // A well-formed in-bounds click at the SAME session still succeeds —
        // proves the rejection is a per-call bounds check, not a
        // session-level failure state.
        session
            .send_mouse(MouseAction::Click { x: 10, y: 10, button: Button::Left })
            .await
            .expect("an in-bounds click still succeeds after a prior rejection");

        session.close().await.expect("close");
    });
}

/// Criterion #3 / INPUT-02 (SC#3): typed text is received by the remote
/// session, observed via screenshot-diff on a reversible, app-independent,
/// standard-integrity surface (process launch is Phase 6 — Pitfall 4): the
/// Start menu's search box, opened via `Ctrl+Esc` (a `KeyAction::Combo`, not
/// a launched app).
///
/// NEVER asserts on or logs the typed string itself beyond its presence via
/// the screenshot diff (Security V5) — only the boolean "did the frame
/// change" is asserted.
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn keyboard_typed_text_is_received() {
    let Some(cfg) = require_target!("keyboard_typed_text_is_received") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let _ = capture_when_ready(&session).await;
        settle().await;

        // Open Start — its search box takes input focus immediately, no
        // click needed. (Ctrl+Esc's own screen-change is independently
        // exercised in `keyboard_combos_are_received`; this test's own
        // before/after pair isolates the text-render change.)
        session
            .send_key(KeyAction::Combo(vec![Key::Ctrl, Key::Esc]))
            .await
            .expect("Ctrl+Esc opens Start");
        settle().await;

        let before_type = session.screenshot().await.expect("screenshot before typing");

        session
            .send_key(KeyAction::Type("rdpilot".into()))
            .await
            .expect("Type round-trips");
        settle().await;

        let after_type = session.screenshot().await.expect("screenshot after typing");

        // Whole-frame comparison at a low threshold: the typed query renders
        // in a small search-box region relative to the full desktop, so
        // REGION_CHANGE_THRESHOLD (tuned for larger cropped regions like a
        // context menu) is not used directly here. DECISION POINT: if flaky
        // on the live VM, narrow this to a rect scoped to the actual
        // observed search-box bounds (Windows 10 vs 11 Start layouts place
        // it differently) at the checkpoint.
        const TYPED_TEXT_CHANGE_THRESHOLD: f32 = 0.0005;
        assert!(
            changed_fraction(&before_type, &after_type) > TYPED_TEXT_CHANGE_THRESHOLD,
            "typed text did not visibly render in the Start search box (SC#3)"
        );

        // Dismiss cleanly.
        session
            .send_key(KeyAction::Combo(vec![Key::Esc]))
            .await
            .expect("Esc closes Start");
        settle().await;

        session.close().await.expect("close");
    });
}

/// Criterion #3 / INPUT-02 (SC#3): key COMBINATIONS are received — `Ctrl+Esc`
/// (opens Start, screenshot-diff observable), `Alt+F4` (opens the "Shut Down
/// Windows" dialog on a focused desktop, screenshot-diff observable, then
/// CANCELLED with Esc so the VM is left untouched — T-03-15), and `Ctrl+A`
/// (round-trip only; a text-selection highlight is not reliably
/// screenshot-diffable). Targets are standard-integrity, reversible OS
/// surfaces only (process launch is Phase 6 — Pitfall 4; never an elevated
/// target).
#[test]
#[ignore = "live: requires a provisioned RDP target (RDPILOT_LIVE=1)"]
fn keyboard_combos_are_received() {
    let Some(cfg) = require_target!("keyboard_combos_are_received") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");
        let _ = capture_when_ready(&session).await;
        settle().await;

        // --- Ctrl+Esc opens Start: screenshot-diff observable. ---
        const COMBO_CHANGE_THRESHOLD: f32 = 0.001;
        let before_start = session.screenshot().await.expect("screenshot before Ctrl+Esc");
        session
            .send_key(KeyAction::Combo(vec![Key::Ctrl, Key::Esc]))
            .await
            .expect("Ctrl+Esc round-trips");
        settle().await;
        let after_start = session.screenshot().await.expect("screenshot after Ctrl+Esc");
        assert!(
            changed_fraction(&before_start, &after_start) > COMBO_CHANGE_THRESHOLD,
            "Ctrl+Esc did not visibly open the Start menu (SC#3)"
        );

        // --- Ctrl+A: round-trip only. A text-selection highlight in the
        // Start search box is not reliably screenshot-diffable (the exact
        // highlight color/extent is theme- and content-dependent), so the
        // acceptance here is that the combo completes without error and the
        // session stays alive — per the plan's explicit carve-out. ---
        session
            .send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A]))
            .await
            .expect("Ctrl+A round-trips");
        assert!(session.screenshot().await.is_ok(), "session alive after Ctrl+A");

        // Close Start before moving to the desktop-focused Alt+F4 case.
        session
            .send_key(KeyAction::Combo(vec![Key::Esc]))
            .await
            .expect("Esc closes Start");
        settle().await;

        // --- Alt+F4 on a focused desktop: opens the "Shut Down Windows"
        // dialog, screenshot-diff observable, then IMMEDIATELY CANCELLED
        // (T-03-15 — never let the shutdown dialog's default action fire;
        // the VM must be left untouched). ---
        let (w, h) = session.desktop_size();
        let (cx, cy) = (
            u16::try_from(w / 2).expect("desktop width fits u16"),
            u16::try_from(h / 2).expect("desktop height fits u16"),
        );
        // Click an empty desktop area first to give the desktop shell input
        // focus (Alt+F4 targets whatever currently has focus).
        session
            .send_mouse(MouseAction::Click { x: cx, y: cy, button: Button::Left })
            .await
            .expect("focusing click round-trips");
        settle().await;

        let before_altf4 = session.screenshot().await.expect("screenshot before Alt+F4");
        session
            .send_key(KeyAction::Combo(vec![Key::Alt, Key::F4]))
            .await
            .expect("Alt+F4 round-trips");
        settle().await;
        let after_altf4 = session.screenshot().await.expect("screenshot after Alt+F4");
        assert!(
            changed_fraction(&before_altf4, &after_altf4) > COMBO_CHANGE_THRESHOLD,
            "Alt+F4 did not visibly open the Shut Down Windows dialog (SC#3)"
        );

        // CANCEL immediately — never let this dialog's default action fire.
        session
            .send_key(KeyAction::Combo(vec![Key::Esc]))
            .await
            .expect("Esc cancels the Shut Down Windows dialog");
        settle().await;

        session.close().await.expect("close");
    });
}

/// SENSOR-02 SC#2 (MANDATORY, D-5.6) + SC#4: the RDPDR PRIMARY path deploys
/// the REAL `rdpilot-sensor.exe` in-band via drive-redirection copy and
/// launches it inside the RDP session with an injected Win+R command
/// (`Session::deploy_and_launch`, D-5.1/D-5.2 — poll-and-retry, no WinRM
/// involved at all). Supersedes Phase 4's throwaway
/// `sensor_ping_pong_under_500ms` (D-5.7; `sensor-responder.ps1` +
/// `deploy-responder.ps1` are deleted).
///
/// `deploy_and_launch`'s returned elapsed IS the SC4 measurement: the first
/// successful ping/pong after a SUCCESSFUL launch (not from the first
/// keystroke attempt, D-5.2) — must be under 1 second.
///
/// RDPDR success here is MANDATORY, not conditionally waivable (D-5.6): the
/// lab VM is configured to allow drive-redirection, and there is no
/// "conditional pass" that lets the WinRM fallback substitute for this path.
/// A failure here should be diagnosed as a drive-redirection Group Policy
/// block on the target (Computer Configuration > Administrative Templates >
/// Windows Components > Remote Desktop Services > Remote Desktop Session Host
/// > Device and Resource Redirection) before anything else.
#[test]
#[ignore = "live: requires a provisioned RDP target + published rdpilot-sensor.exe (RDPILOT_LIVE=1)"]
fn sensor_rdpdr_deploy_and_ping_within_1s() {
    let Some(cfg) = require_target!("sensor_rdpdr_deploy_and_ping_within_1s") else {
        return;
    };
    let sensor_exe = common::sensor_exe_path();
    assert!(
        sensor_exe.exists(),
        "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
         (Plan 01 Task 3: `dotnet publish -r win-x64 -p:PublishAot=true --self-contained` \
         on a Windows host with the .NET 8 SDK)"
    );
    let cfg = cfg.sensor_binary_path(sensor_exe);

    block_on(async {
        // Setting `sensor_binary_path` makes `connect` register the RDPDR
        // static channel automatically (connect.rs, Plan 03 Task 1) — no
        // separate opt-in call needed here.
        let session = rdpilot::Session::connect(&cfg)
            .await
            .expect("connect (RDPDR channel registers when sensor_binary_path is set)");

        let elapsed = session.deploy_and_launch().await.unwrap_or_else(|e| {
            panic!(
                "deploy_and_launch failed on the RDPDR primary path (SC2 mandatory, D-5.6) — \
                 check whether drive-redirection is blocked by Group Policy on the target; \
                 last error: {e:?}"
            )
        });

        session.close().await.expect("close");

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "RDPDR deploy+launch -> first pong took {elapsed:?}, expected < 1s (SC4)"
        );
        // Surfaced for the end-of-phase human-check record (SC4 measurement).
        println!("sensor_rdpdr_deploy_and_ping_within_1s: measured elapsed = {elapsed:?}");
    });
}

/// SENSOR-02 SC#3 + SC#4: the WinRM FALLBACK path proves the REAL
/// `rdpilot-sensor.exe`, deployed and armed by
/// `tests/fixtures/deploy-winrm.ps1` (an AtLogOn Interactive-principal
/// Scheduled Task — the same proven Phase 4 mechanism, adapted for the real
/// exe), answers ping/pong within 1 second of having been launched.
/// Supersedes Phase 4's throwaway `sensor_ping_pong_under_500ms` (D-5.7).
///
/// This test does NOT drive WinRM itself (no pwsh/WSMan reachable from the
/// Linux authoring sandbox) — deploying is a human/host step run BEFORE this
/// test, mirroring Phase 4's `deploy-responder.ps1` convention:
///
/// ```text
/// pwsh crates/rdpilot/tests/fixtures/deploy-winrm.ps1
/// RDPILOT_LIVE=1 cargo test -p rdpilot sensor_winrm_deploy_and_ping_within_1s -- --ignored --test-threads=1
/// pwsh crates/rdpilot/tests/fixtures/deploy-winrm.ps1 -Remove
/// ```
///
/// The 60s outer retry budget absorbs the AtLogOn scheduled-task's own
/// ~20-30s launch latency (RESEARCH Pitfall 5, 04-03-SUMMARY.md) — the SC4
/// "< 1s" bound is the elapsed of the FIRST successful `ping()`, measured
/// separately from that one-time setup wait the retries absorb.
#[test]
#[ignore = "live: requires a provisioned RDP target + deployed rdpilot-sensor.exe via deploy-winrm.ps1 (RDPILOT_LIVE=1)"]
fn sensor_winrm_deploy_and_ping_within_1s() {
    let Some(cfg) = require_target!("sensor_winrm_deploy_and_ping_within_1s") else {
        return;
    };
    block_on(async {
        let session = rdpilot::Session::connect(&cfg).await.expect("connect");

        const RETRY_BUDGET: std::time::Duration = std::time::Duration::from_secs(60);
        const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
        let deadline = std::time::Instant::now() + RETRY_BUDGET;

        let mut last_err: Option<rdpilot::Error> = None;
        let mut result: Option<std::time::Duration> = None;
        while std::time::Instant::now() < deadline {
            match session.ping().await {
                Ok(elapsed) => {
                    result = Some(elapsed);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(RETRY_INTERVAL).await;
                }
            }
        }

        session.close().await.expect("close");

        let elapsed = result.unwrap_or_else(|| {
            panic!(
                "no successful ping within {RETRY_BUDGET:?} — sensor not answering on \
                 RDPILOT_SENSOR (was `pwsh crates/rdpilot/tests/fixtures/deploy-winrm.ps1` run \
                 first? is rdpilot-sensor.exe running in the INTERACTIVE RDP session, not \
                 WinRM's Session 0? last error: {last_err:?})"
            )
        });

        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "WinRM-deployed ping/pong round trip took {elapsed:?}, expected < 1s (SC3 + SC4)"
        );
        // Surfaced for the end-of-phase human-check record (SC3/SC4 measurement).
        println!("sensor_winrm_deploy_and_ping_within_1s: measured round trip = {elapsed:?}");
    });
}

/// Poll `screenshot()` until the first graphics update lands (it errors with
/// `Error::Session` until then). Bounded so a stuck connection fails the test
/// rather than hanging forever.
async fn capture_when_ready(session: &rdpilot::Session) -> Screenshot {
    for _ in 0..100 {
        match session.screenshot().await {
            Ok(shot) => return shot,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    panic!("no framebuffer captured within 10s of connecting (no graphics update arrived)");
}

/// Phase 6 SC#1 / PERC-02: `get_window_list` returns visible windows with
/// HWND/title/rect/z-order/state/pid, and rects land in physical
/// virtual-desktop pixels — the same framebuffer coordinate space
/// `session.desktop_size()` reports (the Phase 7 crop dependency).
///
/// Brings the sensor up in-band via `deploy_and_launch()` (the proven Phase
/// 5 RDPDR path) before issuing any window/process request, per this plan's
/// `<interface_context>`.
#[test]
#[ignore = "live: requires a provisioned RDP target + published rdpilot-sensor.exe (RDPILOT_LIVE=1)"]
fn window_list_returns_visible_windows() {
    let Some(cfg) = require_target!("window_list_returns_visible_windows") else {
        return;
    };
    let sensor_exe = common::sensor_exe_path();
    assert!(
        sensor_exe.exists(),
        "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
         (`dotnet publish -r win-x64 -p:PublishAot=true --self-contained` on a Windows host \
         with the .NET 8 SDK)"
    );
    let cfg = cfg.sensor_binary_path(sensor_exe);

    block_on(async {
        let session = rdpilot::Session::connect(&cfg)
            .await
            .expect("connect (RDPDR channel registers when sensor_binary_path is set)");
        session
            .deploy_and_launch()
            .await
            .expect("deploy_and_launch: sensor must be answering before window/process requests");

        let (desktop_w, desktop_h) = session.desktop_size();

        let windows = session
            .get_window_list()
            .await
            .expect("get_window_list should round-trip successfully (SC#1)");
        assert!(!windows.is_empty(), "expected at least one visible window on the live desktop");

        let in_bounds = |w: &WindowInfo| {
            w.rect.x.saturating_add(w.rect.w) <= desktop_w
                && w.rect.y.saturating_add(w.rect.h) <= desktop_h
        };
        let candidate = windows
            .iter()
            .find(|w| !w.title.is_empty() && w.pid != 0 && in_bounds(w))
            .unwrap_or_else(|| {
                panic!(
                    "no window with a non-empty title, nonzero pid, and a rect within \
                     desktop_size ({desktop_w}x{desktop_h}) — windows seen: {windows:?}"
                )
            });
        // `state` is always a valid `WindowState` variant by construction
        // (Rust's type system) — assert it is one of the three documented
        // values for readability at the call site.
        assert!(matches!(
            candidate.state,
            WindowState::Normal | WindowState::Minimized | WindowState::Maximized
        ));

        // Optional CAP-02 smoke: per-window screenshot crop matches the
        // window's own rect dimensions. Soft — not a hard SC#1 gate (CAP-02
        // is otherwise covered by the offline crop tests in 06-02); log
        // rather than panic if the live crop is unavailable (e.g. the
        // candidate window is occluded/minimized at capture time).
        match session.screenshot_window(candidate).await {
            Ok(shot) => {
                assert_eq!(
                    (shot.width, shot.height),
                    (candidate.rect.w, candidate.rect.h),
                    "screenshot_window crop dimensions should match the window rect (CAP-02 smoke)"
                );
            }
            Err(e) => {
                eprintln!(
                    "[soft] screenshot_window CAP-02 smoke did not succeed for {candidate:?}: {e:?} \
                     (non-fatal — CAP-02 is covered offline by 06-02's crop tests)"
                );
            }
        }

        session.close().await.expect("close");
    });
}

/// Phase 6 SC#2 / PERC-01: `get_process_tree` returns pid/parent-pid/name
/// for every process, and path for at least one ordinary process
/// (best-effort path resolution, D-6.3). Deliberately does NOT assert on
/// `command_line`/`owner` — those are optional extras (D-6.3), not part of
/// this success criterion.
#[test]
#[ignore = "live: requires a provisioned RDP target + published rdpilot-sensor.exe (RDPILOT_LIVE=1)"]
fn process_tree_returns_pid_parent_name_path() {
    let Some(cfg) = require_target!("process_tree_returns_pid_parent_name_path") else {
        return;
    };
    let sensor_exe = common::sensor_exe_path();
    assert!(
        sensor_exe.exists(),
        "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
         (`dotnet publish -r win-x64 -p:PublishAot=true --self-contained` on a Windows host \
         with the .NET 8 SDK)"
    );
    let cfg = cfg.sensor_binary_path(sensor_exe);

    block_on(async {
        let session = rdpilot::Session::connect(&cfg)
            .await
            .expect("connect (RDPDR channel registers when sensor_binary_path is set)");
        session
            .deploy_and_launch()
            .await
            .expect("deploy_and_launch: sensor must be answering before window/process requests");

        let processes = session
            .get_process_tree()
            .await
            .expect("get_process_tree should round-trip successfully (SC#2)");
        assert!(!processes.is_empty(), "expected at least one process on the live desktop");

        let has_pid_and_name = processes.iter().any(|p: &ProcessInfo| p.pid != 0 && !p.name.is_empty());
        assert!(
            has_pid_and_name,
            "expected at least one process record with a nonzero pid and non-empty name"
        );

        let has_path = processes.iter().any(|p: &ProcessInfo| !p.path.is_empty());
        assert!(
            has_path,
            "expected at least one process record with a non-empty path (best-effort \
             path resolution should succeed for ordinary processes)"
        );

        session.close().await.expect("close");
    });
}

/// Phase 6 SC#3 / PERC-04: `set_foreground_window` brings a window forward,
/// confirmed by a SECOND `get_window_list()` query — NOT the setter's own
/// `Ok(())` return (Pitfall 5 / SC#3). Launches notepad.exe first (fire-
/// -and-forget, D-6.2) to get a window that is not already foreground, then
/// focuses it and re-queries.
#[test]
#[ignore = "live: requires a provisioned RDP target + published rdpilot-sensor.exe (RDPILOT_LIVE=1)"]
fn set_foreground_window_confirmed_by_followup_query() {
    let Some(cfg) = require_target!("set_foreground_window_confirmed_by_followup_query") else {
        return;
    };
    let sensor_exe = common::sensor_exe_path();
    assert!(
        sensor_exe.exists(),
        "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
         (`dotnet publish -r win-x64 -p:PublishAot=true --self-contained` on a Windows host \
         with the .NET 8 SDK)"
    );
    let cfg = cfg.sensor_binary_path(sensor_exe);

    block_on(async {
        let session = rdpilot::Session::connect(&cfg)
            .await
            .expect("connect (RDPDR channel registers when sensor_binary_path is set)");
        session
            .deploy_and_launch()
            .await
            .expect("deploy_and_launch: sensor must be answering before window/process requests");

        let pid = session
            .launch_process("notepad.exe", None, None)
            .await
            .expect("launch_process(notepad.exe) should return a PID");
        settle().await;

        let windows_before = session
            .get_window_list()
            .await
            .expect("get_window_list (pre-focus) should round-trip successfully");
        let target = windows_before
            .iter()
            .find(|w| w.pid == pid)
            .unwrap_or_else(|| {
                panic!(
                    "no window owned by launched pid {pid} found in the pre-focus window list \
                     — windows seen: {windows_before:?}"
                )
            });
        let hwnd = target.hwnd;

        session
            .set_foreground_window(hwnd)
            .await
            .expect("set_foreground_window should round-trip successfully");
        settle().await;

        // DECISION POINT (live tuning): if the Win32 foreground-lock timeout
        // (Pitfall 5, 06-RESEARCH) makes this flaky on the target VM — e.g.
        // the just-launched notepad window has not yet finished stealing
        // focus from the RDP shell — insert one additional `settle()` or a
        // short poll-retry around the follow-up `get_window_list()` call
        // here, and note the measured wait at the live-gate checkpoint.
        let windows_after = session
            .get_window_list()
            .await
            .expect("get_window_list (post-focus, SC#3 confirmation) should round-trip successfully");
        let confirmed = windows_after
            .iter()
            .find(|w| w.hwnd == hwnd)
            .unwrap_or_else(|| {
                panic!(
                    "hwnd {hwnd} (the window we just focused) no longer appears in the \
                     follow-up window list — windows seen: {windows_after:?}"
                )
            });
        let min_z_order = windows_after.iter().map(|w| w.z_order).min().unwrap_or(u32::MAX);
        assert_eq!(
            confirmed.z_order, min_z_order,
            "hwnd {hwnd} should be topmost (z_order == {min_z_order}) in the follow-up \
             window list after set_foreground_window (SC#3 — confirmation is the follow-up \
             query, not the setter's own return)"
        );

        session.close().await.expect("close");
    });
}

/// Phase 6 SC#4 / PROC-01: `launch_process` starts notepad.exe and the
/// returned PID appears in a follow-up `get_process_tree()` query
/// (fire-and-forget per D-6.2 — no wait/poll beyond a single `settle()`).
#[test]
#[ignore = "live: requires a provisioned RDP target + published rdpilot-sensor.exe (RDPILOT_LIVE=1)"]
fn launch_process_appears_in_followup_process_tree() {
    let Some(cfg) = require_target!("launch_process_appears_in_followup_process_tree") else {
        return;
    };
    let sensor_exe = common::sensor_exe_path();
    assert!(
        sensor_exe.exists(),
        "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
         (`dotnet publish -r win-x64 -p:PublishAot=true --self-contained` on a Windows host \
         with the .NET 8 SDK)"
    );
    let cfg = cfg.sensor_binary_path(sensor_exe);

    block_on(async {
        let session = rdpilot::Session::connect(&cfg)
            .await
            .expect("connect (RDPDR channel registers when sensor_binary_path is set)");
        session
            .deploy_and_launch()
            .await
            .expect("deploy_and_launch: sensor must be answering before window/process requests");

        let pid = session
            .launch_process("notepad.exe", None, None)
            .await
            .expect("launch_process(notepad.exe) should return a PID (SC#4)");
        settle().await;

        let processes = session
            .get_process_tree()
            .await
            .expect("get_process_tree (follow-up, SC#4 confirmation) should round-trip successfully");
        let launched = processes.iter().find(|p: &&ProcessInfo| p.pid == pid).unwrap_or_else(|| {
            panic!(
                "launched pid {pid} not found in the follow-up process tree — \
                 processes seen: {processes:?}"
            )
        });
        assert!(
            launched.name.to_ascii_lowercase().contains("notepad"),
            "expected the launched process's name to look like notepad.exe, got {:?}",
            launched.name
        );

        session.close().await.expect("close");
    });
}
