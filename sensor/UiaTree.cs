// UiaTree DTOs for the RDPILOT_SENSOR wire protocol (PERC-03, 07-04-PLAN.md
// Task 1; `max_depth` added by 09-02-PLAN.md Task 2, D-9.1). Wire schema is
// authoritatively defined by 09-02's Rust-side `Session::get_uia_tree`
// (crates/rdpilot/src/session.rs) -- the JSON keys below MUST match it
// EXACTLY (snake_case via [JsonPropertyName]):
//   request payload:  {"hwnd": <u64>, "max_depth": <u32>}
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

/// The Uia request payload: `{"hwnd": <u64>, "max_depth": <u32>}`
/// (`max_depth` added by 09-02-PLAN.md Task 2, D-9.1 -- the caller-configurable
/// deeper-walk bound; `1` is the D-7.4-default children-only walk).
internal sealed record UiaTreeRequest
{
    [JsonPropertyName("hwnd")]
    public required ulong Hwnd { get; init; }

    [JsonPropertyName("max_depth")]
    public required uint MaxDepth { get; init; }
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
    /// Safety-cap on the deeper walk's clamped depth (D-9.1, 09-02-PLAN.md
    /// Task 2, T-09-10): the effective walk depth is ALWAYS
    /// `Math.Min(maxDepth, UIA_MAX_WALK_DEPTH)`, regardless of the caller's
    /// requested `max_depth`. This protects the Phase 7 SC#3 500ms
    /// sensor-side walk budget against a pathologically large caller-supplied
    /// value. Conservative starting value; live-tuned at the 09-04 gate
    /// against a real 7-Zip window if the budget is at risk.
    private const uint UIA_MAX_WALK_DEPTH = 4;

    /// Builds the flat `UiaElementRecord[]` for `hwnd`, walking BOUNDED,
    /// LEVEL-BY-LEVEL from the root down to
    /// `Math.Min(maxDepth, UIA_MAX_WALK_DEPTH)` levels (D-7.4's original
    /// depth-1 default, extended by D-9.1's flagged deeper walk): each level
    /// is its own scoped `FindAll(TreeScope.Children, trueCondition)` COM
    /// call per parent discovered at the level above -- NEVER a single
    /// uncapped whole-subtree walk (never `TreeScope` `.Subtree`, T-09-10). The ROOT element itself
    /// is included first (`depth=0, parent_runtime_id=[]`), followed by each
    /// level's children (`depth=d`, `parent_runtime_id` = that level's
    /// parent's RuntimeId). `maxDepth` of `0` or `1` reproduces the exact
    /// prior D-7.4 children-only behavior (root + depth-1 children, no
    /// deeper levels walked).
    ///
    /// Never throws -- a per-element `COMException` (an element destroyed
    /// between a level's `FindAll` and the property read) skips ONLY that
    /// element, at ANY level (D-7.6); the whole method body is wrapped in a
    /// top-level try/catch that degrades to `Success=false` only on total
    /// failure (D-6.4), mirroring `WindowEnumeration.BuildWindowListResponse`'s
    /// discipline exactly. Program.cs's caller also wraps this in its own
    /// try/catch as a second line of defense, same as every other handler in
    /// this file.
    internal static UiaTreeResponse BuildUiaTreeResponse(ulong hwnd, uint maxDepth)
    {
        try
        {
            IUIAutomation automation = UiaInterop.GetRootAutomation();
            IUIAutomationElement root = automation.ElementFromHandle((nint)hwnd);
            IUIAutomationCondition trueCondition = automation.CreateTrueCondition();

            List<UiaElementRecord> records = [];

            // The root itself: depth=0, no parent in this flat result.
            int[] rootRuntimeId = UiaInterop.ReadRuntimeId(root);
            records.Add(BuildRecord(root, rootRuntimeId, depth: 0, parentRuntimeId: []));

            // T-09-10: clamp the caller-requested depth to the safety cap --
            // regardless of what max_depth the wire request carries.
            uint effectiveMaxDepth = Math.Min(maxDepth, UIA_MAX_WALK_DEPTH);

            // Level-by-level BFS: `currentLevel` holds each (element,
            // runtimeId) pair discovered at the PREVIOUS level (starting
            // with just the root at depth 0). Each level performs one
            // per-parent `FindAll(TreeScope.Children, ...)` call -- the same
            // scoped D-7.4 primitive, reused at every depth (never a
            // whole-subtree walk).
            List<(IUIAutomationElement Element, int[] RuntimeId)> currentLevel = [(root, rootRuntimeId)];

            for (uint depth = 1; depth <= effectiveMaxDepth && currentLevel.Count > 0; depth++)
            {
                List<(IUIAutomationElement Element, int[] RuntimeId)> nextLevel = [];

                foreach ((IUIAutomationElement parent, int[] parentRuntimeId) in currentLevel)
                {
                    IUIAutomationElementArray children;
                    try
                    {
                        children = parent.FindAll(TreeScope.Children, trueCondition);
                    }
                    catch (System.Runtime.InteropServices.COMException)
                    {
                        // The parent itself was destroyed/unavailable between
                        // being discovered at the level above and this
                        // level's FindAll call -- skip its subtree, don't
                        // fail the whole response over one stale branch
                        // (D-7.6, same skip-and-continue discipline as the
                        // per-element read below).
                        continue;
                    }

                    int childCount = children.GetLength();
                    for (int i = 0; i < childCount; i++)
                    {
                        try
                        {
                            IUIAutomationElement child = children.GetElement(i);
                            int[] childRuntimeId = UiaInterop.ReadRuntimeId(child);
                            records.Add(BuildRecord(child, childRuntimeId, depth, parentRuntimeId));
                            nextLevel.Add((child, childRuntimeId));
                        }
                        catch (System.Runtime.InteropServices.COMException)
                        {
                            // Element destroyed/unavailable between FindAll
                            // and here (e.g. UIA_E_ELEMENTNOTAVAILABLE) --
                            // skip, don't fail the whole response over one
                            // stale element (D-7.6, mirrors
                            // WindowEnumeration's per-window "destroyed
                            // between EnumWindows and here" skip pattern).
                            continue;
                        }
                    }
                }

                currentLevel = nextLevel;
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
