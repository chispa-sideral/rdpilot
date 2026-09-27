//! `put`/`get` — CLI-03's file-transfer verbs. Each is a thin invoke-and-exit
//! `rdpilot-ipc` client: absolutize the local path (research Pitfall 1 — the
//! daemon's cwd differs from the invoking shell's, so a relative path must
//! never hit the wire), round-trip exactly one `Request`/`WireResponse`
//! frame pair against a required `--session` (D-29), and render the
//! resulting `TransferOutcome` (table by default, `--json` opt-in).
//!
//! No file bytes ever cross the IPC wire — the daemon and CLI share a
//! filesystem (research: "Daemon and CLI run on the SAME machine"), so only
//! paths + `TransferOutcome` metadata travel over the socket.
//!
//! Upload refuses an existing remote destination atomically in the bridge.
//! Download checks the caller-local destination and requires `--force` to replace it.

use rdpilot_ipc::{Request, WireResponse};

use crate::cli::{GetArgs, PutArgs};
use crate::connect::round_trip;
use crate::exit_codes::CliError;
use crate::render::print_json;

/// `put --session <id> --local <path> --remote-name <name>`.
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session` or a failure resolving
/// `--local` to an absolute path; the daemon's own error otherwise (e.g.
/// `SessionNotFound`, `TransferFailed`, `PathTraversal`,
/// `ChecksumMismatch`); or a transport/auto-start failure.
pub async fn put(args: PutArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let local_path = absolutize(&args.local)?;

    let req = Request::Put {
        session,
        local_path: local_path.to_string_lossy().into_owned(),
        remote_name: args.remote_name.clone(),
    };
    match round_trip(req).await? {
        WireResponse::Transfer(outcome) => render_transfer(&outcome, json),
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to Put: {other:?}"
        ))),
    }
}

/// `get --session <id> --remote-name <name> --local <path> [--force]`.
///
/// Refuses to overwrite an existing `--local` destination unless `--force`
/// is passed — this check runs BEFORE the request is sent (no wasted round
/// trip on a refusal the CLI can already see locally).
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session` or a failure resolving
/// `--local` to an absolute path; [`CliError::NoClobber`] if the
/// absolutized `--local` destination already exists and `--force` was not
/// passed; the daemon's own error otherwise; or a transport/auto-start
/// failure.
pub async fn get(args: GetArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    let local_path = absolutize(&args.local)?;

    if local_path.exists() && !args.force {
        return Err(CliError::NoClobber(format!(
            "refusing to overwrite {}; pass --force to override",
            local_path.display()
        )));
    }

    // Download to an absent private sibling path, then atomically replace only
    // after the SDK has verified bytes. A failed download leaves the original intact.
    let replacement = if args.force {
        Some(Replacement::new(&local_path)?)
    } else {
        None
    };
    let download_path = replacement
        .as_ref()
        .map_or(local_path.clone(), |r| r.file());
    let req = Request::Get {
        session,
        remote_name: args.remote_name.clone(),
        local_path: download_path.to_string_lossy().into_owned(),
    };
    match round_trip(req).await? {
        WireResponse::Transfer(outcome) => {
            if let Some(replacement) = replacement {
                std::fs::rename(replacement.file(), &local_path).map_err(|e| {
                    CliError::Internal(format!("could not replace destination: {e}"))
                })?;
            }
            render_transfer(&outcome, json)
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to Get: {other:?}"
        ))),
    }
}

/// Resolve `path` to an absolute path by joining it onto
/// `std::env::current_dir()` when relative (research Pitfall 1, scope
/// decisions: deliberately NOT the stdlib absolutization helper that
/// stabilized in Rust 1.79 — this workspace pins `rust-version = "1.78"`).
/// An already-absolute path passes through unchanged.
fn absolutize(path: &std::path::Path) -> Result<std::path::PathBuf, CliError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            CliError::Internal(format!("failed to resolve the current directory: {e}"))
        })?;
        Ok(cwd.join(path))
    }
}

/// Render a completed `Put`/`Get` transfer's outcome.
fn render_transfer(outcome: &rdpilot_ipc::TransferOutcome, json: bool) -> Result<(), CliError> {
    if json {
        print_json(outcome)
    } else {
        println!(
            "transferred {} bytes (checksum {})",
            outcome.bytes_transferred, outcome.checksum
        );
        Ok(())
    }
}

/// Private sibling directory avoids a file reservation race with SDK create_new.
struct Replacement(std::path::PathBuf);
impl Replacement {
    fn new(destination: &std::path::Path) -> Result<Self, CliError> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let parent = destination
            .parent()
            .ok_or_else(|| CliError::Internal("destination has no parent".into()))?;
        for _ in 0..100 {
            let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = parent.join(format!(
                ".rdpilot-download-{}-{sequence}",
                std::process::id()
            ));
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(CliError::Internal(format!("could not stage download: {e}"))),
            }
        }
        Err(CliError::Internal(
            "could not reserve download staging directory".into(),
        ))
    }
    fn file(&self) -> std::path::PathBuf {
        self.0.join("payload")
    }
}
impl Drop for Replacement {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolutize_passes_an_already_absolute_path_through_unchanged(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let abs = std::path::PathBuf::from("/tmp/some/file.bin");
        let resolved = absolutize(&abs)?;
        assert_eq!(resolved, abs);
        Ok(())
    }

    #[test]
    fn absolutize_joins_a_relative_path_onto_current_dir() -> Result<(), Box<dyn std::error::Error>>
    {
        let cwd = std::env::current_dir()?;
        let resolved = absolutize(std::path::Path::new("relative/file.bin"))?;
        assert_eq!(resolved, cwd.join("relative/file.bin"));
        assert!(resolved.is_absolute());
        Ok(())
    }
}
