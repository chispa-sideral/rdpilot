//! [`resolve`] — the file -> env layered resolution of the daemon-local
//! settings (`share_root`, `bundle_path`, `[viewer]`).

use std::path::PathBuf;

use config::{Config, Environment, File, FileFormat};
use directories::BaseDirs;

use crate::resolved::{ConfigError, ResolvedConfig};

/// Env var prefix (D-27): `RDPILOT_<KEY>`.
const ENV_PREFIX: &str = "RDPILOT";
/// Separator between the prefix and the first key segment (D-27).
const ENV_PREFIX_SEPARATOR: &str = "_";
/// Separator for nested keys (D-27) — unused today (no nested `ResolvedConfig`
/// fields) but set for forward consistency with the `config` crate's own
/// nested-key convention.
const ENV_KEY_SEPARATOR: &str = "__";

/// Build the file+env-layered [`ResolvedConfig`] from an already-constructed
/// `config::Config` (later-added source wins). Factored out of [`resolve`]
/// so a test can inject explicit file/env sources rather than only reading
/// the real platform path and ambient process environment — this is what
/// makes the CONFIG-01 precedence test deterministic.
fn deserialize_layered(built: Config) -> Result<ResolvedConfig, ConfigError> {
    built
        .try_deserialize()
        .map_err(|e| ConfigError::file(e.to_string()))
}

/// Build the file+env `config::Config` layers for the real platform config
/// file ([`crate::config_file_path`], optional — `.required(false)`) and the
/// real process environment (`RDPILOT_` prefix). The env source is added
/// LAST so it wins over the file source (CONFIG-01: env beats file).
pub(crate) fn real_file_and_env_builder() -> config::ConfigBuilder<config::builder::DefaultState> {
    let mut builder = Config::builder();
    if let Some(path) = crate::paths::config_file_path() {
        builder = builder
            .add_source(File::new(&path.to_string_lossy(), FileFormat::Toml).required(false));
    }
    builder.add_source(
        Environment::with_prefix(ENV_PREFIX)
            .prefix_separator(ENV_PREFIX_SEPARATOR)
            .separator(ENV_KEY_SEPARATOR),
    )
}

/// Resolve a [`ResolvedConfig`] from the file -> env layers, env winning.
///
/// # Errors
///
/// Returns [`ConfigError::File`] if the platform config file exists but
/// cannot be parsed, or if the resulting layered config cannot deserialize
/// into [`ResolvedConfig`].
pub fn resolve() -> Result<ResolvedConfig, ConfigError> {
    let built = real_file_and_env_builder()
        .build()
        .map_err(|e| ConfigError::file(e.to_string()))?;
    deserialize_layered(built)
}

/// Fixed subpath appended to the platform data directory for the
/// daemon-local file-transfer staging root default (research "Pitfall 6" /
/// D-10.1) — used only when [`ResolvedConfig::share_root`] is unset.
const DEFAULT_SHARE_ROOT_SUBPATH: [&str; 2] = ["rdpilot", "transfer-staging"];

/// Resolve the daemon-local file-transfer staging root
/// (`rdpilot::ConnectionConfig::share_root`) from `cfg.share_root` when set,
/// else a documented `directories`-based platform-data-dir default
/// (`<data_dir>/rdpilot/transfer-staging`). Never fails/panics: when even
/// `directories::BaseDirs::new()` cannot resolve a home directory on this
/// platform, falls back to `std::env::temp_dir()/rdpilot/transfer-staging`
/// so a caller always gets a usable, non-empty path (never `None`/panic —
/// API-01 discipline mirrored from `rdpilot`).
#[must_use]
pub fn share_root_or_default(cfg: &ResolvedConfig) -> PathBuf {
    if let Some(configured) = &cfg.share_root {
        return PathBuf::from(configured);
    }
    default_share_root()
}

/// The `directories`-based platform-data-dir default staging root, with a
/// `std::env::temp_dir()`-based fallback when no platform data directory can
/// be resolved at all (mirrors `paths::config_file_path`'s
/// `BaseDirs`-optionality handling, but never returns `None` here — a
/// missing `share_root` must still resolve to *something* usable so
/// `put`/`get` are never unconditionally broken by an unresolvable home
/// directory).
fn default_share_root() -> PathBuf {
    let mut root = BaseDirs::new()
        .map(|b| b.data_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    for segment in DEFAULT_SHARE_ROOT_SUBPATH {
        root.push(segment);
    }
    root
}

#[cfg(test)]
fn empty_config() -> ResolvedConfig {
    ResolvedConfig {
        share_root: None,
        bundle_path: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// File -> env precedence, with injected sources (no real home dir or
    /// ambient env involved). Old connection keys and `RDPILOT_HOST` are ignored.
    #[test]
    fn layered_precedence() -> Result<(), Box<dyn std::error::Error>> {
        let file_layer = File::from_str(
            "share_root = \"/file\"\nhost = \"file-host\"",
            FileFormat::Toml,
        );

        let mut env_map: HashMap<String, String> = HashMap::new();
        env_map.insert("RDPILOT_SHARE_ROOT".to_owned(), "/env".to_owned());
        env_map.insert("RDPILOT_HOST".to_owned(), "env-host".to_owned());

        let built = Config::builder()
            .add_source(file_layer.clone())
            .add_source(
                Environment::with_prefix(ENV_PREFIX)
                    .prefix_separator(ENV_PREFIX_SEPARATOR)
                    .separator(ENV_KEY_SEPARATOR)
                    .source(Some(env_map)),
            )
            .build()?;
        let resolved = deserialize_layered(built)?;
        assert_eq!(
            resolved.share_root.as_deref(),
            Some("/env"),
            "env must beat file"
        );

        let file_only = deserialize_layered(Config::builder().add_source(file_layer).build()?)?;
        assert_eq!(file_only.share_root.as_deref(), Some("/file"));
        Ok(())
    }

    /// `share_root_or_default` returns the configured value, verbatim, when
    /// `ResolvedConfig::share_root` is set.
    #[test]
    fn share_root_or_default_returns_the_configured_value_when_set() {
        let mut cfg = empty_config();
        cfg.share_root = Some("/configured/share-root".to_owned());
        assert_eq!(
            share_root_or_default(&cfg),
            PathBuf::from("/configured/share-root")
        );
    }

    /// `share_root_or_default` falls back to a non-empty, `rdpilot`-scoped
    /// platform-data-dir default when `share_root` is unset — never an empty
    /// path, never a panic.
    #[test]
    fn share_root_or_default_returns_a_nonempty_default_path_when_unset() {
        let cfg = empty_config();
        let default_path = share_root_or_default(&cfg);
        assert!(
            !default_path.as_os_str().is_empty(),
            "default share_root path must not be empty"
        );
        let components: Vec<String> = default_path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert!(
            components.contains(&"rdpilot".to_owned()),
            "default share_root must be scoped under an `rdpilot` segment, got {default_path:?}"
        );
        assert!(
            components.contains(&"transfer-staging".to_owned()),
            "default share_root must end in `transfer-staging`, got {default_path:?}"
        );
    }
}
