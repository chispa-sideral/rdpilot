//! [`ConfigFlags`] — the flag/MCP-init override layer (D-27) feeding
//! `rdpilot_config::resolve`'s file -> env -> flag precedence (CONFIG-01).
//!
//! Field names reuse the exact D-27 config-key vocabulary verbatim: `host`,
//! `port`, `username`, `password`, `domain`, `accept-invalid-certs`.

use clap::Args;
use rdpilot_config::ResolvedConfig;

/// The layered-config override flags every session-targeting verb that
/// needs connection details (today: only `connect`) accepts.
#[derive(Debug, Args)]
pub struct ConfigFlags {
    /// Hostname or IP address of the RDP target.
    #[arg(long)]
    pub host: Option<String>,
    /// TCP port (defaults applied by the daemon when omitted).
    #[arg(long)]
    pub port: Option<u16>,
    /// Username for NLA/CredSSP authentication.
    #[arg(long)]
    pub username: Option<String>,
    /// Password for NLA/CredSSP authentication.
    ///
    /// Prefer `RDPILOT_PASSWORD` (env) or the config file over this flag —
    /// a process's command line (and therefore this value) is visible to
    /// other local users via `ps`/shell history (T-13-13).
    #[arg(long, help = "Password (prefer RDPILOT_PASSWORD env var or the config file — visible via `ps`)")]
    pub password: Option<String>,
    /// Optional Windows domain.
    #[arg(long)]
    pub domain: Option<String>,
    /// When set, the server certificate is accepted without validation
    /// (D-15 risk-named passthrough).
    #[arg(long)]
    pub accept_invalid_certs: bool,
}

impl ConfigFlags {
    /// Convert the parsed flags into a [`ResolvedConfig`] override layer for
    /// `rdpilot_config::resolve` — a `false`/absent flag never clobbers a
    /// lower (env/file) layer, per `apply_overrides`'s documented semantics.
    #[must_use]
    pub fn into_overrides(self) -> ResolvedConfig {
        ResolvedConfig {
            host: self.host,
            port: self.port,
            username: self.username,
            password: self.password,
            domain: self.domain,
            accept_invalid_certs: self.accept_invalid_certs,
            share_root: None,
        }
    }
}
