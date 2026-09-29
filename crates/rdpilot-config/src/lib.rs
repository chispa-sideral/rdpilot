//! `rdpilot-config` — layered configuration resolution for `rdpilot`.
//!
//! Two things live here:
//!
//! - [`hosts`]: ssh_config-style host configuration (connection settings,
//!   credentials, `-o` overrides, `rdp://` targets).
//! - [`resolve`]: the daemon-local settings (`share_root`, `bundle_path`,
//!   `[viewer]`) from `config.toml` and `RDPILOT_*` environment variables.
//!
//! Like `rdpilot-ipc`, this crate deliberately has **zero dependency on
//! `rdpilot` (or IronRDP)** — preserving the thin-client premise (D-17).
//!
//! Neither [`ResolvedConfig`] nor [`hosts::ResolvedHost`] implements
//! `Serialize`, and credentials are redacted in `Debug`, so a credential
//! cannot be serialized or logged by accident.

// Per-crate opt-in (matches `rdpilot`'s `lib.rs` convention) — inner
// attributes scope to this crate's compilation unit, including its inline
// `#[cfg(test)] mod tests` (no separate `tests/*.rs` integration crate here).
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

pub mod hosts;
mod paths;
mod resolve;
mod resolved;
mod viewer;

pub use paths::{config_dir, config_file_path, home_dir, hosts_file_path};
pub use resolve::{resolve, share_root_or_default};
pub use resolved::{ConfigError, ResolvedConfig};
pub use viewer::{resolve_viewer, ViewerBind, ViewerConfig};

/// A fully-commented `config.toml` template. All keys ship commented out,
/// so an unedited copy parses to an all-defaults [`ResolvedConfig`].
pub const CONFIG_TEMPLATE: &str = include_str!("../assets/config.toml.template");
