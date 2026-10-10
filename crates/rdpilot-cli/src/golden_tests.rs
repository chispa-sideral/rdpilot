//! Golden tests for the CLI's key-name parser, `--button` parser and the help
//! text of the two verbs that take a button.
//!
//! They name only items that exist before and after the shared input
//! vocabulary moves into its own crate, so they stay byte-identical across
//! that move.
#![allow(clippy::expect_used)]

use clap::error::ErrorKind;
use clap::Parser;

use crate::cli::{Cli, Command, InputCmd};
use crate::exit_codes::CliError;
use crate::verbs::input::parse_key_name;

/// Every canonical key name with its wire spelling, in declaration order.
const CANONICAL_KEYS: [(&str, &str); 67] = [
    ("ctrl", "\"Ctrl\""),
    ("alt", "\"Alt\""),
    ("shift", "\"Shift\""),
    ("a", "\"A\""),
    ("b", "\"B\""),
    ("c", "\"C\""),
    ("d", "\"D\""),
    ("e", "\"E\""),
    ("f", "\"F\""),
    ("g", "\"G\""),
    ("h", "\"H\""),
    ("i", "\"I\""),
    ("j", "\"J\""),
    ("k", "\"K\""),
    ("l", "\"L\""),
    ("m", "\"M\""),
    ("n", "\"N\""),
    ("o", "\"O\""),
    ("p", "\"P\""),
    ("q", "\"Q\""),
    ("r", "\"R\""),
    ("s", "\"S\""),
    ("t", "\"T\""),
    ("u", "\"U\""),
    ("v", "\"V\""),
    ("w", "\"W\""),
    ("x", "\"X\""),
    ("y", "\"Y\""),
    ("z", "\"Z\""),
    ("digit0", "\"Digit0\""),
    ("digit1", "\"Digit1\""),
    ("digit2", "\"Digit2\""),
    ("digit3", "\"Digit3\""),
    ("digit4", "\"Digit4\""),
    ("digit5", "\"Digit5\""),
    ("digit6", "\"Digit6\""),
    ("digit7", "\"Digit7\""),
    ("digit8", "\"Digit8\""),
    ("digit9", "\"Digit9\""),
    ("f1", "\"F1\""),
    ("f2", "\"F2\""),
    ("f3", "\"F3\""),
    ("f4", "\"F4\""),
    ("f5", "\"F5\""),
    ("f6", "\"F6\""),
    ("f7", "\"F7\""),
    ("f8", "\"F8\""),
    ("f9", "\"F9\""),
    ("f10", "\"F10\""),
    ("f11", "\"F11\""),
    ("f12", "\"F12\""),
    ("enter", "\"Enter\""),
    ("esc", "\"Esc\""),
    ("tab", "\"Tab\""),
    ("space", "\"Space\""),
    ("backspace", "\"Backspace\""),
    ("delete", "\"Delete\""),
    ("up", "\"Up\""),
    ("down", "\"Down\""),
    ("left", "\"Left\""),
    ("right", "\"Right\""),
    ("home", "\"Home\""),
    ("end", "\"End\""),
    ("pageup", "\"PageUp\""),
    ("pagedown", "\"PageDown\""),
    ("insert", "\"Insert\""),
    ("win", "\"Win\""),
];

/// Aliases the parser accepts besides the canonical names.
const ALIAS_KEYS: [(&str, &str); 17] = [
    ("0", "\"Digit0\""),
    ("1", "\"Digit1\""),
    ("2", "\"Digit2\""),
    ("3", "\"Digit3\""),
    ("4", "\"Digit4\""),
    ("5", "\"Digit5\""),
    ("6", "\"Digit6\""),
    ("7", "\"Digit7\""),
    ("8", "\"Digit8\""),
    ("9", "\"Digit9\""),
    ("escape", "\"Esc\""),
    ("del", "\"Delete\""),
    ("page-up", "\"PageUp\""),
    ("page-down", "\"PageDown\""),
    ("ins", "\"Insert\""),
    ("windows", "\"Win\""),
    ("super", "\"Win\""),
];

fn wire(name: &str) -> String {
    let key = parse_key_name(name).expect("known key name");
    serde_json::to_string(&key).expect("a key serializes")
}

#[test]
fn every_canonical_key_name_parses_to_its_wire_spelling() {
    for (name, spelling) in CANONICAL_KEYS {
        assert_eq!(wire(name), spelling, "{name}");
    }
}

#[test]
fn every_alias_parses_to_its_wire_spelling() {
    for (alias, spelling) in ALIAS_KEYS {
        assert_eq!(wire(alias), spelling, "{alias}");
    }
}

#[test]
fn key_names_are_trimmed_and_case_insensitive() {
    assert_eq!(wire("  CTRL "), "\"Ctrl\"");
    assert_eq!(wire("PageUp"), "\"PageUp\"");
    assert_eq!(wire("F12"), "\"F12\"");
}

#[test]
fn an_unknown_key_name_names_the_token() {
    match parse_key_name("bogus") {
        Err(CliError::Internal(msg)) => {
            assert_eq!(msg, "unknown key name 'bogus' (original token: 'bogus')");
        }
        other => panic!("expected CliError::Internal, got {other:?}"),
    }
    match parse_key_name("  Bogus ") {
        Err(CliError::Internal(msg)) => {
            assert_eq!(msg, "unknown key name 'bogus' (original token: 'Bogus')");
        }
        other => panic!("expected CliError::Internal, got {other:?}"),
    }
}

fn click_button(extra: &[&str]) -> Result<String, clap::Error> {
    let mut argv = vec![
        "rdpilot",
        "input",
        "click",
        "--session",
        "s",
        "--x",
        "1",
        "--y",
        "1",
    ];
    argv.extend_from_slice(extra);
    match Cli::try_parse_from(argv)?.command {
        Command::Input(InputCmd::Click(args)) => Ok(format!("{:?}", args.button)),
        other => panic!("expected input click, got {other:?}"),
    }
}

fn drag_button(extra: &[&str]) -> Result<String, clap::Error> {
    let mut argv = vec![
        "rdpilot",
        "input",
        "drag",
        "--session",
        "s",
        "--from-x",
        "1",
        "--from-y",
        "2",
        "--to-x",
        "3",
        "--to-y",
        "4",
    ];
    argv.extend_from_slice(extra);
    match Cli::try_parse_from(argv)?.command {
        Command::Input(InputCmd::Drag(args)) => Ok(format!("{:?}", args.button)),
        other => panic!("expected input drag, got {other:?}"),
    }
}

#[test]
fn the_button_defaults_to_left_and_accepts_the_three_lowercase_names() {
    assert_eq!(click_button(&[]).expect("parses"), "Left");
    assert_eq!(drag_button(&[]).expect("parses"), "Left");
    for (name, debug) in [("left", "Left"), ("right", "Right"), ("middle", "Middle")] {
        assert_eq!(click_button(&["--button", name]).expect("parses"), debug);
        assert_eq!(drag_button(&["--button", name]).expect("parses"), debug);
    }
}

#[test]
fn the_button_is_case_sensitive_and_rejects_other_names() {
    for name in ["Left", "x1", "X1", "bogus"] {
        let err = click_button(&["--button", name]).expect_err("rejected");
        assert_eq!(err.kind(), ErrorKind::InvalidValue, "{name}");
        let err = drag_button(&["--button", name]).expect_err("rejected");
        assert_eq!(err.kind(), ErrorKind::InvalidValue, "{name}");
    }
}

#[test]
fn an_invalid_button_error_lists_the_possible_values() {
    let err = click_button(&["--button", "Left"]).expect_err("rejected");
    assert_eq!(
        err.to_string(),
        r#"error: invalid value 'Left' for '--button <BUTTON>'
  [possible values: left, right, middle]

  tip: a similar value exists: 'left'

For more information, try '--help'.
"#
    );
}

fn help_text(argv: &[&str]) -> String {
    let err = Cli::try_parse_from(argv).expect_err("help is not a successful parse");
    assert_eq!(err.kind(), ErrorKind::DisplayHelp);
    err.to_string()
}

#[test]
fn input_click_long_help_is_unchanged() {
    assert_eq!(
        help_text(&["rdpilot", "input", "click", "--help"]),
        r#"`input click --session <id> --x <n> --y <n> [--button <button>] [--double]`

Usage: rdpilot input click [OPTIONS] --session <SESSION> --x <X> --y <Y>

Options:
      --session <SESSION>
          The session to target

      --x <X>
          Target x, in physical virtual-desktop pixels

      --y <Y>
          Target y, in physical virtual-desktop pixels

      --button <BUTTON>
          The button to click (default: left)

          Possible values:
          - left:   The left (primary) mouse button
          - right:  The right (secondary/context-menu) mouse button
          - middle: The middle (wheel) mouse button
          
          [default: left]

      --double
          Double-click instead of a single click

      --json
          Emit machine-readable JSON instead of a human-readable table

  -h, --help
          Print help (see a summary with '-h')
"#
    );
}

#[test]
fn input_click_short_help_is_unchanged() {
    assert_eq!(
        help_text(&["rdpilot", "input", "click", "-h"]),
        r#"`input click --session <id> --x <n> --y <n> [--button <button>] [--double]`

Usage: rdpilot input click [OPTIONS] --session <SESSION> --x <X> --y <Y>

Options:
      --session <SESSION>  The session to target
      --x <X>              Target x, in physical virtual-desktop pixels
      --y <Y>              Target y, in physical virtual-desktop pixels
      --button <BUTTON>    The button to click (default: left) [default: left] [possible values: left, right, middle]
      --double             Double-click instead of a single click
      --json               Emit machine-readable JSON instead of a human-readable table
  -h, --help               Print help (see more with '--help')
"#
    );
}

#[test]
fn input_drag_long_help_is_unchanged() {
    assert_eq!(
        help_text(&["rdpilot", "input", "drag", "--help"]),
        r#"`input drag --session <id> --from-x <n> --from-y <n> --to-x <n> --to-y <n> [--button <button>]`

Usage: rdpilot input drag [OPTIONS] --session <SESSION> --from-x <FROM_X> --from-y <FROM_Y> --to-x <TO_X> --to-y <TO_Y>

Options:
      --session <SESSION>
          The session to target

      --from-x <FROM_X>
          Origin x, in physical virtual-desktop pixels

      --from-y <FROM_Y>
          Origin y, in physical virtual-desktop pixels

      --to-x <TO_X>
          Destination x, in physical virtual-desktop pixels

      --to-y <TO_Y>
          Destination y, in physical virtual-desktop pixels

      --button <BUTTON>
          The button to drag with (default: left)

          Possible values:
          - left:   The left (primary) mouse button
          - right:  The right (secondary/context-menu) mouse button
          - middle: The middle (wheel) mouse button
          
          [default: left]

      --json
          Emit machine-readable JSON instead of a human-readable table

  -h, --help
          Print help (see a summary with '-h')
"#
    );
}

#[test]
fn input_drag_short_help_is_unchanged() {
    assert_eq!(
        help_text(&["rdpilot", "input", "drag", "-h"]),
        r#"`input drag --session <id> --from-x <n> --from-y <n> --to-x <n> --to-y <n> [--button <button>]`

Usage: rdpilot input drag [OPTIONS] --session <SESSION> --from-x <FROM_X> --from-y <FROM_Y> --to-x <TO_X> --to-y <TO_Y>

Options:
      --session <SESSION>  The session to target
      --from-x <FROM_X>    Origin x, in physical virtual-desktop pixels
      --from-y <FROM_Y>    Origin y, in physical virtual-desktop pixels
      --to-x <TO_X>        Destination x, in physical virtual-desktop pixels
      --to-y <TO_Y>        Destination y, in physical virtual-desktop pixels
      --button <BUTTON>    The button to drag with (default: left) [default: left] [possible values: left, right, middle]
      --json               Emit machine-readable JSON instead of a human-readable table
  -h, --help               Print help (see more with '--help')
"#
    );
}
