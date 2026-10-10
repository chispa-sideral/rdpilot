//! [`RecordingConfig`] — the `[recording]` table: whether sessions are
//! recorded, how often frames are taken, where recordings are stored and
//! how much space unkept recordings may use. Resolved from the same file ->
//! env layers as [`crate::ResolvedConfig`]. It holds no credential.
//!
//! Two processes read it:
//! - the CLI resolves the switch for `connect` ([`RecordingConfig::switch_for`]:
//!   `--record`/`--no-record`, then the per-host list, then `enabled`);
//! - the daemon reads `dir`, `max_fps` and `budget_mib` when it starts and
//!   whenever a recording starts.

use std::path::PathBuf;

use config::Config;
use directories::BaseDirs;
use rdpilot_vocab::RecordingTrigger;
use serde::Deserialize;

use crate::resolved::ConfigError;

/// Default frame cap (frames per second).
pub const DEFAULT_MAX_FPS: f64 = 4.0;
/// Lowest accepted `max_fps`.
pub const MIN_MAX_FPS: f64 = 0.5;
/// Highest accepted `max_fps`.
pub const MAX_MAX_FPS: f64 = 8.0;
/// Default space budget for unkept recordings, in MiB (2 GiB).
pub const DEFAULT_BUDGET_MIB: u64 = 2048;

/// One `[[recording.hosts]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HostRecording {
    /// The target as given to `rdpilot connect`: a hosts-file alias as typed,
    /// or the host part of an `rdp://` URL. Matched ASCII case-insensitively.
    pub host: String,
    /// Record sessions to this host (`true`) or not (`false`).
    pub enabled: bool,
}

/// The resolved `[recording]` settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecordingConfig {
    /// Record every session unless a host entry or a connect flag says
    /// otherwise. Default: off.
    #[serde(default)]
    pub enabled: bool,
    /// Most frames per second written to video. Default 4, valid 0.5 to 8.
    #[serde(default = "default_max_fps")]
    pub max_fps: f64,
    /// Where recordings are stored. Default: `<local data dir>/rdpilot/recordings`.
    #[serde(default)]
    pub dir: Option<String>,
    /// Space for unkept recordings, in MiB; also the size cap of a single
    /// recording and the threshold of the kept-over-budget warning.
    #[serde(default = "default_budget_mib")]
    pub budget_mib: u64,
    /// Per-host switches; the first matching entry wins.
    #[serde(default)]
    pub hosts: Vec<HostRecording>,
}

fn default_max_fps() -> f64 {
    DEFAULT_MAX_FPS
}

fn default_budget_mib() -> u64 {
    DEFAULT_BUDGET_MIB
}

impl Default for RecordingConfig {
    fn default() -> Self {
        RecordingConfig {
            enabled: false,
            max_fps: DEFAULT_MAX_FPS,
            dir: None,
            budget_mib: DEFAULT_BUDGET_MIB,
            hosts: Vec::new(),
        }
    }
}

impl RecordingConfig {
    /// Whether a session connected to `target` records from connect, and why.
    ///
    /// `flag` is `Some(true)` for `--record`, `Some(false)` for `--no-record`.
    /// The most specific setting wins: the flag, then the first
    /// `[[recording.hosts]]` entry whose `host` equals `target` (ASCII
    /// case-insensitive), then `enabled`. `None` means off.
    #[must_use]
    pub fn switch_for(&self, target: &str, flag: Option<bool>) -> Option<RecordingTrigger> {
        if let Some(on) = flag {
            return on.then_some(RecordingTrigger::ConnectFlag);
        }
        if let Some(entry) = self
            .hosts
            .iter()
            .find(|h| h.host.eq_ignore_ascii_case(target))
        {
            return entry.enabled.then_some(RecordingTrigger::Host);
        }
        self.enabled.then_some(RecordingTrigger::Config)
    }

    /// The configured directory, or `<local data dir>/rdpilot/recordings`.
    ///
    /// The local (non-roaming) data directory keeps recordings out of
    /// Windows roaming profiles; on Linux it is `.local/share` in the home
    /// directory.
    #[must_use]
    pub fn dir_or_default(&self) -> PathBuf {
        if let Some(dir) = &self.dir {
            return PathBuf::from(dir);
        }
        let mut root = BaseDirs::new()
            .map(|b| b.data_local_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        root.push("rdpilot");
        root.push("recordings");
        root
    }

    /// The budget in bytes.
    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.budget_mib.saturating_mul(1024 * 1024)
    }

    fn validate(self) -> Result<Self, ConfigError> {
        if !(MIN_MAX_FPS..=MAX_MAX_FPS).contains(&self.max_fps) {
            return Err(ConfigError::file(format!(
                "recording.max_fps must be between {MIN_MAX_FPS} and {MAX_MAX_FPS}, got {}",
                self.max_fps
            )));
        }
        if self.budget_mib == 0 {
            return Err(ConfigError::file("recording.budget_mib must be at least 1"));
        }
        if self.hosts.iter().any(|h| h.host.is_empty()) {
            return Err(ConfigError::file(
                "recording.hosts entries need a non-empty host",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RecordingLayer {
    #[serde(default)]
    recording: Option<RecordingConfig>,
}

fn deserialize_recording(built: Config) -> Result<RecordingConfig, ConfigError> {
    built
        .try_deserialize::<RecordingLayer>()
        .map_err(|e| ConfigError::file(e.to_string()))?
        .recording
        .unwrap_or_default()
        .validate()
}

/// Resolve `[recording]` from the platform config file and
/// `RDPILOT_RECORDING__*` environment variables (env wins over file).
///
/// # Errors
///
/// Returns [`ConfigError::File`] if the file cannot be parsed or a value is
/// invalid.
pub fn resolve_recording() -> Result<RecordingConfig, ConfigError> {
    let built = crate::resolve::real_file_and_env_builder()
        .build()
        .map_err(|e| ConfigError::file(e.to_string()))?;
    deserialize_recording(built)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use config::{Environment, File, FileFormat};

    use super::*;
    use crate::hosts::Target;

    fn env(pairs: &[(&str, &str)]) -> Environment {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Environment::with_prefix("RDPILOT")
            .prefix_separator("_")
            .separator("__")
            .source(Some(map))
    }

    fn from_file(toml: &str) -> Result<RecordingConfig, ConfigError> {
        deserialize_recording(
            Config::builder()
                .add_source(File::from_str(toml, FileFormat::Toml))
                .build()
                .map_err(|e| ConfigError::file(e.to_string()))?,
        )
    }

    #[test]
    fn defaults_are_off_4_fps_2_gib_under_the_local_data_dir(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cfg = deserialize_recording(Config::builder().add_source(env(&[])).build()?)?;
        assert_eq!(cfg, RecordingConfig::default());
        assert!(!cfg.enabled);
        assert!((cfg.max_fps - 4.0).abs() < f64::EPSILON);
        assert_eq!(cfg.budget_mib, 2048);
        assert_eq!(cfg.budget_bytes(), 2 * 1024 * 1024 * 1024);
        let dir = cfg.dir_or_default();
        assert!(dir.ends_with(PathBuf::from("rdpilot").join("recordings")));
        if let Some(base) = BaseDirs::new() {
            assert!(dir.starts_with(base.data_local_dir()));
        }
        Ok(())
    }

    #[test]
    fn file_then_env_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let file = File::from_str(
            "[recording]\nenabled = true\nmax_fps = 2.5\ndir = \"/rec\"\nbudget_mib = 10\n\
             [[recording.hosts]]\nhost = \"lab-vm\"\nenabled = false\n",
            FileFormat::Toml,
        );
        let from_file = deserialize_recording(Config::builder().add_source(file.clone()).build()?)?;
        assert!(from_file.enabled);
        assert!((from_file.max_fps - 2.5).abs() < f64::EPSILON);
        assert_eq!(from_file.dir_or_default(), PathBuf::from("/rec"));
        assert_eq!(from_file.budget_bytes(), 10 * 1024 * 1024);
        assert_eq!(
            from_file.hosts,
            vec![HostRecording {
                host: "lab-vm".into(),
                enabled: false
            }]
        );

        let with_env = deserialize_recording(
            Config::builder()
                .add_source(file)
                .add_source(env(&[
                    ("RDPILOT_RECORDING__ENABLED", "false"),
                    ("RDPILOT_RECORDING__MAX_FPS", "8"),
                    ("RDPILOT_RECORDING__DIR", "/env-rec"),
                    ("RDPILOT_RECORDING__BUDGET_MIB", "1"),
                ]))
                .build()?,
        )?;
        assert!(!with_env.enabled);
        assert!((with_env.max_fps - 8.0).abs() < f64::EPSILON);
        assert_eq!(with_env.dir_or_default(), PathBuf::from("/env-rec"));
        assert_eq!(with_env.budget_mib, 1);
        assert_eq!(with_env.hosts.len(), 1, "env keeps the file's host list");
        Ok(())
    }

    #[test]
    fn invalid_values_are_errors() {
        for toml in [
            "[recording]\nmax_fps = 0.4",
            "[recording]\nmax_fps = 8.5",
            "[recording]\nbudget_mib = 0",
            "[recording]\nenabled = \"sometimes\"",
            "[[recording.hosts]]\nhost = \"\"\nenabled = true",
            "[[recording.hosts]]\nenabled = true",
        ] {
            assert!(from_file(toml).is_err(), "{toml} must be rejected");
        }
        assert!(from_file("[recording]\nmax_fps = 0.5").is_ok());
        assert!(from_file("[recording]\nmax_fps = 8").is_ok());
    }

    /// A file without `[recording]`, or without a host list, still loads,
    /// and a file with `[recording]` leaves the other settings unchanged.
    #[test]
    fn recording_table_is_optional_and_does_not_disturb_other_settings(
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            from_file("share_root = \"a\"\n[viewer]\nbind = \"loopback\"")?,
            RecordingConfig::default()
        );
        assert!(from_file("[recording]\nenabled = true")?.hosts.is_empty());

        let with = Config::builder()
            .add_source(File::from_str(
                "share_root = \"a\"\n[viewer]\nbind = \"loopback\"\n[recording]\nenabled = true\n\
                 [[recording.hosts]]\nhost = \"vm.example.com\"\nenabled = true",
                FileFormat::Toml,
            ))
            .build()?;
        let resolved: crate::ResolvedConfig = with.clone().try_deserialize()?;
        assert_eq!(resolved.share_root.as_deref(), Some("a"));
        let viewer = crate::viewer::deserialize_viewer(with)?;
        assert_eq!(viewer.bind, crate::ViewerBind::Loopback);
        Ok(())
    }

    #[test]
    fn dotted_host_names_are_kept_whole() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = from_file(
            "[[recording.hosts]]\nhost = \"vm.example.com\"\nenabled = true\n\
             [[recording.hosts]]\nhost = \"10.0.0.5\"\nenabled = true",
        )?;
        assert_eq!(cfg.hosts[0].host, "vm.example.com");
        assert_eq!(
            cfg.switch_for("vm.example.com", None),
            Some(RecordingTrigger::Host)
        );
        assert_eq!(
            cfg.switch_for("10.0.0.5", None),
            Some(RecordingTrigger::Host)
        );
        assert_eq!(cfg.switch_for("vm", None), None);
        Ok(())
    }

    #[test]
    fn the_most_specific_setting_wins() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = from_file(
            "[recording]\nenabled = true\n\
             [[recording.hosts]]\nhost = \"quiet\"\nenabled = false\n\
             [[recording.hosts]]\nhost = \"loud\"\nenabled = true",
        )?;
        assert_eq!(
            cfg.switch_for("other", None),
            Some(RecordingTrigger::Config)
        );
        assert_eq!(cfg.switch_for("quiet", None), None);
        assert_eq!(cfg.switch_for("loud", None), Some(RecordingTrigger::Host));
        assert_eq!(
            cfg.switch_for("quiet", Some(true)),
            Some(RecordingTrigger::ConnectFlag)
        );
        assert_eq!(cfg.switch_for("loud", Some(false)), None);
        assert_eq!(cfg.switch_for("other", Some(false)), None);

        let off = RecordingConfig::default();
        assert_eq!(off.switch_for("any", None), None);
        assert_eq!(
            off.switch_for("any", Some(true)),
            Some(RecordingTrigger::ConnectFlag)
        );
        Ok(())
    }

    #[test]
    fn host_match_is_case_insensitive_and_the_first_entry_wins(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cfg = from_file(
            "[[recording.hosts]]\nhost = \"Lab-VM\"\nenabled = true\n\
             [[recording.hosts]]\nhost = \"lab-vm\"\nenabled = false",
        )?;
        assert_eq!(cfg.switch_for("LAB-vm", None), Some(RecordingTrigger::Host));
        Ok(())
    }

    /// The key is the target as given to `connect`: an alias as typed, or the
    /// host part of an `rdp://` URL (never the user, password or port).
    #[test]
    fn alias_and_url_targets_match_by_their_name() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = from_file(
            "[[recording.hosts]]\nhost = \"lab-vm\"\nenabled = true\n\
             [[recording.hosts]]\nhost = \"10.1.2.3\"\nenabled = true",
        )?;
        let alias = Target::parse("lab-vm")?;
        let url = Target::parse("rdp://user:pw@10.1.2.3:3390")?;
        let other = Target::parse("rdps://lab-vm.example.com")?;
        assert_eq!(
            cfg.switch_for(alias.name(), None),
            Some(RecordingTrigger::Host)
        );
        assert_eq!(
            cfg.switch_for(url.name(), None),
            Some(RecordingTrigger::Host)
        );
        assert_eq!(cfg.switch_for(other.name(), None), None);
        Ok(())
    }
}
