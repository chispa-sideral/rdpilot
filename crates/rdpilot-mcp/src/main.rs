//! `rdpilot-mcp` — the MCP server binary entry point (MCP-01).
//!
//! Bootstraps an rmcp stdio server: stdin/stdout carry the JSON-RPC MCP
//! wire, so ALL logging goes to stderr (Pitfall 1, T-14-04) — this is the
//! first statement `main` executes, before anything else can possibly log.
//! Thin client only: this binary links `rdpilot-ipc`/`rdpilot-config`/
//! `rmcp`/`schemars` and nothing daemon/SDK-side (D-17; enforced by the
//! `cargo tree` gate in `14-02-PLAN.md`'s `<verification>`).

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
// stdout is the JSON-RPC wire (Pitfall 1) — a stray `println!` corrupts
// every subsequent frame the MCP host tries to parse. `tracing` (stderr-only,
// initialized first below) is this crate's only sanctioned output channel.
#![deny(clippy::print_stdout)]

mod computer;
mod connect;
mod error;
mod handler;
mod timeouts;

use rmcp::{ServiceExt, transport::stdio};

use handler::RdpilotMcpHandler;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // MUST be the first statement: stdout is the JSON-RPC wire, so every
    // log line — including anything logged before this point, which is why
    // nothing may run before it — is routed to stderr only.
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let service = RdpilotMcpHandler::new().serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
