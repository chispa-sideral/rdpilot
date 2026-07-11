//! [`resolve`] / [`apply_overrides`] — the file -> env -> flag/MCP-init
//! layered resolution pipeline (CONFIG-01, D-27).
//!
//! `resolve()` builds the file+env layers via the `config` crate
//! (`Config::builder().add_source(...)`, later sources winning) and then
//! applies the flag/MCP-init layer as a plain Rust `Option`-based override
//! pass — deliberately NOT routed through `config::Source`. Flags (Phase
//! 13's `clap`) and MCP-init params (Phase 14's `rmcp`) already arrive as
//! typed values; shoehorning already-typed values back through a
//! textual/map-shaped `Source` trait would re-stringify and re-parse data
//! that is already correct, for no benefit.

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
    built.try_deserialize().map_err(|e| ConfigError::file(e.to_string()))
}

/// Build the file+env `config::Config` layers for the real platform config
/// file ([`crate::config_file_path`], optional — `.required(false)`) and the
/// real process environment (`RDPILOT_` prefix). The env source is added
/// LAST so it wins over the file source (CONFIG-01: env beats file).
fn real_file_and_env_builder() -> config::ConfigBuilder<config::builder::DefaultState> {
    let mut builder = Config::builder();
    if let Some(path) = crate::paths::config_file_path() {
        builder = builder.add_source(File::new(&path.to_string_lossy(), FileFormat::Toml).required(false));
    }
    builder.add_source(
        Environment::with_prefix(ENV_PREFIX)
            .prefix_separator(ENV_PREFIX_SEPARATOR)
            .separator(ENV_KEY_SEPARATOR),
    )
}

/// Resolve a [`ResolvedConfig`] from the file -> env -> flag/MCP-init layers
/// (CONFIG-01), with the highest-precedence layer that sets a value always
/// winning. `overrides` is the third layer — already-typed values a caller
/// (Phase 13 CLI / Phase 14 MCP) collected from flags or MCP-init params.
///
/// # Errors
///
/// Returns [`ConfigError::File`] if the platform config file exists but
/// cannot be parsed, or if the resulting layered config cannot deserialize
/// into [`ResolvedConfig`].
pub fn resolve(overrides: ResolvedConfig) -> Result<ResolvedConfig, ConfigError> {
    let built = real_file_and_env_builder()
        .build()
        .map_err(|e| ConfigError::file(e.to_string()))?;
    let base = deserialize_layered(built)?;
    Ok(apply_overrides(base, overrides))
}

/// The third layer (flag/MCP-init): override each field of `base` with the
/// corresponding field of `overrides` only when `overrides` actually sets
/// it — `None` (or, for `accept_invalid_certs`, `false`) never clobbers a
/// lower layer's value.
#[must_use]
pub fn apply_overrides(mut base: ResolvedConfig, overrides: ResolvedConfig) -> ResolvedConfig {
    if overrides.host.is_some() {
        base.host = overrides.host;
    }
    if overrides.port.is_some() {
        base.port = overrides.port;
    }
    if overrides.username.is_some() {
        base.username = overrides.username;
    }
    if overrides.password.is_some() {
        base.password = overrides.password;
    }
    if overrides.domain.is_some() {
        base.domain = overrides.domain;
    }
    if overrides.accept_invalid_certs {
        base.accept_invalid_certs = true;
    }
    if overrides.share_root.is_some() {
        base.share_root = overrides.share_root;
    }
    if overrides.sensor_binary_path.is_some() {
        base.sensor_binary_path = overrides.sensor_binary_path;
    }
    base
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

/// An all-`None`/`false` [`ResolvedConfig`] — the identity value for
/// [`apply_overrides`]'s override layer (a caller with nothing to override
/// passes this). Test-only helper (production callers build their own
/// `ResolvedConfig` from parsed flags/MCP-init params in Phase 13/14).
#[cfg(test)]
fn empty_overrides() -> ResolvedConfig {
    ResolvedConfig {
        host: None,
        port: None,
        username: None,
        password: None,
        domain: None,
        accept_invalid_certs: false,
        share_root: None,
        sensor_binary_path: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// CONFIG-01: file -> env -> override precedence, deterministic, with
    /// injected sources (no real home dir / ambient env involved).
    #[test]
    fn layered_precedence() -> Result<(), Box<dyn std::error::Error>> {
        let file_layer = File::from_str("host = \"file-host\"\nport = 1", FileFormat::Toml);

        let mut env_map: HashMap<String, String> = HashMap::new();
        env_map.insert("RDPILOT_HOST".to_owned(), "env-host".to_owned());

        // env beats file: both sources present, env added last.
        let built = Config::builder()
            .add_source(file_layer.clone())
            .add_source(
                Environment::with_prefix(ENV_PREFIX)
                    .prefix_separator(ENV_PREFIX_SEPARATOR)
                    .separator(ENV_KEY_SEPARATOR)
                    .source(Some(env_map.clone())),
            )
            .build()?;
        let resolved = deserialize_layered(built)?;
        assert_eq!(resolved.host.as_deref(), Some("env-host"), "env must beat file");
        assert_eq!(resolved.port, Some(1), "port only set by file, must survive");

        // override wins over env+file.
        let mut with_override = empty_overrides();
        with_override.host = Some("flag-host".to_owned());
        let final_resolved = apply_overrides(resolved.clone(), with_override);
        assert_eq!(
            final_resolved.host.as_deref(),
            Some("flag-host"),
            "override must beat env"
        );

        // with no override: env still wins over file.
        let no_override = apply_overrides(resolved.clone(), empty_overrides());
        assert_eq!(no_override.host.as_deref(), Some("env-host"));

        // file only (no env source at all): file value survives.
        let file_only_built = Config::builder().add_source(file_layer).build()?;
        let file_only = deserialize_layered(file_only_built)?;
        assert_eq!(file_only.host.as_deref(), Some("file-host"));

        Ok(())
    }

    /// `apply_overrides` never clobbers a lower layer with a `None` override.
    #[test]
    fn apply_overrides_none_does_not_clobber() {
        let mut base = empty_overrides();
        base.host = Some("base-host".to_owned());

        let result = apply_overrides(base, empty_overrides());
        assert_eq!(result.host.as_deref(), Some("base-host"));
    }

    /// `apply_overrides` carries `share_root` with the same
    /// override-wins/absent-never-clobbers semantics as every other field.
    #[test]
    fn apply_overrides_share_root_follows_the_same_precedence_rules() {
        let mut base = empty_overrides();
        base.share_root = Some("/base/share".to_owned());

        // An override with no share_root set must not clobber the base value.
        let unchanged = apply_overrides(base.clone(), empty_overrides());
        assert_eq!(unchanged.share_root.as_deref(), Some("/base/share"));

        // An explicit override wins.
        let mut with_override = empty_overrides();
        with_override.share_root = Some("/override/share".to_owned());
        let overridden = apply_overrides(base, with_override);
        assert_eq!(overridden.share_root.as_deref(), Some("/override/share"));
    }

    /// `share_root_or_default` returns the configured value, verbatim, when
    /// `ResolvedConfig::share_root` is set.
    #[test]
    fn share_root_or_default_returns_the_configured_value_when_set() {
        let mut cfg = empty_overrides();
        cfg.share_root = Some("/configured/share-root".to_owned());
        assert_eq!(share_root_or_default(&cfg), PathBuf::from("/configured/share-root"));
    }

    /// `share_root_or_default` falls back to a non-empty, `rdpilot`-scoped
    /// platform-data-dir default when `share_root` is unset — never an empty
    /// path, never a panic.
    #[test]
    fn share_root_or_default_returns_a_nonempty_default_path_when_unset() {
        let cfg = empty_overrides();
        let default_path = share_root_or_default(&cfg);
        assert!(!default_path.as_os_str().is_empty(), "default share_root path must not be empty");
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
