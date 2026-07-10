//! THROWAWAY D-9.1 fidelity spike — NOT real harness code.
//!
//! Launches the real 7-Zip File Manager (`7zFM.exe`) on a disposable Azure VM
//! over RDP, dumps its live flat `UiaElement[]` to stdout for manual human
//! inspection, and writes the first post-launch screenshot to a PNG so any
//! unexpected first-run UI is caught visually (Open Q#2). This is dump-only:
//! NO `assert!`/`assert_eq!`, NO pass/fail logic beyond mapping a hard
//! connect/launch error to `ExitCode::FAILURE`.
//!
//! This resolves Phase 9's single concentrated unknown — whether 7-Zip's UIA
//! tree is fidelity-adequate under the locked `TreeScope_Children` scope
//! (D-7.4) — before any real harness/assertion code is written (D-9.1,
//! mirroring the proven Phase 7 D-7.5 risk-gate pattern, 07-03-PLAN.md).
//!
//! **This file MUST be deleted once the D-9.1 checkpoint records its
//! findings** (Pitfall 2 — it must NOT evolve into `run_proof_harness`; the
//! real harness is Plan 09-02, built fresh from the recorded findings).
//!
//! Run (with a provisioned live target and `RDPILOT_LIVE` armed):
//!
//! ```text
//! RDPILOT_LIVE=1 cargo run -p rdpilot --example spike_7zip_uia_dump
//! ```
//!
//! It composes the PUBLIC API only (`Session::connect` -> `deploy_and_launch`
//! -> `launch_process` -> `get_window_list` -> `screenshot` ->
//! `get_uia_tree`). No `ironrdp`/`image`/`rustls` type appears here (D-09).
//!
//! # Security
//!
//! Mirrors `examples/screenshot.rs`'s discipline: reads the gitignored
//! `.secrets/connection.json` and hands the credentials to
//! [`rdpilot::ConnectionConfig`]; it NEVER prints the password (the config's
//! `Debug` redacts it). Do not log credentials from here.

use std::path::PathBuf;
use std::process::ExitCode;

use rdpilot::{ConnectionConfig, Session, WindowInfo};

/// Where the first post-launch screenshot PNG is written.
const SCREENSHOT_PATH: &str = "spike_7zip_first_launch.png";

/// D-9.6 seeding target: point 7zFM.exe at a known, always-present folder so
/// its file-listing view is deterministic on a fresh VM profile (Pitfall 3).
/// The classic 7-Zip File Manager CLI invocation is `7zFM.exe <path>` — this
/// spike is exactly where that argument form (RESEARCH A1) is empirically
/// confirmed or refuted.
const SEED_PATH: &str = "C:\\Program Files";

/// Bounded poll attempts (mirrors `launch_notepad_and_find_window`,
/// `tests/live_session.rs:1222-1242`) waiting for the 7-Zip window to
/// register after `launch_process` (fire-and-forget, D-6.2).
const POLL_ATTEMPTS: u32 = 20;
const SETTLE: std::time::Duration = std::time::Duration::from_millis(800);

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: could not start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Compose the public API: load config -> connect -> deploy sensor -> launch
/// 7zFM.exe (seeded per D-9.6) -> poll for its window -> screenshot -> dump
/// the flat UiaElement[] -> close. Dump-only, no assertions (Pitfall 2).
async fn run() -> Result<(), String> {
    let cfg = load_config().map_err(|e| format!("config: {e}"))?;

    let sensor_exe = sensor_exe_path();
    if !sensor_exe.exists() {
        return Err(format!(
            "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
             (`dotnet publish -r win-x64 -p:PublishAot=true --self-contained` on a Windows host \
             with the .NET 8 SDK, or set RDPILOT_SENSOR_EXE)"
        ));
    }
    let cfg = cfg.sensor_binary_path(sensor_exe);

    let session = Session::connect(&cfg)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;

    session
        .deploy_and_launch()
        .await
        .map_err(|e| format!("deploy_and_launch failed: {e}"))?;

    eprintln!("[spike] sensor deployed; launching 7zFM.exe seeded at {SEED_PATH:?}");

    let window = launch_7zip_and_find_window(&session).await?;

    eprintln!(
        "[spike] REAL observed 7-Zip window: class_name={:?} title={:?} rect={:?} hwnd={}",
        window.class_name, window.title, window.rect, window.hwnd
    );

    // First post-launch screenshot — catches any unexpected first-run dialog
    // visually (Open Q#2), zero extra cost.
    let shot = session
        .screenshot()
        .await
        .map_err(|e| format!("screenshot failed: {e}"))?;
    let png = shot.to_png().map_err(|e| format!("encode failed: {e}"))?;
    std::fs::write(SCREENSHOT_PATH, &png).map_err(|e| format!("writing {SCREENSHOT_PATH}: {e}"))?;
    println!(
        "[spike] wrote {SCREENSHOT_PATH} ({}x{}, {} bytes) — inspect visually for first-run UI",
        shot.width,
        shot.height,
        png.len()
    );

    // The dump-only call: get_uia_tree once, print every element legibly.
    let elements = session
        .get_uia_tree(window.hwnd)
        .await
        .map_err(|e| format!("get_uia_tree failed: {e}"))?;

    println!(
        "[spike] 7zFM UiaElement[] dump — {} elements (hwnd={}):",
        elements.len(),
        window.hwnd
    );
    for (i, e) in elements.iter().enumerate() {
        println!(
            "  [{i}] id={:?} role={:?} name={:?} bbox=(x={}, y={}, w={}, h={}) enabled={} visible={} focusable={} focused={} depth={} parent_id={:?}",
            e.id,
            e.role,
            e.name,
            e.bbox.x,
            e.bbox.y,
            e.bbox.w,
            e.bbox.h,
            e.enabled,
            e.visible,
            e.focusable,
            e.focused,
            e.depth,
            e.parent_id
        );
    }

    session
        .close()
        .await
        .map_err(|e| format!("close failed: {e}"))?;

    Ok(())
}

/// Launch 7zFM.exe (seeded at [`SEED_PATH`] per D-9.6) on the already-deployed
/// `session` and poll `get_window_list()` until its window appears, returning
/// the matched [`WindowInfo`]. Adapted from
/// `launch_notepad_and_find_window` (`tests/live_session.rs:1222-1242`).
///
/// Does NOT hard-code a class-name guess (Pitfall 1) — matches loosely on any
/// newly-appeared top-level window whose title looks like 7-Zip, purely to
/// obtain an hwnd for the dump. The real class_name/title are eprintln!'d by
/// the caller once found, so the true match predicate is captured live.
async fn launch_7zip_and_find_window(session: &Session) -> Result<WindowInfo, String> {
    session
        .launch_process("7zFM.exe", Some(SEED_PATH), None)
        .await
        .map_err(|e| format!("launch_process(7zFM.exe) failed: {e}"))?;

    for attempt in 0..POLL_ATTEMPTS {
        tokio::time::sleep(SETTLE).await;
        let windows = session
            .get_window_list()
            .await
            .map_err(|e| format!("get_window_list failed while polling: {e}"))?;
        if let Some(w) = windows.iter().find(|w| {
            let title = w.title.to_ascii_lowercase();
            let class = w.class_name.to_ascii_lowercase();
            title.contains("7-zip")
                || title.contains("7zfm")
                || title.contains(&SEED_PATH.to_ascii_lowercase())
                || class.contains("7zfm")
        }) {
            return Ok(w.clone());
        }
        eprintln!("[poll] 7-Zip window not yet visible (attempt {attempt})");
    }
    Err("7-Zip window did not appear in get_window_list after polling (launch_process fired, \
         but no matching window surfaced)"
        .to_string())
}

/// Load `.secrets/connection.json` into a [`ConnectionConfig`].
///
/// Copied (deliberately duplicated, per `examples/screenshot.rs`'s own
/// precedent — RESEARCH Anti-Patterns, `examples/` cannot `mod common;` into
/// `tests/common`) from `examples/screenshot.rs:96-137`.
fn load_config() -> Result<ConnectionConfig, String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".secrets")
        .join("connection.json");

    if !path.exists() {
        return Err(format!(
            "{} not found — provision the target with `infra/manage-env.ps1 up` first",
            path.display()
        ));
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| format!("reading connection.json: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("connection.json is not valid JSON: {e}"))?;

    let host = json
        .get("host")
        .and_then(|v| v.as_str())
        .ok_or("connection.json is missing a string `host`")?;
    let user = json
        .get("user")
        .and_then(|v| v.as_str())
        .ok_or("connection.json is missing a string `user`")?;
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or("connection.json is missing a string `password`")?;
    let port: u16 = json
        .get("rdpPort")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(3389);

    Ok(ConnectionConfig::new(host, user, password)
        .port(port)
        .accept_invalid_certs(true))
}

/// Name of the env var that overrides the default published sensor exe
/// location — mirrors `tests/common::SENSOR_EXE_ENV`/`sensor_exe_path()`
/// (duplicated here because `examples/` cannot `mod common;` into
/// `tests/common`, same rationale as [`load_config`]).
const SENSOR_EXE_ENV: &str = "RDPILOT_SENSOR_EXE";

fn sensor_exe_path() -> PathBuf {
    if let Some(over) = std::env::var_os(SENSOR_EXE_ENV) {
        return PathBuf::from(over);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sensor")
        .join("bin")
        .join("Release")
        .join("net8.0")
        .join("win-x64")
        .join("publish")
        .join("rdpilot-sensor.exe")
}
