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

/// The resolved connection configuration, after the file -> env ->
/// flag/MCP-init layering (CONFIG-01) has been applied.
///
/// Every field is optional at this layer — an absent key deserializes to
/// `None`/`false`, never a panic, so a partial (or entirely absent) config
/// file is always valid input. Deliberately its own owned struct, never
/// `rdpilot::ConnectionConfig` (Decision 1) — the daemon (Phase 12) converts
/// this into a `ConnectionConfig` at startup.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolvedConfig {
    /// Hostname or IP address of the RDP target.
    pub host: Option<String>,
    /// TCP port.
    pub port: Option<u16>,
    /// Username for NLA/CredSSP authentication.
    pub username: Option<String>,
    /// Password for NLA/CredSSP authentication.
    ///
    /// Deliberately never reachable via `Serialize` — this struct does not
    /// derive it at all (D-31, one layer before the `rdpilot-ipc` wire
    /// boundary). Any future phase that adds a config-file WRITE path
    /// (Phase 11 only reads config; no `config init`/write command exists
    /// yet) MUST set `0600` permissions on Unix when creating the file
    /// (`std::os::unix::fs::PermissionsExt`) — a world-readable
    /// `config.toml` would expose this plaintext value to other local
    /// accounts.
    #[serde(default)]
    pub password: Option<String>,
    /// Optional Windows domain.
    pub domain: Option<String>,
    /// When `true`, the server certificate is accepted without validation.
    #[serde(default)]
    pub accept_invalid_certs: bool,
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

    /// A partial TOML fragment (only `host`/`port` present) deserializes
    /// without panicking; every absent key becomes `None`/`false`.
    #[test]
    fn partial_toml_deserializes_with_absent_keys_as_none() -> Result<(), Box<dyn std::error::Error>> {
        let built = config::Config::builder()
            .add_source(config::File::from_str(
                "host = \"10.0.0.5\"\nport = 3389",
                config::FileFormat::Toml,
            ))
            .build()
            .map_err(|e| ConfigError::file(e.to_string()))?;
        let resolved: ResolvedConfig = built
            .try_deserialize()
            .map_err(|e| ConfigError::file(e.to_string()))?;

        assert_eq!(resolved.host.as_deref(), Some("10.0.0.5"));
        assert_eq!(resolved.port, Some(3389));
        assert_eq!(resolved.username, None);
        assert_eq!(resolved.password, None);
        assert_eq!(resolved.domain, None);
        assert!(!resolved.accept_invalid_certs);
        Ok(())
    }
}
