//! Offline real-binary proof of the recording commands: the compiled
//! `rdpilot` CLI against the compiled `rdpilot-daemon` with the fake
//! connector and synthetic frames. Each test gets its own runtime,
//! config, hosts file and recordings directories.
//!
//! Requires `cargo build --workspace` first (the daemon binary is located
//! next to the CLI binary, as in production).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

struct Env {
    root: PathBuf,
    xdg: PathBuf,
    capture: PathBuf,
    calls: usize,
    extra_env: Vec<(String, String)>,
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Env {
    fn new(tag: &str, config: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "rdpilot-cli-rec-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let xdg = root.join("xdg-runtime");
        let capture = root.join("capture");
        let config_dir = root.join("config").join("rdpilot");
        for dir in [&xdg, &capture, &config_dir] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(config_dir.join("config.toml"), config).unwrap();
        let hosts = root.join("hosts");
        std::fs::write(
            &hosts,
            "Host lab-a\n  HostName 10.0.0.5\n  User u\n  PasswordCommand \"printf pw\"\n\
             Host lab-b\n  HostName 10.0.0.5\n  User u\n  PasswordCommand \"printf pw\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&hosts, std::fs::Permissions::from_mode(0o600)).unwrap();
        Env {
            root,
            xdg,
            capture,
            calls: 0,
            extra_env: Vec::new(),
        }
    }

    fn rec_dir(&self) -> PathBuf {
        self.root.join("rec")
    }

    fn hosts(&self) -> String {
        self.root.join("hosts").to_string_lossy().into_owned()
    }

    fn bin() -> PathBuf {
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_rdpilot"));
        assert!(
            bin.with_file_name("rdpilot-daemon").exists(),
            "run `cargo build --workspace` first: the rdpilot-daemon binary must sit next to rdpilot"
        );
        bin
    }

    fn run(&mut self, args: &[&str]) -> Run {
        self.calls += 1;
        let out = self.capture.join(format!("call-{}-stdout.log", self.calls));
        let err = self.capture.join(format!("call-{}-stderr.log", self.calls));
        let mut command = Command::new(Self::bin());
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.xdg)
            .env("RDPILOT_DAEMON_SINK_PATH", self.root.join("sessions.json"))
            .env("RDPILOT_DAEMON_TEST_CONNECTOR", "1")
            .env("RDPILOT_DAEMON_TEST_FRAMES", "1")
            .env("RDPILOT_DAEMON_EMPTY_GRACE_MS", "300")
            .env("RDPILOT_DAEMON_REAP_INTERVAL_MS", "50")
            .env("RDPILOT_RECORDING__DIR", self.rec_dir())
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("HOME", &self.root)
            .env_remove("RDPILOT_RECORDING__ENABLED")
            .env_remove("RDPILOT_RECORDING__MAX_FPS")
            .env_remove("RDPILOT_RECORDING__BUDGET_MIB");
        for (k, v) in &self.extra_env {
            command.env(k, v);
        }
        let status = command
            .stdout(Stdio::from(std::fs::File::create(&out).unwrap()))
            .stderr(Stdio::from(std::fs::File::create(&err).unwrap()))
            .status()
            .unwrap();
        Run {
            status,
            stdout: std::fs::read_to_string(&out).unwrap_or_default(),
            stderr: std::fs::read_to_string(&err).unwrap_or_default(),
        }
    }

    fn ok(&mut self, args: &[&str]) -> String {
        let run = self.run(args);
        assert!(
            run.status.success(),
            "{args:?}: {}\n{}",
            run.stdout,
            run.stderr
        );
        run.stdout
    }

    fn connect(&mut self, name: &str, target: &str, flags: &[&str]) -> String {
        let hosts = self.hosts();
        let mut args = vec!["connect", target, "-F", &hosts, "--name", name];
        args.extend_from_slice(flags);
        self.ok(&args)
    }

    fn recordings(&mut self) -> serde_json::Value {
        serde_json::from_str(&self.ok(&["recording", "list", "--json"])).unwrap()
    }

    fn all_output(&self) -> String {
        let mut all = String::new();
        for entry in std::fs::read_dir(&self.capture).unwrap() {
            all.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default());
        }
        all
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Close every session so the daemon exits after its grace period.
        let _ = self.run(&["disconnect", "--session", "a"]);
        for name in ["b", "c", "d", "e", "f"] {
            let _ = self.run(&["disconnect", "--session", name]);
        }
        std::thread::sleep(std::time::Duration::from_millis(800));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn recording_id(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("recording: on ("))
        .map(|rest| rest.trim_end_matches(')').to_owned())
}

fn dir_names(path: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The switch resolves flag > per-host entry (alias or URL host, as typed)
/// > global, end to end.
#[test]
fn the_recording_switch_resolves_flag_host_and_global() {
    let mut env = Env::new(
        "switch",
        "[[recording.hosts]]\nhost = \"lab-a\"\nenabled = true\n\
         [[recording.hosts]]\nhost = \"10.9.9.9\"\nenabled = true\n",
    );
    let a = env.connect("a", "lab-a", &[]);
    assert!(recording_id(&a).is_some(), "per-host alias: {a}");
    let b = env.connect("b", "lab-b", &[]);
    assert!(recording_id(&b).is_none(), "other alias, same address: {b}");
    let c = env.connect("c", "rdp://u:p@10.9.9.9", &[]);
    assert!(recording_id(&c).is_some(), "per-host URL host: {c}");
    let d = env.connect("d", "lab-a", &["--no-record"]);
    assert!(
        recording_id(&d).is_none(),
        "--no-record beats the host entry"
    );
    let e = env.connect("e", "lab-b", &["--record"]);
    assert!(recording_id(&e).is_some(), "--record");
    env.extra_env
        .push(("RDPILOT_RECORDING__ENABLED".into(), "true".into()));
    let hosts = env.hosts();
    let f = env.ok(&["connect", "lab-b", "-F", &hosts, "--name", "f", "--json"]);
    let f: serde_json::Value = serde_json::from_str(&f).unwrap();
    assert_eq!(
        f["recording"]["state"], "on",
        "global switch from the environment"
    );
    assert_eq!(dir_names(&env.rec_dir()).len(), 4);
    let list: Vec<serde_json::Value> = serde_json::from_str(&env.ok(&["list", "--json"])).unwrap();
    let recording = |name: &str| {
        list.iter()
            .find(|s| s["id"] == name)
            .map(|s| s["recording"].clone())
            .unwrap()
    };
    assert!(recording("a").is_string());
    assert!(recording("b").is_null());
    let table = env.ok(&["list"]);
    assert!(table.lines().next().unwrap().contains("recording"));
    let rec = recording_id(&a).unwrap();
    assert!(table.contains(&rec));
    // The connect password never reaches a recording or any output.
    let output = env.all_output();
    assert!(!output.contains("\npw\n"));
}

#[test]
fn record_annotate_and_keep_commands() {
    let mut env = Env::new("verbs", "");
    let out = env.connect("a", "lab-a", &[]);
    assert!(recording_id(&out).is_none());
    // Not recording: annotate fails clearly and writes nothing.
    let refused = env.run(&["annotate", "--session", "a", "too early"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        refused.stderr.contains("not recording"),
        "{}",
        refused.stderr
    );
    let started = env.ok(&["record", "start", "--session", "a"]);
    assert!(started.starts_with("recording started ("), "{started}");
    let again = env.ok(&["record", "start", "--session", "a"]);
    assert!(again.contains("already recording"), "{again}");
    env.ok(&["annotate", "--session", "a", "first note"]);
    let long = "x".repeat(4097);
    let over = env.run(&["annotate", "--session", "a", &long]);
    assert_eq!(over.status.code(), Some(1));
    assert!(over.stderr.contains("4096"), "{}", over.stderr);
    let json = env.ok(&["record", "stop", "--session", "a", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["changed"], true);
    let id = json["id"].as_str().unwrap().to_owned();
    let redundant = env.ok(&["record", "stop", "--session", "a"]);
    assert!(redundant.contains("not recording"), "{redundant}");
    let unknown = env.run(&["record", "start", "--session", "nobody"]);
    assert_eq!(unknown.status.code(), Some(2));

    std::thread::sleep(std::time::Duration::from_millis(500));
    let listed = env.recordings();
    assert_eq!(listed["recordings"][0]["id"], id.as_str());
    assert_eq!(listed["recordings"][0]["kept"], false);
    assert!(listed["unkept_bytes"].as_u64().unwrap() > 0);
    assert_eq!(listed["budget_bytes"], 2048_u64 * 1024 * 1024);
    env.ok(&["recording", "keep", &id]);
    let kept = env.recordings();
    assert_eq!(kept["recordings"][0]["kept"], true);
    assert_eq!(kept["unkept_bytes"], 0);
    let table = env.ok(&["recording", "list"]);
    let header = table.lines().next().unwrap();
    for column in [
        "id", "session", "host", "started", "duration", "size", "active", "kept",
    ] {
        assert!(header.contains(column), "{header}");
    }
    assert!(table.contains("kept: "), "{table}");
    env.ok(&["recording", "unkeep", &id]);
    assert_eq!(env.recordings()["recordings"][0]["kept"], false);
    let bad = env.run(&["recording", "keep", "20260101T000000Z-ffffffff"]);
    assert_eq!(bad.status.code(), Some(1));

    let events = std::fs::read_to_string(env.rec_dir().join(&id).join("events.jsonl")).unwrap();
    assert!(events.contains("first note"));
    assert!(!events.contains("too early"));
    assert!(!events.contains(&long));
}
