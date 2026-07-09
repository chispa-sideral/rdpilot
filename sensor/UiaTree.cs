// UiaTree DTOs for the RDPILOT_SENSOR wire protocol (PERC-03, 07-04-PLAN.md
// Task 1). Wire schema is authoritatively defined by 07-01's Rust-side
// `UiaElementWire` (crates/rdpilot/src/perception.rs) -- the JSON keys below
// MUST match it EXACTLY (snake_case via [JsonPropertyName]):
//   request payload:  {"hwnd": <u64>}
//   response payload: {"success":true,"data":[UiaElementRecord,...]} or
//                      {"success":false,"error":"..."}
//
// D-7.2/D-7.3: the RuntimeId-join (`id`) and ControlType->role map (`role`)
// are Rust-side (07-01) responsibilities -- this sensor ships RAW
// `runtime_id`/`parent_runtime_id` int arrays and a RAW `control_type` int,
// never a pre-joined/pre-mapped string.
//
// `bbox` reuses `WindowRect` (WindowEnumeration.cs) verbatim -- same
// `x/y/w/h` physical virtual-desktop pixel shape, already registered for
// source-generated JSON, no need for a second bbox type (07-04-PLAN.md
// Task 1's reuse-vs-new choice, executor's call: reuse).
//
// The `UiaTree.BuildUiaTreeResponse` handler that populates these DTOs is
// added in this same plan's Task 2.

using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// The Uia request payload: `{"hwnd": <u64>}`.
internal sealed record UiaTreeRequest
{
    [JsonPropertyName("hwnd")]
    public required ulong Hwnd { get; init; }
}

/// One flat UIA element record (D-7.1's field set, minus `value`/`Pattern`
/// which is explicitly DEFERRED to backlog). `RuntimeId`/`ParentRuntimeId`
/// are RAW int arrays and `ControlType` is a RAW int -- the D-7.2 join and
/// D-7.3 map are performed Rust-side (07-01), NOT here.
internal sealed record UiaElementRecord
{
    [JsonPropertyName("runtime_id")]
    public required int[] RuntimeId { get; init; }

    [JsonPropertyName("control_type")]
    public required int ControlType { get; init; }

    [JsonPropertyName("name")]
    public required string Name { get; init; }

    [JsonPropertyName("bbox")]
    public required WindowRect Bbox { get; init; }

    [JsonPropertyName("enabled")]
    public required bool Enabled { get; init; }

    [JsonPropertyName("visible")]
    public required bool Visible { get; init; }

    [JsonPropertyName("focusable")]
    public required bool Focusable { get; init; }

    [JsonPropertyName("focused")]
    public required bool Focused { get; init; }

    [JsonPropertyName("depth")]
    public required uint Depth { get; init; }

    [JsonPropertyName("parent_runtime_id")]
    public required int[] ParentRuntimeId { get; init; }
}

/// The Uia response payload (D-6.4 success/failure envelope shape):
/// `{"success":true,"data":[...]}` or `{"success":false,"error":"..."}`.
internal sealed record UiaTreeResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public UiaElementRecord[]? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }
}
