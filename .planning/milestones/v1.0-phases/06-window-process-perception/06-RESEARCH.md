# Phase 6: Window + Process Perception - Research

**Researched:** 2026-07-09
**Domain:** RDP DVC request/response protocol generalization (Rust client) + Win32 window/process enumeration and control (C# NativeAOT sensor, no COM)
**Confidence:** MEDIUM-HIGH — the Rust-side protocol generalization is HIGH confidence (grounded directly in this repo's existing code); the C# Win32 P/Invoke specifics are MEDIUM confidence (well-established APIs, but not verified against a live NativeAOT build this session — several are flagged for Wave-0 smoke verification)

## Summary

Phase 6 is the first phase that moves real structured data through the `RDPILOT_SENSOR` request/response envelope — every prior phase (4, 5) only exercised the fixed `Version`/`Ping`/`Pong` handshake. The envelope shape (`{version, req_id, type, payload}`) was deliberately built to grow into this (D-4.3), and `SensorShared::pending`'s `HashMap<u64, oneshot::Sender<()>>` was deliberately built to support concurrent correlated requests (05/06-CONTEXT code_context notes) — this phase is where both of those deferred design decisions get cashed in.

The work splits cleanly into two halves that can proceed almost independently: (1) a Rust-side protocol generalization that turns the single-purpose ping/pong plumbing into a generic typed request/response mechanism, fully testable offline with no VM (mirrors the existing `ping()` test harness exactly); and (2) a C# sensor-side implementation of four new request handlers using plain Win32 P/Invoke — `EnumWindows`/`GetWindowRect`/`GetWindowThreadProcessId` for the window list, `CreateToolhelp32Snapshot`/`Process32NextW` for the process tree (WMI/`System.Management` is ruled out — see the definitive recommendation below), `SetForegroundWindow` for focus, and `CreateProcessW` for launch. Per-window screenshots (D-6.1) need **no sensor-side code at all** — they are a pure client-side crop of the already-captured desktop framebuffer using the existing `Screenshot::crop(Rect)` (`crates/rdpilot/src/screenshot.rs:103-130`), fed by the `Rect` a window-list response carries.

**Primary recommendation:** Generalize `sensor.rs`'s `pending` map to `HashMap<u64, oneshot::Sender<serde_json::Value>>` and make `process()`'s fulfillment generic on "any inbound envelope whose `req_id` is in the pending map, regardless of `msg_type`" rather than adding a new hand-written match arm per feature (see Architecture Pattern 1). On the C# side, use `CreateToolhelp32Snapshot` + P/Invoke, never `System.Management`/WMI, for process enumeration — WMI is a documented NativeAOT/trimming incompatibility (`System.Management.WbemDefPath` throws `InvalidProgramException` under trimming, dotnet/runtime#61960), not merely untested.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Window enumeration (HWND/title/rect/z-order/state) | Remote sensor (C#, in-session) | Rust SDK (client, deserializes into owned `WindowInfo`) | Only a process running inside the interactive RDP session can call `EnumWindows`/`GetWindowRect` against the real remote desktop — RDP itself carries no window metadata (CLAUDE.md §4) |
| Process tree enumeration (PID/parent/name/path) | Remote sensor (C#, in-session) | Rust SDK (client, deserializes into owned `ProcessInfo`) | Same: process listing is a remote-OS-local operation with no RDP-native equivalent |
| Per-window screenshot | Rust SDK (client, crop of local framebuffer) | — | D-6.1 locked decision: client-side crop of the already-captured `DecodedImage`/`Screenshot`, zero sensor round-trip |
| Window focus control (`set_foreground_window`) | Remote sensor (C#, in-session) | Rust SDK (client, sends request + awaits ack) | `SetForegroundWindow` is a remote-OS Win32 call; the RDP transport only carries the request/response envelope |
| Remote process launch | Remote sensor (C#, in-session) | Rust SDK (client, sends request, returns PID) | `CreateProcessW` must run inside the target session; D-6.2 fire-and-forget means the SDK does no polling itself |
| Request/response correlation, timeout, typed error mapping | Rust SDK (`Session`, `session_loop`, `sensor.rs`) | — | Owned-type boundary (D-09): no `ironrdp`/`serde_json::Value` ever crosses into public `Session` method signatures |
| Wire envelope framing + DVC chunking | `ironrdp-dvc` (`encode_dvc_messages`) | Rust SDK (`JsonDvcMessage`) | Already established in Phase 4 — reused unchanged this phase (Don't Hand-Roll) |

## Standard Stack

### Core (no new external dependencies either side — see Package Legitimacy Audit)

| Component | Version | Purpose | Why Standard |
|-----------|---------|---------|--------------|
| `serde_json::Value` | 1.x (already a dependency) | Generic response payload carried through `SensorShared::pending` | Already used for `Envelope.payload`; avoids a new enum-of-payload-types on the Rust side |
| `System.Text.Json` source-gen (`JsonSerializerContext`) | .NET 8 SDK-bundled | AOT-safe (de)serialization of new request/response DTOs | Already established pattern (`EnvelopeJsonContext.cs`); NativeAOT has no runtime reflection fallback |
| Win32 `user32.dll` (`EnumWindows`, `GetWindowRect`, `GetWindowTextW`, `GetClassNameW`, `GetWindowThreadProcessId`, `GetWindow`, `GetWindowPlacement`, `IsIconic`, `IsZoomed`, `SetForegroundWindow`) | OS-native | Window list + focus control | The only Win32 surface that exposes window geometry/z-order/state; no managed wrapper avoids the P/Invoke |
| Win32 `kernel32.dll` (`CreateToolhelp32Snapshot`, `Process32FirstW`, `Process32NextW`, `OpenProcess`, `QueryFullProcessImageNameW`, `CreateProcessW`, `CloseHandle`) | OS-native | Process tree enumeration + launch | See definitive recommendation below — replaces the CLAUDE.md-era assumption of WMI |

### Explicitly NOT used this phase

| Instead of | Ruled out | Why |
|------------|-----------|-----|
| `System.Management` (WMI `Win32_Process`) for process enumeration | Ruled out, contradicts a stale CLAUDE.md stack note | `System.Management.WbemDefPath`'s constructor throws `InvalidProgramException` under IL trimming (dotnet/runtime#61960); COM-interop-under-NativeAOT is exactly the risk category 05-CONTEXT D-5.4 already deferred to Phase 7. `CreateToolhelp32Snapshot` is plain P/Invoke, sidesteps the whole COM/WMI risk class, and needs no publish-time trimming exception. |
| `IUIAutomation` COM for anything in this phase | Ruled out by scope | UIA is explicitly Phase 7 (D-5.4); Phase 6's Win32 surface (`EnumWindows`, `CreateToolhelp32Snapshot`, `CreateProcessW`) is plain P/Invoke, no COM — so this phase does **not** need to resolve the COM-under-NativeAOT question at all (06-CONTEXT Implementation notes, confirmed by reading `RdpilotSensor.csproj`: no `<PackageReference>`, no COM interop today) |
| `StringBuilder`-marshalled `GetWindowTextW`/`GetClassNameW` | Ruled out | `LibraryImport`'s source generator does not support `StringBuilder` marshalling (a documented gap vs. the older attribute-based `DllImport`); use a `Span<char>`/`char[]` buffer instead — see Pitfall 3 |

**Installation:** No new `Cargo.toml` or `.csproj` `<PackageReference>` entries this phase. `crates/rdpilot/Cargo.toml` already carries `serde`, `serde_json`, `tokio` (full features) — sufficient for the payload generalization. `sensor/RdpilotSensor.csproj` already has zero `<PackageReference>`s by design (05-01 comment: "everything ships in the .NET 8 SDK itself") — every new Win32 API this phase is reachable via `[LibraryImport]` against `user32.dll`/`kernel32.dll`, both present on every Windows target with no extra package.

**Version verification:** N/A — no new package versions to pin. `crates/rdpilot/Cargo.toml` dependency versions (`serde_json = "1"`, `tokio = { version = "1", features = ["full"] }`) are unchanged from Phase 2-5; `sensor/RdpilotSensor.csproj` targets `net8.0` unchanged from Phase 5.

## Package Legitimacy Audit

**Not applicable this phase.** Neither the Rust crate nor the C# sensor project needs a new `<PackageReference>`/`Cargo.toml` dependency: every new capability (window enumeration, process enumeration, focus, launch) is reachable via plain Win32 P/Invoke against `user32.dll`/`kernel32.dll`, which ship with every Windows OS and require no NuGet/crates.io package. `slopcheck`/registry verification is skipped because there is nothing to install.

## Architecture Patterns

### System Architecture Diagram

```
Rust SDK (client)                                    C# sensor (RDPILOT_SENSOR DVC server, in-session)
──────────────────                                    ──────────────────────────────────────────────

Session::get_window_list()  ─┐
Session::get_process_tree() ─┼─ alloc req_id           RunRequestLoop() [was RunHandshakeAndPingPongLoop]
Session::set_foreground_window(hwnd) ─┤   register        ReadEnvelope(handle)
Session::launch_process(...) ─┘   oneshot<Value>            │
        │                          in SensorShared.pending  │  dispatch on envelope.Type
        │ input_tx.send(                                    ▼
        │   RdpInputEvent::Request(MsgType, req_id, payload))
        ▼                                              ┌─────────────┬──────────────┬───────────────┬────────────────┐
session_loop::run() select! arm                        │ WindowList  │ ProcessTree  │ SetForeground  │ LaunchProcess  │
        │                                               │  handler    │   handler    │    handler     │    handler     │
        │ build_request_frame(active_stage,              │             │              │                │                │
        │   msg_type, req_id, payload)                   │ EnumWindows │ Toolhelp32   │ SetForeground  │ CreateProcessW │
        │   (generalized from build_ping_frame)          │ +GetWindow* │ Snapshot     │ Window         │                │
        ▼                                               └─────────────┴──────────────┴───────────────┴────────────────┘
ActiveStage::encode_dvc_messages → wire bytes  ──DVC──▶        │              │              │                │
                                                                └──────┬───────┴──────┬───────┴────────────────┘
        ▲                                                              ▼
        │                        WriteEnvelope(handle, Envelope{       same MsgType, req_id,
        │                          Type: <same as request>,             Payload: {success, data|error} }
        │                          ReqId: <echoed>, Payload: ... })
        │
RdpilotSensorProcessor::process() ◀──DVC── (reply bytes)
        │ any envelope with req_id ∈ pending → fulfill oneshot<Value>
        ▼
Session::<method> awaits rx w/ timeout ──▶ deserialize Value into owned SDK type
        │                                    (WindowInfo[] / ProcessInfo[] / () / u32 pid)
        │                                    OR map {success:false, error} → Error::SensorRejected (D-6.4)
        ▼
Caller gets Result<Vec<WindowInfo>> / Result<Vec<ProcessInfo>> / Result<()> / Result<u32>

Per-window screenshot (D-6.1, no round-trip):
Session::screenshot() ──clone SharedFrame──▶ Screenshot::crop(rect from WindowInfo.rect) ──▶ Screenshot
```

### Recommended Project Structure

```
crates/rdpilot/src/
├── sensor.rs         # Envelope/MsgType (extend), SensorShared (generalize pending), RdpilotSensorProcessor (generalize encode/process)
├── session.rs         # Session::{get_window_list, get_process_tree, set_foreground_window, launch_process} — mirror ping()
├── session_loop.rs    # RdpInputEvent::Request(MsgType,u64,Option<Value>) — generalize build_ping_frame → build_request_frame
├── perception.rs      # NEW — owned public types: WindowInfo, WindowState, ProcessInfo (D-09: only owned types leave Session)
└── error.rs           # Add Error::SensorRejected(String) for D-6.4 semantic-failure category

sensor/
├── Program.cs                 # Extend dispatch loop with 4 new MsgType cases; wrap each in try/catch → {success:false,error}
├── Envelope.cs                # Change Payload: object? → System.Text.Json.JsonElement? (AOT source-gen fix, see Pitfall 1)
├── EnvelopeJsonContext.cs     # Register every new request/response DTO with [JsonSerializable(typeof(...))]
├── WindowEnumeration.cs       # NEW — EnumWindows callback (UnmanagedCallersOnly) + Win32 P/Invoke surface for window data
├── ProcessEnumeration.cs      # NEW — CreateToolhelp32Snapshot walk + Win32 P/Invoke surface for process data
└── ProcessLaunch.cs           # NEW — CreateProcessW P/Invoke surface
```

### Pattern 1: Generalize `SensorShared::pending` to carry the reply payload, and make fulfillment type-agnostic

**What:** Today `pending: Mutex<HashMap<u64, oneshot::Sender<()>>>` (`sensor.rs:173`) only signals "a Pong with this req_id arrived" — no data. Change the value type to `oneshot::Sender<serde_json::Value>` and change `process()`'s dispatch (`sensor.rs:248-288`) so that **any** inbound envelope whose `req_id` has a pending entry is treated as that request's answer — fulfilling the sender with `envelope.payload.unwrap_or(Value::Null)` — regardless of `msg_type`. `MsgType::Pong`'s existing special-cased arm becomes just one instance of this generic path (the `req_id == 0` handshake stays its own separate `MsgType::Version` case, untouched).

**When to use:** This is the single generalization point that lets four new request types (`WindowList`, `ProcessTree`, `SetForegroundWindow`, `LaunchProcess`) reuse identical correlation/timeout/fulfillment code instead of four new hand-written match arms.

**Why this shape, not per-type reply tags:** The sensor is purely reactive (never sends unsolicited messages) and the SDK only ever receives replies to requests it originated — so `req_id` alone is a sufficient and already-unique correlation key. Requiring a distinct `XReply` `MsgType` for every request type (doubling the enum) buys nothing extra: `req_id` uniqueness is already the correlation invariant `ping()`/`Pong` relies on today.

**Example (Rust, sensor.rs, generalized shape):**
```rust
// Source: derived from this repo's own sensor.rs:158-186 (SensorShared) and
// sensor.rs:248-288 (process()) — not an external reference.
pub(crate) struct SensorShared {
    pub(crate) pending: Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
    pub(crate) handshake: Mutex<HandshakeState>,
}

// in RdpilotSensorProcessor::process():
match envelope.msg_type {
    MsgType::Version => { /* unchanged — handshake, req_id == 0 always */ }
    // Every other tag (Pong, WindowList, ProcessTree, SetForegroundWindow,
    // LaunchProcess) shares one generic fulfillment path:
    _ => {
        let mut pending = match self.shared.pending.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(sender) = pending.remove(&envelope.req_id) {
            let _ = sender.send(envelope.payload.unwrap_or(serde_json::Value::Null));
        }
        Ok(Vec::new())
    }
}
```

### Pattern 2: Fold outbound requests into a single `RdpInputEvent::Request(MsgType, u64, Option<Value>)`

**What:** `session_loop.rs:45-59`'s `RdpInputEvent` currently has a dedicated `Ping(u64)` variant with a dedicated `build_ping_frame` (`session_loop.rs:219-244`) and a dedicated `RdpilotSensorProcessor::encode_ping` (`sensor.rs:212-227`). Generalize both to accept an arbitrary `(MsgType, req_id, Option<Value>)` triple: `RdpInputEvent::Request(MsgType, u64, Option<serde_json::Value>)`, `build_request_frame(active_stage, msg_type, req_id, payload)`, and `RdpilotSensorProcessor::encode_request(msg_type, req_id, payload)`. `Ping` can be folded into this (`Request(MsgType::Ping, req_id, None)`), or kept as a thin wrapper that calls the generic path — either is safe; folding is recommended to avoid duplicating the `select!` arm five times.

**When to use:** Any time a new sensor-backed `Session` method is added — this is the intended growth point (06-CONTEXT Implementation notes explicitly frames this as "planner's call" but flags "avoid enum sprawl" as the goal).

**Why not one `RdpInputEvent` variant per request type:** Four new variants would mean four nearly-identical `session_loop::run()` `select!` match arms and four nearly-identical `build_*_frame` functions differing only in which `encode_*` method they call — pure duplication with no compensating clarity benefit, since the `MsgType` tag inside `Request` already discriminates for logging/tracing purposes.

### Pattern 3: `Session` methods mirror `ping()`'s exact shape (session.rs:389-431)

**What:** Every new method — `get_window_list()`, `get_process_tree()`, `set_foreground_window(hwnd)`, `launch_process(exe, args, cwd)` — follows the identical five-step shape `ping()` already establishes: (1) handshake fast-fail check, (2) allocate `req_id` via `next_req_id.fetch_add`, (3) insert a `oneshot::channel()` sender into `sensor.pending`, (4) send `RdpInputEvent::Request(...)` into `input_tx`, (5) `tokio::time::timeout(...)` await the receiver, removing the pending entry on timeout so a stale late reply cannot resurrect it.

**What differs per method:** the outbound `payload` (`None` for `get_window_list`/`get_process_tree`; `Some(json!({"hwnd": hwnd}))` for `set_foreground_window`; `Some(json!({"exe": ..., "args": ..., "cwd": ...}))` for `launch_process`), and the post-timeout deserialization of the `Value` into an owned SDK type (`Vec<WindowInfo>`, `Vec<ProcessInfo>`, `()`, `u32`).

**D-6.4 error branching on the received `Value` (new, not present in `ping()`):**
```rust
// Source: pattern derived from this repo's session.rs:419-430 (ping's timeout
// handling), extended for D-6.4's semantic-vs-transport distinction.
match tokio::time::timeout(Duration::from_millis(REQUEST_TIMEOUT_MS), rx).await {
    Ok(Ok(value)) => {
        let success = value.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if success {
            // deserialize value["data"] into the owned SDK type
        } else {
            let reason = value.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
            return Err(Error::sensor_rejected(reason)); // D-6.4 semantic category
        }
    }
    Ok(Err(_recv)) => Err(Error::dvc("sensor channel closed before replying")), // transport
    Err(_elapsed) => { /* remove pending entry */ Err(Error::dvc("request timed out")) } // transport
}
```

**Open question flagged for the planner:** `ping()`'s 500ms timeout was tuned for a trivial no-payload round trip. `get_window_list()`/`get_process_tree()` involve real enumeration work (`EnumWindows` over every top-level window, a full process snapshot walk) on the remote side plus a larger JSON payload over the DVC channel — 500ms may be too tight. Recommend a wider bound (e.g. 2000ms) for the two enumeration calls specifically, verified empirically at the live gate (Wave 4), while keeping `set_foreground_window`/`launch_process` (near-instant remote calls) at 500ms. This is a numeric tuning decision with no authoritative source — flagged in the Assumptions Log.

### Pattern 4: AOT-safe `EnumWindows` callback via `[UnmanagedCallersOnly]` + a `GCHandle`-boxed accumulator

**What:** `EnumWindows` takes a native callback pointer (`WNDENUMPROC`). Marshalling a managed delegate as that pointer through the classic `Marshal.GetFunctionPointerForDelegate` / `[UnmanagedFunctionPointer]` path is unreliable under NativeAOT (delegate marshalling has documented rough edges — dotnet/runtime#97952). The AOT-native pattern is a `static` method annotated `[UnmanagedCallersOnly]`, whose address is taken with `&MethodName` as a `delegate* unmanaged<nint, nint, int>`, passed directly to the `[LibraryImport]`-declared `EnumWindows`. Because the callback must be `static` (no closure state), the accumulator list is passed through `EnumWindows`'s own `lParam` parameter as a pinned `GCHandle` to a managed `List<nint>`, recovered inside the callback via `GCHandle.FromIntPtr(lParam).Target`.

**When to use:** This exact shape applies to any future Win32 enumeration callback this sensor adds (this phase: only `EnumWindows`; a documented reusable pattern for later phases too).

**Example (C#, confidence: MEDIUM — pattern corroborated by search, not verified against a live NativeAOT build this session — verify in Wave 0/1):**
```csharp
// Source: pattern derived from general .NET NativeAOT interop guidance
// (Microsoft docs on UnmanagedCallersOnlyAttribute + community NativeAOT
// P/Invoke examples) — [ASSUMED], verify with a Wave-0/1 smoke build.
[LibraryImport("user32.dll", SetLastError = true)]
[return: MarshalAs(UnmanagedType.Bool)]
private static partial bool EnumWindows(delegate* unmanaged<nint, nint, int> lpEnumFunc, nint lParam);

[UnmanagedCallersOnly]
private static int CollectWindowCallback(nint hwnd, nint lParam)
{
    var handle = GCHandle.FromIntPtr(lParam);
    if (handle.Target is List<nint> list)
    {
        list.Add(hwnd);
    }
    return 1; // continue enumeration
}

private static unsafe List<nint> EnumerateTopLevelWindows()
{
    var windows = new List<nint>();
    GCHandle handle = GCHandle.Alloc(windows);
    try
    {
        EnumWindows(&CollectWindowCallback, GCHandle.ToIntPtr(handle));
    }
    finally
    {
        handle.Free();
    }
    return windows;
}
```

### Pattern 5: Process enumeration via `CreateToolhelp32Snapshot`, never WMI — the definitive recommendation

**What:** `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)` + `Process32FirstW`/`Process32NextW` walking a `PROCESSENTRY32W` struct gives PID (`th32ProcessID`), parent PID (`th32ParentProcessID`), and process name (`szExeFile`, a fixed `MAX_PATH` char buffer — blittable, safe under `[LibraryImport]`) directly, with zero COM and zero runtime reflection. The full path (D-6.3 floor field) is NOT in `PROCESSENTRY32W` — obtain it separately via `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)` + `QueryFullProcessImageNameW(handle, 0, buffer, ref size)` + `CloseHandle`; `PROCESS_QUERY_LIMITED_INFORMATION` (not the older, more privileged `PROCESS_QUERY_INFORMATION`) is sufficient and available even for some elevated/protected processes.

**Why WMI/`System.Management` is ruled out (not just "unverified" — actively broken):** `System.Management`'s `WbemDefPath` static constructor is a **documented** trimming/NativeAOT incompatibility: dotnet/runtime#61960 reports it throws `InvalidProgramException` under trimming. WMI is COM-based under the hood (`IWbemLocator`/`IWbemServices`), landing squarely in the same COM-under-NativeAOT risk category 05-CONTEXT's D-5.4 already deferred to Phase 7 as "unproven." `CreateToolhelp32Snapshot` sidesteps both problems entirely: it is plain P/Invoke with no COM activation and no reflection-based marshalling, so it needs no publish-time trimming exception and no live-VM validation of a risky COM path.

**Command line and owner (D-6.3 extras, not floor fields, degrade gracefully):** Getting another process's command line without WMI requires reading its PEB (`NtQueryInformationProcess` for the PEB address, then `ReadProcessMemory` to walk `RTL_USER_PROCESS_PARAMETERS.CommandLine`) — an undocumented-API technique, `[ASSUMED]`/MEDIUM-LOW confidence, non-trivial, and NOT required by any of the 4 hard success criteria (only PID/parent PID/name/path are — REQUIREMENTS.md PERC-01, ROADMAP SC#2). Recommend implementing this as a best-effort extra that returns `None`/omits the field on any P/Invoke failure (never fails the whole process-tree call), or deferring it outright if it proves too fragile during implementation — consistent with D-6.3's own framing ("floor + high-value extras," extras explicitly optional). Owner (username) similarly requires `OpenProcessToken` + `GetTokenInformation(TokenUser)` + `LookupAccountSidW` — same best-effort treatment.

**Example (C#, confidence: MEDIUM — struct layout and function signatures are well-established Win32, `[ASSUMED]` on the exact `[LibraryImport]` marshalling attributes since not verified against a live NativeAOT build this session):**
```csharp
// Source: PROCESSENTRY32W layout per learn.microsoft.com/windows/win32/api/tlhelp32
// (training knowledge, not re-fetched this session — [ASSUMED], low risk: this
// struct has been stable since Windows XP).
[StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
internal struct PROCESSENTRY32W
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
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] // MAX_PATH
    public string szExeFile;
}

[LibraryImport("kernel32.dll", SetLastError = true)]
private static partial nint CreateToolhelp32Snapshot(uint dwFlags, uint th32ProcessID);

[LibraryImport("kernel32.dll", SetLastError = true)]
[return: MarshalAs(UnmanagedType.Bool)]
private static partial bool Process32FirstW(nint hSnapshot, ref PROCESSENTRY32W lppe);

[LibraryImport("kernel32.dll", SetLastError = true)]
[return: MarshalAs(UnmanagedType.Bool)]
private static partial bool Process32NextW(nint hSnapshot, ref PROCESSENTRY32W lppe);

private const uint TH32CS_SNAPPROCESS = 0x00000002;
```

### Pattern 6: The `Envelope.Payload` type must change from `object?` to `JsonElement?` for AOT source-gen to work

**What:** `Envelope.cs:52`'s `public object? Payload { get; init; }` is fine for the current Version/Ping/Pong phase (payload is always `null`), but the moment a real DTO (e.g. a `WindowListResponse`) needs to be attached, `System.Text.Json`'s source generator cannot statically resolve the runtime type of an `object?`-typed property without either enabling reflection-based polymorphism (which defeats the whole point of `PublishAot`/`JsonSerializerIsReflectionEnabledByDefault=false`) or manually assigning a `JsonElement`/`JsonNode` — both of which are natively DOM-typed and already source-gen-supported without per-type registration. Change `Payload`'s type to `System.Text.Json.JsonElement?`; build a typed response with `JsonSerializer.SerializeToElement(dto, EnvelopeJsonContext.Default.WindowListResponse)` and assign the result; read a typed request with `envelope.Payload.Value.Deserialize(EnvelopeJsonContext.Default.LaunchProcessRequest)`.

**When to use:** Every new request/response DTO this phase (`WindowListResponse`, `ProcessTreeResponse`, `SetForegroundWindowRequest`, `LaunchProcessRequest`, `LaunchProcessResponse`) needs a `[JsonSerializable(typeof(...))]` entry in `EnvelopeJsonContext.cs` AND must round-trip through `Envelope.Payload` as a `JsonElement`, never a raw `object`.

**Confidence:** MEDIUM — corroborated by search (dotnet/runtime discussions #115218/#115303 on AOT polymorphic serialization requiring exactly this kind of DOM-typed escape hatch), but not directly verified against this exact `Envelope`/`EnvelopeJsonContext` pairing this session. **Flag for Wave-0/1 smoke verification**: a minimal `dotnet publish -r win-x64 -p:PublishAot=true` of a `WindowListResponse` round-trip through the changed `Envelope.Payload` type should be the very first thing built in the C#-side wave, before writing any Win32 P/Invoke code, so a source-gen incompatibility surfaces early and cheaply rather than after the Win32 work is done.

### Anti-Patterns to Avoid

- **A new `RdpInputEvent` variant + `build_*_frame` function per request type:** duplicates `session_loop.rs`'s `select!` arm four times over; Pattern 2's unified `Request(MsgType, u64, Option<Value>)` avoids this.
- **`StringBuilder`-marshalled `GetWindowTextW`/`GetClassNameW`:** unsupported by the `[LibraryImport]` source generator (Pitfall 3) — use a fixed `Span<char>`/`char[]` buffer instead.
- **`System.Management`/WMI anywhere in the sensor:** documented trimming/AOT breakage (Pattern 5) — always plain P/Invoke.
- **Polling/waiting inside the C# launch handler for the new process to fully initialize:** contradicts D-6.2 fire-and-forget — reply with the PID the instant `CreateProcessW` returns, do not wait for the process to reach any particular state.
- **Reusing `PROCESS_QUERY_INFORMATION` (the older, fuller-access flag) when `PROCESS_QUERY_LIMITED_INFORMATION` suffices:** unnecessarily widens the sensor's own access rights for no functional benefit — the limited flag is documented as sufficient for `QueryFullProcessImageNameW`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|--------------|-----|
| DVC-level PDU chunking for larger window-list/process-tree JSON payloads | A custom fragmentation scheme | `ironrdp_dvc::encode_dvc_messages` (already used by `build_ping_frame`, `session_loop.rs:238-240`) | Already handles wire-level splitting/reassembly for oversized payloads — explicitly the reason `Envelope.payload` was designed as free-form JSON in Phase 4 (D-4.3) |
| Window enumeration / z-order / process listing itself | A custom driver, hook, or Detours-style injection | Plain user-mode `user32.dll`/`kernel32.dll` P/Invoke (`EnumWindows`, `CreateToolhelp32Snapshot`) | These are exactly the intended, documented, stable Win32 APIs for this — no elevated privilege or kernel component needed |
| A remote-process command-line retrieval library | Hand-rolled `NtQueryInformationProcess`/PEB-walking as a REQUIRED feature | Treat as an optional best-effort extra (D-6.3), degrade to `None` on failure | The undocumented-API technique is real but fragile across Windows versions; it is explicitly NOT one of the 4 hard success criteria — don't let it block the phase |
| Per-window screenshot capture | A sensor-side `PrintWindow`/`BitBlt` capture module | Client-side `Screenshot::crop(Rect)` (already exists, `screenshot.rs:103-130`, fully tested) | D-6.1 locked decision — sensor-side capture for occluded/minimized windows is explicitly deferred to backlog |

**Key insight:** Every new Win32 surface this phase (window enum, process enum via Toolhelp32, focus, launch) has a stable, decades-old, plain-P/Invoke-reachable API — there is no reason to reach for a COM interface, a WMI query, or a third-party wrapper library anywhere in this phase's scope.

## Common Pitfalls

### Pitfall 1: `Envelope.Payload` as `object?` silently breaks NativeAOT source-gen the moment a real payload is attached
**What goes wrong:** The project publishes fine today because `Payload` is always `null` (Version/Ping/Pong carry no data). The instant a `WindowListResponse` is assigned to an `object?`-typed property, `System.Text.Json`'s source generator either fails at publish/analysis time or (worse) silently falls back toward reflection-based serialization paths that `JsonSerializerIsReflectionEnabledByDefault=false` is specifically set to catch.
**Why it happens:** Source-gen serializers need the concrete runtime type statically known at the call site; an `object?`-typed property erases that.
**How to avoid:** Change `Envelope.Payload`'s type to `System.Text.Json.JsonElement?` (Pattern 6) before writing any handler logic; smoke-test a `dotnet publish -p:PublishAot=true` round-trip of one dummy typed payload as the very first Wave-0 task.
**Warning signs:** `JsonSerializerIsReflectionEnabledByDefault=false` publish-time warnings/errors referencing `Envelope` or the new DTOs; or (if publish silently succeeds but the flag is somehow not enforced) a runtime exception on the deployed target when the handler first tries to attach a payload.

### Pitfall 2: `System.Management`/WMI looks tempting from the (stale) CLAUDE.md stack notes but is a known-broken path under this project's NativeAOT constraints
**What goes wrong:** CLAUDE.md's original stack research (written before the C#-sensor architecture was finalized in Phase 5) lists `Win32_Process` via WMI/`Get-CimInstance` as the process-tree approach. Reaching for `System.Management` here reproduces exactly the COM-under-NativeAOT risk class 05-CONTEXT's D-5.4 already flagged as unproven — except this time it is not merely unproven, it is documented-broken (dotnet/runtime#61960).
**Why it happens:** CLAUDE.md is the canonical stack doc and predates the concrete Phase 5 C#-NativeAOT decision; its process-tree guidance is now stale.
**How to avoid:** Use `CreateToolhelp32Snapshot` (Pattern 5) — plain P/Invoke, no COM, no trimming exception needed.
**Warning signs:** Any `<PackageReference>`, `using System.Management`, or `ManagementObjectSearcher` appearing in a plan or diff for this phase.

### Pitfall 3: `LibraryImport` cannot marshal `StringBuilder` — `GetWindowTextW`/`GetClassNameW` need a buffer-based signature
**What goes wrong:** A naive port of a classic `DllImport`-era `GetWindowText(nint hWnd, StringBuilder lpString, int nMaxCount)` signature either fails to compile under the `[LibraryImport]` source generator or (if the generator silently accepts it via a fallback path) performs a full native-buffer-copy per call, which is both slow and not the AOT-idiomatic pattern this project has already established (05-01: "no attribute-based `DllImport`", `[LibraryImport]`-only discipline).
**Why it happens:** `LibraryImport`'s marshalling model is intentionally narrower than `DllImport`'s (a deliberate NativeAOT design tradeoff) and does not include `StringBuilder`.
**How to avoid:** Declare the P/Invoke with a fixed-size `Span<char>` or `char[]` output buffer (e.g. `GetWindowTextW(nint hWnd, Span<char> lpString, int nMaxCount)` with `StringMarshalling = StringMarshalling.Utf16`), then construct a `string` from the returned length.
**Warning signs:** Compiler errors referencing unsupported marshalling for `StringBuilder` parameters on a `[LibraryImport]`-attributed method; or (if it does compile) unexpectedly poor enumeration performance on a window-heavy desktop.

### Pitfall 4: `EnumWindows`/`SetForegroundWindow` need caller-side bounds on title/class-name length before allocating buffers
**What goes wrong:** `GetWindowTextW` can, in principle, be asked to copy an arbitrarily long title into a caller-supplied buffer; a window with a maliciously or accidentally huge title string (or a buffer-size bug) could over-allocate or truncate silently in a way that corrupts JSON framing.
**Why it happens:** Win32 window titles have no hard length contract enforced by the API itself beyond what the caller's buffer allows.
**How to avoid:** Use a fixed, generous but bounded buffer (e.g. 512 UTF-16 code units) for title/class name, and treat truncation (return value == buffer length) as expected/benign rather than an error — never grow the buffer unboundedly in a retry loop.
**Warning signs:** None observed yet (this is a proactive/defensive recommendation, not an empirically-hit bug) — ASVS V5 input-validation discipline extending the same "never trust the wire" posture `sensor.rs`/`Program.cs` already apply to inbound envelope bytes.

### Pitfall 5: `SetForegroundWindow`'s foreground-lock timeout can silently no-op
**What goes wrong:** Windows restricts which processes may steal foreground focus (the "foreground lock timeout" mechanism, `SPI_GETFOREGROUNDLOCKTIMEOUT`) — a call to `SetForegroundWindow` from a background/non-input-owning thread can return `TRUE` while the window is merely flashed in the taskbar rather than actually brought to the front.
**Why it happens:** This is intentional OS anti-focus-stealing behavior, not a bug — but it means "the call succeeded" and "the window is now foreground" are not the same fact.
**How to avoid:** SC#3 already anticipates this — "confirms the focus change in a subsequent window list query," i.e. the caller/test, not the sensor, is responsible for verifying the actual outcome via a follow-up `get_window_list()` call and checking which HWND is now foreground/topmost, exactly as ROADMAP Phase 6 SC#3 specifies. The sensor's own reply should report `success:true` only if `SetForegroundWindow`'s return value is nonzero (call succeeded), not attempt to itself verify the visual outcome.
**Warning signs:** A live test where `set_foreground_window` "succeeds" per its own reply but a follow-up window-list query shows a different window as foreground — expected, not a sign of a bug, as long as the SDK surfaces this as observable via the documented follow-up-query pattern rather than papering over it.

### Pitfall 6: Fire-and-forget `launch_process` (D-6.2) must not leak the child process's standard handles or block on `CreateProcessW`'s own synchronous nature
**What goes wrong:** `CreateProcessW` is itself synchronous (it returns once the process object is created, not once it finishes initializing) — this actually matches D-6.2's fire-and-forget contract well, but naive handling can leak the returned `PROCESS_INFORMATION.hProcess`/`hThread` handles if `CloseHandle` is forgotten, slowly exhausting the sensor's handle table across repeated `launch_process` calls over a long-lived sensor session.
**Why it happens:** `CreateProcessW` always returns open handles to the new process/thread that the caller owns and must explicitly close (unlike some higher-level launch APIs that auto-close).
**How to avoid:** Always `CloseHandle` both `hProcess` and `hThread` immediately after reading `dwProcessId` from `PROCESS_INFORMATION` — the SDK only needs the PID (D-6.2: "returns the new PID"), not a live handle.
**Warning signs:** A live gate that launches multiple processes across one test run and observes gradually degrading sensor behavior (handle-table exhaustion is a slow, cumulative failure, unlikely to show up in a single-launch test — worth a code-review-level check, not necessarily a dedicated live test).

## Code Examples

### Focus request (C#, `SetForegroundWindow` handler sketch)

```csharp
// Source: standard Win32 SetForegroundWindow signature (learn.microsoft.com/windows/win32/api/winuser),
// training knowledge — [ASSUMED], stable API since Windows 2000, low risk.
[LibraryImport("user32.dll", SetLastError = true)]
[return: MarshalAs(UnmanagedType.Bool)]
private static partial bool SetForegroundWindow(nint hWnd);
```

### Process launch (C#, `CreateProcessW` handler sketch)

```csharp
// Source: standard Win32 CreateProcessW signature (learn.microsoft.com/windows/win32/api/processthreadsapi),
// training knowledge — [ASSUMED], stable API, low risk. STARTUPINFOW/PROCESS_INFORMATION
// are blittable structs, safe for [LibraryImport].
[LibraryImport("kernel32.dll", EntryPoint = "CreateProcessW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
[return: MarshalAs(UnmanagedType.Bool)]
private static partial bool CreateProcessW(
    string? lpApplicationName,
    ref char lpCommandLine, // mutable buffer per Win32 contract — command line may be rewritten in place
    nint lpProcessAttributes,
    nint lpThreadAttributes,
    [MarshalAs(UnmanagedType.Bool)] bool bInheritHandles,
    uint dwCreationFlags,
    nint lpEnvironment,
    string? lpCurrentDirectory,
    ref STARTUPINFOW lpStartupInfo,
    out PROCESS_INFORMATION lpProcessInformation);
// After success: read lpProcessInformation.dwProcessId, then CloseHandle both
// hProcess and hThread immediately (Pitfall 6) — do not wait/poll (D-6.2).
```

### Rust-side owned public types (new `perception.rs`, D-09 boundary)

```rust
// Source: derived from D-6.3's field list, no external reference.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: u64,
    pub title: String,
    pub rect: crate::Rect,       // reuses the existing public Rect (screenshot.rs) — same coordinate space (Pitfall below)
    pub z_order: u32,
    pub state: WindowState,
    pub class_name: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState { Normal, Minimized, Maximized }

#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub path: String,
    pub command_line: Option<String>, // D-6.3 extra, best-effort
    pub owner: Option<String>,        // D-6.3 extra, best-effort
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `DllImport`/attribute-based P/Invoke | `[LibraryImport]` source-generated P/Invoke | .NET 7 (2022), mandatory in practice for this project since Phase 5's NativeAOT constraint | No runtime reflection-based marshalling code generation — required for `PublishAot=true` to avoid warnings/failures; already the established convention in `sensor/Program.cs` |
| Managed delegate marshalling for native callbacks | `[UnmanagedCallersOnly]` + `delegate*  unmanaged<...>` function pointers | .NET 5+ (2020), the AOT-idiomatic pattern | Needed the moment `EnumWindows` is P/Invoked — this phase is the sensor's first callback-taking Win32 API |
| WMI (`System.Management`) for process enumeration | `CreateToolhelp32Snapshot` P/Invoke | Effectively "always was the AOT-safe choice"; WMI's incompatibility with trimming/AOT is a known, tracked issue (dotnet/runtime#61960), not a recent regression | Rules out the CLAUDE.md-era stack assumption for this specific phase's process-tree feature |

**Deprecated/outdated:** The CLAUDE.md "Structured Perception" table's WMI recommendation for `Win32_Process` predates Phase 5's concrete C#-NativeAOT decision and should be treated as superseded by this research's Pattern 5 for Phase 6 specifically — CLAUDE.md itself is not being edited by this research (out of scope), but the planner should not follow that specific line item.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|----------------|
| A1 | `Envelope.Payload: object?` → `JsonElement?` is the correct AOT-safe fix for polymorphic payload serialization | Pattern 6 / Pitfall 1 | If wrong, publish-time or runtime JSON serialization failures on the sensor — mitigated by recommending this as the FIRST Wave-0/1 smoke-test task, so the failure (if any) surfaces before Win32 work is invested |
| A2 | `[UnmanagedCallersOnly]` + `delegate* unmanaged<...>` + `GCHandle`-boxed accumulator is the correct AOT-safe pattern for `EnumWindows`'s callback | Pattern 4 | If the exact syntax is wrong, a compile error (cheap to fix) — not a silent runtime failure |
| A3 | `PROCESSENTRY32W` struct layout, `Process32FirstW`/`Process32NextW`, `CreateToolhelp32Snapshot` signatures | Pattern 5 | These are extremely stable, decades-old, unchanged-since-XP Win32 APIs — low risk even though not re-fetched from live docs this session |
| A4 | `CreateProcessW`/`STARTUPINFOW`/`PROCESS_INFORMATION` signatures and the mutable-`lpCommandLine`-buffer contract | Code Examples | Same — stable, well-documented Win32 API; the mutable-buffer detail is a known Win32 gotcha worth calling out explicitly to whoever implements the handler |
| A5 | `PROCESS_QUERY_LIMITED_INFORMATION` is sufficient for `QueryFullProcessImageNameW` against ordinary (non-protected) processes | Pattern 5 | If some target processes require the fuller `PROCESS_QUERY_INFORMATION`, the sensor should widen the flag only for those calls that fail, not globally — low risk, easy to detect and fix at implementation time (an `OpenProcess` failure is observable, not silent) |
| A6 | 500ms is too tight a timeout for `get_window_list()`/`get_process_tree()` and should be widened (recommend 2000ms) | Pattern 3 | If 500ms actually proves sufficient, the wider bound just means a slower failure mode on a genuinely broken sensor — low cost either way; needs live-gate empirical tuning (mirrors how `RUN_DIALOG_SETTLE`/`SESSION_SETTLE` were empirically tuned in Phase 5) |
| A7 | Command-line retrieval via `NtQueryInformationProcess`/PEB-walking is feasible but fragile; recommend treating as optional/deferrable | Pattern 5 | If deferred, D-6.3's "extras" are simply narrower than hoped — does not affect any of the 4 hard success criteria (PID/parent/name/path only) |

## Open Questions

1. **Exact request timeout values for the four new `Session` methods**
   - What we know: `ping()`'s 500ms bound exists and is proven live (Phase 4: measured 165ms round trip). `deploy_and_launch` already establishes the pattern of a longer, phase-specific timeout tuned empirically (D-5.2's `PINGS_PER_LAUNCH_ATTEMPT`/`LAUNCH_ATTEMPTS`).
   - What's unclear: Whether `EnumWindows`/`CreateToolhelp32Snapshot` enumeration on a real remote desktop plus the larger JSON payload over DVC will reliably complete within 500ms, or needs a wider bound.
   - Recommendation: Start with a wider default (e.g. 2000ms) for `get_window_list`/`get_process_tree` specifically, keep 500ms for `set_foreground_window`/`launch_process` (near-instant calls), and empirically tune at the Wave 4 live gate exactly as Phase 3/5 tuned their own timing constants live.

2. **Whether the C# sensor needs its own unit-test project**
   - What we know: No C# test project exists in `sensor/` today (only `Program.cs`/`Envelope.cs`/`EnvelopeJsonContext.cs`/`RdpilotSensor.csproj`). Phase 5's SENSOR-01/02 were verified entirely via the gated live integration suite on the Rust side, not a C#-side test project.
   - What's unclear: Whether the added Win32 P/Invoke surface area this phase (4 new handlers) is complex enough to warrant offline C#-side unit tests for the parts that don't require a live Windows session (e.g. window-state classification logic given `IsIconic`/`IsZoomed` flags, or `JsonElement` payload round-tripping).
   - Recommendation: See Validation Architecture below — recommend a minimal `xunit`-based test project scoped ONLY to pure/testable logic (state classification, JSON DTO round-trips via `EnvelopeJsonContext`), leaving actual Win32 API correctness to the live gate, consistent with the project's established pattern of empirically verifying Win32/RDP-adjacent behavior live rather than mocking it.

## Environment Availability

No new external tool/service dependencies this phase beyond what Phases 1-5 already established and proved live: the disposable Azure Windows VM (`infra/`, `manage-env.ps1`), the .NET 8 SDK + native MSVC/Windows SDK toolchain on a genuine Windows host (required for `dotnet publish -p:PublishAot=true`, unchanged from Phase 5), and the Rust `x86_64-pc-windows-gnu` cross-build toolchain (unchanged from Phase 2). All were verified live and working as of the Phase 5 live gate (2026-07-09) and are expected to remain available for this phase's own live gate — no new environment audit needed.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework (Rust) | Built-in `#[test]`/`#[tokio::test]` (no external test framework — matches Phases 1-5) |
| Framework (C#) | None exists yet — Wave 0 gap, see below |
| Config file | None (Cargo's built-in test harness; no `.csproj` test project yet on the C# side) |
| Quick run command (Rust, offline) | `cargo test -p rdpilot` |
| Full suite command (Rust, gated live) | `RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1` |
| Quick check command (C#) | `dotnet build sensor/RdpilotSensor.csproj` (catches P/Invoke signature and source-gen compile errors) |
| Full publish check (C#) | `dotnet publish sensor/RdpilotSensor.csproj -r win-x64 -p:PublishAot=true --self-contained` (must run on a genuine Windows host — NativeAOT does not cross-compile, per `RdpilotSensor.csproj`'s own comment) |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|---------------------|--------------|
| PERC-02 (ROADMAP SC#1: window list) | `get_window_list()` returns HWND/title/rect/z-order/state | unit (Rust, offline, canned payload) + gated live | `cargo test -p rdpilot get_window_list` / live suite | ❌ Wave 0/1 (unit) + Wave 4 (live) |
| PERC-01 (ROADMAP SC#2: process tree) | `get_process_tree()` returns PID/parent/name/path | unit (Rust, offline, canned payload) + gated live | `cargo test -p rdpilot get_process_tree` / live suite | ❌ Wave 0/1 (unit) + Wave 4 (live) |
| PERC-04 (ROADMAP SC#3: focus) | `set_foreground_window` + follow-up window-list query confirms focus | gated live only (the confirmation IS a live cross-call assertion, not offline-testable) | live suite | ❌ Wave 4 |
| PROC-01 (ROADMAP SC#4: launch) | `launch_process` + follow-up process-tree query shows new PID | gated live only (same reasoning) | live suite | ❌ Wave 4 |
| CAP-02 (per-window screenshot) | Client-side crop of a window's rect produces a correctly-sized/positioned `Screenshot` | unit (Rust, offline, fully covered by EXISTING `screenshot.rs` crop tests) — no new sensor round-trip means no new live-only assertion is strictly required (D-6.1: not one of the 4 hard criteria) | `cargo test -p rdpilot crop` (already passing) + a new offline wiring test using a canned `WindowInfo.rect` | ✅ crop tests exist; ❌ wiring test (Wave 1) |

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot` (Rust) + `dotnet build sensor/RdpilotSensor.csproj` (C#, compile-only check)
- **Per wave merge:** Full Rust offline suite + a `dotnet publish -p:PublishAot=true` smoke check (catches AOT source-gen regressions early, per Pitfall 1)
- **Phase gate:** `RDPILOT_LIVE=1 ... cargo test -p rdpilot -- --include-ignored --test-threads=1` green, covering all 4 hard success criteria, before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] Offline Rust unit tests for the generalized `SensorShared::pending`/`process()` dispatch (mirrors existing `pong_fulfils_and_removes_pending_oneshot` — Wave 1)
- [ ] Offline Rust unit tests for each new `Session` method's success/timeout/semantic-failure branches, using the existing `test_session_with_sensor` helper pattern (Wave 1)
- [ ] A minimal C# `xunit` test project (optional, recommended for pure logic only — window-state classification, `JsonElement` payload round-trip smoke test) — currently does not exist; if added, register it as a new project alongside `RdpilotSensor.csproj`, not merged into it (the sensor itself must stay a pure `Exe`/`PublishAot` project)
- [ ] Gated live tests: `window_list_returns_all_visible_windows`, `process_tree_returns_pid_parent_name_path`, `set_foreground_window_confirmed_by_followup_query`, `launch_process_appears_in_followup_process_tree` (Wave 4, mirrors the one-test-per-success-criterion pattern from `live_session.rs`)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|----------------|---------|-------------------|
| V2 Authentication | No (new surface) | Unchanged — the RDP session's own NLA/CredSSP authentication (Phase 2) is the trust boundary for the whole `RDPILOT_SENSOR` channel; this phase adds no new auth layer, consistent with the existing "by-design unauthenticated [within an already-authenticated RDP session]" model (Pitfall m3 references in `sensor.rs`) |
| V3 Session Management | No | N/A — no new session concept introduced |
| V4 Access Control | Yes (documented, not mitigated further) | `launch_process` is an intentional remote-code-execution primitive (PROC-01's whole purpose) — its "access control" is entirely the pre-existing RDP session's own authentication; no additional gate is introduced or needed this phase, since restricting it further would contradict the requirement itself. Document this as an accepted-by-design capability, not an oversight. |
| V5 Input Validation | Yes | Every inbound envelope byte is still parsed via `serde_json::from_slice::<Envelope>` (Rust) / `JsonSerializer.Deserialize(..., EnvelopeJsonContext...)` (C#) with drop-on-malformed semantics (T-04-01/T-05-01) — extend this discipline to the new payload DTOs (`hwnd`, `exe`, `args`, `cwd` fields), never `unwrap`/throw on a missing/malformed field. Bound window-title/class-name buffer sizes (Pitfall 4) so a pathological string cannot corrupt JSON framing or cause unbounded allocation. |
| V6 Cryptography | No | N/A — no new cryptographic material this phase; TLS (Phase 2, rustls) remains the only crypto boundary |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|-----------------------|
| Malformed/oversized JSON payload on the DVC channel (either direction) crashes the reading side | Denial of Service | Already established discipline (T-04-01/T-05-01): malformed bytes are dropped (`Ok(Vec::new())`/`return null`), never panic/throw — extend unchanged to the new payload shapes |
| A hostile process on the remote desktop with a maliciously long window title | Denial of Service (resource exhaustion via unbounded string allocation) | Bounded buffer for `GetWindowTextW`/`GetClassNameW` (Pitfall 4), truncate rather than grow |
| `launch_process` used to start an arbitrary/unexpected executable | Elevation of Privilege (by design, within the existing RDP trust boundary) | No new mitigation — this is the requirement's intended capability (PROC-01); the RDP session's own NLA/CredSSP authentication is the only and sufficient boundary, matching the project's existing threat model for the whole `RDPILOT_SENSOR` channel |
| Handle-table exhaustion from repeated `launch_process`/`OpenProcess` calls without `CloseHandle` | Denial of Service (resource leak) | Always close `hProcess`/`hThread`/snapshot handles (Pitfall 6) |

## Sources

### Primary (HIGH confidence)
- This repository's own source: `crates/rdpilot/src/{sensor.rs, session.rs, session_loop.rs, screenshot.rs, error.rs, connect.rs}`, `sensor/{Program.cs, Envelope.cs, EnvelopeJsonContext.cs, RdpilotSensor.csproj}`, `.planning/{ROADMAP.md, STATE.md, REQUIREMENTS.md}`, `.planning/phases/06-window-process-perception/06-CONTEXT.md`, `.planning/phases/{04,05}-*/{04,05}-CONTEXT.md` — read directly this session, line numbers cited throughout

### Secondary (MEDIUM confidence)
- [dotnet/runtime#61960 — System.Management.WbemDefPath doesn't work with trimming](https://github.com/dotnet/runtime/issues/61960) — WMI/NativeAOT incompatibility, corroborates the definitive process-enumeration recommendation
- [dotnet/runtime discussion #115218/#115303 — AOT polymorphic serialization](https://github.com/dotnet/runtime) — corroborates the `JsonElement`-payload recommendation (Pattern 6/Pitfall 1)
- [UnmanagedCallersOnlyAttribute Class — Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/api/system.runtime.interopservices.unmanagedcallersonlyattribute) — corroborates the `EnumWindows` callback pattern (Pattern 4)
- General search corroboration that `[LibraryImport]`'s source generator does not support `StringBuilder` marshalling (Pitfall 3)

### Tertiary (LOW confidence, training-knowledge, flagged in Assumptions Log)
- `PROCESSENTRY32W`, `CreateProcessW`/`STARTUPINFOW`/`PROCESS_INFORMATION`, `SetForegroundWindow` exact P/Invoke signatures (A3/A4) — extremely stable, decades-old Win32 APIs, not re-fetched from live Microsoft Learn pages this session, but very low risk given their unchanged-since-XP/2000 status
- PEB/`NtQueryInformationProcess`-based command-line retrieval technique (A7) — undocumented-API pattern, correctly scoped as optional/best-effort per D-6.3, not required for any hard success criterion

## Metadata

**Confidence breakdown:**
- Standard stack / architecture (Rust-side protocol generalization): HIGH — grounded directly in this repo's own existing, working code
- Standard stack / architecture (C#-side Win32 P/Invoke specifics): MEDIUM — well-established APIs, corroborated by search, but not verified against a live NativeAOT build this session (explicitly flagged for Wave-0/1 smoke verification)
- Pitfalls: MEDIUM-HIGH — Pitfalls 1/2/3 are grounded in documented, cited NativeAOT/WMI incompatibilities; Pitfalls 4/5/6 are proactive/reasoned recommendations, not empirically-hit bugs (flagged as such)

**Research date:** 2026-07-09
**Valid until:** 30 days (stable Win32 APIs; the one fast-moving risk is .NET/NativeAOT source-gen behavior around `JsonElement` payloads — re-verify if the phase slips past a .NET SDK minor version bump)
