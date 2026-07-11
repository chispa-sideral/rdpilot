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
//! - `ipc` — the Unix/Windows LISTENER-side security primitives
//!   (bind/accept_and_authorize/authorize_uid on Unix; bind/
//!   accept_and_authorize/socket_path on Windows via an explicit
//!   owner-only pipe DACL, Plan 15-01, closing 12-07's Windows half of
//!   DAEMON-02). Socket-path resolution and length-prefixed framing were
//!   relocated into `rdpilot_ipc::transport` (Plan 13-01) so a thin
//!   CLI/MCP client can share them without depending on this crate.
//! - [`dispatch`] — `rdpilot-ipc::Request` -> registry ops -> `WireResponse`
//!   (Plan 12-04).
//! - [`reconcile`] — disk-persisted crash-restart reconciliation state
//!   (Plan 12-05).
//! - [`lifecycle`] — idle reaper + empty-registry grace-period self-shutdown
//!   (Plan 12-06).
//! - [`server`] — top-level assembly; [`run`] is this crate's public entry
//!   point (Plan 12-06). The client-side connect-or-spawn auto-start helper
//!   now lives in `rdpilot_ipc::transport::connect_or_spawn` (Plan 13-01).

// Per-crate opt-in (matches `rdpilot`/`rdpilot-ipc`'s `lib.rs` convention).
// Targeted `#[allow]`s are used at unavoidable FFI/mutex-poison `expect`
// sites (see `seams.rs`/`registry.rs`) rather than relaxing this crate-wide.
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

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

// Re-exported so `tests/live_daemon_windows_dacl.rs` (Plan 15-01, closing
// 12-07's Windows half of DAEMON-02) can reach the Windows IPC primitives
// across the crate boundary — mirrors the `#[cfg(unix)]` re-export above
// exactly. `authorize_uid` has no Windows analogue (the module doc on
// `ipc/windows.rs` explains why: the DACL itself is the access control,
// enforced by the OS at connect time, not by a post-accept application
// check) so it is deliberately absent from this list.
#[cfg(windows)]
pub use ipc::{accept_and_authorize, bind, socket_path};

// Re-exported so `tests/autostart_lifecycle.rs` (the SC#5 [BLOCKING]
// DAEMON-03 integration test, Plan 12-06) can drive the client-side
// auto-start helper against the real compiled binary across the crate
// boundary. The implementation itself was relocated into
// `rdpilot_ipc::transport::connect_or_spawn` (Plan 13-01) — this is a
// transparent re-export, unix-only for now (matching its own
// `#![cfg(unix)]`).
#[cfg(unix)]
pub use rdpilot_ipc::connect_or_spawn;
