// UiaTree DTOs for the RDPILOT_SENSOR wire protocol (PERC-03, 07-04-PLAN.md
// Task 1). Wire schema is authoritatively defined by 07-01's Rust-side
// `UiaElementWire` (crates/rdpilot/src/perception.rs) -- the JSON keys below
// MUST match it EXACTLY (snake_case via [JsonPropertyName]):
//   request payload:  {"hwnd": <u64>}
//   response payload: {"success":true,"data":[UiaElementRecord,...]} or
//                      {"success":false,"error":"..."}
//
// D-7.2/D-7.3: the RuntimeId-join (`id`) and ControlType->role map (`role`)
// are Rust-side (07-01) responsibilities -- this sensor ships RAW
// `runtime_id`/`parent_runtime_id` int arrays and a RAW `control_type` int,
// never a pre-joined/pre-mapped string.
//
// `bbox` reuses `WindowRect` (WindowEnumeration.cs) verbatim -- same
// `x/y/w/h` physical virtual-desktop pixel shape, already registered for
// source-generated JSON, no need for a second bbox type (07-04-PLAN.md
// Task 1's reuse-vs-new choice, executor's call: reuse).
//
// The `UiaTree.BuildUiaTreeResponse` handler that populates these DTOs is
// added in this same plan's Task 2.

using System.Text.Json.Serialization;

namespace RdpilotSensor;

/// The Uia request payload: `{"hwnd": <u64>}`.
internal sealed record UiaTreeRequest
{
    [JsonPropertyName("hwnd")]
    public required ulong Hwnd { get; init; }
}

/// One flat UIA element record (D-7.1's field set, minus `value`/`Pattern`
/// which is explicitly DEFERRED to backlog). `RuntimeId`/`ParentRuntimeId`
/// are RAW int arrays and `ControlType` is a RAW int -- the D-7.2 join and
/// D-7.3 map are performed Rust-side (07-01), NOT here.
internal sealed record UiaElementRecord
{
    [JsonPropertyName("runtime_id")]
    public required int[] RuntimeId { get; init; }

    [JsonPropertyName("control_type")]
    public required int ControlType { get; init; }

    [JsonPropertyName("name")]
    public required string Name { get; init; }

    [JsonPropertyName("bbox")]
    public required WindowRect Bbox { get; init; }

    [JsonPropertyName("enabled")]
    public required bool Enabled { get; init; }

    [JsonPropertyName("visible")]
    public required bool Visible { get; init; }

    [JsonPropertyName("focusable")]
    public required bool Focusable { get; init; }

    [JsonPropertyName("focused")]
    public required bool Focused { get; init; }

    [JsonPropertyName("depth")]
    public required uint Depth { get; init; }

    [JsonPropertyName("parent_runtime_id")]
    public required int[] ParentRuntimeId { get; init; }
}

/// The Uia response payload (D-6.4 success/failure envelope shape):
/// `{"success":true,"data":[...]}` or `{"success":false,"error":"..."}`.
internal sealed record UiaTreeResponse
{
    [JsonPropertyName("success")]
    public required bool Success { get; init; }

    [JsonPropertyName("data")]
    public UiaElementRecord[]? Data { get; init; }

    [JsonPropertyName("error")]
    public string? Error { get; init; }
}

// ---------------------------------------------------------------------------
// UIA tree-walk handler (Task 2, this plan). Built entirely on top of
// UiaInterop.cs's proven [GeneratedComInterface] interfaces (07-02/07-03) --
// no new COM interop surface, no new <PackageReference>.
// ---------------------------------------------------------------------------

internal static class UiaTree
{
    /// Builds the flat `UiaElementRecord[]` for `hwnd`'s direct children
    /// (D-7.4): `ElementFromHandle(hwnd)` -> a single scoped
    /// `FindAll(TreeScope.Children, trueCondition)` COM call -- NEVER a
    /// tree-walker-style recursion. The ROOT element itself is included first
    /// (`depth=0, parent_runtime_id=[]`) so `depth`/`parent_id` are
    /// meaningfully populated (D-7.4's "depth ~0/1" flat-array bookkeeping),
    /// followed by each direct child (`depth=1`, `parent_runtime_id` = the
    /// root's RuntimeId).
    ///
    /// Never throws -- a per-element `COMException` (an element destroyed
    /// between `FindAll` and the property read) skips ONLY that element
    /// (D-7.6); the whole method body is wrapped in a top-level try/catch
    /// that degrades to `Success=false` only on total failure (D-6.4),
    /// mirroring `WindowEnumeration.BuildWindowListResponse`'s discipline
    /// exactly. Program.cs's caller also wraps this in its own try/catch as
    /// a second line of defense, same as every other handler in this file.
    internal static UiaTreeResponse BuildUiaTreeResponse(ulong hwnd)
    {
        try
        {
            IUIAutomation automation = UiaInterop.GetRootAutomation();
            IUIAutomationElement root = automation.ElementFromHandle((nint)hwnd);

            List<UiaElementRecord> records = [];

            // The root itself: depth=0, no parent in this flat result.
            int[] rootRuntimeId = UiaInterop.ReadRuntimeId(root);
            records.Add(BuildRecord(root, rootRuntimeId, depth: 0, parentRuntimeId: []));

            // D-7.4: the SINGLE scoped call -- never recursive tree-walking.
            IUIAutomationCondition trueCondition = automation.CreateTrueCondition();
            IUIAutomationElementArray children = root.FindAll(TreeScope.Children, trueCondition);
            int childCount = children.GetLength();

            for (int i = 0; i < childCount; i++)
            {
                try
                {
                    IUIAutomationElement child = children.GetElement(i);
                    int[] childRuntimeId = UiaInterop.ReadRuntimeId(child);
                    records.Add(BuildRecord(child, childRuntimeId, depth: 1, parentRuntimeId: rootRuntimeId));
                }
                catch (System.Runtime.InteropServices.COMException)
                {
                    // Element destroyed/unavailable between FindAll and here
                    // (e.g. UIA_E_ELEMENTNOTAVAILABLE) -- skip, don't fail
                    // the whole response over one stale element (D-7.6,
                    // mirrors WindowEnumeration's per-window "destroyed
                    // between EnumWindows and here" skip pattern).
                    continue;
                }
            }

            return new UiaTreeResponse { Success = true, Data = [.. records], Error = null };
        }
        catch (Exception ex)
        {
            return new UiaTreeResponse { Success = false, Data = null, Error = ex.Message };
        }
    }

    /// Reads the D-7.1 property set for a single element (naive, uncached
    /// per-property COM reads -- D-7.7; do NOT add bulk cache-request
    /// retrieval here, that is a live-gate-driven optimization deferred to 07-05 if
    /// SC#3's 500ms budget is at risk). `CurrentName` goes through
    /// `UiaInterop.ReadName` and `id`'s source array through
    /// `UiaInterop.ReadRuntimeId` (both live-diagnosed leak-safe manual
    /// decodes, 07-03) -- never a plain `string`/typed-array property.
    ///
    /// Bbox conversion mirrors `WindowEnumeration`'s `WindowRect` path
    /// exactly (SC#2): physical virtual-desktop pixels, `w = Right-Left`,
    /// `h = Bottom-Top`, negative coordinates clamped to 0 (no DPI/logical
    /// scaling).
    private static UiaElementRecord BuildRecord(IUIAutomationElement element, int[] runtimeId, uint depth, int[] parentRuntimeId)
    {
        Rect32 rect = element.GetCurrentBoundingRectangle();

        return new UiaElementRecord
        {
            RuntimeId = runtimeId,
            ControlType = element.GetCurrentControlType(),
            Name = UiaInterop.ReadName(element),
            Bbox = new WindowRect
            {
                X = (uint)Math.Max(0, rect.Left),
                Y = (uint)Math.Max(0, rect.Top),
                W = (uint)Math.Max(0, rect.Right - rect.Left),
                H = (uint)Math.Max(0, rect.Bottom - rect.Top),
            },
            Enabled = element.GetCurrentIsEnabled(),
            Visible = !element.GetCurrentIsOffscreen(),
            Focusable = element.GetCurrentIsKeyboardFocusable(),
            Focused = element.GetCurrentHasKeyboardFocus(),
            Depth = depth,
            ParentRuntimeId = parentRuntimeId,
        };
    }
}
