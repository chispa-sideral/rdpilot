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

use rdpilot::{Rect, Screenshot};

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
