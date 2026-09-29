//! `PasswordCommand` expansion: `%h %n %r %p %%` and `${VAR}`.
//!
//! The command runs through a shell, so a value substituted for `%h`, `%n`,
//! `%r` or `%p` is validated first (the check OpenSSH 9.6 added after
//! CVE-2023-51385): it must be non-empty, must not start with `-`, and must
//! not contain whitespace, control characters or shell metacharacters. The
//! check applies to every substituted value whatever its source, so
//! `HostName %h` cannot launder a command-line alias into a file value.
//! `${VAR}` values come from the user's own environment and are not checked.

use super::{HostsError, ResolvedHost};

/// The RDP port used for `%p` when no `Port` is set.
pub const DEFAULT_PORT: u16 = 3389;

const FORBIDDEN: &str = "'\"`\\$;&|<>(){}[]*?!#~%^=";

fn check_token(token: char, value: &str, target: &str) -> Result<(), HostsError> {
    let problem = if value.is_empty() {
        "is empty"
    } else if value.starts_with('-') {
        "starts with '-'"
    } else if value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || FORBIDDEN.contains(c))
    {
        "contains a character that is not allowed in a shell command"
    } else {
        return Ok(());
    };
    Err(HostsError::new(format!(
        "PasswordCommand for {target}: %{token} value {problem}"
    )))
}

/// Expand the winning `PasswordCommand`, ready to hand to a shell.
///
/// # Errors
///
/// No `PasswordCommand` won, an unknown `%x`, an unset `${VAR}`, or a
/// substituted value that fails validation.
pub fn expand_password_command(
    host: &ResolvedHost,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<String, HostsError> {
    let label = host.target().to_string();
    let err = |msg: String| HostsError::new(format!("PasswordCommand for {label}: {msg}"));
    let command = host
        .password_command()
        .ok_or_else(|| err("no PasswordCommand is configured".to_owned()))?;
    let mut out = String::new();
    let mut rest = command;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("${") {
            let end = after
                .find('}')
                .ok_or_else(|| err("unterminated ${".to_owned()))?;
            let name = &after[..end];
            let value =
                env(name).ok_or_else(|| err(format!("environment variable {name} is not set")))?;
            out.push_str(&value);
            rest = &after[end + 1..];
            continue;
        }
        if c != '%' {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let mut it = rest[1..].chars();
        let Some(t) = it.next() else {
            return Err(err("trailing % (write %% for a literal %)".to_owned()));
        };
        let value = match t {
            '%' => "%".to_owned(),
            'h' => host.address().to_owned(),
            'n' => host.target().name().to_owned(),
            'r' => host
                .user()
                .ok_or_else(|| err("%r used but no User is set".to_owned()))?
                .to_owned(),
            'p' => host.port().unwrap_or(DEFAULT_PORT).to_string(),
            other => return Err(err(format!("unknown token %{other}"))),
        };
        if t != '%' {
            check_token(t, &value, &label)?;
        }
        out.push_str(&value);
        rest = &rest[1 + t.len_utf8()..];
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::hosts::{resolve_host, HostsInput};

    fn expand(target: &str, hosts: &str, opts: &[&str]) -> Result<String, HostsError> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hosts"), hosts).unwrap();
        let opts: Vec<String> = opts.iter().map(|s| (*s).to_owned()).collect();
        let h = resolve_host(&HostsInput {
            target,
            options: &opts,
            file: None,
            config_dir: Some(dir.path()),
            home: None,
        })?;
        let env = |k: &str| (k == "PW_VAR").then(|| "envval".to_owned());
        expand_password_command(&h, &env)
    }

    const CMD: &str = "PasswordCommand=vault get %h %n %r %p";

    #[test]
    fn tokens() {
        let hosts = "Host web1\n  HostName 10.0.0.5\n  User alice\n  Port 3390\n";
        let out = expand(
            "web1",
            hosts,
            &["PasswordCommand=vault get %h %n %r %p 100%%"],
        )
        .unwrap();
        assert_eq!(out, "vault get 10.0.0.5 web1 alice 3390 100%");
        let out = expand("web1", "User u\n", &["PasswordCommand=echo %p"]).unwrap();
        assert_eq!(out, "echo 3389");
    }

    #[test]
    fn env_vars() {
        assert_eq!(
            expand("w", "", &["PasswordCommand=echo ${PW_VAR}"]).unwrap(),
            "echo envval"
        );
        let err = expand("w", "", &["PasswordCommand=echo ${NOPE}"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("NOPE is not set"), "{err}");
        assert!(expand("w", "", &["PasswordCommand=echo ${open"]).is_err());
        // A plain shell $ is left for the shell; values are not re-scanned.
        assert_eq!(
            expand("w", "", &["PasswordCommand=echo $HOME"]).unwrap(),
            "echo $HOME"
        );
    }

    #[test]
    fn unknown_token_and_missing_user() {
        assert!(expand("w", "", &["PasswordCommand=echo %x"])
            .unwrap_err()
            .to_string()
            .contains("unknown token %x"));
        assert!(expand("w", "", &["PasswordCommand=echo %r"])
            .unwrap_err()
            .to_string()
            .contains("no User"));
        assert!(expand("w", "", &["PasswordCommand=echo %"]).is_err());
    }

    #[test]
    fn command_line_value_in_url_is_validated() {
        // A URL user with a shell metacharacter, percent-encoded.
        let err = expand("rdp://a%3Brm@host", "", &[CMD])
            .unwrap_err()
            .to_string();
        assert!(err.contains("%r value contains a character"), "{err}");
        // Untouched when %r is not used.
        assert!(expand("rdp://a%3Brm@host", "", &["PasswordCommand=echo %p"]).is_ok());
    }

    #[test]
    fn file_value_is_validated() {
        let hosts = "Host web\n  User \"a b\"\n";
        let err = expand("web", hosts, &["PasswordCommand=echo %r"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("%r value contains a character"), "{err}");
        let hosts = "Host web\n  User -x\n";
        assert!(expand("web", hosts, &["PasswordCommand=echo %r"])
            .unwrap_err()
            .to_string()
            .contains("starts with '-'"));
        let hosts = "Host web\n  HostName 'q\n";
        assert!(expand("web", hosts, &["PasswordCommand=echo %h"]).is_err());
    }

    #[test]
    fn host_name_percent_h_cannot_launder_a_command_line_alias() {
        let hosts = "Host *\n  HostName %h\n";
        for alias in [
            "a;touch-x",
            "a$(id)",
            "a`id`",
            "a&b",
            "a|b",
            "a b",
            "-oProxy",
        ] {
            let err = expand(alias, hosts, &["PasswordCommand=echo %h"])
                .unwrap_err()
                .to_string();
            assert!(err.contains("%h value"), "{alias}: {err}");
            let err = expand(alias, hosts, &["PasswordCommand=echo %n"])
                .unwrap_err()
                .to_string();
            assert!(err.contains("%n value"), "{alias}: {err}");
        }
    }

    #[test]
    fn every_forbidden_character_is_rejected_and_ordinary_values_pass() {
        for c in FORBIDDEN.chars() {
            let hosts = format!("Host w\n  User \"a{c}b\"\n");
            assert!(
                expand("w", &hosts, &["PasswordCommand=x %r"]).is_err(),
                "{c:?}"
            );
        }
        for ok in ["alice", "a.b-c_d", "user@corp", "a+b", "a,b", "a/b", "a:b"] {
            let hosts = format!("Host w\n  User {ok}\n");
            assert!(
                expand("w", &hosts, &["PasswordCommand=x %r"]).is_ok(),
                "{ok}"
            );
        }
    }

    #[test]
    fn ipv6_literal_keeps_colons_and_loses_brackets() {
        let out = expand("rdp://u@[fe80::1]", "", &["PasswordCommand=x %h %n"]).unwrap();
        assert_eq!(out, "x fe80::1 fe80::1");
    }

    #[test]
    fn url_percent_n_is_the_hostname_never_the_url() {
        let out = expand("rdp://u@Host.Example", "", &["PasswordCommand=x %n"]).unwrap();
        assert_eq!(out, "x Host.Example");
    }

    #[test]
    fn errors_name_target_without_password() {
        let err = expand("rdp://u:hunter2@ho st", "", &["PasswordCommand=x"]);
        assert!(!format!("{err:?}").contains("hunter2"));
        let err = expand("rdp://u:hunter2@h", "", &["PasswordCommand=x %z"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("u:***@h") && !err.contains("hunter2"), "{err}");
    }
}
