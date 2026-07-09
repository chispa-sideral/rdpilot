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
// user32.dll/kernel32.dll — no COM.
// ---------------------------------------------------------------------------

internal static partial class WindowControl
{
    private const int SwRestore = 9;

    // Source: standard Win32 SetForegroundWindow signature
    // (learn.microsoft.com/windows/win32/api/winuser), RESEARCH Code
    // Examples — [ASSUMED], stable API since Windows 2000, low risk.
    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool SetForegroundWindow(nint hWnd);

    [LibraryImport("user32.dll")]
    private static partial nint GetForegroundWindow();

    [LibraryImport("user32.dll")]
    private static partial uint GetWindowThreadProcessId(nint hWnd, nint lpdwProcessId);

    [LibraryImport("kernel32.dll")]
    private static partial uint GetCurrentThreadId();

    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool AttachThreadInput(uint idAttach, uint idAttachTo, [MarshalAs(UnmanagedType.Bool)] bool fAttach);

    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool BringWindowToTop(nint hWnd);

    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool ShowWindow(nint hWnd, int nCmdShow);

    /// Calls `SetForegroundWindow` and reports `success:true` ONLY if the
    /// Win32 call itself returned nonzero (Pitfall 5 — call-success, not
    /// visual outcome). On a zero return, replies `success:false` carrying
    /// the last Win32 error for diagnostics.
    ///
    /// LIVE-DIAGNOSED BUG FIX (06-05 live gate): a bare `SetForegroundWindow`
    /// call from this background sensor process — which owns no window and
    /// has received no recent user input of its own — is subject to
    /// Windows' foreground-lock-timeout restriction (Pitfall 5's documented
    /// risk, now confirmed live): the call can return TRUE while the target
    /// window's Z-order never actually changes (confirmed live: 10 poll
    /// attempts, target hwnd's z_order never moved off a fixed position
    /// several windows below topmost). The standard, well-established
    /// workaround is to temporarily attach this thread's input queue to the
    /// CURRENT foreground window's input queue via `AttachThreadInput`
    /// before calling `SetForegroundWindow` — input-queue attachment is one
    /// of the conditions Windows accepts as proof of "the calling thread is
    /// allowed to set the foreground window" (see Raymond Chen's
    /// documented `SetForegroundWindow` rules). `BringWindowToTop` +
    /// `ShowWindow(SW_RESTORE)` are added alongside as belt-and-suspenders
    /// for a minimized target. Threads are always detached again in a
    /// `finally`, even on failure, so no lingering input-queue attachment
    /// survives this call.
    internal static SetForegroundWindowResponse Focus(SetForegroundWindowRequest request)
    {
        nint hWnd = (nint)request.Hwnd;
        nint foreground = GetForegroundWindow();
        uint foregroundThread = GetWindowThreadProcessId(foreground, nint.Zero);
        uint targetThread = GetWindowThreadProcessId(hWnd, nint.Zero);
        uint currentThread = GetCurrentThreadId();

        bool attachedToForeground = foregroundThread != 0
            && foregroundThread != currentThread
            && AttachThreadInput(currentThread, foregroundThread, true);
        bool attachedToTarget = targetThread != 0
            && targetThread != currentThread
            && targetThread != foregroundThread
            && AttachThreadInput(currentThread, targetThread, true);

        bool ok;
        int lastError = 0;
        try
        {
            ShowWindow(hWnd, SwRestore);
            BringWindowToTop(hWnd);
            ok = SetForegroundWindow(hWnd);
            if (!ok)
            {
                lastError = Marshal.GetLastPInvokeError();
            }
        }
        finally
        {
            if (attachedToTarget)
            {
                AttachThreadInput(currentThread, targetThread, false);
            }
            if (attachedToForeground)
            {
                AttachThreadInput(currentThread, foregroundThread, false);
            }
        }

        if (ok)
        {
            return new SetForegroundWindowResponse { Success = true, Data = null, Error = null };
        }

        return new SetForegroundWindowResponse
        {
            Success = false,
            Data = null,
            Error = $"SetForegroundWindow returned FALSE (LastError={lastError})",
        };
    }
}
