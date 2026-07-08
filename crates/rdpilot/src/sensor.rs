//! The `RDPILOT_SENSOR` dynamic virtual channel transport (SENSOR-03).
//!
//! Crate-internal only (D-09): nothing here is re-exported from `lib.rs`. This
//! module owns the durable `{ version, req_id, type, payload }` JSON envelope
//! (D-4.3), the `RdpilotSensorProcessor` (IronRDP `DvcProcessor` implementation),
//! and the `SensorShared` correlation state that lets `Session::ping()` (a later
//! plan) round-trip a request across the dedicated session-loop OS thread.
//!
//! This module implements ONLY the `Version`/`Ping`/`Pong` message types
//! (D-4.3). `WindowList`/`ProcessTree`/`Uia` payloads are Phase 6-7 work and are
//! deliberately absent here — the envelope shape is designed to grow into them
//! without reworking framing.
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

/// The set of message types implemented this phase (D-4.3): only the version
/// handshake and the ping/pong heartbeat. Serializes as the bare externally-
/// tagged unit string form — `"Version"`, `"Ping"`, `"Pong"` — matching the
/// wire shape verified in RESEARCH Q2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MsgType {
    Version,
    Ping,
    Pong,
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
}
