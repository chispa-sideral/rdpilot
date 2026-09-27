//! Thin guest carrier. Cua owns desktop behavior; this crate owns process lifetime.
pub mod runtime;
pub mod transfer;
#[cfg(windows)]
pub mod windows;
pub mod wts_framing;
