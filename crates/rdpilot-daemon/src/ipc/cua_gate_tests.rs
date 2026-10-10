//! Cua gate: schema patch, stripping, refusal, takeover, batches, in-flight
//! bookkeeping.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use rdpilot_vocab::RawInput;

use super::*;
use crate::control::TakeError;
use crate::events::{EventKind, EventSource, SessionEvents};
use crate::seams::{DaemonError, HumanInput, SendFuture};

#[derive(Default)]
struct Sink(Mutex<Vec<RawInput>>);

impl HumanInput for Sink {
    fn send(&self, events: Vec<RawInput>) -> SendFuture<'_, Result<(), DaemonError>> {
        self.0.lock().unwrap().extend(events);
        Box::pin(async { Ok(()) })
    }
}

struct Fixture {
    gate: CuaGate,
    control: Arc<SessionControl>,
    events: Arc<SessionEvents>,
    sink: Arc<Sink>,
}

fn fixture() -> Fixture {
    let events = Arc::new(SessionEvents::new(&"notepad".parse().unwrap(), 1, None));
    let sink = Arc::new(Sink::default());
    let control = Arc::new(SessionControl::new(
        "notepad",
        1,
        Arc::clone(&events),
        None,
        Some(Arc::clone(&sink) as Arc<dyn HumanInput>),
    ));
    Fixture {
        gate: CuaGate::new(Arc::clone(&control)),
        control,
        events,
        sink,
    }
}

fn address() -> IpAddr {
    "100.101.102.103".parse().unwrap()
}

async fn human(f: &Fixture) -> crate::control::Grant {
    f.control.take(address(), async {}).await.unwrap()
}

fn call(id: u64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})
}

fn forwarded(inbound: Inbound) -> Value {
    match inbound {
        Inbound::Forward(message) => message,
        other => panic!("expected Forward, got {other:?}"),
    }
}

#[test]
fn the_read_only_list_is_pinned_and_unknown_tools_act() {
    let mut sorted = READ_ONLY_CUA_TOOLS.to_vec();
    sorted.sort_unstable();
    assert_eq!(sorted, READ_ONLY_CUA_TOOLS, "keep the list sorted");
    assert_eq!(READ_ONLY_CUA_TOOLS.len(), 22);
    for tool in ["get_window_state", "list_windows", "screenshot", "zoom"] {
        assert!(!is_acting(tool));
    }
    for tool in [
        "type_text",
        "click",
        "launch_app",
        "health_report",
        "a_tool_from_a_newer_cua",
        "",
    ] {
        assert!(is_acting(tool), "{tool}");
    }
}

#[test]
fn tools_list_gets_takeover_on_acting_tools_only_and_nothing_else_changes() {
    let mut f = fixture();
    let request = json!({"jsonrpc":"2.0","id":"l1","method":"tools/list"});
    assert_eq!(forwarded(f.gate.inbound(request.clone()).0), request);
    let schema =
        json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]});
    let mut response = json!({"jsonrpc":"2.0","id":"l1","result":{"tools":[
        {"name":"type_text","description":"d","inputSchema":schema},
        {"name":"list_windows","inputSchema":{"type":"object"}},
        {"name":"click","inputSchema":{"type":"object"}},
    ],"nextCursor":"c"}});
    f.gate.outbound(&mut response);
    let tools = response["result"]["tools"].as_array().unwrap();
    let mut expected = schema.clone();
    expected["properties"]["takeover"] = json!({
        "type": "boolean",
        "default": false,
        "description": "Take control of this session from a human viewer before acting. Default false.",
    });
    assert_eq!(tools[0]["inputSchema"], expected);
    assert_eq!(tools[0]["description"], "d");
    assert_eq!(tools[1]["inputSchema"], json!({"type":"object"}));
    assert_eq!(
        tools[2]["inputSchema"]["properties"]["takeover"]["type"],
        "boolean"
    );
    assert_eq!(response["result"]["nextCursor"], "c");
    // A response to another id is not patched; a batch response is.
    let mut other = json!({"jsonrpc":"2.0","id":"l1","result":{"tools":[{"name":"click","inputSchema":{"type":"object"}}]}});
    f.gate.outbound(&mut other);
    assert!(other["result"]["tools"][0]["inputSchema"]
        .get("properties")
        .is_none());
    let _ = f
        .gate
        .inbound(json!([{"jsonrpc":"2.0","id":7,"method":"tools/list"}]));
    let mut batch = json!([{"jsonrpc":"2.0","id":7,"result":{"tools":[{"name":"click","inputSchema":{"type":"object"}}]}}]);
    f.gate.outbound(&mut batch);
    assert!(batch[0]["result"]["tools"][0]["inputSchema"]["properties"]["takeover"].is_object());
}

#[test]
fn under_agent_control_takeover_is_stripped_and_everything_passes() {
    let mut f = fixture();
    for arguments in [
        json!({"text":"hi"}),
        json!({"text":"hi","takeover":false}),
        json!({"text":"hi","takeover":true}),
        json!({"text":"hi","takeover":"yes"}),
    ] {
        let (inbound, transition) = f.gate.inbound(call(1, "type_text", arguments));
        assert!(transition.is_none());
        assert_eq!(
            forwarded(inbound),
            call(1, "type_text", json!({"text":"hi"}))
        );
    }
    assert!(f.events.after(0).events.is_empty());
}

#[tokio::test]
async fn under_a_human_lease_acting_calls_are_refused_and_read_only_calls_pass() {
    let mut f = fixture();
    human(&f).await;
    let (inbound, _) = f
        .gate
        .inbound(call(5, "type_text", json!({"text":"x","takeover":false})));
    let Inbound::Answer { request, reply } = inbound else {
        panic!("expected a local answer");
    };
    assert_eq!(request, call(5, "type_text", json!({"text":"x"})));
    assert_eq!(reply["id"], 5);
    assert_eq!(reply["result"]["isError"], true);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text
        .starts_with("session \"notepad\" is controlled by human viewer 100.101.102.103 since "));
    assert!(text.ends_with(
        " UTC; wait and retry, or repeat this call with \"takeover\": true to take control"
    ));
    assert_eq!(f.control.inflight(), 0, "a refused call is not in flight");
    // Read-only calls pass; `takeover` on them is stripped and ignored.
    let (inbound, transition) = f.gate.inbound(call(
        6,
        "get_window_state",
        json!({"pid":1,"takeover":true}),
    ));
    assert!(transition.is_none());
    assert_eq!(
        forwarded(inbound),
        call(6, "get_window_state", json!({"pid":1}))
    );
    assert!(
        f.control.check_agent().is_err(),
        "still under human control"
    );
    // An acting notification is dropped.
    let (inbound, _) = f
        .gate
        .inbound(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"click"}}));
    assert!(matches!(inbound, Inbound::Drop));
}

#[tokio::test]
async fn takeover_true_ends_the_lease_with_releases_before_forwarding() {
    let mut f = fixture();
    let grant = human(&f).await;
    f.control
        .input(
            &grant.lease,
            grant.generation,
            (0, 0),
            vec![RawInput::Key {
                code: 0x2A,
                extended: false,
                down: true,
            }],
        )
        .await
        .unwrap();
    f.sink.0.lock().unwrap().clear();
    let (inbound, transition) =
        f.gate
            .inbound(call(9, "type_text", json!({"text":"abc","takeover":true})));
    let transition = transition.expect("the lease ended");
    f.control.discharge(transition).await;
    assert_eq!(
        *f.sink.0.lock().unwrap(),
        vec![RawInput::Key {
            code: 0x2A,
            extended: false,
            down: false
        }]
    );
    assert_eq!(
        forwarded(inbound),
        call(9, "type_text", json!({"text":"abc"}))
    );
    assert_eq!(f.control.inflight(), 1);
    let last = f.events.after(0).events.pop().unwrap();
    assert_eq!(last.source, EventSource::Cua);
    assert!(matches!(last.kind, EventKind::ControlTakenOver { .. }));
    assert!(f.control.check_agent().is_ok());
}

#[tokio::test]
async fn batches_are_checked_per_call() {
    let mut f = fixture();
    // Under agent control a batch passes, stripped.
    let batch = json!([
        call(1, "click", json!({"x":1,"takeover":false})),
        call(2, "list_windows", json!({})),
    ]);
    let forwarded_batch = forwarded(f.gate.inbound(batch).0);
    assert_eq!(forwarded_batch[0], call(1, "click", json!({"x":1})));
    assert_eq!(f.control.inflight(), 1);
    let mut answer = json!([{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"m"}}]);
    f.gate.outbound(&mut answer);
    assert_eq!(
        f.control.inflight(),
        0,
        "an error response settles the call"
    );

    // Under a human lease a batch with a refused call is answered locally.
    human(&f).await;
    let batch = json!([
        call(3, "click", json!({})),
        call(4, "list_windows", json!({})),
        {"jsonrpc":"2.0","method":"notifications/progress"},
    ]);
    let Inbound::Answer { reply, .. } = f.gate.inbound(batch).0 else {
        panic!("expected a local answer");
    };
    let replies = reply.as_array().unwrap();
    assert_eq!(replies.len(), 2, "no answer to the notification");
    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(replies[1]["error"]["code"], -32000);
    assert!(replies[1]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("send read-only calls separately"));
    // A read-only batch passes.
    let batch = json!([
        call(5, "list_windows", json!({})),
        call(6, "zoom", json!({}))
    ]);
    assert!(matches!(f.gate.inbound(batch).0, Inbound::Forward(_)));
    // Any acting call with takeover: true takes over for the whole batch.
    let batch = json!([
        call(7, "click", json!({})),
        call(8, "type_text", json!({"takeover":true})),
    ]);
    let (inbound, transition) = f.gate.inbound(batch);
    assert!(transition.is_some());
    let sent = forwarded(inbound);
    assert!(sent[1]["params"]["arguments"].get("takeover").is_none());
    assert_eq!(f.control.inflight(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_human_take_waits_for_forwarded_acting_calls_and_their_eviction_or_close() {
    let mut f = fixture();
    forwarded(f.gate.inbound(call(1, "hold", json!({}))).0);
    let busy = f
        .control
        .take_within(address(), async {}, Duration::from_secs(10))
        .await;
    assert_eq!(busy.unwrap_err(), TakeError::Busy);
    let mut answer = json!({"jsonrpc":"2.0","id":1,"result":{"content":[]}});
    f.gate.outbound(&mut answer);
    assert_eq!(f.control.inflight(), 0);
    // A reused id counts once.
    forwarded(f.gate.inbound(call(2, "hold", json!({}))).0);
    forwarded(f.gate.inbound(call(2, "hold", json!({}))).0);
    assert_eq!(f.control.inflight(), 1);
    let mut answer = json!({"jsonrpc":"2.0","id":2,"result":{"content":[]}});
    f.gate.outbound(&mut answer);
    assert_eq!(f.control.inflight(), 0);
    // Eviction at the cap releases the oldest count.
    for id in 0..(PENDING_CAP as u64 + 3) {
        forwarded(f.gate.inbound(call(100 + id, "hold", json!({}))).0);
    }
    assert_eq!(f.control.inflight(), PENDING_CAP);
    // Closing the attachment releases the rest; a take then succeeds.
    let control = Arc::clone(&f.control);
    drop(f.gate);
    assert_eq!(control.inflight(), 0);
    assert!(control.take(address(), async {}).await.is_ok());
}
