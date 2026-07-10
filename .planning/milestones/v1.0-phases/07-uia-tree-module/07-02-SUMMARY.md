---
phase: 07-uia-tree-module
plan: 02
subsystem: sensor
tags: [csharp, dotnet8, nativeaot, com-interop, generatedcominterface, uiautomation, win32]

# Dependency graph
requires:
  - phase: 07-uia-tree-module (07-01)
    provides: "Rust-side MsgType::Uia wire scaffolding, Session::get_uia_tree(hwnd) round-trip helper (zero file overlap with this plan)"
  - phase: 06-window-process-perception
    provides: "sensor/WindowEnumeration.cs [LibraryImport] P/Invoke discipline, Rect32 struct shape, sensor/Program.cs's RunAotSmokeTest AOT risk-gate pattern"
provides:
  - "sensor/UiaInterop.cs: four hand-authored [GeneratedComInterface] UIA COM interfaces (IUIAutomation, IUIAutomationElement, IUIAutomationElementArray, IUIAutomationCondition) with GUID-verified, complete in-order vtable slot lists"
  - "UiaInterop.GetRootAutomation() — CoCreateInstance(CLSID_CUIAutomation) via hand-rolled ole32.dll P/Invoke + StrategyBasedComWrappers"
  - "UiaInterop.ReadRuntimeId() — leak-safe manual SAFEARRAY(int) decode via hand-rolled oleaut32.dll P/Invokes"
  - "sensor/Program.cs: --smoke-test-uia arg + RunUiaAotSmokeTest() exercising every risky marshalling path on the desktop root element"
  - "Proof that the sensor still AOT-trims/source-gens clean (linux-x64 surrogate publish, zero ILC/trim warnings)"
affects: [07-03-risk-gate, 07-04-uia-handler]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "GeneratedComInterface members must be declared as Get-prefixed methods, not C# properties (SYSLIB1091 — properties are unsupported on GeneratedComInterface-attributed interfaces)"
    - "Marshal.SafeArrayGet*/SafeArrayDestroy do not exist in .NET Core/.NET 8 — hand-roll the equivalent oleaut32.dll exports via [LibraryImport] instead"
    - "Complete in-order vtable slot declarations (real + never-invoked placeholder methods) required for GeneratedComInterface — no _VTblGap shortcuts (dotnet/runtime#102421)"

key-files:
  created:
    - sensor/UiaInterop.cs
  modified:
    - sensor/Program.cs

key-decisions:
  - "Rect32 struct redeclared as a new top-level type in UiaInterop.cs (matching WindowEnumeration.cs's private Rect32 shape exactly) rather than sharing the existing type, to keep this plan's file scope to UiaInterop.cs/Program.cs only."

requirements-completed: [PERC-03]

# Metrics
duration: ~20min
completed: 2026-07-09
---

# Phase 7 Plan 02: UIA GeneratedComInterface Interop + AOT Smoke-Test Scaffolding Summary

**Hand-authored the four GUID-verified `[GeneratedComInterface]` UIA COM interfaces (19/41/2/0 vtable slots) plus a `--smoke-test-uia` AOT risk-gate entry point that exercises CoCreateInstance/SAFEARRAY/BSTR/BOOL/RECT/FindAll on the desktop root — compiles clean and AOT-trims clean on the linux-x64 offline surrogate; behavioral validation on Windows is deferred to 07-03.**

## Performance

- **Duration:** ~20 min
- **Tasks:** 2/2 completed
- **Files modified:** 2 (1 created, 1 modified)

## Accomplishments
- Hand-ported `IUIAutomation` (19 slots), `IUIAutomationElement` (41 slots), `IUIAutomationElementArray` (2 slots), and `IUIAutomationCondition` (0 slots — empty marker) from `UIAutomationClient.idl`'s GUID-verified member order, with complete in-order placeholder methods for every unused slot (no `_VTblGap` shortcuts).
- `UiaInterop.GetRootAutomation()` obtains the root `IUIAutomation` via a hand-rolled `ole32.dll` `CoCreateInstance` P/Invoke + `StrategyBasedComWrappers` (no CsWin32, no new package).
- `UiaInterop.ReadRuntimeId()` performs a leak-safe manual `SAFEARRAY(int)` decode (`finally`-guarded free).
- `sensor/Program.cs` gained `--smoke-test-uia` / `RunUiaAotSmokeTest()`, a throwaway diagnostic exercising every risky marshalling path on the desktop root element, in risk order (SAFEARRAY first, then BSTR, BOOL, RECT, FindAll).
- Proved via `dotnet build` and a `linux-x64 -p:PublishAot=true --self-contained` surrogate publish that the new interop code compiles and AOT-trims/source-gens with zero warnings.
- `RdpilotSensor.csproj` retains zero `<PackageReference>` entries (D-7.5 preserved).

## Task Commits

Each task was committed atomically:

1. **Task 1: Hand-author UiaInterop.cs** - `7c61812` (feat)
2. **Task 2: Add --smoke-test-uia + RunUiaAotSmokeTest()** - `31b5cf2` (feat)

_No plan-metadata commit was made ahead of this SUMMARY; STATE.md/ROADMAP.md updates and the final docs commit follow this file's creation, per the executor protocol._

## Files Created/Modified
- `sensor/UiaInterop.cs` - Four hand-authored `[GeneratedComInterface]` UIA interfaces, `CoCreateInstance`/`GetRootAutomation`, leak-safe `ReadRuntimeId` SAFEARRAY decode, `TreeScope` flags enum, `Rect32` struct.
- `sensor/Program.cs` - `--smoke-test-uia` arg, `RunUiaAotSmokeTest()`, `User32Interop.GetDesktopWindow` P/Invoke.

## Decisions Made
- `Rect32` is a new top-level struct in `UiaInterop.cs` (identical `[StructLayout(LayoutKind.Sequential)] {int Left,Top,Right,Bottom}` shape to `WindowEnumeration.cs`'s private nested `Rect32`) rather than exposing/reusing the existing type — keeps this plan's file-modification scope to exactly `sensor/UiaInterop.cs`/`sensor/Program.cs` as the plan frontmatter specifies, while still following RESEARCH Open Question #2's "reuse the exact shape" recommendation.
- UIA propget members (`GetLength`, `GetCurrentControlType`, `GetCurrentName`, `GetCurrentHasKeyboardFocus`, `GetCurrentIsKeyboardFocusable`, `GetCurrentIsEnabled`, `GetCurrentIsOffscreen`, `GetCurrentBoundingRectangle`) are declared as `Get`-prefixed methods rather than C# properties (forced by SYSLIB1091 — see Deviations).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] GeneratedComInterface does not support C# instance properties (SYSLIB1091)**
- **Found during:** Task 1 (`dotnet build` immediately after first draft of UiaInterop.cs)
- **Issue:** The plan's action text and RESEARCH's Pattern/Code Examples describe UIA propget members (`Length`, `CurrentControlType`, `CurrentName`, `CurrentHasKeyboardFocus`, `CurrentIsKeyboardFocusable`, `CurrentIsEnabled`, `CurrentIsOffscreen`, `CurrentBoundingRectangle`) as C# properties on the `[GeneratedComInterface]`-attributed interfaces. The .NET 8 ComInterfaceGenerator rejects this outright: `SYSLIB1091: The instance property '...' is declared in the interface '...', which has the 'GeneratedComInterfaceAttribute' applied` — instance properties are unsupported on `[GeneratedComInterface]` interfaces, full stop (not a marshalling-attribute nuance, a hard structural restriction not called out in RESEARCH).
- **Fix:** Converted every propget member to a parameterless `Get`-prefixed method (`GetLength()`, `GetCurrentControlType()`, `GetCurrentName()`, `GetCurrentHasKeyboardFocus()`, `GetCurrentIsKeyboardFocusable()`, `GetCurrentIsEnabled()`, `GetCurrentIsOffscreen()`, `GetCurrentBoundingRectangle()`). Vtable slot order is unaffected (still one slot per member, same declaration order). `[MarshalAs(UnmanagedType.Bool)]` moved from the (now-invalid) property position to `[return: MarshalAs(UnmanagedType.Bool)]` on the corresponding method (CS0592 fix, same root cause).
- **Files modified:** `sensor/UiaInterop.cs`, `sensor/Program.cs` (smoke test call sites updated to method-call syntax).
- **Verification:** `dotnet build sensor/RdpilotSensor.csproj -c Release` — 0 warnings, 0 errors.
- **Committed in:** `7c61812` (Task 1 commit; the smoke-test call-site update landed in `31b5cf2`, Task 2).

**2. [Rule 1 - Bug] `Marshal.SafeArrayGetLBound`/`GetUBound`/`GetElement`/`SafeArrayDestroy` do not exist in .NET Core/.NET 8**
- **Found during:** Task 1 (`dotnet build`, second compile pass after fixing Deviation #1)
- **Issue:** RESEARCH's "Don't Hand-Roll" table and the plan's action text both direct `ReadRuntimeId` to use `System.Runtime.InteropServices.Marshal.SafeArrayGetLBound`/`SafeArrayGetUBound`/`SafeArrayGetElement`/`SafeArrayDestroy`, describing them as "documented static helpers, not part of the AOT-incompatible built-in COM interop system." Confirmed by `dotnet build`: `error CS0117: 'Marshal' does not contain a definition for 'SafeArrayGetLBound'` (and the same for the other three). Verified independently via `strings` against the installed .NET 8 SDK's `System.Runtime.InteropServices.dll` reference assembly — no `SafeArray*` symbols exist at all. These are a .NET-Framework-only surface that was never ported to .NET Core/.NET 5+.
- **Fix:** Hand-rolled the equivalent native OLE Automation exports directly via `[LibraryImport("oleaut32.dll")]`: `SafeArrayGetLBound(nint, uint, out int)`, `SafeArrayGetUBound(nint, uint, out int)`, `SafeArrayGetElement(nint, in int, out int)`, `SafeArrayDestroy(nint)` — consistent with this file's existing `CoCreateInstance` P/Invoke discipline and the project's established "hand-roll native calls with `[LibraryImport]`" convention. `ReadRuntimeId`'s leak-safety contract (T-07-03: always `SafeArrayDestroy` in a `finally`, even on the exception path) is unchanged.
- **Files modified:** `sensor/UiaInterop.cs`.
- **Verification:** `dotnet build sensor/RdpilotSensor.csproj -c Release` — 0 warnings, 0 errors; `dotnet publish -r linux-x64 -p:PublishAot=true --self-contained` — clean, zero ILC/trim/source-gen warnings.
- **Committed in:** `7c61812` (Task 1 commit).

---

**Total deviations:** 2 auto-fixed (both Rule 1 — code as researched/planned does not compile under the actual .NET 8 SDK; both are genuine, empirically-discovered facts about the source generator and BCL surface, not architectural changes). RESEARCH itself flagged this exact territory as MEDIUM-LOW confidence pending the AOT smoke-test gate — this plan's offline `dotnet build`/`dotnet publish` gate did exactly the job it was designed to do, one gate earlier than the live-behavioral 07-03 gate.
**Impact on plan:** No scope change. Both fixes are mechanical substitutions within the same file/task boundaries the plan specified; the resulting vtable slot layout, GUIDs, and interface shapes are unchanged from RESEARCH's Hand-Authoring Reference.

## Issues Encountered
None beyond the two deviations documented above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness

`sensor/UiaInterop.cs` and `--smoke-test-uia` are ready for the 07-03 live Windows risk gate. That gate must:
- Publish (win-x64, `PublishAot=true`, self-contained) and RUN `rdpilot-sensor.exe --smoke-test-uia` on a genuine Windows host (Azure VM, per the established Phase 6 `az vm run-command invoke` pattern) — this plan's linux-x64 surrogate proves compile/AOT-trim cleanliness only; it cannot execute `CoCreateInstance`/UIA (RESEARCH Pitfall 7).
- Empirically resolve RESEARCH Assumptions A1 (SAFEARRAY decode correctness), A2 (BSTR/`StringMarshalling.Utf16` on `GetCurrentName`), and A3 (`[MarshalAs(UnmanagedType.Bool)]` on the four BOOL-returning `Get*` methods) — all are first-attempt strategies with documented `[PreserveSig]`-raw-value fallbacks noted in-line, not yet validated.
- Confirm `GUID`/slot-order correctness end-to-end (a mismatch self-corrects as `E_NOINTERFACE`/`COMException` at `GetRootAutomation()` or `ElementFromHandle`, per RESEARCH A4 — should surface immediately, not silently).

No blockers. `sensor/RdpilotSensor.csproj` still has zero `<PackageReference>` entries.

---
*Phase: 07-uia-tree-module*
*Completed: 2026-07-09*

## Self-Check: PASSED

- FOUND: sensor/UiaInterop.cs
- FOUND: sensor/Program.cs
- FOUND: commit 7c61812
- FOUND: commit 31b5cf2
- FOUND: .planning/phases/07-uia-tree-module/07-02-SUMMARY.md
