# Feature Landscape

**Domain:** AI-driven computer use over RDP — Windows SDK for remote desktop perception and control
**Researched:** 2026-06-04
**Scope anchor:** v1 = READ/INSPECT tasks proven by a scripted harness. No live LLM, no heavy write automation.

---

## Prior Art: How Agents Expect to Consume Computer Use

Understanding the consumption shape is prerequisite to categorizing features. Every major computer-use framework was examined.

### Anthropic Claude computer_20251124 (HIGH confidence)

Source: [Anthropic Computer Use Tool Docs](https://platform.claude.com/docs/en/docs/agents-and-tools/tool-use/computer-use-tool)

**Action vocabulary (what the agent emits):**
- `screenshot` — capture current display; returns base64 PNG
- `left_click` / `right_click` / `middle_click` / `double_click` / `triple_click` — `{coordinate: [x,y]}`
- `mouse_move` — `{coordinate: [x,y]}`
- `left_click_drag` — `{startCoordinate, coordinate}`
- `left_mouse_down` / `left_mouse_up` — fine-grained drag control
- `type` — `{text: "..."}` — sends string as keyboard events
- `key` — `{text: "ctrl+s"}` — key combo
- `hold_key` — hold key for duration (seconds)
- `scroll` — `{coordinate, scroll_direction, scroll_amount}`
- `wait` — pause
- `zoom` — `{region: [x1,y1,x2,y2]}` — view sub-region at full res (v20251124 only)
- Modifier keys via `text` parameter on click/scroll: `"shift"`, `"ctrl"`, `"alt"`, `"super"`

**Tool declaration shape:**
```json
{
  "type": "computer_20251124",
  "name": "computer",
  "display_width_px": 1024,
  "display_height_px": 768,
  "enable_zoom": true
}
```

**Perception model:** Pixel screenshots only. No structured accessibility tree. The SDK is the harness that executes actions and returns `tool_result` with screenshot base64.

**Coordinate system:** Absolute pixel positions in the declared display dimensions. Agent generates coordinates; harness executes.

### OpenAI CUA / computer-use-preview (HIGH confidence)

Source: [OpenAI Computer Use Guide](https://developers.openai.com/api/docs/guides/tools-computer-use)

**Action types:** `click`, `double_click`, `drag`, `move`, `scroll`, `type`, `keypress`, `wait`, `screenshot`

Mouse actions accept optional `keys` array for modifier-assisted clicks (Ctrl+click, Shift+click). Drag takes a `path` of coordinate pairs.

**Perception model:** Pixel screenshots only, `detail: 'original'` preferred for click accuracy. No accessibility tree. Coordinates are absolute pixel space.

**Key insight:** OpenAI explicitly notes coordinate remapping is the harness's responsibility when screenshots are downscaled before being sent.

### Microsoft UFO / UFO2 (MEDIUM confidence)

Sources: [UFO paper](https://arxiv.org/html/2402.07939v1), [UFO2 paper](https://arxiv.org/html/2504.14603v1), [UFO docs](https://microsoft.github.io/UFO/)

**Perception model — dual layer:**
1. Screenshots: clean screenshot + annotated screenshot (Set-of-Marks overlay — bounding boxes numbered by control)
2. Structured UIA data per control: `name`, `control_type`, `bounding_rect`, `enabled`, `visible`

UFO focuses on 10 control types: Button, Edit, TabItem, Document, ListItem, MenuItem, TreeItem, ComboBox, Hyperlink, ScrollBar.

UFO2 adds a hybrid pipeline: UIA metadata + OmniParser v2 vision detections, merged by IoU deduplication, producing "pseudo-UIA objects" that cover custom-rendered controls not exposed via accessibility.

**Action vocabulary:** Click, SetText, GetText, Scroll, Annotate, Summary — plus native Windows API calls when available (preferred over GUI when semantically equivalent).

**Key insight for rdpilot:** UFO treats UIA + screenshot as complementary, not alternatives. The agent receives both a visual and a structured signal per turn. This is the most directly relevant prior art — it uses the same Windows UIA backend that rdpilot's sensor helper will expose.

### Microsoft OmniParser (MEDIUM confidence)

Source: [OmniParser](https://microsoft.github.io/OmniParser/)

Converts UI screenshots into structured DOM-like element lists: bounding boxes, interactability flags, semantic descriptions from OCR + icon recognition. Used as a perception augmentation layer on top of screenshots. Output: list of `{bbox, label, description, interactable: bool}`.

**Key insight:** OmniParser's output format — flat list of labeled bounding boxes — represents the minimal perception shape an agent needs when no accessibility tree is available. rdpilot's UIA tree already provides this more accurately; OmniParser is the fallback for custom-rendered controls.

### Playwright Accessibility Snapshot (MEDIUM confidence)

Source: [Playwright ARIA Snapshots](https://playwright.dev/docs/aria-snapshots)

YAML tree of accessible elements: `role`, `name`, `attributes` (checked, disabled, expanded, level, pressed, selected). Playwright Agent CLI adds per-element `ref` identifiers for addressing elements without coordinates.

**Key insight for shaping the UIA output:** Playwright's snapshot proves agents work well with a *flat-ish* representation where each element has role + name + state flags + a stable ref. The tree hierarchy is preserved but agents mostly address individual elements by ref, not by traversing parent-child chains.

### Windows UI Automation element properties (HIGH confidence)

Source: [MS UIA Property IDs](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-automation-element-propids)

Canonical per-element properties available from the Windows UIA API:
- `Name` — accessible name (button label, field caption)
- `ControlType` — one of 38 types (Button, CheckBox, ComboBox, DataGrid, Document, Edit, Hyperlink, Image, List, ListItem, Menu, MenuItem, ProgressBar, RadioButton, ScrollBar, Tab, TabItem, Text, ToolBar, ToolTip, Tree, TreeItem, Window, etc.)
- `BoundingRectangle` — `{left, top, width, height}` in physical screen pixels
- `IsEnabled` / `IsOffscreen` / `IsKeyboardFocusable` / `HasKeyboardFocus`
- `AutomationId` — stable programmatic identifier (when set by the app)
- `ClassName` — Win32/WPF class name
- `ProcessId` — owning process
- `NativeWindowHandle` — HWND
- Pattern support: `InvokePattern`, `ValuePattern`, `SelectionPattern`, `ExpandCollapsePattern`, `ScrollPattern`, `TextPattern`, etc.

---

## Recommended Perception Shape for rdpilot

Based on the above prior art, the SDK's structured perception output for a window's UIA tree should be a **flat array of element descriptors**, not a raw tree dump:

```typescript
interface UiaElement {
  id: string;              // stable ref — AutomationId or generated path hash
  role: string;            // ControlType name, e.g. "Button", "Edit", "MenuItem"
  name: string;            // accessible name
  bbox: { x: number; y: number; w: number; h: number };  // desktop coords
  enabled: boolean;
  visible: boolean;        // !IsOffscreen
  focusable: boolean;      // IsKeyboardFocusable
  focused: boolean;        // HasKeyboardFocus
  value?: string;          // ValuePattern.Value when available
  expandable?: boolean;    // ExpandCollapsePattern present
  expanded?: boolean;
  depth: number;           // tree depth — preserves rough hierarchy without full nesting
  parentId?: string;       // optional parent ref for reconstruction
}
```

**Why flat:** Agents (Anthropic, OpenAI, UFO) all address elements by coordinate or ref, not by tree traversal. Flat arrays are easier to embed in prompts, filter, and pass to the action dispatcher. Tree structure can be reconstructed from `parentId` + `depth` when needed but is not the primary shape.

**Why bounding boxes in desktop coords:** Agents map between the screenshot (pixel space) and the element list. Bounding boxes must be in the same coordinate space as the screenshot, which for rdpilot is the remote desktop coordinate space.

---

## Table Stakes (Must-Have for Read/Inspect v1)

These features are required for the v1 harness to connect, perceive, and report.

### TS-1: Session Connect / Auth (NLA + credentials)

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | Requires RDP stack choice (IronRDP/FreeRDP/MSTSC wrapper) |
| **Why required** | Nothing works without a connected session |

Establish an RDP connection to a Windows target using Network Level Authentication (NLA/CredSSP). Accept hostname, port, username, password (+ domain). Handle auth errors as typed exceptions.

### TS-2: Session Lifecycle (open, keepalive, teardown)

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low-Medium |
| **Dependencies** | TS-1 |
| **Why required** | Long-running harness loops need stable sessions; disconnects must be detected and surfaced |

Manage session state machine: connecting → connected → disconnected → reconnecting. Keepalive heartbeat to prevent idle timeout. Graceful teardown (logoff vs disconnect). Expose session status as observable/event stream.

### TS-3: Screenshot — Full Desktop

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low |
| **Dependencies** | TS-1, RDP bitmap channel |
| **Why required** | Primary perception input for all agent frameworks |

Capture the current full-desktop frame from the RDP bitmap channel. Return as PNG bytes + display dimensions. This is the universal computer-use harness contract.

### TS-4: Screenshot — Per-Window (crop to window bounds)

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low (given TS-3 + TS-7) |
| **Dependencies** | TS-3, TS-7 (window geometry) |
| **Why required** | Reduces token cost and noise for agents focused on a specific app; UFO and rdpilot's own read/inspect use case centers on a specific remote-only program |

Crop the full desktop screenshot to a specific window's bounding rectangle. Returns image + the window's top-left offset (so coordinates in the crop are remappable to desktop space).

### TS-5: Mouse Input — Move, Click, Scroll, Drag

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low-Medium |
| **Dependencies** | TS-1, RDP input channel |
| **Why required** | Navigation (clicking tabs, expanding nodes, scrolling lists) is needed even for read/inspect tasks |

Map Anthropic/OpenAI action vocabulary to RDP input injection:
- `mouse_move({x,y})`
- `left_click({x,y})`, `right_click({x,y})`, `double_click({x,y})`
- `scroll({x, y, direction, amount})`
- `left_click_drag({from, to})`

Coordinates are in remote desktop pixel space.

### TS-6: Keyboard Input — Type Text + Key Combos

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low-Medium |
| **Dependencies** | TS-1, RDP input channel |
| **Why required** | Search fields, filter inputs, navigation shortcuts (Alt+Tab, Tab, Enter, arrow keys) are common even in read/inspect flows |

- `type_text(text: string)` — Unicode string → scancode sequence
- `key_combo(keys: string[])` — e.g. `["ctrl","a"]`, `["alt","tab"]`, `["F5"]`

### TS-7: Window Enumeration (titles, geometry, foreground/z-order)

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low (via sensor helper) |
| **Dependencies** | Sensor helper transport (TS-10) |
| **Why required** | Agent must know which window to focus; cropped screenshots require geometry; navigation requires foreground state |

Via sensor helper: return list of visible windows with:
- HWND, process ID, title, class name
- Bounding rect (screen coordinates)
- Z-order / is-foreground flag
- Is-minimized / is-maximized state

### TS-8: UIA Tree Retrieval (structured accessibility snapshot)

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium-High |
| **Dependencies** | Sensor helper transport (TS-10), UIA API on remote |
| **Why required** | Core differentiator of rdpilot; the "structured perception" half of the dual-layer model; critical for read/inspect without vision-only scraping |

Via sensor helper: retrieve UIA element tree for a target window (or desktop root). Serialize to the flat `UiaElement[]` shape described above. Include: id, role, name, bbox, enabled, visible, focusable, focused, value, depth.

Options for scope: full tree (depth-limited), subtree from element, filtered by control type.

### TS-9: Process Tree Enumeration

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low (via sensor helper / WMI/WinRM) |
| **Dependencies** | Sensor helper or WinRM channel |
| **Why required** | Agent needs to verify the target program is running; launch verification; process/window correlation |

Return: PID, parent PID, name (image name), command line, session ID, memory usage (optional). Correlates with window list via PID.

### TS-10: Sensor Helper Bootstrap + Transport

| Attribute | Value |
|-----------|-------|
| **Complexity** | High |
| **Dependencies** | TS-1 (RDP session), choice of transport (RDP virtual channel vs WinRM) |
| **Why required** | All structured perception (TS-7, TS-8, TS-9) is sourced from the sensor helper; nothing structured works without it |

Deploy a thin Windows executable/service to the remote machine (via RDP file transfer or WinRM). Establish a communication channel (RDP virtual channel preferred — in-band; WinRM as fallback). The helper is a dumb request/response sensor: it executes perception queries (GetWindows, GetUiaTree, GetProcessList) and returns serialized results. It has zero agent logic.

### TS-11: Remote Process Launch + Observe

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | WinRM or sensor helper shell channel |
| **Why required** | Harness proof requires launching a program (e.g., the remote-only app) and confirming it started |

Launch a named executable on the remote machine (e.g. `pwsh.exe`, `notepad.exe`, `target_app.exe`). Return PID. Poll / wait for process to appear in window list. Capture stdout/stderr for console processes via WinRM.

### TS-12: Wait / Idle Detection

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | TS-3 (screenshot diff), TS-7 (window state), optional UIA event stream |
| **Why required** | Agent loops must know when the UI has settled before acting; acting on a loading state produces garbage |

Two tiers:
1. **Screen-stable wait** — poll screenshots at interval; declare stable when pixel diff falls below threshold for N frames. Simple, reliable.
2. **Window/process ready wait** — poll until target window appears in enumeration and is not minimized.

Advanced (post-v1): hook into UIA event stream for change notifications (foreground changes, structure changes).

### TS-13: Clipboard Read/Write

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low-Medium |
| **Dependencies** | RDP clipboard virtual channel or sensor helper |
| **Why required** | Read: extracting text from remote app (copy a value the agent found); Write: pasting text into fields faster than keyboard simulation |

Get/set remote clipboard text. The RDP protocol has a clipboard virtual channel (CLIPRDR); alternatively the sensor helper can drive the clipboard API directly.

---

## Differentiators (Valuable but Not Required for v1)

### D-1: UIA Event Streaming (change notifications)

| Attribute | Value |
|-----------|-------|
| **Complexity** | High |
| **Dependencies** | TS-10, persistent sensor helper connection |

Subscribe to UIA automation events (structure changed, property changed, focus changed) and stream them to the local SDK consumer. Enables reactive agent loops instead of polling. Useful post-v1 for efficiency.

### D-2: Per-Element Screenshot / Zoom

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low (given TS-4 + TS-8) |
| **Dependencies** | TS-4, TS-8 |

Crop screenshot to a specific UIA element's bounding box. Mirrors Claude's `zoom` action. Useful for inspecting small text, icons, data cells. Natural extension once TS-4 and TS-8 exist.

### D-3: Set-of-Marks Screenshot Rendering

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | TS-3, TS-8 |

Overlay numbered bounding boxes on the screenshot for each UIA element — the same technique UFO uses for annotated screenshots. Reduces coordinate guessing; agent can refer to element "12" and the harness maps that to the bbox center. Aligns with OmniParser output shape.

### D-4: UIA Invoke / Value Pattern (programmatic interaction)

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | TS-10, sensor helper |

Use UIA control patterns (InvokePattern for buttons, ValuePattern for edits, ExpandCollapsePattern for tree/combo) instead of simulated mouse clicks. More reliable than coordinate-based clicking for known control types. UFO2 classifies this as "native API preferred over GUI."

### D-5: Session Reconnect / Resume

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | TS-2 |

Detect disconnection and attempt reconnection, restoring session state. Important for long-running agent loops over flaky or rate-limited RDP connections.

### D-6: Multi-Monitor Awareness

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | TS-3, TS-7 |

Enumerate remote monitors, capture per-monitor screenshots, map window positions across monitors. Required when the remote-only program spans or lives on a secondary display.

### D-7: Resolution / DPI Negotiation

| Attribute | Value |
|-----------|-------|
| **Complexity** | Medium |
| **Dependencies** | TS-1 |

Negotiate RDP session display resolution and DPI to match what the agent models expect (e.g., 1280x800 or 1920x1080). Prevents the 1024x768 fallback that breaks many UI automation setups when the session was originally connected at a different resolution.

### D-8: Structured Output — JSON-serializable Snapshot

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low (given TS-8) |
| **Dependencies** | TS-8 |

First-class serialization of the `UiaElement[]` snapshot to JSON (and back), with a stable schema version. Enables logging, diffing snapshots across time, and replaying agent sessions. Low implementation cost; high value for debugging.

### D-9: Window Focus / Bring-to-Front

| Attribute | Value |
|-----------|-------|
| **Complexity** | Low |
| **Dependencies** | TS-10, sensor helper |

Programmatically bring a window to the foreground (SetForegroundWindow) via the sensor helper. More reliable than Alt+Tab simulation for ensuring the target window is active before interaction.

---

## Anti-Features (Explicitly NOT Building)

These are deliberate non-goals for v1, some permanently, some deferred.

### AF-1: AI Agent Logic on the Remote Machine

**Why exclude:** The entire point of rdpilot is that intelligence stays local. Running the agent on the remote session is the "obvious" path that rdpilot explicitly rejects.

**What to do instead:** The sensor helper is a dumb request/response component. It executes perception queries; it makes no decisions.

### AF-2: MCP Server / Anthropic Shim / CLI Packaging

**Why exclude (v1):** v1's first consumer is a scripted harness, not a live LLM. Packaging is a later milestone. Adding it now mixes SDK concerns with delivery format concerns.

**What to do instead:** The SDK's clean API is the foundation. MCP/shim wrappers are added in a subsequent milestone once the API stabilizes on real usage.

### AF-3: Non-Windows RDP Targets (Linux/xrdp, macOS)

**Why exclude:** Windows-only v1 lets the SDK use UIA, WinRM, WMI, RAIL, PowerShell — none of which exist on Linux/xrdp. Cross-platform lowers to lowest-common-denominator pixel scraping, eliminating the structured perception advantage.

**What to do instead:** Windows-only for v1. Re-evaluate after the core loop is proven.

### AF-4: Heavy Write / Destructive Automation Guardrails

**Why exclude (v1):** Read/inspect tasks have low blast radius. Write guardrails (confirm-before-delete, action auditing, rollback) are non-trivial and should not be designed without real write-task usage driving requirements.

**What to do instead:** Build the read path cleanly. Add write guardrails in a dedicated milestone when actual write tasks are defined.

### AF-5: Published-Package Polish (public API stability, multi-registry distribution)

**Why exclude (v1):** Personal tooling first. Stability guarantees and polished public docs add constraint before the API has been validated by real usage.

**What to do instead:** Design the API to be clean (it will become public), but do not freeze it or invest in distribution until after v1 validation.

### AF-6: Multi-Session Orchestration / Concurrency at Scale

**Why exclude (v1):** One session, one agent. Concurrency introduces session pooling, locking, and resource management that are orthogonal to proving the single-session loop.

**What to do instead:** Single-session SDK that can be instantiated multiple times independently; concurrency is the caller's concern.

### AF-7: Vision-Only Perception (OmniParser replacement)

**Why exclude:** rdpilot's value is the dual-layer model (pixels + UIA structure). Building a custom vision-based element detector duplicates OmniParser/UFO2 without adding value over what Windows UIA already provides more accurately.

**What to do instead:** Use UIA for structured perception. If a specific app has custom-rendered controls not in the UIA tree, that is a later research item, not a v1 concern.

### AF-8: Remote-Assist Co-Driving UX

**Why exclude:** Human-in-the-loop co-driving (agent + human simultaneously controlling the same session) is a UX product, not an SDK feature. Out of scope.

---

## Feature Dependencies Map

```
TS-1 (Connect/Auth)
  └─ TS-2 (Lifecycle)
  └─ TS-3 (Screenshot Full)
       └─ TS-4 (Screenshot Window) ── requires TS-7
       └─ TS-12 (Wait/Idle) ── screen-stable tier
  └─ TS-5 (Mouse Input)
  └─ TS-6 (Keyboard Input)
  └─ TS-10 (Sensor Helper)
       └─ TS-7 (Window Enum)
            └─ TS-4 (Screenshot Window)
            └─ TS-12 (Wait/Idle) ── window-ready tier
       └─ TS-8 (UIA Tree)
       └─ TS-9 (Process Tree)
       └─ TS-11 (Remote Process Launch) ── also uses WinRM
       └─ TS-13 (Clipboard)
```

**Critical path for v1 harness:**
TS-1 → TS-2 → TS-10 → {TS-3, TS-7, TS-8, TS-9} → TS-4, TS-11, TS-12

---

## MVP Feature Set (v1 read/inspect harness)

**Must build (all 13 table-stakes features):**
1. TS-1: Connect/NLA auth
2. TS-2: Session lifecycle
3. TS-3: Full desktop screenshot
4. TS-4: Per-window screenshot
5. TS-5: Mouse input
6. TS-6: Keyboard input
7. TS-7: Window enumeration
8. TS-8: UIA tree retrieval (flat `UiaElement[]` shape)
9. TS-9: Process tree enumeration
10. TS-10: Sensor helper bootstrap + transport
11. TS-11: Remote process launch + observe
12. TS-12: Wait / idle detection
13. TS-13: Clipboard read/write

**Strongly recommended for v1 (low cost, high agent-compatibility value):**
- D-8: JSON-serializable snapshot (no extra implementation beyond TS-8)
- D-9: Window focus / bring-to-front (trivial via sensor helper; needed before almost every interaction)

**Defer (build after v1 validates the core loop):**
- D-1 through D-7: all meaningful but not required to prove read/inspect

---

## Sources

- [Anthropic Computer Use Tool Documentation](https://platform.claude.com/docs/en/docs/agents-and-tools/tool-use/computer-use-tool) — HIGH confidence, official
- [OpenAI Computer Use Guide](https://developers.openai.com/api/docs/guides/tools-computer-use) — HIGH confidence, official
- [UFO: A UI-Focused Agent for Windows OS Interaction (arXiv)](https://arxiv.org/html/2402.07939v1) — MEDIUM confidence, research paper
- [UFO2: The Desktop AgentOS (arXiv)](https://arxiv.org/html/2504.14603v1) — MEDIUM confidence, research paper
- [UFO3 Documentation](https://microsoft.github.io/UFO/) — MEDIUM confidence, official project docs
- [OmniParser for Pure Vision Based GUI Agent](https://microsoft.github.io/OmniParser/) — MEDIUM confidence, official project docs
- [Playwright ARIA Snapshots](https://playwright.dev/docs/aria-snapshots) — HIGH confidence, official docs
- [Windows UIA Automation Element Property IDs](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-automation-element-propids) — HIGH confidence, official Microsoft docs
- [pywinauto UIA Element Info](https://pywinauto.readthedocs.io/en/latest/code/pywinauto.uia_element_info.html) — MEDIUM confidence, library docs
