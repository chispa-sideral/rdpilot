//! `click`/`scroll`/`drag`/`type`/`key`/`launch`/`foreground` — CLI-02's
//! input + launch verbs. Each is a thin invoke-and-exit `rdpilot-ipc`
//! client: build the appropriate `MouseAction`/`KeyAction`, round-trip
//! exactly one `Request`/`WireResponse` frame pair against a required
//! `--session` (D-29), and render the SPECIFIC response variant it expects
//! (table by default, `--json` opt-in).

use rdpilot_ipc::{Request, WireResponse};
use rdpilot_vocab::{Key, KeyAction, MouseAction};

use crate::cli::{ClickArgs, DragArgs, KeyArgs, ScrollArgs, TypeArgs};
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
    let button = args.button;
    let action = if args.double {
        MouseAction::DoubleClick {
            x: args.x,
            y: args.y,
            button,
        }
    } else {
        MouseAction::Click {
            x: args.x,
            y: args.y,
            button,
        }
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
    let action = MouseAction::Scroll {
        x: args.x,
        y: args.y,
        dy: args.dy,
    };
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
    let button = args.button;
    let action = MouseAction::Drag {
        from_x: args.from_x,
        from_y: args.from_y,
        to_x: args.to_x,
        to_y: args.to_y,
        button,
    };
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
    let action = KeyAction::Type(args.text);
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
    let keys: Vec<Key> = args
        .combo
        .split(',')
        .map(parse_key_name)
        .collect::<Result<_, _>>()?;
    let action = KeyAction::Combo(keys);
    expect_ack(round_trip(Request::Key { session, action }).await?, json)
}

/// Parse one `--combo` key name (case-insensitive) into a [`Key`].
///
/// Delegates to `rdpilot_vocab::parse_key_name`, the one key-name table.
///
/// # Errors
///
/// [`CliError::Internal`] with a legible message naming the offending token
/// if `name` does not match any known key (T-13-17: no silent drop, no
/// panic).
pub(crate) fn parse_key_name(name: &str) -> Result<Key, CliError> {
    rdpilot_vocab::parse_key_name(name).map_err(CliError::Internal)
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
        other => Err(CliError::Internal(format!(
            "unexpected response: {other:?}"
        ))),
    }
}
