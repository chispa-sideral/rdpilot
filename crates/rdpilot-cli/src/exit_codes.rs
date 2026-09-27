//! [`CliError`] — the client-side error type every verb handler returns —
//! and [`exit_code_for`] — the D-28 distinct-non-zero-exit-code mapping.

use std::fmt;
use std::process::ExitCode;

use rdpilot_ipc::{WireError, WireErrorCode, daemon_incompatible_message};

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
    /// The daemon completed the harmless List probe but reported a legacy or
    /// incompatible wire identity. Raised before credentials or operations.
    DaemonIncompatible(Option<u32>),
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
            CliError::DaemonIncompatible(observed) => write!(f, "{}", daemon_incompatible_message(*observed)),
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
            WireErrorCode::SecureDesktopActive => 9,
            WireErrorCode::UacPromptNotActive => 10,
            WireErrorCode::UacResponseUnconfirmed => 11,
            // `Internal`, the client-only `DaemonUnreachable` wire variant
            // (never actually produced by the daemon — see
            // `CliError::DaemonUnreachable` for the real client-side path),
            // and any future `#[non_exhaustive]` code all fall back to 1.
            _ => 1,
        },
        CliError::DaemonUnreachable(_) => 3,
        CliError::DaemonIncompatible(_) => 12,
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

/// The kebab-case wire-style error code string for `err` (D-28), used by
/// the `--json` error-rendering path in `main.rs`
/// (`{"error":{"code":"<kebab>","message":"..."}}`). For [`CliError::Wire`]
/// this is exactly the wire's own `#[serde(rename_all = "kebab-case")]`
/// string (`WireErrorCode` serializes to e.g. `"session-not-found"`); every
/// client-local class gets its own fixed kebab spelling so the two families
/// share one legible vocabulary.
#[must_use]
pub fn code_str_for(err: &CliError) -> String {
    match err {
        CliError::Wire(wire) => serde_json::to_string(&wire.code)
            .ok()
            .map(|s| s.trim_matches('"').to_owned())
            .unwrap_or_else(|| "internal".to_owned()),
        CliError::DaemonUnreachable(_) => "daemon-unreachable".to_owned(),
        CliError::DaemonIncompatible(_) => "daemon-incompatible".to_owned(),
        CliError::NoClobber(_) => "no-clobber".to_owned(),
        CliError::MissingConfig(_) => "missing-config".to_owned(),
        CliError::Internal(_) => "internal".to_owned(),
        CliError::Transport(_) => "transport".to_owned(),
    }
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
        assert_eq!(code_for(&wire(WireErrorCode::SecureDesktopActive)), 9);
        assert_eq!(code_for(&wire(WireErrorCode::UacPromptNotActive)), 10);
        assert_eq!(code_for(&wire(WireErrorCode::UacResponseUnconfirmed)), 11);
        assert_eq!(code_for(&wire(WireErrorCode::Internal)), 1);
    }

    /// Every wire error class maps to a DISTINCT exit code (D-28) — a
    /// regression guard specifically for the UAC-related codes (9/10/11),
    /// proving they do not collide with any of the 1-8 codes already taken.
    #[test]
    fn distinct_codes_for_every_wire_error_class_are_pairwise_unique() {
        let codes = [
            WireErrorCode::SessionNotFound,
            WireErrorCode::TransferFailed,
            WireErrorCode::PathTraversal,
            WireErrorCode::ChecksumMismatch,
            WireErrorCode::DuplicateSession,
            WireErrorCode::SecureDesktopActive,
            WireErrorCode::UacPromptNotActive,
            WireErrorCode::UacResponseUnconfirmed,
        ]
        .map(|code| code_for(&wire(code)));
        let unique: std::collections::HashSet<u8> = codes.iter().copied().collect();
        assert_eq!(unique.len(), codes.len(), "expected every wire error class to map to a distinct exit code: {codes:?}");
    }

    #[test]
    fn distinct_codes_for_client_local_classes() {
        assert_eq!(code_for(&CliError::DaemonUnreachable("x".to_owned())), 3);
        assert_eq!(code_for(&CliError::DaemonIncompatible(None)), 12);
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

    #[test]
    fn code_str_for_wire_errors_matches_the_wire_kebab_case_string() {
        assert_eq!(code_str_for(&wire(WireErrorCode::SessionNotFound)), "session-not-found");
        assert_eq!(code_str_for(&wire(WireErrorCode::TransferFailed)), "transfer-failed");
        assert_eq!(code_str_for(&wire(WireErrorCode::PathTraversal)), "path-traversal");
        assert_eq!(code_str_for(&wire(WireErrorCode::ChecksumMismatch)), "checksum-mismatch");
        assert_eq!(code_str_for(&wire(WireErrorCode::DuplicateSession)), "duplicate-session");
        assert_eq!(code_str_for(&wire(WireErrorCode::SecureDesktopActive)), "secure-desktop-active");
        assert_eq!(code_str_for(&wire(WireErrorCode::UacPromptNotActive)), "uac-prompt-not-active");
        assert_eq!(code_str_for(&wire(WireErrorCode::UacResponseUnconfirmed)), "uac-response-unconfirmed");
        assert_eq!(code_str_for(&wire(WireErrorCode::Internal)), "internal");
    }

    #[test]
    fn code_str_for_client_local_classes_is_distinct_and_stable() {
        assert_eq!(code_str_for(&CliError::DaemonUnreachable("x".to_owned())), "daemon-unreachable");
        assert_eq!(code_str_for(&CliError::DaemonIncompatible(None)), "daemon-incompatible");
        assert_eq!(code_str_for(&CliError::NoClobber("x".to_owned())), "no-clobber");
        assert_eq!(code_str_for(&CliError::MissingConfig("x".to_owned())), "missing-config");
        assert_eq!(code_str_for(&CliError::Internal("x".to_owned())), "internal");
        assert_eq!(code_str_for(&CliError::Transport("x".to_owned())), "transport");
    }
}
