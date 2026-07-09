// The `RDPILOT_SENSOR` envelope — the SERVER side of the fixed wire protocol
// (D-4.3, D-5.7, D-6.4). Mirrors crates/rdpilot/src/sensor.rs's
// `Envelope`/`MsgType` byte-for-byte: `{ version, req_id, type, payload }`,
// with `type` serialized as the bare externally-tagged unit-string form
// ("Version"/"Ping"/"Pong"/"WindowList"/...).
//
// Phase 6 (06-01-PLAN.md's <wire_contract>, authoritative) adds WindowList/
// ProcessTree/SetForegroundWindow/LaunchProcess request/response payloads on
// top of the Phase 5 Version/Ping/Pong handshake+heartbeat.

namespace RdpilotSensor;

/// The SDK's own protocol version for the RDPILOT_SENSOR envelope. Matches
/// `crate::sensor::PROTOCOL_VERSION` on the Rust side.
internal static class ProtocolVersion
{
    internal const uint Value = 1;
}

/// The set of message types (D-4.3/D-5.4 handshake+heartbeat, Phase 6
/// perception/control requests). `System.Text.Json` writes this as the bare
/// string form ("Version"/"Ping"/"Pong"/"WindowList"/...) via
/// `JsonStringEnumConverter` wired through `EnvelopeJsonContext` — matching
/// the Rust side's serde externally-tagged unit-variant wire shape exactly.
/// Order/names mirror `crates/rdpilot/src/sensor.rs`'s `MsgType` (06-01).
internal enum MsgType
{
    Version,
    Ping,
    Pong,
    WindowList,
    ProcessTree,
    SetForegroundWindow,
    LaunchProcess,
    Uia,
}

/// The durable request/response envelope carried over `RDPILOT_SENSOR`:
/// `{ version, req_id, type, payload }` (D-4.3). Field names match the Rust
/// `serde` wire names exactly (snake_case `req_id`, bare `type`, `payload`,
/// `version`) via `[JsonPropertyName]` — this record's own C# property names
/// are PascalCase by convention, the wire name is pinned per-property so the
/// JSON byte shape is identical to what `sensor.rs`'s tests assert.
///
/// `req_id` is a correlation id (a request's reply echoes the same `req_id`
/// and `type`, D-6.4). `Payload` is `System.Text.Json.JsonElement?` — NOT
/// `object?` (RESEARCH Pitfall 1 / Pattern 6, 06-03-PLAN Task 1): under
/// NativeAOT with `JsonSerializerIsReflectionEnabledByDefault=false`, an
/// `object?`-typed property silently defeats the source generator the
/// instant a real payload is attached (the generator cannot know the
/// concrete runtime type ahead of time). `JsonElement?` is natively
/// DOM-typed by the source generator with no reflection fallback: callers
/// attach a concrete payload via
/// `JsonSerializer.SerializeToElement(dto, EnvelopeJsonContext.Default.SomeDto)`
/// and read one back via
/// `envelope.Payload?.Deserialize(EnvelopeJsonContext.Default.SomeDto)`.
internal sealed record Envelope
{
    [System.Text.Json.Serialization.JsonPropertyName("version")]
    public required uint Version { get; init; }

    [System.Text.Json.Serialization.JsonPropertyName("req_id")]
    public required ulong ReqId { get; init; }

    [System.Text.Json.Serialization.JsonPropertyName("type")]
    public required MsgType Type { get; init; }

    [System.Text.Json.Serialization.JsonPropertyName("payload")]
    public System.Text.Json.JsonElement? Payload { get; init; }
}
