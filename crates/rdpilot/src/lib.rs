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

// API-01/SC#4: make "no unsafe/unwrap/expect in library code" a compile-time
// guarantee, not a convention. These are deliberately `lib.rs` INNER
// attributes (crate-scoped), NOT a `Cargo.toml [lints]` table — a `[lints]`
// table is package-scoped and would also apply to `tests/live_session.rs`,
// breaking its 116 legitimate `.expect()` calls. Inner attributes scope to
// this library crate's compilation unit only, leaving separate `tests/*.rs`
// integration-test crate roots untouched. Do not "helpfully" migrate these to
// `Cargo.toml` — that will break the live-test suite.
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

mod config;
mod bootstrap;
mod connect;
mod error;
mod framebuffer;
mod input;
mod keepalive;
// `pub` (unlike every other internal module here): the daemon crate calls
// `rdpilot::perception::elevation_prompt_active` directly rather than
// through a crate-root re-export, since it is a free function alongside
// the module's owned types, not a type itself. The module's `*Wire`
// structs stay `pub(crate)` (D-09) and are therefore still invisible
// outside this crate despite the module itself
// being reachable.
pub mod perception;
mod rdpdr_backend;
mod rdpsnd_stub;
mod screenshot;
mod sensor;
mod session;
mod session_loop;
mod uac;
mod worldstate;

pub use config::ConnectionConfig;
pub use bootstrap::BootstrapStage;
pub use error::{Error, Result};
pub use input::{Button, Key, KeyAction, MouseAction};
pub use perception::{ProcessInfo, UiaElement, UiaScope, WindowInfo, WindowState};
pub use screenshot::{Rect, Screenshot};
pub use session::{Session, TransferOutcome};
pub use uac::{UacDecision, UacResponseOutcome};
pub use worldstate::{UiaMode, WorldState, WorldStateOptions};

// `connect`, `framebuffer`, `keepalive`, and `session_loop` are internal — they
// are implementation details driven by `Session`, never part of the public
// surface. The public API is exactly: `Session`, `ConnectionConfig`,
// `Screenshot`, `Rect`, `Error`, `Result`, `MouseAction`, `KeyAction`,
// `Button`, `Key`, `WindowInfo`, `WindowState`, `ProcessInfo`, `UiaElement`,
// `UiaScope`, `WorldStateOptions`, `UiaMode`, `WorldState`, `TransferOutcome`,
// `UacDecision`, `UacResponseOutcome` (owned SDK types only, D-09), plus
// `perception::elevation_prompt_active` (a free function reached via the
// `pub mod perception` path rather than a crate-root re-export). The
// module's `*Wire` structs stay crate-internal and are never reachable
// outside this crate (D-09). `worldstate`/`uac` stay private `mod`s — only
// their named types are `pub use`-re-exported, matching `screenshot`.
