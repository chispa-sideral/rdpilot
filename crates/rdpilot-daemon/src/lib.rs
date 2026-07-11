//! `rdpilot-daemon` — the long-lived local IPC daemon that owns `rdpilot`'s
//! in-memory session registry (DAEMON-01/02/03/04, SESSION-01/03/04).
//!
//! This is the ONLY crate in the workspace that legitimately depends on
//! both `rdpilot` (the RDP SDK) and `rdpilot-ipc` (the dependency-free wire
//! protocol) — see [`error_map`] for the `rdpilot::Error -> WireErrorCode`
//! mapping Phase 11 deliberately deferred here (D-28).
//!
//! ## Module map
//!
//! - [`seams`] — `SessionConnector`/`ManagedSession`/`ReconciliationSink`
//!   traits, `SessionEntry`, `DaemonError` (Plan 12-02).
//! - [`error_map`] — `rdpilot::Error -> WireErrorCode` and
//!   `DaemonError -> WireError` mapping (Plan 12-02).
//! - [`registry`] — the atomic claim-then-connect session registry
//!   (Plan 12-03).
//! - `ipc` — the Unix/Windows local-user-scoped transport + framing
//!   (Plan 12-04).
//! - [`dispatch`] — `rdpilot-ipc::Request` -> registry ops -> `WireResponse`
//!   (Plan 12-04).
//! - [`reconcile`] — disk-persisted crash-restart reconciliation state
//!   (Plan 12-05).
//! - [`lifecycle`] — idle reaper + empty-registry grace-period self-shutdown
//!   (Plan 12-06).
//! - [`autostart`] — client-side connect-or-spawn helper (Plan 12-06).
//! - [`server`] — top-level assembly; [`run`] is this crate's public entry
//!   point (Plan 12-06).

// Per-crate opt-in (matches `rdpilot`/`rdpilot-ipc`'s `lib.rs` convention).
// Targeted `#[allow]`s are used at unavoidable FFI/mutex-poison `expect`
// sites (see `seams.rs`/`registry.rs`) rather than relaxing this crate-wide.
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

mod autostart;
mod dispatch;
mod error_map;
mod ipc;
mod lifecycle;
mod reconcile;
mod registry;
mod seams;
mod server;

pub use reconcile::{JsonReconciliationSink, ReconciliationRecord, scan_orphans, seed_into};
pub use registry::Registry;
pub use seams::{DaemonError, ManagedSession, ReconciliationSink, SessionConnector, SessionEntry};
pub use server::{RunConfig, run};

// Re-exported so `tests/ipc_security.rs` (the SC#4 [BLOCKING] DAEMON-02
// integration test, Plan 12-04) can reach the Unix IPC primitives across
// the crate boundary — integration tests link this crate as an external
// dependency and can only see items reachable from the crate root.
#[cfg(unix)]
pub use ipc::{accept_and_authorize, authorize_uid, bind, socket_path};

// Re-exported so `tests/autostart_lifecycle.rs` (the SC#5 [BLOCKING]
// DAEMON-03 integration test, Plan 12-06) can drive the client-side
// auto-start helper against the real compiled binary across the crate
// boundary — Unix-only for now (`autostart.rs`'s own `#![cfg(unix)]`).
#[cfg(unix)]
pub use autostart::connect_or_spawn;
