// AOT-safe JSON source generator for the RDPILOT_SENSOR envelope (SENSOR-01,
// D-5.3). Native AOT cannot generate reflection-based `JsonSerializer`
// metadata at runtime, so all (de)serialization in Program.cs MUST go
// through `EnvelopeJsonContext.Default.Envelope` — never the reflection-based
// `JsonSerializer.Serialize<Envelope>(...)` overload (RESEARCH Anti-Patterns).
//
// `UseStringEnumConverter = true` makes the source generator emit `MsgType`
// as the bare string form ("Version"/"Ping"/"Pong") matching the Rust side's
// serde externally-tagged unit-variant wire shape — without this the enum
// would serialize as an integer and break wire compatibility with sensor.rs.

using System.Text.Json.Serialization;

namespace RdpilotSensor;

[JsonSourceGenerationOptions(UseStringEnumConverter = true)]
[JsonSerializable(typeof(Envelope))]
internal partial class EnvelopeJsonContext : JsonSerializerContext
{
}
