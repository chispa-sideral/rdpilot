//! The `clap` command tree (research Pattern 1/2, D-13.1/D-29): a global
//! `Cli` struct mixing flat lifecycle leaf variants (`Connect`/`List`/
//! `Disconnect`) with a grouped `Session` subcommand family that shares the
//! SAME leaf arg structs, plus a global `--json` flag.
//!
//! `Session`/`ConnectArgs`/`SessionArg` are structured so adding the
//! `Perceive`/`Input`/`File` grouped families (Plans 13-06/13-07) and the
//! flat `Put`/`Get` leaves is additive — no rework of this plan's shape.

use clap::{Args, Parser, Subcommand};

use crate::config_flags::ConfigFlags;

/// `rdpilot` — a thin CLI client for the `rdpilot-daemon` session registry.
#[derive(Debug, Parser)]
#[command(name = "rdpilot", version, about = "Control a remote Windows desktop over RDP through the rdpilot daemon.")]
pub struct Cli {
    /// The verb to run.
    #[command(subcommand)]
    pub command: Command,

    /// Emit machine-readable JSON instead of a human-readable table.
    #[arg(long, global = true)]
    pub json: bool,
}

/// The top-level verb set: flat lifecycle leaves plus the grouped `session`
/// family (research Pattern 1).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open a new session against a remote RDP target (flat: `rdpilot connect ...`).
    Connect(ConnectArgs),
    /// List the sessions currently known to the daemon (flat: `rdpilot list`).
    List,
    /// Disconnect a named/identified session (flat: `rdpilot disconnect ...`).
    Disconnect(SessionArg),

    /// Session lifecycle verbs, grouped (`rdpilot session connect|list|disconnect ...`).
    #[command(subcommand)]
    Session(SessionCmd),
}

/// The grouped `session` subcommand family — shares `ConnectArgs`/`SessionArg`
/// with the flat leaves above so flat and grouped invocations parse
/// identically (research Pattern 1).
#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// Open a new session against a remote RDP target.
    Connect(ConnectArgs),
    /// List the sessions currently known to the daemon.
    List,
    /// Disconnect a named/identified session.
    Disconnect(SessionArg),
}

/// Arguments for `connect` (D-29, SESSION-01): an optional caller-supplied
/// session name plus the layered config-override flags.
#[derive(Debug, Args)]
pub struct ConnectArgs {
    /// Caller-supplied session name; omit for an auto-generated id (D-29).
    #[arg(long)]
    pub name: Option<String>,

    #[command(flatten)]
    pub config: ConfigFlags,
}

/// A required `--session` field on every session-scoped leaf args struct
/// (research Pattern 2, D-29: required on every session-scoped verb, absent
/// on `connect`/`list`).
#[derive(Debug, Args)]
pub struct SessionArg {
    /// The session to target.
    #[arg(long)]
    pub session: String,
}
