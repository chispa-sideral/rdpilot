//! [`ConnectionConfig`] — the owned, environment-agnostic description of an RDP
//! target.
//!
//! The library never reads a hard-coded secrets path or environment variable
//! (D-12): the caller supplies every value. Credentials are pass-through
//! parameters handed to NLA/CredSSP by our own IronRDP client; `rdpilot` never
//! touches OS credential machinery (D-14) and never logs credential fields.

use std::fmt;
use std::path::{Path, PathBuf};

/// Default RDP port.
pub const DEFAULT_PORT: u16 = 3389;

/// Default requested desktop width, in pixels.
pub const DEFAULT_WIDTH: u16 = 1920;

/// Default requested desktop height, in pixels.
pub const DEFAULT_HEIGHT: u16 = 1080;

/// Owned configuration for a single RDP connection.
///
/// Construct with [`ConnectionConfig::new`] and adjust optional fields with the
/// builder-style setters. The type is environment-agnostic: where the host,
/// credentials, and dimensions come from (env vars, a config file, a UI) is the
/// caller's concern.
///
/// # Security
///
/// The [`Debug`](std::fmt::Debug) implementation **redacts** the password so it
/// can never leak into logs or panic messages (D-14). Do not log the password
/// field by any other route.
#[derive(Clone)]
pub struct ConnectionConfig {
    /// Hostname or IP address of the RDP target.
    host: String,
    /// TCP port (defaults to [`DEFAULT_PORT`]).
    port: u16,
    /// Username for NLA/CredSSP authentication.
    username: String,
    /// Password for NLA/CredSSP authentication (redacted in `Debug`).
    password: String,
    /// Optional Windows domain.
    domain: Option<String>,
    /// Requested desktop width, in pixels.
    width: u16,
    /// Requested desktop height, in pixels.
    height: u16,
    /// When `true`, the server certificate is accepted without validation.
    ///
    /// Risk-named and **default `false`**. Intended only for self-signed
    /// workgroup lab targets (D-15). Thumbprint pinning is deferred.
    accept_invalid_certs: bool,
    /// Local filesystem path of the sensor executable to serve over the
    /// RDPDR redirected drive (D-5.1, SENSOR-02).
    ///
    /// `None` (the default) means no RDPDR static channel is registered at
    /// connect time — the connect path is byte-for-byte the pre-Phase-5
    /// behavior. Owned `PathBuf` (D-09 — no third-party type in the public
    /// signature).
    sensor_binary_path: Option<PathBuf>,
}

impl ConnectionConfig {
    /// Create a configuration for `host` with the given credentials.
    ///
    /// Port defaults to [`DEFAULT_PORT`], dimensions to
    /// [`DEFAULT_WIDTH`]x[`DEFAULT_HEIGHT`], no domain, and certificate
    /// validation **on**. Use the builder setters to override.
    pub fn new(
        host: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port: DEFAULT_PORT,
            username: username.into(),
            password: password.into(),
            domain: None,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            accept_invalid_certs: false,
            sensor_binary_path: None,
        }
    }

    /// Set the TCP port (builder).
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the Windows domain (builder).
    #[must_use]
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Set the requested desktop dimensions (builder).
    #[must_use]
    pub fn dimensions(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Accept the server certificate without validation (builder).
    ///
    /// **Risk:** disables MITM protection. Use only against a self-signed lab
    /// target you control (D-15). Defaults to `false`.
    #[must_use]
    pub fn accept_invalid_certs(mut self, accept: bool) -> Self {
        self.accept_invalid_certs = accept;
        self
    }

    /// Set the local filesystem path of the sensor executable to serve over
    /// the RDPDR redirected drive (builder, D-5.1, SENSOR-02).
    ///
    /// When set, `connect::connect` registers the RDPDR static channel
    /// (`RdpilotDriveBackend`) announcing a `RDPILOT` drive that serves this
    /// file, alongside the existing `DrdynvcClient` registration. When unset
    /// (the default), no RDPDR channel is registered and the connect path is
    /// unchanged from pre-Phase-5 behavior.
    #[must_use]
    pub fn sensor_binary_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.sensor_binary_path = Some(path.into());
        self
    }

    /// Hostname or IP of the target.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// TCP port.
    pub fn get_port(&self) -> u16 {
        self.port
    }

    /// Username for authentication.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Password for authentication.
    ///
    /// Intended to be consumed by the connect path; do not log the returned
    /// value.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Optional Windows domain.
    pub fn get_domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    /// Requested desktop width.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Requested desktop height.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Whether invalid server certificates are accepted.
    pub fn get_accept_invalid_certs(&self) -> bool {
        self.accept_invalid_certs
    }

    /// Local filesystem path of the sensor executable served over the RDPDR
    /// redirected drive, if configured (D-5.1, SENSOR-02).
    ///
    /// `None` means the RDPDR static channel is not registered at connect
    /// time.
    pub fn get_sensor_binary_path(&self) -> Option<&Path> {
        self.sensor_binary_path.as_deref()
    }
}

/// Redacted `Debug`: never prints the password (D-14, threat T-02-02).
impl fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("domain", &self.domain)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("accept_invalid_certs", &self.accept_invalid_certs)
            .field("sensor_binary_path", &self.sensor_binary_path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let cfg = ConnectionConfig::new("host", "user", "pw");
        assert_eq!(cfg.get_port(), DEFAULT_PORT);
        assert_eq!(cfg.width(), DEFAULT_WIDTH);
        assert_eq!(cfg.height(), DEFAULT_HEIGHT);
        assert_eq!(cfg.get_domain(), None);
        // Cert validation must default ON.
        assert!(!cfg.get_accept_invalid_certs());
        // No RDPDR channel is registered unless a sensor path is configured.
        assert_eq!(cfg.get_sensor_binary_path(), None);
    }

    #[test]
    fn sensor_binary_path_builder_and_getter_roundtrip() {
        let cfg = ConnectionConfig::new("h", "u", "p").sensor_binary_path("/tmp/rdpilot-sensor.exe");
        assert_eq!(
            cfg.get_sensor_binary_path(),
            Some(std::path::Path::new("/tmp/rdpilot-sensor.exe"))
        );
    }

    #[test]
    fn builders_apply() {
        let cfg = ConnectionConfig::new("h", "u", "p")
            .port(3390)
            .domain("CORP")
            .dimensions(800, 600)
            .accept_invalid_certs(true);
        assert_eq!(cfg.get_port(), 3390);
        assert_eq!(cfg.get_domain(), Some("CORP"));
        assert_eq!(cfg.width(), 800);
        assert_eq!(cfg.height(), 600);
        assert!(cfg.get_accept_invalid_certs());
    }

    #[test]
    fn debug_redacts_password() {
        let cfg = ConnectionConfig::new("host", "user", "super-secret-pw");
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("super-secret-pw"));
        assert!(rendered.contains("<redacted>"));
        // Non-secret fields remain visible for diagnostics.
        assert!(rendered.contains("host"));
        assert!(rendered.contains("user"));
    }
}
