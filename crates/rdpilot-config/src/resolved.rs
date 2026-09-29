//! [`ResolvedConfig`] — the layered-resolution output type, and [`ConfigError`]
//! — the owned error type (D-09) callers see.
//!
//! `ResolvedConfig` deliberately derives ONLY `Debug, Clone, Deserialize` —
//! never `Serialize` (structural credential-leak prevention, D-31 applied one
//! layer before the `rdpilot-ipc` wire boundary). `ConfigError` source-erases
//! `config::ConfigError` into an owned `String` so no third-party error type
//! ever appears in this crate's public signatures (D-09), mirroring
//! `rdpilot::Error`'s own `Display`-only external-detail style.

use serde::Deserialize;

/// The resolved daemon-local configuration (`config.toml` and `RDPILOT_*`
/// environment variables).
///
/// Connection settings (host, port, user, credentials, ...) are not here:
/// they live in the hosts file, see [`crate::hosts`]. Keys for them left in an
/// old `config.toml`, and `RDPILOT_HOST`-style variables, are ignored.
///
/// Every field is optional; an absent key is `None`, so a partial (or
/// absent) config file is always valid input. Deliberately does not derive
/// `Serialize`.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolvedConfig {
    /// Local filesystem path of the daemon-local file-transfer staging root
    /// (`ConnectionConfig::share_root`, D-10.1/FILE-01/FILE-02).
    ///
    /// This is daemon-local operational config, never a wire-transmitted
    /// value from `Request::Connect` (research Pitfall 6) -- a caller
    /// configures it via `config.toml` or `RDPILOT_SHARE_ROOT`. `None` (the default) means
    /// [`crate::share_root_or_default`] falls back to a documented
    /// platform-data-dir default rather than leaving `put`/`get`
    /// unconfigured.
    #[serde(default)]
    pub share_root: Option<String>,
    /// Directory containing the pinned Cua archive, bridge executable and manifest.
    /// Daemon-local configuration, resolved from `RDPILOT_BUNDLE_PATH` or config.
    /// Unset leaves native RDP recovery available without deploying a bridge.
    #[serde(default)]
    pub bundle_path: Option<String>,
}

/// Owned configuration error (D-09): no third-party type (`config::ConfigError`,
/// `toml::*`) ever appears in a public signature — external detail is
/// source-erased into an owned `String`, mirroring `rdpilot::Error`'s style.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The config file could not be read or parsed.
    #[error("failed to read/parse the config file: {0}")]
    File(String),
    /// The platform config directory could not be resolved (e.g. no `HOME`/
    /// `APPDATA` on this platform).
    #[error("could not resolve the platform config directory: {0}")]
    PathResolution(String),
}

impl ConfigError {
    /// Construct a [`ConfigError::File`] from any error type displayable as
    /// a string — source-erases `config::ConfigError` (D-09).
    #[allow(dead_code)] // Consumed by Task 2's path_resolution flow and Task 3's resolve() pipeline (interface-first); already exercised by this file's own inline test.
    pub(crate) fn file(msg: impl Into<String>) -> Self {
        ConfigError::File(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Old connection keys are ignored; absent keys are `None`.
    #[test]
    fn old_connection_keys_are_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let built = config::Config::builder()
            .add_source(config::File::from_str(
                "host = \"10.0.0.5\"\nport = 3389\nshare_root = \"/s\"",
                config::FileFormat::Toml,
            ))
            .build()
            .map_err(|e| ConfigError::file(e.to_string()))?;
        let resolved: ResolvedConfig = built
            .try_deserialize()
            .map_err(|e| ConfigError::file(e.to_string()))?;

        assert_eq!(resolved.share_root.as_deref(), Some("/s"));
        assert_eq!(resolved.bundle_path, None);
        Ok(())
    }
}
