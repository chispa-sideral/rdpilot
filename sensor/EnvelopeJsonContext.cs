// AOT-safe JSON source generator for the RDPILOT_SENSOR envelope (SENSOR-01,
// D-5.3). Native AOT cannot generate reflection-based `JsonSerializer`
// metadata at runtime, so all (de)serialization in Program.cs MUST go
// through `EnvelopeJsonContext.Default.<Type>` — never the reflection-based
// `JsonSerializer.Serialize<T>(...)` overload (RESEARCH Anti-Patterns).
//
// `UseStringEnumConverter = true` makes the source generator emit `MsgType`
// as the bare string form ("Version"/"Ping"/"Pong"/"WindowList"/...) matching
// the Rust side's serde externally-tagged unit-variant wire shape — without
// this the enum would serialize as an integer and break wire compatibility
// with sensor.rs.
//
// Every payload DTO carried inside `Envelope.Payload` (now `JsonElement?`,
// RESEARCH Pitfall 1 / Pattern 6, 06-03-PLAN Task 1) MUST be registered here
// with its own `[JsonSerializable(typeof(...))]` entry — there is no
// reflection fallback under NativeAOT (JsonSerializerIsReflectionEnabledByDefault=false).

using System.Text.Json.Serialization;

namespace RdpilotSensor;

[JsonSourceGenerationOptions(UseStringEnumConverter = true)]
[JsonSerializable(typeof(Envelope))]
[JsonSerializable(typeof(WindowRect))]
[JsonSerializable(typeof(WindowRecord))]
[JsonSerializable(typeof(WindowListResponse))]
internal partial class EnvelopeJsonContext : JsonSerializerContext
{
}
