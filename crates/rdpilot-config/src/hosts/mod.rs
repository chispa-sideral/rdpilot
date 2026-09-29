//! ssh_config-style host configuration: `Host` blocks, first value wins,
//! `Include`, `-o` overrides, `rdp://` targets and `PasswordCommand`.
//!
//! Pure library code: file access happens only for the paths in
//! [`HostsInput`], and the environment is passed in where needed, so tests
//! never touch the real home directory.

mod expand;
mod keywords;
mod parse;
mod pattern;
mod resolve;
mod target;

use std::fmt;

pub use expand::{expand_password_command, DEFAULT_PORT};
pub use resolve::{resolve_host, HostsInput, ResolvedHost, Row, Setting, Source};
pub use target::{Target, UrlTarget};

/// A hosts-configuration error. Never contains a password or PasswordCommand output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HostsError(String);

impl HostsError {
    pub(crate) fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// A credential. `Debug` is redacted and there is no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wrap a credential.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The credential itself. Only for handing to the daemon.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}
