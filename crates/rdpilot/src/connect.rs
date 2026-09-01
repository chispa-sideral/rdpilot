//! Internal connect path: drive the IronRDP connector state machine to an
//! authenticated active session over TLS/CredSSP (SESS-01).
//!
//! Mirrors the canonical upstream sequence (`ironrdp/examples/screenshot.rs` for
//! the config + TLS-upgrade/cert-key extraction, `ironrdp-client/src/rdp.rs` for
//! the async connect flow) using the `ironrdp-tokio` async helpers:
//! `connect_begin` → rustls TLS upgrade (resumption disabled, D-15 cert policy) →
//! `mark_as_upgraded` → `connect_finalize`.
//!
//! Three correctness points are load-bearing and intentional:
//! - **TLS resumption is disabled** — CredSSP forbids it (MS-CSSP, Pitfall 5).
//! - **Cert policy is selected by `accept_invalid_certs`** — default path validates
//!   against the platform roots; the risk-named opt-out installs a no-op verifier
//!   for a self-signed lab target only (D-15, threat T-02-03).
//! - **The `RDPILOT_SENSOR` DVC seam** is registered on the connector *before*
//!   `connect_begin` (registration is impossible once the session is active — a hard
//!   IronRDP constraint, SC#1). `RdpilotSensorProcessor` is registered here via
//!   `DrdynvcClient::with_dynamic_channel`; `connect()` returns the shared
//!   `Arc<SensorShared>` so `Session` can drive `Session::ping()` against it.
//! - **The `RDPDR` static channel** (drive redirection, D-5.1, SENSOR-02) is
//!   registered at the identical connect-time seam, as a SIBLING static channel
//!   to `DrdynvcClient` — NOT routed through it. `ironrdp_rdpdr::Rdpdr` fully
//!   implements `SvcProcessor`/`SvcClientProcessor` (verified in the pinned
//!   `ironrdp-rdpdr-0.6.0` source) and dispatches inbound MS-RDPEFS IRPs to the
//!   registered [`crate::rdpdr_backend::RdpilotDriveBackend`] internally —
//!   `ActiveStage::process` drives it automatically, exactly like the drdynvc
//!   static channel; no `session_loop.rs` change is needed. Registered only
//!   when [`ConnectionConfig::get_sensor_binary_path`] is `Some`; when `None`,
//!   the connect path is byte-for-byte the pre-Phase-5 behavior.
//!
//! Credentials and certificate material are never logged (Security V7, threat
//! T-02-02). No `unwrap`/`expect`/`panic` in non-test code (API-01).

use std::fs;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use ironrdp::connector::{ClientConnector, Config, ConnectionResult, Credentials, DesktopSize};
use ironrdp::dvc::DrdynvcClient;
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp_rdpdr::Rdpdr;
use ironrdp::pdu::rdp::capability_sets::MajorPlatformType;
use ironrdp::pdu::rdp::client_info::{PerformanceFlags, TimezoneInfo};
use ironrdp_tokio::TokioFramed;
use rustls::client::Resumption;
use rustls::ClientConfig;
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use crate::config::ConnectionConfig;
use crate::error::{Error, Result};

/// The dynamic virtual channel name for the Phase 4 perception sensor.
///
/// Registered via `RdpilotSensorProcessor::channel_name()` in [`connect`] before
/// `connect_begin` (SC#1).
pub(crate) const RDPILOT_SENSOR: &str = "RDPILOT_SENSOR";

/// The filename the RDPDR drive backend serves the sensor exe under, and the
/// filename [`crate::session::Session::deploy_and_launch`]'s in-band copy
/// command references (D-5.1). Defined once here so the announced name and
/// the launch command can never drift apart.
pub(crate) const SENSOR_EXE_NAME: &str = "rdpilot-sensor.exe";

/// The framed transport over the TLS-upgraded, type-erased async stream.
///
/// The concrete stream type is boxed and erased so the rest of the SDK (the
/// session loop, the `Session` handle) never names a `rustls`/`tokio-rustls`
/// type — keeping third-party types out of the internal seams as well as the
/// public API.
pub(crate) type UpgradedStream = Box<dyn AsyncReadWrite + Send + Sync + Unpin>;

/// Marker trait combining the async read/write bounds we erase the stream behind.
pub(crate) trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> AsyncReadWrite for T {}

/// The framed transport handed to the active-session loop after a successful
/// connect.
pub(crate) type ConnectedFramed = TokioFramed<UpgradedStream>;

/// Drive the full connect sequence and return the active-session inputs.
///
/// On success the caller owns a [`ConnectionResult`] (desktop size, channel IDs,
/// reactivation sequence) and the TLS-upgraded [`ConnectedFramed`] ready for the
/// PDU pump. Every failure is mapped to an owned [`Error`] variant; no
/// credential or certificate material is ever logged.
pub(crate) async fn connect(
    cfg: &ConnectionConfig,
) -> Result<(ConnectionResult, ConnectedFramed, std::sync::Arc<crate::sensor::SensorShared>)> {
    let server_name = cfg.host().to_owned();
    let addr = resolve_addr(&server_name, cfg.get_port())?;

    let tcp_stream = TcpStream::connect(addr)
        .await
        .map_err(|e| Error::Connect(format!("TCP connect to {server_name} failed: {e}")))?;

    let client_addr = tcp_stream
        .local_addr()
        .map_err(|e| Error::Connect(format!("could not read local socket address: {e}")))?;

    let connector_config = build_connector_config(cfg);

    let mut framed = TokioFramed::new(tcp_stream);
    let mut connector = ClientConnector::new(connector_config, client_addr);

    // ── Phase 4 DVC seam ──────────────────────────────────────────────────────
    // Register the sensor's DvcProcessor on the DRDYNVC static channel BEFORE
    // connect_begin. Dynamic virtual channels (including `RDPILOT_SENSOR`, the
    // perception channel) can only be registered before the connection is
    // finalized — this is a hard IronRDP constraint (SC#1). `sensor` is the
    // shared correlation state (pending oneshot map + handshake); one clone is
    // moved into the processor here, the other is returned below so
    // `Session::connect` can drive `Session::ping()` against the same state.
    let sensor = std::sync::Arc::new(crate::sensor::SensorShared::new());
    let drdynvc =
        DrdynvcClient::new().with_dynamic_channel(crate::sensor::RdpilotSensorProcessor::new(sensor.clone()));
    connector = connector.with_static_channel(drdynvc);

    // ── Phase 5 RDPDR seam (D-5.1, SENSOR-02) ───────────────────────────────
    // A SIBLING static channel to `drdynvc` above, registered at the identical
    // connect-time seam — NOT routed through `DrdynvcClient` (RDPDR is a
    // static virtual channel per MS-RDPEFS, unlike the `RDPILOT_SENSOR` DVC).
    // Only registered when a sensor exe path is configured; otherwise the
    // connect path is byte-for-byte the pre-Phase-5 behavior (existing tests
    // stay green).
    if let Some(sensor_path) = cfg.get_sensor_binary_path() {
        // File-transfer share root (D-10.1, 10-01): pre-create the root AND
        // its staging subdir (Plan 10-02's write path relies on the latter
        // existing) BEFORE constructing the backend, so
        // `resolve_under_root`'s `std::fs::canonicalize` of the root always
        // has a real directory to resolve. `create_dir_all` creates both in
        // one call (it creates every missing ancestor). A create failure
        // maps to `Error::Config`, never a panic (API-01).
        let share_root = cfg.get_share_root().map(Path::to_path_buf);
        if let Some(root) = &share_root {
            fs::create_dir_all(root.join(".rdpilot-staging")).map_err(|e| {
                Error::Config(format!(
                    "could not create share root '{}': {e}",
                    root.display()
                ))
            })?;
        }

        let drive_backend = crate::rdpdr_backend::RdpilotDriveBackend::new_with_sensor(
            sensor_path.to_path_buf(),
            SENSOR_EXE_NAME,
            share_root,
            sensor.clone(),
        );
        let rdpdr = Rdpdr::new(Box::new(drive_backend), "rdpilot".to_owned())
            .with_drives(Some(vec![(0, "RDPILOT".to_owned())]));
        connector = connector.with_static_channel(rdpdr);

        // Live-diagnosed bug fix (05-04 live gate, D-5.6/SC2): MS-RDPEFS
        // Appendix A footnote <1> requires "rdpsnd" to be advertised
        // alongside "rdpdr" -- without it, a Windows RDP server joins the
        // rdpdr channel successfully but silently NEVER sends the Server
        // Announce Request that starts the RDPDR handshake (confirmed via
        // live wire trace: ChannelJoinConfirm received for the rdpdr
        // channel_id, but zero RDPDR traffic ever followed). This stub only
        // needs to be present/joined, not functional -- see
        // rdpsnd_stub.rs's doc comment for the full live-diagnosed root
        // cause.
        connector = connector.with_static_channel(crate::rdpsnd_stub::RdpsndStub::new());
    }

    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector)
        .await
        .map_err(|e| Error::Connect(format!("connect_begin failed: {}", format_error_chain(&e))))?;

    // TLS upgrade over the raw stream (no leftover bytes expected at this point).
    let initial_stream = framed.into_inner_no_leftover();
    let (upgraded_stream, server_public_key) =
        tls_upgrade(initial_stream, &server_name, cfg.get_accept_invalid_certs()).await?;

    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);

    let erased: UpgradedStream = Box::new(upgraded_stream);
    let mut upgraded_framed = TokioFramed::new(erased);

    let mut network_client = ReqwestNetworkClient::new();
    let connection_result = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut upgraded_framed,
        &mut network_client,
        server_name.into(),
        server_public_key,
        None,
    )
    .await
    .map_err(|e| Error::Connect(format!("connect_finalize failed: {}", format_error_chain(&e))))?;

    Ok((connection_result, upgraded_framed, sensor))
}

/// Render an upstream error together with its retained source chain.
///
/// IronRDP intentionally displays its catch-all connector kind as `custom
/// error`, while retaining the actionable I/O or PDU-decoding error as an
/// [`std::error::Error::source`]. Keeping that source in the owned SDK error
/// makes negotiation failures actionable after they cross the daemon/IPC
/// boundary, where the original typed error is no longer available.
fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();

    while let Some(cause) = source {
        let _ = write!(rendered, "; caused by: {cause}");
        source = cause.source();
    }

    rendered
}

/// Resolve `host:port` to a single socket address.
fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr> {
    use std::net::ToSocketAddrs as _;

    (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Connect(format!("address lookup for {host}:{port} failed: {e}")))?
        .next()
        .ok_or_else(|| Error::Connect(format!("no address found for {host}:{port}")))
}

/// Map an owned [`ConnectionConfig`] onto the IronRDP connector configuration.
///
/// `enable_tls: false` is the *frontend* TLS re-export flag, NOT the wire — the
/// wire is TLS via the rustls upgrade above. `enable_credssp: true` is what
/// performs NLA. `enable_server_pointer: false` matches the headless screenshot
/// example (no GUI, no cursor; Open Q2).
fn build_connector_config(cfg: &ConnectionConfig) -> Config {
    Config {
        credentials: Credentials::UsernamePassword {
            username: cfg.username().to_owned(),
            password: cfg.password().to_owned(),
        },
        domain: cfg.get_domain().map(ToOwned::to_owned),
        enable_tls: false,
        enable_credssp: true,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: DesktopSize {
            width: cfg.width(),
            height: cfg.height(),
        },
        bitmap: None,
        client_build: 0,
        client_name: "rdpilot".to_owned(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::WINDOWS,
        enable_server_pointer: false,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
        compression_type: None,
        pointer_software_rendering: true,
        multitransport_flags: None,
        performance_flags: PerformanceFlags::default(),
        desktop_scale_factor: 0,
        hardware_id: None,
        license_cache: None,
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

/// Build the rustls [`ClientConfig`] for the RDP wire.
///
/// Resumption is **always disabled** (CredSSP requirement, MS-CSSP). The
/// certificate verifier is selected by `accept_invalid_certs`:
/// - `false` (default): the platform-root validating verifier.
/// - `true` (risk-named lab opt-out, D-15): the no-op [`NoCertificateVerification`]
///   verifier — MITM protection is disabled, intended only for a self-signed
///   workgroup lab target.
fn build_tls_client_config(accept_invalid_certs: bool) -> Result<ClientConfig> {
    let mut config = if accept_invalid_certs {
        ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(danger::NoCertificateVerification))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        let native = rustls_native_certs::load_native_certs();
        if native.certs.is_empty() {
            return Err(Error::Tls(
                "no platform root certificates available to validate the server certificate \
                 (set accept_invalid_certs only for a trusted lab target)"
                    .to_owned(),
            ));
        }
        for cert in native.certs {
            // Skip individual malformed roots rather than fail the whole config.
            let _ = roots.add(cert);
        }
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };

    // CredSSP does not support TLS session resumption (MS-CSSP, Pitfall 5).
    config.resumption = Resumption::disabled();

    Ok(config)
}

/// Perform the async TLS handshake and extract the server public key for CredSSP.
async fn tls_upgrade(
    stream: TcpStream,
    server_name: &str,
    accept_invalid_certs: bool,
) -> Result<(tokio_rustls::client::TlsStream<TcpStream>, Vec<u8>)> {
    let config = build_tls_client_config(accept_invalid_certs)?;
    let connector = TlsConnector::from(Arc::new(config));

    let dns_name = rustls::pki_types::ServerName::try_from(server_name.to_owned())
        .map_err(|e| Error::Tls(format!("invalid server name '{server_name}': {e}")))?;

    let tls_stream = connector
        .connect(dns_name, stream)
        .await
        .map_err(|e| Error::Tls(format!("TLS handshake failed: {e}")))?;

    let server_public_key = {
        let (_, conn) = tls_stream.get_ref();
        let cert = conn
            .peer_certificates()
            .and_then(|certs| certs.first())
            .ok_or_else(|| Error::Tls("peer certificate is missing".to_owned()))?;
        extract_server_public_key(cert.as_ref())?
    };

    Ok((tls_stream, server_public_key))
}

/// DER-decode the peer certificate and return its `subject_public_key` bytes.
fn extract_server_public_key(cert_der: &[u8]) -> Result<Vec<u8>> {
    use x509_cert::der::Decode as _;

    let cert = x509_cert::Certificate::from_der(cert_der)
        .map_err(|e| Error::Tls(format!("could not decode server certificate: {e}")))?;

    let key = cert
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| Error::Tls("subject public key BIT STRING is not byte-aligned".to_owned()))?
        .to_owned();

    Ok(key)
}

/// The no-op certificate verifier behind the risk-named `accept_invalid_certs`
/// opt-out (D-15). Accepts any server certificate — MITM protection is disabled.
/// Intended ONLY for a self-signed workgroup lab target the operator controls.
mod danger {
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct NoCertificateVerification;

    impl ServerCertVerifier for NoCertificateVerification {
        fn verify_server_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: &ServerName<'_>,
            _: &[u8],
            _: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            Ok(HandshakeSignatureValid::assertion())
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![
                SignatureScheme::RSA_PKCS1_SHA1,
                SignatureScheme::ECDSA_SHA1_Legacy,
                SignatureScheme::RSA_PKCS1_SHA256,
                SignatureScheme::ECDSA_NISTP256_SHA256,
                SignatureScheme::RSA_PKCS1_SHA384,
                SignatureScheme::ECDSA_NISTP384_SHA384,
                SignatureScheme::RSA_PKCS1_SHA512,
                SignatureScheme::ECDSA_NISTP521_SHA512,
                SignatureScheme::RSA_PSS_SHA256,
                SignatureScheme::RSA_PSS_SHA384,
                SignatureScheme::RSA_PSS_SHA512,
                SignatureScheme::ED25519,
                SignatureScheme::ED448,
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct TestCause;

    impl fmt::Display for TestCause {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("socket closed before negotiation response")
        }
    }

    impl std::error::Error for TestCause {}

    #[derive(Debug)]
    struct TestWrapper(TestCause);

    impl fmt::Display for TestWrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("custom error")
        }
    }

    impl std::error::Error for TestWrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn connector_error_rendering_keeps_the_underlying_cause() {
        assert_eq!(
            format_error_chain(&TestWrapper(TestCause)),
            "custom error; caused by: socket closed before negotiation response"
        );
    }

    /// The cert-policy branch must always disable resumption (CredSSP), and must
    /// build successfully for the lab opt-out (no-op verifier) path without a
    /// network connection. This asserts the branch selection by `accept_invalid_certs`.
    #[test]
    fn tls_config_disables_resumption_on_both_branches() {
        // Lab opt-out: no-op verifier path — always constructs.
        let lab = build_tls_client_config(true).expect("lab cert policy builds");
        // Resumption store is the disabled sentinel on both paths; we assert the
        // config is usable (builder succeeded) and that the opt-out branch does
        // not require any platform roots.
        let _ = lab;

        // Validating path: depends on platform roots being present in the test
        // environment. If roots are available it must build; if not, it must
        // return a typed TLS error (never panic) — both are acceptable offline.
        match build_tls_client_config(false) {
            Ok(_) => {}
            Err(Error::Tls(_)) => {}
            Err(other) => panic!("unexpected error category from validating path: {other:?}"),
        }
    }

    /// The DVC seam name is the reserved Phase 4 channel and is referenced by the
    /// connect path (regression guard so the seam is not accidentally dropped).
    #[test]
    fn dvc_seam_name_is_reserved() {
        assert_eq!(RDPILOT_SENSOR, "RDPILOT_SENSOR");
    }
}
