// Hand-authored [GeneratedComInterface] UIA COM interop (D-7.5,
// 07-02-PLAN.md Task 1). No official Microsoft [GeneratedComInterface] UIA
// bindings exist (07-RESEARCH.md confirmed via exhaustive search) -- these
// four interfaces are hand-ported from the real UIAutomationClient.idl's
// GUID-verified, exact vtable slot order ("Hand-Authoring Reference"
// section of 07-RESEARCH.md).
//
// CRITICAL (Pitfall 1): vtable slot = C# declaration order. The .NET 8
// source generator does NOT support the classic gap-skipping placeholder
// idiom (confirmed regression, dotnet/runtime#102421, unresolved) -- every
// intervening slot between real members MUST be declared as a real,
// never-invoked placeholder method (`_reservedN()`, N = the native slot
// number), in order, or every subsequent "real" slot lands on the wrong
// native method and calling it corrupts memory instead of erroring cleanly.
//
// This is Wave-1 offline scaffolding only (07-02-PLAN.md): it proves the
// interfaces compile and the sensor still AOT-trims clean on a linux-x64
// surrogate publish. The LOW-confidence marshalling assumptions this file
// depends on (RESEARCH A1 SAFEARRAY, A2 BSTR, A3 BOOL) are NOT validated
// here -- they are empirically resolved on a real Windows host at the
// dedicated 07-03 risk gate via --smoke-test-uia (Program.cs). Do not
// build the real UiaTree handler (07-04) on top of this file until 07-03
// has passed.
//
// No new <PackageReference> -- [GeneratedComInterface] and
// StrategyBasedComWrappers are built into the .NET 8 SDK
// (System.Runtime.InteropServices.Marshalling namespace) (D-7.5).

using System.Runtime.InteropServices;
using System.Runtime.InteropServices.Marshalling;

namespace RdpilotSensor;

/// UIA `TreeScope` flags (UIAutomationClient.idl). Only `Children` (D-7.4's
/// fixed, single-scoped-call tree walk) is used this phase; the remaining
/// values are declared for fidelity with the native `[Flags]` enum.
/// Pitfall 5: `Children` MUST be `2`, not `1` (`Element`) -- passing the
/// wrong value returns a one-element array containing only the root.
[Flags]
internal enum TreeScope
{
    None = 0,
    Element = 1,
    Children = 2,
    Descendants = 4,
    Parent = 8,
    Ancestors = 16,
}

/// A bounding rectangle in the native Win32 `RECT` layout
/// (`{LONG Left, Top, Right, Bottom}`, blittable). Deliberately the same
/// field names/order/[StructLayout(LayoutKind.Sequential)] shape as the
/// `Rect32` already proven under [LibraryImport] in
/// `WindowEnumeration.cs` (RESEARCH Open Question #2 recommendation: reuse
/// the identical layout rather than inventing a new one) -- declared as its
/// own top-level type here because `WindowEnumeration.Rect32` is a private
/// nested type of a different static class and this plan's file scope is
/// `UiaInterop.cs`/`Program.cs` only.
[StructLayout(LayoutKind.Sequential)]
internal struct Rect32
{
    public int Left;
    public int Top;
    public int Right;
    public int Bottom;
}

/// The "match everything" condition object `CreateTrueCondition()` produces
/// and `FindAll` consumes. This phase only ever passes it BACK into
/// `FindAll` as an opaque `IUnknown`-derived pointer -- no member is ever
/// called on it, so it is declared as an empty marker interface (0 vtable
/// slots reserved in C#; the real COM object has members, but none are ever
/// invoked so none need a declared slot).
[GeneratedComInterface]
[Guid("352ffba8-0973-437c-a61f-f64cafd81df9")]
internal partial interface IUIAutomationCondition
{
}

/// The array `IUIAutomationElement.FindAll` returns. Declare 2 slots, both
/// real -- `Length` and `GetElement` are the only members
/// `get_uia_tree`(07-04)'s enumeration loop needs.
[GeneratedComInterface]
[Guid("14314595-b4bc-4055-95f2-58f2e42c9855")]
internal partial interface IUIAutomationElementArray
{
    /// Slot 1 (REAL). SYSLIB1091: instance properties are not
    /// supported in a [GeneratedComInterface]-attributed interface --
    /// declared as a get-method instead of a C# property.
    int GetLength();

    /// Slot 2 (REAL).
    IUIAutomationElement GetElement(int index);
}

/// The per-element UIA interface. Declares all 41 slots from
/// `UIAutomationClient.idl`'s member order (slot 1 through 41, the last
/// slot this phase's field set needs) -- 9 are real (D-7.1's field set +
/// the tree-walk/identity plumbing), 32 are never-invoked placeholders
/// required only to keep every real slot's vtable offset correct
/// (Pitfall 1).
///
/// LIVE-DIAGNOSED BUG FIX (07-03 risk gate, RESEARCH Assumption A2): the
/// first-attempt `StringMarshalling = StringMarshalling.Utf16` interface-
/// level strategy for `CurrentName`'s `BSTR` return -- declaring the member
/// as a plain C# `string GetCurrentName()` -- crashed the live win-x64 AOT
/// binary with `STATUS_HEAP_CORRUPTION` (0xC0000374, faulting in
/// `ntdll.dll`) the instant `GetCurrentName()` was called (confirmed via
/// per-step `--smoke-test-uia` tracing: `ReadRuntimeId` and everything
/// before it succeeded cleanly; the crash landed exactly here). Root cause:
/// a COM `BSTR` is allocated by the OLE Automation allocator
/// (`SysAllocString`) and MUST be freed with `SysFreeString` -- but
/// `StringMarshalling.Utf16`'s implicit marshaller treats the returned
/// pointer as a plain null-terminated `LPWSTR` and frees it with
/// `CoTaskMemFree` instead, corrupting the OLE Automation heap the moment
/// the mismatched allocator/deallocator pair collide. Fixed per RESEARCH's
/// own documented fallback: declare the slot as `[PreserveSig]` returning
/// the raw HRESULT plus an `out nint` to the raw `BSTR` pointer (identical
/// shape to `GetRuntimeId`'s SAFEARRAY fallback below), then decode via
/// `Marshal.PtrToStringBSTR` and free via `Marshal.FreeBSTR` in the managed
/// caller (<see cref="UiaInterop.ReadName"/>) -- both of which ARE present
/// on .NET 8's `Marshal` (unlike the `SafeArrayGet*` family removed in
/// .NET Core), so no hand-rolled P/Invoke is needed here. The interface-
/// level `StringMarshalling` attribute is removed entirely: this was its
/// only consumer.
[GeneratedComInterface]
[Guid("d22108aa-8ac5-49a5-837b-37bbb3d7591e")]
internal partial interface IUIAutomationElement
{
    /// Slot 1: `SetFocus` -- placeholder, never called.
    void _reserved1();

    /// Slot 2 (REAL): `GetRuntimeId([out,retval] SAFEARRAY(int)* runtimeId)`.
    /// Feeds `id` (D-7.2). `[PreserveSig]` returning the raw `int` HRESULT
    /// plus an `out nint` to the raw `SAFEARRAY*` -- RESEARCH Pitfall 2 /
    /// Assumption A1: no confirmed `[GeneratedComInterface]` marshalling
    /// pattern exists for `SAFEARRAY(int)`, so this is deliberately NOT a
    /// typed `int[]` return; the caller (<see cref="UiaInterop.ReadRuntimeId"/>)
    /// manually decodes the raw pointer via `Marshal.SafeArrayGet*`. This is
    /// the single riskiest line of code in the whole phase (per RESEARCH) --
    /// it is exercised FIRST, in isolation, by the 07-03 `--smoke-test-uia`
    /// gate, before any other property read.
    [PreserveSig]
    int GetRuntimeId(out nint runtimeId);

    /// Slot 3: `FindFirst` -- placeholder, never called.
    void _reserved3();

    /// Slot 4 (REAL): `FindAll([in] TreeScope scope, [in] IUIAutomationCondition* condition, [out,retval] IUIAutomationElementArray** found)`.
    /// The single scoped tree-walk call (D-7.4) -- NOT `IUIAutomationTreeWalker`
    /// recursion; a `TreeScope.Children` call returns a flat one-level array
    /// in one COM round trip.
    IUIAutomationElementArray FindAll(TreeScope scope, IUIAutomationCondition condition);

    // Slots 5-17 (13 slots): FindFirstBuildCache ... GetCachedChildren --
    // placeholders, never called.
    void _reserved5();
    void _reserved6();
    void _reserved7();
    void _reserved8();
    void _reserved9();
    void _reserved10();
    void _reserved11();
    void _reserved12();
    void _reserved13();
    void _reserved14();
    void _reserved15();
    void _reserved16();
    void _reserved17();

    /// Slot 18: `CurrentProcessId` (propget) -- placeholder, never called.
    void _reserved18();

    /// Slot 19 (REAL): `CurrentControlType` (propget, native `CONTROLTYPEID`
    /// i.e. plain `int`). Feeds `role` (D-7.3) -- the friendly-string
    /// mapping table lives Rust-side (07-01), the sensor ships the raw int.
    /// SYSLIB1091: instance properties are not supported in a
    /// [GeneratedComInterface]-attributed interface -- declared as a
    /// get-method instead of a C# property (same for every other propget
    /// slot below).
    int GetCurrentControlType();

    /// Slot 20: `CurrentLocalizedControlType` (propget) -- placeholder,
    /// deliberately never used (D-7.3 explicitly rejects localized names).
    void _reserved20();

    /// Slot 21 (REAL): `CurrentName` (propget, native `BSTR`). Feeds `name`.
    /// See the interface doc comment above for the marshalling
    /// strategy/live-diagnosed fix -- `[PreserveSig]` + raw `out nint` BSTR
    /// pointer, decoded manually by <see cref="UiaInterop.ReadName"/>
    /// (RESEARCH Assumption A2 fallback).
    [PreserveSig]
    int GetCurrentName(out nint name);

    // Slots 22-23: CurrentAcceleratorKey, CurrentAccessKey -- placeholders,
    // never called.
    void _reserved22();
    void _reserved23();

    /// Slot 24 (REAL): `CurrentHasKeyboardFocus` (propget, native 4-byte
    /// `BOOL`). Feeds `focused`. `[MarshalAs(UnmanagedType.Bool)]` is the
    /// RESEARCH Pitfall 3 / Assumption A3 first attempt -- if the 07-03 live
    /// gate shows a size mismatch (property always reads true/false
    /// regardless of actual state), the documented fallback is
    /// `[PreserveSig]` + raw `int` return + manual `!= 0` check (no
    /// marshaller dependency at all).
    [return: MarshalAs(UnmanagedType.Bool)]
    bool GetCurrentHasKeyboardFocus();

    /// Slot 25 (REAL): `CurrentIsKeyboardFocusable` (propget, `BOOL`). Feeds
    /// `focusable`. Same `[MarshalAs(UnmanagedType.Bool)]` first-attempt /
    /// `[PreserveSig]`-raw-int fallback as slot 24.
    [return: MarshalAs(UnmanagedType.Bool)]
    bool GetCurrentIsKeyboardFocusable();

    /// Slot 26 (REAL): `CurrentIsEnabled` (propget, `BOOL`). Feeds
    /// `enabled`. Same marshalling strategy/fallback as slot 24.
    [return: MarshalAs(UnmanagedType.Bool)]
    bool GetCurrentIsEnabled();

    // Slots 27-35 (9 slots): CurrentAutomationId ... CurrentItemType --
    // placeholders, never called.
    void _reserved27();
    void _reserved28();
    void _reserved29();
    void _reserved30();
    void _reserved31();
    void _reserved32();
    void _reserved33();
    void _reserved34();
    void _reserved35();

    /// Slot 36 (REAL): `CurrentIsOffscreen` (propget, `BOOL`). Feeds
    /// `visible = !IsOffscreen` (negated at the call site, 07-04). Same
    /// marshalling strategy/fallback as slot 24.
    [return: MarshalAs(UnmanagedType.Bool)]
    bool GetCurrentIsOffscreen();

    // Slots 37-40 (4 slots): CurrentOrientation ... CurrentItemStatus --
    // placeholders, never called.
    void _reserved37();
    void _reserved38();
    void _reserved39();
    void _reserved40();

    /// Slot 41 (REAL): `CurrentBoundingRectangle` (propget, native `RECT`
    /// returned by out-pointer). Feeds `bbox` (D-7.1) -- reuses the
    /// `Rect32` blittable struct shape (see its doc comment above).
    Rect32 GetCurrentBoundingRectangle();

    // Do not declare slot 42+ (CurrentLabeledBy onward, plus all Cached*
    // mirrors and GetClickablePoint) -- the interface simply ends here; the
    // real COM object has more members but none are ever called.
}

/// The UIA automation root. Declares 19 slots (2 real, 17 placeholder) --
/// obtained via <see cref="UiaInterop.GetRootAutomation"/>, never via a
/// managed `new CUIAutomation()` activation wrapper (no such wrapper exists
/// under `[GeneratedComInterface]`; Pattern 3).
[GeneratedComInterface]
[Guid("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee")]
internal partial interface IUIAutomation
{
    // Slots 1-3: CompareElements, CompareRuntimeIds, GetRootElement --
    // placeholders, never called.
    void _reserved1();
    void _reserved2();
    void _reserved3();

    /// Slot 4 (REAL): `ElementFromHandle([in] HWND hwnd, [out,retval] IUIAutomationElement** element)`.
    /// Returns the root automation element for `get_uia_tree(hwnd)`
    /// (07-04) -- and, in this plan's smoke test, for the desktop root
    /// (always available, no target-app dependency).
    IUIAutomationElement ElementFromHandle(nint hwnd);

    // Slots 5-18 (14 slots): ElementFromPoint ... CreateCacheRequest --
    // placeholders, never called.
    void _reserved5();
    void _reserved6();
    void _reserved7();
    void _reserved8();
    void _reserved9();
    void _reserved10();
    void _reserved11();
    void _reserved12();
    void _reserved13();
    void _reserved14();
    void _reserved15();
    void _reserved16();
    void _reserved17();
    void _reserved18();

    /// Slot 19 (REAL): `CreateTrueCondition([out,retval] IUIAutomationCondition** newCondition)`.
    /// Produces the "match everything" condition `FindAll` needs (D-7.4).
    IUIAutomationCondition CreateTrueCondition();

    // Do not declare slot 20+ (CreateFalseCondition onward) -- the
    // interface simply ends here; the real COM object has more members but
    // none are ever called.
}

/// `CoCreateInstance` P/Invoke, `GetRootAutomation()`, and the manual
/// `SAFEARRAY` runtime-id decode (T-07-03: leak-safe via a `finally`
/// `SafeArrayDestroy` call). No new `<PackageReference>` -- everything used
/// here ships in the .NET 8 SDK.
///
/// DEVIATION from RESEARCH's "Don't Hand-Roll" table (discovered at this
/// plan's offline `dotnet build` gate, CS0117): `Marshal.SafeArrayGetLBound`/
/// `GetUBound`/`GetElement`/`SafeArrayDestroy` do NOT exist on
/// `System.Runtime.InteropServices.Marshal` in .NET Core/.NET 8 (they are a
/// .NET-Framework-only surface) -- RESEARCH's assumption that these "plain
/// static P/Invoke-backed helpers" are "still available" outside the
/// AOT-incompatible built-in COM interop system does not hold for this SDK.
/// The fix (Rule 1 -- code as researched does not compile) is to hand-roll
/// the equivalent native `oleaut32.dll` SAFEARRAY exports directly via
/// `[LibraryImport]`, which is both AOT-safe and consistent with this
/// file's existing `CoCreateInstance` P/Invoke discipline.
internal static partial class UiaInterop
{
    private const uint ClsctxInprocServer = 0x1;

    private static readonly Guid ClsidCuiAutomation = new("ff48dba4-60ef-4201-aa87-54103eef594e");
    private static readonly Guid IidIuiAutomation = new("30cbe57d-d9d0-452a-ab13-7ac5ac4825ee");

    /// Hand-rolled `CoCreateInstance` (ole32.dll) -- `Guid` is a blittable
    /// 16-byte struct, no custom marshaller needed for `in Guid` under
    /// `[LibraryImport]` (Pattern 3).
    [LibraryImport("ole32.dll")]
    private static partial int CoCreateInstance(in Guid rclsid, nint pUnkOuter, uint dwClsContext, in Guid riid, out nint ppv);

    /// Hand-rolled OLE Automation SAFEARRAY exports (oleaut32.dll) -- the
    /// direct native replacements for the non-existent-under-.NET-8
    /// `Marshal.SafeArrayGet*`/`SafeArrayDestroy` helpers (see the class
    /// doc comment above). `rgIndices`/`pv` are single-`LONG` pointers
    /// because <see cref="ReadRuntimeId"/> only ever decodes a 1-dimensional
    /// `SAFEARRAY(int)`.
    [LibraryImport("oleaut32.dll")]
    private static partial int SafeArrayGetLBound(nint psa, uint nDim, out int lowerBound);

    [LibraryImport("oleaut32.dll")]
    private static partial int SafeArrayGetUBound(nint psa, uint nDim, out int upperBound);

    [LibraryImport("oleaut32.dll")]
    private static partial int SafeArrayGetElement(nint psa, in int rgIndices, out int value);

    [LibraryImport("oleaut32.dll")]
    private static partial int SafeArrayDestroy(nint psa);

    /// Obtains the root `IUIAutomation` instance under NativeAOT (Pattern
    /// 3): `CoCreateInstance(CLSID_CUIAutomation, ..., IID_IUIAutomation)`
    /// then `StrategyBasedComWrappers.GetOrCreateObjectForComInstance` wraps
    /// the raw pointer -- there is no managed activation-wrapper shortcut
    /// (`new CUIAutomation()`) under `[GeneratedComInterface]`.
    internal static IUIAutomation GetRootAutomation()
    {
        int hr = CoCreateInstance(ClsidCuiAutomation, 0, ClsctxInprocServer, IidIuiAutomation, out nint ppv);
        if (hr != 0 || ppv == 0)
        {
            throw new COMException("CoCreateInstance(CLSID_CUIAutomation) failed", hr);
        }

        StrategyBasedComWrappers wrappers = new();
        object obj = wrappers.GetOrCreateObjectForComInstance(ppv, CreateObjectFlags.None);
        return (IUIAutomation)obj;
    }

    /// Manual `SAFEARRAY(int)` decode for `IUIAutomationElement.GetRuntimeId`
    /// (RESEARCH Pitfall 2 / Assumption A1 -- the single riskiest line of
    /// code in the phase). ALWAYS frees the native `SAFEARRAY` in a
    /// `finally` block, even on the exception path (T-07-03: no leaked
    /// native allocation).
    internal static int[] ReadRuntimeId(IUIAutomationElement element)
    {
        int hr = element.GetRuntimeId(out nint psa);
        if (hr != 0 || psa == 0)
        {
            throw new COMException("IUIAutomationElement.GetRuntimeId failed", hr);
        }

        try
        {
            int hrLBound = SafeArrayGetLBound(psa, 1, out int lowerBound);
            int hrUBound = SafeArrayGetUBound(psa, 1, out int upperBound);
            if (hrLBound != 0 || hrUBound != 0)
            {
                throw new COMException(
                    "SafeArrayGetLBound/SafeArrayGetUBound failed on GetRuntimeId's SAFEARRAY",
                    hrLBound != 0 ? hrLBound : hrUBound);
            }

            int length = upperBound - lowerBound + 1;
            int[] result = new int[length];
            for (int i = 0; i < length; i++)
            {
                int index = lowerBound + i;
                int hrElement = SafeArrayGetElement(psa, in index, out int value);
                if (hrElement != 0)
                {
                    throw new COMException("SafeArrayGetElement failed on GetRuntimeId's SAFEARRAY", hrElement);
                }

                result[i] = value;
            }

            return result;
        }
        finally
        {
            SafeArrayDestroy(psa);
        }
    }

    /// Manual `BSTR` decode for `IUIAutomationElement.GetCurrentName`
    /// (RESEARCH Assumption A2 -- live-diagnosed fallback, see the
    /// interface doc comment on <see cref="IUIAutomationElement.GetCurrentName"/>).
    /// `Marshal.PtrToStringBSTR`/`Marshal.FreeBSTR` (unlike the
    /// `SafeArrayGet*` family) DO exist on .NET 8's `Marshal` -- both are
    /// simple pointer-math helpers around the OLE Automation BSTR layout,
    /// not part of the AOT-incompatible built-in COM interop system -- so
    /// no hand-rolled P/Invoke is needed here, only the correct allocator-
    /// matched free (`SysFreeString` under the hood, via `FreeBSTR`)
    /// instead of the implicit marshaller's incorrect `CoTaskMemFree`.
    /// ALWAYS frees the native `BSTR` in a `finally` block, even on the
    /// exception path (same leak-safety discipline as
    /// <see cref="ReadRuntimeId"/>/T-07-03).
    internal static string ReadName(IUIAutomationElement element)
    {
        int hr = element.GetCurrentName(out nint bstr);
        if (hr != 0)
        {
            throw new COMException("IUIAutomationElement.GetCurrentName failed", hr);
        }

        if (bstr == 0)
        {
            // A null BSTR is a valid empty-string result (e.g. an
            // unnamed element) -- not an error, nothing to free.
            return string.Empty;
        }

        try
        {
            return Marshal.PtrToStringBSTR(bstr);
        }
        finally
        {
            Marshal.FreeBSTR(bstr);
        }
    }
}
