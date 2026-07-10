//! [`config_file_path`] — platform config-dir resolution (D-27, CONFIG-02).
//!
//! Uses `directories::BaseDirs` (the OS-level config root), NOT
//! `directories::ProjectDirs` — `ProjectDirs::from(...).config_dir()` appends
//! an unwanted extra `config` subfolder on Windows, producing
//! `%APPDATA%\rdpilot\config\config.toml` instead of D-27's exact
//! `%APPDATA%\rdpilot\config.toml`. `BaseDirs::config_dir()` plus a manual
//! `.join("rdpilot").join("config.toml")` hits the D-27 spec exactly on both
//! platforms.

use std::path::PathBuf;

use directories::BaseDirs;

/// Resolve the platform-conventional config file path (D-27):
/// `~/.config/rdpilot/config.toml` on Unix/XDG-style hosts,
/// `%APPDATA%\rdpilot\config.toml` on Windows.
///
/// Returns `None` only when `BaseDirs::new()` itself cannot resolve a home
/// directory on this platform (e.g. no `HOME`/`USERPROFILE`) — an
/// environment-level condition, not a config-content error.
///
/// A future config-file WRITE path (out of Phase 11's scope — this crate
/// only reads config) must set `0600` permissions on Unix
/// (`std::os::unix::fs::PermissionsExt`) when first creating this file, so a
/// plaintext password does not become world-readable.
#[must_use]
pub fn config_file_path() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.config_dir().join("rdpilot").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_path_matches_platform_convention() {
        let Some(path) = config_file_path() else {
            // No resolvable home dir on this host — nothing more to assert.
            return;
        };
        let components: Vec<_> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("config.toml"),
            "path must end in config.toml, got {path:?}"
        );
        assert_eq!(
            path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
            Some("rdpilot"),
            "the config.toml's immediate parent must be exactly one `rdpilot` segment, got {path:?}"
        );
        // Guards the ProjectDirs pitfall: no doubled `config` directory
        // segment appears anywhere in the path (BaseDirs::config_dir() is
        // already the OS-level config root; ProjectDirs would additionally
        // nest a literal "config" folder under the app folder).
        let config_segment_count = components.iter().filter(|c| c.as_str() == "config").count();
        assert_eq!(
            config_segment_count, 0,
            "no doubled `config` directory segment expected, got {path:?}"
        );
    }

    /// CONFIG-02: the shipped template parses into [`crate::ResolvedConfig`]
    /// (all keys commented -> all `None`/`false`).
    #[test]
    fn template_parses_into_resolved_config() -> Result<(), Box<dyn std::error::Error>> {
        let built = config::Config::builder()
            .add_source(config::File::from_str(crate::CONFIG_TEMPLATE, config::FileFormat::Toml))
            .build()?;
        let resolved: crate::ResolvedConfig = built.try_deserialize()?;

        assert_eq!(resolved.host, None);
        assert_eq!(resolved.port, None);
        assert_eq!(resolved.username, None);
        assert_eq!(resolved.password, None);
        assert_eq!(resolved.domain, None);
        assert!(!resolved.accept_invalid_certs);
        Ok(())
    }

    /// CONFIG-02: the template documents every D-27 key by name.
    #[test]
    fn template_documents_every_d27_key() {
        for key in ["host", "port", "username", "password", "domain", "accept_invalid_certs"] {
            assert!(
                crate::CONFIG_TEMPLATE.contains(key),
                "template must mention key `{key}`"
            );
        }
    }
}
