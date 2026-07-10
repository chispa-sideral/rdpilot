//! `rdpilot-config` — layered configuration resolution for `rdpilot`.
//!
//! Resolves target host + credentials through a **file -> env -> flag/MCP-init**
//! precedence (CONFIG-01), with the highest-precedence layer deterministically
//! winning, from a platform-conventional config file (CONFIG-02, D-27).
//!
//! Like `rdpilot-ipc`, this crate deliberately has **zero dependency on
//! `rdpilot` (or IronRDP)** — preserving the thin-client premise (D-17): a
//! CLI/MCP client binary that links only `rdpilot-config` never pulls in
//! IronRDP/rustls/tokio-full just to read a config file. [`ResolvedConfig`]
//! is its own owned struct, never `rdpilot::ConnectionConfig`; the
//! `ResolvedConfig -> rdpilot::ConnectionConfig` conversion is a Phase 12
//! daemon-startup step, out of this crate's scope.
//!
//! [`ResolvedConfig`] deliberately does **not** implement `Serialize` — a
//! structural credential-leak prevention one layer earlier than the
//! `rdpilot-ipc` wire boundary (D-31): if this type can never be serialized
//! at all, it cannot be wire-serialized by accident even before
//! `rdpilot-ipc`'s credential-free-by-construction DTOs are considered.

// Per-crate opt-in (matches `rdpilot`'s `lib.rs` convention) — inner
// attributes scope to this crate's compilation unit, including its inline
// `#[cfg(test)] mod tests` (no separate `tests/*.rs` integration crate here).
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

mod resolved;

pub use resolved::{ConfigError, ResolvedConfig};
