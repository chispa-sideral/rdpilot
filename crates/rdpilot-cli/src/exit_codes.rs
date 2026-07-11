//! [`CliError`] — the client-side error type every verb handler returns —
//! and [`exit_code_for`] — the D-28 distinct-non-zero-exit-code mapping.

use std::fmt;
use std::process::ExitCode;

use rdpilot_ipc::{WireError, WireErrorCode};

/// The client-side error type every verb handler returns. Wraps either a
/// typed daemon-side [`WireError`] (D-28) or a client-local failure class
/// that never crosses the wire.
#[derive(Debug)]
pub enum CliError {
    /// A typed error returned by the daemon over the wire (D-28).
    Wire(WireError),
    /// `connect_or_spawn`'s bounded backoff exhausted without the daemon
    /// becoming reachable — client-only; `WireErrorCode::DaemonUnreachable`
    /// is never actually wire-transmitted by the daemon (research CLI-03
    /// taxonomy).
    DaemonUnreachable(String),
    /// A destination file already exists and `--force` was not supplied
    /// (CLI-local class; the check itself is wired in Plan 13-07).
    #[allow(dead_code)] // Declared now (D-28's full 8-code taxonomy), constructed by Plan 13-07's `put`/`get` --force check.
    NoClobber(String),
    /// A resolved configuration is missing a value required for this verb
    /// (e.g. `connect` needs host/username/password after file->env->flag
    /// resolution).
    MissingConfig(String),
    /// The daemon replied with a `WireResponse` variant this verb handler
    /// did not expect.
    Internal(String),
    /// A transport-layer failure (frame encode/decode, I/O) below the wire
    /// protocol itself.
    Transport(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Wire(err) => write!(f, "{err}"),
            CliError::DaemonUnreachable(msg) => write!(f, "daemon unreachable: {msg}"),
            CliError::NoClobber(msg) => write!(f, "destination already exists (use --force to overwrite): {msg}"),
            CliError::MissingConfig(msg) => write!(f, "missing required configuration: {msg}"),
            CliError::Internal(msg) => write!(f, "unexpected daemon response: {msg}"),
            CliError::Transport(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<WireError> for CliError {
    fn from(err: WireError) -> Self {
        CliError::Wire(err)
    }
}

/// The DISTINCT non-zero exit code for each [`CliError`] class (D-28).
fn code_for(err: &CliError) -> u8 {
    match err {
        CliError::Wire(wire) => match wire.code {
            WireErrorCode::SessionNotFound => 2,
            WireErrorCode::TransferFailed => 4,
            WireErrorCode::PathTraversal => 5,
            WireErrorCode::ChecksumMismatch => 6,
            WireErrorCode::DuplicateSession => 7,
            // `Internal`, the client-only `DaemonUnreachable` wire variant
            // (never actually produced by the daemon — see
            // `CliError::DaemonUnreachable` for the real client-side path),
            // and any future `#[non_exhaustive]` code all fall back to 1.
            _ => 1,
        },
        CliError::DaemonUnreachable(_) => 3,
        CliError::NoClobber(_) => 8,
        CliError::MissingConfig(_) | CliError::Internal(_) | CliError::Transport(_) => 1,
    }
}

/// Map `err` to a [`ExitCode`] via [`code_for`]. Only ever called on the
/// `Err` path — a successful verb handler exits `0` via
/// `ExitCode::SUCCESS` in `main.rs`.
#[must_use]
pub fn exit_code_for(err: &CliError) -> ExitCode {
    ExitCode::from(code_for(err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(code: WireErrorCode) -> CliError {
        CliError::Wire(WireError { code, message: "x".to_owned() })
    }

    #[test]
    fn distinct_codes_for_every_wire_error_class() {
        assert_eq!(code_for(&wire(WireErrorCode::SessionNotFound)), 2);
        assert_eq!(code_for(&wire(WireErrorCode::TransferFailed)), 4);
        assert_eq!(code_for(&wire(WireErrorCode::PathTraversal)), 5);
        assert_eq!(code_for(&wire(WireErrorCode::ChecksumMismatch)), 6);
        assert_eq!(code_for(&wire(WireErrorCode::DuplicateSession)), 7);
        assert_eq!(code_for(&wire(WireErrorCode::Internal)), 1);
    }

    #[test]
    fn distinct_codes_for_client_local_classes() {
        assert_eq!(code_for(&CliError::DaemonUnreachable("x".to_owned())), 3);
        assert_eq!(code_for(&CliError::NoClobber("x".to_owned())), 8);
        assert_eq!(code_for(&CliError::MissingConfig("x".to_owned())), 1);
        assert_eq!(code_for(&CliError::Internal("x".to_owned())), 1);
        assert_eq!(code_for(&CliError::Transport("x".to_owned())), 1);
    }

    #[test]
    fn cli_error_display_never_panics_and_includes_context() {
        let rendered = format!("{}", CliError::MissingConfig("host is required".to_owned()));
        assert!(rendered.contains("host is required"));
    }
}
