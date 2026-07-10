//! `rdpilot-ipc` — the daemon<->client wire protocol for `rdpilot`.
//!
//! This crate defines pure `serde` DTOs for the not-yet-built daemon (Phase
//! 12) and thin CLI/MCP client binaries (Phase 13/14) to exchange over local
//! IPC. It deliberately has **zero dependency on `rdpilot` (or IronRDP)** —
//! preserving the thin-client premise (D-17): a CLI/MCP binary that links
//! only `rdpilot-ipc` never pulls in IronRDP/rustls/tokio-full just to build
//! a request or parse a response.
//!
//! Two structural guarantees live here, not as conventions but as compiler-
//! and test-enforced facts:
//!
//! - **SESSION-02:** every session-scoped `Request` verb embeds a plain,
//!   required `session: SessionId` field (never `Option`, never
//!   `#[serde(default)]`) — a JSON payload omitting `session` is a hard
//!   `serde_json` deserialize error, not a silent fallback. The
//!   `SessionScoped` trait's exhaustive match makes a future verb added
//!   without a `session` field a *compile* error, not just a test gap.
//! - **CONFIG-03 / D-31:** `WireResponse` and its constituent DTOs never
//!   define a password/credential-shaped field. This crate does not depend
//!   on `rdpilot-config`, so a `Credentials`-shaped type structurally cannot
//!   enter the wire graph.
//!
//! The `rdpilot::Error -> WireErrorCode` mapping is deliberately **not**
//! defined here (Decision 1, Phase 11 scope) — only the `WireError`/
//! `WireErrorCode` *types* are. The mapping function lives in the Phase 12
//! daemon crate, the only consumer that legitimately depends on both
//! `rdpilot` and `rdpilot-ipc`.

// Per-crate opt-in (matches `rdpilot`'s `lib.rs` convention) — inner
// attributes scope to this crate's compilation unit, including its inline
// `#[cfg(test)] mod tests` (no separate `tests/*.rs` integration crate here).
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

mod error;
mod request;
mod session_id;
mod transfer;

pub use error::{WireError, WireErrorCode};
pub use request::{Request, SessionScoped};
pub use session_id::SessionId;
pub use transfer::TransferOutcome;
