---
phase: 05-sensor-bootstrap-deployment
plan: 01
subsystem: sensor
tags: [dotnet8, nativeaot, libraryimport, wts, p-invoke, csharp]

# Dependency graph
requires:
  - phase: 04-dvc-transport-channel
    provides: "RDPILOT_SENSOR DVC envelope protocol (Version/Ping/Pong, req_id-correlated) this sensor implements the server side of"
provides:
  - "sensor/RdpilotSensor.csproj -- NativeAOT self-contained win-x64 console project (PublishAot=true, SelfContained=true, JSON source-gen, AllowUnsafeBlocks)"
  - "sensor/Program.cs -- WTS LibraryImport P/Invoke surface (Utf8/ANSI marshalling), bounded open-retry loop, framing-prefix-strip read loop, Version handshake + req_id-correlated Ping->Pong"
  - "sensor/Envelope.cs + sensor/EnvelopeJsonContext.cs -- AOT-safe source-generated JSON envelope matching the Rust wire shape byte-for-byte"
  - "Confirmed NativeAOT win-x64 published binary: 2,699,264 bytes (~2.57 MiB), no external .NET runtime dependency (SC1), SHA256-identical VM-built vs locally-retrieved"
affects: ["05-04-PLAN", "any future sensor module addition (window/process/UIA in Phase 6/7)"]

# Tech tracking
tech-stack:
  added: [".NET 8 SDK (NativeAOT publish, win-x64)", "wtsapi32.dll P/Invoke via LibraryImport"]
  patterns:
    - "LibraryImport-only P/Invoke (never DllImport) for AOT-safe source-generated marshalling (T-05-02); AllowUnsafeBlocks permits only the generator's own emitted pinned-pointer code for byte[] buffer params, not hand-written unsafe"
    - "JsonSerializerContext source-generation (EnvelopeJsonContext) instead of reflection-based JsonSerializer.Serialize<T> -- required for AOT trimming safety"
    - "Read loop scans forward for the first '{' byte before parsing JSON, discarding a DVC-framing binary prefix (byte-for-byte reuse of the Phase-4 PowerShell fixture's empirically-proven behavior, D-5.7); JsonException is caught and dropped per-message, never terminating Main's loop"

key-files:
  created:
    - sensor/RdpilotSensor.csproj
    - sensor/Program.cs
    - sensor/Envelope.cs
    - sensor/EnvelopeJsonContext.cs
    - sensor/.gitignore
  modified:
    - .gitignore

key-decisions:
  - "Task 1 checkpoint:decision resolved: publish runs on the Azure Windows VM (Phase 1 target), not the operator's ARM64 workstation -- sidesteps any ARM64-host win-x64 native-toolchain cross-targeting risk; the exe never needs to leave the machine before the RDPDR copy step."
  - "StringMarshalling resolved empirically (live, during Task 3 publish/run): WTSVirtualChannelOpenEx is ANSI-only -- wtsapi32.dll exports no W entry point. StringMarshalling.Utf16 caused the LibraryImport source generator to target a nonexistent WTSVirtualChannelOpenExW, silently failing every open attempt (the sensor process started, exhausted its retry budget, and exited with no DVC ever appearing on the wire). Flipped to StringMarshalling.Utf8 (the ANSI 'A' entry point) -- commit f4a48f3 -- matching the Phase-4 PowerShell fixture's proven CharSet.Ansi convention, resolving RESEARCH Open Question #3 / Assumption A3 with the ANSI branch, not Utf16."
  - "AllowUnsafeBlocks added to the csproj (commit 2c1237a): the LibraryImport source generator emits pinned-pointer marshalling code for the byte[]-buffer WTSVirtualChannelRead/Write signatures, which only compiles under /unsafe -- CS0227/SYSLIB1062 without it. LibraryImport remains the sole AOT-safe P/Invoke mechanism (T-05-02 unaffected); this flag only permits the generator's own emitted code, not hand-written unsafe."
  - "csproj doc-comment reworded (commit 491fdbb) to avoid a bare XML double-hyphen inside a comment body (XML 1.0 sec 2.5) -- MSBuild's XML parser rejected the whole .csproj with MSB4025 when the comment literally contained `--self-contained`."
  - "D-5.3 binary-size unknown resolved: NativeAOT win-x64 self-contained publish measures 2,699,264 bytes (~2.57 MiB) -- at the low end of the RESEARCH-flagged 5-30+ MiB range, comfortably supporting the SC4 1-second cold-start-to-pong budget. SHA256 verified identical between the VM-built artifact and the locally-retrieved copy (supply-chain integrity check, no tampering in transit)."

patterns-established:
  - "Windows-only build/publish steps in this project run on the Azure VM build host, not the Linux authoring sandbox or the ARM64 workstation -- established as the default NativeAOT build location for this phase and any future .NET sensor work."

requirements-completed: [SENSOR-01]

# Metrics
duration: ~55min (Task 2 authoring ~15min offline; Task 3 live publish + 3 live-run bug fixes spanning 2026-07-09T13:27:54+02:00 -> 2026-07-09T14:20:10+02:00)
completed: 2026-07-09
---

# Phase 5 Plan 1: C# NativeAOT Sensor (Version/Ping/Pong Server) Summary

**Self-contained NativeAOT `rdpilot-sensor.exe` (2.57 MiB, win-x64, no external .NET runtime) implementing the RDPILOT_SENSOR DVC server side via LibraryImport WTS P/Invoke with ANSI (Utf8) marshalling, req_id-correlated Version/Ping/Pong over source-generated JSON.**

## Performance

- **Duration:** ~55 min total (offline authoring ~15 min; live publish + 3 bug-fix commits ~40 min)
- **Tasks:** 3 (1 checkpoint:decision, 1 auto, 1 checkpoint:human-verify) -- all resolved/complete
- **Files modified:** 6 (5 created, 1 modified) + 3 live-run fix commits touching `sensor/RdpilotSensor.csproj` (x2) and `sensor/Program.cs`

## Accomplishments

- Task 1 (checkpoint:decision): build host resolved to the Azure Windows VM (not the ARM64 workstation), sidestepping cross-targeting risk entirely.
- Task 2 (offline, greenfield): authored `sensor/RdpilotSensor.csproj` (PublishAot, SelfContained, win-x64, InvariantGlobalization, no NuGet packages), `sensor/Envelope.cs` (byte-for-byte Rust wire-shape match: `version`/`req_id`/`type`/`payload`), `sensor/EnvelopeJsonContext.cs` (AOT source-gen JSON), and `sensor/Program.cs` (LibraryImport WTS surface, bounded ~20-attempt/500ms open-retry poll, framing-prefix-strip read loop, Version-first handshake then req_id-correlated Ping->Pong, `JsonException`-scoped catch that never terminates `Main`).
- Task 3 (live, SC1 gate): ran `dotnet publish -c Release -r win-x64 -p:PublishAot=true --self-contained` on the Azure VM. Exit 0; publish directory contains only `rdpilot-sensor.exe` + `.pdb` (no hostfxr/hostpolicy/coreclr companion); the exe runs with no .NET runtime installed. Binary size 2,699,264 bytes (~2.57 MiB). SHA256 verified identical between the VM-built artifact and the locally-retrieved copy.
- Three live-run bugs found and fixed during the Task 3 publish/run cycle (all offline-tested afterward, part of the 72/72 passing offline suite carried into 05-04): invalid XML comment (`491fdbb`), missing `AllowUnsafeBlocks` for LibraryImport byte[] marshalling (`2c1237a`), and the StringMarshalling Utf16->Utf8 flip once the ANSI-only nature of `WTSVirtualChannelOpenEx` was confirmed live (`f4a48f3`).

## Task Commits

Each task was committed atomically:

1. **Task 2: Author the C# NativeAOT sensor project** - `1fe5582` (feat) - `sensor/RdpilotSensor.csproj`, `Program.cs`, `Envelope.cs`, `EnvelopeJsonContext.cs`, `sensor/.gitignore`, root `.gitignore`
2. **Task 3 live-run fix 1: invalid XML comment** - `491fdbb` (fix) - `sensor/RdpilotSensor.csproj`
3. **Task 3 live-run fix 2: AllowUnsafeBlocks for LibraryImport byte[] marshalling** - `2c1237a` (fix) - `sensor/RdpilotSensor.csproj`
4. **Task 3 live-run fix 3: StringMarshalling Utf16 -> Utf8** - `f4a48f3` (fix) - `sensor/Program.cs`

## Files Created/Modified

- `sensor/RdpilotSensor.csproj` - NativeAOT self-contained win-x64 console project config, AllowUnsafeBlocks, no NuGet package refs
- `sensor/Program.cs` - `Wts` LibraryImport P/Invoke class (Utf8 marshalling), open-retry poll loop, framing-prefix-strip read loop, Version/Ping/Pong handshake logic
- `sensor/Envelope.cs` - `Envelope` record + `MsgType` string values matching the Rust `sensor.rs` wire shape
- `sensor/EnvelopeJsonContext.cs` - `JsonSerializerContext` source-generation for AOT-safe `Envelope` (de)serialization
- `sensor/.gitignore`, root `.gitignore` - `bin/`/`obj/` exclusions for the new `sensor/` project

## Decisions Made

See frontmatter `key-decisions` for the full list. Summary: build host = Azure VM (Task 1); StringMarshalling resolved to Utf8/ANSI (WTSVirtualChannelOpenEx has no W export, confirmed live); AllowUnsafeBlocks added for the LibraryImport source generator's own pinned-pointer marshalling code (not a relaxation of the LibraryImport-only P/Invoke discipline); D-5.3 binary-size unknown resolved at 2.57 MiB.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Invalid XML double-hyphen in csproj comment**
- **Found during:** Task 3 (live NativeAOT publish)
- **Issue:** The doc comment's inline `--self-contained` literal contained a bare XML double-hyphen, which XML 1.0 sec 2.5 forbids anywhere inside a comment body; MSBuild's XML parser rejected the whole `.csproj` with MSB4025.
- **Fix:** Reworded the comment to avoid the double-hyphen without changing its meaning.
- **Files modified:** `sensor/RdpilotSensor.csproj`
- **Committed in:** `491fdbb`

**2. [Rule 1 - Bug] Missing AllowUnsafeBlocks for LibraryImport byte[] marshalling**
- **Found during:** Task 3 (live NativeAOT publish)
- **Issue:** The LibraryImport source generator emits pinned-pointer marshalling code for the byte[]-buffer `WTSVirtualChannelRead`/`Write` P/Invoke signatures, which only compiles under `/unsafe`; publish failed with CS0227/SYSLIB1062.
- **Fix:** Added `<AllowUnsafeBlocks>true</AllowUnsafeBlocks>` to the csproj. LibraryImport remains the sole P/Invoke mechanism (T-05-02 unaffected) — this only permits the generator's own emitted unsafe code.
- **Files modified:** `sensor/RdpilotSensor.csproj`
- **Committed in:** `2c1237a`

**3. [Rule 1 - Bug] WTSVirtualChannelOpenEx StringMarshalling.Utf16 silently failed**
- **Found during:** Task 3 (live NativeAOT publish/run)
- **Issue:** `WTSVirtualChannelOpenEx` is an ANSI-only Win32 API — `wtsapi32.dll` exports no wide-string (`W`) variant. `StringMarshalling.Utf16` caused the LibraryImport source generator to target a nonexistent `WTSVirtualChannelOpenExW` entry point, silently failing every open attempt: the process started, exhausted its retry budget, and exited, with the `RDPILOT_SENSOR` DVC never appearing on the client wire trace.
- **Fix:** Flipped to `StringMarshalling.Utf8` (the ANSI `A` entry point), matching the Phase-4 PowerShell fixture's proven-live `CharSet.Ansi` convention — the fallback branch RESEARCH Open Question #3 / Assumption A3 already anticipated.
- **Files modified:** `sensor/Program.cs`
- **Committed in:** `f4a48f3`

---

**Total deviations:** 3 auto-fixed (all Rule 1 — bugs surfaced only by the live NativeAOT publish/run, none discoverable in the offline Linux sandbox).
**Impact on plan:** All three fixes were required for the sensor to build and open the DVC channel at all; no scope creep. Two further live-diagnosed bugs affecting this plan's surface (`rdpsnd` stub requirement, RDPDR IRP gaps) surfaced during Plan 04's live gate and are recorded in `05-04-SUMMARY.md`.

## Issues Encountered

None beyond the three auto-fixed live-run bugs above.

## User Setup Required

None beyond the Task 1 checkpoint (Windows host + .NET 8 SDK selection, resolved to the Azure VM) — already satisfied and confirmed live during Task 3.

## Next Phase Readiness

- SENSOR-01 fully satisfied: `rdpilot-sensor.exe` is a real, live-published, live-run NativeAOT self-contained binary (2.57 MiB) with no external .NET runtime dependency, and it answers the RDPILOT_SENSOR Version/Ping/Pong protocol correctly (proven end-to-end by Plan 04's live gate).
- D-5.3 (binary size, 5-30+ MiB unknown) resolved: 2,699,264 bytes.
- No known stubs; Version/Ping/Pong-only scope was intentional for this phase (D-5.4, no COM/UIA yet — deferred to Phase 6/7).

## Self-Check: PASSED

All 5 claimed created files exist on disk (`sensor/RdpilotSensor.csproj`, `sensor/Program.cs`, `sensor/Envelope.cs`, `sensor/EnvelopeJsonContext.cs`, `sensor/.gitignore`); all 4 claimed commit hashes (`1fe5582`, `491fdbb`, `2c1237a`, `f4a48f3`) are present in `git log --oneline --all`.

---
*Phase: 05-sensor-bootstrap-deployment*
*Completed: 2026-07-09*
