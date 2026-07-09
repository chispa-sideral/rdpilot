// WindowList DTOs for the RDPILOT_SENSOR wire protocol — the Phase 6 wire
// schema is defined authoritatively in 06-01-PLAN.md's <wire_contract> and
// mirrored here field-for-field (snake_case, via [JsonPropertyName]) to
// match the Rust side's `WindowInfoWire`/`RectWire`/`WindowStateWire`
// (crates/rdpilot/src/perception.rs, 06-01).
//
// `rect` fields are PHYSICAL virtual-desktop pixels (the framebuffer
// coordinate space) — never DPI-scaled logical coordinates (D-6.3,
// 06-CONTEXT Implementation notes; a Phase 7 criterion depends on this).
//
// This file's Task-1 contents (this plan, 06-03) are the DTOs alone,
// deliberately defined and registered in EnvelopeJsonContext BEFORE any
// Win32 P/Invoke code is written — proving the JsonElement-typed
// Envelope.Payload round-trip (RESEARCH Pitfall 1) is the plan's front-
// loaded risk gate. The EnumWindows-based handler that populates these
// DTOs is added in this same plan's Task 2.

using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// A window's bounding rectangle in physical virtual-desktop pixels.
internal sealed record WindowRect
{
    [JsonPropertyName("x")]
    public required uint X { get; init; }

    [JsonPropertyName("y")]
    public required uint Y { get; init; }

    [JsonPropertyName("w")]
    public required uint W { get; init; }

    [JsonPropertyName("h")]
    public required uint H { get; init; }
}

/// One top-level window record (D-6.3 floor + class name / owning PID
/// extras). `State` is the bare lowercase string "normal"/"minimized"/
/// "maximized" per the wire contract (matching the Rust `WindowState`
/// serde rename, NOT a JSON enum converter — kept as a plain `string` here
/// so this DTO stays a pure data record; the classification logic lives in
/// the Task-2 handler).
internal sealed record WindowRecord
{
    [JsonPropertyName("hwnd")]
    public required ulong Hwnd { get; init; }

    [JsonPropertyName("title")]
    public required string Title { get; init; }

    [JsonPropertyName("rect")]
    public required WindowRect Rect { get; init; }

    [JsonPropertyName("z_order")]
    public required uint ZOrder { get; init; }

    [JsonPropertyName("state")]
    public required string State { get; init; }

    [JsonPropertyName("class_name")]
    public required string ClassName { get; init; }

    [JsonPropertyName("pid")]
    public required uint Pid { get; init; }
}

/// The WindowList response payload (D-6.4 success/failure envelope shape):
/// `{"success":true,"data":[...]}` or `{"success":false,"error":"..."}`.
internal sealed record WindowListResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public WindowRecord[]? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }
}
