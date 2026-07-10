---
phase: 07-uia-tree-module
plan: 04
subsystem: sensor
tags: [csharp, dotnet8, nativeaot, com-interop, uiautomation, wire-protocol]

# Dependency graph
requires:
  - phase: 07-uia-tree-module (07-01)
    provides: "Rust-side MsgType::Uia, owned UiaElement/UiaElementWire pair, control_type_to_role/runtime_id_to_string, Session::get_uia_tree(hwnd) -- the wire contract this plan's C# DTOs must match exactly"
  - phase: 07-uia-tree-module (07-03)
    provides: "LIVE-VERIFIED sensor/UiaInterop.cs -- ReadRuntimeId (SAFEARRAY) and ReadName (BSTR) leak-safe manual decodes, all four GeneratedComInterface interfaces proven on real Windows"
provides:
  - "sensor/UiaTree.cs: UiaTreeRequest/UiaElementRecord/UiaTreeResponse DTOs + UiaTree.BuildUiaTreeResponse(hwnd) -- the real (non-smoke-test) UIA handler"
  - "sensor/Program.cs: MsgType.Uia dispatch arm + BuildUiaTreeReplyEnvelope(reqId, payload)"
  - "Wire-complete Uia request/response round trip matching 07-01's UiaElementWire field-for-field"
affects: [07-05-live-gate]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "bbox reuses WindowRect (WindowEnumeration.cs) verbatim rather than declaring a second x/y/w/h record -- same namespace, already JSON-registered, identical wire shape"
    - "Root element included as its own flat-array record (depth=0, parent_runtime_id=[]) ahead of its direct children (depth=1) so depth/parent_id are meaningfully populated by a single-scope Children walk (D-7.4)"
    - "Grep-gated doc comments must avoid the literal gated strings even in negative/explanatory prose -- an accept-criteria grep -c ... == 0 counts the whole file, comments included, not just code"

key-files:
  created: []
  modified:
    - sensor/Envelope.cs
    - sensor/UiaTree.cs
    - sensor/EnvelopeJsonContext.cs
    - sensor/Program.cs

key-decisions:
  - "Reused WindowRect for the bbox field rather than declaring a new UiaBboxRecord (07-04-PLAN.md left this to executor discretion) -- same namespace, same x/y/w/h shape 07-01's RectWire expects, already registered in EnvelopeJsonContext, zero duplication."
  - "Split DTO-only Task 1 and handler-plus-dispatch Task 2 into two atomic commits by first writing UiaTree.cs with DTOs only, building/committing, then appending the handler in a second edit -- matches the plan's task boundary even though both pieces were drafted together initially."

requirements-completed: [PERC-03]

# Metrics
duration: ~25min
completed: 2026-07-09
---

# Phase 7 Plan 04: UIA Tree Handler Summary

**Implemented the real `UiaTree.BuildUiaTreeResponse(hwnd)` handler on top of 07-03's live-verified `UiaInterop.cs`: a single `ElementFromHandle` -> `FindAll(TreeScope.Children, trueCondition)` COM call producing a flat root+children `UiaElementRecord[]` in physical-pixel space, with per-element `COMException` skip and top-level degrade, wired into the `MsgType.Uia` dispatch arm -- compiles clean and AOT-trims clean (zero ILC/trim/source-gen warnings) on the linux-x64 offline surrogate.**

## Performance

- **Duration:** ~25 min
- **Tasks:** 2/2 completed
- **Files modified:** 4 (1 created, 3 modified)

## Accomplishments

- `Envelope.cs`: `MsgType.Uia` appended to the enum (after `LaunchProcess`).
- `sensor/UiaTree.cs` (new): `UiaTreeRequest` (`{hwnd}`), `UiaElementRecord` (the full D-7.1 wire field set: `runtime_id`/`control_type`/`name`/`bbox`/`enabled`/`visible`/`focusable`/`focused`/`depth`/`parent_runtime_id`, matching 07-01's `UiaElementWire` byte-for-byte), and `UiaTreeResponse` (`{success,data,error}`) -- bbox reuses `WindowRect` verbatim.
- `UiaTree.BuildUiaTreeResponse(hwnd)`: `UiaInterop.GetRootAutomation()` -> `ElementFromHandle((nint)hwnd)` -> the root's own record (depth=0) -> a single `CreateTrueCondition()` + `FindAll(TreeScope.Children, ...)` scoped call (D-7.4, no `TreeWalker`/recursive tree-walking) -> each child's record (depth=1, `parent_runtime_id` = root's `RuntimeId`). Every property read uses `UiaInterop.ReadRuntimeId`/`ReadName` (07-03's live-diagnosed leak-safe decodes) plus the proven `Get*` BOOL/RECT methods -- naive uncached per-property reads only (D-7.7, no `CreateCacheRequest`/bulk cache request).
- Per-element `COMException` is caught and `continue`d (D-7.6) so one stale/destroyed element cannot sink the response; a separate top-level `try/catch` degrades the WHOLE response to `Success=false` only on total handler failure (D-6.4).
- `EnvelopeJsonContext.cs`: `[JsonSerializable]` entries for `UiaTreeRequest`, `UiaElementRecord`, `UiaTreeResponse` (no reflection JSON under NativeAOT).
- `Program.cs`: `BuildUiaTreeReplyEnvelope(reqId, payload)` (deserialize -> degrade-on-null/exception -> serialize, mirroring `BuildSetForegroundWindowReplyEnvelope`) + the `MsgType.Uia` dispatch arm in `RunHandshakeAndPingPongLoop`.
- `dotnet build` clean (0 warnings/errors); `dotnet publish -r linux-x64 -p:PublishAot=true --self-contained` clean (zero ILC/trim/source-gen warnings) -- offline compile/AOT-trim proof only; the behavioral win-x64 proof is deferred to 07-05's live gate.

## Task Commits

Each task was committed atomically:

1. **Task 1: Add MsgType.Uia + the UIA DTOs + JsonSerializable registrations** - `df8ccc5` (feat) - `sensor/Envelope.cs`, `sensor/UiaTree.cs`, `sensor/EnvelopeJsonContext.cs`
2. **Task 2: Implement BuildUiaTreeResponse and wire the Program.cs dispatch arm** - `fb2cb98` (feat) - `sensor/UiaTree.cs`, `sensor/Program.cs`

## Files Created/Modified

- `sensor/Envelope.cs` - `MsgType` enum gained the `Uia` variant.
- `sensor/UiaTree.cs` (new) - `UiaTreeRequest`/`UiaElementRecord`/`UiaTreeResponse` DTOs + `UiaTree.BuildUiaTreeResponse(hwnd)`/`BuildRecord` handler.
- `sensor/EnvelopeJsonContext.cs` - `[JsonSerializable]` registrations for the three new DTOs.
- `sensor/Program.cs` - `BuildUiaTreeReplyEnvelope(reqId, payload)` + the `MsgType.Uia` dispatch arm.

## Decisions Made

See frontmatter `key-decisions`. Summary: reused `WindowRect` for `bbox` (no new bbox type) as the plan's own reuse-vs-new discretion allowed; split the drafted DTOs+handler into two atomic task commits matching the plan's task boundaries.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Grep-gated doc comments initially tripped their own acceptance-criteria grep gate**
- **Found during:** Task 2, running the acceptance-criteria grep checks (`grep -c "TreeWalker\|GetFirstChildElement" sensor/UiaTree.cs` and `grep -c "CreateCacheRequest" sensor/UiaTree.cs`) before the AOT publish step.
- **Issue:** The plan's own acceptance criteria specify `grep -c "TreeWalker\|GetFirstChildElement" sensor/UiaTree.cs == 0` and `grep -c "CreateCacheRequest" sensor/UiaTree.cs == 0` -- whole-file literal counts, not code-only. My first-draft doc comments explaining the design ("NEVER a TreeWalker recursion", "do NOT add CreateCacheRequest here") used the literal gated strings in explanatory prose, tripping the count to 2 and 1 respectively even though the code itself never calls either API.
- **Fix:** Reworded the three offending comments to convey the same intent without the literal gated substrings (e.g. "tree-walker-style recursion" instead of "`TreeWalker` recursion"; "bulk cache-request retrieval" instead of "`CreateCacheRequest`").
- **Files modified:** `sensor/UiaTree.cs`.
- **Verification:** Re-ran both grep gates -- both return 0; `dotnet build` still clean.
- **Committed in:** `fb2cb98` (Task 2 commit; caught before commit, no separate fix commit needed).

---

**Total deviations:** 1 auto-fixed (Rule 1, cosmetic/wording -- no correctness impact, caught and fixed before the Task 2 commit).
**Impact on plan:** No scope change. The handler's actual behavior (single `FindAll(TreeScope.Children, ...)` scoped call, naive uncached reads, no `CreateCacheRequest`) was correct from the first draft; only the doc-comment wording needed adjustment to satisfy the plan's literal grep gate.

## Issues Encountered

None beyond the deviation documented above.

## User Setup Required

None -- no external service configuration required. This is offline sensor code; the behavioral proof against real UIA elements is 07-05's live gate.

## Next Phase Readiness

- The `Uia` request/response round trip is wire-complete and offline-verified (compile + AOT-trim clean). 07-05's live gate can now call `Session::get_uia_tree(hwnd)` (07-01) end-to-end against a real window (Notepad, per ROADMAP's SC#1 test target) and expect a populated `UiaElement[]` back.
- 07-05 should verify SC#3 (500ms budget for a standard Win32 app's `TreeScope_Children` walk) against this naive-uncached-reads implementation (D-7.7) -- only reach for `CreateCacheRequest` bulk caching if that budget is actually at risk live.
- 07-05 should also be the first DIRECT live exercise of `CurrentHasKeyboardFocus`/`CurrentIsKeyboardFocusable`/`CurrentIsOffscreen` in this handler's own code path (07-03's smoke test only directly exercised `CurrentIsEnabled` among the four BOOL-returning slots, though all four share the identical proven marshalling declaration).
- No known stubs introduced by this plan -- `BuildUiaTreeResponse` is a complete, real handler (not scaffolding), built entirely on 07-03's live-verified interop.

## Threat Flags

None beyond what 07-04-PLAN's own `<threat_model>` already covers (T-07-01, T-07-05, T-07-06, T-07-03, T-07-SC) -- no new network endpoints, auth paths, or trust-boundary-crossing surface was introduced beyond the `Uia {hwnd}` request/response the plan itself specifies.

## Self-Check: PASSED

- FOUND: `sensor/UiaTree.cs` (created, contains `UiaTreeRequest`, `UiaElementRecord`, `UiaTreeResponse`, `UiaTree.BuildUiaTreeResponse`)
- FOUND: `sensor/Program.cs` contains `BuildUiaTreeReplyEnvelope` and the `MsgType.Uia` dispatch arm
- FOUND: `sensor/Envelope.cs` `MsgType` enum contains `Uia`
- FOUND: `sensor/EnvelopeJsonContext.cs` contains `[JsonSerializable(typeof(UiaTreeResponse))]`
- FOUND: commit `df8ccc5` in `git log --oneline`
- FOUND: commit `fb2cb98` in `git log --oneline`
- CONFIRMED: `grep -c "TreeWalker\|GetFirstChildElement" sensor/UiaTree.cs` == 0
- CONFIRMED: `grep -c "CreateCacheRequest" sensor/UiaTree.cs` == 0
- CONFIRMED: `dotnet build sensor/RdpilotSensor.csproj -c Release` -- 0 warnings, 0 errors
- CONFIRMED: `dotnet publish sensor/RdpilotSensor.csproj -c Release -r linux-x64 -p:PublishAot=true --self-contained` -- zero ILC/trim/source-gen warnings

---
*Phase: 07-uia-tree-module*
*Completed: 2026-07-09*
