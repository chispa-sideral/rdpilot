//! Host configuration through the real `rdpilot` binary: hosts files, `-o`,
//! `-F`, rdp:// targets, `config resolve` and `PasswordCommand`.
//!
//! Every invocation runs with `HOME`, `XDG_CONFIG_HOME` and `APPDATA` in a
//! per-test temp dir, so a developer's own hosts file never leaks in. Output
//! goes to files, not pipes (the auto-started daemon inherits stdio).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

struct Sandbox {
    root: PathBuf,
    calls: usize,
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "rdpilot-cli-hosts-{label}-{}-{nanos}",
            std::process::id()
        ));
        for dir in ["xdg", "capture", "home"] {
            std::fs::create_dir_all(root.join(dir)).expect("create sandbox dir");
        }
        Self { root, calls: 0 }
    }

    fn sink(&self) -> PathBuf {
        self.root.join("sessions.json")
    }

    /// The auto-started daemon's diagnostics file, so secret scans cover it.
    fn diagnostics(&self) -> PathBuf {
        self.root.join("diag/diagnostics.json")
    }

    /// Write a hosts file (mode 0600) and return its path.
    fn hosts(&self, text: &str) -> PathBuf {
        let path = self.root.join("hosts");
        std::fs::write(&path, text).expect("write hosts file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("chmod hosts file");
        }
        path
    }

    fn run(&mut self, args: &[&str], extra_env: &[(&str, &str)]) -> Run {
        self.calls += 1;
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
        let out = self
            .root
            .join(format!("capture/call-{}-out.log", self.calls));
        let err = self
            .root
            .join(format!("capture/call-{}-err.log", self.calls));
        let mut command = Command::new(&bin);
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", self.root.join("xdg"))
            .env("XDG_CONFIG_HOME", self.root.join("home/.config"))
            .env("HOME", self.root.join("home"))
            .env("APPDATA", self.root.join("home/appdata"))
            .env("RDPILOT_DAEMON_SINK_PATH", self.sink())
            .env("RDPILOT_DAEMON_DIAGNOSTICS_PATH", self.diagnostics())
            .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
            .env("RDPILOT_DAEMON_IDLE_TIMEOUT_MS", "30000")
            .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "1500")
            .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50")
            .stdout(Stdio::from(
                std::fs::File::create(&out).expect("stdout file"),
            ))
            .stderr(Stdio::from(
                std::fs::File::create(&err).expect("stderr file"),
            ));
        for (k, v) in extra_env {
            command.env(k, v);
        }
        let status = command.status().expect("spawn rdpilot");
        Run {
            status,
            stdout: std::fs::read_to_string(&out).unwrap_or_default(),
            stderr: std::fs::read_to_string(&err).unwrap_or_default(),
        }
    }

    /// Every captured output plus the daemon's sink and diagnostics files.
    fn everything(&self) -> String {
        let mut all = String::new();
        if let Ok(rd) = std::fs::read_dir(self.root.join("capture")) {
            for e in rd.flatten() {
                all.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
            }
        }
        all.push_str(&std::fs::read_to_string(self.sink()).unwrap_or_default());
        all.push_str(&std::fs::read_to_string(self.diagnostics()).unwrap_or_default());
        all
    }
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("utf-8 temp path")
}

#[test]
fn removed_connection_flags_are_rejected_before_anything_runs() {
    let mut sb = Sandbox::new("removed-flags");
    for flags in [
        &["--host", "h"][..],
        &["--port", "1"],
        &["--username", "u"],
        &["--password", "p"],
        &["--domain", "d"],
        &["--accept-invalid-certs"],
    ] {
        let mut args = vec!["connect", "web"];
        args.extend_from_slice(flags);
        let run = sb.run(&args, &[]);
        assert_eq!(run.status.code(), Some(2), "{flags:?}: {}", run.stderr);
    }
    assert!(!sb.root.join("xdg/rdpilot/daemon.sock").exists());
}

#[test]
fn config_resolve_shows_sources_redacts_and_never_runs_the_command() {
    let mut sb = Sandbox::new("resolve");
    let marker = sb.root.join("ran-marker");
    let hosts = sb.hosts(&format!(
        "Host lab\n  HostName 10.0.0.9\n  User alice\n  PasswordCommand \"touch {} && echo x\"\n",
        path_str(&marker)
    ));
    let run = sb.run(
        &[
            "config",
            "resolve",
            "lab",
            "-F",
            path_str(&hosts),
            "-o",
            "Port=3390",
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let out = run.stdout;
    assert!(
        out.contains("HostName") && out.contains("10.0.0.9"),
        "{out}"
    );
    assert!(out.contains("hosts:2"), "{out}");
    assert!(out.contains("3390") && out.contains("-o #1"), "{out}");
    assert!(out.contains("built-in default"), "{out}");
    assert!(
        out.contains("PasswordCommand") && out.contains("touch "),
        "{out}"
    );
    assert!(
        !marker.exists(),
        "config resolve must not run PasswordCommand"
    );
}

#[test]
fn config_resolve_redacts_url_and_option_passwords() {
    let mut sb = Sandbox::new("redact");
    let hosts = sb.hosts("");
    let run = sb.run(
        &[
            "config",
            "resolve",
            "rdp://u:hunter2@h",
            "-o",
            "Password=other",
            "-F",
            path_str(&hosts),
            "--json",
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    assert!(!sb.everything().contains("hunter2") && !sb.everything().contains("other"));
    let json: serde_json::Value = serde_json::from_str(run.stdout.trim()).expect("json");
    assert_eq!(json["target"], "rdp://u:***@h");
    let settings = json["settings"].as_array().expect("settings");
    let pw = settings
        .iter()
        .find(|s| s["keyword"] == "Password")
        .expect("Password row");
    assert_eq!(pw["value"], "<redacted>");
    assert_eq!(pw["source"], "url");
}

#[test]
fn a_missing_dash_f_file_is_an_error() {
    let mut sb = Sandbox::new("missing-f");
    let missing = sb.root.join("nope");
    let run = sb.run(&["config", "resolve", "web", "-F", path_str(&missing)], &[]);
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stderr.contains("configuration error"), "{}", run.stderr);
}

#[test]
fn old_environment_variables_change_nothing() {
    let mut sb = Sandbox::new("old-env");
    let hosts = sb.hosts("Host lab\n  HostName 10.0.0.9\n");
    let run = sb.run(
        &["config", "resolve", "lab", "-F", path_str(&hosts)],
        &[
            ("RDPILOT_HOST", "evil-host"),
            ("RDPILOT_PASSWORD", "evil-pw"),
            ("RDPILOT_USERNAME", "evil-user"),
        ],
    );
    assert!(run.status.success(), "{}", run.stderr);
    assert!(run.stdout.contains("10.0.0.9"));
    assert!(!run.stdout.contains("evil"), "{}", run.stdout);
}

#[test]
fn connect_through_a_hosts_file_alias_with_a_password_command() {
    let mut sb = Sandbox::new("connect-alias");
    let hosts = sb.hosts(
        "Host lab\n  HostName 10.0.0.5\n  User u\n  Port 3390\n  PasswordCommand \"printf pw\"\n",
    );
    let f = path_str(&hosts).to_owned();
    let run = sb.run(
        &["connect", "lab", "-F", &f, "--name", "lab", "--json"],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let list = sb.run(&["list", "--json"], &[]);
    let sessions: serde_json::Value = serde_json::from_str(list.stdout.trim()).expect("json");
    assert_eq!(sessions[0]["host"], "10.0.0.5");
    let dis = sb.run(&["disconnect", "--session", "lab"], &[]);
    assert!(dis.status.success(), "{}", dis.stderr);
}

#[test]
fn url_password_never_reaches_output_or_the_daemon_sink() {
    let mut sb = Sandbox::new("url-secret");
    let run = sb.run(
        &[
            "connect",
            "rdp://u:hunter2-url@10.0.0.6",
            "--name",
            "urlpw",
            "--json",
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let _ = sb.run(&["list", "--json"], &[]);
    let _ = sb.run(&["disconnect", "--session", "urlpw"], &[]);
    assert!(!sb.everything().contains("hunter2-url"));
}

#[test]
fn option_password_never_reaches_output_or_the_daemon_sink() {
    let mut sb = Sandbox::new("option-secret");
    let opt = "Password=hunter2-opt";
    let run = sb.run(
        &[
            "connect",
            "rdp://u@10.0.0.6",
            "-o",
            opt,
            "--name",
            "optpw",
            "--json",
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let _ = sb.run(&["list", "--json"], &[]);

    let dup = sb.run(
        &[
            "connect",
            "rdp://u@10.0.0.6",
            "-o",
            opt,
            "--name",
            "optpw",
            "--json",
        ],
        &[],
    );
    assert!(!dup.status.success(), "duplicate session name must fail");

    let hosts = sb.hosts("Host lab2\n  HostName 10.0.0.8\n  User u\n");
    let f = path_str(&hosts).to_owned();
    let run = sb.run(
        &[
            "connect", "lab2", "-F", &f, "-o", opt, "--name", "optpw2", "--json",
        ],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);

    let run = sb.run(
        &["connect", "rdp://10.0.0.9", "-o", opt, "--name", "optpw3"],
        &[],
    );
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stderr.contains("no User"), "{}", run.stderr);

    for name in ["optpw", "optpw2"] {
        let dis = sb.run(&["disconnect", "--session", name], &[]);
        assert!(dis.status.success(), "{}", dis.stderr);
    }
    assert!(!sb.everything().contains("hunter2-opt"));
}

#[test]
fn session_name_does_not_select_a_host_block() {
    let mut sb = Sandbox::new("name-alias");
    let hosts = sb.hosts(
        "Host lab\n  HostName 10.0.0.5\n  User u\n  PasswordCommand \"printf pw\"\n\
         Host decoy\n  HostName 10.9.9.9\n  User d\n  PasswordCommand \"printf pw\"\n",
    );
    let f = path_str(&hosts).to_owned();
    let run = sb.run(
        &["connect", "lab", "-F", &f, "--name", "decoy", "--json"],
        &[],
    );
    assert!(run.status.success(), "{}", run.stderr);
    let list = sb.run(&["list", "--json"], &[]);
    let sessions: serde_json::Value = serde_json::from_str(list.stdout.trim()).expect("json");
    assert_eq!(sessions[0]["host"], "10.0.0.5");
    let dis = sb.run(&["disconnect", "--session", "decoy"], &[]);
    assert!(dis.status.success(), "{}", dis.stderr);
}

#[test]
fn session_name_does_not_supply_settings_to_a_url_target() {
    let mut sb = Sandbox::new("name-url");
    let hosts = sb.hosts("Host decoy\n  User d\n  PasswordCommand \"printf pw\"\n");
    let run = sb.run(
        &[
            "connect",
            "rdp://10.0.0.7",
            "-F",
            path_str(&hosts),
            "--name",
            "decoy",
        ],
        &[],
    );
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stderr.contains("no User"), "{}", run.stderr);
}

#[test]
fn a_failing_password_command_is_a_config_error_without_its_output() {
    let mut sb = Sandbox::new("failing-cmd");
    let hosts = sb.hosts("Host lab\n  User u\n  PasswordCommand \"echo topsecret; exit 1\"\n");
    let run = sb.run(&["connect", "lab", "-F", path_str(&hosts)], &[]);
    assert_eq!(run.status.code(), Some(1));
    assert!(
        run.stderr.contains("PasswordCommand for lab failed"),
        "{}",
        run.stderr
    );
    assert!(!sb.everything().contains("topsecret"));
    assert!(!sb.root.join("xdg/rdpilot/daemon.sock").exists());
}

#[test]
fn a_missing_user_or_credential_is_a_missing_config_error() {
    let mut sb = Sandbox::new("missing");
    let hosts = sb.hosts("Host nouser\n  Password x\nHost nopw\n  User u\n");
    let f = path_str(&hosts).to_owned();
    let run = sb.run(&["connect", "nouser", "-F", &f], &[]);
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stderr.contains("no User"), "{}", run.stderr);
    let run = sb.run(&["connect", "nopw", "-F", &f], &[]);
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stderr.contains("no Password"), "{}", run.stderr);
}

#[cfg(unix)]
#[test]
fn a_group_readable_hosts_file_with_a_password_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let mut sb = Sandbox::new("perms");
    let hosts = sb.hosts("Host lab\n  User u\n  Password sekrit\n");
    std::fs::set_permissions(&hosts, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    let run = sb.run(&["connect", "lab", "-F", path_str(&hosts)], &[]);
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stderr.contains("chmod 600"), "{}", run.stderr);
    assert!(!sb.everything().contains("sekrit"));
}
