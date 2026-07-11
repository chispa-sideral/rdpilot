//! `screenshot`/`world-state`/`uia`/`window list`/`process list` — CLI-02's
//! perception verbs. Each is a thin invoke-and-exit `rdpilot-ipc` client:
//! round-trip exactly one `Request`/`WireResponse` frame pair against a
//! required `--session` (D-29), and render the SPECIFIC response variant it
//! expects (table by default, `--json` opt-in). `screenshot`/`world-state`
//! base64-decode the wire's `png_base64` and write raw PNG bytes to
//! `--output` only — binary bytes never hit stdout (D-13.1).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rdpilot_ipc::{Request, WireResponse, WireUiaMode, WireUiaScope, WireWorldStateOptions};

use crate::cli::{ScreenshotArgs, SessionArg, UiaArgs, UiaModeArg, UiaScopeArg, WorldStateArgs};
use crate::connect::round_trip;
use crate::exit_codes::CliError;
use crate::render::{print_json, render_process_table, render_uia_table, render_window_table};

/// `perceive screenshot --session <id> --output <path>` (D-13.1).
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`, a base64 decode
/// failure, or a failure writing `--output`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn screenshot(args: ScreenshotArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::Screenshot { session }).await? {
        WireResponse::Screenshot { png_base64 } => {
            let bytes = decode_png(&png_base64)?;
            write_output(&args.output, &bytes)?;
            if json {
                print_json(&serde_json::json!({ "output": args.output.display().to_string(), "bytes": bytes.len() }))
            } else {
                println!("wrote {} ({} bytes)", args.output.display(), bytes.len());
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to Screenshot: {other:?}"))),
    }
}

/// `perceive world-state --session <id> [--screenshot] [--window-list] \
/// [--uia-mode <mode>] [--hwnd N ...] [--output <path>]`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`, a base64 decode
/// failure, or a failure writing `--output`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn world_state(args: WorldStateArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let uia = match args.uia_mode {
        UiaModeArg::None => WireUiaMode::None,
        UiaModeArg::Foreground => WireUiaMode::Foreground,
        UiaModeArg::All => WireUiaMode::AllTopLevel,
        UiaModeArg::Hwnd => WireUiaMode::Hwnd(args.hwnd.clone()),
    };
    let options = WireWorldStateOptions { screenshot: args.screenshot, window_list: args.window_list, uia };
    match round_trip(Request::WorldState { session, options }).await? {
        WireResponse::WorldState { timestamp, capture_span_ms, screenshot, window_list, uia } => {
            let mut written_bytes: Option<usize> = None;
            if let (Some(png_base64), Some(output)) = (&screenshot, &args.output) {
                let bytes = decode_png(png_base64)?;
                write_output(output, &bytes)?;
                written_bytes = Some(bytes.len());
            }
            if json {
                print_json(&serde_json::json!({
                    "timestamp": timestamp,
                    "capture_span_ms": capture_span_ms,
                    "screenshot_captured": screenshot.is_some(),
                    "screenshot_written_bytes": written_bytes,
                    "window_list": window_list,
                    "uia": uia,
                }))
            } else {
                println!("timestamp: {timestamp}");
                println!("capture_span_ms: {capture_span_ms}");
                if let Some(n) = written_bytes {
                    // `args.output` is `Some` whenever `written_bytes` is `Some` (see the `if let` above).
                    if let Some(output) = &args.output {
                        println!("wrote {} ({n} bytes)", output.display());
                    }
                } else if screenshot.is_some() {
                    println!("screenshot captured (pass --output to write it)");
                }
                if let Some(windows) = window_list {
                    print!("{}", render_window_table(&windows));
                }
                if let Some(uia_groups) = uia {
                    for (hwnd, elements) in uia_groups {
                        println!("uia for hwnd {hwnd}:");
                        print!("{}", render_uia_table(&elements));
                    }
                }
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to WorldState: {other:?}"))),
    }
}

/// `perceive uia --session <id> --hwnd <n> --scope <children|subtree> [--max-depth <n>]`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn uia(args: UiaArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let scope = match args.scope {
        UiaScopeArg::Children => WireUiaScope::Children,
        UiaScopeArg::Subtree => WireUiaScope::Subtree { max_depth: args.max_depth.unwrap_or(1) },
    };
    match round_trip(Request::Uia { session, hwnd: args.hwnd, scope }).await? {
        WireResponse::Uia { elements } => {
            if json {
                print_json(&elements)
            } else {
                print!("{}", render_uia_table(&elements));
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to Uia: {other:?}"))),
    }
}

/// `perceive window list --session <id>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn window_list(args: SessionArg, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::WindowList { session }).await? {
        WireResponse::WindowList { windows } => {
            if json {
                print_json(&windows)
            } else {
                print!("{}", render_window_table(&windows));
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to WindowList: {other:?}"))),
    }
}

/// `perceive process list --session <id>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn process_list(args: SessionArg, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::ProcessList { session }).await? {
        WireResponse::ProcessList { processes } => {
            if json {
                print_json(&processes)
            } else {
                print!("{}", render_process_table(&processes));
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to ProcessList: {other:?}"))),
    }
}

/// Base64-decode a wire `png_base64` field.
fn decode_png(png_base64: &str) -> Result<Vec<u8>, CliError> {
    STANDARD.decode(png_base64).map_err(|e| CliError::Internal(format!("invalid base64 screenshot data: {e}")))
}

/// Write decoded PNG bytes to `output` — never to stdout (D-13.1).
fn write_output(output: &std::path::Path, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(output, bytes)
        .map_err(|e| CliError::Internal(format!("failed to write {}: {e}", output.display())))
}
