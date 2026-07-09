// LaunchProcess (fire-and-forget) DTOs + handler for the RDPILOT_SENSOR
// wire protocol — the Phase 6 wire schema is defined authoritatively in
// 06-01-PLAN.md's <wire_contract>:
//   request payload:  {"exe": <string>, "args": <string|null>, "cwd": <string|null>}
//   response payload: {"success":true,"data":{"pid":<u32>}} or {"success":false,"error":"..."}
//
// D-6.2 (fire-and-forget): replies with the new process's PID the instant
// `CreateProcessW` returns — never waits/polls for the process to reach any
// particular state. Pitfall 6: `CreateProcessW` always returns OPEN handles
// to the new process/thread that the caller owns; both `hProcess` and
// `hThread` are closed immediately after reading `dwProcessId`, reusing the
// shared `ProcessEnumeration.CloseHandle` P/Invoke declaration (this plan's
// Task 1) rather than redeclaring an equivalent signature against the same
// kernel32.dll export.

using System.Runtime.InteropServices;
using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// The LaunchProcess request payload: `{"exe": <string>, "args":
/// <string|null>, "cwd": <string|null>}`.
internal sealed record LaunchProcessRequest
{
    [JsonPropertyName("exe")]
    public required string Exe { get; init; }

    [JsonPropertyName("args")]
    public string? Args { get; init; }

    [JsonPropertyName("cwd")]
    public string? Cwd { get; init; }
}

/// The `data` shape of a successful LaunchProcess reply: `{"pid":<u32>}`.
internal sealed record LaunchProcessData
{
    [JsonPropertyName("pid")]
    public required uint Pid { get; init; }
}

/// The LaunchProcess response payload (D-6.4 success/failure envelope
/// shape).
internal sealed record LaunchProcessResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public LaunchProcessData? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }
}

// ---------------------------------------------------------------------------
// Win32 process launch. [LibraryImport] only (05-01 discipline) against
// kernel32.dll — no COM. `CreateProcessW`'s `lpCommandLine` is a MUTABLE
// buffer per the Win32 contract (the OS may rewrite it in place) — built
// here from a `char[]`, never a read-only/interned `string`.
// ---------------------------------------------------------------------------

internal static partial class ProcessLaunch
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct STARTUPINFOW
    {
        public uint cb;
        public nint lpReserved;
        public nint lpDesktop;
        public nint lpTitle;
        public uint dwX;
        public uint dwY;
        public uint dwXSize;
        public uint dwYSize;
        public uint dwXCountChars;
        public uint dwYCountChars;
        public uint dwFillAttribute;
        public uint dwFlags;
        public ushort wShowWindow;
        public ushort cbReserved2;
        public nint lpReserved2;
        public nint hStdInput;
        public nint hStdOutput;
        public nint hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct PROCESS_INFORMATION
    {
        public nint hProcess;
        public nint hThread;
        public uint dwProcessId;
        public uint dwThreadId;
    }

    // Source: standard Win32 CreateProcessW signature
    // (learn.microsoft.com/windows/win32/api/processthreadsapi), RESEARCH
    // Code Examples — [ASSUMED], stable API, low risk. STARTUPINFOW/
    // PROCESS_INFORMATION are blittable structs (all-primitive/nint fields,
    // same "no MarshalAs on an embedded field" discipline this plan's Task 1
    // established for PROCESSENTRY32W), safe for [LibraryImport].
    [LibraryImport("kernel32.dll", EntryPoint = "CreateProcessW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool CreateProcessW(
        string? lpApplicationName,
        ref char lpCommandLine,
        nint lpProcessAttributes,
        nint lpThreadAttributes,
        [MarshalAs(UnmanagedType.Bool)] bool bInheritHandles,
        uint dwCreationFlags,
        nint lpEnvironment,
        string? lpCurrentDirectory,
        ref STARTUPINFOW lpStartupInfo,
        out PROCESS_INFORMATION lpProcessInformation);

    /// Fire-and-forget launch (D-6.2): calls `CreateProcessW` and replies
    /// with the PID the instant it returns — never waits/polls for the
    /// process to initialize. Closes BOTH `hProcess` and `hThread`
    /// immediately after reading `dwProcessId` (Pitfall 6).
    internal static LaunchProcessResponse Launch(LaunchProcessRequest request)
    {
        string commandLine = string.IsNullOrEmpty(request.Args)
            ? request.Exe
            : $"{request.Exe} {request.Args}";

        // A mutable, null-terminated buffer — CreateProcessW's Win32
        // contract permits it to rewrite the command line in place; never
        // pass a read-only/interned managed string directly.
        char[] commandLineBuffer = new char[commandLine.Length + 1];
        commandLine.CopyTo(0, commandLineBuffer, 0, commandLine.Length);
        commandLineBuffer[commandLine.Length] = '\0';

        STARTUPINFOW startupInfo = default;
        startupInfo.cb = (uint)Marshal.SizeOf<STARTUPINFOW>();

        bool ok = CreateProcessW(
            null,
            ref commandLineBuffer[0],
            0,
            0,
            false,
            0,
            0,
            request.Cwd,
            ref startupInfo,
            out PROCESS_INFORMATION processInfo);

        if (!ok)
        {
            int lastError = Marshal.GetLastPInvokeError();
            return new LaunchProcessResponse
            {
                Success = false,
                Data = null,
                Error = $"CreateProcessW failed (LastError={lastError})",
            };
        }

        // The instant we have the PID, close BOTH handles (Pitfall 6) — the
        // SDK only needs the PID, not a live handle; never wait/poll for the
        // process to reach any particular state (D-6.2).
        uint pid = processInfo.dwProcessId;
        ProcessEnumeration.CloseHandle(processInfo.hProcess);
        ProcessEnumeration.CloseHandle(processInfo.hThread);

        return new LaunchProcessResponse
        {
            Success = true,
            Data = new LaunchProcessData { Pid = pid },
            Error = null,
        };
    }
}
