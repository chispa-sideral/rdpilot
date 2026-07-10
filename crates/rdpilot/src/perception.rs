//! Owned SDK perception types (PERC-01, PERC-02) plus the crate-internal
//! wire structs and conversions that deserialize `RDPILOT_SENSOR` DVC reply
//! payloads into them.
//!
//! Only the owned public types ([`WindowInfo`], [`WindowState`],
//! [`ProcessInfo`]) leave this crate's public API (D-09): the `*Wire` structs
//! below exist purely to `serde::Deserialize` the snake_case wire shape (the
//! Phase 6 wire contract, `06-01-PLAN.md`) and are never exposed. Keeping the
//! owned types serde-free means the public API is not coupled to the wire
//! format and can evolve independently of it.
//!
//! Malformed/unexpected-shape reply JSON deserializes to a typed `Err`
//! (missing/extra fields, wrong types) rather than panicking — callers (a
//! later plan's `Session::get_window_list`/`get_process_tree`) map that to a
//! [`crate::Error`], never `unwrap`/`expect`/`panic!` (API-01, T-06-01).
//!
//! `rect` is in physical virtual-desktop pixels (the framebuffer coordinate
//! space), matching [`crate::Rect`]'s existing contract — the C# sensor MUST
//! NOT emit DPI-scaled logical coordinates (D-6.3).

use serde::{Deserialize, Serialize};

/// A single top-level window on the remote desktop (PERC-01).
///
/// Reuses [`crate::Rect`] (the existing public rectangle type from
/// `screenshot.rs`) so callers crop a [`crate::Screenshot`] with the exact
/// geometry a window reports, in the same coordinate space.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowInfo {
    /// The native Win32 window handle (`HWND`), as a raw integer.
    pub hwnd: u64,
    /// The window's title bar text (may be empty for untitled windows).
    pub title: String,
    /// The window's bounding rectangle, in physical virtual-desktop pixels
    /// (the framebuffer coordinate space, D-6.3).
    pub rect: crate::Rect,
    /// Z-order position: `0` is topmost.
    pub z_order: u32,
    /// Whether the window is normal, minimized, or maximized.
    pub state: WindowState,
    /// The window class name (e.g. `"Notepad"`, `"CabinetWClass"`).
    pub class_name: String,
    /// The process id that owns this window.
    pub pid: u32,
}

/// A window's visibility/restore state (PERC-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    /// Neither minimized nor maximized.
    Normal,
    /// Minimized (iconic).
    Minimized,
    /// Maximized (zoomed).
    Maximized,
}

/// A single process in the remote machine's process tree (PROC-01).
///
/// `pid`/`parent_pid`/`name`/`path` are the D-6.3 hard-requirement floor;
/// `command_line`/`owner` are best-effort extras that gracefully degrade to
/// `None` when the sensor cannot retrieve them for a given process (D-6.3).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessInfo {
    /// The process id.
    pub pid: u32,
    /// The parent process id.
    pub parent_pid: u32,
    /// The process's image (executable) file name.
    pub name: String,
    /// The full path to the process's executable image.
    pub path: String,
    /// The process's command line, if the sensor could retrieve it
    /// (best-effort extra, D-6.3).
    pub command_line: Option<String>,
    /// The process's owning user account, if the sensor could retrieve it
    /// (best-effort extra, D-6.3).
    pub owner: Option<String>,
}

/// Crate-internal wire shape for [`crate::Rect`] (the Phase 6 wire contract's
/// `{"x":..,"y":..,"w":..,"h":..}` object).
///
/// A separate wire struct (rather than deriving `Deserialize` on
/// [`crate::Rect`] itself) keeps the public [`crate::Rect`] type serde-free
/// (D-09) — the wire format is this module's concern alone.
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Consumed by Plan 02's Session::get_window_list (interface-first).
pub(crate) struct RectWire {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl RectWire {
    #[allow(dead_code)] // Consumed by Plan 02's Session::get_window_list (interface-first).
    pub(crate) fn into_owned(self) -> crate::Rect {
        crate::Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

/// Crate-internal wire shape for [`WindowState`]: the wire contract's
/// lowercase string enum (`"normal"`/`"minimized"`/`"maximized"`).
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Consumed by Plan 02's Session::get_window_list (interface-first).
#[serde(rename_all = "lowercase")]
pub(crate) enum WindowStateWire {
    Normal,
    Minimized,
    Maximized,
}

impl WindowStateWire {
    #[allow(dead_code)] // Consumed by Plan 02's Session::get_window_list (interface-first).
    pub(crate) fn into_owned(self) -> WindowState {
        match self {
            WindowStateWire::Normal => WindowState::Normal,
            WindowStateWire::Minimized => WindowState::Minimized,
            WindowStateWire::Maximized => WindowState::Maximized,
        }
    }
}

/// Crate-internal wire shape for [`WindowInfo`] (the Phase 6 wire contract's
/// `WindowList` `data` array element).
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Consumed by Plan 02's Session::get_window_list (interface-first).
pub(crate) struct WindowInfoWire {
    hwnd: u64,
    title: String,
    rect: RectWire,
    z_order: u32,
    state: WindowStateWire,
    class_name: String,
    pid: u32,
}

impl WindowInfoWire {
    #[allow(dead_code)] // Consumed by Plan 02's Session::get_window_list (interface-first).
    pub(crate) fn into_owned(self) -> WindowInfo {
        WindowInfo {
            hwnd: self.hwnd,
            title: self.title,
            rect: self.rect.into_owned(),
            z_order: self.z_order,
            state: self.state.into_owned(),
            class_name: self.class_name,
            pid: self.pid,
        }
    }
}

/// Crate-internal wire shape for [`ProcessInfo`] (the Phase 6 wire contract's
/// `ProcessTree` `data` array element).
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Consumed by Plan 02's Session::get_process_tree (interface-first).
pub(crate) struct ProcessInfoWire {
    pid: u32,
    parent_pid: u32,
    name: String,
    path: String,
    command_line: Option<String>,
    owner: Option<String>,
}

impl ProcessInfoWire {
    #[allow(dead_code)] // Consumed by Plan 02's Session::get_process_tree (interface-first).
    pub(crate) fn into_owned(self) -> ProcessInfo {
        ProcessInfo {
            pid: self.pid,
            parent_pid: self.parent_pid,
            name: self.name,
            path: self.path,
            command_line: self.command_line,
            owner: self.owner,
        }
    }
}

/// The caller-configurable scope for a [`crate::Session::get_uia_tree`] walk
/// (D-9.1) — an owned SDK type only, no wire/`ironrdp` leak (D-09).
///
/// `Children` is the original D-7.4-locked default: a depth-1
/// `TreeScope_Children` walk (maps to wire `max_depth: 1`), preserving
/// Phase 7/8 behavior and latency for every existing caller unchanged.
///
/// `Subtree { max_depth }` is the D-9.1 HUMAN-APPROVED flagged, scoped
/// addition: a bounded, level-by-level deeper walk (never an uncapped
/// `TreeScope_Subtree`) up to `max_depth` levels below the target window,
/// clamped sensor-side to a safety cap (`UIA_MAX_WALK_DEPTH` in
/// `sensor/UiaTree.cs`) to protect the Phase 7 SC#3 500ms sensor-side walk
/// budget. This re-assesses D-7.4's children-only lock as an explicit,
/// caller-opt-in capability -- not a silent redesign of the UIA subsystem.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum UiaScope {
    /// The D-7.4 default: `TreeScope_Children` only (wire `max_depth: 1`).
    Children,
    /// The D-9.1 flagged deeper walk, bounded to `max_depth` levels
    /// (sensor-side clamped to `UIA_MAX_WALK_DEPTH`).
    Subtree {
        /// How many levels below the target window to walk (wire `max_depth`).
        max_depth: u32,
    },
}

/// A single UI Automation element in a flat, `TreeScope_Children`-scoped
/// tree walk (PERC-03, D-7.4).
///
/// Reuses [`crate::Rect`] (the same public rectangle type as
/// [`WindowInfo::rect`]) so `bbox` shares the framebuffer/window-list
/// coordinate space: physical virtual-desktop pixels, `u32 x/y/w/h`
/// (SC#2, D-7.1).
///
/// `value` (`ValuePattern`) is explicitly DEFERRED to backlog (D-7.1) and
/// has no field here. `depth`/`parent_id` are flat-array bookkeeping fields
/// for a `TreeScope_Children`-only walk (depth is ~0/1) that future-proof
/// the wire shape for a later, deeper-tree phase (D-7.4).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiaElement {
    /// The element's stable-ish identity: the UIA `RuntimeId` int array
    /// joined to a single string (D-7.2). Chosen over `AutomationId`, which
    /// is empty on non-instrumented Win32 apps (e.g. Notepad).
    pub id: String,
    /// The element's UIA `ControlType` mapped to a stable, locale-independent
    /// English friendly string (e.g. `"Button"`, `"Window"`) — NOT
    /// `LocalizedControlType` (D-7.3). Unmapped/unknown `ControlType` ids
    /// map to `"Unknown"`.
    pub role: String,
    /// The element's UIA `Name` property (may be empty).
    pub name: String,
    /// The element's bounding rectangle, in physical virtual-desktop pixels
    /// (the framebuffer coordinate space, matching [`crate::Rect`] and
    /// [`WindowInfo::rect`] exactly — SC#2).
    pub bbox: crate::Rect,
    /// Whether the element is enabled (`IsEnabled` UIA property).
    pub enabled: bool,
    /// Whether the element is visible (derived from `IsOffscreen`/similar on
    /// the sensor side; wire-level boolean passthrough here).
    pub visible: bool,
    /// Whether the element is keyboard-focusable (`IsKeyboardFocusable`).
    pub focusable: bool,
    /// Whether the element currently has keyboard focus (`HasKeyboardFocus`).
    pub focused: bool,
    /// The element's depth in the flat tree-walk result. For the
    /// `TreeScope_Children`-only walk this phase performs, values are ~0
    /// (the root) / 1 (its direct children) — future-proofing for a later
    /// deeper-tree phase (D-7.4).
    pub depth: u32,
    /// The parent element's [`UiaElement::id`] (same `RuntimeId`-join
    /// format, D-7.2), or an empty string if there is no parent in this
    /// flat result.
    pub parent_id: String,
}

/// The 41 UIA `ControlType` ids (stable since Windows 8.1) mapped to a
/// stable, locale-independent English friendly string (D-7.3). Unknown ids
/// (including any future additions) map to `"Unknown"`.
///
/// Source: `learn.microsoft.com/en-us/windows/win32/winauto/uiauto-controltype-ids`
/// (07-RESEARCH.md Code Examples).
#[allow(dead_code)] // Consumed by Task 2's Session::get_uia_tree (interface-first).
pub(crate) fn control_type_to_role(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        50039 => "SemanticZoom",
        50040 => "AppBar",
        _ => "Unknown",
    }
}

/// Join a UIA `RuntimeId` (or `parent_runtime_id`) int array into a stable,
/// deterministic `"-"`-joined string (D-7.2, Claude's Discretion). `"-"`
/// avoids the ambiguity `"."` could introduce if a component were ever
/// negative. An empty slice joins to an empty string.
#[allow(dead_code)] // Consumed by Task 2's Session::get_uia_tree (interface-first).
pub(crate) fn runtime_id_to_string(ids: &[i32]) -> String {
    ids.iter().map(i32::to_string).collect::<Vec<_>>().join("-")
}

/// Crate-internal wire shape for [`UiaElement`] (the Phase 7 wire
/// contract's `Uia` `data` array element).
///
/// The sensor ships RAW `runtime_id`/`parent_runtime_id` int arrays and a
/// RAW `control_type` int; [`UiaElementWire::into_owned`] performs the
/// D-7.2 join and D-7.3 map here in the SDK (chosen for offline
/// testability and to keep the AOT sensor binary logic-thin, per
/// 07-RESEARCH's Architectural Responsibility Map).
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Consumed by Task 2's Session::get_uia_tree (interface-first).
pub(crate) struct UiaElementWire {
    runtime_id: Vec<i32>,
    control_type: i32,
    name: String,
    bbox: RectWire,
    enabled: bool,
    visible: bool,
    focusable: bool,
    focused: bool,
    depth: u32,
    parent_runtime_id: Vec<i32>,
}

impl UiaElementWire {
    #[allow(dead_code)] // Consumed by Task 2's Session::get_uia_tree (interface-first).
    pub(crate) fn into_owned(self) -> UiaElement {
        UiaElement {
            id: runtime_id_to_string(&self.runtime_id),
            role: control_type_to_role(self.control_type).to_string(),
            name: self.name,
            bbox: self.bbox.into_owned(),
            enabled: self.enabled,
            visible: self.visible,
            focusable: self.focusable,
            focused: self.focused,
            depth: self.depth,
            parent_id: runtime_id_to_string(&self.parent_runtime_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canned `WindowList` `data` JSON array (per the wire_contract)
    /// deserializes into `Vec<WindowInfo>` with correct
    /// hwnd/title/rect(x,y,w,h)/z_order/state/class_name/pid, and
    /// `state:"minimized"` maps to `WindowState::Minimized`.
    #[test]
    fn window_list_data_deserializes_into_owned_window_info() {
        let data = serde_json::json!([
            {
                "hwnd": 65536,
                "title": "Untitled - Notepad",
                "rect": {"x": 10, "y": 20, "w": 800, "h": 600},
                "z_order": 0,
                "state": "minimized",
                "class_name": "Notepad",
                "pid": 4242
            }
        ]);

        let wires: Vec<WindowInfoWire> =
            serde_json::from_value(data).expect("canned WindowList data deserializes");
        let windows: Vec<WindowInfo> = wires.into_iter().map(WindowInfoWire::into_owned).collect();

        assert_eq!(windows.len(), 1);
        let w = &windows[0];
        assert_eq!(w.hwnd, 65536);
        assert_eq!(w.title, "Untitled - Notepad");
        assert_eq!(
            w.rect,
            crate::Rect {
                x: 10,
                y: 20,
                w: 800,
                h: 600
            }
        );
        assert_eq!(w.z_order, 0);
        assert_eq!(w.state, WindowState::Minimized);
        assert_eq!(w.class_name, "Notepad");
        assert_eq!(w.pid, 4242);
    }

    /// A canned `ProcessTree` `data` JSON array deserializes into
    /// `Vec<ProcessInfo>`; a record with `command_line:null` and
    /// `owner:null` yields `None` for both (graceful-degradation fields,
    /// D-6.3).
    #[test]
    fn process_tree_data_deserializes_and_degrades_optional_fields() {
        let data = serde_json::json!([
            {
                "pid": 4242,
                "parent_pid": 4,
                "name": "notepad.exe",
                "path": "C:\\Windows\\System32\\notepad.exe",
                "command_line": null,
                "owner": null
            }
        ]);

        let wires: Vec<ProcessInfoWire> =
            serde_json::from_value(data).expect("canned ProcessTree data deserializes");
        let processes: Vec<ProcessInfo> = wires.into_iter().map(ProcessInfoWire::into_owned).collect();

        assert_eq!(processes.len(), 1);
        let p = &processes[0];
        assert_eq!(p.pid, 4242);
        assert_eq!(p.parent_pid, 4);
        assert_eq!(p.name, "notepad.exe");
        assert_eq!(p.path, "C:\\Windows\\System32\\notepad.exe");
        assert_eq!(p.command_line, None);
        assert_eq!(p.owner, None);
    }

    /// A canned `Uia` `data` JSON array (per the wire contract, raw
    /// `runtime_id`/`control_type`/`parent_runtime_id`) deserializes into
    /// `Vec<UiaElementWire>`, maps via `into_owned` to `Vec<UiaElement>`,
    /// and every field equals the source — no loss across the round trip
    /// (SC#4).
    #[test]
    fn uia_element_wire_round_trips() {
        let data = serde_json::json!([
            {
                "runtime_id": [42, -3, 7],
                "control_type": 50000,
                "name": "OK",
                "bbox": {"x": 10, "y": 20, "w": 80, "h": 24},
                "enabled": true,
                "visible": true,
                "focusable": true,
                "focused": false,
                "depth": 1,
                "parent_runtime_id": [42, -3]
            }
        ]);

        let wires: Vec<UiaElementWire> =
            serde_json::from_value(data).expect("canned Uia data deserializes");
        let elements: Vec<UiaElement> = wires.into_iter().map(UiaElementWire::into_owned).collect();

        assert_eq!(elements.len(), 1);
        let e = &elements[0];
        assert_eq!(e.id, "42--3-7");
        assert_eq!(e.role, "Button");
        assert_eq!(e.name, "OK");
        assert_eq!(
            e.bbox,
            crate::Rect {
                x: 10,
                y: 20,
                w: 80,
                h: 24
            }
        );
        assert!(e.enabled);
        assert!(e.visible);
        assert!(e.focusable);
        assert!(!e.focused);
        assert_eq!(e.depth, 1);
        assert_eq!(e.parent_id, "42--3");
    }

    /// `runtime_id_to_string` joins a `RuntimeId` int array with `-` into a
    /// stable, deterministic string; the same input always yields the same
    /// output (D-7.2).
    #[test]
    fn runtime_id_to_string_is_deterministic() {
        let ids = [42, -3, 7];
        assert_eq!(runtime_id_to_string(&ids), "42--3-7");
        assert_eq!(runtime_id_to_string(&ids), runtime_id_to_string(&ids));
        assert_eq!(runtime_id_to_string(&[]), "");
    }

    /// `control_type_to_role` maps known `ControlType` ids to their stable
    /// English friendly string, and any out-of-table id to `"Unknown"`
    /// (D-7.3).
    #[test]
    fn control_type_maps_known_and_unknown_ids() {
        assert_eq!(control_type_to_role(50000), "Button");
        assert_eq!(control_type_to_role(50032), "Window");
        assert_eq!(control_type_to_role(50004), "Edit");
        assert_eq!(control_type_to_role(99999), "Unknown");
    }

    /// `WindowState::Minimized` serializes as the lowercase string
    /// `"minimized"`, matching the existing `WindowStateWire`
    /// `#[serde(rename_all = "lowercase")]` deserialization convention.
    #[test]
    fn window_state_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(WindowState::Minimized).expect("WindowState serializes"),
            serde_json::json!("minimized")
        );
        assert_eq!(
            serde_json::to_value(WindowState::Normal).expect("WindowState serializes"),
            serde_json::json!("normal")
        );
        assert_eq!(
            serde_json::to_value(WindowState::Maximized).expect("WindowState serializes"),
            serde_json::json!("maximized")
        );
    }

    /// A fully-populated `WindowInfo` serializes without error and exposes
    /// its documented fields (D-8.4).
    #[test]
    fn window_info_serializes_with_expected_fields() {
        let window = WindowInfo {
            hwnd: 65536,
            title: "Untitled - Notepad".to_string(),
            rect: crate::Rect {
                x: 10,
                y: 20,
                w: 800,
                h: 600,
            },
            z_order: 0,
            state: WindowState::Minimized,
            class_name: "Notepad".to_string(),
            pid: 4242,
        };
        let value = serde_json::to_value(&window).expect("WindowInfo serializes");
        let obj = value.as_object().expect("WindowInfo serializes as a JSON object");
        assert_eq!(obj.get("hwnd").and_then(|v| v.as_u64()), Some(65536));
        assert_eq!(obj.get("state").and_then(|v| v.as_str()), Some("minimized"));
        assert_eq!(obj.get("pid").and_then(|v| v.as_u64()), Some(4242));
    }

    /// A fully-populated `ProcessInfo` serializes without error and exposes
    /// its documented fields (D-8.4).
    #[test]
    fn process_info_serializes_with_expected_fields() {
        let process = ProcessInfo {
            pid: 4242,
            parent_pid: 4,
            name: "notepad.exe".to_string(),
            path: "C:\\Windows\\System32\\notepad.exe".to_string(),
            command_line: None,
            owner: Some("SYSTEM".to_string()),
        };
        let value = serde_json::to_value(&process).expect("ProcessInfo serializes");
        let obj = value.as_object().expect("ProcessInfo serializes as a JSON object");
        assert_eq!(obj.get("pid").and_then(|v| v.as_u64()), Some(4242));
        assert_eq!(obj.get("name").and_then(|v| v.as_str()), Some("notepad.exe"));
        assert_eq!(obj.get("owner").and_then(|v| v.as_str()), Some("SYSTEM"));
    }

    /// A fully-populated `UiaElement` serializes without error and exposes
    /// its documented fields, including `id`/`role`/`bbox` (D-8.4).
    #[test]
    fn uia_element_serializes_with_expected_fields() {
        let element = UiaElement {
            id: "42--3-7".to_string(),
            role: "Button".to_string(),
            name: "OK".to_string(),
            bbox: crate::Rect {
                x: 10,
                y: 20,
                w: 80,
                h: 24,
            },
            enabled: true,
            visible: true,
            focusable: true,
            focused: false,
            depth: 1,
            parent_id: "42--3".to_string(),
        };
        let value = serde_json::to_value(&element).expect("UiaElement serializes");
        let obj = value.as_object().expect("UiaElement serializes as a JSON object");
        assert_eq!(obj.get("id").and_then(|v| v.as_str()), Some("42--3-7"));
        assert_eq!(obj.get("role").and_then(|v| v.as_str()), Some("Button"));
        let bbox = obj.get("bbox").and_then(|v| v.as_object()).expect("bbox is an object");
        assert_eq!(bbox.get("w").and_then(|v| v.as_u64()), Some(80));
    }

    /// A canned `Uia` `data` JSON record at `depth: 2` (one level deeper
    /// than the Phase 7 `TreeScope_Children`-only walk ever produced) with a
    /// non-empty `parent_runtime_id` deserializes and maps via `into_owned`
    /// to a `UiaElement` with `depth == 2` and a non-empty `parent_id` --
    /// proving the existing wire shape already supports Plan 09-02's D-9.1
    /// deeper walk without any wire-struct change (only `session.rs`'s
    /// request side gains `max_depth`).
    #[test]
    fn uia_element_wire_round_trips_at_depth_two_with_nonempty_parent() {
        let data = serde_json::json!([
            {
                "runtime_id": [42, -3, 7, 9],
                "control_type": 50011,
                "name": "File",
                "bbox": {"x": 5, "y": 6, "w": 40, "h": 18},
                "enabled": true,
                "visible": true,
                "focusable": true,
                "focused": false,
                "depth": 2,
                "parent_runtime_id": [42, -3, 7]
            }
        ]);

        let wires: Vec<UiaElementWire> =
            serde_json::from_value(data).expect("canned depth=2 Uia data deserializes");
        let elements: Vec<UiaElement> = wires.into_iter().map(UiaElementWire::into_owned).collect();

        assert_eq!(elements.len(), 1);
        let e = &elements[0];
        assert_eq!(e.depth, 2);
        assert_eq!(e.parent_id, "42--3-7");
        assert!(!e.parent_id.is_empty(), "a depth-2 record must carry a non-empty parent_id");
        assert_eq!(e.role, "MenuItem");
    }

    /// `UiaScope::Children` and `UiaScope::Subtree { max_depth }` both
    /// serialize without error (D-09 owned-type discipline sanity check;
    /// the actual wire `max_depth` mapping lives in `session.rs`).
    #[test]
    fn uia_scope_variants_serialize() {
        let children = serde_json::to_value(UiaScope::Children).expect("UiaScope::Children serializes");
        assert_eq!(children, serde_json::json!("Children"));

        let subtree = serde_json::to_value(UiaScope::Subtree { max_depth: 3 }).expect("UiaScope::Subtree serializes");
        assert_eq!(subtree, serde_json::json!({"Subtree": {"max_depth": 3}}));
    }
}
