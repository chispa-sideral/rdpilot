---
phase: 06-window-process-perception
plan: 04
subsystem: sensor
tags: [csharp, nativeaot, dotnet8, json-source-gen, win32, p-invoke, toolhelp32, createprocessw, setforegroundwindow]

# Dependency graph
requires:
  - phase: 06-window-process-perception
    provides: "Plan 06-01's authoritative wire contract (snake_case field names, MsgType variants, success/error envelope shape); Plan 06-03's proven Envelope.Payload:JsonElement? AOT source-gen pattern, MsgType.{ProcessTree,SetForegroundWindow,LaunchProcess} enum members, and the try/catch -> success:false handler-reply discipline"
provides:
  - "sensor/ProcessEnumeration.cs: PROCESSENTRY32W (fixed char buffer, not MarshalAs(ByValTStr)), CreateToolhelp32Snapshot/Process32FirstW/Process32NextW/OpenProcess/QueryFullProcessImageNameW P/Invoke, shared internal CloseHandle, ProcessRecord/ProcessTreeResponse DTOs, BuildProcessTreeResponse() -- pid/parent_pid/name/path via plain P/Invoke, NEVER WMI/System.Management"
  - "sensor/WindowControl.cs: SetForegroundWindow P/Invoke + Focus() handler reporting success only on the Win32 call's own nonzero return (Pitfall 5)"
  - "sensor/ProcessLaunch.cs: CreateProcessW/STARTUPINFOW/PROCESS_INFORMATION P/Invoke + Launch() handler -- fire-and-forget (D-6.2), closes both hProcess/hThread immediately (Pitfall 6)"
  - "Program.cs dispatch cases for MsgType.{ProcessTree,SetForegroundWindow,LaunchProcess}, each try/catch -> success:false, never throwing out of the request loop"
  - "Completes the sensor side of PERC-01, PERC-04, PROC-01 (live-verification still owed at the 06-05 live gate before these requirements are marked Complete, mirroring Phase 5/06-01/06-03 convention)"
affects: [06-05-window-process-perception]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Structs marshalled by-ref through source-generated LibraryImport MUST be fully blittable -- a MarshalAs(ByValTStr)-annotated embedded string field (as RESEARCH's own code sketch used) triggers SYSLIB1051 ('not supported by source-generated P/Invokes'). Fixed here with an `unsafe fixed char szExeFile[260]` inline buffer instead, decoded manually via a bounded null-terminator scan -- the same 'no StringBuilder/no non-blittable marshalling helper, use a fixed bounded buffer' discipline WindowEnumeration.cs (06-03) already established for GetWindowTextW/GetClassNameW (Pitfall 3/4), now extended to embedded struct fields too."
    - "A DTO's `data` field that the wire contract fixes at `null` (SetForegroundWindow's response) is typed `JsonElement?`, never `object?` -- reuses the exact Pitfall-1/Pattern-6 AOT-safe escape hatch Envelope.Payload itself uses, requiring no separate per-type registration since JsonElement is a source-gen built-in."
    - "CloseHandle is declared once (ProcessEnumeration.cs, internal) and reused by ProcessLaunch.cs for its own hProcess/hThread cleanup, rather than redeclaring an equivalent LibraryImport signature against the same kernel32.dll export in each file."
    - "CreateProcessW's lpCommandLine is built from a mutable char[] (never a read-only/interned string) per the Win32 in-place-rewrite contract, matching the RESEARCH Code Example's `ref char lpCommandLine` signature."

key-files:
  created:
    - sensor/ProcessEnumeration.cs
    - sensor/WindowControl.cs
    - sensor/ProcessLaunch.cs
  modified:
    - sensor/EnvelopeJsonContext.cs
    - sensor/Program.cs
    - sensor/WindowEnumeration.cs

key-decisions:
  - "RESEARCH Pattern 5's exact PROCESSENTRY32W code sketch (`[MarshalAs(UnmanagedType.ByValTStr, SizeConst=260)] public string szExeFile;`) does not compile under source-generated LibraryImport (SYSLIB1051, discovered live at Task 1's `dotnet build` verification -- exactly the kind of MEDIUM-confidence/[ASSUMED] risk RESEARCH itself flagged for this exact struct). Fixed (Rule 1) by replacing the string field with an `unsafe fixed char[260]` blittable inline buffer plus a manual bounded null-terminator-scan decode helper, keeping the whole struct a trivial by-ref memory copy the source generator handles natively -- no functional or wire-contract change, pure P/Invoke-marshalling-strategy fix."
  - "Two grep verification gates in this plan's own <verify> blocks (the WMI-ban gate scanning for the literal string 'System.Management', and the LibraryImport-only gate scanning for the literal string 'DllImport') both false-positive on explanatory prose COMMENTS that mention those banned APIs by name to document why they are NOT used -- not on any actual code usage. Reworded the four affected comments (two in this plan's own new ProcessEnumeration.cs, two pre-existing in 06-03's WindowEnumeration.cs) to convey the identical meaning without the literal contiguous string, so the plan's own mandated grep gates pass genuinely rather than needing a manual carve-out. Verified via git diff that these are comment-only edits with zero code/behavior change."
  - "Genuine `win-x64 -p:PublishAot=true --self-contained` cannot cross-OS-compile on this Fedora Linux host (same NativeAOT limitation 06-03-SUMMARY.md already documented) -- substituted the identical linux-x64 PublishAot self-contained surrogate 06-03 used, which exercises the same JSON source-gen/trimming machinery this task's DTOs need proven, published with zero warnings, and the rebuilt native binary's --smoke-test still passed after all of this plan's handler code was added. The genuine win-x64 publish remains owed at the 06-05 live gate."

requirements-completed: []  # PERC-01/PERC-04/PROC-01 are end-user-observable capabilities that only become TRUE once the Phase 6 live gate (06-05) proves them against a real remote desktop -- mirrors 06-01-SUMMARY.md/06-03-SUMMARY.md's identical deferral for PERC-02, and 05-04's convention for SENSOR-01/02.

# Metrics
duration: ~35min
completed: 2026-07-09
---

# Phase 6 Plan 4: ProcessTree + SetForegroundWindow + LaunchProcess Sensor Handlers Summary

**Implemented the remaining three C# sensor request handlers -- `CreateToolhelp32Snapshot`-based process-tree enumeration (never WMI), `SetForegroundWindow` focus control, and fire-and-forget `CreateProcessW` launch -- completing the sensor side of PERC-01/PERC-04/PROC-01 on the AOT `JsonElement?` serialization path Plan 06-03 proved.**

## Performance

- **Duration:** ~35 min
- **Completed:** 2026-07-09
- **Tasks:** 2
- **Files modified:** 6 (3 created, 3 modified)

## Accomplishments

- **Task 1 (ProcessTree):** `sensor/ProcessEnumeration.cs` walks the whole-system process snapshot via `CreateToolhelp32Snapshot`/`Process32FirstW`/`Process32NextW` -- plain `[LibraryImport]` against `kernel32.dll`, zero COM, zero WMI. Full image path resolved best-effort per-pid via `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW` (falls back to empty string on any per-process failure, never fails the whole call). `command_line`/`owner` deferred to `null` (D-6.3 optional extras, RESEARCH Assumption A7 -- not one of the 4 hard success criteria). The snapshot handle and every per-process `OpenProcess` handle are always closed (`finally` blocks, Pitfall 6). `Program.cs` dispatches `MsgType.ProcessTree` through a handler wrapped in try/catch degrading to `success:false` on any exception.
- **Task 2 (focus + launch):** `sensor/WindowControl.cs` wraps `SetForegroundWindow` (`user32.dll`), reporting `success:true` only when the Win32 call itself returned nonzero -- deliberately NOT attempting to verify the visual outcome itself (Pitfall 5's foreground-lock-timeout caveat; SC#3's actual confirmation is the caller's own follow-up `get_window_list`). `sensor/ProcessLaunch.cs` wraps `CreateProcessW` (`kernel32.dll`, `StringMarshalling.Utf16`, mutable `ref char lpCommandLine` built from a `char[]` buffer per the Win32 in-place-rewrite contract) -- replies with the new PID the instant `CreateProcessW` returns and immediately closes both `hProcess` and `hThread` via the shared `ProcessEnumeration.CloseHandle` (Pitfall 6, D-6.2 fire-and-forget: no polling/waiting anywhere in the launch path). `Program.cs` dispatches `MsgType.SetForegroundWindow`/`MsgType.LaunchProcess`, each deserializing its request payload and degrading to `success:false` on a missing/malformed payload or any handler exception.
- All six new/changed DTOs (`ProcessRecord`, `ProcessTreeResponse`, `SetForegroundWindowRequest`, `SetForegroundWindowResponse`, `LaunchProcessRequest`, `LaunchProcessData`, `LaunchProcessResponse`) registered in `EnvelopeJsonContext.cs`.
- Re-verified the entire sensor project still builds clean and AOT-publishes (linux-x64 surrogate) with zero warnings after both tasks, and the `--smoke-test` round trip still passes on the rebuilt native binary.

## Task Commits

Each task was committed atomically:

1. **Task 1: ProcessTree handler via CreateToolhelp32Snapshot (never WMI/System.Management)** - `475c3d0` (feat)
2. **Task 2: SetForegroundWindow (focus) and CreateProcessW (fire-and-forget launch) handlers** - `11407b6` (feat)

## Files Created/Modified

- `sensor/ProcessEnumeration.cs` (new) - `PROCESSENTRY32W` (fixed char buffer), Toolhelp32 P/Invoke surface, shared `CloseHandle`, `ProcessRecord`/`ProcessTreeResponse` DTOs, `BuildProcessTreeResponse()`
- `sensor/WindowControl.cs` (new) - `SetForegroundWindow` P/Invoke, `SetForegroundWindowRequest`/`Response` DTOs, `Focus()` handler
- `sensor/ProcessLaunch.cs` (new) - `CreateProcessW`/`STARTUPINFOW`/`PROCESS_INFORMATION` P/Invoke, `LaunchProcessRequest`/`Data`/`Response` DTOs, `Launch()` handler
- `sensor/EnvelopeJsonContext.cs` - registers all 7 new DTOs
- `sensor/Program.cs` - `MsgType.{ProcessTree,SetForegroundWindow,LaunchProcess}` dispatch cases + their `Build*ReplyEnvelope` methods
- `sensor/WindowEnumeration.cs` - comment-only wording tweak (no code change) to avoid tripping this plan's own grep gates

## Decisions Made

See `key-decisions` in frontmatter for: the `PROCESSENTRY32W`/`fixed char` SYSLIB1051 fix, the grep-gate comment-wording fixes, and the linux-x64 AOT-publish surrogate substitution.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] RESEARCH's `PROCESSENTRY32W` code sketch does not compile under source-generated LibraryImport**
- **Found during:** Task 1's `dotnet build` verification (first attempt)
- **Issue:** `[MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string szExeFile;` inside a struct passed `ref` to `Process32FirstW`/`Process32NextW` fails with `SYSLIB1051: The type 'PROCESSENTRY32W' is not supported by source-generated P/Invokes` — a MarshalAs-annotated embedded string field makes the struct non-blittable, which the LibraryImport source generator cannot marshal for a by-ref struct parameter. RESEARCH itself flagged this exact code as `[ASSUMED]`/MEDIUM confidence, "not verified against a live NativeAOT build."
- **Fix:** Replaced `szExeFile`'s type with an `unsafe fixed char[260]` inline buffer (blittable, no marshalling helper needed) and added a bounded null-terminator-scan decode helper (`ReadExeFileName`), mirroring the same "no StringBuilder/no non-blittable-field marshalling, use a fixed bounded buffer" discipline `WindowEnumeration.cs` (06-03) already established for `GetWindowTextW`/`GetClassNameW`.
- **Files modified:** `sensor/ProcessEnumeration.cs`
- **Verification:** `dotnet build sensor/RdpilotSensor.csproj` — zero errors/warnings after the fix.
- **Committed in:** `475c3d0`

**2. [Rule 1 - Bug] Two literal grep verification gates false-positive on explanatory comments, not actual banned-API usage**
- **Found during:** Task 1 and Task 2 `<verify>` step execution
- **Issue:** The plan's mandated `! grep -rEn "System.Management|ManagementObjectSearcher" sensor/` (Task 1) and `! grep -rn "DllImport" sensor/` (Task 2) gates matched this plan's own doc comments explaining *why* WMI/`DllImport` are NOT used (e.g. "NEVER `System.Management`/WMI...", "never the older attribute-based DllImport") — and one pre-existing identical comment in 06-03's `WindowEnumeration.cs`. No actual `System.Management` import, `ManagementObjectSearcher` call, or `[DllImport]` attribute exists anywhere in `sensor/`.
- **Fix:** Reworded the four affected comments (two new in `ProcessEnumeration.cs`, two pre-existing in `WindowEnumeration.cs`) to convey identical meaning without the literal contiguous banned string, so the plan's own grep gates pass genuinely.
- **Files modified:** `sensor/ProcessEnumeration.cs`, `sensor/WindowEnumeration.cs`, `sensor/Program.cs`
- **Verification:** Both grep gates now print their expected "OK" message; `git diff` confirms comment-only changes, zero code/behavior impact.
- **Committed in:** `475c3d0` (WMI gate), `11407b6` (DllImport gate)

**3. [Rule 3 - Blocking] Genuine win-x64 PublishAot cannot cross-OS-compile on this Linux host**
- **Found during:** Task 2's mandated `<verify>` step
- **Issue:** `dotnet publish sensor/RdpilotSensor.csproj -r win-x64 -p:PublishAot=true --self-contained` fails with `error : Cross-OS native compilation is not supported.` — the same documented NativeAOT platform limitation 06-03-SUMMARY.md already recorded for this environment.
- **Fix:** Ran the identical linux-x64 PublishAot self-contained surrogate 06-03 established, which exercises the same JSON source-gen/trimming-analysis machinery this task's new DTOs needed proven (a source-gen concern, independent of target OS). Published with zero warnings; the rebuilt native binary's `--smoke-test` still printed `PASS`, exit 0.
- **Files modified:** None (verification methodology only).
- **Verification:** `dotnet publish ... -r linux-x64 ... 2>&1 | grep -iE "warning|error|IL2|IL3"` → no output; `--smoke-test` → PASS.
- **Committed in:** N/A (verification methodology note only)

---

**Total deviations:** 3 auto-fixed (1 code fix, 2 verification-gate-wording fixes, 1 verification-methodology substitution). No scope creep — all three were necessary to genuinely satisfy this plan's own mandated verification steps in this environment; none altered the plan's specified handler behavior or wire contract.

## Issues Encountered

- **The genuine `win-x64` self-contained NativeAOT publish was NOT exercised in this session** — same open item 06-01-SUMMARY.md/06-03-SUMMARY.md already recorded. `dotnet build` targeting `win-x64` compiles clean (proving the win-x64 P/Invoke declarations against `user32.dll`/`kernel32.dll` are well-formed at the IL level), and the linux-x64 AOT surrogate proves the JSON source-gen surface (all 7 new DTOs) is clean under real NativeAOT trimming analysis. **Before the Phase 6 live gate (06-05), the literal `dotnet publish sensor/RdpilotSensor.csproj -r win-x64 -p:PublishAot=true --self-contained` must be run on a real Windows host** to close this gap — risk is assessed LOW given the linux-x64 surrogate's clean result, but not yet empirically zero, especially for the `unsafe fixed char[260]` buffer and the `ref char lpCommandLine`/mutable-buffer `CreateProcessW` signature, neither of which existed before this plan.
- No live verification of any of this plan's three handlers against a real Windows target has been performed (no VM available in this session) — SC#3 (focus-change confirmation via follow-up window-list query) and PROC-01's actual process-launch/enumeration behavior are unverified beyond compile/AOT-publish-clean and the pre-existing `--smoke-test` DTO round-trip. This is expected and owed to 06-05 per the plan's own interface-first, live-gate-at-the-end structure (matches 06-01/06-02/06-03's identical deferral pattern).

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- Plan 06-05 (the Phase 6 live gate) should: (1) run the genuine `win-x64 -p:PublishAot=true --self-contained` publish on a real Windows host, (2) live-verify `get_process_tree()` returns a plausible pid/parent_pid/name/path list, (3) live-verify `set_foreground_window()` + a follow-up `get_window_list()` confirms the focus change (SC#3, Pitfall 5), and (4) live-verify `launch_process()` returns a PID that a follow-up `get_process_tree()` can find.
- All four Phase 6 sensor request handlers (`WindowList` from 06-03, `ProcessTree`/`SetForegroundWindow`/`LaunchProcess` from this plan) now share the identical proven `Envelope.Payload:JsonElement?` AOT pattern, `EnvelopeJsonContext` registration discipline, and try/catch → `success:false` (D-6.4) reply shape — no further sensor-side plumbing generalization is needed for Phase 6's scope.
- The `PROCESSENTRY32W`/`fixed char` fix in this plan is the second time this project has had to deviate from a RESEARCH `[ASSUMED]` code sketch that didn't survive contact with the real LibraryImport source generator (the first being 06-03's `Envelope.Payload` retype itself) — worth flagging as a pattern for future phases: any struct RESEARCH sketch containing a `MarshalAs`-annotated field should be treated as unverified until an actual `dotnet build` confirms it.

---
*Phase: 06-window-process-perception*
*Completed: 2026-07-09*

## Self-Check: PASSED

All created files verified present on disk (`sensor/ProcessEnumeration.cs`, `sensor/WindowControl.cs`, `sensor/ProcessLaunch.cs`); both task commits (`475c3d0`, `11407b6`) verified present in `git log`.
