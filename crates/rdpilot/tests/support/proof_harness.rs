//! Shared proof-harness composition (D-9.3): the single `run_proof_harness`
//! entry point consumed by BOTH `examples/proof_harness.rs` and the gated
//! `proof_harness_end_to_end` test in `tests/live_session.rs`, wired in via
//! `#[path]` module declarations from each crate root.
//!
//! # Pitfall 5 — this file is compiled TWICE, in two DIFFERENT crate roots
//!
//! `examples/proof_harness.rs` declares
//! `#[path = "../tests/support/proof_harness.rs"] mod proof_harness;` (its own
//! binary crate), and `tests/live_session.rs` declares
//! `#[path = "support/proof_harness.rs"] mod proof_harness;` (the
//! `live_session` integration-test binary crate). Each compilation produces an
//! entirely separate module tree — `crate::` inside THIS file means whichever
//! crate root happened to include it, NOT the `rdpilot` library, and there is
//! no shared state between the two compiled copies. Always reach the SDK via
//! the full `rdpilot::` path (`use rdpilot::{Session, ...}`), exactly as
//! `live_session.rs`'s own top-level `use rdpilot::{...}` already does — never
//! a `crate::`-qualified `Session` or any bare name that only resolves via a
//! local `use` re-export in one of the two callers.
//!
//! This module lives under `tests/support/`, a test/example-support crate
//! root member — it sits OUTSIDE the library's `#![deny(clippy::expect_used)]`
//! / `#![deny(clippy::unwrap_used)]` scope (`crates/rdpilot/src/lib.rs`'s
//! inner attributes only apply to the library's own compilation unit), so
//! `.expect()` would be permitted here exactly as it is in the rest of
//! `tests/live_session.rs`. The RETURNED error type is still the SDK's own
//! `rdpilot::Result` — no bespoke harness error enum (D-09).

use rdpilot::{Button, Key, KeyAction, MouseAction, Rect, Screenshot, Session, UiaElement, UiaScope, WindowInfo};

/// The full, individually quoted 7-Zip File Manager install path (D-9.6,
/// human-confirmed live by 09-01). The sensor's
/// `CreateProcessW(lpApplicationName=null, ...)` call does NOT PATH-search a
/// bare filename — a bare `"7zFM.exe"` fails live with `LastError=2`
/// (`ERROR_FILE_NOT_FOUND`); only the complete, quoted path works.
const SEVEN_ZIP_EXE: &str = "\"C:\\Program Files\\7-Zip\\7zFM.exe\"";

/// D-9.6 seed argument: pre-navigates 7zFM to a known, always-present folder
/// (`C:\Program Files`) so its listview/title are deterministic rather than
/// whatever 7-Zip's default view happens to be. Individually quoted — the
/// sensor concatenates `exe` + `args` into a single command line before
/// calling `CreateProcessW`, so both parts need their own quoting.
const SEVEN_ZIP_SEED_ARGS: &str = "\"C:\\Program Files\"";

/// The exact, human-confirmed (09-01) window class for 7-Zip File Manager.
/// Matched EXACTLY — never a title substring, since the title verbatim
/// reflects whichever folder is currently navigated (unstable; the 09-01
/// spike observed it read `"C:\Program Files\"` purely because of the D-9.6
/// seed path).
const SEVEN_ZIP_CLASS: &str = "7-Zip::FM";

/// SC#2 deeper-walk request depth. `09-02-SUMMARY.md` records the sensor-side
/// safety cap `UIA_MAX_WALK_DEPTH = 4` (the sensor always takes
/// `Math.Min(caller_max_depth, UIA_MAX_WALK_DEPTH)`); requesting the full
/// budget here gives the walk the best chance of reaching the deeper
/// elements the 09-01 spike proved were one-or-more levels below the four
/// depth-1 containers (menu items nested under `MenuBar`, toolbar buttons
/// under `ToolBar`, and the seeded `C:\Program Files` listview rows nested
/// under `Pane`) — the caller-supplied value never bypasses the sensor's own
/// cap, so this cannot regress the Phase 7 SC#3 latency budget beyond what
/// 09-02 already bounds.
/// LIVE-TUNED (09-04 gate): the initial value of 4 (the sensor's full
/// `UIA_MAX_WALK_DEPTH` cap) measured a real deeper-walk latency of 555.2ms
/// against 7-Zip's actual tree (179 total elements) — over the Phase 7 SC#3
/// 500ms sensor-side budget (T-09-12). All SC#2-recorded deeper elements
/// (menu items, toolbar buttons) were observed at depth=2, so the walk does
/// not need to reach depth 4 to satisfy SC#2. Tuned down to 3 (one level of
/// headroom past the observed depth=2 matches, in case a listview row sits
/// one level deeper under `Pane` than the menu/toolbar items do) and
/// re-measured live — see 09-04-SUMMARY.md for the re-measured latency.
const SC2_MAX_DEPTH: u32 = 3;

/// Menu-bar item names the 09-01 spike's captured screenshot showed present
/// on 7zFM's menu bar (`File Edit View Favorites Tools Help`) but ABSENT from
/// the `TreeScope_Children`-only dump (09-01-SUMMARY.md) — the canonical
/// D-9.2 "menu-open" navigation default.
const MENU_ITEM_NAMES: &[&str] = &["File", "Edit", "View", "Favorites", "Tools", "Help"];

/// Toolbar button names the 09-01 spike's captured screenshot showed present
/// (`Add Extract Test Copy Move Delete Info`) but likewise ABSENT from the
/// depth-1 dump.
const TOOLBAR_BUTTON_NAMES: &[&str] = &["Add", "Extract", "Test", "Copy", "Move", "Delete", "Info"];

/// Region-diff threshold, mirroring `live_session.rs`'s
/// `REGION_CHANGE_THRESHOLD` (D-3.4) — the fraction of pixels within a
/// cropped rect that must differ before a region counts as "meaningfully
/// changed" (well above ordinary codec/cursor noise, low enough that a real
/// menu/dropdown opening reliably crosses it).
const REGION_CHANGE_THRESHOLD: f32 = 0.01;

/// A single recorded harness step: `(step_name, passed, detail)` — the D-9.4
/// stdout trace shape.
pub type ProofStep = (&'static str, bool, String);

/// The full proof-harness run result (D-9.4): an ordered step trace plus the
/// overall pass/fail.
pub struct ProofReport {
    /// Ordered `(step_name, passed, detail)` entries — the D-9.4 stdout
    /// trace: `screenshot`, `launch_7zip`, `uia_deeper_walk`, `navigate`,
    /// `verify_navigation` (fewer entries if an earlier step aborted the
    /// run).
    pub steps: Vec<ProofStep>,
    /// `true` only if every recorded step passed.
    pub passed: bool,
}

/// Run the full scripted proof loop (PROOF-01): screenshot [SC#1] -> launch
/// 7-Zip (D-9.6 seeding) and find its window (exact `"7-Zip::FM"` class,
/// D-9.1) -> a deeper UIA walk asserting real 7-Zip elements [SC#2, via the
/// 09-02 `UiaScope::Subtree`] -> navigate (D-9.2 menu-open default) [SC#3] ->
/// bounded-poll verify (D-9.5) [SC#3 verify].
///
/// A logical assertion failure (element not found, navigation didn't
/// visibly change the desktop) is recorded as a failed step and the run
/// still returns `Ok(ProofReport { passed: false, .. })` — subsequent steps
/// that depend on the failed one are simply not attempted. Only a hard SDK
/// transport error (a broken connection, a malformed sensor reply) ever
/// propagates as `Err(rdpilot::Error)` (Pitfall 5 / D-09: the SDK's own
/// `Result`, no bespoke harness error type).
pub async fn run_proof_harness(session: &Session) -> rdpilot::Result<ProofReport> {
    let mut steps: Vec<ProofStep> = Vec::new();

    // Step 1 / SC#1: screenshot succeeds with nonzero dimensions.
    let shot = session.screenshot().await?;
    let sc1_pass = shot.width > 0 && shot.height > 0;
    steps.push((
        "screenshot",
        sc1_pass,
        format!("{}x{} pixels", shot.width, shot.height),
    ));
    if !sc1_pass {
        return Ok(finish(steps));
    }

    // Step 2: launch 7-Zip (D-9.6 seeding) and find its window (exact
    // "7-Zip::FM" class match, D-9.1).
    let window = match launch_7zip_and_find_window(session).await? {
        Some(w) => {
            steps.push((
                "launch_7zip",
                true,
                format!("hwnd={} class={:?} rect={:?}", w.hwnd, w.class_name, w.rect),
            ));
            w
        }
        None => {
            steps.push((
                "launch_7zip",
                false,
                format!("no window with class {SEVEN_ZIP_CLASS:?} appeared within the poll budget"),
            ));
            return Ok(finish(steps));
        }
    };

    // Step 3 / SC#2: a deeper UIA walk (UiaScope::Subtree), asserting REAL
    // deeper elements (menu items / toolbar buttons / listview rows) are
    // present — NOT just the 5 depth-1 containers the 09-01 spike proved are
    // all `TreeScope_Children` returns.
    // [Rule 2] Timed independently of the surrounding poll/launch steps so the
    // SUMMARY can record the deeper-walk latency against the Phase 7 SC#3
    // 500ms sensor-side budget (T-09-12 mitigation) — measures exactly the
    // get_uia_tree round-trip, not the whole harness run.
    let uia_walk_start = std::time::Instant::now();
    let elements = session
        .get_uia_tree(window.hwnd, UiaScope::Subtree { max_depth: SC2_MAX_DEPTH })
        .await?;
    let uia_walk_elapsed = uia_walk_start.elapsed();
    let deeper = find_deeper_elements(&elements, window.rect);
    if deeper.is_empty() {
        steps.push((
            "uia_deeper_walk",
            false,
            format!(
                "expected at least one deeper (depth > 1) menu item / toolbar button / \
                 listview row with a nonzero-area bbox inside the window rect — got 0 of {} \
                 total elements (roles seen: {:?})",
                elements.len(),
                elements.iter().map(|e| e.role.as_str()).collect::<Vec<_>>()
            ),
        ));
        return Ok(finish(steps));
    }
    steps.push((
        "uia_deeper_walk",
        true,
        format!(
            "found {} deeper element(s) beyond the depth-1 containers (e.g. {} {:?} depth={} \
             bbox={:?}) out of {} total elements — get_uia_tree latency {:.1}ms (Subtree \
             max_depth={SC2_MAX_DEPTH}, Phase 7 SC#3 budget 500ms)",
            deeper.len(),
            deeper[0].role,
            deeper[0].name,
            deeper[0].depth,
            deeper[0].bbox,
            elements.len(),
            uia_walk_elapsed.as_secs_f64() * 1000.0
        ),
    ));

    // Step 4 / SC#3: navigate — D-9.2's safe default is a menu-open click
    // (prefer "File" if present; else the first recorded deeper element).
    let target = pick_navigation_target(&deeper);
    let (cx, cy) = bbox_center(target.bbox);

    // [Rule 1 fix] Ensure the 7-Zip window actually holds OS foreground focus
    // before clicking. `launch_process` is fire-and-forget (D-6.2) and does
    // NOT guarantee focus — Phase 6 SC#3's live diagnosis
    // (`set_foreground_window_confirmed_by_followup_query`) established that
    // a freshly launched window can sit topmost-but-unfocused, and a single
    // click on a menu-bar item of an unfocused window is commonly consumed
    // as a pure activation click (bringing the window forward) rather than
    // also opening the menu. Every other live navigation test in this crate
    // (`live_session.rs`) explicitly focuses+settles before clicking; this
    // harness omitted that step.
    if let Err(e) = session.set_foreground_window(window.hwnd).await {
        eprintln!(
            "[proof] note: set_foreground_window before navigate failed \
             (non-fatal, click may still land): {e}"
        );
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let before = session.screenshot().await?;
    match session
        .send_mouse(MouseAction::Click {
            x: u16::try_from(cx).unwrap_or(u16::MAX),
            y: u16::try_from(cy).unwrap_or(u16::MAX),
            button: Button::Left,
        })
        .await
    {
        Ok(()) => steps.push((
            "navigate",
            true,
            format!("clicked {} {:?} at ({cx},{cy})", target.role, target.name),
        )),
        Err(e) => {
            steps.push(("navigate", false, format!("send_mouse failed: {e}")));
            return Ok(finish(steps));
        }
    }

    // Step 5 / SC#3 verify (D-9.5): bounded poll-with-timeout — never a lone
    // fixed sleep — confirming the click visibly changed the desktop around
    // the clicked element (a menu/dropdown opening).
    let (desktop_w, desktop_h) = session.desktop_size();
    let verify_rect = clamped_rect(
        target.bbox.x.saturating_sub(20),
        target.bbox.y,
        target.bbox.w.saturating_add(300),
        target.bbox.h.saturating_add(300),
        desktop_w,
        desktop_h,
    );
    let mut verified = false;
    let mut detail = String::new();
    for attempt in 0..15u32 {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let after = session.screenshot().await?;
        if region_changed(&before, &after, verify_rect) {
            verified = true;
            detail = format!(
                "region around ({},{}) visibly changed after {} poll attempt(s)",
                target.bbox.x,
                target.bbox.y,
                attempt + 1
            );
            break;
        }
        eprintln!("[proof] poll: waiting for navigation to visibly change (attempt {attempt})");
        detail = format!("no visible change in {verify_rect:?} after {} poll attempt(s)", attempt + 1);
    }
    steps.push(("verify_navigation", verified, detail));

    // Best-effort dismissal of whatever the click opened, so the desktop is
    // left clean for a re-run. Not a harness step (D-9.4 stays scoped to the
    // 4 SCs) — a failure here is only logged, never recorded as a step.
    if let Err(e) = session.send_key(KeyAction::Combo(vec![Key::Esc])).await {
        eprintln!("[proof] note: Esc dismiss after navigation failed (non-fatal): {e}");
    }

    Ok(finish(steps))
}

/// Assemble the final `ProofReport` — `passed` is `true` only when every
/// recorded step passed (and at least one step ran).
fn finish(steps: Vec<ProofStep>) -> ProofReport {
    let passed = !steps.is_empty() && steps.iter().all(|(_, pass, _)| *pass);
    ProofReport { steps, passed }
}

/// Launch 7-Zip (D-9.6 seeding form) and bounded-poll `get_window_list` until
/// the exact `"7-Zip::FM"`-classed window appears, returning `None` (a
/// logical, recordable failure) rather than panicking if it never does —
/// adapted from `live_session.rs`'s `launch_notepad_and_find_window`, whose
/// `launch_process` is fire-and-forget (D-6.2), so the window can take a
/// moment to register after the call returns.
async fn launch_7zip_and_find_window(session: &Session) -> rdpilot::Result<Option<WindowInfo>> {
    session
        .launch_process(SEVEN_ZIP_EXE, Some(SEVEN_ZIP_SEED_ARGS), None)
        .await?;

    for attempt in 0..20u32 {
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        let windows = session.get_window_list().await?;
        if let Some(w) = windows.iter().find(|w| w.class_name == SEVEN_ZIP_CLASS) {
            return Ok(Some(w.clone()));
        }
        eprintln!("[proof] poll: {SEVEN_ZIP_CLASS} window not yet visible (attempt {attempt})");
    }
    Ok(None)
}

/// True when `inner` lies fully within `outer` — scopes SC#2's deeper
/// elements to the target window's own rect, guarding against a stray
/// off-window match (e.g. a taskbar element sharing the same walk).
fn rect_contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x.saturating_add(inner.w) <= outer.x.saturating_add(outer.w)
        && inner.y.saturating_add(inner.h) <= outer.y.saturating_add(outer.h)
}

/// `true` when `e` matches one of the 09-01 spike's recorded DEEPER 7-Zip
/// elements — a named menu item, a named toolbar button, or a listview row
/// (`ListItem`/`DataItem`) from the seeded `C:\Program Files` listing.
fn is_recorded_deeper_element(e: &UiaElement) -> bool {
    (e.role == "MenuItem" && MENU_ITEM_NAMES.contains(&e.name.as_str()))
        || (e.role == "Button" && TOOLBAR_BUTTON_NAMES.contains(&e.name.as_str()))
        || e.role == "ListItem"
        || e.role == "DataItem"
}

/// Collects the elements matching [`is_recorded_deeper_element`] — depth > 1
/// (past the four named depth-1 containers: `ToolBar`/`Pane`/`TitleBar`/
/// `MenuBar`, per the 09-01 dump), a nonzero-area bbox, and located inside
/// `window_rect` [SC#2].
fn find_deeper_elements(elements: &[UiaElement], window_rect: Rect) -> Vec<&UiaElement> {
    elements
        .iter()
        .filter(|e| e.depth > 1)
        .filter(|e| e.bbox.w > 0 && e.bbox.h > 0)
        .filter(|e| rect_contains(window_rect, e.bbox))
        .filter(|e| is_recorded_deeper_element(e))
        .collect()
}

/// D-9.2's safe default: a menu-open click on `"File"` if present in the
/// matched deeper elements; otherwise the first recorded deeper element
/// found (still a real, individually-nameable, on-screen element — never a
/// depth-1 container).
fn pick_navigation_target<'a>(deeper: &[&'a UiaElement]) -> &'a UiaElement {
    deeper
        .iter()
        .find(|e| e.role == "MenuItem" && e.name == "File")
        .copied()
        .unwrap_or(deeper[0])
}

/// Center point of `bbox`, in physical virtual-desktop pixels — the click
/// target for [`MouseAction::Click`].
fn bbox_center(bbox: Rect) -> (u32, u32) {
    (bbox.x + bbox.w / 2, bbox.y + bbox.h / 2)
}

/// Fraction of pixels that differ between `before` and `after`, compared
/// pixel-for-pixel over their full extent (`0.0..=1.0`). Mirrors
/// `live_session.rs`'s helper of the same name (duplicated here, not
/// `super::`-referenced, because this module is compiled inside TWO
/// different crate roots — see the file's top doc comment).
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

/// `true` when a meaningful fraction ([`REGION_CHANGE_THRESHOLD`]) of pixels
/// within `rect` differ between `before` and `after` (D-3.4). Returns
/// `false` (never panics) if `rect` is out of bounds for either screenshot.
fn region_changed(before: &Screenshot, after: &Screenshot, rect: Rect) -> bool {
    let (Ok(before_crop), Ok(after_crop)) = (before.crop(rect), after.crop(rect)) else {
        return false;
    };
    changed_fraction(&before_crop, &after_crop) > REGION_CHANGE_THRESHOLD
}

/// Clamp a `w x h` rectangle anchored at `(x, y)` so it never exceeds
/// `(max_w, max_h)` — builds an "around the target element" region for
/// [`region_changed`] without risking an out-of-bounds crop.
fn clamped_rect(x: u32, y: u32, w: u32, h: u32, max_w: u32, max_h: u32) -> Rect {
    Rect {
        x,
        y,
        w: w.min(max_w.saturating_sub(x)),
        h: h.min(max_h.saturating_sub(y)),
    }
}
