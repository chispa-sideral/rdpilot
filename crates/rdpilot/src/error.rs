//! The crate-wide error type and `Result` alias.
//!
//! Library code never panics (API-01): every fallible operation returns
//! [`Error`]. Third-party error types (`image`, `ironrdp*`, `rustls`) are
//! **never** exposed here — they are source-erased into owned variants so the
//! public surface stays stable across upstream churn (D-09).

/// All errors the `rdpilot` SDK can return.
///
/// Variants are intentionally coarse and own their data (no borrowed or
/// third-party types) so the error surface is `'static`, `Send + Sync`, and
/// safe to marshal across an FFI / MCP boundary in the future.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The RDP connection or NLA/CredSSP authentication failed.
    #[error("connection or authentication failed: {0}")]
    Connect(String),

    /// The TLS handshake failed or the server certificate was rejected.
    #[error("TLS or certificate error: {0}")]
    Tls(String),

    /// A graphics/codec PDU could not be decoded into a framebuffer.
    #[error("framebuffer decode error: {0}")]
    Decode(String),

    /// A PNG (or other image) encode operation failed.
    #[error("image encode error: {0}")]
    Encode(String),

    /// A caller-supplied [`Rect`](crate::Rect) fell outside the image bounds.
    ///
    /// Carries the offending rectangle and the actual image dimensions so the
    /// caller can correct the request without guessing.
    #[error(
        "crop rectangle ({rect_x},{rect_y} {rect_w}x{rect_h}) is out of bounds \
         for a {image_w}x{image_h} image"
    )]
    CropOutOfBounds {
        /// Requested rectangle origin x.
        rect_x: u32,
        /// Requested rectangle origin y.
        rect_y: u32,
        /// Requested rectangle width.
        rect_w: u32,
        /// Requested rectangle height.
        rect_h: u32,
        /// Actual image width.
        image_w: u32,
        /// Actual image height.
        image_h: u32,
    },

    /// A [`ConnectionConfig`](crate::ConnectionConfig) value failed validation.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// The active session or its transport failed (placeholder for the session
    /// loop introduced by a later plan in this phase).
    #[error("session/transport error: {0}")]
    Session(String),

    /// A caller-supplied mouse coordinate fell outside the negotiated
    /// desktop size.
    ///
    /// Mirrors [`Error::CropOutOfBounds`] exactly: rejected *before* any
    /// `ironrdp_input::Operation`/PDU is constructed (D-3.2, SC#4). This is
    /// the SDK-level correctness backstop for `MousePdu::encode`'s own
    /// silent `as u8` wheel-magnitude wraparound (Pitfall 1, T-03-01) — a
    /// bounds violation must always surface as a typed error, never a
    /// silent clamp or wrap.
    #[error(
        "coordinate ({x},{y}) is out of bounds for a {desktop_w}x{desktop_h} desktop"
    )]
    CoordinateOutOfBounds {
        /// The offending x coordinate.
        x: u32,
        /// The offending y coordinate.
        y: u32,
        /// The negotiated desktop width.
        desktop_w: u32,
        /// The negotiated desktop height.
        desktop_h: u32,
    },

    /// A DVC (Dynamic Virtual Channel) transport error: registration, channel
    /// lookup, encode/decode, or handshake failure on the `RDPILOT_SENSOR`
    /// channel (Phase 4, SENSOR-03).
    #[error("DVC transport error: {0}")]
    Dvc(String),

    /// The in-band sensor deploy/launch bootstrap failed (D-5.1/D-5.2,
    /// SENSOR-02): the injected Win+R launch sequence did not produce a
    /// responding sensor (no pong) within the poll-and-retry budget.
    #[error("sensor bootstrap failed: {0}")]
    Bootstrap(String),

    /// The sensor understood and processed a request but rejected it as a
    /// semantic failure (D-6.4) — e.g. `set_foreground_window` given a
    /// closed/invalid `hwnd`, or `launch_process` given an exe that could
    /// not start. Distinct from [`Error::Dvc`] (a transport/timeout/decode
    /// failure): a `SensorRejected` means the round trip succeeded and the
    /// reply's `success` field was `false`.
    #[error("sensor rejected the request: {0}")]
    SensorRejected(String),

    /// A file-transfer path (Rust-side `RdpilotDriveBackend` validator,
    /// D-10.2/FILE-03) resolved outside the configured share root, or could
    /// not be resolved under it at all. Distinct from [`Error::Dvc`]/
    /// [`Error::SensorRejected`] (transport/semantic failures) so a rejected
    /// path is never conflated with a transient transfer failure (D-10.4).
    #[error("path rejected: {0}")]
    PathTraversal(String),

    /// A downloaded/uploaded file's computed SHA-256 digest did not match
    /// the expected digest (D-10.5, FILE-02). Carries both hex digests so
    /// the caller can report the mismatch without recomputing anything.
    #[error("checksum mismatch: expected {expected}, actual {actual}")]
    ChecksumMismatch {
        /// The expected SHA-256 digest, hex-encoded.
        expected: String,
        /// The actual computed SHA-256 digest, hex-encoded.
        actual: String,
    },
}

/// Convenience alias for results returned by the `rdpilot` public API.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Construct a [`Error::CropOutOfBounds`] from a rectangle and the image
    /// dimensions it was checked against.
    ///
    /// Internal helper so the screenshot module does not have to spell out the
    /// six fields at every call site.
    pub(crate) fn crop_out_of_bounds(
        rect: &crate::Rect,
        image_w: u32,
        image_h: u32,
    ) -> Self {
        Error::CropOutOfBounds {
            rect_x: rect.x,
            rect_y: rect.y,
            rect_w: rect.w,
            rect_h: rect.h,
            image_w,
            image_h,
        }
    }

    /// Construct a [`Error::CoordinateOutOfBounds`] from an offending
    /// coordinate and the desktop dimensions it was checked against.
    ///
    /// Internal helper so `input.rs`'s bounds check (a later plan) does not
    /// have to spell out the four fields at every call site — mirrors
    /// [`Error::crop_out_of_bounds`] exactly.
    pub(crate) fn coordinate_out_of_bounds(x: u32, y: u32, desktop_w: u32, desktop_h: u32) -> Self {
        Error::CoordinateOutOfBounds {
            x,
            y,
            desktop_w,
            desktop_h,
        }
    }

    /// Construct a [`Error::Dvc`] from any error type displayable as a string.
    ///
    /// Third-party errors (`ironrdp-dvc`, `serde_json`) are source-erased into
    /// an owned `String` (D-09) — mirrors [`Error::coordinate_out_of_bounds`]'s
    /// role as the single call-site-friendly constructor for its variant.
    pub(crate) fn dvc(msg: impl Into<String>) -> Self {
        Error::Dvc(msg.into())
    }

    /// Construct a [`Error::Bootstrap`] from any message displayable as a
    /// string (D-5.2) — mirrors [`Error::dvc`]'s role as the single
    /// call-site-friendly constructor for its variant.
    pub(crate) fn bootstrap(msg: impl Into<String>) -> Self {
        Error::Bootstrap(msg.into())
    }

    /// Construct a [`Error::SensorRejected`] from any message displayable as
    /// a string (D-6.4) — mirrors [`Error::bootstrap`]'s role as the single
    /// call-site-friendly constructor for its variant.
    #[allow(dead_code)] // Consumed by Plan 02's request-issuing Session methods (interface-first).
    pub(crate) fn sensor_rejected(msg: impl Into<String>) -> Self {
        Error::SensorRejected(msg.into())
    }

    /// Construct a [`Error::PathTraversal`] from any message displayable as
    /// a string (D-10.2/D-10.4) — mirrors [`Error::sensor_rejected`]'s role
    /// as the single call-site-friendly constructor for its variant.
    #[allow(dead_code)] // Consumed by Plan 10-01 Task 2's resolve_under_root and Plan 10-02's write path.
    pub(crate) fn path_traversal(msg: impl Into<String>) -> Self {
        Error::PathTraversal(msg.into())
    }

    /// Construct a [`Error::ChecksumMismatch`] from the expected and actual
    /// hex-encoded SHA-256 digests (D-10.5) — mirrors
    /// [`Error::path_traversal`]'s role as the single call-site-friendly
    /// constructor for its variant.
    #[allow(dead_code)] // Consumed by Plan 10-04's checksum verification (interface-first).
    pub(crate) fn checksum_mismatch(expected: impl Into<String>, actual: impl Into<String>) -> Self {
        Error::ChecksumMismatch {
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

/// Ensure the error renders without leaking any internal/third-party detail
/// beyond the strings we deliberately place in each variant.
impl Error {
    /// A short, stable category string useful for logging/metrics without
    /// exposing the inner message.
    pub fn category(&self) -> &'static str {
        match self {
            Error::Connect(_) => "connect",
            Error::Tls(_) => "tls",
            Error::Decode(_) => "decode",
            Error::Encode(_) => "encode",
            Error::CropOutOfBounds { .. } => "crop_out_of_bounds",
            Error::Config(_) => "config",
            Error::Session(_) => "session",
            Error::CoordinateOutOfBounds { .. } => "coordinate_out_of_bounds",
            Error::Dvc(_) => "dvc",
            Error::Bootstrap(_) => "bootstrap",
            Error::SensorRejected(_) => "sensor_rejected",
            Error::PathTraversal(_) => "path_traversal",
            Error::ChecksumMismatch { .. } => "checksum_mismatch",
        }
    }
}

// `thiserror` already provides `Display`; this is a compile-time assertion that
// the error is `Send + Sync + 'static` (required for FFI/async boundaries).
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Error>();
};

#[cfg(test)]
mod tests {
    use super::*;

    /// `Error::Bootstrap` reports the `"bootstrap"` category and renders its
    /// message via `Display` (D-5.2) -- mirrors the existing `Error::Dvc`
    /// coverage pattern for a new call-site-friendly variant.
    #[test]
    fn bootstrap_category_and_message_render() {
        let err = Error::bootstrap("no pong after 3 launch attempts");
        assert_eq!(err.category(), "bootstrap");
        assert!(matches!(err, Error::Bootstrap(_)));
        let rendered = format!("{err}");
        assert!(rendered.contains("sensor bootstrap failed"));
        assert!(rendered.contains("no pong after 3 launch attempts"));
    }

    /// `Error::sensor_rejected` reports the `"sensor_rejected"` category
    /// (D-6.4), matches `Error::SensorRejected(_)`, and its `Display`
    /// contains the reason -- distinguishing a semantic sensor-side
    /// rejection from a transport/timeout `Error::Dvc`.
    #[test]
    fn sensor_rejected_category_and_message_render() {
        let err = Error::sensor_rejected("window closed");
        assert_eq!(err.category(), "sensor_rejected");
        assert!(matches!(err, Error::SensorRejected(_)));
        let rendered = format!("{err}");
        assert!(rendered.contains("sensor rejected the request"));
        assert!(rendered.contains("window closed"));
    }

    /// `Error::path_traversal` reports the `"path_traversal"` category
    /// (D-10.2/D-10.4), matches `Error::PathTraversal(_)`, and its `Display`
    /// contains the rejection reason -- mirrors the existing
    /// `sensor_rejected_category_and_message_render` coverage pattern for a
    /// new call-site-friendly variant.
    #[test]
    fn path_traversal_category_and_message_render() {
        let err = Error::path_traversal("escapes share root");
        assert_eq!(err.category(), "path_traversal");
        assert!(matches!(err, Error::PathTraversal(_)));
        let rendered = format!("{err}");
        assert!(rendered.contains("path rejected"));
        assert!(rendered.contains("escapes share root"));
    }

    /// `Error::checksum_mismatch` reports the `"checksum_mismatch"` category
    /// (D-10.5) and its `Display` names both the expected and actual hex
    /// digests -- mirrors `path_traversal_category_and_message_render`.
    #[test]
    fn checksum_mismatch_category_and_message_render() {
        let err = Error::checksum_mismatch("aaaa", "bbbb");
        assert_eq!(err.category(), "checksum_mismatch");
        assert!(matches!(err, Error::ChecksumMismatch { .. }));
        let rendered = format!("{err}");
        assert!(rendered.contains("checksum mismatch"));
        assert!(rendered.contains("aaaa"));
        assert!(rendered.contains("bbbb"));
    }
}
