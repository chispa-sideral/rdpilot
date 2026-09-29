//! ssh-style pattern matching for `Host` lines (port of OpenSSH's
//! `match_pattern` / `match_hostname`).
//!
//! As in ssh, the *target host is lowercased* and patterns are matched *as
//! written*: a pattern containing capital letters never matches.

/// `*` matches any run of characters, `?` exactly one; everything else is literal.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || (p[pi] != '*' && p[pi] == t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Does a `Host` line with these patterns apply to `host`? A `!pattern` that
/// matches rejects the whole line; otherwise at least one positive pattern
/// must match. A line of only negated patterns never matches.
pub(crate) fn host_line_matches(patterns: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let mut positive = false;
    for pat in patterns {
        if let Some(neg) = pat.strip_prefix('!') {
            if glob_match(neg, &host) {
                return false;
            }
        } else if glob_match(pat, &host) {
            positive = true;
        }
    }
    positive
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn pats(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn wildcards() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(glob_match("web?", "web1"));
        assert!(!glob_match("web?", "web"));
        assert!(glob_match("*.corp.example", "a.b.corp.example"));
        assert!(glob_match("a*b*c", "aXXbYYc"));
        assert!(!glob_match("a*b*c", "aXXbYY"));
        assert!(glob_match("10.0.*", "10.0.5.7"));
    }

    #[test]
    fn negation_wins_and_negation_only_never_matches() {
        assert!(host_line_matches(&pats(&["*", "!secret*"]), "web1"));
        assert!(!host_line_matches(&pats(&["*", "!secret*"]), "secret1"));
        assert!(!host_line_matches(&pats(&["!secret*"]), "web1"));
        assert!(!host_line_matches(&pats(&["!a"]), "a"));
    }

    #[test]
    fn host_is_lowercased_and_patterns_match_as_written() {
        assert!(host_line_matches(&pats(&["devbox"]), "devbox"));
        assert!(host_line_matches(&pats(&["devbox"]), "DEVBOX"));
        assert!(!host_line_matches(&pats(&["DevBox"]), "devbox"));
        assert!(!host_line_matches(&pats(&["DevBox"]), "DEVBOX"));
        assert!(host_line_matches(&pats(&["*"]), "DEVBOX"));
    }
}
