---
phase: 06-window-process-perception
plan: 03
subsystem: sensor
tags: [csharp, nativeaot, dotnet8, json-source-gen, win32, p-invoke, unmanagedcallersonly, dvc]

# Dependency graph
requires:
  - phase: 06-window-process-perception
    provides: "Plan 06-01's Rust-side generalized DVC plumbing and the authoritative Phase 6 wire contract (snake_case field names, MsgType variants, success/error envelope shape) this plan's C# DTOs mirror byte-for-byte"
  - phase: 05-sensor-bootstrap-deployment
    provides: "sensor/RdpilotSensor.csproj (NativeAOT self-contained win-x64), Envelope/EnvelopeJsonContext/Program.cs scaffolding, [LibraryImport]-only P/Invoke discipline, WTS channel open/read/write/close loop"
provides:
  - "Envelope.Payload retyped object? -> System.Text.Json.JsonElement? (RESEARCH Pitfall 1 / Pattern 6), proven under a real NativeAOT publish with zero reflection fallback BEFORE any Win32 handler code was written"
  - "MsgType gains WindowList/ProcessTree/SetForegroundWindow/LaunchProcess, mirroring the Rust MsgType (06-01)"
  - "sensor/WindowEnumeration.cs: WindowRect/WindowRecord/WindowListResponse DTOs (all registered in EnvelopeJsonContext) + a full EnumWindows-based WindowList handler (geometry/z-order/state/class-name/pid) in physical virtual-desktop pixels"
  - "Program.cs MsgType.WindowList dispatch case, replying success:true/false (D-6.4) and never throwing out of the request loop (T-06-01)"
  - "A --smoke-test CLI path in Program.cs (temporary, this plan's risk-gate artifact) exercising the full DTO -> Envelope.Payload -> wire bytes -> Envelope -> DTO round trip exclusively via source-generated EnvelopeJsonContext.Default.* overloads"
affects: [06-04-window-process-perception, 06-05-window-process-perception]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Envelope.Payload as JsonElement? (not object?) is the AOT-safe polymorphic-payload pattern for this project's NativeAOT + JsonSerializerIsReflectionEnabledByDefault=false constraint (RESEARCH Pattern 6)"
    - "EnumWindows/any future Win32 enumeration callback: static [UnmanagedCallersOnly] method + delegate* unmanaged<...> + GCHandle-boxed accumulator passed through lParam (RESEARCH Pattern 4)"
    - "GetWindowTextW/GetClassNameW via fixed stackalloc Span<char> buffers (RESEARCH Pitfall 3: LibraryImport cannot marshal StringBuilder), bounded to 512 UTF-16 units with truncation treated as benign (Pitfall 4)"
    - "z_order = EnumWindows enumeration index (already front-to-back per the documented Win32 contract) rather than a separate GetWindow(GW_HWNDPREV) walk — the simpler of the plan's two documented-equivalent options"
    - "Handler-level try/catch -> success:false + error message (D-6.4), never throwing out of the request dispatch loop (mirrors ReadEnvelope's T-05-01 drop-never-crash discipline on the response side too)"

key-files:
  created:
    - sensor/WindowEnumeration.cs
  modified:
    - sensor/Envelope.cs
    - sensor/EnvelopeJsonContext.cs
    - sensor/Program.cs

key-decisions:
  - "This execution environment (Fedora Linux, no dotnet SDK preinstalled) is not the ARM64-Windows dev machine referenced elsewhere in STATE.md/06-01-SUMMARY.md. dotnet-sdk-8.0 was installed via dnf (a legitimate Fedora system package, not an arbitrary npm/pip install subject to the slopsquat-package exclusion) purely to compile/verify the C# work; two Fedora dotnet packaging symlinks (/usr/share/dotnet/dotnet, /usr/lib64/dotnet/sdk) had to be created manually because the RPM's postinstall scriptlet path did not match this Fedora 44 dotnet-host layout."
  - "NativeAOT cannot cross-OS-compile (confirmed empirically: 'error : Cross-OS native compilation is not supported' from Microsoft.NETCore.Native.Publish.targets) — the plan's literal verify command (dotnet publish -r win-x64 -p:PublishAot=true --self-contained) cannot produce a real win-x64 native binary on this Linux host. Substituted a same-OS linux-x64 PublishAot=true --self-contained publish as a surrogate: it exercises the identical JSON source-gen / trimming-analysis machinery this task's risk gate is about (Pitfall 1 is a source-gen concern, not a P/Invoke-resolution concern), producing a genuinely AOT-compiled, stripped, self-contained native binary with zero warnings, and the --smoke-test path was executed against that real native binary (not dotnet run/JIT) to prove runtime correctness, not just clean compilation. The genuine win-x64 self-contained AOT publish must still be run on the real Windows dev machine before the Phase 6 live gate (06-05) — this is the same category of environment substitution 06-01-SUMMARY.md documented for the Rust toolchain."
  - "z-order uses the EnumWindows enumeration index directly rather than a GetWindow(GW_HWNDPREV) walk (both were plan-sanctioned options) — EnumWindows already returns top-level windows in front-to-back Z-order per the documented Win32 contract, so the extra P/Invoke surface would have been unused/redundant."
  - "Negative window-rect origins (a real possibility on multi-monitor virtual-desktop layouts where a monitor sits left of/above the primary) clamp to 0 rather than wrapping, because the Phase 6 wire contract's x/y fields are u32 (06-01-PLAN.md, locked) — documented as a known limitation in the code, not silently masked, and out of this plan's scope to change the wire contract itself."

requirements-completed: []  # PERC-02 is an end-user-observable capability that only becomes TRUE once the Phase 6 live gate (06-05) passes — mirrors the Phase 5 convention (SENSOR-01/02 marked Complete only at 05-04's live gate) and 06-01-SUMMARY.md's identical deferral for this same requirement.

# Metrics
duration: ~40min
completed: 2026-07-09
---

# Phase 6 Plan 3: C# Sensor JsonElement Risk Gate + WindowList Handler Summary

**Retyped `Envelope.Payload` from `object?` to `JsonElement?` and proved a typed DTO round-trips under a real NativeAOT publish with zero reflection fallback, then built the `EnumWindows`-based WindowList handler (geometry/z-order/state/class-name/pid, physical pixels, `[LibraryImport]`-only, no COM) on that now-proven serialization path.**

## Performance

- **Duration:** ~40 min (includes one-time `dotnet-sdk-8.0` toolchain installation)
- **Completed:** 2026-07-09
- **Tasks:** 2
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments
- **Task 1 (risk gate, done first):** `Envelope.Payload` is now `System.Text.Json.JsonElement?`; `MsgType` gained `WindowList`/`ProcessTree`/`SetForegroundWindow`/`LaunchProcess`; `WindowRect`/`WindowRecord`/`WindowListResponse` DTOs were defined and registered in `EnvelopeJsonContext`; a temporary `--smoke-test` CLI path proves a full `WindowListResponse` round-trips through the retyped `Payload` (serialize -> attach -> wire bytes -> deserialize -> read back) using only source-generated `EnvelopeJsonContext.Default.*` overloads. This was verified against a **real, stripped, self-contained NativeAOT-compiled binary** (not `dotnet run`/JIT) with zero source-gen/trimming warnings.
- **Task 2:** Implemented the `WindowList` handler in `sensor/WindowEnumeration.cs`: `EnumWindows` via a `static [UnmanagedCallersOnly]` callback + `delegate* unmanaged<>` + a `GCHandle`-boxed `List<nint>` accumulator (no managed-delegate marshalling); `GetWindowRect`/`GetWindowTextW`/`GetClassNameW`/`GetWindowThreadProcessId`/`IsIconic`/`IsZoomed`/`IsWindowVisible`, all `[LibraryImport]`-only against `user32.dll`; title/class reads via fixed 512-UTF-16 `stackalloc` buffers (no `StringBuilder`); state classified `minimized`/`maximized`/`normal`; rects emitted in physical virtual-desktop pixels. `Program.cs` dispatches `MsgType.WindowList` through a handler wrapped in try/catch that degrades to `success:false` + the exception message on any failure, never throwing out of the request loop.
- Re-verified the entire AOT source-gen surface (DTOs + the new Win32 P/Invoke handler together) publishes with **zero warnings** and the smoke test still passes on the rebuilt native binary after Task 2's changes.

## Task Commits

Each task was committed atomically:

1. **Task 1: Retype Envelope.Payload to JsonElement? and prove a typed round-trip under PublishAot** - `648b6f6` (feat)
2. **Task 2: Implement the WindowList handler (EnumWindows + geometry/z-order/state, plain P/Invoke, no COM)** - `5bfce91` (feat)

## Files Created/Modified
- `sensor/Envelope.cs` - `Payload: object? -> JsonElement?`; `MsgType` gains 4 members
- `sensor/EnvelopeJsonContext.cs` - registers `WindowRect`/`WindowRecord`/`WindowListResponse` alongside `Envelope`
- `sensor/WindowEnumeration.cs` (new) - Task 1: the three window DTOs; Task 2 (appended): the `user32.dll` P/Invoke surface, the `[UnmanagedCallersOnly]` `EnumWindows` callback, and `BuildWindowListResponse()`
- `sensor/Program.cs` - `Main(string[] args)` gains a `--smoke-test` path (`RunAotSmokeTest`); dispatch loop gains a `MsgType.WindowList` case wired through `BuildWindowListReplyEnvelope`

## Decisions Made
See `key-decisions` in frontmatter for: the dotnet-sdk-8.0 toolchain install + Fedora symlink fix, the linux-x64 AOT-publish surrogate substitution for the cross-OS-incompatible win-x64 target, the enumeration-index z-order choice, and the multi-monitor negative-origin clamp-to-0 known limitation.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Installed and repaired the dotnet SDK toolchain (not present in this execution environment)**
- **Found during:** Pre-Task-1 environment check
- **Issue:** No `dotnet` binary existed at all in this session's Fedora Linux environment (unlike whatever host the plan's verify commands assume). `sudo dnf install -y dotnet-sdk-8.0` installed the package, but `dotnet --version` still failed with "No .NET SDKs were found" — this Fedora dotnet-host package layout (`/usr/lib64/dotnet`) and the dotnet-sdk-8.0 package's content path (`/usr/share/dotnet/sdk`) are split, and the RPM's own postinstall scriptlet expects a `/usr/share/dotnet/dotnet` muxer symlink that was missing.
- **Fix:** Installed `dotnet-sdk-8.0` via `dnf` (a Fedora system package — not an npm/pip-style arbitrary install subject to the slopsquat exclusion), then created two symlinks: `/usr/share/dotnet/dotnet -> /usr/lib64/dotnet/dotnet` and `/usr/lib64/dotnet/sdk -> /usr/share/dotnet/sdk` (plus `sdk-manifests`/`templates`). `dotnet --list-sdks` then correctly reported `8.0.422`.
- **Files modified:** None (system package + symlinks only, no repo files touched).
- **Verification:** `dotnet build sensor/RdpilotSensor.csproj` succeeded end-to-end for the rest of the session.
- **Committed in:** N/A (environment setup only, not a source/config change)

**2. [Rule 3 - Blocking] Substituted a linux-x64 PublishAot surrogate for the plan's literal win-x64 verify command**
- **Found during:** Task 1's mandated AOT smoke-test verification, before any Win32 code was written
- **Issue:** `dotnet publish sensor/RdpilotSensor.csproj -r win-x64 -p:PublishAot=true --self-contained` fails immediately with `error : Cross-OS native compilation is not supported.` (from `Microsoft.NETCore.Native.Publish.targets`) — this is a hard NativeAOT platform limitation (also documented in the csproj's own comment), not a code defect, and it made the plan's literal `<verify>` grep pipeline for Task 1 a **false positive**: piping the `error` line through `grep -iE "warning|error|IL2|IL3" | grep -iE "envelope|payload|window|reflection"` yields no match (the cross-OS error text doesn't mention those keywords), so the verify command's `|| echo "NO source-gen warnings..."` fallback would print a misleading "clean" result without a publish ever having actually succeeded.
- **Fix:** Ran `dotnet publish sensor/RdpilotSensor.csproj -r linux-x64 -p:PublishAot=true --self-contained -p:RuntimeIdentifier=linux-x64` as a same-OS surrogate. This exercises the exact JSON source-generator / IL-trimming analysis machinery Pitfall 1 is about (a source-gen/reflection-fallback concern, independent of target OS) and produces a genuinely AOT-compiled, stripped, self-contained ELF binary. Ran that real binary's `--smoke-test` path directly (not `dotnet run`, not JIT) to prove runtime correctness, not merely a clean compile. Result: zero warnings on both the Task 1 DTO-only surface and the Task 2 full Win32-handler surface; `--smoke-test` printed `PASS` (exit 0) both times.
- **Files modified:** None (verification methodology only).
- **Verification:** `dotnet publish ... -r linux-x64 ... 2>&1 | grep -iE "warning|error|IL2|IL3"` → no output (genuinely clean, not a false-positive fallback); the published `rdpilot-sensor` binary's `--smoke-test` → `PASS`, exit 0, confirmed via `file` to be a real stripped native ELF, not a managed/JIT artifact.
- **Committed in:** N/A (no source changes — verification methodology note only)

---

**Total deviations:** 2 auto-fixed (both blocking, both environment/verification-methodology, zero source-code deviations from the plan's specified DTOs/handlers/dispatch logic)
**Impact on plan:** No scope creep — both deviations were necessary to run this plan's own required verification in an environment without a genuine Windows host, and neither touched a repository file. The plan's actual code (Envelope.cs, EnvelopeJsonContext.cs, WindowEnumeration.cs, Program.cs) was implemented exactly as specified.

## Issues Encountered
- **The genuine `win-x64` self-contained NativeAOT publish was NOT exercised in this session** — this is the single most important open item from this plan. NativeAOT does not cross-OS-compile (Linux -> Windows), so `dotnet publish -r win-x64 -p:PublishAot=true --self-contained` can only ever be validated on a real Windows host with the native MSVC/Windows SDK toolchain. Everything achievable without that host was done: `dotnet build` targeting `win-x64` compiles clean (proving the win-x64 P/Invoke declarations against `user32.dll` are well-formed at the IL level), and the linux-x64 AOT surrogate proves the JSON source-gen risk gate (Pitfall 1) is resolved with a real native, trimmed, self-contained binary that runs the smoke test correctly. **Before the Phase 6 live gate (06-05), someone with access to the real Windows dev machine should run the literal `dotnet publish sensor/RdpilotSensor.csproj -r win-x64 -p:PublishAot=true --self-contained` and confirm zero warnings** — this closes the one gap this session's environment could not close. Given the linux-x64 surrogate's clean result and the fact that Pitfall 1 is a source-gen concern rather than a P/Invoke-target concern, risk of a genuine win-x64 surprise is assessed as LOW but not yet empirically zero.
- This mirrors 06-01-SUMMARY.md's identical open item for the Rust `x86_64-pc-windows-gnu` build — this execution environment (Fedora Linux) still does not match the ARM64-Windows dev machine STATE.md's build-discipline note describes.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Plan 06-04 (ProcessTree/SetForegroundWindow/LaunchProcess handlers) can build directly on this plan's now-proven `JsonElement?`-typed `Envelope.Payload` pattern, the `MsgType` variants already added, and the same try/catch -> `success:false` (D-6.4) handler-reply discipline established here.
- Plan 06-05 (the Phase 6 live gate) should: (1) run the genuine `win-x64 -p:PublishAot=true --self-contained` publish on the real Windows dev machine per the Issues Encountered note above, and (2) live-verify the WindowList handler's rects genuinely land in physical virtual-desktop pixel space against the framebuffer, per this plan's `<verification>` section.
- The `--smoke-test` CLI path added to `Program.cs` in Task 1 is intentionally temporary/diagnostic (this plan's risk-gate artifact) — it is harmless to leave in place (it only activates on the exact `--smoke-test` argument, otherwise the normal WTS channel loop runs unchanged) but a later plan may choose to remove it once the AOT risk is considered permanently retired.

---
*Phase: 06-window-process-perception*
*Completed: 2026-07-09*

## Self-Check: PASSED

All created/modified files verified present on disk; both task commits (`648b6f6`, `5bfce91`) verified present in `git log`.
