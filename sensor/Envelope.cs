// The `RDPILOT_SENSOR` envelope — the SERVER side of the fixed wire protocol
// (D-4.3, D-5.7). Mirrors crates/rdpilot/src/sensor.rs's `Envelope`/`MsgType`
// byte-for-byte: `{ version, req_id, type, payload }`, with `type` serialized
// as the bare externally-tagged unit-string form ("Version"/"Ping"/"Pong").
//
// Phase 5 stays Version/Ping/Pong only (D-5.4, no COM/UIA payload types yet).

namespace RdpilotSensor;

/// The SDK's own protocol version for the RDPILOT_SENSOR envelope. Matches
/// `crate::sensor::PROTOCOL_VERSION` on the Rust side.
internal static class ProtocolVersion
{
    internal const uint Value = 1;
}

/// The set of message types implemented this phase (D-4.3/D-5.4): only the
/// version handshake and the ping/pong heartbeat. `System.Text.Json` writes
/// this as the bare string form ("Version"/"Ping"/"Pong") via
/// `JsonStringEnumConverter` wired through `EnvelopeJsonContext` — matching
/// the Rust side's serde externally-tagged unit-variant wire shape exactly.
internal enum MsgType
{
    Version,
    Ping,
    Pong,
}

/// The durable request/response envelope carried over `RDPILOT_SENSOR`:
/// `{ version, req_id, type, payload }` (D-4.3). Field names match the Rust
/// `serde` wire names exactly (snake_case `req_id`, bare `type`, `payload`,
/// `version`) via `[JsonPropertyName]` — this record's own C# property names
/// are PascalCase by convention, the wire name is pinned per-property so the
/// JSON byte shape is identical to what `sensor.rs`'s tests assert.
///
/// `req_id` is a correlation id (Ping requests are correlated to their Pong
/// reply); `payload` stays `null` for all v1 message types but remains part
/// of the shape so future `WindowList`/`ProcessTree`/`Uia` payloads (Phase
/// 6-7) can be added without reworking framing.
internal sealed record Envelope
{
    [System.Text.Json.Serialization.JsonPropertyName("version")]
    public required uint Version { get; init; }

    [System.Text.Json.Serialization.JsonPropertyName("req_id")]
    public required ulong ReqId { get; init; }

    [System.Text.Json.Serialization.JsonPropertyName("type")]
    public required MsgType Type { get; init; }

    [System.Text.Json.Serialization.JsonPropertyName("payload")]
    public object? Payload { get; init; }
}
