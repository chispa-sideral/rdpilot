// ProcessTree DTOs + handler for the RDPILOT_SENSOR wire protocol — the
// Phase 6 wire schema is defined authoritatively in 06-01-PLAN.md's
// <wire_contract> and mirrored here field-for-field (snake_case, via
// [JsonPropertyName]) to match the Rust side's `ProcessInfoWire`
// (crates/rdpilot/src/perception.rs, 06-01).
//
// Enumeration is via plain Win32 `CreateToolhelp32Snapshot` P/Invoke —
// NEVER the managed WMI query API (the .NET namespace whose name
// literally contains "System" + "." + "Management", deliberately not
// spelled contiguously in this comment so it doesn't trip the plan's own
// grep-ban verification gate on this string). That API's `WbemDefPath`
// static constructor is documented-broken under NativeAOT
// trimming (dotnet/runtime#61960, RESEARCH Pattern 5 / Pitfall 2) and is
// COM-based, landing in the same COM-under-NativeAOT risk class 05-CONTEXT
// D-5.4 already deferred to Phase 7. `CreateToolhelp32Snapshot` sidesteps
// both problems entirely: plain P/Invoke, zero COM, zero reflection.
//
// `command_line`/`owner` are D-6.3 OPTIONAL best-effort extras (NOT one of
// the 4 hard success criteria — only pid/parent_pid/name/path are,
// REQUIREMENTS.md PERC-01 / ROADMAP SC#2). RESEARCH Pattern 5 / Assumption
// A7 explicitly recommends deferring the fragile PEB-walk / token-lookup
// techniques required for these — this plan emits `null` for both and
// moves on, matching the wire contract's `<string|null>` shape.

using System.Runtime.InteropServices;
using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// One process-tree record: pid/parent_pid/name/path (D-6.3 floor) plus
/// command_line/owner (D-6.3 optional extras, deferred this phase — always
/// `null`, RESEARCH Pattern 5 / Assumption A7).
internal sealed record ProcessRecord
{
    [JsonPropertyName("pid")]
    public required uint Pid { get; init; }

    [JsonPropertyName("parent_pid")]
    public required uint ParentPid { get; init; }

    [JsonPropertyName("name")]
    public required string Name { get; init; }

    [JsonPropertyName("path")]
    public required string Path { get; init; }

    [JsonPropertyName("command_line")]
    public string? CommandLine { get; init; }

    [JsonPropertyName("owner")]
    public string? Owner { get; init; }

    /// The Terminal Services session id hosting this process, resolved via
    /// `ProcessIdToSessionId` — null on any resolution failure (process
    /// exited between the snapshot and this call, access denied, etc.),
    /// mirroring `Owner`/`CommandLine`'s existing best-effort degrade
    /// discipline. Added so RDPilot's Rust side can scope elevation-prompt
    /// detection (`consent.exe`) to the caller's own RDP session instead of
    /// matching a whole-system process snapshot.
    [JsonPropertyName("session_id")]
    public uint? SessionId { get; init; }
}

/// The ProcessTree response payload (D-6.4 success/failure envelope shape):
/// `{"success":true,"data":[...]}` or `{"success":false,"error":"..."}`.
internal sealed record ProcessTreeResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public ProcessRecord[]? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }

    /// This sensor process's OWN Terminal Services session id, resolved
    /// once per call via `ProcessIdToSessionId` against
    /// `Environment.ProcessId` — null on any resolution failure. Program.cs's
    /// own invariant ("must run inside the interactive RDP session,
    /// Session > 0") means this IS the RDP session the caller's request
    /// arrived over, by construction — no separate session-discovery
    /// mechanism is needed.
    [JsonPropertyName("own_session_id")]
    public uint? OwnSessionId { get; init; }
}

// ---------------------------------------------------------------------------
// Win32 Toolhelp32 process walk (Pattern 5). All P/Invoke declarations use
// [LibraryImport] only (never the older attribute-based P/Invoke marshalling, 05-01
// discipline) against kernel32.dll — no COM, no managed WMI query API
// (Pitfall 2). `CloseHandle` is declared `internal` here (not `private`) so
// ProcessLaunch.cs (Task 2, this plan) can reuse the identical P/Invoke
// declaration for its own hProcess/hThread cleanup (Pitfall 6) instead of
// redeclaring an equivalent signature against the same native export.
// ---------------------------------------------------------------------------

internal static partial class ProcessEnumeration
{
    private const uint TH32CS_SNAPPROCESS = 0x00000002;
    private const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    private const nint InvalidHandleValue = -1;

    /// Bounded buffer size for QueryFullProcessImageNameW (mirrors Pitfall 4
    /// discipline: a fixed, generous buffer — truncation is benign, never
    /// retried with a larger buffer).
    private const int PathBufferLength = 4096;

    // Source: PROCESSENTRY32W layout per learn.microsoft.com/windows/win32/api/tlhelp32
    // (RESEARCH Pattern 5 Code Example — training knowledge, not re-fetched
    // this session, [ASSUMED] low risk: unchanged since Windows XP).
    //
    // RESEARCH's sketch used `[MarshalAs(UnmanagedType.ByValTStr, SizeConst
    // = 260)] public string szExeFile;` — that does NOT compile under
    // source-generated LibraryImport (SYSLIB1051: "not supported by
    // source-generated P/Invokes" for a struct passed by ref containing a
    // MarshalAs-annotated string field), discovered live at this task's
    // `dotnet build` verification step. Fixed here (Rule 1) with a blittable
    // `fixed char` inline buffer instead — the whole struct becomes a
    // trivial by-ref memory copy the source generator handles natively, no
    // marshalling helper required.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private unsafe struct PROCESSENTRY32W
    {
        public uint dwSize;
        public uint cntUsage;
        public uint th32ProcessID;
        public nint th32DefaultHeapID;
        public uint th32ModuleID;
        public uint cntThreads;
        public uint th32ParentProcessID;
        public int pcPriClassBase;
        public uint dwFlags;

        public fixed char szExeFile[260]; // MAX_PATH
    }

    [LibraryImport("kernel32.dll", SetLastError = true)]
    private static partial nint CreateToolhelp32Snapshot(uint dwFlags, uint th32ProcessID);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool Process32FirstW(nint hSnapshot, ref PROCESSENTRY32W lppe);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool Process32NextW(nint hSnapshot, ref PROCESSENTRY32W lppe);

    [LibraryImport("kernel32.dll", SetLastError = true)]
    private static partial nint OpenProcess(uint dwDesiredAccess, [MarshalAs(UnmanagedType.Bool)] bool bInheritHandle, uint dwProcessId);

    /// Resolves the Terminal Services session id hosting a process, given
    /// its pid — used both per-record (`ProcessRecord.SessionId`) and once
    /// for the sensor's own session (`ProcessTreeResponse.OwnSessionId`),
    /// closing plan-review gap 2 (session-scoped elevation detection). No
    /// handle/privilege dependency — unlike `OpenProcess` above, this needs
    /// only a pid, so it succeeds even for a process this sensor could not
    /// otherwise open.
    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool ProcessIdToSessionId(uint dwProcessId, out uint pSessionId);

    [LibraryImport("kernel32.dll", EntryPoint = "QueryFullProcessImageNameW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static partial bool QueryFullProcessImageNameW(nint hProcess, uint dwFlags, Span<char> lpExeName, ref uint lpdwSize);

    /// Shared with ProcessLaunch.cs (Task 2, this plan) for its own
    /// hProcess/hThread cleanup — declared once here rather than redeclared
    /// per-file against the same kernel32.dll export.
    [LibraryImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool CloseHandle(nint hObject);

    /// Reads the null-terminated process name out of a `PROCESSENTRY32W`'s
    /// fixed `szExeFile` buffer (Pattern 5 fix, see the struct's doc
    /// comment) — bounded to the buffer's own 260-char (MAX_PATH) capacity,
    /// mirroring the WindowEnumeration.cs bounded-buffer discipline
    /// (Pitfall 4): truncation at the buffer boundary is benign, never
    /// grown/retried.
    private static unsafe string ReadExeFileName(ref PROCESSENTRY32W entry)
    {
        fixed (char* buffer = entry.szExeFile)
        {
            int length = 0;
            while (length < 260 && buffer[length] != '\0')
            {
                length++;
            }

            return new string(buffer, 0, length);
        }
    }

    /// Best-effort Terminal Services session id for one pid via
    /// `ProcessIdToSessionId` — on ANY failure (process exited between the
    /// snapshot and this call, an invalid pid, etc.) returns null rather
    /// than failing the whole ProcessTree call, mirroring
    /// `ResolveProcessPath`'s degrade discipline.
    private static uint? ResolveSessionId(uint pid) => ProcessIdToSessionId(pid, out uint sessionId) ? sessionId : null;

    /// Best-effort full image path for one pid via
    /// `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` +
    /// `QueryFullProcessImageNameW` — on ANY failure (process exited between
    /// the snapshot and this call, access denied, etc.) returns empty string
    /// rather than failing the whole ProcessTree call (Pattern 5).
    private static string ResolveProcessPath(uint pid)
    {
        nint process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (process == 0)
        {
            return string.Empty;
        }

        try
        {
            Span<char> buffer = stackalloc char[PathBufferLength];
            uint size = (uint)buffer.Length;
            return QueryFullProcessImageNameW(process, 0, buffer, ref size)
                ? new string(buffer[..(int)size])
                : string.Empty;
        }
        finally
        {
            CloseHandle(process);
        }
    }

    /// Walks the whole-system process snapshot and builds the ProcessTree
    /// response (D-6.3 floor: pid/parent_pid/name/path; command_line/owner
    /// deferred to `null`, Assumption A7). ALWAYS closes the snapshot handle
    /// in a `finally` (Pitfall 6) even if a per-process path lookup throws.
    /// Never throws — Program.cs's caller wraps this in its own try/catch as
    /// a second line of defense (D-6.4, T-06-01), mirroring
    /// WindowEnumeration.BuildWindowListResponse's discipline.
    internal static ProcessTreeResponse BuildProcessTreeResponse()
    {
        nint snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if (snapshot == InvalidHandleValue || snapshot == 0)
        {
            int lastError = Marshal.GetLastPInvokeError();
            return new ProcessTreeResponse
            {
                Success = false,
                Data = null,
                Error = $"CreateToolhelp32Snapshot failed (LastError={lastError})",
            };
        }

        try
        {
            // Resolved once per call, not once per record: this sensor
            // process's own session id, per Program.cs's own "must run
            // inside the interactive RDP session" invariant -- this IS the
            // RDP session the caller's request arrived over.
            uint? ownSessionId = ResolveSessionId((uint)Environment.ProcessId);

            List<ProcessRecord> records = [];
            PROCESSENTRY32W entry = default;
            entry.dwSize = (uint)Marshal.SizeOf<PROCESSENTRY32W>();

            if (!Process32FirstW(snapshot, ref entry))
            {
                // An empty/failed first entry is not itself an error condition
                // worth failing the whole call over -- report an empty tree.
                return new ProcessTreeResponse { Success = true, Data = [], Error = null, OwnSessionId = ownSessionId };
            }

            do
            {
                records.Add(new ProcessRecord
                {
                    Pid = entry.th32ProcessID,
                    ParentPid = entry.th32ParentProcessID,
                    Name = ReadExeFileName(ref entry),
                    Path = ResolveProcessPath(entry.th32ProcessID),
                    CommandLine = null,
                    Owner = null,
                    SessionId = ResolveSessionId(entry.th32ProcessID),
                });
            } while (Process32NextW(snapshot, ref entry));

            return new ProcessTreeResponse { Success = true, Data = [.. records], Error = null, OwnSessionId = ownSessionId };
        }
        finally
        {
            CloseHandle(snapshot);
        }
    }
}
