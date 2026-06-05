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
        }
    }
}

// `thiserror` already provides `Display`; this is a compile-time assertion that
// the error is `Send + Sync + 'static` (required for FFI/async boundaries).
const _: fn() = || {
    fn assert_send_sync_static<T: Send + Sync + 'static>() {}
    assert_send_sync_static::<Error>();
};
