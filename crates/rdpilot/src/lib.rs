//! `rdpilot` — a reusable SDK that opens and manages an RDP session to a remote
//! Windows machine and exposes that session as a controllable, *perceivable*
//! surface (screenshots, input injection, and structured perception).
//!
//! This crate is the transport-layer foundation. Intelligence (AI computer use)
//! stays on the local workstation and operates the remote desktop *through*
//! `rdpilot` — the agent never runs on the remote machine.
//!
//! # Public API boundary
//!
//! The public surface exposes only owned SDK types. Third-party types
//! (`ironrdp*`, `image`, `rustls`) are internal implementation details and never
//! appear in `pub` signatures, keeping the API stable across upstream churn and
//! ready for future PyO3/MCP marshalling.
//!
//! Library code never panics: every fallible operation returns [`Error`] via the
//! crate [`Result`] alias. There are no `unwrap`/`expect`/`panic!` calls in
//! non-test code.

mod config;
mod error;
mod screenshot;

pub use config::ConnectionConfig;
pub use error::{Error, Result};
pub use screenshot::{Rect, Screenshot};

// The remaining public surface (`Session`, framebuffer wiring, the connect path)
// is introduced by later plans in this phase. This plan establishes the
// workspace, the dependency baseline, and the no-live-target types.
