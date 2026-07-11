//! The `clap` command tree (research Pattern 1/2, D-13.1/D-29): a global
//! `Cli` struct mixing flat lifecycle leaf variants (`Connect`/`List`/
//! `Disconnect`) with grouped subcommand families (`Session`/`Perceive`/
//! `Input`) that share leaf arg structs where sensible, plus a global
//! `--json` flag.
//!
//! `Perceive`/`Input` are grouped-ONLY (research D-13.1: "grouped-only"
//! for the perception/input verb sets — no flat top-level spelling, unlike
//! `Session`'s flat+grouped duality). Every leaf's args struct carries a
//! required `session: String` field (D-29, research Pattern 2) except
//! `ConnectArgs`/`List`, which are session-less by design.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

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

/// The top-level verb set: flat lifecycle leaves plus the grouped `session`/
/// `perceive`/`input` families (research Pattern 1).
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

    /// Perception verbs, grouped-only (`rdpilot perceive screenshot|world-state|uia|window|process ...`, CLI-02).
    #[command(subcommand)]
    Perceive(PerceiveCmd),

    /// Input + launch verbs, grouped-only (`rdpilot input click|scroll|drag|type|key|launch|foreground ...`, CLI-02).
    #[command(subcommand)]
    Input(InputCmd),

    /// Upload a local file to the remote transfer root (flat: `rdpilot put ...`, CLI-03).
    Put(PutArgs),
    /// Download a file from the remote transfer root (flat: `rdpilot get ...`, CLI-03).
    Get(GetArgs),

    /// File-transfer verbs, grouped (`rdpilot file put|get ...`, CLI-03).
    #[command(subcommand)]
    File(FileCmd),
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

// --- Perceive (CLI-02) --------------------------------------------------

/// The grouped-only `perceive` subcommand family (research D-13.1/Pattern 1).
#[derive(Debug, Subcommand)]
pub enum PerceiveCmd {
    /// Capture a screenshot of the remote desktop, base64-decode it, and
    /// write the raw PNG bytes to `--output` (D-13.1 — binary bytes never
    /// hit stdout).
    Screenshot(ScreenshotArgs),
    /// Fetch a correlated desktop snapshot (screenshot/window-list/uia a-la-carte).
    WorldState(WorldStateArgs),
    /// Fetch a UI Automation tree for one window.
    Uia(UiaArgs),
    /// The `perceive window` noun group.
    #[command(subcommand)]
    Window(WindowCmd),
    /// The `perceive process` noun group.
    #[command(subcommand)]
    Process(ProcessCmd),
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

/// Which UIA tree(s), if any, a `world-state` request should fetch — the CLI
/// spelling of `rdpilot_ipc::WireUiaMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum UiaModeArg {
    /// Fetch no UIA tree at all (the default — `world-state`'s screenshot-only
    /// a-la-carte default per research; explicit opt-in per component).
    None,
    /// Fetch the UIA tree of only the current foreground window.
    Foreground,
    /// Fetch the UIA tree for every top-level window currently listed.
    All,
    /// Fetch the UIA tree for each `--hwnd` given (repeatable).
    Hwnd,
}

/// `perceive world-state --session <id> [--screenshot] [--window-list] \
/// [--uia-mode <mode>] [--hwnd N ...] [--output <path>]`.
#[derive(Debug, Args)]
pub struct WorldStateArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// Capture a full-desktop screenshot as part of this snapshot.
    #[arg(long)]
    pub screenshot: bool,
    /// Include the top-level window list in the response.
    #[arg(long = "window-list")]
    pub window_list: bool,
    /// Which UIA tree(s), if any, to fetch (default: none).
    #[arg(long = "uia-mode", value_enum, default_value = "none")]
    pub uia_mode: UiaModeArg,
    /// Window handle(s) to fetch a UIA tree for — only meaningful with
    /// `--uia-mode hwnd` (repeatable).
    #[arg(long)]
    pub hwnd: Vec<u64>,
    /// Where to write the embedded screenshot's decoded PNG bytes, if
    /// `--screenshot` was requested and the daemon returned one. Optional —
    /// omitting it while also passing `--screenshot` simply skips the
    /// on-disk write.
    #[arg(long)]
    pub output: Option<PathBuf>,
}

/// How deep a `uia` tree walk should go — the CLI spelling of
/// `rdpilot_ipc::WireUiaScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum UiaScopeArg {
    /// Immediate children only.
    Children,
    /// A bounded, deeper walk up to `--max-depth` levels.
    Subtree,
}

/// `perceive uia --session <id> --hwnd <n> --scope <children|subtree> [--max-depth <n>]`.
#[derive(Debug, Args)]
pub struct UiaArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// The target window handle.
    #[arg(long)]
    pub hwnd: u64,
    /// How deep to walk the UIA tree.
    #[arg(long, value_enum)]
    pub scope: UiaScopeArg,
    /// How many levels below the target window to walk. Only meaningful
    /// with `--scope subtree` (defaults to 1 level if omitted).
    #[arg(long = "max-depth")]
    pub max_depth: Option<u32>,
}

/// The `perceive window` noun group.
#[derive(Debug, Subcommand)]
pub enum WindowCmd {
    /// List the top-level windows on the remote desktop.
    List(SessionArg),
}

/// The `perceive process` noun group.
#[derive(Debug, Subcommand)]
pub enum ProcessCmd {
    /// List the remote process tree.
    List(SessionArg),
}

// --- Input + launch (CLI-02) ---------------------------------------------

/// The grouped-only `input` subcommand family (research D-13.1/Pattern 1).
#[derive(Debug, Subcommand)]
pub enum InputCmd {
    /// Move to `(x, y)` then press-and-release (or double-click, with
    /// `--double`) a mouse button.
    Click(ClickArgs),
    /// Move to `(x, y)` then scroll vertically.
    Scroll(ScrollArgs),
    /// Press a button at the origin, move to the destination, then release.
    Drag(DragArgs),
    /// Type literal text, one Unicode code point at a time.
    #[command(name = "type")]
    Type(TypeArgs),
    /// Press a combination of named keys, in order, then release in reverse
    /// order.
    Key(KeyArgs),
    /// Launch a process on the remote machine.
    Launch(LaunchArgs),
    /// Bring a remote window to the foreground.
    Foreground(ForegroundArgs),
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

/// `input launch --session <id> --exe <path> [--args <string>] [--cwd <path>]`.
#[derive(Debug, Args)]
pub struct LaunchArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// The executable path.
    #[arg(long)]
    pub exe: String,
    /// Optional command-line arguments.
    #[arg(long)]
    pub args: Option<String>,
    /// Optional working directory.
    #[arg(long)]
    pub cwd: Option<String>,
}

/// `input foreground --session <id> --hwnd <n>`.
#[derive(Debug, Args)]
pub struct ForegroundArgs {
    /// The session to target.
    #[arg(long)]
    pub session: String,
    /// The target window handle.
    #[arg(long)]
    pub hwnd: u64,
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

/// `put --session <id> --local <path> --remote-name <name> [--force]`
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
    /// Accepted for forward-compatibility only: `put`'s REMOTE destination
    /// is not currently existence-checked (no cheap way to probe the remote
    /// filesystem without a new sensor round trip, out of scope this
    /// phase — see backlog Phase 999.5, symmetric remote no-clobber). This
    /// flag changes nothing about `put`'s wire behavior yet; it exists so a
    /// caller's `put ... --force` script does not need to change once
    /// remote no-clobber ships. Contrast with `get --force`, which IS fully
    /// enforced client-side today.
    #[arg(
        long,
        help = "Accepted for forward-compat only — remote overwrite is NOT prevented this phase (see backlog: symmetric remote no-clobber, Phase 999.5)"
    )]
    pub force: bool,
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
    #[arg(long, help = "Overwrite an existing local destination (without it, a pre-existing --local path is refused)")]
    pub force: bool,
}
