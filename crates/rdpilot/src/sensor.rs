//! The `RDPILOT_SENSOR` dynamic virtual channel transport (SENSOR-03).
//!
//! Crate-internal only (D-09): nothing here is re-exported from `lib.rs`. This
//! module owns the durable `{ version, req_id, type, payload }` JSON envelope
//! (D-4.3), the `RdpilotSensorProcessor` (IronRDP `DvcProcessor` implementation),
//! and the `SensorShared` correlation state that lets `Session::ping()` (a later
//! plan) round-trip a request across the dedicated session-loop OS thread.
//!
//! This module implements the `Version`/`Ping`/`Pong` message types (D-4.3),
//! the Phase 6 `WindowList`/`ProcessTree`/`SetForegroundWindow`/
//! `LaunchProcess` request/response types, and Phase 7's `Uia` tree-walk
//! request type. All non-`Version` replies share one generic,
//! msg_type-agnostic fulfilment path keyed on `req_id` (RESEARCH Pattern 1)
//! — the envelope shape did not need reworking to add `Uia`.
//!
//! No `unwrap`/`expect`/`panic!` outside `#[cfg(test)]` (API-01): malformed or
//! oversized inbound JSON from the (by-design unauthenticated, Pitfall m3) DVC
//! channel is always mapped to a typed drop or `Error::Dvc`, never a panic
//! (ASVS V5, T-04-01).

use serde::{Deserialize, Serialize};

/// The SDK's own protocol version for the `RDPILOT_SENSOR` envelope.
///
/// Sent as the first outbound message (`RdpilotSensorProcessor::start()`,
/// SC#3) and compared against the peer's echoed value to detect version skew.
pub(crate) const PROTOCOL_VERSION: u32 = 1;

/// The durable request/response envelope carried over `RDPILOT_SENSOR`
/// (D-4.3): `{ version, req_id, type, payload }`.
///
/// `req_id` is a correlation id reserved for future concurrent queries
/// (Phases 6-7); `payload` stays `None` for all `v1` message types but MUST
/// remain part of the wire shape so later `WindowList`/`ProcessTree`/`Uia`
/// payloads can be added without reworking framing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub(crate) version: u32,
    pub(crate) req_id: u64,
    #[serde(rename = "type")]
    pub(crate) msg_type: MsgType,
    pub(crate) payload: Option<serde_json::Value>,
}

/// The set of message types this crate can send/receive over the
/// `RDPILOT_SENSOR` DVC (D-4.3, Phase 6 wire contract, extended by Phase 7,
/// Phase 10): the version handshake, the ping/pong heartbeat, the four
/// Phase 6 request/response types, Phase 7's `Uia` tree-walk request, and
/// Phase 10's `FileTransfer` sensor-mediated upload/download trigger.
/// Serializes as the bare externally-tagged unit string form — `"Version"`,
/// `"Ping"`, `"Pong"`, `"WindowList"`, `"ProcessTree"`,
/// `"SetForegroundWindow"`, `"LaunchProcess"`, `"Uia"`, `"FileTransfer"` —
/// matching the wire shape verified in RESEARCH Q2. `Uia`/`FileTransfer`
/// reuse the existing generic non-`Version` fulfilment path (RESEARCH
/// Pattern 1) with zero dispatch/correlation changes. `FileTransfer` carries
/// a single request/reply pair with an upload/download discriminant in the
/// payload (10-CONTEXT Claude's Discretion) rather than two separate
/// variants — matches the `{op, remote_path, share_name}` request /
/// `{success, data, error, error_kind}` reply shape (10-03-PLAN Task 1); the
/// payload DTOs live sensor-side only (`sensor/FileTransfer.cs`) since this
/// crate's `sensor_request` returns the raw `serde_json::Value` reply for
/// Plan 10-04 to interpret (D-09 crate-internal-only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MsgType {
    Version,
    Ping,
    Pong,
    WindowList,
    ProcessTree,
    SetForegroundWindow,
    LaunchProcess,
    Uia,
    FileTransfer,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializing a `Version` envelope produces the exact wire shape RESEARCH
    /// Q2 documents: `type` is the bare string `"Version"`, `req_id` is `0`.
    #[test]
    fn version_envelope_serializes_with_bare_type_string() {
        let env = Envelope {
            version: 1,
            req_id: 0,
            msg_type: MsgType::Version,
            payload: None,
        };
        let value = serde_json::to_value(&env).expect("envelope serializes");
        assert_eq!(value["type"], "Version");
        assert_eq!(value["req_id"], 0);
        assert_eq!(value["version"], 1);
        assert!(value["payload"].is_null());
    }

    /// A `Ping` envelope round-trips byte-for-value through `to_vec` →
    /// `from_slice` (the exact codec path `JsonDvcMessage`/`process()` use).
    #[test]
    fn ping_envelope_round_trips_to_vec_from_slice() {
        let env = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 7,
            msg_type: MsgType::Ping,
            payload: None,
        };
        let bytes = serde_json::to_vec(&env).expect("ping envelope encodes");
        let decoded: Envelope = serde_json::from_slice(&bytes).expect("ping envelope decodes");
        assert_eq!(decoded, env);
    }

    /// Malformed/truncated bytes must yield a typed `Err`, never panic — the
    /// processor maps this to a dropped message (API-01, ASVS V5, T-04-01).
    #[test]
    fn garbage_bytes_yield_err_never_panic() {
        let garbage: &[u8] = b"{not valid json";
        let result: Result<Envelope, _> = serde_json::from_slice(garbage);
        assert!(result.is_err());
    }

    /// Serializing a `FileTransfer` envelope produces the exact wire shape
    /// (10-03-PLAN Task 1 acceptance criterion): `type` is the bare string
    /// `"FileTransfer"`, matching `sensor/Envelope.cs`'s `MsgType` enum in
    /// the same ordinal position (last variant).
    #[test]
    fn file_transfer_envelope_serializes_with_bare_type_string() {
        let env = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 3,
            msg_type: MsgType::FileTransfer,
            payload: None,
        };
        let value = serde_json::to_value(&env).expect("envelope serializes");
        assert_eq!(value["type"], "FileTransfer");
        assert_eq!(value["req_id"], 3);
        assert_eq!(value["version"], PROTOCOL_VERSION);
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::bootstrap::{BootstrapProgress, BootstrapStage};

use ironrdp::core::{ensure_size, impl_as_any, Encode, EncodeResult, WriteCursor};
use ironrdp::dvc::{DvcEncode, DvcMessage, DvcProcessor};
use ironrdp::pdu::PduResult;
use tokio::sync::oneshot;

/// An outbound envelope wrapped for the `ironrdp-dvc` wire (`Encode` +
/// `DvcEncode`).
///
/// `ironrdp_dvc::encode_dvc_messages` (called by a later plan's session-loop
/// dispatch arm) already handles the wire-level PDU splitting/reassembly for
/// oversized payloads — this wrapper only needs to hand over the raw JSON
/// bytes (D-4.3, "Don't Hand-Roll" chunking).
pub(crate) struct JsonDvcMessage(Vec<u8>);

impl JsonDvcMessage {
    /// Serialize `value` to JSON and wrap it for the DVC encode path.
    fn new<T: Serialize>(value: &T) -> std::result::Result<Self, serde_json::Error> {
        Ok(Self(serde_json::to_vec(value)?))
    }
}

impl Encode for JsonDvcMessage {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ensure_size!(in: dst, size: self.0.len());
        dst.write_slice(&self.0);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "rdpilot-sensor-json"
    }

    fn size(&self) -> usize {
        self.0.len()
    }
}

impl DvcEncode for JsonDvcMessage {}

/// The outcome of the version handshake (SC#3): the very first exchange on
/// the `RDPILOT_SENSOR` channel.
///
/// `Session::ping()` (a later plan) checks this before attempting a round
/// trip and fails fast with `Error::Dvc` if `Mismatched` — a version skew is
/// always a clear typed error, never silent corruption (T-04-02).
#[derive(Debug)]
pub(crate) enum HandshakeState {
    /// No `Version` reply has been processed yet.
    Pending,
    /// The peer's version matched `PROTOCOL_VERSION`.
    Ok,
    /// The peer's version did not match; carries both sides for a useful
    /// error message.
    Mismatched { local: u32, remote: u32 },
}

/// Correlation state shared between `RdpilotSensorProcessor` (mutated on the
/// dedicated session-loop OS thread when a reply's bytes are decoded) and
/// `Session` (mutated from whatever thread calls a later plan's
/// `Session::ping()`).
///
/// `oneshot::Sender` is runtime-agnostic, so it safely crosses the OS-thread
/// boundary between the session loop and the caller's async context
/// (RESEARCH Q1). `std::sync::Mutex` is correct here because both fields are
/// only ever locked for a brief synchronous mutation, never held across an
/// `.await`.
#[derive(Debug)]
pub(crate) struct SensorShared {
    /// Fields are `pub(crate)` (not accessed via getters): `Session::ping()`
    /// (session.rs) and `RdpilotSensorProcessor::process()` (this module)
    /// both need direct lock access to the same two mutexes, and both are
    /// already crate-internal-only (D-09) — no accessor indirection needed.
    pub(crate) pending: Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
    pub(crate) handshake: Mutex<HandshakeState>,
    pub(crate) bootstrap: BootstrapProgress,
}

impl SensorShared {
    /// A fresh, unstarted correlation state: no pending requests, handshake
    /// not yet completed.
    pub(crate) fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            handshake: Mutex::new(HandshakeState::Pending),
            bootstrap: BootstrapProgress::new(),
        }
    }
}

/// The `ironrdp-dvc` [`DvcProcessor`] for the `RDPILOT_SENSOR` channel
/// (SENSOR-03).
///
/// `start()` unconditionally sends the `Version` handshake as the first
/// outbound message (SC#3); `process()` drives the handshake state machine
/// and generically fulfils any pending `req_id` correlation regardless of
/// reply `msg_type` (RESEARCH Pattern 1). Crate-internal only — no
/// `ironrdp-dvc` type is ever re-exported from this crate's public API
/// (D-09).
pub(crate) struct RdpilotSensorProcessor {
    shared: Arc<SensorShared>,
}

// Without `AsAny`, `get_dvc::<RdpilotSensorProcessor>()` /
// `channel_processor_downcast_ref::<RdpilotSensorProcessor>()`'s `TypeId`
// lookup silently returns `None` (Pitfall 4) — must be added immediately
// after the struct definition.
impl_as_any!(RdpilotSensorProcessor);

impl RdpilotSensorProcessor {
    /// Build a new processor sharing correlation state with its `Session`
    /// counterpart.
    pub(crate) fn new(shared: Arc<SensorShared>) -> Self {
        Self { shared }
    }

    /// Build an envelope for `msg_type`/`req_id`/`payload` and return it as a
    /// single boxed outbound DVC message (RESEARCH Pattern 2: one encode path
    /// for every outbound request type, not one hand-written method per
    /// `MsgType`).
    ///
    /// The caller (the session-loop dispatch arm) hands the result to
    /// `ironrdp_dvc::encode_dvc_messages` for wire-level chunking — this
    /// method never chunks (D-4.3).
    pub(crate) fn encode_request(
        &self,
        msg_type: MsgType,
        req_id: u64,
        payload: Option<serde_json::Value>,
    ) -> std::result::Result<Vec<DvcMessage>, serde_json::Error> {
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            req_id,
            msg_type,
            payload,
        };
        let msg = JsonDvcMessage::new(&envelope)?;
        Ok(vec![Box::new(msg)])
    }

    pub(crate) fn sensor_shared(&self) -> Arc<SensorShared> {
        self.shared.clone()
    }
}

impl DvcProcessor for RdpilotSensorProcessor {
    fn channel_name(&self) -> &str {
        crate::connect::RDPILOT_SENSOR
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 0,
            msg_type: MsgType::Version,
            payload: None,
        };
        let msg = JsonDvcMessage::new(&envelope).map_err(|e| {
            ironrdp::pdu::pdu_other_err!("rdpilot-sensor: version envelope encode failed", source: e)
        })?;
        self.shared.bootstrap.record(BootstrapStage::DvcChannelCreated);
        Ok(vec![Box::new(msg)])
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        // Malformed/unknown-shape JSON from the (by-design unauthenticated,
        // Pitfall m3) channel is dropped, never propagated as a hard error and
        // never a panic (API-01, ASVS V5, T-04-01).
        let Ok(envelope) = serde_json::from_slice::<Envelope>(payload) else {
            return Ok(Vec::new());
        };

        match envelope.msg_type {
            MsgType::Version => {
                let mut handshake = match self.shared.handshake.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                *handshake = if envelope.version == PROTOCOL_VERSION {
                    HandshakeState::Ok
                } else {
                    HandshakeState::Mismatched {
                        local: PROTOCOL_VERSION,
                        remote: envelope.version,
                    }
                };
                self.shared.bootstrap.record(BootstrapStage::VersionReceived);
                // The handshake is a terminal two-message exchange; no reply.
                Ok(Vec::new())
            }
            // Every other reply type (Pong, WindowList, ProcessTree,
            // SetForegroundWindow, LaunchProcess, and any future addition)
            // shares one generic, msg_type-agnostic fulfilment path
            // (RESEARCH Pattern 1): if `req_id` has a pending oneshot, hand it
            // the raw reply payload and let the awaiting caller (`Session`)
            // interpret/deserialize it for its own request type. A reply for
            // an unknown/already-fulfilled req_id is silently dropped — it
            // cannot resurrect or double-fulfil a stale entry (T-06-04).
            _ => {
                let mut pending = match self.shared.pending.lock() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if let Some(sender) = pending.remove(&envelope.req_id) {
                    if envelope.msg_type == MsgType::Pong {
                        self.shared.bootstrap.record(BootstrapStage::PongReceived);
                    }
                    // Ignore the Result: the receiver may already be gone if
                    // a caller-side timeout fired first.
                    let _ = sender.send(envelope.payload.unwrap_or(serde_json::Value::Null));
                }
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod processor_tests {
    use super::*;

    fn fresh_processor() -> (RdpilotSensorProcessor, Arc<SensorShared>) {
        let shared = Arc::new(SensorShared::new());
        (RdpilotSensorProcessor::new(shared.clone()), shared)
    }

    /// `start()` sends exactly one Version envelope with `req_id == 0` and
    /// `version == PROTOCOL_VERSION` — the unconditional first outbound
    /// message (SC#3 first-message guard).
    #[test]
    fn start_emits_single_version_envelope_with_req_id_zero() {
        let (mut processor, shared) = fresh_processor();

        let messages = processor.start(0).expect("start() succeeds");
        assert_eq!(messages.len(), 1, "start() must emit exactly one message");

        // Decode the bytes back the same way JsonDvcMessage encodes them: the
        // wrapper's Encode writes the raw JSON bytes verbatim.
        let mut buf = vec![0u8; messages[0].size()];
        let mut cursor = WriteCursor::new(&mut buf);
        messages[0].encode(&mut cursor).expect("encode succeeds");

        let decoded: Envelope = serde_json::from_slice(&buf).expect("decodes as an envelope");
        assert_eq!(decoded.req_id, 0);
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.msg_type, MsgType::Version);
        assert_eq!(shared.bootstrap.snapshot(), vec![BootstrapStage::DvcChannelCreated]);
    }

    /// `channel_name()` returns the same reserved constant `connect.rs` uses
    /// to register the channel (SC#1) — not a duplicated literal.
    #[test]
    fn channel_name_matches_reserved_const() {
        let (processor, _shared) = fresh_processor();
        assert_eq!(processor.channel_name(), crate::connect::RDPILOT_SENSOR);
    }

    fn encode_envelope(env: &Envelope) -> Vec<u8> {
        serde_json::to_vec(env).expect("envelope encodes")
    }

    /// A matching-version `Version` reply sets `HandshakeState::Ok`.
    #[test]
    fn matching_version_reply_sets_handshake_ok() {
        let (mut processor, shared) = fresh_processor();
        let reply = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 0,
            msg_type: MsgType::Version,
            payload: None,
        };

        let out = processor
            .process(0, &encode_envelope(&reply))
            .expect("process() succeeds");
        assert!(out.is_empty(), "handshake reply produces no outbound message");

        let handshake = match shared.handshake.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(matches!(*handshake, HandshakeState::Ok));
        assert_eq!(shared.bootstrap.snapshot(), vec![BootstrapStage::VersionReceived]);
    }

    /// A mismatched-version `Version` reply sets `HandshakeState::Mismatched`
    /// with both sides recorded (SC#3 negative path — the only place this is
    /// testable per RESEARCH A3, since the phase's own responder always
    /// matches).
    #[test]
    fn mismatched_version_reply_sets_handshake_mismatched() {
        let (mut processor, shared) = fresh_processor();
        let reply = Envelope {
            version: PROTOCOL_VERSION + 1,
            req_id: 0,
            msg_type: MsgType::Version,
            payload: None,
        };

        processor
            .process(0, &encode_envelope(&reply))
            .expect("process() succeeds");

        let handshake = match shared.handshake.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match *handshake {
            HandshakeState::Mismatched { local, remote } => {
                assert_eq!(local, PROTOCOL_VERSION);
                assert_eq!(remote, PROTOCOL_VERSION + 1);
            }
            _ => panic!("expected Mismatched, got a different HandshakeState"),
        }
    }

    /// A `Pong` with a known `req_id` fulfils the pending `oneshot` and
    /// removes the entry so it cannot be double-fulfilled. A payload-less
    /// reply fulfils with `Value::Null` (the generic fulfilment path's
    /// `unwrap_or(Value::Null)` default).
    #[test]
    fn pong_fulfils_and_removes_pending_oneshot() {
        let (mut processor, shared) = fresh_processor();
        let (tx, mut rx) = oneshot::channel();
        {
            let mut pending = match shared.pending.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            pending.insert(7, tx);
        }

        let reply = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 7,
            msg_type: MsgType::Pong,
            payload: None,
        };
        processor
            .process(0, &encode_envelope(&reply))
            .expect("process() succeeds");

        assert_eq!(
            rx.try_recv(),
            Ok(serde_json::Value::Null),
            "oneshot must be fulfilled with Value::Null for a payload-less reply"
        );

        let pending = match shared.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!pending.contains_key(&7), "entry must be removed after fulfilling");
        assert_eq!(shared.bootstrap.snapshot(), vec![BootstrapStage::PongReceived]);
    }

    /// A `Version` reply (req_id 0) still drives the handshake state machine
    /// and never touches the pending map — the Version arm is handled
    /// separately from the generic fulfilment arm and does not consult
    /// `pending` at all, even though `req_id` is 0 and 0 is a value a caller
    /// could theoretically register.
    #[test]
    fn version_reply_drives_handshake_and_never_touches_pending_map() {
        let (mut processor, shared) = fresh_processor();
        {
            let mut pending = match shared.pending.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let (tx, _rx) = oneshot::channel();
            pending.insert(0, tx);
        }

        let reply = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 0,
            msg_type: MsgType::Version,
            payload: None,
        };
        processor
            .process(0, &encode_envelope(&reply))
            .expect("process() succeeds");

        let handshake = match shared.handshake.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(matches!(*handshake, HandshakeState::Ok));

        let pending = match shared.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(
            pending.contains_key(&0),
            "the Version arm must never touch the pending map, even for req_id 0"
        );
    }

    /// A `WindowList` reply with a pending `req_id` fulfils that req_id's
    /// oneshot with `envelope.payload`, proving the fulfilment path is
    /// msg_type-agnostic, NOT special-cased to `Pong` (RESEARCH Pattern 1).
    #[test]
    fn window_list_reply_fulfils_pending_oneshot_with_payload() {
        let (mut processor, shared) = fresh_processor();
        let (tx, mut rx) = oneshot::channel();
        {
            let mut pending = match shared.pending.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            pending.insert(42, tx);
        }

        let payload = serde_json::json!({"success": true, "data": []});
        let reply = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 42,
            msg_type: MsgType::WindowList,
            payload: Some(payload.clone()),
        };
        processor
            .process(0, &encode_envelope(&reply))
            .expect("process() succeeds");

        assert_eq!(
            rx.try_recv(),
            Ok(payload),
            "oneshot must be fulfilled with the reply's payload verbatim"
        );

        let pending = match shared.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!pending.contains_key(&42), "entry must be removed after fulfilling");
    }

    /// A reply whose `req_id` is NOT in the pending map is dropped
    /// (`Ok(empty)`, no panic) and leaves the map unchanged (T-06-04: a late
    /// reply for an unknown req_id must not resurrect or corrupt state).
    #[test]
    fn reply_for_unknown_req_id_is_dropped_and_map_unchanged() {
        let (mut processor, shared) = fresh_processor();
        {
            let mut pending = match shared.pending.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            let (tx, _rx) = oneshot::channel();
            pending.insert(1, tx);
        }

        let reply = Envelope {
            version: PROTOCOL_VERSION,
            req_id: 999,
            msg_type: MsgType::Pong,
            payload: None,
        };
        let out = processor
            .process(0, &encode_envelope(&reply))
            .expect("process() succeeds");
        assert!(out.is_empty());

        let pending = match shared.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert_eq!(pending.len(), 1, "map must be unchanged");
        assert!(pending.contains_key(&1), "the unrelated pending entry must survive untouched");
    }

    /// Malformed/truncated inbound bytes are dropped (`Ok(empty)`), never a
    /// panic (API-01, ASVS V5, T-04-01).
    #[test]
    fn malformed_payload_is_dropped_without_panic() {
        let (mut processor, _shared) = fresh_processor();
        let out = processor.process(0, b"{not valid json").expect("never returns Err, never panics");
        assert!(out.is_empty());
    }
}
