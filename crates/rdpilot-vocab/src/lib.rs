//! `rdpilot-vocab` — the vocabulary shared by every `rdpilot` crate.
//!
//! One type owns each concept, and the other crates use it directly:
//!
//! - [`input`]: mouse buttons, logical keys with the key-name parser and the
//!   Set-1 scancode table, mouse and key actions, and the raw pointer and key
//!   events of a human viewer.
//! - [`recording`]: why a recording starts ([`RecordingTrigger`]) and the
//!   serde codec that restricts a `Connect` request to the three triggers a
//!   client can name.
//! - [`error`]: the fixed set of error codes on the wire ([`WireErrorCode`]).
//!
//! The crate has `serde` as its only normal dependency. A thin client (the
//! CLI or the MCP server) therefore links these types without `rdpilot`,
//! `rdpilot-daemon` or IronRDP. Each spelling a surface uses (IPC, viewer
//! JSON, manifest, CLI) is declared here next to its type, so adding a key
//! changes this crate only.

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![warn(missing_docs)]

pub mod error;
pub mod input;
pub mod recording;

pub use error::WireErrorCode;
pub use input::{
    parse_key_name, Button, Key, KeyAction, MouseAction, PointerButton, RawInput, Set1Scancode,
};
pub use recording::{connect_record, RecordingTrigger};
