//! [`ViewerConfig`] — the `[viewer]` table for `rdpilot view`, resolved from
//! the same file -> env layers as [`crate::ResolvedConfig`]. It holds no
//! credential. The CLI applies its own flags on top.

use std::net::Ipv4Addr;

use config::Config;
use serde::Deserialize;

use crate::resolved::ConfigError;

/// Which addresses the live viewer binds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum ViewerBind {
    /// `127.0.0.1` only.
    #[serde(rename = "loopback")]
    Loopback,
    /// `127.0.0.1` plus this host's Tailscale address, when one is found.
    #[default]
    #[serde(rename = "loopback+tailnet")]
    LoopbackAndTailnet,
}

impl std::str::FromStr for ViewerBind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "loopback" => Ok(ViewerBind::Loopback),
            "loopback+tailnet" => Ok(ViewerBind::LoopbackAndTailnet),
            other => Err(format!(
                "invalid viewer bind \"{other}\" (expected \"loopback\" or \"loopback+tailnet\")"
            )),
        }
    }
}

/// The resolved `[viewer]` settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ViewerConfig {
    /// Bind set. Default: loopback plus tailnet.
    #[serde(default)]
    pub bind: ViewerBind,
    /// Explicit Tailscale IPv4 address (skips automatic detection).
    #[serde(default)]
    pub tailnet_address: Option<Ipv4Addr>,
}

#[derive(Debug, Default, Deserialize)]
struct ViewerLayer {
    #[serde(default)]
    viewer: ViewerConfig,
}

fn deserialize_viewer(built: Config) -> Result<ViewerConfig, ConfigError> {
    built
        .try_deserialize::<ViewerLayer>()
        .map(|layer| layer.viewer)
        .map_err(|e| ConfigError::file(e.to_string()))
}

/// Resolve `[viewer]` from the platform config file and `RDPILOT_VIEWER__*`
/// environment variables (env wins over file).
///
/// # Errors
///
/// Returns [`ConfigError::File`] if the file cannot be parsed or a value is
/// invalid.
pub fn resolve_viewer() -> Result<ViewerConfig, ConfigError> {
    let built = crate::resolve::real_file_and_env_builder()
        .build()
        .map_err(|e| ConfigError::file(e.to_string()))?;
    deserialize_viewer(built)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use config::{Environment, File, FileFormat};

    use super::*;

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

    #[test]
    fn default_is_loopback_plus_tailnet_without_override() -> Result<(), Box<dyn std::error::Error>>
    {
        let cfg = deserialize_viewer(Config::builder().add_source(env(&[])).build()?)?;
        assert_eq!(cfg, ViewerConfig::default());
        assert_eq!(cfg.bind, ViewerBind::LoopbackAndTailnet);
        assert_eq!(cfg.tailnet_address, None);
        Ok(())
    }

    #[test]
    fn file_then_env_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let file = File::from_str(
            "host = \"h\"\n[viewer]\nbind = \"loopback\"\ntailnet_address = \"100.64.0.7\"",
            FileFormat::Toml,
        );
        let from_file = deserialize_viewer(Config::builder().add_source(file.clone()).build()?)?;
        assert_eq!(from_file.bind, ViewerBind::Loopback);
        assert_eq!(
            from_file.tailnet_address,
            Some(Ipv4Addr::new(100, 64, 0, 7))
        );

        let with_env = deserialize_viewer(
            Config::builder()
                .add_source(file)
                .add_source(env(&[
                    ("RDPILOT_VIEWER__BIND", "loopback+tailnet"),
                    ("RDPILOT_VIEWER__TAILNET_ADDRESS", "100.100.1.2"),
                ]))
                .build()?,
        )?;
        assert_eq!(with_env.bind, ViewerBind::LoopbackAndTailnet);
        assert_eq!(
            with_env.tailnet_address,
            Some(Ipv4Addr::new(100, 100, 1, 2))
        );
        Ok(())
    }

    #[test]
    fn invalid_bind_is_an_error() -> Result<(), Box<dyn std::error::Error>> {
        let built = Config::builder()
            .add_source(env(&[("RDPILOT_VIEWER__BIND", "0.0.0.0")]))
            .build()?;
        assert!(deserialize_viewer(built).is_err());
        assert!("0.0.0.0".parse::<ViewerBind>().is_err());
        assert_eq!("loopback".parse::<ViewerBind>(), Ok(ViewerBind::Loopback));
        Ok(())
    }

    /// An existing config file without `[viewer]` still loads, and a file
    /// with `[viewer]` still loads the connection settings unchanged.
    #[test]
    fn viewer_table_does_not_disturb_the_connection_config(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let without = Config::builder()
            .add_source(File::from_str("host = \"a\"", FileFormat::Toml))
            .build()?;
        assert_eq!(deserialize_viewer(without)?, ViewerConfig::default());

        let with = Config::builder()
            .add_source(File::from_str(
                "host = \"a\"\nport = 3390\n[viewer]\nbind = \"loopback\"",
                FileFormat::Toml,
            ))
            .build()?;
        let resolved: crate::ResolvedConfig = with.try_deserialize()?;
        assert_eq!(resolved.host.as_deref(), Some("a"));
        assert_eq!(resolved.port, Some(3390));
        Ok(())
    }
}
