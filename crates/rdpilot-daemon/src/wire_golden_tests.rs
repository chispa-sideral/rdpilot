//! Golden tests for the viewer input body and the recording-trigger
//! spellings.
//!
//! They name only items that exist before and after the shared input
//! vocabulary moves into its own crate, so they stay byte-identical across
//! that move.
#![allow(clippy::expect_used)]

use serde_json::{json, Value};

use crate::events::RecordingTrigger;
use crate::viewer::InputBody;

fn body(events: Value) -> Value {
    json!({"lease": "abc", "generation": 3, "width": 1280, "height": 720, "events": events})
}

fn parse(events: Value) -> Result<Value, serde_json::Error> {
    let body = serde_json::from_value::<InputBody>(body(events))?;
    Ok(serde_json::to_value(&body.events).expect("events serialize"))
}

#[test]
fn viewer_events_round_trip_with_their_snake_case_spellings() {
    let events = json!([
        {"type": "move", "x": 0, "y": 0},
        {"type": "move", "x": 1279, "y": 719},
        {"type": "wheel", "vertical": true, "units": 120},
        {"type": "wheel", "vertical": true, "units": -120},
        {"type": "wheel", "vertical": false, "units": 120},
        {"type": "wheel", "vertical": false, "units": -120},
        {"type": "key", "code": 30, "extended": false, "down": true},
        {"type": "key", "code": 30, "extended": false, "down": false},
        {"type": "key", "code": 28, "extended": true, "down": true},
        {"type": "key", "code": 28, "extended": true, "down": false},
    ]);
    assert_eq!(parse(events.clone()).expect("parses"), events);
}

#[test]
fn every_viewer_button_round_trips_down_and_up() {
    for name in ["left", "middle", "right", "x1", "x2"] {
        for down in [true, false] {
            let events = json!([{"type": "button", "button": name, "down": down}]);
            assert_eq!(parse(events.clone()).expect("parses"), events, "{name}");
        }
    }
}

#[test]
fn viewer_events_reject_other_spellings() {
    for event in [
        json!({"type": "Move", "x": 1, "y": 2}),
        json!({"type": "pointer_move", "x": 1, "y": 2}),
        json!({"type": "button", "button": "X1", "down": true}),
        json!({"type": "button", "button": "back", "down": true}),
        json!({"type": "button", "button": "Left", "down": true}),
        json!({"type": "wheel", "vertical": true}),
    ] {
        assert!(parse(json!([event.clone()])).is_err(), "{event}");
    }
}

#[test]
fn recording_triggers_keep_their_manifest_spellings() {
    let cases = [
        (RecordingTrigger::Config, "config"),
        (RecordingTrigger::Host, "host"),
        (RecordingTrigger::ConnectFlag, "connect_flag"),
        (RecordingTrigger::Cli, "cli"),
        (RecordingTrigger::Viewer, "viewer"),
    ];
    for (trigger, spelling) in cases {
        let json = format!("\"{spelling}\"");
        assert_eq!(serde_json::to_string(&trigger).expect("serializes"), json);
        let back: RecordingTrigger = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, trigger);
        assert_eq!(trigger.as_str(), spelling);
    }
}

#[test]
fn recording_triggers_reject_the_ipc_spellings() {
    for spelling in ["GlobalConfig", "HostConfig", "ConnectFlag", "Cli", "Viewer"] {
        let json = format!("\"{spelling}\"");
        assert!(
            serde_json::from_str::<RecordingTrigger>(&json).is_err(),
            "{spelling}"
        );
    }
}
