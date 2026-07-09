# Phase 7: UIA Tree Module - Research

**Researched:** 2026-07-09
**Domain:** COM interop under C# .NET 8 NativeAOT (`[GeneratedComInterface]`), Windows UI Automation (`IUIAutomation`)
**Confidence:** MEDIUM-HIGH (the wire-protocol/Rust side is HIGH — pure extension of a proven pattern; the COM-under-AOT mechanics are MEDIUM — every fact below is sourced from official docs/the real `UIAutomationClient.idl`/`windows-rs` metadata mirror, but no end-to-end "IUIAutomation + `[GeneratedComInterface]` + NativeAOT" worked example was found anywhere on the public web — this combination appears genuinely novel; the AOT smoke-test gate D-7.5 mandates is the correct and only way to de-risk this before real handler code)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-7.1 (`UiaElement` field set):** `id`, `role`, `name`, `bbox` (physical virtual-desktop pixels, `u32 x/y/w/h` matching `crate::Rect`), `enabled`, `visible`, `focusable`, `focused`, `depth`, `parentId`. `value` (`ValuePattern`) is DEFERRED to backlog.
- **D-7.2 (`id` mapping):** `id` = stringified UIA `RuntimeId` (join the int array to a stable string). `RuntimeId` chosen over `AutomationId` because `AutomationId` is empty on non-instrumented Win32 apps like Notepad — the literal SC#1 test target.
- **D-7.3 (`role` mapping):** `role` = UIA `ControlType` id mapped to a stable English friendly string (e.g. `UIA_ButtonControlTypeId` → `"Button"`). NOT `LocalizedControlType`.
- **D-7.4 (tree scope / depth representation):** Tree scope = `TreeScope_Children` ONLY, fixed (no `max_depth` parameter in v1). A single scoped COM call, not `TreeWalker` recursion. `depth`/`parentId` are flat-array bookkeeping fields that future-proof the wire shape for a later phase. Caller-configurable `max_depth` is an explicit BACKLOG item.
- **D-7.5 (COM interop approach — highest-risk entry condition):** SPIKE-FIRST with C# .NET 8 `[GeneratedComInterface]`, hand-authoring the `IUIAutomation`/`IUIAutomationElement`/`IUIAutomationTreeWalker` interface declarations (no official Microsoft-shipped bindings exist — RESEARCH confirmed this). Gate on a throwaway `dotnet publish -p:PublishAot=true` smoke test BEFORE any real UIA handler code is written, mirroring `sensor/Program.cs`'s `RunAotSmokeTest`. Do NOT use `System.Windows.Automation` (rejected — reflection-heavy, AOT-incompatible) and do NOT use `ComImport` (rejected — documented not-AOT/trim-safe). CsWin32 considered and NOT chosen (avoids a new build-time NuGet dependency).
- **D-7.6 (per-element error/degrade behavior):** Per-element failure = skip-the-failing-element, don't sink the response. Mirrors the Phase 6 `WindowEnumeration` per-window defensive skip pattern for `COMException` `UIA_E_ELEMENTNOTAVAILABLE`. The top-level try/catch still degrades the WHOLE response to `{success:false,error}` only on total handler failure (D-6.4 contract, unchanged).
- **D-7.7 (latency approach):** Naive uncached per-property COM reads first; verify against the SC#3 500ms budget empirically at the live gate. Reach for `IUIAutomation::CreateCacheRequest` bulk-cached retrieval ONLY if the live gate shows the 500ms bound is actually at risk for Notepad.

### Claude's Discretion

- Exact `IUIAutomation`/`IUIAutomationElement` interface member subset hand-authored in the `[GeneratedComInterface]` declarations (only what D-7.1's field set requires). **This research resolves this discretion — see the "Hand-Authoring Reference" section below for the exact slot lists.**
- Exact wire-contract JSON key names for the new `Uia`/`UiaElement` payload (follow the established snake_case convention).
- Whether `RuntimeId`-to-string join uses a delimiter like `.` or `-` (D-7.2) — any stable, deterministic format is acceptable. **This research recommends `-`** (see Code Examples below).

### Deferred Ideas (OUT OF SCOPE)

- **`value` / `ValuePattern` field** on `UiaElement` — deferred to backlog (D-7.1). Requires per-control-pattern branching logic in the handler; explicitly excluded from this phase's field set.
- **Caller-configurable `max_depth` parameter / deeper tree scopes than `TreeScope_Children`** — deferred to backlog (D-7.4). Revisit only if Phase 9's scripted proof harness needs a deeper tree than a single children-level walk provides.
- `WorldState` correlation of screenshot + window list + UIA snapshot — Phase 8 (API-02).
- The public typed `Session` API surface hiding all sensor-protocol details — Phase 8 (API-01).
- The scripted end-to-end proof harness — Phase 9 (PROOF-01).

</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-------------------|
| PERC-03 | User can retrieve the UI Automation tree as a flat `UiaElement[]` (id, role, name, bbox, enabled, visible, focusable, focused, value?, depth, parentId) — `value?` deferred per D-7.1 | The "Hand-Authoring Reference" section provides the exact `[GeneratedComInterface]` interface/slot/GUID list needed to read every D-7.1 field except `value`; "Pattern 1" shows the `Session::get_uia_tree` extension mirroring the proven Phase 6 round-trip; the ControlType→role mapping table and RuntimeId→id join resolve D-7.2/D-7.3 concretely; Common Pitfalls #2/#6 and the AOT smoke-test Code Example directly address D-7.5's highest-risk entry condition and D-7.6's per-element skip requirement |

</phase_requirements>

## Summary

Phase 7 adds one new sensor capability, `get_uia_tree(hwnd)`, on top of an already-proven wire protocol (Phase 4's envelope, Phase 6's request/response round-trip and per-element defensive-skip patterns). The Rust side is almost entirely mechanical: add `MsgType::Uia`, add `UiaElement`/`UiaElementWire` mirroring `WindowInfo`/`WindowInfoWire`, add `Session::get_uia_tree(hwnd)` mirroring `get_window_list`. None of that is a research risk — it is Phase 6's pattern applied once more.

The entire risk of this phase is concentrated in one place: making `IUIAutomation` COM interop compile and run under `dotnet publish -p:PublishAot=true` using `[GeneratedComInterface]` (System.Runtime.InteropServices.Marshalling), because classic `System.Windows.Automation` and `[ComImport]` are both confirmed AOT-incompatible (D-7.5), and CsWin32 was explicitly rejected. **No official Microsoft-shipped `[GeneratedComInterface]` UIA bindings exist** — confirmed by exhaustive search; the only pre-built UIA interop surfaces on NuGet (`Interop.UIAutomationClient`) are classic TLB-imported `[ComImport]` interfaces, which are NOT AOT-safe and must not be used. The vtable-shaped interface declarations (`IUIAutomation`, `IUIAutomationElement`, `IUIAutomationElementArray`, `IUIAutomationCondition`) must be hand-authored from the real `UIAutomationClient.idl`, which this research fetched directly and used to produce exact, ordered, GUID-verified method lists below.

The single most load-bearing technical fact this research surfaces: **`[GeneratedComInterface]` assigns vtable slots by C# declaration order, and .NET 8's source generator does NOT support `_VTblGapN_M()` placeholder shortcuts** (confirmed regression, tracked as [dotnet/runtime#102421](https://github.com/dotnet/runtime/issues/102421), unresolved as of this research). This means every interface must be declared as a **complete, in-order slot list from slot 1 through the last member actually needed**, using real typed methods for members Phase 7 calls and cheap placeholder methods (any compatible signature, never invoked) for every intervening slot. This research computed the exact ordered slot lists for all four interfaces needed and confirms the total footprint is small: **19 slots for `IUIAutomation`, 41 for `IUIAutomationElement`, 2 for `IUIAutomationElementArray`, 0 for `IUIAutomationCondition`** (an opaque pass-through pointer with no called members). `IUIAutomationTreeWalker` is **not needed at all** — `IUIAutomation::FindAll(TreeScope_Children, trueCondition)` returns a flat one-level array in a single COM call, which is both simpler and a better match for D-7.4's "single scoped COM call, not TreeWalker recursion" requirement than a `TreeWalker::GetFirstChildElement`/`GetNextSiblingElement` walk would be.

**Primary recommendation:** Hand-author the four interfaces above with `[GeneratedComInterface]` + `[Guid(...)]` using the exact slot orders and GUIDs in this document (sourced directly from `UIAutomationClient.idl` and cross-verified against the `windows-rs` win32metadata mirror), obtain the root `IUIAutomation` via a hand-rolled `ole32.dll` `CoCreateInstance` P/Invoke + `StrategyBasedComWrappers.GetOrCreateObjectForComInstance` (no CsWin32), and gate all of it behind a `--smoke-test-uia` AOT publish smoke test — mirroring `sensor/Program.cs`'s existing `RunAotSmokeTest` — before writing the real `UiaTree` handler. Treat `GetRuntimeId`'s `SAFEARRAY(int)` return as the second-highest risk item after the vtable mechanics themselves; no confirmed working pattern for `SAFEARRAY` under `[GeneratedComInterface]` was found, so this must be validated live inside the same smoke test, not assumed.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| UIA COM interop (CoCreateInstance, IUIAutomation calls) | Remote Sensor (C# NativeAOT, in-session) | — | UI Automation is a COM subsystem that only works inside the interactive session hosting the target window; must run on the C# sensor, never the Rust client |
| Tree-scoped element enumeration (`FindAll(TreeScope_Children, ...)`) | Remote Sensor | — | Same reason — COM object lifetime is tied to the sensor process's session |
| RuntimeId→string / ControlType→friendly-string mapping | Remote Sensor (raw data) OR SDK (owned type conversion) | Both | The C# sensor could pre-map ControlType ids to strings before sending, OR ship raw ints and let the Rust `*Wire::into_owned()` conversion do it (mirrors D-6.3's WindowState pattern: sensor sends a small lowercase string, Rust maps to an enum-like owned field). **Recommendation below: sensor sends the friendly string directly** — consistent with existing `WindowState` wire convention (`"normal"`/`"minimized"`/`"maximized"` strings, not raw enum ints) |
| Coordinate-space alignment (`bbox` in physical virtual-desktop pixels) | Remote Sensor (produces the values) | SDK (never remaps) | Established in Phase 6 (D-6.3): sensor emits already-physical pixels; SDK never applies DPI scaling. `RECT`→`Rect{u32 x,y,w,h}` conversion in the sensor must follow the exact `WindowRect` pattern already in `WindowEnumeration.cs` |
| Wire envelope / request-response round trip / correlation | Rust SDK (`Session`, `sensor.rs`, `session_loop.rs`) | C# Sensor (`Program.cs` dispatch) | Fixed since Phase 4; `Uia` is one more `MsgType` value riding the existing generic fulfilment path (RESEARCH Pattern 1, established Phase 6) — zero framing changes needed |
| Public typed `UiaElement` SDK type | Rust SDK (`perception.rs`) | — | Mirrors `WindowInfo`/`ProcessInfo` exactly (D-09 owned-types-only) |

## Standard Stack

### Core

| Component | Version | Purpose | Why Standard |
|-----------|---------|---------|---------------|
| .NET SDK | 8.0.422 (confirmed installed on this dev host) | Sensor build/publish toolchain | Locked since Phase 5 (SENSOR-01); `[GeneratedComInterface]` (`System.Runtime.InteropServices.Marshalling`) ships in .NET 8, no package needed |
| `System.Runtime.InteropServices.Marshalling` | Built into .NET 8 SDK (no NuGet package) | `[GeneratedComInterface]`, `[GeneratedComClass]`, `StrategyBasedComWrappers` | The AOT-safe, trim-safe COM interop source generator — this is the ONLY viable mechanism per D-7.5 (rejects `System.Windows.Automation` and `ComImport`) [CITED: learn.microsoft.com/en-us/dotnet/standard/native-interop/comwrappers-source-generation] |
| `UIAutomationClient.idl` (Windows SDK header, hand-ported, NOT a package) | Windows 10 SDK 10.0.14393.0 vintage (interfaces/GUIDs listed below are stable since Windows 7 and unchanged through current Win11 SDKs) | Authoritative source for interface member order + GUIDs | No NuGet/official `[GeneratedComInterface]` UIA bindings exist [VERIFIED: exhaustive WebSearch across Microsoft Learn, dotnet/runtime issues, NuGet — confirmed absent] |

**No new `<PackageReference>` is required** — matches the phase's locked constraint (`RdpilotSensor.csproj` has zero package references; D-7.5 explicitly avoids CsWin32 to keep it that way).

### Alternatives Considered (all rejected per D-7.5, documented here so they are not re-proposed)

| Instead of | Could Use | Why Not |
|------------|-----------|---------|
| Hand-authored `[GeneratedComInterface]` | `Interop.UIAutomationClient` NuGet package | Classic TLB-imported `[ComImport]` interfaces — NOT AOT-safe (`ComImport` uses the runtime IL-stub COM interop system, incompatible with NativeAOT) [VERIFIED: nuget.org package description + `[ComImport]` AOT-incompatibility confirmed in Microsoft's own ComWrappers docs] |
| Hand-authored `[GeneratedComInterface]` | `System.Windows.Automation` (WPF UIA managed wrapper) | Reflection-heavy managed assembly, explicitly rejected by D-7.5, not designed for trimming/AOT |
| Hand-authored `[GeneratedComInterface]` | CsWin32 source generator | D-7.5 explicitly avoids adding a new build-time NuGet dependency; CsWin32 does generate `[GeneratedComInterface]`-style bindings for some Win32 surfaces but UIA coverage/AOT-readiness was not independently verified and is out of scope per the locked decision |

## Package Legitimacy Audit

No new packages are installed this phase (zero `<PackageReference>` entries added — `[GeneratedComInterface]` is a built-in .NET 8 SDK feature, not a NuGet package). `slopcheck`/registry verification is N/A.

| Package | Registry | Disposition |
|---------|----------|-------------|
| — none — | — | No install step this phase |

## Architecture Patterns

### System Architecture Diagram

```
Rust SDK (Session::get_uia_tree(hwnd))
   │
   │ 1. sensor_request(MsgType::Uia, {hwnd}, ENUMERATION_TIMEOUT_MS)
   │    (existing Phase-6 round-trip helper: alloc req_id, register
   │    oneshot in SensorShared::pending, send RdpInputEvent::Request,
   │    tokio::time::timeout)
   ▼
RDPILOT_SENSOR DVC (existing envelope, unchanged)
   │
   ▼
C# Sensor Program.cs dispatch loop
   │
   │ 2. new `else if (msg.Type == MsgType.Uia)` arm
   │    → BuildUiaTreeReplyEnvelope(msg.ReqId, msg.Payload)
   ▼
UiaTree.cs handler (NEW this phase)
   │
   │ 3. deserialize request payload {hwnd}
   │ 4. UiaInterop.GetRootAutomation()  ── CoCreateInstance(CLSID_CUIAutomation,
   │                                        IID_IUIAutomation) via hand-rolled
   │                                        ole32.dll P/Invoke + StrategyBasedComWrappers
   │ 5. automation.ElementFromHandle(hwnd)   → root IUIAutomationElement
   │ 6. automation.CreateTrueCondition()     → IUIAutomationCondition
   │ 7. root.FindAll(TreeScope_Children, trueCondition) → IUIAutomationElementArray
   │    (ONE COM call — no TreeWalker recursion, D-7.4)
   │ 8. for i in 0..array.Length:
   │      element = array.GetElement(i)
   │      try { read 6 properties + GetRuntimeId } catch (COMException) { skip, D-7.6 }
   │ 9. map ControlType id → friendly string (table below)
   │ 10. RECT → {x,y,w,h} (u32, physical pixels — same conversion as WindowRect)
   ▼
WindowListResponse-style {success:true, data:[UiaElementRecord,...]} JSON
   │
   ▼ (back over DVC, existing generic fulfilment path — Rust deserializes
      into UiaElementWire → UiaElement, D-09 owned types)
Rust SDK: Vec<UiaElement> returned to caller
```

### Recommended Project Structure (new files only)

```
sensor/
├── UiaInterop.cs          # NEW: hand-authored [GeneratedComInterface] declarations
│                           #   (IUIAutomation, IUIAutomationElement,
│                           #   IUIAutomationElementArray, IUIAutomationCondition),
│                           #   CoCreateInstance P/Invoke, GetRootAutomation() helper,
│                           #   ControlType id → friendly-string table
├── UiaTree.cs              # NEW: request/response DTOs (mirrors WindowEnumeration.cs
│                           #   shape) + BuildUiaTreeResponse(hwnd) handler
├── Program.cs               # MODIFIED: += MsgType.Uia dispatch arm,
│                           #   += `--smoke-test-uia` arg (mirrors --smoke-test)
├── Envelope.cs              # MODIFIED: += MsgType.Uia
├── EnvelopeJsonContext.cs   # MODIFIED: += [JsonSerializable] entries for the new DTOs

crates/rdpilot/src/
├── sensor.rs                # MODIFIED: += MsgType::Uia
├── perception.rs            # MODIFIED: += UiaElement, UiaElementWire (mirrors
│                           #   WindowInfo/WindowInfoWire exactly)
├── session.rs                # MODIFIED: += Session::get_uia_tree(hwnd)
```

### Pattern 1: Reuse the generic request/response round trip (RESEARCH Pattern 1/3, established Phase 6)

**What:** `Session::get_uia_tree(hwnd)` is a two-line addition once `MsgType::Uia` exists — it calls the existing `sensor_request()` helper exactly like `get_window_list`/`get_process_tree`.
**When to use:** Always — this is not a phase-specific decision, it is the established pattern.
**Example:**
```rust
// Source: crates/rdpilot/src/session.rs:583-593 (existing get_window_list, the
// direct template — Phase 7 adds an analogous method with a request payload)
pub async fn get_uia_tree(&self, hwnd: u64) -> Result<Vec<UiaElement>> {
    let data = self
        .sensor_request(
            crate::sensor::MsgType::Uia,
            Some(serde_json::json!({ "hwnd": hwnd })),
            ENUMERATION_TIMEOUT_MS, // reuse the existing 2000ms enumeration bound;
                                    // SC#3's 500ms is the WALK budget the sensor
                                    // itself must hit — see Common Pitfalls
        )
        .await?;
    let wires: Vec<crate::perception::UiaElementWire> =
        serde_json::from_value(data).map_err(|e| Error::dvc(format!("malformed Uia reply: {e}")))?;
    Ok(wires.into_iter().map(crate::perception::UiaElementWire::into_owned).collect())
}
```

### Pattern 2: Hand-authoring a `[GeneratedComInterface]` COM interface from an IDL (the core D-7.5 mechanics)

**What:** Basic shape, confirmed from Microsoft's official ComWrappers source-generation docs:
```csharp
// Source: learn.microsoft.com/en-us/dotnet/standard/native-interop/comwrappers-source-generation
[GeneratedComInterface]
[Guid("3faca0d2-e7f1-4e9c-82a6-404fd6e0aab8")]
internal partial interface IFoo
{
    void Method(int i);
}
```
Key mechanics [CITED: learn.microsoft.com/en-us/dotnet/standard/native-interop/comwrappers-source-generation, github.com/dotnet/runtime/blob/main/docs/design/libraries/ComInterfaceGenerator/DerivedComInterfaces.md]:
- **Only `IUnknown`-derived interfaces are supported** (no `IDispatch`/dual) — UIA's COM interfaces are pure `IUnknown` vtable interfaces (not automation-compatible dual interfaces), so this is a clean fit, no workaround needed.
- **Vtable slot = C# declaration order.** The generator lays out the vtable exactly as the interface is written, top to bottom, with base-interface members preceding derived-interface members if you use C# interface inheritance (`interface IDerived : IBase`) — do NOT re-declare base members with `new` (that IS required in the OLD `[ComImport]` model but is explicitly WRONG here and produces an incorrect vtable).
- **Implicit `HRESULT`:** a non-`void` C# return type is translated to a native method whose LAST parameter is the `[out, retval]` pointer for that value; the `HRESULT` itself becomes an exception on failure (or use `[PreserveSig]` to get the raw `int` HRESULT back and handle it yourself — needed for the `GetRuntimeId`/`SAFEARRAY` case, see Common Pitfalls).
- **`[MarshalAs]` support is limited** — "source-generated interop for P/Invokes and COM only respects a small subset of `MarshalAsAttribute`... recommended to use `MarshalUsingAttribute` instead" [CITED, GitHub search synthesis]. For the members this phase needs, `[MarshalAs(UnmanagedType.Bool)]` (4-byte Win32 `BOOL`, matches the existing `WindowEnumeration.cs` P/Invoke convention already used for `IsIconic`/`IsZoomed`/`IsWindowVisible`) is the right call — confirm it compiles under `[GeneratedComInterface]` specifically in the smoke test, since the limited-support caveat means this needs empirical confirmation, not just precedent from `[LibraryImport]`.

### Pattern 3: Obtaining the `IUIAutomation` root instance under NativeAOT (no `new CUIAutomation()`)

**What:** Under classic managed COM (`ComImport`), you can do `new CUIAutomation()` because the runtime auto-generates a coclass activation wrapper. Under `[GeneratedComInterface]` there is no such wrapper — you must call `CoCreateInstance` yourself and wrap the returned raw pointer.
**Confirmed pattern** [CITED: simonmourier.com/blog/csharp-Native-AoT-How-to-get-pointer-to-Managed-class-to-pass-as-parameter-to-Co, adapted to remove the CsWin32 dependency per D-7.5]:
```csharp
// Hand-rolled CoCreateInstance P/Invoke (ole32.dll) — Guid is a blittable
// 16-byte struct, no special marshaller needed for `in Guid` under LibraryImport.
[LibraryImport("ole32.dll")]
private static partial int CoCreateInstance(
    in Guid rclsid, nint pUnkOuter, uint dwClsContext, in Guid riid, out nint ppv);

private const uint CLSCTX_INPROC_SERVER = 0x1;
private static readonly Guid ClsidCuiAutomation = new("ff48dba4-60ef-4201-aa87-54103eef594e");
private static readonly Guid IidIuiAutomation = new("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee");

internal static IUIAutomation GetRootAutomation()
{
    int hr = CoCreateInstance(ClsidCuiAutomation, 0, CLSCTX_INPROC_SERVER, IidIuiAutomation, out nint ppv);
    if (hr != 0 || ppv == 0)
    {
        throw new COMException("CoCreateInstance(CLSID_CUIAutomation) failed", hr);
    }
    var cw = new System.Runtime.InteropServices.Marshalling.StrategyBasedComWrappers();
    object obj = cw.GetOrCreateObjectForComInstance(ppv, System.Runtime.InteropServices.CreateObjectFlags.None);
    return (IUIAutomation)obj;
}
```
`StrategyBasedComWrappers` is built into the .NET 8 SDK (`System.Runtime.InteropServices.Marshalling` namespace) — no new package.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Tree-scoped element enumeration | Recursive `IUIAutomationTreeWalker::GetFirstChildElement`/`GetNextSiblingElement` walk | `IUIAutomation::FindAll(TreeScope_Children, trueCondition)` (single COM call) | D-7.4 explicitly requires a single scoped COM call, not TreeWalker recursion; `FindAll` is both simpler to hand-author (no need for `IUIAutomationTreeWalker` at all — one fewer interface to declare) and matches the locked decision directly |
| ControlType→string mapping | A dynamic/reflection-based lookup of `UIA_*ControlTypeId` constants | A static `switch`/match table (given in full below) | The full set is small (41 entries), stable since Windows 7, and a static table is both AOT-safe and matches the project's existing `ClassifyWindowState`-style plain-string-mapping convention (`WindowEnumeration.cs`) |
| `SAFEARRAY` element access | Hand-rolled unsafe pointer arithmetic over the raw `SAFEARRAY*` | `System.Runtime.InteropServices.Marshal.SafeArrayGetLBound`/`GetUBound`/`GetElement` (still available, NOT part of the AOT-incompatible IL-stub COM interop system — these are plain static P/Invoke-backed helpers) | Reduces surface area for memory-safety bugs; these Marshal helpers are documented as usable outside the built-in-COM-interop system |

**Key insight:** every "don't hand-roll" item above exists because the obvious first instinct (recursive tree walk, dynamic reflection, raw pointer math) is exactly the kind of code D-7.5's rejected alternatives (`System.Windows.Automation`, `ComImport`) already do for you — but do so in an AOT-incompatible way. The hand-authored `[GeneratedComInterface]` surface must stay minimal and typed.

## `IUIAutomation` / `IUIAutomationElement` / `IUIAutomationElementArray` — Hand-Authoring Reference

All GUIDs and method orders below were fetched directly from a mirror of the real Windows SDK `UIAutomationClient.idl` [CITED: raw.githubusercontent.com/tpn/winsdk-10/master/Include/10.0.14393.0/um/UIAutomationClient.idl] and cross-verified against the `windows-rs` win32metadata-generated mirror [CITED: microsoft.github.io/windows-docs-rs — Microsoft's own machine-generated bindings, independently derived from the same win32metadata source] plus multiple community Go/Java COM bindings that list the same GUIDs. Two independent sources agreeing on GUID + order gives this table MEDIUM-HIGH confidence — not HIGH, because no genuine Windows SDK header was read directly from an actual SDK install (this dev host is Linux; the winsdk-10 GitHub mirror is a third-party archival copy, not `learn.microsoft.com` itself, though it matches the metadata mirror byte-for-byte on every field checked).

### GUIDs

| Symbol | GUID |
|---|---|
| `CLSID_CUIAutomation` | `ff48dba4-60ef-4201-aa87-54103eef594e` |
| `IID_IUIAutomation` | `30cbe57d-d9d0-452a-ab13-7ac5ac4825ee` |
| `IID_IUIAutomationElement` | `d22108aa-8ac5-49a5-837b-37bbb3d7591e` |
| `IID_IUIAutomationElementArray` | `14314595-b4bc-4055-95f2-58f2e42c9855` |
| `IID_IUIAutomationCondition` | `352ffba8-0973-437c-a61f-f64cafd81df9` |

### `IUIAutomation` — declare 19 slots (2 real, 17 placeholder)

Only slots 4 (`ElementFromHandle`) and 19 (`CreateTrueCondition`) are ever called. **Every slot 1-18 except 4 must still be declared** (any signature, never invoked) or slots 4/19 land on the wrong native method:

| # | Native member | This phase |
|---|---|---|
| 1 | `CompareElements` | placeholder |
| 2 | `CompareRuntimeIds` | placeholder |
| 3 | `GetRootElement` | placeholder |
| **4** | `ElementFromHandle([in] HWND hwnd, [out,retval] IUIAutomationElement** element)` | **REAL — returns the root element for `get_uia_tree(hwnd)`** |
| 5 | `ElementFromPoint` | placeholder |
| 6 | `GetFocusedElement` | placeholder |
| 7 | `GetRootElementBuildCache` | placeholder |
| 8 | `ElementFromHandleBuildCache` | placeholder |
| 9 | `ElementFromPointBuildCache` | placeholder |
| 10 | `GetFocusedElementBuildCache` | placeholder |
| 11 | `CreateTreeWalker` | placeholder |
| 12 | `ControlViewWalker` (propget) | placeholder |
| 13 | `ContentViewWalker` (propget) | placeholder |
| 14 | `RawViewWalker` (propget) | placeholder |
| 15 | `RawViewCondition` (propget) | placeholder |
| 16 | `ControlViewCondition` (propget) | placeholder |
| 17 | `ContentViewCondition` (propget) | placeholder |
| 18 | `CreateCacheRequest` | placeholder |
| **19** | `CreateTrueCondition([out,retval] IUIAutomationCondition** newCondition)` | **REAL — produces the "match everything" condition `FindAll` needs** |

Do not declare slots 20+ (`CreateFalseCondition` onward) — the interface simply ends at slot 19 in the C# declaration; the real COM object has more members, but you never call them so they never need a C# slot.

### `IUIAutomationElement` — declare 41 slots (9 real, 32 placeholder)

| # | Native member | This phase | D-7.1 field |
|---|---|---|---|
| 1 | `SetFocus` | placeholder | — |
| **2** | `GetRuntimeId([out,retval] SAFEARRAY(int)* runtimeId)` | **REAL — see Common Pitfalls for SAFEARRAY marshalling risk** | `id` (D-7.2) |
| 3 | `FindFirst` | placeholder | — |
| **4** | `FindAll([in] TreeScope scope, [in] IUIAutomationCondition* condition, [out,retval] IUIAutomationElementArray** found)` | **REAL — the single scoped tree-walk call (D-7.4)** | — |
| 5-17 | `FindFirstBuildCache` … `GetCachedChildren` | placeholder (13 slots) | — |
| 18 | `CurrentProcessId` (propget) | placeholder | — |
| **19** | `CurrentControlType` (propget, returns `CONTROLTYPEID` i.e. `int`) | **REAL** | `role` (D-7.3, mapped via table below) |
| 20 | `CurrentLocalizedControlType` (propget) | placeholder | (explicitly NOT used — D-7.3 rejects localized names) |
| **21** | `CurrentName` (propget, returns `BSTR`) | **REAL** | `name` |
| 22-23 | `CurrentAcceleratorKey`, `CurrentAccessKey` | placeholder | — |
| **24** | `CurrentHasKeyboardFocus` (propget, returns `BOOL`) | **REAL** | `focused` |
| **25** | `CurrentIsKeyboardFocusable` (propget, returns `BOOL`) | **REAL** | `focusable` |
| **26** | `CurrentIsEnabled` (propget, returns `BOOL`) | **REAL** | `enabled` |
| 27-35 | `CurrentAutomationId` … `CurrentItemType` | placeholder (9 slots) | — |
| **36** | `CurrentIsOffscreen` (propget, returns `BOOL`) | **REAL** (negate for `visible`) | `visible = !IsOffscreen` |
| 37-40 | `CurrentOrientation` … `CurrentItemStatus` | placeholder (4 slots) | — |
| **41** | `CurrentBoundingRectangle` (propget, returns `RECT` by out-pointer) | **REAL** | `bbox` |

Do not declare slots 42+ (`CurrentLabeledBy` onward, plus all `Cached*` mirrors and `GetClickablePoint`) — same rule as above.

### `IUIAutomationElementArray` — declare 2 slots, both real

| # | Native member | This phase |
|---|---|---|
| 1 | `Length` (propget, returns `int`) | **REAL** |
| 2 | `GetElement([in] int index, [out,retval] IUIAutomationElement** element)` | **REAL** |

### `IUIAutomationCondition` — declare 0 members

The condition object returned by `CreateTrueCondition()` is only ever passed BACK into `FindAll` as an opaque `IUnknown`-derived pointer — this phase never calls a method on it. Declare it as an empty marker interface:
```csharp
[GeneratedComInterface]
[Guid("352ffba8-0973-437c-a61f-f64cafd81df9")]
internal partial interface IUIAutomationCondition
{
}
```

## Common Pitfalls

### Pitfall 1: `_VTblGapN_M()` placeholder methods do not work under `[GeneratedComInterface]`
**What goes wrong:** Copying the classic `[ComImport]` idiom of skipping unwanted vtable slots with `_VTblGap1_23()`-style methods causes access violations or `MissingMethodException` under the source generator.
**Why it happens:** Confirmed regression — the generator treats a gap declaration as a real method to marshal, not a slot placeholder [CITED: github.com/dotnet/runtime/issues/102421, unresolved, milestone "Future"].
**How to avoid:** Declare a REAL (never-invoked) placeholder method per unused slot instead, with any convenient signature (e.g. `void _reserved3();`) — this is the only confirmed-working technique. The slot lists above already account for every gap.
**Warning signs:** A crash or garbage return the moment you call a "real" method that comes AFTER a `_VTblGap`-style declaration in your interface.

### Pitfall 2: `GetRuntimeId`'s `SAFEARRAY(int)` return has no confirmed `[GeneratedComInterface]` marshalling pattern
**What goes wrong:** `SAFEARRAY` support under the ComInterfaceGenerator was tracked as a planned feature; no confirmed end-to-end working example for `SAFEARRAY(int)` under `[GeneratedComInterface]` was found in this research [ASSUMED gap — could not verify either way].
**Why it happens:** `SAFEARRAY` is a legacy VB/Automation-era type distinct from a plain COM array; the source generator's marshalling support matrix does not clearly document it.
**How to avoid:** Declare `GetRuntimeId` with `[PreserveSig]` returning the raw `int` HRESULT plus an `out nint` raw pointer to the `SAFEARRAY`, then manually decode with `System.Runtime.InteropServices.Marshal.SafeArrayGetLBound`/`SafeArrayGetUBound`/`SafeArrayGetElement` (documented static helpers, not part of the AOT-incompatible built-in IL-stub interop) and call `Marshal.SafeArrayDestroy` (or the equivalent free) when done. **This is the single riskiest line of code in the whole phase — validate it inside the AOT smoke test FIRST**, before any other UIA property read.
**Warning signs:** A working smoke test for `ElementFromHandle`/`CurrentName`/`CurrentBoundingRectangle` that then throws/crashes specifically on `GetRuntimeId` — isolate it immediately rather than debugging alongside other new code.

### Pitfall 3: `[MarshalAs]` support under source-generated COM is a documented "small subset"
**What goes wrong:** `[MarshalAs(UnmanagedType.Bool)]` (needed for `BOOL`-returning properties like `CurrentIsEnabled`) or `[MarshalAs(UnmanagedType.BStr)]`/`StringMarshalling.Utf16` (needed for `CurrentName`'s `BSTR`) may not compile or may marshal incorrectly under `[GeneratedComInterface]` specifically, even though the identical attribute is proven-working on the existing `[LibraryImport]` P/Invokes in `WindowEnumeration.cs`.
**Why it happens:** Microsoft's own docs state source-generated interop "only respects a small subset of `MarshalAsAttribute`... recommended to use `MarshalUsingAttribute` instead" — this caveat is specifically about the COM/`GeneratedComInterface` path, not `[LibraryImport]` P/Invokes.
**How to avoid:** Set `StringMarshalling = StringMarshalling.Utf16` at the `[GeneratedComInterface]` attribute level for `IUIAutomationElement` as a first attempt for `BSTR` properties (COM strings are BSTR by convention; verify this in the smoke test rather than assuming); for `BOOL` properties try `[MarshalAs(UnmanagedType.Bool)]` on the return, and if the compiler rejects it, fall back to `[PreserveSig]` + raw `int` return + manual `!= 0` check (guaranteed to work, no marshaller dependency at all).
**Warning signs:** A `bool` property that always reads `true` (or always `false`) regardless of actual element state — classic symptom of a 4-byte/1-byte size mismatch between Win32 `BOOL` and C# `bool`'s marshalled size.

### Pitfall 4: Declaring base-interface members with `new` (the `[ComImport]` idiom) breaks the vtable under `[GeneratedComInterface]`
**What goes wrong:** If any interface used C# inheritance (not needed for this phase's flat interface list, but a trap if the planner later adds `IUIAutomationElement2`/`3` etc.), re-declaring base members with the classic `new` keyword produces an INCORRECT vtable layout.
**Why it happens:** The old `[ComImport]` model requires shadowing every base member; the new generator does the opposite — it automatically emits shadowing and expects you to declare ONLY the new derived members.
**How to avoid:** None of this phase's four interfaces use C# inheritance (all four are declared flat, directly implementing `IUnknown`) — this pitfall is documented so it is not accidentally introduced in a future phase extending these interfaces (e.g. adding `ValuePattern` support from the backlog).

### Pitfall 5: `TreeScope_Children` value must be `2`, not `1`
**What goes wrong:** Passing `TreeScope_Element` (value `1`, "the element itself") instead of `TreeScope_Children` (value `2`) to `FindAll` returns a one-element array containing only the root, not its children.
**How to avoid:** `TreeScope` is a `[Flags]`-style native enum: `TreeScope_None=0x0, TreeScope_Element=0x1, TreeScope_Children=0x2, TreeScope_Descendants=0x4, TreeScope_Parent=0x8, TreeScope_Ancestors=0x10` [CITED: learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/ne-uiautomationclient-treescope]. Pass the literal integer `2` (or declare a matching C# `[Flags] enum TreeScope { Children = 2, ... }`) to `FindAll`.

### Pitfall 6: A destroyed/stale element mid-enumeration must not sink the whole response (D-7.6)
**What goes wrong:** Reading any property on an `IUIAutomationElement` obtained from `FindAll` can throw `COMException` with `UIA_E_ELEMENTNOTAVAILABLE` (`0x80040201`) if the underlying UI element was destroyed between enumeration and property read.
**How to avoid:** Wrap EACH element's property-read block in its own `try`/`catch (COMException)` and `continue` (skip that element only) — mirrors `WindowEnumeration.cs`'s exact "destroyed between EnumWindows and here — skip, don't fail" comment/pattern at the per-window level. The outer `BuildUiaTreeResponse` still has its own top-level `try`/`catch` (mirroring `BuildWindowListReplyEnvelope`) that degrades the WHOLE response to `success:false` only on total handler failure — the per-element skip is a layer underneath that, not a replacement (D-7.6 is explicit about this).

### Pitfall 7: `dotnet publish -p:PublishAot=true` does not cross-OS-compile (carried forward from Phase 5/6)
**What goes wrong:** Attempting to validate the AOT smoke test from this Linux dev host directly.
**How to avoid:** The `--smoke-test-uia` binary must be published and RUN on a genuine Windows host (or the disposable Azure VM via `az vm run-command invoke`, the pattern Phase 6's live gate already established after WinRM auth failed from this host) — this is unchanged from Phase 5/6's confirmed constraint, just re-flagged here because it applies with extra force to COM interop (a `CoCreateInstance` call has zero meaning on Linux).

## Code Examples

### ControlType id → friendly-string mapping table (D-7.3)

Complete table for the 41 control types defined as of Windows 8.1's UIA extensions (stable; no removals since) [CITED: learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controltype-ids]:

```rust
// Source: crates/rdpilot/src/perception.rs (new helper, OR mirror in C# — see
// Architectural Responsibility Map recommendation to do this mapping sensor-side,
// consistent with the WindowState "normal"/"minimized"/"maximized" wire convention)
fn control_type_to_role(id: i32) -> &'static str {
    match id {
        50000 => "Button", 50001 => "Calendar", 50002 => "CheckBox",
        50003 => "ComboBox", 50004 => "Edit", 50005 => "Hyperlink",
        50006 => "Image", 50007 => "ListItem", 50008 => "List",
        50009 => "Menu", 50010 => "MenuBar", 50011 => "MenuItem",
        50012 => "ProgressBar", 50013 => "RadioButton", 50014 => "ScrollBar",
        50015 => "Slider", 50016 => "Spinner", 50017 => "StatusBar",
        50018 => "Tab", 50019 => "TabItem", 50020 => "Text",
        50021 => "ToolBar", 50022 => "ToolTip", 50023 => "Tree",
        50024 => "TreeItem", 50025 => "Custom", 50026 => "Group",
        50027 => "Thumb", 50028 => "DataGrid", 50029 => "DataItem",
        50030 => "Document", 50031 => "SplitButton", 50032 => "Window",
        50033 => "Pane", 50034 => "Header", 50035 => "HeaderItem",
        50036 => "Table", 50037 => "TitleBar", 50038 => "Separator",
        50039 => "SemanticZoom", 50040 => "AppBar",
        _ => "Unknown",
    }
}
```
(If mapped sensor-side in C#, this becomes an equivalent `switch` expression returning `string` — recommended location per the Architectural Responsibility Map above, mirroring `ClassifyWindowState`'s existing sensor-side string-mapping precedent.)

### RuntimeId → string join (D-7.2, Claude's Discretion — recommend `-` delimiter)

```rust
// Any stable deterministic format is acceptable per D-7.2's Claude's Discretion.
// "-" avoids collision with typical negative-number formatting ambiguity that "."
// could theoretically introduce if a runtime-id component were ever signed.
fn runtime_id_to_string(ids: &[i32]) -> String {
    ids.iter().map(i32::to_string).collect::<Vec<_>>().join("-")
}
```

### AOT smoke-test gate shape (D-7.5, mirrors `RunAotSmokeTest`)

```csharp
// New Program.cs arg, sibling to the existing --smoke-test:
private const string UiaSmokeTestArg = "--smoke-test-uia";
// In Main(): if (args[0] == UiaSmokeTestArg) return RunUiaAotSmokeTest();

private static int RunUiaAotSmokeTest()
{
    try
    {
        IUIAutomation automation = UiaInterop.GetRootAutomation(); // CoCreateInstance
        nint desktop = GetDesktopWindow(); // existing-style [LibraryImport] to user32.dll
        IUIAutomationElement root = automation.ElementFromHandle(desktop);
        // The single riskiest call — isolate it (Pitfall 2):
        int[] runtimeId = UiaInterop.ReadRuntimeId(root);
        string name = root.CurrentName;                 // BSTR marshalling proof
        bool enabled = root.CurrentIsEnabled;            // BOOL marshalling proof
        RECT rect = root.CurrentBoundingRectangle;       // struct-by-out-pointer proof
        IUIAutomationCondition trueCond = automation.CreateTrueCondition();
        IUIAutomationElementArray children = root.FindAll(2 /* TreeScope_Children */, trueCond);
        int count = children.Length;

        Console.WriteLine($"[smoke-test-uia] PASS: root name='{name}' enabled={enabled} " +
            $"rect={rect.Left},{rect.Top},{rect.Right},{rect.Bottom} runtimeId.len={runtimeId.Length} " +
            $"children={count}");
        return 0;
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine($"[smoke-test-uia] FAIL: {ex}");
        return 1;
    }
}
```
This exercises EVERY risky marshalling path (CoCreateInstance, BSTR, BOOL, RECT-by-value, SAFEARRAY, FindAll) in one throwaway binary, on the real desktop root element (always available, no Notepad dependency), before any real `UiaTree.cs` handler code depends on any of it — exactly the D-7.5 gate requirement.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| `[ComImport]` + `DllImport` COM interop | `[GeneratedComInterface]` + `[LibraryImport]` (source-generated, trim/AOT-safe) | .NET 8 (Nov 2023) | Old approach is a hard compile/runtime blocker for NativeAOT publish — not a style preference |
| `System.Windows.Automation` (WPF managed UIA wrapper) | Raw COM `IUIAutomation` interop | N/A — `System.Windows.Automation` was never AOT-compatible (reflection-heavy since its .NET Framework origins) | Confirms D-7.5's rejection is not a regression, it is a permanent constraint of this API |

**Deprecated/outdated:** `Interop.UIAutomationClient` (NuGet) — still published and functional under classic managed COM, but fundamentally incompatible with this phase's NativeAOT requirement; do not consult its generated code as a "shortcut" — it uses TLB-import shadowing (`new` keyword) which produces a WRONG vtable if copied into a `[GeneratedComInterface]` declaration (Pitfall 4).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|----------------|
| A1 | `GetRuntimeId`'s `SAFEARRAY(int)` marshalling works via the `[PreserveSig]` + manual `Marshal.SafeArrayGet*` fallback under `[GeneratedComInterface]` + NativeAOT | Common Pitfalls #2 | If wrong, `id`/D-7.2 has no data source; would need a different identity strategy (e.g. drop RuntimeId, use a synthetic index-based id, degrading D-7.2's rationale) — HIGH impact, isolate and validate first in the smoke test |
| A2 | `StringMarshalling.Utf16` correctly marshals `BSTR`-returning COM properties (`CurrentName`) under `[GeneratedComInterface]` | Common Pitfalls #3, `IUIAutomationElement` table | If wrong, `name` field is garbage/corrupted; fallback is `[MarshalAs(UnmanagedType.BStr)]` explicitly per-property or a raw `nint`+`Marshal.PtrToStringBSTr`+`Marshal.FreeBSTr` manual path |
| A3 | `[MarshalAs(UnmanagedType.Bool)]` on a `[GeneratedComInterface]` property return correctly marshals 4-byte Win32 `BOOL` | Common Pitfalls #3 | If wrong, `enabled`/`focusable`/`focused`/`visible` fields are always true or always false; fallback is `[PreserveSig]` + raw `int` + `!= 0` |
| A4 | The `tpn/winsdk-10` GitHub mirror of `UIAutomationClient.idl` is byte-accurate to the genuine Windows SDK for the interfaces/GUIDs used here | `IUIAutomation`/`IUIAutomationElement` reference tables | LOW — independently cross-checked against `windows-rs`'s official win32metadata-generated docs and multiple community bindings, all agreeing; a GUID mismatch would surface immediately as `E_NOINTERFACE` in the AOT smoke test, so this is self-correcting at the first gate, not a silent failure |
| A5 | The whole "sensor sends the friendly ControlType string" vs. "sensor sends raw int, Rust maps" architectural choice (recommended: sensor-side string) is left to the planner as Claude's Discretion, not re-litigated here | Architectural Responsibility Map | LOW — either choice satisfies D-7.3 equally; flagged only so the planner picks one consistently, matching `WindowState`'s existing sensor-side-string convention |

## Open Questions

1. **Does `[GeneratedComInterface]`'s implicit HRESULT-to-exception translation correctly surface `UIA_E_ELEMENTNOTAVAILABLE` as a catchable `COMException` with the expected HRESULT, or does it need `[PreserveSig]` everywhere to inspect the HRESULT directly?**
   - What we know: Microsoft's docs confirm the default behavior converts non-`S_OK` HRESULTs to a thrown exception (unspecified exact type in the docs excerpt found — likely `COMException` matching classic interop convention, but not explicitly confirmed for the source-generated path).
   - What's unclear: whether the thrown exception type/HRESULT-property behavior is identical to classic `[ComImport]`'s `COMException`, or a different type is used.
   - Recommendation: The AOT smoke test (Code Examples above) should also deliberately trigger a `COMException` (e.g. call a method on a stale/disposed element reference) to confirm the exact exception type and that `.HResult` reads back `0x80040201` before writing the real per-element try/catch in `UiaTree.cs`.

2. **What is `IUIAutomationElement.CurrentBoundingRectangle`'s exact `RECT` field-name capitalization/layout expectation for `[GeneratedComInterface]`'s implicit struct marshalling?**
   - What we know: `RECT` is `{LONG Left, Top, Right, Bottom}`, a blittable 16-byte struct; Phase 6's `Rect32` in `WindowEnumeration.cs` already declares an identical shape (`Left/Top/Right/Bottom`, all `int`) and it works under `[LibraryImport]`.
   - What's unclear: whether `[GeneratedComInterface]`'s COM-return-by-out-pointer struct marshalling for a property getter has any additional requirement beyond blittability (e.g. explicit `[StructLayout(LayoutKind.Sequential)]`, which `Rect32` already has).
   - Recommendation: Reuse the exact `Rect32` struct definition/layout from `WindowEnumeration.cs` verbatim for `CurrentBoundingRectangle`'s return type — do not redefine a new struct, minimizing the surface for a fresh mistake.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| .NET 8 SDK | Sensor build + AOT publish | ✓ | 8.0.422 | — |
| Windows host (for `dotnet publish -p:PublishAot=true`) | AOT publish, cannot cross-compile from Linux | ✗ (this dev host is Linux) | — | Publish on the disposable Azure VM via `az vm run-command invoke` (Phase 6's proven pattern, since WinRM auth failed from this host) |
| Rust toolchain (`cargo`/`rustc`) | Offline Rust-side verification | ✓ (present, but pinned to `stable-x86_64-pc-windows-gnu` channel) | — | Requires MinGW gcc on PATH (existing project constraint since Phase 2, not new to this phase); offline verification against the native Linux target remains an acceptable substitute per the Phase 6 precedent (06-01's "no cfg(windows) code" note) as long as this phase's new Rust code also stays platform-agnostic |
| Live disposable Azure VM (`manage-env.ps1 up`) | Wave 4 live gate (SC#1-4) | ✗ (torn down after Phase 6's live gate per STATE.md) | — | Provision fresh via `manage-env.ps1 up` at Wave 4, matching every prior phase's live-gate pattern |
| Notepad (target app for SC#1) | Live gate test target | Assumed present (standard Windows 10/11 component, already used as the Phase 3/6/9 canonical target) | — | — |

**Missing dependencies with fallback:** Windows host for AOT publish (Azure VM `az vm run-command invoke`); live VM (provision on demand at Wave 4).

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | `cargo test` (Rust side, existing) + `dotnet publish`-gated smoke test (C# side, existing pattern) + gated `#[ignore]` live integration tests (`tests/live_session.rs`, `RDPILOT_LIVE` env gate) |
| Config file | none — convention-based, matches Phase 6 |
| Quick run command | `cargo test -p rdpilot --lib` (offline unit tests for `UiaElement`/`UiaElementWire` conversion, `control_type_to_role`, `runtime_id_to_string`) |
| Full suite command | `RDPILOT_LIVE=1 cargo test -p rdpilot --test live_session -- --ignored` (against a live provisioned VM) |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|---------------------|--------------|
| PERC-03 / SC#1 | `get_uia_tree(hwnd)` for Notepad returns flat `UiaElement[]` with all fields populated | live (gated) | `RDPILOT_LIVE=1 cargo test -p rdpilot --test live_session uia_tree_returns_populated_elements -- --ignored` | ❌ Wave 4 |
| PERC-03 / SC#2 | `bbox` coordinates share the framebuffer/window-list physical-pixel space | live (gated), cross-checked against `get_window_list`'s rect for the same window | same test file, assertion inside SC#1's test or a dedicated case | ❌ Wave 4 |
| PERC-03 / SC#3 | `TreeScope_Children` walk completes within 500ms for a standard Win32 app | live (gated), timed | same test file, `Instant::now()` wrap around the `get_uia_tree` call | ❌ Wave 4 |
| PERC-03 / SC#4 | Response round-trips through `serde_json` without loss | unit (offline) | `cargo test -p rdpilot --lib uia_element_wire_round_trips` | ❌ Wave 1/2 |
| D-7.2 (RuntimeId join) | Deterministic string join | unit (offline) | `cargo test -p rdpilot --lib runtime_id_to_string_is_deterministic` | ❌ Wave 1 |
| D-7.3 (ControlType map) | Known ids map to correct friendly strings, unknown id maps to "Unknown" | unit (offline) | `cargo test -p rdpilot --lib control_type_maps_known_and_unknown_ids` | ❌ Wave 1 |
| D-7.6 (per-element skip) | A malformed/partial canned reply array with one bad element still yields the good elements | unit (offline, canned JSON, mirrors Phase 6's `WindowInfoWire` test style) | `cargo test -p rdpilot --lib` | ❌ Wave 1 |

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot --lib` (offline; the C# side has no fast offline test — the AOT smoke test IS the equivalent gate and must be run at least once per Wave 2/3 boundary, not per-commit, since it requires a real `dotnet publish -p:PublishAot=true` on Windows)
- **Per wave merge:** full offline suite + (Wave 2 only) the `--smoke-test-uia` AOT publish gate
- **Phase gate:** live suite green before `/gsd-verify-work` (Wave 4)

### Wave 0 Gaps
- None — `crates/rdpilot/tests/live_session.rs` and the `RDPILOT_LIVE`/`require_target!`/`tests/common/mod.rs` gated-test harness already exist and cover the exact shape this phase's live tests need (established Phase 2-6).

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|--------------------|
| V5 Input Validation | yes | Malformed/oversized JSON from the (by-design unauthenticated) DVC channel must deserialize to a typed `Err`/drop, never panic — unchanged existing discipline (`sensor.rs` doc comment), applies identically to the new `Uia` request payload `{hwnd}` |
| V6 Cryptography | no | Not applicable — no new crypto surface this phase |
| V4 Access Control | no | Unchanged — DVC remains unauthenticated transport per the existing, already-accepted Pitfall m3 posture; not re-litigated this phase |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|------------------------|
| Malicious/garbage `hwnd` value in the `Uia` request payload (a value that is not a real window handle, or belongs to a different, sensitive process) | Tampering / Information Disclosure | `ElementFromHandle` on an invalid handle returns a COM error (caught by the existing top-level `try`/`catch` → `success:false`), not a crash; UIA itself enforces the OS's own cross-process element-access rules (no privilege escalation possible via a bad hwnd — this is Windows' own security boundary, not something this phase needs to re-implement) |
| `SAFEARRAY` handle leak on the manual `Marshal.SafeArrayGet*` fallback path (Pitfall 2) | Denial of Service (resource exhaustion via repeated leaked allocations) | Always pair the manual `SAFEARRAY` decode with its matching free/destroy call, even on the exception path (`finally` block) — flagged explicitly because this is hand-rolled, unlike the rest of the interop which the source generator manages automatically |

## Sources

### Primary (HIGH confidence)
- [ComWrappers source generation - .NET | Microsoft Learn](https://learn.microsoft.com/en-us/dotnet/standard/native-interop/comwrappers-source-generation) — GUID attribute usage, vtable/derived-interface rules, implicit HRESULT translation, `IUnknown`-only constraint, `[MarshalAs]` subset limitation
- [DerivedComInterfaces.md — dotnet/runtime](https://github.com/dotnet/runtime/blob/main/docs/design/libraries/ComInterfaceGenerator/DerivedComInterfaces.md) — vtable slot assignment mechanics, base/derived interface shadowing rules
- [UIAutomationClient.idl (tpn/winsdk-10 mirror)](https://github.com/tpn/winsdk-10/blob/master/Include/10.0.14393.0/um/UIAutomationClient.idl) — authoritative interface member order + GUIDs for `IUIAutomation`, `IUIAutomationElement`, `IUIAutomationElementArray`, `IUIAutomationCondition`, `CUIAutomation` coclass
- [windows-rs IUIAutomationElement docs](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/Accessibility/struct.IUIAutomationElement.html) and [IUIAutomation docs](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/Accessibility/struct.IUIAutomation.html) — cross-verification of member order (Microsoft's own win32metadata-generated bindings)
- [Control Type Identifiers (UIAutomationClient.h) — Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controltype-ids) — full ControlType id→name table (D-7.3)
- [TreeScope enum — Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/ne-uiautomationclient-treescope) — `TreeScope_Children = 0x2` confirmed
- [dotnet/runtime#102421 — ComWrappers source generator doesn't handle _VTblGap* methods](https://github.com/dotnet/runtime/issues/102421) — confirmed unresolved regression, the basis for Common Pitfall 1

### Secondary (MEDIUM confidence)
- [Simon Mourier's Blog — C# Native AoT, pointer to Managed class / CoCreateInstance pattern](https://www.simonmourier.com/blog/csharp-Native-AoT-How-to-get-pointer-to-Managed-class-to-pass-as-parameter-to-Co/) — CoCreateInstance + `StrategyBasedComWrappers` pattern (adapted here to remove its CsWin32 dependency)
- IID_IUIAutomationElement / IID_IUIAutomationElementArray / IID_IUIAutomationCondition — cross-verified across the IDL mirror + multiple independent community Go/Java bindings (search-result synthesis, no single canonical Microsoft page found publishing the raw GUID string directly)

### Tertiary (LOW confidence — flagged for live validation)
- `SAFEARRAY(int)` marshalling under `[GeneratedComInterface]` (Assumption A1) — no confirmed working example found; the manual `Marshal.SafeArrayGet*` fallback is a reasoned inference, not a verified pattern
- `[MarshalAs(UnmanagedType.Bool)]` and `StringMarshalling.Utf16`/BSTR behavior specifically under `[GeneratedComInterface]` (Assumptions A2/A3) — documented as "a small subset" supported, exact behavior for these specific cases not directly demonstrated in any source found

## Metadata

**Confidence breakdown:**
- Standard stack / Rust-side wire extension: HIGH — pure mechanical repetition of the proven Phase 6 pattern, zero new unknowns
- COM interop mechanics (vtable order, GUIDs, CoCreateInstance): MEDIUM-HIGH — every fact is sourced from the real IDL and official docs, cross-verified across independent sources, but the SPECIFIC combination (UIA + `[GeneratedComInterface]` + NativeAOT) has no known prior-art worked example anywhere — this is why D-7.5 correctly mandates a smoke-test gate rather than trusting research alone
- Marshalling details (SAFEARRAY, BSTR, BOOL under the source generator specifically): LOW-MEDIUM — flagged explicitly in the Assumptions Log; this is exactly what the AOT smoke test exists to resolve empirically, consistent with the project's established "reason offline, live-tune empirically" methodology (D-7.7 and every prior phase's timing-constant precedent)
- Pitfalls: HIGH for the vtable-gap and TreeScope-value pitfalls (directly sourced); MEDIUM for the marshalling-attribute pitfalls (reasoned from documented limitations, not directly reproduced)

**Research date:** 2026-07-09
**Valid until:** 30 days (stable Win32/COM API surface, but the `[GeneratedComInterface]` source generator itself is actively evolving — re-check `dotnet/runtime` issue trackers for #102421 and SAFEARRAY-support status if this research is consulted after a .NET 8.0.4xx+ SDK update)
