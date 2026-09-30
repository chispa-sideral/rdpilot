//! Minimal HTTPS client for GitHub release metadata and assets.
//!
//! Base URLs are constructor arguments so tests can point the client at an
//! in-process stub server; they are never read from configuration.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Connect timeout for every request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Overall timeout for a metadata request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Overall timeout for an asset download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest metadata or checksum body accepted.
const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;
/// Largest asset accepted.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// Why a fetch failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FetchError {
    /// HTTP 404: the release or asset does not exist.
    NotFound,
    /// Network failure, timeout, or any other HTTP status: GitHub could not
    /// answer, which says nothing about whether the release exists.
    Unavailable(String),
}

/// Where release metadata and assets come from.
#[derive(Debug, Clone)]
pub(crate) struct Endpoints {
    /// REST API root, `https://api.github.com` in production.
    pub api: String,
    /// Web root for `releases/download/<tag>/<asset>`, `https://github.com`
    /// in production.
    pub web: String,
}

impl Endpoints {
    pub(crate) fn github() -> Self {
        Self {
            api: "https://api.github.com".to_owned(),
            web: "https://github.com".to_owned(),
        }
    }

    /// `<web>/<repo>/releases/download/<tag>/<asset>`.
    pub(crate) fn asset_url(&self, repo: &str, tag: &str, asset: &str) -> String {
        format!("{}/{repo}/releases/download/{tag}/{asset}", self.web)
    }

    /// `<api>/repos/<repo>/releases?per_page=100`.
    pub(crate) fn releases_url(&self, repo: &str) -> String {
        format!("{}/repos/{repo}/releases?per_page=100", self.api)
    }
}

pub(crate) struct Http {
    client: reqwest::Client,
    pub(crate) endpoints: Endpoints,
}

impl Http {
    /// `use_env_proxy` honours `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` (and the
    /// system proxy on Windows and macOS). The daemon reads them once, at
    /// start.
    pub(crate) fn new(endpoints: Endpoints, use_env_proxy: bool) -> Result<Self, String> {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            // Skip individual malformed roots rather than fail the client.
            let _ = roots.add(cert);
        }
        // The SDK's RDP TLS uses the aws-lc-rs provider; use the same one
        // explicitly so no second crypto provider is linked or selected.
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS setup failed: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let mut builder = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("rdpilot/", env!("CARGO_PKG_VERSION")));
        if !use_env_proxy {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|e| format!("HTTP client setup failed: {e}"))?;
        Ok(Self { client, endpoints })
    }

    async fn get(&self, url: &str, timeout: Duration) -> Result<reqwest::Response, FetchError> {
        let response = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json, */*")
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| FetchError::Unavailable(describe(&e)))?;
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::NOT_FOUND => Err(FetchError::NotFound),
            status => Err(FetchError::Unavailable(format!(
                "HTTP {status} from {}",
                host_of(url)
            ))),
        }
    }

    /// A small text body (release lists, checksum files).
    pub(crate) async fn text(&self, url: &str) -> Result<(String, Option<String>), FetchError> {
        let mut response = self.get(url, REQUEST_TIMEOUT).await?;
        let next = next_link(response.headers());
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Unavailable(describe(&e)))?
        {
            if body.len() + chunk.len() > MAX_TEXT_BYTES {
                return Err(FetchError::Unavailable(format!(
                    "response from {} is too large",
                    host_of(url)
                )));
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(body).map_err(|_| {
            FetchError::Unavailable(format!("response from {} is not UTF-8", host_of(url)))
        })?;
        Ok((text, next))
    }

    /// Stream an asset into `dest` (created, must not exist) and return its
    /// lower-case hex SHA-256.
    pub(crate) async fn download(&self, url: &str, dest: &Path) -> Result<String, FetchError> {
        let mut response = self.get(url, DOWNLOAD_TIMEOUT).await?;
        let io = |e: std::io::Error| FetchError::Unavailable(format!("cannot write download: {e}"));
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dest)
            .await
            .map_err(io)?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| FetchError::Unavailable(describe(&e)))?
        {
            total += chunk.len() as u64;
            if total > MAX_DOWNLOAD_BYTES {
                return Err(FetchError::Unavailable(format!(
                    "download from {} is too large",
                    host_of(url)
                )));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(io)?;
        }
        file.flush().await.map_err(io)?;
        file.sync_all().await.map_err(io)?;
        Ok(hex(&hasher.finalize()))
    }
}

/// Lower-case hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
}

/// The error and its sources, without the URL query.
fn describe(error: &reqwest::Error) -> String {
    let mut text = if error.is_timeout() {
        "timed out".to_owned()
    } else if error.is_connect() {
        "connection failed".to_owned()
    } else {
        "request failed".to_owned()
    };
    if let Some(url) = error.url() {
        text.push_str(&format!(" ({})", url.host_str().unwrap_or("?")));
    }
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        text.push_str(&format!(": {inner}"));
        source = inner.source();
    }
    text
}

/// The `rel="next"` target of a GitHub `Link` header.
fn next_link(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let link = headers.get(reqwest::header::LINK)?.to_str().ok()?;
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        params
            .split(';')
            .any(|p| p.trim() == r#"rel="next""#)
            .then(|| {
                url.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_owned()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_link_finds_the_next_page() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::LINK,
            r#"<https://api.example/r?page=2>; rel="next", <https://api.example/r?page=5>; rel="last""#
                .parse()
                .unwrap(),
        );
        assert_eq!(
            next_link(&headers).as_deref(),
            Some("https://api.example/r?page=2")
        );
        headers.insert(
            reqwest::header::LINK,
            r#"<https://api.example/r?page=1>; rel="prev""#.parse().unwrap(),
        );
        assert_eq!(next_link(&headers), None);
    }

    #[test]
    fn hex_is_lower_case() {
        assert_eq!(hex(&[0xab, 0x01]), "ab01");
    }
}
