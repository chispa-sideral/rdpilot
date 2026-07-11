//! `dispatch_computer` — the `computer_20250124` action -> `Request`
//! mapping table (research §Computer-Use Action -> IPC Verb Mapping), the
//! three EXPLICIT gap rejections, and the `wait`/`hold_key` duration caps.

use std::str::FromStr;
use std::time::Duration;

use rdpilot_ipc::{Request, SessionId, WireButton, WireKeyAction, WireMouseAction, WireResponse, parse_wire_key};
use rmcp::model::{CallToolResult, ContentBlock};

use super::{ComputerAction, ComputerArgs, ScrollDirection, scale_to_native};
use crate::connect::round_trip_bounded;
use crate::error::McpError;
use crate::handler::RdpilotMcpHandler;
use crate::timeouts;

/// `wait`/`hold_key`'s `duration` cap, in seconds (Pitfall 5, matching
/// Anthropic's own reference implementation's `duration <= 100` check).
const MAX_DURATION_SECS: f64 = 100.0;

/// Split an xdotool-style `"+"`-joined key combo (e.g. `"ctrl+s"`) into
/// `WireKey`s via the one canonical `rdpilot_ipc::parse_wire_key` table,
/// mapping an unknown token to a legible [`McpError::InvalidArgument`]
/// (never a panic/silent drop, T-13-17).
fn parse_key_combo(text: &str) -> Result<Vec<rdpilot_ipc::WireKey>, McpError> {
    text.split('+').map(str::trim).map(|tok| parse_wire_key(tok).map_err(McpError::invalid_argument)).collect()
}

/// Build a signed `WHEEL_DELTA`-notch `dy` for `scroll_amount` notches in
/// `direction` (`up` = positive/away from the user, `down` = negative).
fn scroll_dy(scroll_amount: u32, direction_is_up: bool) -> i16 {
    let capped_notches = i64::from(scroll_amount).min(i64::from(u16::MAX) / 120);
    let magnitude = capped_notches * 120;
    let signed = if direction_is_up { magnitude } else { -magnitude };
    signed.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
}

/// A plain success acknowledgement rendered as a short text content block
/// — every mouse/key action that has no richer payload to return (mirrors
/// `WireResponse::Ack`'s own "generic success" shape).
fn ack_result() -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text("ok")])
}

impl RdpilotMcpHandler {
    /// Dispatch one `computer_20250124` action: resolve the session,
    /// source native desktop dimensions when the action carries a
    /// coordinate, scale through [`scale_to_native`], and issue the
    /// corresponding bounded daemon round trip(s) — or return an EXPLICIT
    /// tool error for the three genuine SDK gaps
    /// (`left_mouse_down`/`left_mouse_up`, `cursor_position`, horizontal
    /// `scroll_direction`) rather than a silent no-op (T-14-10).
    pub async fn dispatch_computer(&self, args: ComputerArgs) -> Result<CallToolResult, McpError> {
        let session = SessionId::from_str(&args.session).map_err(McpError::invalid_argument)?;

        match args.action {
            ComputerAction::Screenshot => self.computer_screenshot(session).await,
            ComputerAction::CursorPosition => Err(McpError::invalid_argument(
                "cursor_position is unsupported: no SDK pointer-position getter exists; take a screenshot to observe the pointer instead",
            )),
            ComputerAction::MouseMove { coordinate } => {
                let (x, y) = self.scale_for_session(&session, coordinate).await?;
                self.computer_mouse(session, WireMouseAction::Move { x, y }).await
            }
            ComputerAction::LeftClick { coordinate, text: _ } => {
                let (x, y) = self.scale_for_session(&session, coordinate).await?;
                self.computer_mouse(session, WireMouseAction::Click { x, y, button: WireButton::Left }).await
            }
            ComputerAction::RightClick { coordinate, text: _ } => {
                let (x, y) = self.scale_for_session(&session, coordinate).await?;
                self.computer_mouse(session, WireMouseAction::Click { x, y, button: WireButton::Right }).await
            }
            ComputerAction::MiddleClick { coordinate, text: _ } => {
                let (x, y) = self.scale_for_session(&session, coordinate).await?;
                self.computer_mouse(session, WireMouseAction::Click { x, y, button: WireButton::Middle }).await
            }
            ComputerAction::DoubleClick { coordinate, text: _ } => {
                let (x, y) = self.scale_for_session(&session, coordinate).await?;
                self.computer_mouse(session, WireMouseAction::DoubleClick { x, y, button: WireButton::Left }).await
            }
            ComputerAction::TripleClick { coordinate, text: _ } => {
                let (x, y) = self.scale_for_session(&session, coordinate).await?;
                // No atomic wire primitive: three sequential Click round
                // trips, ~100ms apart (research mapping table).
                for i in 0..3u8 {
                    self.computer_mouse(session.clone(), WireMouseAction::Click { x, y, button: WireButton::Left })
                        .await?;
                    if i < 2 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
                Ok(ack_result())
            }
            ComputerAction::LeftClickDrag { start_coordinate, coordinate } => {
                let (native_w, native_h) = self.native_dims(&session).await?;
                let (from_x, from_y) =
                    scale_to_native(start_coordinate[0], start_coordinate[1], native_w, native_h);
                let (to_x, to_y) = scale_to_native(coordinate[0], coordinate[1], native_w, native_h);
                self.computer_mouse(
                    session,
                    WireMouseAction::Drag { from_x, from_y, to_x, to_y, button: WireButton::Left },
                )
                .await
            }
            ComputerAction::LeftMouseDown { .. } => Err(McpError::invalid_argument(
                "left_mouse_down is unsupported: WireMouseAction has no press-only primitive; use left_click_drag instead",
            )),
            ComputerAction::LeftMouseUp { .. } => Err(McpError::invalid_argument(
                "left_mouse_up is unsupported: WireMouseAction has no release-only primitive; use left_click_drag instead",
            )),
            ComputerAction::Key { text } => {
                let combo = parse_key_combo(&text)?;
                self.computer_key(session, WireKeyAction::Combo(combo)).await
            }
            ComputerAction::Type { text } => self.computer_key(session, WireKeyAction::Type(text)).await,
            ComputerAction::Scroll { coordinate, scroll_direction, scroll_amount, text: _ } => {
                match scroll_direction {
                    ScrollDirection::Left | ScrollDirection::Right => Err(McpError::invalid_argument(
                        "horizontal scroll (scroll_direction: left/right) is unsupported: rdpilot's Scroll wire verb is vertical-only (D-3.3)",
                    )),
                    ScrollDirection::Up | ScrollDirection::Down => {
                        let coordinate = coordinate.ok_or_else(|| {
                            McpError::invalid_argument(
                                "scroll requires a coordinate: no cursor-position tracking exists to default to",
                            )
                        })?;
                        let (x, y) = self.scale_for_session(&session, coordinate).await?;
                        let dy = scroll_dy(scroll_amount, matches!(scroll_direction, ScrollDirection::Up));
                        self.computer_mouse(session, WireMouseAction::Scroll { x, y, dy }).await
                    }
                }
            }
            ComputerAction::HoldKey { text, duration } => {
                check_duration_cap(duration, "hold_key")?;
                let combo = parse_key_combo(&text)?;
                // Approximated as an atomic press+release combo (research
                // A3) — no genuine SDK "hold for N seconds while other
                // actions may also occur" primitive exists. Documented
                // limitation, not a rejection.
                self.computer_key(session, WireKeyAction::Combo(combo)).await
            }
            ComputerAction::Wait { duration } => {
                check_duration_cap(duration, "wait")?;
                // No daemon round trip: a bounded local sleep only.
                tokio::time::sleep(Duration::from_secs_f64(duration)).await;
                Ok(ack_result())
            }
        }
    }

    /// Fetch `session`'s native desktop dimensions via `Request::DesktopSize`
    /// (never PNG-sniffed) and scale `coordinate` through
    /// [`scale_to_native`].
    async fn scale_for_session(&self, session: &SessionId, coordinate: [u32; 2]) -> Result<(u16, u16), McpError> {
        let (native_w, native_h) = self.native_dims(session).await?;
        Ok(scale_to_native(coordinate[0], coordinate[1], native_w, native_h))
    }

    /// Round-trip `Request::DesktopSize` for `session`, returning
    /// `(width, height)`.
    async fn native_dims(&self, session: &SessionId) -> Result<(u32, u32), McpError> {
        let resp =
            round_trip_bounded(Request::DesktopSize { session: session.clone() }, timeouts::FAST).await?;
        match resp {
            WireResponse::DesktopSize { width, height } => Ok((u32::from(width), u32::from(height))),
            WireResponse::Error(err) => Err(McpError::from(err)),
            other => Err(McpError::invalid_argument(format!("expected DesktopSize, got {other:?}"))),
        }
    }

    /// Issue a `Request::Mouse` round trip and map the response to a
    /// [`CallToolResult`].
    async fn computer_mouse(&self, session: SessionId, action: WireMouseAction) -> Result<CallToolResult, McpError> {
        let resp = round_trip_bounded(Request::Mouse { session, action }, timeouts::FAST).await?;
        match resp {
            WireResponse::Ack => Ok(ack_result()),
            WireResponse::Error(err) => Err(McpError::from(err)),
            other => Err(McpError::invalid_argument(format!("expected Ack, got {other:?}"))),
        }
    }

    /// Issue a `Request::Key` round trip and map the response to a
    /// [`CallToolResult`].
    async fn computer_key(&self, session: SessionId, action: WireKeyAction) -> Result<CallToolResult, McpError> {
        let resp = round_trip_bounded(Request::Key { session, action }, timeouts::FAST).await?;
        match resp {
            WireResponse::Ack => Ok(ack_result()),
            WireResponse::Error(err) => Err(McpError::from(err)),
            other => Err(McpError::invalid_argument(format!("expected Ack, got {other:?}"))),
        }
    }

    /// Issue `Request::Screenshot` and pass the base64 PNG straight
    /// through as an rmcp image content block (no decode/re-encode).
    async fn computer_screenshot(&self, session: SessionId) -> Result<CallToolResult, McpError> {
        let resp = round_trip_bounded(Request::Screenshot { session }, timeouts::FAST).await?;
        match resp {
            WireResponse::Screenshot { png_base64 } => {
                Ok(CallToolResult::success(vec![ContentBlock::image(png_base64, "image/png")]))
            }
            WireResponse::Error(err) => Err(McpError::from(err)),
            other => Err(McpError::invalid_argument(format!("expected Screenshot, got {other:?}"))),
        }
    }
}

/// Reject a `wait`/`hold_key` `duration` exceeding [`MAX_DURATION_SECS`]
/// (Pitfall 5) with a clear tool error naming both the action and the cap.
fn check_duration_cap(duration: f64, action: &str) -> Result<(), McpError> {
    if !duration.is_finite() || !(0.0..=MAX_DURATION_SECS).contains(&duration) {
        return Err(McpError::invalid_argument(format!(
            "{action} duration {duration} exceeds the {MAX_DURATION_SECS}s cap"
        )));
    }
    Ok(())
}

#[cfg(test)]
// Test-only fail-fast assertions -- mirrors this crate's `rdpilot-daemon`
// sibling's established convention (`server.rs`/`registry.rs`/
// `lifecycle.rs`): `.expect_err(...)` on a `Result` a test has already
// asserted is an `Err` is the clearest way to extract-and-message-check
// that error, and this crate-wide `#![deny(clippy::expect_used)]` is a
// production-code guard, not a test-ergonomics one.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn scroll_dy_up_is_positive_120_notch_multiples() {
        assert_eq!(scroll_dy(3, true), 360);
    }

    #[test]
    fn scroll_dy_down_is_negative_120_notch_multiples() {
        assert_eq!(scroll_dy(3, false), -360);
    }

    #[test]
    fn duration_cap_rejects_values_over_100_seconds() {
        assert!(check_duration_cap(100.0, "wait").is_ok());
        let err = check_duration_cap(100.5, "wait").expect_err("duration over the cap must be rejected");
        assert!(err.to_string().contains("100"), "error should name the cap: {err}");
    }

    #[test]
    fn duration_cap_rejects_negative_and_non_finite_values() {
        assert!(check_duration_cap(-1.0, "hold_key").is_err());
        assert!(check_duration_cap(f64::NAN, "hold_key").is_err());
        assert!(check_duration_cap(f64::INFINITY, "hold_key").is_err());
    }

    #[test]
    fn parse_key_combo_splits_on_plus_and_rejects_unknown_tokens() {
        let combo = parse_key_combo("ctrl+s").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(combo, vec![rdpilot_ipc::WireKey::Ctrl, rdpilot_ipc::WireKey::S]);

        let err = parse_key_combo("ctrl+nope").expect_err("unknown key token must be rejected");
        assert!(err.to_string().contains("nope"), "error must name the offending token: {err}");
    }

    fn args(action: ComputerAction) -> ComputerArgs {
        ComputerArgs { session: "test-session".to_owned(), action }
    }

    /// The three genuine SDK gaps (T-14-10) are EXPLICIT tool-error
    /// rejections, asserted by name, and never touch the daemon (no live
    /// connection needed for this test).
    #[tokio::test]
    async fn left_mouse_down_is_rejected_by_name() {
        let handler = RdpilotMcpHandler::new();
        let err = handler
            .dispatch_computer(args(ComputerAction::LeftMouseDown { coordinate: [1, 1] }))
            .await
            .expect_err("left_mouse_down must be an explicit rejection, never a silent no-op");
        let msg = err.to_string();
        assert!(msg.contains("left_mouse_down"), "error must name the action: {msg}");
        assert!(msg.contains("left_click_drag"), "error must name the supported alternative: {msg}");
    }

    #[tokio::test]
    async fn left_mouse_up_is_rejected_by_name() {
        let handler = RdpilotMcpHandler::new();
        let err = handler
            .dispatch_computer(args(ComputerAction::LeftMouseUp { coordinate: [1, 1] }))
            .await
            .expect_err("left_mouse_up must be an explicit rejection, never a silent no-op");
        let msg = err.to_string();
        assert!(msg.contains("left_mouse_up"), "error must name the action: {msg}");
        assert!(msg.contains("left_click_drag"), "error must name the supported alternative: {msg}");
    }

    #[tokio::test]
    async fn cursor_position_is_rejected_by_name() {
        let handler = RdpilotMcpHandler::new();
        let err = handler
            .dispatch_computer(args(ComputerAction::CursorPosition))
            .await
            .expect_err("cursor_position must be an explicit rejection, never a silent no-op");
        assert!(err.to_string().contains("cursor_position"), "error must name the action: {err}");
    }

    #[tokio::test]
    async fn horizontal_scroll_directions_are_rejected_by_name() {
        let handler = RdpilotMcpHandler::new();
        for direction in [ScrollDirection::Left, ScrollDirection::Right] {
            let err = handler
                .dispatch_computer(args(ComputerAction::Scroll {
                    coordinate: Some([1, 1]),
                    scroll_direction: direction,
                    scroll_amount: 1,
                    text: None,
                }))
                .await
                .expect_err("horizontal scroll must be an explicit rejection, never a silent no-op");
            let msg = err.to_string();
            assert!(msg.to_lowercase().contains("scroll"), "error must name scroll: {msg}");
            assert!(msg.contains("vertical") || msg.contains("D-3.3"), "error must explain the vertical-only limit: {msg}");
        }
    }

    #[tokio::test]
    async fn wait_over_the_duration_cap_is_rejected_without_a_daemon_round_trip() {
        let handler = RdpilotMcpHandler::new();
        let err = handler
            .dispatch_computer(args(ComputerAction::Wait { duration: 101.0 }))
            .await
            .expect_err("duration over the cap must be rejected");
        assert!(err.to_string().contains("wait"), "error should name the action: {err}");
    }

    #[tokio::test]
    async fn hold_key_over_the_duration_cap_is_rejected_without_a_daemon_round_trip() {
        let handler = RdpilotMcpHandler::new();
        let err = handler
            .dispatch_computer(args(ComputerAction::HoldKey { text: "ctrl".to_owned(), duration: 200.0 }))
            .await
            .expect_err("duration over the cap must be rejected");
        assert!(err.to_string().contains("hold_key"), "error should name the action: {err}");
    }

    #[tokio::test]
    async fn key_with_an_unknown_token_is_rejected_without_a_daemon_round_trip() {
        let handler = RdpilotMcpHandler::new();
        let err = handler
            .dispatch_computer(args(ComputerAction::Key { text: "ctrl+nope".to_owned() }))
            .await
            .expect_err("unknown key token must be rejected");
        assert!(err.to_string().contains("nope"), "error must name the offending token: {err}");
    }
}
