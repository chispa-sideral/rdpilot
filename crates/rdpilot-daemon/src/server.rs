//! Top-level daemon assembly — binds the IPC listener, constructs the
//! registry, and drives the lifecycle tasks (Plan 12-06).
//!
//! Stub for now (Plan 12-02): [`run`] is a minimal placeholder so
//! `main.rs`/the crate compile end-to-end. Real config load + registry +
//! listener + lifecycle-task assembly is filled in by Plan 12-06.

use crate::seams::DaemonError;

/// The daemon's public entry point, called by `main.rs`.
///
/// # Errors
///
/// Returns [`DaemonError`] if daemon startup fails (Plan 12-06 fills in the
/// real failure modes: config load, listener bind, etc.).
pub async fn run() -> Result<(), DaemonError> {
    Ok(())
}
