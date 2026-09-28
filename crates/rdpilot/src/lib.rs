//! RDP connection management, offline Cua deployment and transparent native MCP transport.
//! Cua owns desktop behavior; native framebuffer/input provide independent recovery.
//! Attachments are bound to one RDP connection incarnation and never replay requests.
#![deny(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod bootstrap;
mod bridge;
mod config;
mod connect;
mod error;
mod framebuffer;
mod input;
mod keepalive;
mod rdpdr_backend;
mod rdpsnd_stub;
mod screenshot;
mod session;
mod session_loop;
pub use bootstrap::BootstrapStage;
pub use bridge::CuaAttachment;
pub use config::ConnectionConfig;
pub use error::{Error, Result};
pub use framebuffer::{FrameStatus, FrameWatch};
pub use input::{Button, Key, KeyAction, MouseAction};
pub use screenshot::{Rect, Screenshot};
pub use session::{Session, TransferOutcome};
