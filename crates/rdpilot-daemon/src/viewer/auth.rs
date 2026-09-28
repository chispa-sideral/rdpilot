//! Request checks for the live viewer, in this order: Host (DNS-rebinding
//! guard) -> Origin and `Sec-Fetch-Site` (cross-origin guard) -> token ->
//! method. Every failure is a bare 403 (405 for a method other than GET
//! after the other checks pass). Nothing here logs.

use std::net::SocketAddr;

use hyper::http::request::Parts;
use hyper::{header, Method, StatusCode};
use subtle::ConstantTimeEq;

/// Number of random bytes in a token (64 hex characters).
const TOKEN_BYTES: usize = 32;

/// The per-start access token. `Debug` is redacted; the value is compared
/// in constant time.
#[derive(Clone)]
pub(crate) struct Token(String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

impl Token {
    /// 32 bytes from the OS CSPRNG, hex-encoded.
    pub(crate) fn generate() -> Result<Self, String> {
        let mut bytes = [0_u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
        Ok(Token(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    #[cfg(test)]
    pub(crate) fn from_test_value(value: &str) -> Self {
        Token(value.to_owned())
    }

    /// The token text, for the owner-only IPC response only.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    fn matches(&self, candidate: &str) -> bool {
        bool::from(self.0.as_bytes().ct_eq(candidate.as_bytes()))
    }
}

/// The checks every request must pass.
pub(crate) struct AuthPolicy {
    token: Token,
    /// Allowed `host:port` authorities, lowercase.
    authorities: Vec<String>,
}

impl AuthPolicy {
    /// Allow exactly the bound socket authorities, plus `localhost:<port>`
    /// for the loopback address.
    pub(crate) fn new(token: Token, bound: &[SocketAddr]) -> Self {
        let mut authorities = Vec::new();
        for addr in bound {
            authorities.push(addr.to_string());
            if addr.ip().is_loopback() {
                authorities.push(format!("localhost:{}", addr.port()));
            }
        }
        AuthPolicy { token, authorities }
    }

    fn authority_allowed(&self, value: &str) -> bool {
        let value = value.to_ascii_lowercase();
        self.authorities.iter().any(|a| *a == value)
    }

    /// Check one request. `Ok(())` means it may be routed.
    pub(crate) fn check(&self, parts: &Parts) -> Result<(), StatusCode> {
        // 1. Host: exact match against a bound authority.
        let host = header_str(parts, header::HOST).ok_or(StatusCode::FORBIDDEN)?;
        if !self.authority_allowed(host) {
            return Err(StatusCode::FORBIDDEN);
        }

        // 2. Origin, when present, must be this viewer's own origin.
        if let Some(origin) = header_str(parts, header::ORIGIN) {
            let allowed = origin
                .strip_prefix("http://")
                .is_some_and(|authority| self.authority_allowed(authority));
            if !allowed {
                return Err(StatusCode::FORBIDDEN);
            }
        }

        // 2b. Sec-Fetch-Site: only same-origin or a direct navigation, except
        // that a top-level document navigation to `/` may come from another
        // site (a user clicking the printed URL in a chat or web page). The
        // token and Host checks still apply to it, and `frame-ancestors
        // 'none'` prevents framing.
        let path = parts.uri.path();
        if let Some(site) = header_str(parts, "sec-fetch-site") {
            let same = matches!(site, "same-origin" | "none");
            let document_navigation = path == "/"
                && parts.method == Method::GET
                && header_str(parts, "sec-fetch-mode") == Some("navigate")
                && header_str(parts, "sec-fetch-dest") == Some("document");
            if !same && !document_navigation {
                return Err(StatusCode::FORBIDDEN);
            }
        }

        // 3. Token: the document takes it from `?token=`; the API only from
        // `Authorization: Bearer`.
        let presented = if path == "/" {
            query_param(parts.uri.query(), "token")
        } else {
            header_str(parts, header::AUTHORIZATION).and_then(|v| v.strip_prefix("Bearer "))
        };
        if !presented.is_some_and(|t| self.token.matches(t)) {
            return Err(StatusCode::FORBIDDEN);
        }

        // 4. Method: read-only.
        if parts.method != Method::GET {
            return Err(StatusCode::METHOD_NOT_ALLOWED);
        }
        Ok(())
    }
}

fn header_str(parts: &Parts, name: impl header::AsHeaderName) -> Option<&str> {
    parts.headers.get(name).and_then(|v| v.to_str().ok())
}

/// The first value of `key` in a query string (no decoding: tokens and
/// sequence numbers are plain ASCII).
pub(crate) fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}
