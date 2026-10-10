//! Golden tests for the JSON spellings of input, recording-trigger and error
//! code values on the IPC wire.
//!
//! Each case parses an exact JSON string as a [`Request`], serializes it again
//! and expects the same string, so serde is pinned in both directions with the
//! field order included. The tests name only items that exist before and after
//! the shared input vocabulary moves into its own crate, so they stay
//! byte-identical across that move.
#![allow(clippy::expect_used)]

use crate::{Request, WireErrorCode};

fn round_trip(json: &str) {
    let request: Request = serde_json::from_str(json).expect("parses");
    let again = serde_json::to_string(&request).expect("serializes");
    assert_eq!(again, json);
}

fn rejection(json: &str) -> String {
    serde_json::from_str::<Request>(json)
        .expect_err("rejected")
        .to_string()
}

fn mouse(action: &str) -> String {
    format!(r#"{{"op":"Mouse","session":"web","action":{action}}}"#)
}

fn connect(record: &str) -> String {
    format!(
        r#"{{"op":"Connect","name":null,"host":"h","port":null,"username":"u","password":"p","domain":null,"accept_invalid_certs":false,"cua_enabled":false,"cua_version":"latest-dev","cua_auto_download":true,"connect_ack":false,"record":{record}}}"#
    )
}

#[test]
fn key_combo_with_every_key_spelling_round_trips() {
    round_trip(
        r#"{"op":"Key","session":"web","action":{"Combo":["Ctrl","Alt","Shift","A","B","C","D","E","F","G","H","I","J","K","L","M","N","O","P","Q","R","S","T","U","V","W","X","Y","Z","Digit0","Digit1","Digit2","Digit3","Digit4","Digit5","Digit6","Digit7","Digit8","Digit9","F1","F2","F3","F4","F5","F6","F7","F8","F9","F10","F11","F12","Enter","Esc","Tab","Space","Backspace","Delete","Up","Down","Left","Right","Home","End","PageUp","PageDown","Insert","Win"]}}"#,
    );
}

#[test]
fn key_combo_keeps_the_order_of_the_keys() {
    round_trip(
        r#"{"op":"Key","session":"web","action":{"Combo":["Win","Ctrl","Digit0","PageDown"]}}"#,
    );
    round_trip(r#"{"op":"Key","session":"web","action":{"Combo":[]}}"#);
}

#[test]
fn type_text_round_trips() {
    round_trip(r#"{"op":"Key","session":"web","action":{"Type":"héllo \"wörld\""}}"#);
}

#[test]
fn mouse_actions_round_trip() {
    round_trip(&mouse(r#"{"Move":{"x":1,"y":2}}"#));
    for button in ["Left", "Right", "Middle"] {
        round_trip(&mouse(&format!(
            r#"{{"Click":{{"x":10,"y":20,"button":"{button}"}}}}"#
        )));
        round_trip(&mouse(&format!(
            r#"{{"DoubleClick":{{"x":10,"y":20,"button":"{button}"}}}}"#
        )));
        round_trip(&mouse(&format!(
            r#"{{"Drag":{{"from_x":1,"from_y":2,"to_x":3,"to_y":4,"button":"{button}"}}}}"#
        )));
    }
    round_trip(&mouse(r#"{"Scroll":{"x":5,"y":6,"dy":-120}}"#));
    round_trip(&mouse(r#"{"Scroll":{"x":5,"y":6,"dy":240}}"#));
    round_trip(&mouse(r#"{"Move":{"x":65535,"y":65535}}"#));
}

#[test]
fn connect_record_spellings_round_trip() {
    for record in [
        r#""GlobalConfig""#,
        r#""HostConfig""#,
        r#""ConnectFlag""#,
        "null",
    ] {
        round_trip(&connect(record));
    }
}

#[test]
fn connect_without_record_serializes_it_as_null() {
    let json = connect("null");
    let without = json.replace(r#","record":null"#, "");
    assert!(!without.contains("record"));
    let request: Request = serde_json::from_str(&without).expect("parses");
    assert_eq!(serde_json::to_string(&request).expect("serializes"), json);
}

#[test]
fn connect_record_rejects_every_other_spelling() {
    for record in [
        r#""Cli""#,
        r#""Viewer""#,
        r#""config""#,
        r#""connect_flag""#,
        r#""host""#,
        "1",
    ] {
        assert!(
            serde_json::from_str::<Request>(&connect(record)).is_err(),
            "{record}"
        );
    }
    assert_eq!(
        rejection(&connect(r#""Cli""#)),
        "unknown variant `Cli`, expected one of `GlobalConfig`, `HostConfig`, `ConnectFlag`"
    );
    assert_eq!(
        rejection(&connect(r#""Viewer""#)),
        "unknown variant `Viewer`, expected one of `GlobalConfig`, `HostConfig`, `ConnectFlag`"
    );
}

#[test]
fn key_and_button_spellings_are_case_sensitive() {
    assert!(serde_json::from_str::<Request>(
        r#"{"op":"Key","session":"web","action":{"Combo":["ctrl"]}}"#
    )
    .is_err());
    assert!(
        serde_json::from_str::<Request>(&mouse(r#"{"Click":{"x":1,"y":2,"button":"X1"}}"#))
            .is_err()
    );
    assert!(
        serde_json::from_str::<Request>(&mouse(r#"{"Click":{"x":1,"y":2,"button":"left"}}"#))
            .is_err()
    );
}

#[test]
fn every_wire_error_code_has_its_kebab_case_spelling() {
    let cases = [
        (WireErrorCode::SessionNotFound, "\"session-not-found\""),
        (WireErrorCode::DaemonUnreachable, "\"daemon-unreachable\""),
        (WireErrorCode::TransferFailed, "\"transfer-failed\""),
        (WireErrorCode::PathTraversal, "\"path-traversal\""),
        (WireErrorCode::ChecksumMismatch, "\"checksum-mismatch\""),
        (WireErrorCode::Internal, "\"internal\""),
        (WireErrorCode::DuplicateSession, "\"duplicate-session\""),
        (WireErrorCode::Recording, "\"recording\""),
        (WireErrorCode::BundleUnavailable, "\"bundle-unavailable\""),
        (WireErrorCode::HumanControl, "\"human-control\""),
    ];
    for (code, json) in cases {
        assert_eq!(serde_json::to_string(&code).expect("serializes"), json);
        let back: WireErrorCode = serde_json::from_str(json).expect("parses");
        assert_eq!(back, code);
    }
}
