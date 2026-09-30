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
mod connect;
mod exit_codes;
mod password_command;
mod render;
mod verbs;

use clap::Parser;

use cli::{
    Cli, Command, ConfigCmd, FileCmd, InputCmd, PerceiveCmd, RecordCmd, RecordingCmd, SessionCmd,
};
use exit_codes::{code_str_for, exit_code_for, CliError};

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match dispatch(cli.command, json).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // D-28: legible under both human and machine output. `--json`
            // emits the same shape a successful verb would (stdout), so a
            // scripted caller never has to branch its parser on exit
            // status alone to find the error payload.
            if json {
                let payload = serde_json::json!({
                    "error": { "code": code_str_for(&err), "message": err.to_string() }
                });
                println!("{payload}");
            } else {
                eprintln!("error: {err}");
            }
            exit_code_for(&err)
        }
    }
}

/// Route the parsed [`Command`] (flat or grouped-`session`/`perceive`/
/// `input`) to its verb handler — flat and grouped spellings of the same
/// verb call the exact same handler function.
async fn dispatch(command: Command, json: bool) -> Result<(), CliError> {
    match command {
        Command::Connect(args) | Command::Session(SessionCmd::Connect(args)) => {
            verbs::session::connect(args, json).await
        }
        Command::Ping(args) => verbs::session::ping(args, json).await,
        Command::List | Command::Session(SessionCmd::List) => verbs::session::list(json).await,
        Command::Disconnect(args) | Command::Session(SessionCmd::Disconnect(args)) => {
            verbs::session::disconnect(args, json).await
        }

        Command::Screenshot(args) | Command::Perceive(PerceiveCmd::Screenshot(args)) => {
            verbs::perceive::screenshot(args, json).await
        }

        Command::Input(InputCmd::Click(args)) => verbs::input::click(args, json).await,
        Command::Input(InputCmd::Scroll(args)) => verbs::input::scroll(args, json).await,
        Command::Input(InputCmd::Drag(args)) => verbs::input::drag(args, json).await,
        Command::Input(InputCmd::Type(args)) => verbs::input::type_text(args, json).await,
        Command::Input(InputCmd::Key(args)) => verbs::input::key(args, json).await,

        Command::Put(args) | Command::File(FileCmd::Put(args)) => {
            verbs::file::put(args, json).await
        }
        Command::Get(args) | Command::File(FileCmd::Get(args)) => {
            verbs::file::get(args, json).await
        }
        Command::Config(ConfigCmd::Resolve(args)) => verbs::config::resolve(args, json),
        Command::View(args) => verbs::view::view(args, json).await,
        Command::Record(RecordCmd::Start(args)) => verbs::record::start(args, json).await,
        Command::Record(RecordCmd::Stop(args)) => verbs::record::stop(args, json).await,
        Command::Annotate(args) => verbs::record::annotate(args, json).await,
        Command::Recording(RecordingCmd::List) => verbs::record::list(json).await,
        Command::Recording(RecordingCmd::Keep(args)) => verbs::record::keep(args, true, json).await,
        Command::Recording(RecordingCmd::Unkeep(args)) => {
            verbs::record::keep(args, false, json).await
        }
    }
}
