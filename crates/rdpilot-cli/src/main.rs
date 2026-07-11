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

use cli::{Cli, Command, InputCmd, PerceiveCmd, ProcessCmd, SessionCmd, WindowCmd};
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

/// Route the parsed [`Command`] (flat or grouped-`session`/`perceive`/
/// `input`) to its verb handler — flat and grouped spellings of the same
/// verb call the exact same handler function.
async fn dispatch(command: Command, json: bool) -> Result<(), CliError> {
    match command {
        Command::Connect(args) | Command::Session(SessionCmd::Connect(args)) => verbs::session::connect(args, json).await,
        Command::List | Command::Session(SessionCmd::List) => verbs::session::list(json).await,
        Command::Disconnect(args) | Command::Session(SessionCmd::Disconnect(args)) => {
            verbs::session::disconnect(args, json).await
        }

        Command::Perceive(PerceiveCmd::Screenshot(args)) => verbs::perceive::screenshot(args, json).await,
        Command::Perceive(PerceiveCmd::WorldState(args)) => verbs::perceive::world_state(args, json).await,
        Command::Perceive(PerceiveCmd::Uia(args)) => verbs::perceive::uia(args, json).await,
        Command::Perceive(PerceiveCmd::Window(WindowCmd::List(args))) => verbs::perceive::window_list(args, json).await,
        Command::Perceive(PerceiveCmd::Process(ProcessCmd::List(args))) => verbs::perceive::process_list(args, json).await,

        Command::Input(InputCmd::Click(args)) => verbs::input::click(args, json).await,
        Command::Input(InputCmd::Scroll(args)) => verbs::input::scroll(args, json).await,
        Command::Input(InputCmd::Drag(args)) => verbs::input::drag(args, json).await,
        Command::Input(InputCmd::Type(args)) => verbs::input::type_text(args, json).await,
        Command::Input(InputCmd::Key(args)) => verbs::input::key(args, json).await,
        Command::Input(InputCmd::Launch(args)) => verbs::input::launch(args, json).await,
        Command::Input(InputCmd::Foreground(args)) => verbs::input::foreground(args, json).await,
    }
}
