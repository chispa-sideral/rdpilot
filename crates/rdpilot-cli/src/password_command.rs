//! Run a `PasswordCommand` and capture its stdout as the password.
//!
//! The command runs through the platform shell (`sh -c` on Unix, `cmd /C` on
//! Windows) with stdin closed, stderr inherited (so an interactive helper can
//! still show a prompt on the terminal) and stdout captured. One trailing
//! newline is stripped. The child is killed if it is still running after the
//! timeout; a grandchild it spawned may outlive it.

use std::process::Stdio;
use std::time::Duration;

use rdpilot_config::hosts::Secret;
use tokio::io::AsyncReadExt;

use crate::exit_codes::CliError;

/// How long a `PasswordCommand` may run.
pub const TIMEOUT: Duration = Duration::from_secs(30);

fn shell(cmd: &str) -> tokio::process::Command {
    #[cfg(windows)]
    let std_cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").raw_arg(cmd);
        c
    };
    #[cfg(not(windows))]
    let std_cmd = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    };
    tokio::process::Command::from(std_cmd)
}

/// Run `cmd` and return its stdout, minus one trailing newline.
///
/// # Errors
///
/// [`CliError::Config`] naming `label` when the command cannot start, exits
/// non-zero, prints nothing, prints non-UTF-8, or exceeds `timeout`. The
/// error never contains the command's output.
pub async fn run(cmd: &str, label: &str, timeout: Duration) -> Result<Secret, CliError> {
    let fail = |why: String| CliError::Config(format!("PasswordCommand for {label} failed: {why}"));
    let mut command = shell(cmd);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| fail(format!("could not start the shell: {e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("no stdout".to_owned()))?;
    let work = async {
        let mut buf = Vec::new();
        let read = stdout.read_to_end(&mut buf).await;
        let status = child.wait().await;
        (buf, read, status)
    };
    let Ok((buf, read, status)) = tokio::time::timeout(timeout, work).await else {
        // Dropping `work` drops the child, which kill_on_drop then kills.
        return Err(fail(format!("timed out after {} s", timeout.as_secs())));
    };
    read.map_err(|e| fail(format!("could not read its output: {e}")))?;
    let status = status.map_err(|e| fail(format!("could not wait for it: {e}")))?;
    if !status.success() {
        return Err(fail(status.to_string()));
    }
    let mut text =
        String::from_utf8(buf).map_err(|_| fail("output is not valid UTF-8".to_owned()))?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    if text.is_empty() {
        return Err(fail("empty output".to_owned()));
    }
    Ok(Secret::new(text))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const LONG: Duration = Duration::from_secs(10);

    #[cfg(unix)]
    #[tokio::test]
    async fn captures_stdout_and_strips_one_newline() {
        let s = run("printf 'pw\\n\\n'", "t", LONG).await.unwrap();
        assert_eq!(s.expose(), "pw\n");
        let s = run("printf 'pw'", "t", LONG).await.unwrap();
        assert_eq!(s.expose(), "pw");
        let s = run("printf 'pw\\r\\n'", "t", LONG).await.unwrap();
        assert_eq!(s.expose(), "pw");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failures_never_contain_the_output() {
        let err = run("echo topsecret; exit 3", "host1", LONG)
            .await
            .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("host1") && text.contains('3'), "{text}");
        assert!(!text.contains("topsecret"), "{text}");

        let err = run("true", "host1", LONG).await.unwrap_err().to_string();
        assert!(err.contains("empty output"), "{err}");
        let err = run("printf '\\n'", "host1", LONG)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty output"), "{err}");
        let err = run("printf '\\377\\376'", "host1", LONG)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_and_reports() {
        let started = std::time::Instant::now();
        let err = run(
            "echo topsecret; exec sleep 30",
            "host1",
            Duration::from_millis(300),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("timed out"), "{err}");
        assert!(!err.contains("topsecret"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdin_is_closed() {
        // `cat` would block forever on an open terminal stdin; with /dev/null
        // it ends at once with no output.
        let err = run("cat", "t", LONG).await.unwrap_err().to_string();
        assert!(err.contains("empty output"), "{err}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cmd_echo() {
        let s = run("echo pw", "t", LONG).await.unwrap();
        assert_eq!(s.expose().trim_end(), "pw");
    }
}
