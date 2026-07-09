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

use serde::Deserialize;

/// A single top-level window on the remote desktop (PERC-01).
///
/// Reuses [`crate::Rect`] (the existing public rectangle type from
/// `screenshot.rs`) so callers crop a [`crate::Screenshot`] with the exact
/// geometry a window reports, in the same coordinate space.
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq)]
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
}
