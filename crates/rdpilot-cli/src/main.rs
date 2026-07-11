//! `rdpilot` — the CLI binary entry point (CLI-01/02/03).
//!
//! Parses [`cli::Cli`], dispatches to the verb handler, and translates the
//! top-level `Result<(), exit_codes::CliError>` into a distinct non-zero
//! `process::ExitCode` (D-28). Single-shot invoke-and-exit process — never
//! a persistent client — so a `current_thread` Tokio runtime is sufficient.
//!
//! Dispatch is a scaffolding stub in this task (Task 1) — real verb
//! handlers land in `verbs::session` (Task 2), which replaces the body of
//! [`dispatch`] below with calls into it.

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
// This task (Task 1) scaffolds the transport client/exit-code/render
// modules interface-first — `dispatch` below is a stub, so several public
// items here have no caller yet. Task 2 wires `verbs::session` in and this
// allow is removed; mirrors the same "interface-first" convention already
// established in `rdpilot-daemon::registry` (Plan 12-03).
#![allow(dead_code, unused_imports)]

mod cli;
mod config_flags;
mod connect;
mod exit_codes;
mod render;

use clap::Parser;

use cli::{Cli, Command};
use exit_codes::{CliError, exit_code_for};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.command, cli.json).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            exit_code_for(&err)
        }
    }
}

/// Route the parsed [`Command`] to its verb handler. Scaffolding stub
/// (Task 1) — Task 2 wires this to `verbs::session::{connect,list,disconnect}`.
async fn dispatch(_command: Command, _json: bool) -> Result<(), CliError> {
    Err(CliError::Internal("verb dispatch not yet wired (Task 2)".to_owned()))
}
