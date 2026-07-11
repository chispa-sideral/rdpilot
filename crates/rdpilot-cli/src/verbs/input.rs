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
/// # Errors
///
/// [`CliError::Internal`] with a legible message naming the offending token
/// if `name` does not match any known key (T-13-17: no silent drop, no
/// panic).
fn parse_key_name(name: &str) -> Result<WireKey, CliError> {
    let trimmed = name.trim();
    Ok(match trimmed.to_lowercase().as_str() {
        "ctrl" => WireKey::Ctrl,
        "alt" => WireKey::Alt,
        "shift" => WireKey::Shift,
        "a" => WireKey::A,
        "b" => WireKey::B,
        "c" => WireKey::C,
        "d" => WireKey::D,
        "e" => WireKey::E,
        "f" => WireKey::F,
        "g" => WireKey::G,
        "h" => WireKey::H,
        "i" => WireKey::I,
        "j" => WireKey::J,
        "k" => WireKey::K,
        "l" => WireKey::L,
        "m" => WireKey::M,
        "n" => WireKey::N,
        "o" => WireKey::O,
        "p" => WireKey::P,
        "q" => WireKey::Q,
        "r" => WireKey::R,
        "s" => WireKey::S,
        "t" => WireKey::T,
        "u" => WireKey::U,
        "v" => WireKey::V,
        "w" => WireKey::W,
        "x" => WireKey::X,
        "y" => WireKey::Y,
        "z" => WireKey::Z,
        "digit0" | "0" => WireKey::Digit0,
        "digit1" | "1" => WireKey::Digit1,
        "digit2" | "2" => WireKey::Digit2,
        "digit3" | "3" => WireKey::Digit3,
        "digit4" | "4" => WireKey::Digit4,
        "digit5" | "5" => WireKey::Digit5,
        "digit6" | "6" => WireKey::Digit6,
        "digit7" | "7" => WireKey::Digit7,
        "digit8" | "8" => WireKey::Digit8,
        "digit9" | "9" => WireKey::Digit9,
        "f1" => WireKey::F1,
        "f2" => WireKey::F2,
        "f3" => WireKey::F3,
        "f4" => WireKey::F4,
        "f5" => WireKey::F5,
        "f6" => WireKey::F6,
        "f7" => WireKey::F7,
        "f8" => WireKey::F8,
        "f9" => WireKey::F9,
        "f10" => WireKey::F10,
        "f11" => WireKey::F11,
        "f12" => WireKey::F12,
        "enter" => WireKey::Enter,
        "esc" | "escape" => WireKey::Esc,
        "tab" => WireKey::Tab,
        "space" => WireKey::Space,
        "backspace" => WireKey::Backspace,
        "delete" | "del" => WireKey::Delete,
        "up" => WireKey::Up,
        "down" => WireKey::Down,
        "left" => WireKey::Left,
        "right" => WireKey::Right,
        "home" => WireKey::Home,
        "end" => WireKey::End,
        "pageup" | "page-up" => WireKey::PageUp,
        "pagedown" | "page-down" => WireKey::PageDown,
        "insert" | "ins" => WireKey::Insert,
        "win" | "windows" | "super" => WireKey::Win,
        other => return Err(CliError::Internal(format!("unknown key name '{other}' in --combo (original token: '{trimmed}')"))),
    })
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
