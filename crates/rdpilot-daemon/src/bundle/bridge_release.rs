//! rdpilot-bridge releases of this project.
//!
//! A release build uses the release whose tag is `v<daemon version>`. A
//! development build (no such release) uses a bridge from `bundle_path`,
//! else the newest published release, with a warning.

use super::cua::ApiRelease;
use rdpilot_bridge_protocol::BRIDGE_EXE_NAME;

/// This project's repository.
pub(crate) const RELEASE_REPO: &str = "chispa-sideral/rdpilot";
/// Checksum file of every bridge release.
pub(crate) const SHA256SUMS_ASSET: &str = "SHA256SUMS";

/// The release tag of a version.
pub(crate) fn release_tag(version: &str) -> String {
    format!("v{version}")
}

/// The version of a `v<MAJOR.MINOR.PATCH>` tag. Other tags (for example a
/// milestone tag `v1.0`) are not bridge releases.
pub(crate) fn tag_version(tag: &str) -> Option<semver::Version> {
    let version = semver::Version::parse(tag.strip_prefix('v')?).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}

/// The newest published (not draft, not pre-release) bridge release that
/// has both assets.
pub(crate) fn newest(releases: &[ApiRelease]) -> Option<semver::Version> {
    releases
        .iter()
        .filter(|r| {
            !r.draft
                && !r.prerelease
                && r.has_asset(BRIDGE_EXE_NAME)
                && r.has_asset(SHA256SUMS_ASSET)
        })
        .filter_map(|r| tag_version(&r.tag_name))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::cua::ApiAsset;

    fn rel(tag: &str, draft: bool, prerelease: bool, assets: &[&str]) -> ApiRelease {
        ApiRelease {
            tag_name: tag.to_owned(),
            draft,
            prerelease,
            assets: assets
                .iter()
                .map(|n| ApiAsset {
                    name: (*n).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn newest_skips_drafts_prereleases_milestones_and_incomplete_releases() {
        let both = [BRIDGE_EXE_NAME, SHA256SUMS_ASSET];
        let releases = vec![
            rel("v5.1.0", false, false, &both),
            rel("v5.10.0", false, false, &both),
            rel("v6.0.0", true, false, &both),
            rel("v5.11.0", false, true, &both),
            rel("v5.12.0", false, false, &[BRIDGE_EXE_NAME]),
            rel("v7.0", false, false, &both),
            rel("other-v9.0.0", false, false, &both),
        ];
        assert_eq!(newest(&releases), Some(semver::Version::new(5, 10, 0)));
        assert_eq!(newest(&[]), None);
    }

    #[test]
    fn tags() {
        assert_eq!(release_tag("5.1.0"), "v5.1.0");
        assert_eq!(tag_version("v5.1.0"), Some(semver::Version::new(5, 1, 0)));
        assert_eq!(tag_version("v1.0"), None);
        assert_eq!(tag_version("5.1.0"), None);
        assert_eq!(tag_version("v5.1.0-rc.1"), None);
    }
}
