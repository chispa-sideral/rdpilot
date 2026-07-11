//! `click`/`scroll`/`drag`/`type`/`key`/`launch`/`foreground` — CLI-02's
//! input + launch verbs. Each is a thin invoke-and-exit `rdpilot-ipc`
//! client: build the appropriate `WireMouseAction`/`WireKeyAction`, round-trip
//! exactly one `Request`/`WireResponse` frame pair against a required
//! `--session` (D-29), and render the SPECIFIC response variant it expects
//! (table by default, `--json` opt-in).

use rdpilot_ipc::{Request, WireButton, WireKey, WireKeyAction, WireMouseAction, WireResponse};

use crate::cli::{ButtonArg, ClickArgs, DragArgs, ForegroundArgs, KeyArgs, LaunchArgs, ScrollArgs, TypeArgs};
use crate::connect::round_trip;
use crate::exit_codes::CliError;
use crate::render::print_json;

/// `input click --session <id> --x <n> --y <n> [--button <button>] [--double]`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn click(args: ClickArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let button = wire_button(args.button);
    let action = if args.double {
        WireMouseAction::DoubleClick { x: args.x, y: args.y, button }
    } else {
        WireMouseAction::Click { x: args.x, y: args.y, button }
    };
    expect_ack(round_trip(Request::Mouse { session, action }).await?, json)
}

/// `input scroll --session <id> --x <n> --y <n> --dy <n>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn scroll(args: ScrollArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let action = WireMouseAction::Scroll { x: args.x, y: args.y, dy: args.dy };
    expect_ack(round_trip(Request::Mouse { session, action }).await?, json)
}

/// `input drag --session <id> --from-x <n> --from-y <n> --to-x <n> --to-y <n> [--button <button>]`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn drag(args: DragArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let button = wire_button(args.button);
    let action =
        WireMouseAction::Drag { from_x: args.from_x, from_y: args.from_y, to_x: args.to_x, to_y: args.to_y, button };
    expect_ack(round_trip(Request::Mouse { session, action }).await?, json)
}

/// `input type --session <id> --text <string>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn type_text(args: TypeArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let action = WireKeyAction::Type(args.text);
    expect_ack(round_trip(Request::Key { session, action }).await?, json)
}

/// `input key --session <id> --combo <comma-separated key names>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session` or an unrecognized key
/// name in `--combo` (legible error, never a panic — T-13-17); the daemon's
/// own error otherwise; or a transport/auto-start failure.
pub async fn key(args: KeyArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let keys: Vec<WireKey> = args.combo.split(',').map(parse_key_name).collect::<Result<_, _>>()?;
    let action = WireKeyAction::Combo(keys);
    expect_ack(round_trip(Request::Key { session, action }).await?, json)
}

/// `input launch --session <id> --exe <path> [--args <string>] [--cwd <path>]`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn launch(args: LaunchArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::LaunchProcess { session, exe: args.exe, args: args.args, cwd: args.cwd }).await? {
        WireResponse::Pid { pid } => {
            if json {
                print_json(&serde_json::json!({ "pid": pid }))
            } else {
                println!("{pid}");
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response to LaunchProcess: {other:?}"))),
    }
}

/// `input foreground --session <id> --hwnd <n>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn foreground(args: ForegroundArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    expect_ack(round_trip(Request::SetForeground { session, hwnd: args.hwnd }).await?, json)
}

/// The CLI spelling of `rdpilot_ipc::WireButton` -> the wire type itself.
fn wire_button(b: ButtonArg) -> WireButton {
    match b {
        ButtonArg::Left => WireButton::Left,
        ButtonArg::Right => WireButton::Right,
        ButtonArg::Middle => WireButton::Middle,
    }
}

/// Parse one `--combo` key name (case-insensitive, matching
/// `rdpilot_ipc::WireKey`'s variant spellings) into a [`WireKey`].
///
/// Delegates to `rdpilot_ipc::parse_wire_key` — the ONE canonical key-name
/// table shared with the (future) MCP surface (research "Don't Hand-Roll");
/// this CLI no longer holds its own copy.
///
/// # Errors
///
/// [`CliError::Internal`] with a legible message naming the offending token
/// if `name` does not match any known key (T-13-17: no silent drop, no
/// panic).
fn parse_key_name(name: &str) -> Result<WireKey, CliError> {
    rdpilot_ipc::parse_wire_key(name).map_err(CliError::Internal)
}

/// Interpret a round-tripped [`WireResponse`] that every input/launch verb
/// (except `launch` itself) expects: [`WireResponse::Ack`] on success.
fn expect_ack(response: WireResponse, json: bool) -> Result<(), CliError> {
    match response {
        WireResponse::Ack => {
            if json {
                print_json(&serde_json::json!({ "ok": true }))
            } else {
                println!("ok");
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!("unexpected response: {other:?}"))),
    }
}
