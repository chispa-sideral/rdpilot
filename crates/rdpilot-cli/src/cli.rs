//! Named connection management, native RDP recovery and explicit file transfer.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `rdpilot` — a thin CLI client for the `rdpilot-daemon` session registry.
#[derive(Debug, Parser)]
#[command(
    name = "rdpilot",
    version,
    about = "Control a remote Windows desktop over RDP through the rdpilot daemon."
)]
pub struct Cli {
    /// The verb to run.
    #[command(subcommand)]
    pub command: Command,

    /// Emit machine-readable JSON instead of a human-readable table.
    #[arg(long, global = true)]
    pub json: bool,
}

/// The top-level verb set: flat lifecycle leaves plus the grouped `session`/
/// `perceive`/`input` families (research Pattern 1).
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open a new session against a remote RDP target (flat: `rdpilot connect ...`).
    Connect(ConnectArgs),
    /// List the sessions currently known to the daemon (flat: `rdpilot list`).
    List,
    /// Round-trip a bridge ping.
    Ping(SessionArg),
    /// Capture the native RDP framebuffer.
    Screenshot(ScreenshotArgs),
    /// Disconnect a named/identified session (flat: `rdpilot disconnect ...`).
    Disconnect(SessionArg),

    /// Session lifecycle verbs, grouped (`rdpilot session connect|list|disconnect ...`).
    #[command(subcommand)]
    Session(SessionCmd),

    /// Perception verbs, grouped-only (`rdpilot perceive screenshot ...`, CLI-02).
    #[command(subcommand)]
    Perceive(PerceiveCmd),

    /// Native recovery input, grouped-only (`rdpilot input click|scroll|drag|type|key ...`, CLI-02).
    #[command(subcommand)]
    Input(InputCmd),

    /// Upload a local file to the remote transfer root (flat: `rdpilot put ...`, CLI-03).
    Put(PutArgs),
    /// Download a file from the remote transfer root (flat: `rdpilot get ...`, CLI-03).
    Get(GetArgs),

    /// File-transfer verbs, grouped (`rdpilot file put|get ...`, CLI-03).
    #[command(subcommand)]
    File(FileCmd),

    /// Inspect host configuration (`rdpilot config resolve ...`).
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Start the read-only live viewer for the running daemon and print its
    /// URLs. Runs until Ctrl-C; never starts a daemon.
    View(ViewArgs),
}

/// `view [--bind loopback|loopback+tailnet] [--tailnet-address <ipv4>]`.
/// Unset flags fall back to the `[viewer]` config table and
/// `RDPILOT_VIEWER__*` environment variables.
#[derive(Debug, Args)]
pub struct ViewArgs {
    /// Bind set: `loopback` (127.0.0.1 only) or `loopback+tailnet` (also the
    /// host's Tailscale address). Default: `loopback+tailnet`.
    #[arg(long, value_parser = parse_viewer_bind)]
    pub bind: Option<rdpilot_config::ViewerBind>,
    /// Tailscale IPv4 address to bind when automatic detection finds none or
    /// several (must be in 100.64.0.0/10 and present on this host).
    #[arg(long = "tailnet-address")]
    pub tailnet_address: Option<std::net::Ipv4Addr>,
}

fn parse_viewer_bind(value: &str) -> Result<rdpilot_config::ViewerBind, String> {
    value.parse()
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

/// Which host to configure, and how: an alias or `rdp://` URL, plus overrides.
#[derive(Debug, Args)]
pub struct TargetArgs {
    /// Host alias from the hosts file, or an `rdp://[domain\\]user[:password]@host[:port]`
    /// URL (`rdps://` is the same). A password in a URL is visible in `ps` and shell
    /// history; prefer `PasswordCommand`.
    pub target: String,

    /// Override a setting, ssh-style (`-o Keyword=value`). Repeatable; the first
    /// value obtained for a setting wins, so `-o` beats the hosts file.
    #[arg(short = 'o', long = "option", value_name = "KEYWORD=VALUE")]
    pub options: Vec<String>,

    /// Read this hosts file instead of the user's (must exist).
    #[arg(short = 'F', value_name = "FILE")]
    pub file: Option<PathBuf>,
}

/// Arguments for `connect` (D-29, SESSION-01): the target, its overrides and
/// an optional caller-supplied session name.
#[derive(Debug, Args)]
pub struct ConnectArgs {
    #[command(flatten)]
    pub target: TargetArgs,

    /// Caller-supplied session name; omit for an auto-generated id (D-29).
    #[arg(long)]
    pub name: Option<String>,
}

/// The `config` subcommand family.
#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Show the settings a target resolves to, with where each came from.
    /// Never runs a `PasswordCommand` and never prints a password.
    Resolve(TargetArgs),
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

// --- Perceive (CLI-02) --------------------------------------------------

/// The grouped-only `perceive` subcommand family (research D-13.1/Pattern 1).
#[derive(Debug, Subcommand)]
pub enum PerceiveCmd {
    /// Capture a screenshot of the remote desktop, base64-decode it, and
    /// write the raw PNG bytes to `--output` (D-13.1 — binary bytes never
    /// hit stdout).
    Screenshot(ScreenshotArgs),
}

/// `perceive screenshot --session <id> --output <path>` (D-13.1: `--output`
/// is REQUIRED, no default — omitting it is a clap parse error).
#[derive(Debug, Args)]
pub struct ScreenshotArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// Where to write the decoded PNG bytes. Required — image bytes are
    /// never written to stdout (D-13.1).
    #[arg(long)]
    pub output: PathBuf,
}

/// Native RDP input for bootstrap and recovery.
#[derive(Debug, Subcommand)]
pub enum InputCmd {
    Click(ClickArgs),
    Scroll(ScrollArgs),
    Drag(DragArgs),
    #[command(name = "type")]
    Type(TypeArgs),
    Key(KeyArgs),
}

/// A mouse button — the CLI spelling of `rdpilot_ipc::WireButton`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ButtonArg {
    /// The left (primary) mouse button.
    Left,
    /// The right (secondary/context-menu) mouse button.
    Right,
    /// The middle (wheel) mouse button.
    Middle,
}

/// `input click --session <id> --x <n> --y <n> [--button <button>] [--double]`.
#[derive(Debug, Args)]
pub struct ClickArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// Target x, in physical virtual-desktop pixels.
    #[arg(long)]
    pub x: u16,
    /// Target y, in physical virtual-desktop pixels.
    #[arg(long)]
    pub y: u16,
    /// The button to click (default: left).
    #[arg(long, value_enum, default_value = "left")]
    pub button: ButtonArg,
    /// Double-click instead of a single click.
    #[arg(long)]
    pub double: bool,
}

/// `input scroll --session <id> --x <n> --y <n> --dy <n>`.
#[derive(Debug, Args)]
pub struct ScrollArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// Target x, in physical virtual-desktop pixels.
    #[arg(long)]
    pub x: u16,
    /// Target y, in physical virtual-desktop pixels.
    #[arg(long)]
    pub y: u16,
    /// Signed count of `WHEEL_DELTA` (120-unit) notches, positive = away
    /// from the user.
    #[arg(long, allow_hyphen_values = true)]
    pub dy: i16,
}

/// `input drag --session <id> --from-x <n> --from-y <n> --to-x <n> --to-y <n> [--button <button>]`.
#[derive(Debug, Args)]
pub struct DragArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// Origin x, in physical virtual-desktop pixels.
    #[arg(long = "from-x")]
    pub from_x: u16,
    /// Origin y, in physical virtual-desktop pixels.
    #[arg(long = "from-y")]
    pub from_y: u16,
    /// Destination x, in physical virtual-desktop pixels.
    #[arg(long = "to-x")]
    pub to_x: u16,
    /// Destination y, in physical virtual-desktop pixels.
    #[arg(long = "to-y")]
    pub to_y: u16,
    /// The button to drag with (default: left).
    #[arg(long, value_enum, default_value = "left")]
    pub button: ButtonArg,
}

/// `input type --session <id> --text <string>`.
#[derive(Debug, Args)]
pub struct TypeArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// The literal text to type.
    #[arg(long)]
    pub text: String,
}

/// `input key --session <id> --combo <comma-separated key names>`.
#[derive(Debug, Args)]
pub struct KeyArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// Comma-separated key names (e.g. `ctrl,a`), pressed in order and
    /// released in reverse order. Unrecognized names produce a legible
    /// error, never a panic.
    #[arg(long)]
    pub combo: String,
}

// --- File transfer (CLI-03) ----------------------------------------------

/// The grouped `file` subcommand family — shares `PutArgs`/`GetArgs` with
/// the flat `Put`/`Get` leaves above so flat and grouped invocations parse
/// identically (research Pattern 1, D-13.1).
#[derive(Debug, Subcommand)]
pub enum FileCmd {
    /// Upload a local file to the remote transfer root.
    Put(PutArgs),
    /// Download a file from the remote transfer root.
    Get(GetArgs),
}

/// `put --session <id> --local <path> --remote-name <name>`
/// (CLI-03). No file bytes cross the wire — the daemon and CLI share a
/// filesystem, so only the absolutized local path + the remote destination
/// name travel over IPC.
#[derive(Debug, Args)]
pub struct PutArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// The local file to upload. Resolved to an absolute path (joined onto
    /// the CLI process's own current directory, NOT the daemon's) before
    /// being sent — the daemon's cwd differs from the invoking shell's.
    #[arg(long)]
    pub local: std::path::PathBuf,
    /// The destination name under the remote transfer root.
    #[arg(long = "remote-name")]
    pub remote_name: String,
}

/// `get --session <id> --remote-name <name> --local <path> [--force]`
/// (CLI-03). The local destination is fully no-clobber-enforced: if it
/// already exists and `--force` is absent, the CLI refuses before ever
/// sending the request (`CliError::NoClobber`, exit code 8).
#[derive(Debug, Args)]
pub struct GetArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// The source name under the remote transfer root.
    #[arg(long = "remote-name")]
    pub remote_name: String,
    /// The local destination path. Resolved to an absolute path (joined
    /// onto the CLI process's own current directory, NOT the daemon's)
    /// before being sent, and checked for a pre-existing file at that
    /// absolute path (no-clobber).
    #[arg(long)]
    pub local: std::path::PathBuf,
    /// Overwrite an existing local destination. Without this flag, a
    /// pre-existing `--local` path is refused (exit code 8) before any
    /// request is sent to the daemon.
    #[arg(
        long,
        help = "Overwrite an existing local destination (without it, a pre-existing --local path is refused)"
    )]
    pub force: bool,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        let mut argv = vec!["rdpilot"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv)
    }

    #[test]
    fn connect_takes_a_target_options_and_a_hosts_file() {
        let cli = parse(&[
            "connect",
            "web1",
            "-o",
            "Port=3390",
            "-o",
            "User=a",
            "-F",
            "h.conf",
            "--name",
            "n",
        ])
        .expect("parses");
        let Command::Connect(args) = cli.command else {
            panic!("expected connect");
        };
        assert_eq!(args.target.target, "web1");
        assert_eq!(args.target.options, ["Port=3390", "User=a"]);
        assert_eq!(args.target.file, Some(PathBuf::from("h.conf")));
        assert_eq!(args.name.as_deref(), Some("n"));
    }

    #[test]
    fn grouped_and_config_forms_parse() {
        assert!(parse(&["session", "connect", "rdp://u@h"]).is_ok());
        assert!(parse(&["config", "resolve", "web1", "-o", "CuaEnabled=no"]).is_ok());
        assert!(parse(&["connect"]).is_err(), "the target is required");
    }

    #[test]
    fn removed_connection_flags_are_rejected() {
        for flag in [
            &["--host", "h"][..],
            &["--port", "1"],
            &["--username", "u"],
            &["--password", "p"],
            &["--domain", "d"],
            &["--accept-invalid-certs"],
        ] {
            let mut args = vec!["connect", "web1"];
            args.extend_from_slice(flag);
            assert!(parse(&args).is_err(), "{flag:?} must be rejected");
        }
    }
}
