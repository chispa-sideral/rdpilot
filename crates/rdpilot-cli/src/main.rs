//! `rdpilot` — the CLI binary entry point (CLI-01/02/03).
//!
//! Parses [`cli::Cli`], dispatches to the verb handler, and translates the
//! top-level `Result<(), exit_codes::CliError>` into a distinct non-zero
//! `process::ExitCode` (D-28). Single-shot invoke-and-exit process — never
//! a persistent client — so a `current_thread` Tokio runtime is sufficient.

#![deny(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]

mod cli;
mod config_flags;
mod connect;
mod exit_codes;
mod render;
mod verbs;

use clap::Parser;

use cli::{Cli, Command, SessionCmd};
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

/// Route the parsed [`Command`] (flat or grouped-`session`) to its verb
/// handler — both spellings call the exact same handler function.
async fn dispatch(command: Command, json: bool) -> Result<(), CliError> {
    match command {
        Command::Connect(args) | Command::Session(SessionCmd::Connect(args)) => verbs::session::connect(args, json).await,
        Command::List | Command::Session(SessionCmd::List) => verbs::session::list(json).await,
        Command::Disconnect(args) | Command::Session(SessionCmd::Disconnect(args)) => {
            verbs::session::disconnect(args, json).await
        }
    }
}
