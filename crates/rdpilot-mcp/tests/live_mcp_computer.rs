//! MCP-04 (live half) -- real click landing near screen edges/corners
//! through the `computer` mega-tool (gated, live-only).
//!
//! The PURE `scale_to_native` coordinate math is already offline-proven by
//! `tests/scale_to_native.rs` (BLOCKING, exact corner/tie-break arithmetic).
//! What that offline suite CANNOT prove is whether the scaled coordinates it
//! computes actually LAND on the intended element on a REAL Windows desktop
//! -- that is what this file closes, driving the REAL compiled
//! `rdpilot-mcp` binary as an rmcp client subprocess (identical Pattern 2 to
//! `tests/live_proof.rs`/PROOF-03) against a REAL remote Windows target.
//!
//! # Design: two well-known landmarks, not a computed target (Pitfall 3)
//!
//! The click coordinates below are HARDCODED in the advertised 1280x800
//! space, chosen because their expected target is a well-known, stable
//! Windows UI element -- never derived from a pre-click `rdpilot_uia` query.
//! `rdpilot_uia` is used AFTER each click ONLY to verify the click landed,
//! never to compute where to click (the coordinate REASONING never leaves
//! the advertised 1280x800 space; native-pixel bboxes read via UIA are used
//! only to name/describe what was clicked in the trace, not to pick a
//! target). This mirrors the plan's binding constraint precisely.
//!
//! 7-Zip File Manager is launched, foregrounded, then maximized via a
//! `computer` `key` action (`"win+up"`, no coordinate needed) so its own
//! window fills the screen -- at that point the SCREEN's corners and the
//! WINDOW's corners coincide, and two of 7-Zip FM's own chrome elements
//! become reliable, named landmarks at those corners:
//!
//! - **Top-left corner** -> the classic Win32 menu bar's leftmost item,
//!   `"File"` (7-Zip FM's menu bar starts flush at the window's top-left).
//!   Verified by re-fetching the window's UIA children after the click and
//!   checking that some element now reports `focused: true` (opening a menu
//!   moves focus onto it) -- checked by name (`"File"`) first, falling back
//!   to "any element focused" for robustness against exact UIA naming
//!   variance (mirrors `crates/rdpilot-cli/tests/live_cli_verbs.rs`'s CLI-02
//!   click-landing verification, which uses the identical "any focused"
//!   fallback).
//! - **Top-right corner** -> the standard Windows title-bar `"Minimize"`
//!   button (always the leftmost of the three window-control buttons,
//!   itself positioned at the extreme top-right of a maximized window).
//!   Verified via a SEPARATE, more decisive signal than UIA focus: a
//!   follow-up `rdpilot_window_list` poll must report the window's `state`
//!   transitioning to `"minimized"` -- clicking Minimize is the LAST corner
//!   test (nothing else needs the window visible afterward), so ending the
//!   sequence there is deliberate, not an oversight.
//!
//! Exact pixel offsets from the true corners are a documented ESTIMATE
//! (small margins into the menu bar / title bar), calibrated for real fit
//! only once this test actually RUNS against the live VM (Plan 15-06) --
//! this is expected and acceptable for OFFLINE-AUTHORED, live-run-deferred
//! code; a landing mismatch here is exactly the kind of finding Plan 15-06
//! exists to surface and fix, not a defect in this authoring pass.
//!
//! # Gating (D-18) / placement / security
//!
//! Identical to `tests/live_proof.rs`: `#[ignore]`-gated tokio test,
//! `RDPILOT_LIVE` + `.secrets/connection.json` opt-in (D-18), `tests/*.rs`
//! placement (`env!("CARGO_BIN_EXE_rdpilot-mcp")` only resolves there, not
//! in `--example` binaries), and the password is passed only as an MCP
//! tool-call JSON argument over the child's stdin -- never printed, never in
//! the child's env/argv (T-15-06).

use std::path::PathBuf;
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

// `computer/scale.rs` is a PURE, zero-dependency module (see its own doc
// comment and `tests/scale_to_native.rs`'s identical precedent) -- pulled in
// directly via `#[path]` so the advertised-space constants below are the
// EXACT same source the binary links, never a hand-duplicated literal that
// could silently drift from D-14.2 LOCKED (1280x800).
#[path = "../src/computer/scale.rs"]
mod scale;
use scale::ADVERTISED_WIDTH;

/// Name of the opt-in env var that arms this live suite (D-18) -- duplicated
/// from `crates/rdpilot/tests/common/mod.rs::LIVE_ENV` (unreachable here,
/// D-17), mirroring `tests/live_proof.rs`'s identical convention.
const LIVE_ENV: &str = "RDPILOT_LIVE";

/// Live-tuned (Plan 15-06) advertised-space coordinates for the top-left
/// "File" menu click, empirically calibrated against this VM's real
/// negotiated 1920x1080 desktop (`Request::DesktopSize`, confirmed via a
/// direct daemon probe) and 7-Zip FM's real, live-measured chrome layout
/// AFTER a genuine fresh maximize (see the restore-before-maximize doc
/// comment below -- 7-Zip FM's remembered "maximized" placement is stale
/// and undersized until forced through one restore/re-maximize cycle).
///
/// **What was live-diagnosed (two rounds):** round 1 measured the "File"
/// `MenuItem`'s native bbox against a STALE pre-restore maximize
/// (`x: 60..92, y: 83..102`) and produced `[51, 69]` -- this worked for the
/// window instance measured, but a fresh window instance's stale-maximize
/// bounds turned out NOT to be identical across launches (contrary to the
/// initial assumption), so round-1 coordinates were unreliable. Round 2
/// re-measured AFTER adding the restore-before-maximize fix, against the
/// TRUE full-screen bounds every window reaches once genuinely
/// re-maximized (`TitleBar` bbox spanning the real 0..1920 native width,
/// not a stale ~1408px remnant): "File"'s real bbox became
/// `x: 0..32, y: 23..42`, center (16, 32.5), inverted through the SAME
/// `scale_to_native` scale factors (1920/1280 = 1.5x, 1080/800 = 1.35x)
/// this file's own `scale::ADVERTISED_WIDTH` uses. A genuine full-screen
/// maximize is deterministic across window instances (unlike the stale
/// remembered placement it replaces), so THIS calibration generalizes.
const TOP_LEFT_ADVERTISED: [u32; 2] = [11, 24];

/// Live-tuned (Plan 15-06) advertised-space coordinates for the top-right
/// "Minimize" button click -- same round-2 (post-restore-fix, true
/// full-screen) methodology as [`TOP_LEFT_ADVERTISED`]; see its doc
/// comment for the full round-1-vs-round-2 diagnosis. The "Minimize"
/// button's real native bbox, measured against the TRUE full-screen
/// maximize, is `x: 1779..1826, y: 0..22` (now genuinely flush against the
/// real screen-width edge, unlike round 1's stale `x: 1345..1392`), center
/// (1802.5, 11), inverted through the identical scale factors.
const TOP_RIGHT_ADVERTISED: [u32; 2] = [1202, 8];

struct LiveTarget {
    host: String,
    user: String,
    password: String,
    port: u16,
}

fn connection_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".secrets").join("connection.json")
}

fn load_live_target() -> Option<LiveTarget> {
    std::env::var_os(LIVE_ENV)?;

    let path = connection_file();
    if !path.exists() {
        return None;
    }

    let raw = std::fs::read_to_string(&path)
        .expect("RDPILOT_LIVE is set and .secrets/connection.json exists but could not be read");
    let json: serde_json::Value = serde_json::from_str(&raw).expect(".secrets/connection.json is not valid JSON");

    let host =
        json.get("host").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `host`").to_owned();
    let user =
        json.get("user").and_then(|v| v.as_str()).expect(".secrets/connection.json is missing a string `user`").to_owned();
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `password`")
        .to_owned();
    let port: u16 =
        json.get("rdpPort").and_then(serde_json::Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(3389);

    Some(LiveTarget { host, user, password, port })
}

fn record(steps: &mut Vec<(String, bool, String)>, name: &str, passed: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("  [{}] {name}: {detail}", if passed { "PASS" } else { "FAIL" });
    steps.push((name.to_owned(), passed, detail));
}

fn text_of(result: &CallToolResult) -> String {
    result.content.iter().filter_map(ContentBlock::as_text).map(|t| t.text.clone()).collect::<Vec<_>>().join(" ")
}

fn args_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(map) => map,
        other => panic!("expected a JSON object for tool-call arguments, got {other:?}"),
    }
}

/// See `tests/live_proof.rs::call`'s identical doc comment: an `Err` here is
/// a JSON-RPC protocol-level error (how every tool under test surfaces an
/// `McpError` today); `is_error` is also checked defensively.
async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    name: &'static str,
    arguments: serde_json::Value,
) -> Result<CallToolResult, String> {
    client
        .call_tool(CallToolRequestParams::new(name).with_arguments(args_object(arguments)))
        .await
        .map_err(|e| format!("{name} call failed: {e}"))
        .and_then(|result| {
            if result.is_error == Some(true) {
                Err(format!("{name} returned is_error=true: {}", text_of(&result)))
            } else {
                Ok(result)
            }
        })
}

fn json_of(result: &Result<CallToolResult, String>) -> Option<serde_json::Value> {
    result.as_ref().ok().and_then(|r| serde_json::from_str::<serde_json::Value>(&text_of(r)).ok())
}

/// Poll `rdpilot_window_list` (up to 20x, 500ms apart) for a window whose
/// `class_name` is `"7-Zip::FM"`, returning its `hwnd` -- mirrors
/// `crates/rdpilot-cli/tests/live_cli_verbs.rs::connect_and_launch_7zip`'s
/// identical polling shape.
async fn poll_for_7zip_fm_hwnd(client: &rmcp::service::RunningService<rmcp::RoleClient, ()>, session: &str) -> Option<u64> {
    for attempt in 0..20 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let result = call(client, "rdpilot_window_list", serde_json::json!({ "session": session })).await;
        if let Some(windows) = json_of(&result).and_then(|v| v.get("windows").and_then(|w| w.as_array().cloned())) {
            if let Some(w) = windows.iter().find(|w| w.get("class_name").and_then(|c| c.as_str()) == Some("7-Zip::FM")) {
                return w.get("hwnd").and_then(serde_json::Value::as_u64);
            }
        }
    }
    None
}

/// Poll `rdpilot_window_list` (up to 20x, 500ms apart) for `hwnd`'s `state`
/// field to equal `expected_state` (e.g. `"maximized"`/`"minimized"`).
async fn poll_for_window_state(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    session: &str,
    hwnd: u64,
    expected_state: &str,
) -> bool {
    for attempt in 0..20 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let result = call(client, "rdpilot_window_list", serde_json::json!({ "session": session })).await;
        if let Some(windows) = json_of(&result).and_then(|v| v.get("windows").and_then(|w| w.as_array().cloned())) {
            if let Some(w) = windows.iter().find(|w| w.get("hwnd").and_then(serde_json::Value::as_u64) == Some(hwnd)) {
                if w.get("state").and_then(|s| s.as_str()) == Some(expected_state) {
                    return true;
                }
            }
        }
    }
    false
}

/// `true` if `rdpilot_uia`'s `subtree` scope (depth 3) of `hwnd` reports an
/// element whose `name` contains `name_substr` (case-insensitive) AND
/// `focused == true`, OR (fallback, mirroring
/// `crates/rdpilot-cli/tests/live_cli_verbs.rs`'s CLI-02 click-landing
/// verification) ANY element reports `focused == true`. Returns
/// `(landed, detail)`.
///
/// **Live-diagnosed (Plan 15-06), same root cause as CLI-02:** `children`
/// scope only reaches depth-1 (Window/ToolBar/Pane/TitleBar/MenuBar) -- the
/// Phase 9 D-9.1 finding (STATE.md) that 7-Zip FM's actual menu
/// items/toolbar buttons live deeper. Opening the "File" menu genuinely
/// sets `focused: true` on the depth-2 `MenuItem` itself (live-confirmed),
/// which `children` scope can never see. `subtree`/`max_depth: 3` is the
/// same live-tuned depth 09-04 confirmed (30 deeper elements, 130.2ms,
/// well under the 500ms sensor-side budget).
async fn verify_focus_via_uia(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    session: &str,
    hwnd: u64,
    name_substr: &str,
) -> (bool, String) {
    let uia_args = serde_json::json!({ "session": session, "hwnd": hwnd, "scope": { "scope": "subtree", "max_depth": 3 } });
    let result = call(client, "rdpilot_uia", uia_args).await;
    let Some(elements) = json_of(&result).and_then(|v| v.get("elements").and_then(|e| e.as_array().cloned())) else {
        return (false, match &result {
            Ok(r) => format!("rdpilot_uia succeeded but did not parse as an elements array: {}", text_of(r)),
            Err(e) => e.clone(),
        });
    };
    let named_and_focused = elements.iter().any(|e| {
        e.get("focused").and_then(serde_json::Value::as_bool) == Some(true)
            && e.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.to_lowercase().contains(&name_substr.to_lowercase()))
    });
    let any_focused = elements.iter().any(|e| e.get("focused").and_then(serde_json::Value::as_bool) == Some(true));
    if named_and_focused {
        (true, format!("an element named ~{name_substr:?} is focused (of {} children)", elements.len()))
    } else if any_focused {
        (true, format!("no element named ~{name_substr:?} is focused, but SOME element is (of {} children)", elements.len()))
    } else {
        (false, format!("no element is focused after the click (of {} children)", elements.len()))
    }
}

#[tokio::test]
#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]
async fn computer_click_lands_near_screen_edges_and_corners_mcp04_live() {
    let Some(target) = load_live_target() else {
        eprintln!("skipping MCP-04 live half: RDPILOT_LIVE unset or .secrets/connection.json absent");
        return;
    };

    // The live-tuned corner constants must stay inside the fixed advertised
    // space they were computed against (D-14.2 LOCKED, `scale::
    // ADVERTISED_WIDTH`/`ADVERTISED_HEIGHT`) -- keeps this file's own
    // single-source-of-truth import genuinely load-bearing rather than a
    // stale doc-comment-only reference.
    assert!(TOP_LEFT_ADVERTISED[0] < ADVERTISED_WIDTH && TOP_LEFT_ADVERTISED[1] < scale::ADVERTISED_HEIGHT);
    assert!(TOP_RIGHT_ADVERTISED[0] < ADVERTISED_WIDTH && TOP_RIGHT_ADVERTISED[1] < scale::ADVERTISED_HEIGHT);

    let mut steps: Vec<(String, bool, String)> = Vec::new();
    let session = "mcp04-live";

    println!("=== rdpilot MCP-04 live half (real click landing near screen edges/corners) ===");

    let mcp_bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot-mcp"));
    let transport =
        TokioChildProcess::new(Command::new(&mcp_bin)).expect("spawn the compiled rdpilot-mcp binary as a subprocess");
    let client = ()
        .serve(transport)
        .await
        .expect("complete the MCP initialize handshake against the real rdpilot-mcp binary");

    // --- connect ---
    let connect_args = serde_json::json!({
        "name": session,
        "host": target.host,
        "port": target.port,
        "username": target.user,
        "password": target.password,
        "accept_invalid_certs": true,
    });
    let connect_result = call(&client, "rdpilot_connect", connect_args).await;
    let connect_ok = connect_result.is_ok();
    record(
        &mut steps,
        "rdpilot_connect",
        connect_ok,
        match &connect_result {
            Ok(_) => format!("connected as session {session:?}"),
            Err(e) => e.clone(),
        },
    );
    assert!(connect_ok, "MCP-04 live half cannot continue past a failed rdpilot_connect");

    // --- launch + foreground 7-Zip File Manager (continuity with the
    // Phase 9 proof / PROOF-02/03) ---
    let launch_args = serde_json::json!({
        "session": session,
        "exe": "C:\\Program Files\\7-Zip\\7zFM.exe",
    });
    let launch_result = call(&client, "rdpilot_launch", launch_args).await;
    record(
        &mut steps,
        "rdpilot_launch 7zFM.exe",
        launch_result.is_ok(),
        match &launch_result {
            Ok(_) => "launched 7-Zip File Manager".to_owned(),
            Err(e) => e.clone(),
        },
    );

    let hwnd = poll_for_7zip_fm_hwnd(&client, session).await;
    record(
        &mut steps,
        "rdpilot_window_list (7-Zip::FM)",
        hwnd.is_some(),
        match hwnd {
            Some(h) => format!("found 7-Zip::FM at hwnd={h}"),
            None => "never observed a 7-Zip::FM window after polling".to_owned(),
        },
    );
    let Some(hwnd) = hwnd else {
        let all_passed = steps.iter().all(|(_, passed, _)| *passed);
        println!("MCP-04 LIVE HALF: {}", if all_passed { "PASS" } else { "FAIL" });
        let _ = client.cancel().await;
        panic!("MCP-04 live half cannot continue without a 7-Zip::FM window -- see the PASS/FAIL trace above");
    };

    let foreground_result = call(&client, "rdpilot_foreground", serde_json::json!({ "session": session, "hwnd": hwnd })).await;
    record(
        &mut steps,
        "rdpilot_foreground",
        foreground_result.is_ok(),
        match &foreground_result {
            Ok(_) => format!("foregrounded hwnd={hwnd}"),
            Err(e) => e.clone(),
        },
    );

    // --- maximize the window (win+up) so its corners coincide with the
    // SCREEN's corners -- no coordinate needed for this action.
    //
    // Live-diagnosed (Plan 15-06): 7-Zip FM persists its OWN remembered
    // window placement (including a "maximized" `GetWindowPlacement` flag)
    // across launches, independent of the CURRENT session's real desktop
    // size -- a freshly launched window can already report
    // `state: "maximized"` while its real, UIA-measurable chrome bounds
    // are stale/narrower than the actual negotiated desktop (confirmed via
    // a direct `Request::DesktopSize` probe showing 1920x1080 while the
    // window's own title bar bbox spanned only ~1408px). Sending "win+up"
    // to an ALREADY-maximized window is a genuine Windows no-op -- it
    // never recomputes fresh bounds. An unconditional "win+down" (restore)
    // immediately before "win+up" forces a real restore -> maximize cycle,
    // so Windows recomputes the maximize bounds fresh against the CURRENT
    // work area every time, regardless of what stale state the window
    // launched with. Tolerant of the restore being a no-op when the window
    // was genuinely already restored (state normal) -- only the outcome of
    // the FOLLOWING maximize is asserted. ---
    // Live-diagnosed: on THIS window's stale remembered "maximized"
    // placement, a single "win+down" does not land on "normal" -- it goes
    // straight to "minimized" (confirmed via a direct manual probe: state
    // transitioned maximized -> minimized on one press). Either outcome
    // ("normal" or "minimized") is equally sufficient to un-stick the
    // stale maximize state; "win+up" reliably RE-maximizes fresh from
    // EITHER. A settle (not a state poll targeting one specific
    // intermediate state) is used here since which of the two states is
    // reached is not the property under test.
    let restore_args = serde_json::json!({ "session": session, "action": "key", "text": "win+down" });
    let _ = call(&client, "computer", restore_args).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Live-diagnosed (the actual root cause of the maximize step's
    // flakiness): "win+down" genuinely MINIMIZES this window (see above --
    // it does not land on a plain "normal" restored state), and a
    // minimized window necessarily LOSES OS foreground focus (Windows
    // hands focus to whatever's next, typically the desktop). "win+up" is
    // a GLOBAL hotkey that acts on whatever currently has foreground focus
    // -- without re-foregrounding the target window first, it was being
    // dispatched to the wrong context entirely, explaining why the
    // maximize poll kept failing even though the restore itself succeeded.
    // A manual reproduction confirmed re-foregrounding between the two key
    // presses is what makes the subsequent maximize land on the intended
    // window.
    let _ = call(&client, "rdpilot_foreground", serde_json::json!({ "session": session, "hwnd": hwnd })).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let maximize_args = serde_json::json!({ "session": session, "action": "key", "text": "win+up" });
    let maximize_result = call(&client, "computer", maximize_args).await;
    let maximized = maximize_result.is_ok() && poll_for_window_state(&client, session, hwnd, "maximized").await;
    // The `state` flag (GetWindowPlacement) can flip to "maximized" slightly
    // ahead of the actual visual re-layout completing over RDP -- an extra
    // settle here (mirrors the crate's own established
    // deploy_and_launch/navigate "settle before interacting" precedent,
    // STATE.md) gives the UIA tree (queried by the corner clicks right
    // after) a moment to catch up with the now-genuinely-maximized bounds.
    tokio::time::sleep(Duration::from_millis(500)).await;
    record(
        &mut steps,
        "computer key win+up (maximize)",
        maximized,
        if maximized {
            "window state is now \"maximized\"".to_owned()
        } else {
            match &maximize_result {
                Ok(_) => "key press succeeded but the window never reported state=\"maximized\"".to_owned(),
                Err(e) => e.clone(),
            }
        },
    );

    // --- a computer screenshot, in the fixed advertised space (MCP-02) ---
    let screenshot_args = serde_json::json!({ "session": session, "action": "screenshot" });
    let screenshot_result = call(&client, "computer", screenshot_args).await;
    let screenshot_ok = screenshot_result
        .as_ref()
        .map(|r| r.content.iter().any(|c| c.as_image().is_some()))
        .unwrap_or(false);
    record(
        &mut steps,
        "computer screenshot (advertised 1280x800 space)",
        screenshot_ok,
        match &screenshot_result {
            Ok(r) if screenshot_ok => format!("received an image content block ({} content block(s) total)", r.content.len()),
            Ok(_) => "computer screenshot succeeded but returned no image content block".to_owned(),
            Err(e) => e.clone(),
        },
    );

    // --- corner 1: top-left -- the menu bar's "File" menu (Plan 15-06
    // live-tuned coordinates, see TOP_LEFT_ADVERTISED's doc comment) ---
    let top_left = TOP_LEFT_ADVERTISED;
    let top_left_click_args =
        serde_json::json!({ "session": session, "action": "left_click", "coordinate": top_left });
    let top_left_click_result = call(&client, "computer", top_left_click_args).await;
    let (top_left_landed, top_left_detail) = if top_left_click_result.is_err() {
        (false, top_left_click_result.err().unwrap_or_default())
    } else {
        verify_focus_via_uia(&client, session, hwnd, "File").await
    };
    record(
        &mut steps,
        &format!("computer left_click near top-left corner {top_left:?} (advertised space) -> \"File\" menu"),
        top_left_landed,
        top_left_detail,
    );

    // --- corner 2 (last): top-right -- the title bar's "Minimize" button
    // (Plan 15-06 live-tuned coordinates, see TOP_RIGHT_ADVERTISED's doc
    // comment) ---
    let top_right = TOP_RIGHT_ADVERTISED;
    let top_right_click_args =
        serde_json::json!({ "session": session, "action": "left_click", "coordinate": top_right });
    let top_right_click_result = call(&client, "computer", top_right_click_args).await;
    let top_right_landed =
        top_right_click_result.is_ok() && poll_for_window_state(&client, session, hwnd, "minimized").await;
    record(
        &mut steps,
        &format!("computer left_click near top-right corner {top_right:?} (advertised space) -> \"Minimize\" button"),
        top_right_landed,
        if top_right_landed {
            "window state is now \"minimized\"".to_owned()
        } else {
            match &top_right_click_result {
                Ok(_) => "click succeeded but the window never reported state=\"minimized\"".to_owned(),
                Err(e) => e.clone(),
            }
        },
    );

    let disconnect_result = call(&client, "rdpilot_disconnect", serde_json::json!({ "session": session })).await;
    record(
        &mut steps,
        "rdpilot_disconnect",
        disconnect_result.is_ok(),
        match &disconnect_result {
            Ok(_) => format!("disconnected session {session:?}"),
            Err(e) => e.clone(),
        },
    );

    let _ = client.cancel().await;

    let all_passed = steps.iter().all(|(_, passed, _)| *passed);
    println!("MCP-04 LIVE HALF: {}", if all_passed { "PASS" } else { "FAIL" });

    assert!(all_passed, "one or more MCP-04 live-half steps failed -- see the PASS/FAIL trace above");
}
