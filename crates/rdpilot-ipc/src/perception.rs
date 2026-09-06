//! Owned wire mirrors of `rdpilot`'s perception types (`WindowInfo`,
//! `ProcessInfo`, `UiaElement`, and friends) plus the `world_state` options
//! DTO.
//!
//! Deliberately duplicates each SDK type's field shape rather than importing
//! it (the `transfer.rs` convention, Decision 1): `rdpilot-ipc` must never
//! depend on `rdpilot`, so every wire DTO that corresponds to an SDK type is
//! its own owned mirror, kept in sync by convention/tests, not by a shared
//! type.

use serde::{Deserialize, Serialize};

/// Wire mirror of `rdpilot::Rect` — a rectangle in physical
/// virtual-desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRect {
    /// Origin x (left edge), in pixels.
    pub x: u32,
    /// Origin y (top edge), in pixels.
    pub y: u32,
    /// Width, in pixels.
    pub w: u32,
    /// Height, in pixels.
    pub h: u32,
}

/// Wire mirror of `rdpilot::WindowState` — a window's visibility/restore
/// state. Serializes as a lowercase string (`"normal"`/`"minimized"`/
/// `"maximized"`), matching the SDK's own wire convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireWindowState {
    /// Neither minimized nor maximized.
    Normal,
    /// Minimized (iconic).
    Minimized,
    /// Maximized (zoomed).
    Maximized,
}

/// Wire mirror of `rdpilot::WindowInfo` — a single top-level window on the
/// remote desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireWindowInfo {
    /// The native Win32 window handle (`HWND`), as a raw integer.
    pub hwnd: u64,
    /// The window's title bar text (may be empty for untitled windows).
    pub title: String,
    /// The window's bounding rectangle, in physical virtual-desktop pixels.
    pub rect: WireRect,
    /// Z-order position: `0` is topmost.
    pub z_order: u32,
    /// Whether the window is normal, minimized, or maximized.
    pub state: WireWindowState,
    /// The window class name (e.g. `"Notepad"`, `"CabinetWClass"`).
    pub class_name: String,
    /// The process id that owns this window.
    pub pid: u32,
}

/// Wire mirror of `rdpilot::ProcessInfo` — a single process in the remote
/// machine's process tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireProcessInfo {
    /// The process id.
    pub pid: u32,
    /// The parent process id.
    pub parent_pid: u32,
    /// The process's image (executable) file name.
    pub name: String,
    /// The full path to the process's executable image.
    pub path: String,
    /// The process's command line, if the sensor could retrieve it
    /// (best-effort extra).
    pub command_line: Option<String>,
    /// The process's owning user account, if the sensor could retrieve it
    /// (best-effort extra).
    pub owner: Option<String>,
    /// The Terminal Services session id hosting this process, if the
    /// sensor could resolve it (best-effort extra, ticket BF8Q9K6FGZ2APN8F).
    pub session_id: Option<u32>,
}

/// Wire mirror of `rdpilot::UiaScope` — how deep a UIA tree walk should go.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireUiaScope {
    /// `TreeScope_Children` only (wire `max_depth: 1`).
    Children,
    /// A bounded, level-by-level deeper walk up to `max_depth` levels below
    /// the target window.
    Subtree {
        /// How many levels below the target window to walk.
        max_depth: u32,
    },
}

/// Wire mirror of `rdpilot::UiaElement` — a single UI Automation element in
/// a flat tree-walk result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireUiaElement {
    /// The element's stable-ish identity (UIA `RuntimeId`, joined to a
    /// single string).
    pub id: String,
    /// The element's UIA `ControlType`, mapped to a stable, locale-independent
    /// English friendly string (e.g. `"Button"`, `"Window"`).
    pub role: String,
    /// The element's UIA `Name` property (may be empty).
    pub name: String,
    /// The element's bounding rectangle, in physical virtual-desktop pixels.
    pub bbox: WireRect,
    /// Whether the element is enabled (`IsEnabled` UIA property).
    pub enabled: bool,
    /// Whether the element is visible.
    pub visible: bool,
    /// Whether the element is keyboard-focusable (`IsKeyboardFocusable`).
    pub focusable: bool,
    /// Whether the element currently has keyboard focus
    /// (`HasKeyboardFocus`).
    pub focused: bool,
    /// The element's depth in the flat tree-walk result.
    pub depth: u32,
    /// The parent element's [`WireUiaElement::id`], or an empty string if
    /// there is no parent in this flat result.
    pub parent_id: String,
}

/// Wire mirror of `rdpilot::UiaMode` — which UIA tree(s), if any, a
/// `world_state` request should fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireUiaMode {
    /// Fetch no UIA tree at all.
    None,
    /// Fetch the UIA tree of only the current foreground window.
    Foreground,
    /// Fetch the UIA tree for each of the given window handles, in the
    /// given order.
    Hwnd(Vec<u64>),
    /// Fetch the UIA tree for every top-level window currently listed.
    AllTopLevel,
}

/// Wire mirror of `rdpilot::WorldStateOptions` — a-la-carte selection of
/// which components a `world_state` request should capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireWorldStateOptions {
    /// Whether to capture a full-desktop screenshot.
    pub screenshot: bool,
    /// Whether to include the top-level window list in the response.
    pub window_list: bool,
    /// Which UIA tree(s), if any, to fetch.
    pub uia: WireUiaMode,
    /// Whether to fetch and surface `elevation_active` (session-scoped
    /// UAC/elevation consent-prompt detection, ticket BF8Q9K6FGZ2APN8F).
    /// `#[serde(default)]` so an older client that predates this field
    /// still deserializes cleanly, defaulting to `false` (no behavior
    /// change for existing callers).
    #[serde(default)]
    pub elevation_check: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rect() -> WireRect {
        WireRect { x: 1, y: 2, w: 3, h: 4 }
    }

    #[test]
    fn wire_rect_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let rect = sample_rect();
        let json = serde_json::to_string(&rect)?;
        let parsed: WireRect = serde_json::from_str(&json)?;
        assert_eq!(parsed, rect);
        Ok(())
    }

    #[test]
    fn wire_window_state_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        for state in [WireWindowState::Normal, WireWindowState::Minimized, WireWindowState::Maximized] {
            let json = serde_json::to_string(&state)?;
            let parsed: WireWindowState = serde_json::from_str(&json)?;
            assert_eq!(parsed, state);
        }
        Ok(())
    }

    #[test]
    fn wire_window_state_serializes_to_lowercase_strings() -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (WireWindowState::Normal, "\"normal\""),
            (WireWindowState::Minimized, "\"minimized\""),
            (WireWindowState::Maximized, "\"maximized\""),
        ];
        for (state, expected) in cases {
            let json = serde_json::to_string(&state)?;
            assert_eq!(json, expected);
        }
        Ok(())
    }

    #[test]
    fn wire_window_info_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let info = WireWindowInfo {
            hwnd: 65536,
            title: "Notepad".to_owned(),
            rect: sample_rect(),
            z_order: 0,
            state: WireWindowState::Normal,
            class_name: "Notepad".to_owned(),
            pid: 4242,
        };
        let json = serde_json::to_string(&info)?;
        let parsed: WireWindowInfo = serde_json::from_str(&json)?;
        assert_eq!(parsed.hwnd, info.hwnd);
        assert_eq!(parsed.title, info.title);
        assert_eq!(parsed.rect, info.rect);
        assert_eq!(parsed.z_order, info.z_order);
        assert_eq!(parsed.state, info.state);
        assert_eq!(parsed.class_name, info.class_name);
        assert_eq!(parsed.pid, info.pid);
        Ok(())
    }

    #[test]
    fn wire_process_info_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let info = WireProcessInfo {
            pid: 4242,
            parent_pid: 4,
            name: "notepad.exe".to_owned(),
            path: "C:\\Windows\\notepad.exe".to_owned(),
            command_line: Some("notepad.exe file.txt".to_owned()),
            owner: None,
            session_id: Some(1),
        };
        let json = serde_json::to_string(&info)?;
        let parsed: WireProcessInfo = serde_json::from_str(&json)?;
        assert_eq!(parsed.pid, info.pid);
        assert_eq!(parsed.parent_pid, info.parent_pid);
        assert_eq!(parsed.name, info.name);
        assert_eq!(parsed.path, info.path);
        assert_eq!(parsed.command_line, info.command_line);
        assert_eq!(parsed.owner, info.owner);
        assert_eq!(parsed.session_id, info.session_id);
        Ok(())
    }

    #[test]
    fn wire_uia_scope_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        for scope in [WireUiaScope::Children, WireUiaScope::Subtree { max_depth: 3 }] {
            let json = serde_json::to_string(&scope)?;
            let _: WireUiaScope = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    #[test]
    fn wire_uia_element_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let element = WireUiaElement {
            id: "1.2.3".to_owned(),
            role: "Button".to_owned(),
            name: "OK".to_owned(),
            bbox: sample_rect(),
            enabled: true,
            visible: true,
            focusable: true,
            focused: false,
            depth: 1,
            parent_id: "1.2".to_owned(),
        };
        let json = serde_json::to_string(&element)?;
        let parsed: WireUiaElement = serde_json::from_str(&json)?;
        assert_eq!(parsed.id, element.id);
        assert_eq!(parsed.role, element.role);
        assert_eq!(parsed.name, element.name);
        assert_eq!(parsed.bbox, element.bbox);
        assert_eq!(parsed.enabled, element.enabled);
        assert_eq!(parsed.visible, element.visible);
        assert_eq!(parsed.focusable, element.focusable);
        assert_eq!(parsed.focused, element.focused);
        assert_eq!(parsed.depth, element.depth);
        assert_eq!(parsed.parent_id, element.parent_id);
        Ok(())
    }

    #[test]
    fn wire_uia_mode_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        for mode in [
            WireUiaMode::None,
            WireUiaMode::Foreground,
            WireUiaMode::Hwnd(vec![1, 2, 3]),
            WireUiaMode::AllTopLevel,
        ] {
            let json = serde_json::to_string(&mode)?;
            let _: WireUiaMode = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    #[test]
    fn wire_world_state_options_round_trips_through_serde_json() -> Result<(), Box<dyn std::error::Error>> {
        let options = WireWorldStateOptions {
            screenshot: true,
            window_list: true,
            uia: WireUiaMode::Foreground,
            elevation_check: true,
        };
        let json = serde_json::to_string(&options)?;
        let parsed: WireWorldStateOptions = serde_json::from_str(&json)?;
        assert_eq!(parsed.screenshot, options.screenshot);
        assert_eq!(parsed.elevation_check, options.elevation_check);
        assert_eq!(parsed.window_list, options.window_list);
        Ok(())
    }

    /// A `WireWorldStateOptions` JSON payload with NO `elevation_check`
    /// field at all (an older client that predates this ticket) still
    /// deserializes, defaulting to `false` -- no behavior change for
    /// existing callers.
    #[test]
    fn wire_world_state_options_defaults_elevation_check_when_absent() -> Result<(), Box<dyn std::error::Error>> {
        let json = r#"{"screenshot":true,"window_list":true,"uia":"None"}"#;
        let parsed: WireWorldStateOptions = serde_json::from_str(json)?;
        assert!(!parsed.elevation_check);
        Ok(())
    }
}
