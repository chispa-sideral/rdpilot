//! Upstream Cua driver releases: tag parsing, channel selection and
//! `checksums.txt` parsing.
//!
//! Upstream marks every cua-driver-rs release as a pre-release and shares
//! the repository with other products, so selection uses the tag prefix and
//! semantic-version order, never GitHub's "latest" or `prerelease` flags or
//! publication dates.

use serde::Deserialize;

use super::Arch;

/// Upstream repository.
pub(crate) const CUA_REPO: &str = "trycua/cua";
/// Tag prefix of a stable (or release-candidate) driver release.
const STABLE_PREFIX: &str = "cua-driver-rs-v";
/// Tag prefix of a nightly driver release.
const NIGHTLY_PREFIX: &str = "nightly-cua-driver-rs-v";
/// Checksum file of every upstream release.
pub(crate) const CHECKSUMS_ASSET: &str = "checksums.txt";

/// One driver release, identified by its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CuaRelease {
    pub tag: String,
    pub version: semver::Version,
}

impl CuaRelease {
    /// Parse `cua-driver-rs-v<semver>` or `nightly-cua-driver-rs-v<semver>`.
    pub(crate) fn from_tag(tag: &str) -> Option<Self> {
        let version = tag
            .strip_prefix(NIGHTLY_PREFIX)
            .or_else(|| tag.strip_prefix(STABLE_PREFIX))?;
        Some(Self {
            tag: tag.to_owned(),
            version: semver::Version::parse(version).ok()?,
        })
    }

    /// The release for a cached version string.
    pub(crate) fn from_version(version: &str) -> Option<Self> {
        let version = semver::Version::parse(version).ok()?;
        let prefix = if version.pre.as_str().starts_with("nightly") {
            NIGHTLY_PREFIX
        } else {
            STABLE_PREFIX
        };
        Some(Self {
            tag: format!("{prefix}{version}"),
            version,
        })
    }

    /// Nightlies and other pre-release versions belong only to `latest-dev`.
    pub(crate) fn is_stable(&self) -> bool {
        self.version.pre.is_empty()
    }

    /// The Windows binary archive of this release for `arch`.
    pub(crate) fn asset(&self, arch: Arch) -> String {
        format!(
            "cua-driver-rs-{}-windows-{}-binary.zip",
            self.version,
            arch.as_str()
        )
    }
}

/// A resolution channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Channel {
    /// Newest release, nightlies included.
    LatestDev,
    /// Newest stable release.
    Latest,
}

impl Channel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Channel::LatestDev => "latest-dev",
            Channel::Latest => "latest",
        }
    }

    pub(crate) fn accepts(self, release: &CuaRelease) -> bool {
        match self {
            Channel::LatestDev => true,
            Channel::Latest => release.is_stable(),
        }
    }
}

/// What `CuaVersion` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wanted {
    Channel(Channel),
    Tag(CuaRelease),
}

pub(crate) fn parse_wanted(value: &str) -> Result<Wanted, String> {
    match value {
        "latest-dev" => Ok(Wanted::Channel(Channel::LatestDev)),
        "latest" => Ok(Wanted::Channel(Channel::Latest)),
        tag => CuaRelease::from_tag(tag).map(Wanted::Tag).ok_or_else(|| {
            format!(
                "CuaVersion {tag:?} is not latest-dev, latest or a tag such as \
                 cua-driver-rs-v<version> or nightly-cua-driver-rs-v<version>"
            )
        }),
    }
}

/// The release fields selection needs.
#[derive(Debug, Deserialize)]
pub(crate) struct ApiRelease {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiAsset {
    pub name: String,
}

impl ApiRelease {
    pub(crate) fn has_asset(&self, name: &str) -> bool {
        self.assets.iter().any(|a| a.name == name)
    }
}

/// The highest version in `channel` that publishes the archive for `arch`
/// and a checksum file. Other products and drafts are ignored.
pub(crate) fn select(releases: &[ApiRelease], channel: Channel, arch: Arch) -> Option<CuaRelease> {
    releases
        .iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let release = CuaRelease::from_tag(&r.tag_name)?;
            (channel.accepts(&release)
                && r.has_asset(&release.asset(arch))
                && r.has_asset(CHECKSUMS_ASSET))
            .then_some(release)
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// The SHA-256 of `asset` in a checksum file. Lines look like
/// `<64 hex>  <name>` or `<64 hex> *<name>`; anything else (Markdown
/// headings, fences) is ignored. Exactly one line must name the asset.
pub(crate) fn parse_checksum(text: &str, asset: &str) -> Result<String, String> {
    let matches: Vec<String> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.len() <= 64 || !line.is_char_boundary(64) {
                return None;
            }
            let (hash, rest) = line.split_at(64);
            if !hash.bytes().all(|b| b.is_ascii_hexdigit()) || !rest.starts_with([' ', '\t']) {
                return None;
            }
            let name = rest.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name);
            (name == asset).then(|| hash.to_ascii_lowercase())
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("the checksum file does not list {asset}")),
        _ => Err(format!("the checksum file lists {asset} more than once")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A release list in upstream's shape: every cua-driver-rs release is a
    /// pre-release, other products share the repository, and a nightly of
    /// an older version is published after a newer stable release.
    pub(crate) fn fixture_releases() -> Vec<ApiRelease> {
        let rel = |tag: &str, assets: &[&str]| ApiRelease {
            tag_name: tag.to_owned(),
            draft: false,
            prerelease: true,
            assets: assets
                .iter()
                .map(|n| ApiAsset {
                    name: (*n).to_owned(),
                })
                .collect(),
        };
        let cua = |version: &str| {
            [
                "checksums.txt".to_owned(),
                format!("cua-driver-rs-{version}-windows-x86_64-binary.zip"),
                format!("cua-driver-rs-{version}-windows-x86_64.zip"),
            ]
        };
        let a = cua("7.3.3-nightly.20990101.5");
        let b = cua("7.3.4");
        let c = cua("7.3.5-nightly.20990102.6");
        let d = cua("7.3.3");
        let e = cua("7.4.0");
        fn refs(v: &[String; 3]) -> Vec<&str> {
            v.iter().map(String::as_str).collect()
        }
        let mut draft = rel("cua-driver-rs-v7.4.0", &refs(&e));
        draft.draft = true;
        vec![
            // Published last, but older than the stable 7.3.4.
            rel("nightly-cua-driver-rs-v7.3.3-nightly.20990101.5", &refs(&a)),
            rel("nightly-lume-v9.9.9", &["lume.zip"]),
            rel("cua-driver-rs-v7.3.4", &refs(&b)),
            rel("nightly-cua-driver-rs-v7.3.5-nightly.20990102.6", &refs(&c)),
            rel("cua-perception-v9.0.0", &["checksums.txt"]),
            rel("cua-driver-rs-v7.3.3", &refs(&d)),
            draft,
            // A newer stable tag without a Windows archive is skipped.
            rel("cua-driver-rs-v7.3.9", &["checksums.txt"]),
        ]
    }

    #[test]
    fn channels_select_by_version_not_flags_or_order() {
        let releases = fixture_releases();
        let dev = select(&releases, Channel::LatestDev, Arch::X86_64).unwrap();
        assert_eq!(dev.tag, "nightly-cua-driver-rs-v7.3.5-nightly.20990102.6");
        let latest = select(&releases, Channel::Latest, Arch::X86_64).unwrap();
        assert_eq!(latest.tag, "cua-driver-rs-v7.3.4");
        assert_eq!(select(&[], Channel::Latest, Arch::X86_64), None);
    }

    #[test]
    fn a_stable_release_outranks_its_own_nightlies() {
        let mut releases = fixture_releases();
        releases.push(ApiRelease {
            tag_name: "cua-driver-rs-v7.3.5".into(),
            draft: false,
            prerelease: true,
            assets: [
                "checksums.txt",
                "cua-driver-rs-7.3.5-windows-x86_64-binary.zip",
            ]
            .iter()
            .map(|n| ApiAsset {
                name: (*n).to_owned(),
            })
            .collect(),
        });
        let dev = select(&releases, Channel::LatestDev, Arch::X86_64).unwrap();
        assert_eq!(dev.tag, "cua-driver-rs-v7.3.5");
    }

    #[test]
    fn wanted_values() {
        assert_eq!(
            parse_wanted("latest-dev"),
            Ok(Wanted::Channel(Channel::LatestDev))
        );
        assert_eq!(parse_wanted("latest"), Ok(Wanted::Channel(Channel::Latest)));
        let Ok(Wanted::Tag(t)) = parse_wanted("nightly-cua-driver-rs-v7.0.1-nightly.20990101.1")
        else {
            panic!("nightly tag")
        };
        assert!(!t.is_stable());
        assert_eq!(
            t.asset(Arch::X86_64),
            "cua-driver-rs-7.0.1-nightly.20990101.1-windows-x86_64-binary.zip"
        );
        let Ok(Wanted::Tag(t)) = parse_wanted("cua-driver-rs-v7.0.2") else {
            panic!("stable tag")
        };
        assert!(t.is_stable());
        assert!(parse_wanted("v7.0.2").is_err());
        assert!(parse_wanted("cua-driver-rs-vX").is_err());
    }

    #[test]
    fn versions_round_trip_to_tags() {
        for tag in [
            "cua-driver-rs-v7.0.2",
            "nightly-cua-driver-rs-v7.0.3-nightly.20990101.9",
        ] {
            let r = CuaRelease::from_tag(tag).unwrap();
            assert_eq!(CuaRelease::from_version(&r.version.to_string()).unwrap(), r);
        }
    }

    #[test]
    fn checksum_formats() {
        let h = "ab".repeat(32);
        let other = "cd".repeat(32);
        let asset = "cua-driver-rs-7.0.2-windows-x86_64-binary.zip";
        let markdown = format!(
            "## SHA256 Checksums\n\n```\n{other}  cua-driver-rs-7.0.2-windows-x86_64.zip\n{h}  {asset}\n```\n"
        );
        assert_eq!(parse_checksum(&markdown, asset), Ok(h.clone()));
        let plain = format!("{} *{asset}\n", h.to_ascii_uppercase());
        assert_eq!(parse_checksum(&plain, asset), Ok(h.clone()));
        assert!(parse_checksum(&format!("{h}  other.zip\n"), asset).is_err());
        assert!(parse_checksum(&format!("{h}  {asset}\n{other}  {asset}\n"), asset).is_err());
        assert!(parse_checksum(&format!("{h}{asset}\n"), asset).is_err());
    }
}
