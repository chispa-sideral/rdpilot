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

using System.Runtime.InteropServices;
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

// ---------------------------------------------------------------------------
// Win32 EnumWindows handler (Task 2, this plan). All P/Invoke declarations
// use [LibraryImport] only (never the older attribute-based DllImport, 05-01
// discipline) against user32.dll — no COM, no managed WMI query API
// (RESEARCH Pitfall 2). Title/class-name buffers are fixed-size Span<char>
// (Pitfall 3: LibraryImport cannot marshal StringBuilder) bounded to
// TitleBufferLength UTF-16 units (Pitfall 4: truncation is benign, never
// grown unboundedly).
// ---------------------------------------------------------------------------

internal static partial class WindowEnumeration
{
    /// Bounded buffer size for GetWindowTextW/GetClassNameW (Pitfall 4) — a
    /// pathological window title cannot over-allocate or corrupt JSON
    /// framing; truncation (return value == buffer length) is treated as
    /// benign, never retried with a larger buffer.
    private const int TitleBufferLength = 512;

    [StructLayout(LayoutKind.Sequential)]
    private struct Rect32
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static unsafe partial bool EnumWindows(delegate* unmanaged<nint, nint, int> lpEnumFunc, nint lParam);

    [LibraryImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool GetWindowRect(nint hWnd, out Rect32 lpRect);

    [LibraryImport("user32.dll", EntryPoint = "GetWindowTextW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
    private static partial int GetWindowTextW(nint hWnd, Span<char> lpString, int nMaxCount);

    [LibraryImport("user32.dll", EntryPoint = "GetClassNameW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
    private static partial int GetClassNameW(nint hWnd, Span<char> lpClassName, int nMaxCount);

    [LibraryImport("user32.dll", SetLastError = true)]
    private static partial uint GetWindowThreadProcessId(nint hWnd, out uint lpdwProcessId);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool IsIconic(nint hWnd);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool IsZoomed(nint hWnd);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool IsWindowVisible(nint hWnd);

    /// AOT-safe EnumWindows callback (RESEARCH Pattern 4): a `static`
    /// `[UnmanagedCallersOnly]` method whose address is taken as a
    /// `delegate* unmanaged<...>` — the classic managed-delegate marshalling
    /// path (`Marshal.GetFunctionPointerForDelegate`) is unreliable under
    /// NativeAOT (dotnet/runtime#97952). Because the callback must be
    /// `static` with no closure state, the accumulator is threaded through
    /// `lParam` as a pinned `GCHandle` to a `List<nint>`, recovered here via
    /// `GCHandle.FromIntPtr`.
    [UnmanagedCallersOnly]
    private static int CollectWindowHandleCallback(nint hwnd, nint lParam)
    {
        GCHandle handle = GCHandle.FromIntPtr(lParam);
        if (handle.Target is List<nint> list)
        {
            list.Add(hwnd);
        }
        return 1; // non-zero: continue enumeration
    }

    private static unsafe List<nint> EnumerateTopLevelWindowHandles()
    {
        List<nint> windows = [];
        GCHandle handle = GCHandle.Alloc(windows);
        try
        {
            EnumWindows(&CollectWindowHandleCallback, GCHandle.ToIntPtr(handle));
        }
        finally
        {
            handle.Free();
        }
        return windows;
    }

    private static string ReadBoundedWindowText(nint hwnd)
    {
        Span<char> buffer = stackalloc char[TitleBufferLength];
        int length = GetWindowTextW(hwnd, buffer, buffer.Length);
        return length > 0 ? new string(buffer[..length]) : string.Empty;
    }

    private static string ReadBoundedClassName(nint hwnd)
    {
        Span<char> buffer = stackalloc char[TitleBufferLength];
        int length = GetClassNameW(hwnd, buffer, buffer.Length);
        return length > 0 ? new string(buffer[..length]) : string.Empty;
    }

    private static string ClassifyWindowState(nint hwnd)
    {
        // IsIconic/IsZoomed checked in this order because a window cannot
        // be both minimized and maximized simultaneously on Win32 — the
        // first match wins, falling through to "normal".
        if (IsIconic(hwnd))
        {
            return "minimized";
        }

        if (IsZoomed(hwnd))
        {
            return "maximized";
        }

        return "normal";
    }

    /// Enumerates top-level windows and builds the WindowList response
    /// (D-6.3 floor + class name / owning PID extras). Never throws — every
    /// per-window Win32 call is defensive (a window destroyed mid-
    /// enumeration is simply skipped) so a single misbehaving window cannot
    /// abort the whole response; Program.cs's caller wraps this in its own
    /// try/catch as a second line of defense (D-6.4, T-06-01).
    internal static WindowListResponse BuildWindowListResponse()
    {
        List<nint> hwnds = EnumerateTopLevelWindowHandles();
        List<WindowRecord> records = new(hwnds.Count);

        // EnumWindows already yields top-level windows in Z-order, front-to-
        // back (the documented Win32 contract) -- the enumeration index IS
        // the z-order, so no separate GetWindow(GW_HWNDPREV) walk is needed
        // (RESEARCH Task-2 action: "index in enumeration order ... or via
        // GetWindow(GW_HWNDPREV) walk" -- this is the simpler of the two
        // documented-equivalent options).
        for (int i = 0; i < hwnds.Count; i++)
        {
            nint hwnd = hwnds[i];

            // Simple documented filter (plan Task 2 action): skip invisible
            // top-level windows (owner/helper/message-only windows). Every
            // other visible window is kept, even one with an empty title --
            // that is itself observable state a caller may care about.
            if (!IsWindowVisible(hwnd))
            {
                continue;
            }

            if (!GetWindowRect(hwnd, out Rect32 rect))
            {
                // Destroyed between EnumWindows and here -- skip, don't fail
                // the whole response over one stale handle.
                continue;
            }

            GetWindowThreadProcessId(hwnd, out uint pid);

            records.Add(new WindowRecord
            {
                Hwnd = unchecked((ulong)(long)hwnd),
                Title = ReadBoundedWindowText(hwnd),
                Rect = new WindowRect
                {
                    // Physical virtual-desktop pixels, no DPI/logical
                    // scaling (D-6.3). KNOWN LIMITATION: a window on a
                    // monitor to the left of / above the primary monitor in
                    // a multi-monitor layout can have a negative native
                    // Left/Top; the wire contract's x/y fields are u32
                    // (06-01-PLAN.md, locked), so such coordinates clamp to
                    // 0 here rather than wrapping -- out of this plan's
                    // scope to change the wire contract.
                    X = (uint)Math.Max(0, rect.Left),
                    Y = (uint)Math.Max(0, rect.Top),
                    W = (uint)Math.Max(0, rect.Right - rect.Left),
                    H = (uint)Math.Max(0, rect.Bottom - rect.Top),
                },
                ZOrder = (uint)i,
                State = ClassifyWindowState(hwnd),
                ClassName = ReadBoundedClassName(hwnd),
                Pid = pid,
            });
        }

        return new WindowListResponse { Success = true, Data = [.. records], Error = null };
    }
}
