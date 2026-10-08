//! Resolution: URL components, then `-o` options, then the hosts file, then
//! the built-in default. The first value obtained for each setting wins.

use std::fmt;
use std::path::{Path, PathBuf};

use super::keywords::{self, Keyword, Kind, KEYWORDS};
use super::parse::{command_value, split_line, words};
use super::pattern::host_line_matches;
use super::target::Target;
use super::{HostsError, Secret, DEFAULT_PORT};

/// Built-in default, read after every file.
const BUILT_IN_DEFAULT: &str =
    "Host *\n  CuaEnabled yes\n  CuaVersion latest-dev\n  CuaAutoDownload yes\n";
/// Deepest `Include` nesting before an error (as in ssh).
const MAX_INCLUDE_DEPTH: usize = 16;

/// Where a setting's value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A component of the `rdp://` URL.
    Url,
    /// The Nth (1-based) `-o` option.
    Option(usize),
    /// A hosts file line.
    File {
        /// The file.
        path: PathBuf,
        /// 1-based line number.
        line: usize,
    },
    /// The built-in default.
    Default,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Url => write!(f, "url"),
            Source::Option(n) => write!(f, "-o #{n}"),
            Source::File { path, line } => write!(f, "{}:{line}", path.display()),
            Source::Default => write!(f, "built-in default"),
        }
    }
}

/// One resolved setting.
#[derive(Clone)]
pub struct Setting {
    kw: &'static Keyword,
    value: String,
    /// Where the value came from.
    pub source: Source,
}

impl Setting {
    /// Canonical keyword name.
    #[must_use]
    pub fn keyword(&self) -> &'static str {
        self.kw.name
    }

    /// The value as written; a password is not redacted here.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// The value with credentials replaced by `<redacted>`.
    #[must_use]
    pub fn display_value(&self) -> &str {
        if self.kw.secret {
            "<redacted>"
        } else {
            &self.value
        }
    }
}

impl fmt::Debug for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Setting")
            .field("keyword", &self.kw.name)
            .field("value", &self.display_value())
            .field("source", &self.source)
            .finish()
    }
}

/// A line of `config resolve` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Keyword.
    pub keyword: &'static str,
    /// Redacted value.
    pub value: String,
    /// Source description.
    pub source: String,
}

/// Inputs to [`resolve_host`].
#[derive(Debug, Clone, Copy)]
pub struct HostsInput<'a> {
    /// The `connect` target argument.
    pub target: &'a str,
    /// `-o` options, in order.
    pub options: &'a [String],
    /// `-F`: read this file instead of the user's hosts file. Must exist.
    pub file: Option<&'a Path>,
    /// Directory holding the user's `hosts` file; relative `Include` paths
    /// resolve here. `None` (no home directory): no user file is read.
    pub config_dir: Option<&'a Path>,
    /// Home directory for `~` in `Include` paths.
    pub home: Option<&'a Path>,
}

/// The result of resolution. No `Serialize`; `Debug` redacts credentials.
#[derive(Debug, Clone)]
pub struct ResolvedHost {
    target: Target,
    host_name: String,
    settings: Vec<Setting>,
}

impl ResolvedHost {
    /// The parsed target.
    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The address to connect to: `HostName`, else the target name.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.host_name
    }

    fn find(&self, name: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.kw.name == name)
    }

    /// `Port`, if set.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.find("Port").and_then(|s| s.value.parse().ok())
    }

    /// `User`, if set.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.find("User").map(|s| s.value.as_str())
    }

    /// `Domain`, if set.
    #[must_use]
    pub fn domain(&self) -> Option<&str> {
        self.find("Domain").map(|s| s.value.as_str())
    }

    /// The password, if a literal `Password` won the credential slot.
    #[must_use]
    pub fn password(&self) -> Option<Secret> {
        self.find("Password").map(|s| Secret::new(s.value.clone()))
    }

    /// The command as written, if `PasswordCommand` won the credential slot.
    #[must_use]
    pub fn password_command(&self) -> Option<&str> {
        self.find("PasswordCommand").map(|s| s.value.as_str())
    }

    /// `AcceptInvalidCerts` (default no).
    #[must_use]
    pub fn accept_invalid_certs(&self) -> bool {
        self.find("AcceptInvalidCerts")
            .is_some_and(|s| s.value == "yes")
    }

    /// `CuaEnabled` (default yes).
    #[must_use]
    pub fn cua_enabled(&self) -> bool {
        !matches!(self.find("CuaEnabled"), Some(s) if s.value == "no")
    }

    /// `CuaVersion` (default `latest-dev`).
    #[must_use]
    pub fn cua_version(&self) -> &str {
        self.find("CuaVersion")
            .map_or("latest-dev", |s| s.value.as_str())
    }

    /// `CuaAutoDownload` (default yes).
    #[must_use]
    pub fn cua_auto_download(&self) -> bool {
        !matches!(self.find("CuaAutoDownload"), Some(s) if s.value == "no")
    }

    /// Every setting that has a value, in keyword-table order.
    #[must_use]
    pub fn settings(&self) -> &[Setting] {
        &self.settings
    }

    /// Rows for `config resolve`: every setting with a value, plus the
    /// resolved address and the port when not set.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.find("HostName").is_none() {
            rows.push(Row {
                keyword: "HostName",
                value: self.host_name.clone(),
                source: "target name".to_owned(),
            });
        }
        if self.find("Port").is_none() {
            rows.push(Row {
                keyword: "Port",
                value: DEFAULT_PORT.to_string(),
                source: "not set; rdpilot default".to_owned(),
            });
        }
        for s in &self.settings {
            rows.push(Row {
                keyword: s.kw.name,
                value: s.display_value().to_owned(),
                source: s.source.to_string(),
            });
        }
        rows.sort_by_key(|r| keywords::lookup(r.keyword).map_or(usize::MAX, keywords::index_of));
        rows
    }
}

enum Origin<'a> {
    File(&'a Path),
    Default,
}

impl Origin<'_> {
    fn label(&self) -> String {
        match self {
            Origin::File(p) => p.display().to_string(),
            Origin::Default => "built-in default".to_owned(),
        }
    }
}

struct Collector<'a> {
    name: &'a str,
    config_dir: Option<&'a Path>,
    home: Option<&'a Path>,
    settings: Vec<Setting>,
}

impl Collector<'_> {
    /// Keep the first value obtained for a slot.
    fn offer(&mut self, kw: &'static Keyword, value: String, source: Source) {
        if !self.settings.iter().any(|s| s.kw.slot == kw.slot) {
            self.settings.push(Setting { kw, value, source });
        }
    }

    fn load_text(
        &mut self,
        text: &str,
        origin: &Origin<'_>,
        stack: &mut Vec<PathBuf>,
    ) -> Result<(), HostsError> {
        let mut active = true; // lines before the first Host apply to every host
        for (idx, line) in text.lines().enumerate() {
            let number = idx + 1;
            let at = |msg: String| HostsError::new(format!("{}:{number}: {msg}", origin.label()));
            let Some((word, rest)) = split_line(line) else {
                continue;
            };
            if word.eq_ignore_ascii_case("Host") {
                let pats = words(rest).map_err(at)?;
                if pats.is_empty() {
                    return Err(at("Host needs at least one pattern".to_owned()));
                }
                active = host_line_matches(&pats, self.name);
                continue;
            }
            if word.eq_ignore_ascii_case("Match") {
                return Err(at("Match is not supported".to_owned()));
            }
            let Some(kw) = keywords::lookup(word) else {
                return Err(at(format!("unknown keyword \"{word}\"")));
            };
            let source = match origin {
                Origin::File(p) => Source::File {
                    path: p.to_path_buf(),
                    line: number,
                },
                Origin::Default => Source::Default,
            };
            match kw.kind {
                Kind::Include => {
                    let pats = words(rest).map_err(at)?;
                    if pats.is_empty() {
                        return Err(at("Include needs a path".to_owned()));
                    }
                    if active {
                        for pat in pats {
                            self.include(&pat, stack).map_err(|e| at(e.to_string()))?;
                        }
                    }
                }
                Kind::Command => {
                    let value = keywords::validate(kw, &command_value(rest)).map_err(at)?;
                    if active {
                        self.offer(kw, value, source);
                    }
                }
                _ => {
                    let mut w = words(rest).map_err(at)?;
                    if w.len() != 1 {
                        return Err(at(format!(
                            "{} expects exactly one value (quote values that contain spaces)",
                            kw.name
                        )));
                    }
                    let value = keywords::validate(kw, &w.remove(0)).map_err(at)?;
                    if active {
                        self.offer(kw, value, source);
                    }
                }
            }
        }
        Ok(())
    }

    fn include(&mut self, pat: &str, stack: &mut Vec<PathBuf>) -> Result<(), HostsError> {
        let expanded = match (pat.strip_prefix("~/"), self.home) {
            (Some(tail), Some(home)) => home.join(tail),
            _ => PathBuf::from(pat),
        };
        let full = match self.config_dir {
            Some(dir) if !expanded.is_absolute() => dir.join(expanded),
            _ => expanded,
        };
        let matches = glob::glob(&full.to_string_lossy())
            .map_err(|e| HostsError::new(format!("invalid Include pattern: {e}")))?;
        let mut files: Vec<PathBuf> = matches
            .filter_map(Result::ok)
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        for file in files {
            let canon = std::fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            if stack.contains(&canon) {
                return Err(HostsError::new(format!(
                    "Include cycle: {} is already being read",
                    file.display()
                )));
            }
            if stack.len() >= MAX_INCLUDE_DEPTH {
                return Err(HostsError::new(format!(
                    "Include nested more than {MAX_INCLUDE_DEPTH} levels deep at {}",
                    file.display()
                )));
            }
            let text = std::fs::read_to_string(&file)
                .map_err(|e| HostsError::new(format!("cannot read {}: {e}", file.display())))?;
            stack.push(canon);
            let result = self.load_text(&text, &Origin::File(&file), stack);
            stack.pop();
            result?;
        }
        Ok(())
    }

    fn load_file(&mut self, path: &Path, must_exist: bool) -> Result<(), HostsError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !must_exist => return Ok(()),
            Err(e) => {
                return Err(HostsError::new(format!(
                    "cannot read {}: {e}",
                    path.display()
                )))
            }
        };
        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.load_text(&text, &Origin::File(path), &mut vec![canon])
    }
}

/// Resolve `input.target` to its settings.
///
/// # Errors
///
/// Any parse, target, `-o`, `-F`, `Include` or permission error. Messages
/// never contain a password.
pub fn resolve_host(input: &HostsInput<'_>) -> Result<ResolvedHost, HostsError> {
    let target = Target::parse(input.target)?;
    let mut c = Collector {
        name: target.name(),
        config_dir: input.config_dir,
        home: input.home,
        settings: Vec::new(),
    };

    if let Target::Url(u) = &target {
        let mut offer = |idx: usize, value: Option<String>| {
            if let (Some(kw), Some(v)) = (KEYWORDS.get(idx), value) {
                c.offer(kw, v, Source::Url);
            }
        };
        offer(keywords::USER, u.user.clone());
        offer(keywords::DOMAIN, u.domain.clone());
        offer(
            keywords::PASSWORD,
            u.password.as_ref().map(|p| p.expose().to_owned()),
        );
        offer(keywords::PORT, u.port.map(|p| p.to_string()));
    }

    for (i, opt) in input.options.iter().enumerate() {
        let n = i + 1;
        let at = |msg: String| HostsError::new(format!("-o #{n}: {msg}"));
        let (word, rest) =
            split_line(opt).ok_or_else(|| at("expected Keyword=value".to_owned()))?;
        let kw = match keywords::lookup(word) {
            Some(kw) if kw.kind != Kind::Include => kw,
            _ if ["host", "match", "include"].contains(&word.to_ascii_lowercase().as_str()) => {
                return Err(at(format!("{word} cannot be used with -o")));
            }
            _ => return Err(at(format!("unknown keyword \"{word}\""))),
        };
        let value = if kw.kind == Kind::Command {
            command_value(rest)
        } else {
            let mut w = words(rest).map_err(at)?;
            if w.len() != 1 {
                return Err(at(format!("{} expects exactly one value", kw.name)));
            }
            w.remove(0)
        };
        let value = keywords::validate(kw, &value).map_err(at)?;
        c.offer(kw, value, Source::Option(n));
    }

    match input.file {
        Some(file) => c.load_file(file, true)?,
        None => {
            if let Some(dir) = input.config_dir {
                c.load_file(&dir.join("hosts"), false)?;
            }
        }
    }
    c.load_text(BUILT_IN_DEFAULT, &Origin::Default, &mut Vec::new())?;

    let host_name = match c.settings.iter().find(|s| s.kw.name == "HostName") {
        Some(s) => expand_host_name(&s.value, target.name())?,
        None => target.name().to_owned(),
    };
    let mut settings = c.settings;
    settings.sort_by_key(|s| keywords::index_of(s.kw));
    let resolved = ResolvedHost {
        target,
        host_name,
        settings,
    };
    check_permissions(&resolved)?;
    Ok(resolved)
}

/// `HostName` supports `%h` (the target name) and `%%`.
fn expand_host_name(value: &str, name: &str) -> Result<String, HostsError> {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('h') => out.push_str(name),
            Some('%') => out.push('%'),
            Some(other) => {
                return Err(HostsError::new(format!(
                    "HostName: unknown token %{other} (only %h and %% are allowed)"
                )))
            }
            None => {
                return Err(HostsError::new(
                    "HostName: trailing % (write %% for a literal %)",
                ))
            }
        }
    }
    Ok(out)
}

/// A literal `Password` read from a file other users can read is refused.
#[cfg(unix)]
fn check_permissions(host: &ResolvedHost) -> Result<(), HostsError> {
    use std::os::unix::fs::PermissionsExt;
    let Some(setting) = host.find("Password") else {
        return Ok(());
    };
    let Source::File { path, .. } = &setting.source else {
        return Ok(());
    };
    let mode = std::fs::metadata(path)
        .map_err(|e| HostsError::new(format!("cannot stat {}: {e}", path.display())))?
        .permissions()
        .mode();
    if mode & 0o044 != 0 {
        return Err(HostsError::new(format!(
            "{} contains a Password but is readable by other users; run: chmod 600 {}",
            path.display(),
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_permissions(_host: &ResolvedHost) -> Result<(), HostsError> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;

    struct Env {
        dir: tempfile::TempDir,
    }

    impl Env {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let p = self.dir.path().join(name);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, text).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
            }
            p
        }
        fn hosts(&self, text: &str) {
            self.write("hosts", text);
        }
        fn resolve(&self, target: &str, opts: &[&str]) -> Result<ResolvedHost, HostsError> {
            let opts: Vec<String> = opts.iter().map(|s| (*s).to_owned()).collect();
            resolve_host(&HostsInput {
                target,
                options: &opts,
                file: None,
                config_dir: Some(self.dir.path()),
                home: Some(self.dir.path()),
            })
        }
    }

    fn src(h: &ResolvedHost, kw: &str) -> String {
        h.settings()
            .iter()
            .find(|s| s.keyword() == kw)
            .map(|s| s.source.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn no_file_gives_defaults() {
        let e = Env::new();
        let h = e.resolve("web1", &[]).unwrap();
        assert_eq!(h.address(), "web1");
        assert_eq!(h.port(), None);
        assert!(h.cua_enabled());
        assert!(!h.accept_invalid_certs());
        assert_eq!(src(&h, "CuaEnabled"), "built-in default");
        assert_eq!(h.cua_version(), "latest-dev");
        assert!(h.cua_auto_download());
        assert_eq!(src(&h, "CuaVersion"), "built-in default");
        assert_eq!(src(&h, "CuaAutoDownload"), "built-in default");
    }

    #[test]
    fn cua_version_and_auto_download_overrides() {
        let e = Env::new();
        e.hosts("Host pinned\n  CuaVersion cua-driver-rs-v9.8.7\n  CuaAutoDownload no\n");
        let h = e.resolve("pinned", &[]).unwrap();
        assert_eq!(h.cua_version(), "cua-driver-rs-v9.8.7");
        assert!(!h.cua_auto_download());
        assert!(src(&h, "CuaVersion").ends_with("hosts:2"));
        let o = e
            .resolve("pinned", &["CuaVersion=latest", "CuaAutoDownload=yes"])
            .unwrap();
        assert_eq!(o.cua_version(), "latest");
        assert!(o.cua_auto_download());
        assert_eq!(src(&o, "CuaVersion"), "-o #1");
        for bad in ["CuaVersion=a/b", "CuaVersion=..\\x", "CuaVersion=a b"] {
            assert!(e.resolve("web", &[bad]).is_err(), "{bad}");
        }
        e.hosts("Host bad\n  CuaVersion ../x\n");
        let err = e.resolve("bad", &[]).unwrap_err().to_string();
        assert!(
            err.contains("hosts:2") && err.contains("CuaVersion"),
            "{err}"
        );
    }

    #[test]
    fn bundle_path_is_not_a_hosts_keyword() {
        let e = Env::new();
        for o in ["bundle_path=/opt/bundle", "BundlePath=/opt/bundle"] {
            let err = e.resolve("web", &[o]).unwrap_err().to_string();
            assert!(err.contains("unknown keyword"), "{o}: {err}");
        }
    }

    #[test]
    fn host_block_alias_and_first_value_wins() {
        let e = Env::new();
        e.hosts(
            "Host web1\n  HostName 10.0.0.5\n  Port 3390\n  User alice\nHost *\n  Port 1111\n  User bob\n  Domain CORP\n",
        );
        let h = e.resolve("web1", &[]).unwrap();
        assert_eq!(h.address(), "10.0.0.5");
        assert_eq!(h.port(), Some(3390));
        assert_eq!(h.user(), Some("alice"));
        assert_eq!(h.domain(), Some("CORP"));
        assert!(src(&h, "Port").ends_with("hosts:3"));
        let other = e.resolve("other", &[]).unwrap();
        assert_eq!(other.address(), "other");
        assert_eq!(other.port(), Some(1111));
        assert_eq!(other.user(), Some("bob"));
    }

    #[test]
    fn lines_before_first_host_apply_to_every_host() {
        let e = Env::new();
        e.hosts("User everyone\nHost web1\n  User alice\n");
        assert_eq!(e.resolve("web1", &[]).unwrap().user(), Some("everyone"));
        assert_eq!(e.resolve("zzz", &[]).unwrap().user(), Some("everyone"));
    }

    #[test]
    fn options_beat_file_and_url_beats_options() {
        let e = Env::new();
        e.hosts("Host web1\n  Port 3390\n  User alice\n");
        let h = e.resolve("web1", &["Port=4000", "User bob"]).unwrap();
        assert_eq!(h.port(), Some(4000));
        assert_eq!(h.user(), Some("bob"));
        assert_eq!(src(&h, "Port"), "-o #1");
        assert_eq!(src(&h, "User"), "-o #2");
        let u = e
            .resolve("rdp://carol@web1:5000", &["Port=4000", "User bob"])
            .unwrap();
        assert_eq!(u.port(), Some(5000));
        assert_eq!(u.user(), Some("carol"));
        assert_eq!(src(&u, "User"), "url");
        // Host patterns match the URL hostname.
        assert_eq!(u.domain(), None);
    }

    #[test]
    fn url_host_matches_host_blocks() {
        let e = Env::new();
        e.hosts("Host 10.0.*\n  Domain LAB\n");
        assert_eq!(
            e.resolve("rdp://u@10.0.0.5", &[]).unwrap().domain(),
            Some("LAB")
        );
        assert_eq!(
            e.resolve("rdp://u@192.168.0.5", &[]).unwrap().domain(),
            None
        );
    }

    #[test]
    fn host_pattern_case_follows_ssh() {
        let e = Env::new();
        e.hosts("Host DevBox\n  User upper\nHost devbox\n  User lower\n");
        for t in ["devbox", "DEVBOX"] {
            assert_eq!(e.resolve(t, &[]).unwrap().user(), Some("lower"), "{t}");
        }
    }

    #[test]
    fn negation_and_patterns() {
        let e = Env::new();
        e.hosts("Host * !secret*\n  User general\n");
        assert_eq!(e.resolve("web", &[]).unwrap().user(), Some("general"));
        assert_eq!(e.resolve("secret1", &[]).unwrap().user(), None);
    }

    #[test]
    fn options_cannot_use_host_match_include() {
        let e = Env::new();
        for o in ["Host=x", "Match=x", "Include=x"] {
            let err = e.resolve("web", &[o]).unwrap_err().to_string();
            assert!(err.contains("cannot be used with -o"), "{o}: {err}");
        }
        assert!(e
            .resolve("web", &["Bogus=1"])
            .unwrap_err()
            .to_string()
            .contains("unknown keyword"));
        assert!(e.resolve("web", &["Port=0"]).is_err());
        assert!(e.resolve("web", &["CuaEnabled=maybe"]).is_err());
    }

    #[test]
    fn cua_default_and_override() {
        let e = Env::new();
        e.hosts("Host off\n  CuaEnabled no\n");
        assert!(e.resolve("on", &[]).unwrap().cua_enabled());
        assert!(!e.resolve("off", &[]).unwrap().cua_enabled());
        assert!(!e.resolve("on", &["CuaEnabled=no"]).unwrap().cua_enabled());
        assert!(e.resolve("off", &["CuaEnabled=yes"]).unwrap().cua_enabled());
    }

    #[test]
    fn host_name_expansion() {
        let e = Env::new();
        e.hosts("Host web1\n  HostName %h.corp.example\n  AcceptInvalidCerts yes\nHost lit\n  HostName 100%%\n");
        let h = e.resolve("web1", &[]).unwrap();
        assert_eq!(h.address(), "web1.corp.example");
        assert!(h.accept_invalid_certs());
        assert_eq!(e.resolve("lit", &[]).unwrap().address(), "100%");
        e.hosts("Host x\n  HostName %q\n");
        assert!(e.resolve("x", &[]).is_err());
    }

    #[test]
    fn parse_errors_name_file_and_line() {
        let e = Env::new();
        e.hosts("Host a\n  Bogus 1\n");
        let err = e.resolve("a", &[]).unwrap_err().to_string();
        assert!(
            err.contains("hosts:2") && err.contains("unknown keyword"),
            "{err}"
        );
        e.hosts("Match all\n");
        assert!(e
            .resolve("a", &[])
            .unwrap_err()
            .to_string()
            .contains("Match is not supported"));
        e.hosts("Host a\n  Port abc\n");
        assert!(e
            .resolve("a", &[])
            .unwrap_err()
            .to_string()
            .contains("hosts:2"));
        e.hosts("Host a\n  User a b\n");
        assert!(e
            .resolve("a", &[])
            .unwrap_err()
            .to_string()
            .contains("exactly one value"));
    }

    #[test]
    fn quoting_and_equals() {
        let e = Env::new();
        e.hosts("Host=web1\nUser=\"a b\"\nDomain = CORP # note\n");
        let h = e.resolve("web1", &[]).unwrap();
        assert_eq!(h.user(), Some("a b"));
        assert_eq!(h.domain(), Some("CORP"));
    }

    #[test]
    fn include_glob_sorted_and_missing_skipped() {
        let e = Env::new();
        e.write("conf.d/b.conf", "Host web\n  User from-b\n  Port 2\n");
        e.write("conf.d/a.conf", "Host web\n  User from-a\n");
        e.hosts(
            "Include conf.d/*.conf\nInclude missing/*\nInclude nothing.conf\nHost web\n  Port 9\n",
        );
        let h = e.resolve("web", &[]).unwrap();
        assert_eq!(h.user(), Some("from-a"));
        assert_eq!(h.port(), Some(2));
    }

    #[test]
    fn include_not_read_inside_non_matching_host() {
        let e = Env::new();
        e.write("inc.conf", "User included\n");
        e.hosts("Host other\n  Include inc.conf\n");
        assert_eq!(e.resolve("web", &[]).unwrap().user(), None);
        assert_eq!(e.resolve("other", &[]).unwrap().user(), Some("included"));
    }

    #[test]
    fn include_host_state_does_not_leak_back() {
        let e = Env::new();
        e.write("inc.conf", "Host never\n  User x\n");
        e.hosts("Include inc.conf\nUser after\n");
        assert_eq!(e.resolve("web", &[]).unwrap().user(), Some("after"));
    }

    #[test]
    fn include_tilde() {
        let e = Env::new();
        e.write("home-inc.conf", "User tilde\n");
        e.hosts("Include ~/home-inc.conf\n");
        assert_eq!(e.resolve("web", &[]).unwrap().user(), Some("tilde"));
    }

    #[test]
    fn include_cycle_and_depth() {
        let e = Env::new();
        e.write("a.conf", "Include b.conf\n");
        e.write("b.conf", "Include a.conf\n");
        e.hosts("Include a.conf\n");
        let err = e.resolve("web", &[]).unwrap_err().to_string();
        assert!(err.contains("cycle") && err.contains(".conf:1"), "{err}");

        let e = Env::new();
        for i in 0..20 {
            e.write(&format!("d{i}.conf"), &format!("Include d{}.conf\n", i + 1));
        }
        e.write("d20.conf", "User deep\n");
        e.hosts("Include d0.conf\n");
        assert!(e
            .resolve("web", &[])
            .unwrap_err()
            .to_string()
            .contains("levels deep"));
    }

    #[test]
    fn dash_f_replaces_user_file_and_must_exist() {
        let e = Env::new();
        e.hosts("Host web\n  User user-file\n");
        let alt = e.write("alt-hosts", "Host web\n  User alt\n");
        let opts: [String; 0] = [];
        let h = resolve_host(&HostsInput {
            target: "web",
            options: &opts,
            file: Some(&alt),
            config_dir: Some(e.dir.path()),
            home: None,
        })
        .unwrap();
        assert_eq!(h.user(), Some("alt"));
        let missing = e.dir.path().join("nope");
        let err = resolve_host(&HostsInput {
            target: "web",
            options: &opts,
            file: Some(&missing),
            config_dir: Some(e.dir.path()),
            home: None,
        })
        .unwrap_err();
        assert!(err.to_string().contains("cannot read"), "{err}");
    }

    #[test]
    fn password_and_command_share_a_slot() {
        let e = Env::new();
        e.hosts("Host web\n  PasswordCommand printenv X\n  Password file-secret\n");
        let h = e.resolve("web", &[]).unwrap();
        assert_eq!(h.password_command(), Some("printenv X"));
        assert!(h.password().is_none());
        let h = e.resolve("web", &["Password=opt-secret"]).unwrap();
        assert!(h.password_command().is_none());
        assert_eq!(h.password().unwrap().expose(), "opt-secret");
        let h = e
            .resolve("rdp://u:url-secret@web", &["Password=opt-secret"])
            .unwrap();
        assert_eq!(h.password().unwrap().expose(), "url-secret");
    }

    #[test]
    fn secrets_never_in_debug_or_rows() {
        let e = Env::new();
        let h = e
            .resolve("rdp://u:hunter2@web", &["Password=other"])
            .unwrap();
        assert!(!format!("{h:?}").contains("hunter2"));
        let rows = e.resolve("web", &["Password=hunter2"]).unwrap().rows();
        let pw = rows.iter().find(|r| r.keyword == "Password").unwrap();
        assert_eq!(pw.value, "<redacted>");
        assert!(!format!("{rows:?}").contains("hunter2"));
    }

    #[test]
    fn rows_are_in_table_order_with_unset_port_labelled() {
        let e = Env::new();
        e.hosts("Host web\n  User a\n");
        let rows = e.resolve("web", &[]).unwrap().rows();
        let names: Vec<_> = rows.iter().map(|r| r.keyword).collect();
        assert_eq!(
            names,
            [
                "HostName",
                "Port",
                "User",
                "CuaEnabled",
                "CuaVersion",
                "CuaAutoDownload"
            ]
        );
        assert_eq!(rows[1].value, "3389");
        assert!(rows[1].source.starts_with("not set"));
    }

    #[cfg(unix)]
    #[test]
    fn readable_password_file_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let e = Env::new();
        let p = e.write("hosts", "Host web\n  Password secret\n");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        let err = e.resolve("web", &[]).unwrap_err().to_string();
        assert!(err.contains("chmod 600"), "{err}");
        assert!(!err.contains("secret\n"));
        // A password from -o or a PasswordCommand does not care about the mode.
        assert!(e.resolve("web", &["Password=x"]).is_ok());
        fs::write(&p, "Host web\n  PasswordCommand printenv X\n").unwrap();
        assert!(e.resolve("web", &[]).is_ok());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&p, "Host web\n  Password secret\n").unwrap();
        assert!(e.resolve("web", &[]).is_ok());
    }

    #[test]
    fn rdp_and_rdps_resolve_to_the_same_settings() {
        let e = Env::new();
        e.hosts("Host h.example\n  Port 3391\n  Domain CORP\n  AcceptInvalidCerts yes\n  CuaEnabled no\n");
        let a = e.resolve("rdp://u:pw@h.example", &[]).unwrap();
        let b = e.resolve("rdps://u:pw@h.example", &[]).unwrap();
        // The scheme only changes the redacted display label; the connect
        // request carries no scheme, so equal settings mean equal requests.
        assert_eq!(a.rows(), b.rows());
        assert_eq!(a.address(), b.address());
        assert_eq!(a.port(), Some(3391));
        assert_eq!(a.port(), b.port());
        assert_eq!(a.user(), b.user());
        assert_eq!(a.domain(), Some("CORP"));
        assert_eq!(a.domain(), b.domain());
        assert_eq!(
            a.password().as_ref().map(Secret::expose),
            b.password().as_ref().map(Secret::expose)
        );
        assert_eq!(a.password_command(), b.password_command());
        assert!(a.accept_invalid_certs() && b.accept_invalid_certs());
        assert!(!a.cua_enabled() && !b.cua_enabled());

        let lit = e.resolve("rdp://CORP\\alice:pw@h.example", &[]).unwrap();
        let enc = e.resolve("rdp://CORP%5Calice:pw@h.example", &[]).unwrap();
        assert_eq!(lit.domain(), Some("CORP"));
        assert_eq!(lit.user(), Some("alice"));
        assert_eq!(src(&lit, "Domain"), "url");
        assert_eq!(src(&lit, "Domain"), src(&enc, "Domain"));
        assert_eq!(lit.rows(), enc.rows());
    }
}
