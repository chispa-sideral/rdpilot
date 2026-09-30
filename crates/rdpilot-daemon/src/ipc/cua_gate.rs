//! The control gate on a Cua attachment: which tool calls may reach Cua
//! while a human viewer holds the session's control lease.
//!
//! - Read-only tools ([`READ_ONLY_CUA_TOOLS`]) always pass. Every other tool
//!   name, including a name this list does not know, is acting.
//! - The daemon adds an optional boolean `takeover` argument to every acting
//!   tool in `tools/list` results, and removes `takeover` from the
//!   arguments of every `tools/call` before it reaches Cua.
//! - Under a human lease an acting call is answered here with an `isError`
//!   result that names the holder and the `takeover` argument, and is not
//!   forwarded. With `"takeover": true` the lease ends (its releases are
//!   discharged by the caller) and the call is forwarded.
//! - In a JSON-RPC batch every call is checked the same way. A batch with a
//!   refused acting call is answered here as a whole and not forwarded; its
//!   read-only calls get an error that says to send them separately. Current
//!   MCP has no batches; this keeps local and Cua answers apart.
//! - Acting calls with an id count as in flight until Cua answers (result or
//!   error), so a human take waits for them. At most [`PENDING_CAP`] ids are
//!   kept; evicting one releases its count (that call no longer holds up a
//!   take). A closed attachment releases every count.

use std::collections::VecDeque;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::control::{SessionControl, Transition};

/// Cua tools that only read state. Derived from the Cua driver's own
/// `readOnlyHint` annotations (cua-driver-rs 0.30.5, the version that
/// `latest-dev` resolved to when this list was written), plus the older
/// name `screenshot`. Any tool not listed here is acting: a tool added by a
/// Cua upgrade is refused under human control until it is added here.
const READ_ONLY_CUA_TOOLS: &[&str] = &[
    "check_for_update",
    "check_permissions",
    "clipboard_read",
    "debug_window_info",
    "get_accessibility_tree",
    "get_agent_cursor_state",
    "get_browser_state",
    "get_config",
    "get_cursor_position",
    "get_desktop_state",
    "get_recording_state",
    "get_screen_size",
    "get_session",
    "get_session_state",
    "get_window_state",
    "list_apps",
    "list_sessions",
    "list_windows",
    "parse_visual_regions",
    "screenshot",
    "verify_state",
    "zoom",
];

/// Most `tools/list` and acting-call ids remembered per attachment: the
/// same bound as the call tracker's.
const PENDING_CAP: usize = crate::events::CUA_PENDING_CAP;

/// The error code of a read-only call answered locally in a refused batch.
const BATCH_REFUSED_CODE: i64 = -32000;

const BATCH_REFUSED_MESSAGE: &str = "batch not forwarded because it contains calls refused \
under human control; send read-only calls separately";

/// Whether `name` acts on the guest (anything not known to be read-only).
fn is_acting(name: &str) -> bool {
    !READ_ONLY_CUA_TOOLS.contains(&name)
}

/// The schema of the `takeover` argument added to acting tools.
fn takeover_property() -> Value {
    json!({
        "type": "boolean",
        "default": false,
        "description": "Take control of this session from a human viewer before acting. \
    Default false.",
    })
}

/// What to do with one message from the caller.
#[derive(Debug)]
pub(crate) enum Inbound {
    /// Forward this (possibly changed) message to Cua.
    Forward(Value),
    /// Do not forward; write `reply` to the caller. `request` is the
    /// message as the caller sent it, minus `takeover`.
    Answer { request: Value, reply: Value },
    /// Do not forward and do not answer (a refused notification).
    Drop,
}

/// One `tools/call` inside a message.
struct Call {
    acting: bool,
    takeover: bool,
    id: Option<Value>,
}

/// The gate of one attachment.
pub(crate) struct CuaGate {
    control: Arc<SessionControl>,
    lists: VecDeque<Value>,
    acting: VecDeque<Value>,
}

impl CuaGate {
    pub(crate) fn new(control: Arc<SessionControl>) -> Self {
        CuaGate {
            control,
            lists: VecDeque::new(),
            acting: VecDeque::new(),
        }
    }

    /// The control this gate checks.
    pub(crate) fn control(&self) -> &Arc<SessionControl> {
        &self.control
    }

    /// Classify one message from the caller. A returned transition must be
    /// discharged before a forwarded message is sent.
    pub(crate) fn inbound(&mut self, mut message: Value) -> (Inbound, Option<Transition>) {
        let calls: Vec<Call> = match &mut message {
            Value::Array(items) => items.iter_mut().filter_map(|m| self.inspect(m)).collect(),
            single => self.inspect(single).into_iter().collect(),
        };
        if !calls.iter().any(|c| c.acting) {
            return (Inbound::Forward(message), None);
        }
        // `Call::takeover` is already false for read-only calls.
        let takeover = calls.iter().any(|c| c.takeover);
        let answered: Vec<Value> = calls
            .into_iter()
            .filter(|c| c.acting)
            .filter_map(|c| c.id)
            .collect();
        match self.control.admit_cua(answered.len(), takeover) {
            Ok(transition) => {
                for id in answered {
                    self.remember_acting(id);
                }
                (Inbound::Forward(message), transition)
            }
            Err(refusal) => {
                let reply = match &message {
                    Value::Array(items) => {
                        let replies: Vec<Value> = items
                            .iter()
                            .filter_map(|item| refused_reply(item, &refusal.mcp_message))
                            .collect();
                        if replies.is_empty() {
                            return (Inbound::Drop, None);
                        }
                        Value::Array(replies)
                    }
                    single => match refused_reply(single, &refusal.mcp_message) {
                        Some(reply) => reply,
                        None => return (Inbound::Drop, None),
                    },
                };
                (
                    Inbound::Answer {
                        request: message,
                        reply,
                    },
                    None,
                )
            }
        }
    }

    /// Note a `tools/list` id, strip `takeover` from a `tools/call`, and
    /// describe the call.
    fn inspect(&mut self, message: &mut Value) -> Option<Call> {
        let object = message.as_object_mut()?;
        let method = object.get("method").and_then(Value::as_str)?.to_owned();
        let id = object.get("id").filter(|id| !id.is_null()).cloned();
        if method == "tools/list" {
            if let Some(id) = id {
                push_capped(&mut self.lists, id);
            }
            return None;
        }
        if method != "tools/call" {
            return None;
        }
        let params = object.get_mut("params").and_then(Value::as_object_mut);
        let name = params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let takeover = params
            .and_then(|p| p.get_mut("arguments"))
            .and_then(Value::as_object_mut)
            .and_then(|args| args.remove("takeover"))
            .is_some_and(|value| value == Value::Bool(true));
        let acting = is_acting(&name);
        Some(Call {
            acting,
            // `takeover` on a read-only tool is removed and ignored.
            takeover: acting && takeover,
            id,
        })
    }

    fn remember_acting(&mut self, id: Value) {
        // A reused id replaces the older call; only one answer will come.
        if let Some(pos) = self.acting.iter().position(|p| *p == id) {
            self.acting.remove(pos);
            self.control.cua_finished(1);
        }
        if self.acting.len() >= PENDING_CAP {
            self.acting.pop_front();
            self.control.cua_finished(1);
        }
        self.acting.push_back(id);
    }

    /// Patch `tools/list` results and settle answered acting calls in one
    /// message from Cua.
    pub(crate) fn outbound(&mut self, message: &mut Value) {
        match message {
            Value::Array(items) => items.iter_mut().for_each(|item| self.response(item)),
            single => self.response(single),
        }
    }

    fn response(&mut self, message: &mut Value) {
        let Some(object) = message.as_object_mut() else {
            return;
        };
        if object.contains_key("method")
            || !(object.contains_key("result") || object.contains_key("error"))
        {
            return;
        }
        let Some(id) = object.get("id").cloned() else {
            return;
        };
        if let Some(pos) = self.acting.iter().position(|p| *p == id) {
            self.acting.remove(pos);
            self.control.cua_finished(1);
        }
        if let Some(pos) = self.lists.iter().position(|p| *p == id) {
            self.lists.remove(pos);
            if let Some(tools) = object
                .get_mut("result")
                .and_then(|r| r.get_mut("tools"))
                .and_then(Value::as_array_mut)
            {
                tools.iter_mut().for_each(patch_tool);
            }
        }
    }
}

impl Drop for CuaGate {
    fn drop(&mut self) {
        self.control.cua_finished(self.acting.len());
    }
}

fn push_capped(queue: &mut VecDeque<Value>, id: Value) {
    if queue.len() >= PENDING_CAP {
        queue.pop_front();
    }
    queue.push_back(id);
}

/// Add `takeover` to an acting tool's input schema.
fn patch_tool(tool: &mut Value) {
    let acting = tool
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(is_acting);
    if !acting {
        return;
    }
    let Some(schema) = tool.get_mut("inputSchema").and_then(Value::as_object_mut) else {
        return;
    };
    let properties = schema
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(properties) = properties.as_object_mut() {
        properties.insert("takeover".to_owned(), takeover_property());
    }
}

/// The local answer to one element of a refused message: the human-control
/// `isError` result for an acting call, a JSON-RPC error for anything else
/// with an id, nothing for a notification.
fn refused_reply(message: &Value, text: &str) -> Option<Value> {
    let id = message.get("id").filter(|id| !id.is_null())?.clone();
    let acting = message.get("method").and_then(Value::as_str) == Some("tools/call")
        && is_acting(
            message
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    Some(if acting {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": text }], "isError": true },
        })
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": BATCH_REFUSED_CODE, "message": BATCH_REFUSED_MESSAGE },
        })
    })
}

#[cfg(test)]
#[path = "cua_gate_tests.rs"]
mod tests;
