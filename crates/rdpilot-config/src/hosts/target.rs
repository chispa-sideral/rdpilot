//! Connection targets: a host alias or an `rdp://` / `rdps://` URL.

use std::fmt;

use percent_encoding::percent_decode_str;

use super::{HostsError, Secret};

/// What the user typed after `connect`.
#[derive(Debug, Clone)]
pub enum Target {
    /// A name looked up in the hosts file (or used as the address).
    Alias(String),
    /// An RDP URL; its components are the highest-precedence settings.
    Url(UrlTarget),
}

/// The parts of an `rdp://[domain\]user[:password]@host[:port]` URL.
#[derive(Debug, Clone)]
pub struct UrlTarget {
    /// `rdp` or `rdps` (identical behaviour).
    pub scheme: String,
    pub user: Option<String>,
    pub domain: Option<String>,
    pub password: Option<Secret>,
    /// Host as written (no IPv6 brackets, original case).
    pub host: String,
    pub port: Option<u16>,
}

impl Target {
    /// Parse a target argument.
    ///
    /// # Errors
    ///
    /// A malformed URL, an unsupported scheme, or a URL with a path, query or
    /// fragment. Messages never contain the URL's password.
    pub fn parse(input: &str) -> Result<Self, HostsError> {
        if input.is_empty() {
            return Err(HostsError::new("the target is empty"));
        }
        let Some((scheme, _)) = input.split_once("://") else {
            return Ok(Target::Alias(input.to_owned()));
        };
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "rdp" && scheme != "rdps" {
            return Err(HostsError::new(format!(
                "unsupported URL scheme \"{scheme}\" (expected rdp:// or rdps://)"
            )));
        }
        let url =
            url::Url::parse(input).map_err(|e| HostsError::new(format!("invalid RDP URL: {e}")))?;
        let host = match url.host() {
            Some(url::Host::Ipv6(a)) => a.to_string(),
            Some(url::Host::Ipv4(a)) => a.to_string(),
            Some(url::Host::Domain(d)) if !d.is_empty() => decode(d)?,
            _ => return Err(HostsError::new("the RDP URL has no host")),
        };
        let raw_user = decode(url.username())?;
        let (domain, user) = match raw_user.split_once('\\') {
            Some((d, u)) => (non_empty(d), non_empty(u)),
            None => (None, non_empty(&raw_user)),
        };
        let password = match url.password() {
            Some(p) => non_empty(&decode(p)?).map(Secret::new),
            None => None,
        };
        let parsed = UrlTarget {
            scheme,
            user,
            domain,
            password,
            host,
            port: url.port(),
        };
        let redacted = parsed.to_string();
        if !url.path().is_empty() {
            return Err(HostsError::new(format!(
                "{redacted}: the RDP URL must not have a path (not even a trailing /)"
            )));
        }
        if url.query().is_some() {
            return Err(HostsError::new(format!(
                "{redacted}: the RDP URL must not have a query"
            )));
        }
        if url.fragment().is_some() {
            return Err(HostsError::new(format!(
                "{redacted}: the RDP URL must not have a fragment"
            )));
        }
        Ok(Target::Url(parsed))
    }

    /// The name `Host` patterns match and `%n` expands to: the alias, or the
    /// URL's hostname.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Target::Alias(a) => a,
            Target::Url(u) => &u.host,
        }
    }
}

fn decode(s: &str) -> Result<String, HostsError> {
    percent_decode_str(s)
        .decode_utf8()
        .map(std::borrow::Cow::into_owned)
        .map_err(|_| HostsError::new("the RDP URL contains percent-encoding that is not UTF-8"))
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

/// Redacted form: `rdp://[domain\]user[:***]@host[:port]`.
impl fmt::Display for UrlTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://", self.scheme)?;
        if self.user.is_some() || self.domain.is_some() || self.password.is_some() {
            if let Some(d) = &self.domain {
                write!(f, "{d}\\")?;
            }
            if let Some(u) = &self.user {
                write!(f, "{u}")?;
            }
            if self.password.is_some() {
                write!(f, ":***")?;
            }
            write!(f, "@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            write!(f, "{}", self.host)?;
        }
        if let Some(p) = self.port {
            write!(f, ":{p}")?;
        }
        Ok(())
    }
}

/// Redacted: an alias is shown as is, a URL without its password.
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Alias(a) => write!(f, "{a}"),
            Target::Url(u) => write!(f, "{u}"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn url(s: &str) -> UrlTarget {
        match Target::parse(s).unwrap() {
            Target::Url(u) => u,
            Target::Alias(a) => panic!("alias {a}"),
        }
    }

    fn err(s: &str) -> String {
        Target::parse(s).unwrap_err().to_string()
    }

    #[test]
    fn alias() {
        assert!(matches!(Target::parse("web1").unwrap(), Target::Alias(a) if a == "web1"));
        assert!(Target::parse("").is_err());
    }

    #[test]
    fn full_url() {
        let u = url("rdp://CORP%5Calice:p%40ss@Host.Example:3390");
        assert_eq!(u.scheme, "rdp");
        assert_eq!(u.domain.as_deref(), Some("CORP"));
        assert_eq!(u.user.as_deref(), Some("alice"));
        assert_eq!(u.password.as_ref().map(Secret::expose), Some("p@ss"));
        assert_eq!(u.host, "Host.Example");
        assert_eq!(u.port, Some(3390));
        assert_eq!(u.to_string(), "rdp://CORP\\alice:***@Host.Example:3390");
    }

    #[test]
    fn rdps_is_the_same_and_minimal_forms() {
        let u = url("rdps://10.0.0.5");
        assert_eq!(u.scheme, "rdps");
        assert_eq!(u.host, "10.0.0.5");
        assert!(u.user.is_none() && u.password.is_none() && u.port.is_none());
        assert_eq!(url("rdp://bob@h").user.as_deref(), Some("bob"));
        assert!(url("rdp://@h").user.is_none());
    }

    #[test]
    fn ipv6_host() {
        let u = url("rdp://u@[fe80::1]:3390");
        assert_eq!(u.host, "fe80::1");
        assert_eq!(u.to_string(), "rdp://u@[fe80::1]:3390");
    }

    #[test]
    fn every_path_query_fragment_is_rejected() {
        for bad in [
            "rdp://u@host/",
            "rdps://u@host/",
            "rdp://u@host/x",
            "rdp://u@host/x/y",
            "rdp://u@host?",
            "rdp://u@host?a=b",
            "rdp://u@host#",
            "rdp://u@host#f",
            "rdp://u@host/?",
        ] {
            assert!(Target::parse(bad).is_err(), "{bad} must be rejected");
        }
        assert!(Target::parse("rdp://u@host").is_ok());
    }

    #[test]
    fn errors_never_show_the_password() {
        let e = err("rdp://u:hunter2@host/x");
        assert!(!e.contains("hunter2"), "{e}");
        assert!(e.contains("u:***@host"), "{e}");
        assert!(!err("rdp://u:hunter2@host:99999").contains("hunter2"));
        assert!(!err("rdp://u:hunter2@host?q").contains("hunter2"));
    }

    #[test]
    fn bad_urls() {
        assert!(err("http://host").contains("unsupported URL scheme"));
        assert!(err("rdp://").contains("no host"));
        assert!(Target::parse("rdp://u@host:99999").is_err());
        assert!(Target::parse("rdp://%ff@host").is_err());
    }

    #[test]
    fn name_is_the_hostname_never_the_url() {
        assert_eq!(Target::parse("rdp://u:p@Host").unwrap().name(), "Host");
        assert_eq!(Target::parse("web1").unwrap().name(), "web1");
    }
}
