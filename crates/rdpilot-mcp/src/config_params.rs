//! [`McpConnectParams`] — the MCP-init connection-param override layer
//! (D-27) feeding `rdpilot_config::resolve`'s third (flag/MCP-init) layer.
//!
//! Field names reuse the EXACT D-27 config-key vocabulary verbatim (`host`,
//! `port`, `username`, `password`, `domain`, `accept_invalid_certs`),
//! mirroring `rdpilot-cli::config_flags::ConfigFlags`'s identical field set
//! and `into_overrides` semantics field-for-field — the CLI's `--host` flag
//! and the MCP `rdpilot_connect` tool's `host` param are the SAME D-27 key,
//! just arriving through two different typed front-ends (`clap` vs.
//! `serde`/`schemars`) onto the one shared `rdpilot_config::resolve`
//! pipeline (CONFIG-01).

use rdpilot_config::ResolvedConfig;
use schemars::JsonSchema;
use serde::Deserialize;

/// The MCP-init connection-param override layer every `rdpilot_connect`
/// call accepts (flattened onto that tool's top-level input).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpConnectParams {
    /// Hostname or IP address of the RDP target.
    #[serde(default)]
    pub host: Option<String>,
    /// TCP port (defaults applied by the daemon when omitted).
    #[serde(default)]
    pub port: Option<u16>,
    /// Username for NLA/CredSSP authentication.
    #[serde(default)]
    pub username: Option<String>,
    /// Password for NLA/CredSSP authentication.
    #[serde(default)]
    pub password: Option<String>,
    /// Optional Windows domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// When `true`, the server certificate is accepted without validation
    /// (D-15 risk-named passthrough).
    #[serde(default)]
    pub accept_invalid_certs: bool,
}

impl McpConnectParams {
    /// Convert into a [`ResolvedConfig`] override layer for
    /// `rdpilot_config::resolve` — a `None`/`false` field never clobbers a
    /// lower (file/env) layer, mirroring
    /// `ConfigFlags::into_overrides`'s documented semantics exactly
    /// (`share_root` is never MCP-init-configurable — daemon-local
    /// operational config, research Pitfall 6).
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn into_overrides_maps_every_d27_key_verbatim() {
        let params = McpConnectParams {
            host: Some("10.0.0.5".to_owned()),
            port: Some(3389),
            username: Some("alice".to_owned()),
            password: Some("secret".to_owned()),
            domain: Some("CORP".to_owned()),
            accept_invalid_certs: true,
        };
        let overrides = params.into_overrides();
        assert_eq!(overrides.host.as_deref(), Some("10.0.0.5"));
        assert_eq!(overrides.port, Some(3389));
        assert_eq!(overrides.username.as_deref(), Some("alice"));
        assert_eq!(overrides.password.as_deref(), Some("secret"));
        assert_eq!(overrides.domain.as_deref(), Some("CORP"));
        assert!(overrides.accept_invalid_certs);
        assert!(overrides.share_root.is_none(), "share_root must never be MCP-init-configurable");
    }

    #[test]
    fn absent_fields_never_clobber_via_into_overrides() {
        let overrides = McpConnectParams::default().into_overrides();
        assert!(overrides.host.is_none());
        assert!(overrides.port.is_none());
        assert!(overrides.username.is_none());
        assert!(overrides.password.is_none());
        assert!(overrides.domain.is_none());
        assert!(!overrides.accept_invalid_certs);
    }

    #[test]
    fn default_params_deserialize_from_an_empty_json_object() {
        let params: McpConnectParams = serde_json::from_str("{}").expect("an empty object must deserialize");
        assert!(params.host.is_none());
        assert!(!params.accept_invalid_certs);
    }

    #[test]
    fn params_deserialize_the_exact_d27_key_spellings() {
        let json = r#"{"host":"h","port":3389,"username":"u","password":"p","domain":"d","accept_invalid_certs":true}"#;
        let params: McpConnectParams = serde_json::from_str(json).expect("D-27 keys must deserialize verbatim");
        assert_eq!(params.host.as_deref(), Some("h"));
        assert_eq!(params.port, Some(3389));
        assert_eq!(params.username.as_deref(), Some("u"));
        assert_eq!(params.password.as_deref(), Some("p"));
        assert_eq!(params.domain.as_deref(), Some("d"));
        assert!(params.accept_invalid_certs);
    }
}
