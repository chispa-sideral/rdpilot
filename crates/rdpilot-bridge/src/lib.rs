//! Thin guest carrier. Cua owns desktop behavior; this crate owns process lifetime.
#[cfg(windows)]
pub mod guest;
pub mod install;
pub mod runtime;
pub mod transfer;
#[cfg(windows)]
pub mod windows;
pub mod wts_framing;
