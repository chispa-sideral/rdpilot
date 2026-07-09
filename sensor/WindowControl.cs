// SetForegroundWindow (focus) DTOs + handler for the RDPILOT_SENSOR wire
// protocol — the Phase 6 wire schema is defined authoritatively in
// 06-01-PLAN.md's <wire_contract>:
//   request payload:  {"hwnd": <u64>}
//   response payload: {"success":true,"data":null} or {"success":false,"error":"..."}
//
// Pitfall 5 (RESEARCH): Windows' foreground-lock-timeout mechanism means a
// SetForegroundWindow call can return TRUE while the window is merely
// flashed in the taskbar rather than actually brought to the front. This
// handler reports success ONLY on the Win32 call's own nonzero return value
// — it does NOT itself attempt to verify the visual outcome. SC#3's actual
// focus confirmation is the caller's own follow-up get_window_list query
// (ROADMAP Phase 6 SC#3, session.rs::set_foreground_window doc comment).

using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// The SetForegroundWindow request payload: `{"hwnd": <u64>}`.
internal sealed record SetForegroundWindowRequest
{
    [JsonPropertyName("hwnd")]
    public required ulong Hwnd { get; init; }
}

/// The SetForegroundWindow response payload (D-6.4 success/failure envelope
/// shape). `Data` is always `null` — the wire contract's `data` shape for
/// this request is `null` (the success flag alone is the result). Typed as
/// `JsonElement?` (not `object?`, RESEARCH Pitfall 1 / Pattern 6) so the
/// AOT source generator can serialize the property without a separate
/// per-type registration — `JsonElement` is a built-in DOM type the
/// generator already supports natively (proven by `Envelope.Payload` itself
/// using the identical pattern).
internal sealed record SetForegroundWindowResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public JsonElement? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }
}

// ---------------------------------------------------------------------------
// Win32 focus control. [LibraryImport] only (05-01 discipline) against
// user32.dll — no COM.
// ---------------------------------------------------------------------------

internal static partial class WindowControl
{
    // Source: standard Win32 SetForegroundWindow signature
    // (learn.microsoft.com/windows/win32/api/winuser), RESEARCH Code
    // Examples — [ASSUMED], stable API since Windows 2000, low risk.
    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetForegroundWindow(nint hWnd);

    /// Calls `SetForegroundWindow` and reports `success:true` ONLY if the
    /// Win32 call itself returned nonzero (Pitfall 5 — call-success, not
    /// visual outcome). On a zero return, replies `success:false` carrying
    /// the last Win32 error for diagnostics.
    internal static SetForegroundWindowResponse Focus(SetForegroundWindowRequest request)
    {
        bool ok = SetForegroundWindow((nint)request.Hwnd);
        if (ok)
        {
            return new SetForegroundWindowResponse { Success = true, Data = null, Error = null };
        }

        int lastError = Marshal.GetLastPInvokeError();
        return new SetForegroundWindowResponse
        {
            Success = false,
            Data = null,
            Error = $"SetForegroundWindow returned FALSE (LastError={lastError})",
        };
    }
}
