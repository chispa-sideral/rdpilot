//! Cua bundle acquisition: find, download, verify and cache the two guest
//! components, then assemble the directory the RDP drive channel serves.
//!
//! - rdpilot-bridge.exe comes from this project's GitHub release whose tag
//!   matches the daemon version (a development build falls back to
//!   `bundle_path`, then to the newest release with a warning).
//! - The Cua driver archive comes from the upstream trycua/cua release that
//!   `CuaVersion` selects.
//!
//! Both are checked against the checksum file of their own release (same
//! origin as the file; there is no compiled-in hash or version). Files in
//! the daemon-local `bundle_path` directory are trusted as they are.
//! Everything here runs in the daemon before the RDP logon, so a failure
//! fails the connect closed.

mod bridge_release;
mod cache;
mod cua;
mod http;
mod layout;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rdpilot_bridge_protocol::{BundleManifest, BRIDGE_EXE_NAME, MANIFEST_NAME};
use sha2::{Digest, Sha256};

use self::cache::{sha256_file, Cache};
use self::cua::{Channel, CuaRelease, Wanted};
use self::http::{hex, Endpoints, FetchError, Http};
use crate::seams::{BoxFuture, BundleSource};

/// Cache component name of the Cua driver archive.
pub(crate) const CUA_COMPONENT: &str = "cua-driver";
/// Cache component name of the bridge executable.
pub(crate) const BRIDGE_COMPONENT: &str = "rdpilot-bridge";
/// How long a resolved `latest-dev`/`latest` or newest-bridge answer is
/// reused without asking GitHub again.
const RESOLVED_TTL: Duration = Duration::from_secs(3600);
/// Release list pages read (100 releases each).
const MAX_RELEASE_PAGES: usize = 3;

/// Guest architecture. Only x86_64 today; the cache and asset names are
/// keyed by it so arm64 can be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
}

impl Arch {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
        }
    }
}

/// What one connect asks for.
#[derive(Debug, Clone)]
pub struct BundleRequest {
    /// `CuaVersion`.
    pub cua_version: String,
    /// `CuaAutoDownload`.
    pub auto_download: bool,
    pub arch: Arch,
}

/// A verified bundle directory ready to serve.
#[derive(Debug, Clone)]
pub struct PreparedBundle {
    pub dir: PathBuf,
    pub bundle_id: String,
    /// Notes for the user (version differences, offline fallbacks).
    pub warnings: Vec<String>,
}

/// Which part could not be obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    CuaDriver,
    Bridge,
    BundlePath,
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Component::CuaDriver => "Cua driver",
            Component::Bridge => "rdpilot-bridge",
            Component::BundlePath => "bundle_path",
        })
    }
}

/// Why it could not be obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    Offline(String),
    ChecksumMismatch { expected: String, actual: String },
    NoMatchingRelease(String),
    UnsupportedArchiveLayout(String),
    MissingPath(String),
    AutoDownloadDisabled,
    Io(String),
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cause::Offline(detail) => write!(f, "offline or GitHub unreachable ({detail})"),
            Cause::ChecksumMismatch { expected, actual } => write!(
                f,
                "checksum failure (published {expected}, downloaded {actual}); nothing was cached"
            ),
            Cause::NoMatchingRelease(detail) => write!(f, "no matching release ({detail})"),
            Cause::UnsupportedArchiveLayout(detail) => {
                write!(f, "unsupported archive layout ({detail})")
            }
            Cause::MissingPath(detail) => write!(f, "missing path ({detail})"),
            Cause::AutoDownloadDisabled => {
                write!(f, "not in the cache, and CuaAutoDownload is no")
            }
            Cause::Io(detail) => write!(f, "local error ({detail})"),
        }
    }
}

/// A fail-closed connect error: component, version, architecture, cause and
/// the fixes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleError {
    pub component: Component,
    pub version: String,
    pub arch: Arch,
    pub cause: Cause,
}

impl BundleError {
    fn new(component: Component, version: &str, arch: Arch, cause: Cause) -> Self {
        Self {
            component,
            version: version.to_owned(),
            arch,
            cause,
        }
    }
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Cua is enabled but {} {} ({}) is not available: {}. Fixes: connect \
             native-only with -o CuaEnabled=no; pin a release with -o CuaVersion=<tag>; \
             set bundle_path in config.toml to a directory with rdpilot-bridge.exe \
             and the Cua archive; or restore network access",
            self.component,
            self.version,
            self.arch.as_str(),
            self.cause
        )
    }
}

impl std::error::Error for BundleError {}

/// A component file ready to go into a bundle.
struct Obtained {
    path: PathBuf,
    sha256: String,
    version: String,
}

/// What `bundle_path` holds.
#[derive(Default)]
struct Local {
    bridge: Option<PathBuf>,
    cua: Option<(CuaRelease, PathBuf)>,
}

/// Result of fetching one bridge release.
enum BridgeFetch {
    /// No release with this tag (HTTP 404 on its SHA256SUMS).
    NotFound,
    Failed(Cause),
}

fn io_cause(e: impl fmt::Display) -> Cause {
    Cause::Io(e.to_string())
}

fn offline(error: &FetchError) -> Cause {
    match error {
        FetchError::NotFound => Cause::Offline("HTTP 404 for the release list".to_owned()),
        FetchError::Unavailable(detail) => Cause::Offline(detail.clone()),
    }
}

/// Run blocking file work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

async fn hash(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    blocking(move || sha256_file(&path).map_err(|e| format!("{}: {e}", path.display()))).await
}

/// Hard link, else copy.
fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

/// Downloads, verifies and caches bundles.
pub(crate) struct Acquirer {
    http: Http,
    cache: Cache,
    daemon_version: String,
    ttl: Duration,
}

impl Acquirer {
    pub(crate) fn new(http: Http, cache: Cache, daemon_version: String) -> Self {
        Self {
            http,
            cache,
            daemon_version,
            ttl: RESOLVED_TTL,
        }
    }

    pub(crate) async fn prepare(
        &self,
        request: &BundleRequest,
        bundle_path: Option<&Path>,
    ) -> Result<PreparedBundle, BundleError> {
        let mut warnings = Vec::new();
        let local = match bundle_path {
            Some(dir) => scan_local(dir, request.arch)?,
            None => Local::default(),
        };
        let cua = self.cua(request, &local, &mut warnings).await?;
        let bridge = self.bridge(request, &local, &mut warnings).await?;
        let (dir, bundle_id) = self.assemble(&cua, &bridge, request.arch).await?;
        Ok(PreparedBundle {
            dir,
            bundle_id,
            warnings,
        })
    }

    // ---- Cua driver ----------------------------------------------------

    async fn cua(
        &self,
        request: &BundleRequest,
        local: &Local,
        warnings: &mut Vec<String>,
    ) -> Result<Obtained, BundleError> {
        let arch = request.arch;
        let wanted = cua::parse_wanted(&request.cua_version).map_err(|e| {
            BundleError::new(
                Component::CuaDriver,
                &request.cua_version,
                arch,
                Cause::NoMatchingRelease(e),
            )
        })?;
        if let Some((release, path)) = &local.cua {
            // The archive in bundle_path fixes the version.
            if let Wanted::Tag(tag) = &wanted {
                if tag.version != release.version {
                    warnings.push(format!(
                        "bundle_path holds Cua {}; CuaVersion {} is ignored",
                        release.version, request.cua_version
                    ));
                }
            }
            let version = release.version.to_string();
            let sha256 = hash(path)
                .await
                .map_err(|e| BundleError::new(Component::CuaDriver, &version, arch, io_cause(e)))?;
            return Ok(Obtained {
                path: path.clone(),
                sha256,
                version,
            });
        }
        let release = match wanted {
            Wanted::Tag(release) => release,
            Wanted::Channel(channel) => self.resolve_channel(channel, request, warnings).await?,
        };
        let version = release.version.to_string();
        let error = |cause| BundleError::new(Component::CuaDriver, &version, arch, cause);
        let asset = release.asset(arch);
        if let Some((path, sha256)) = self.cache.lookup(CUA_COMPONENT, &version, arch, &asset) {
            return Ok(Obtained {
                path,
                sha256,
                version,
            });
        }
        if !request.auto_download {
            return Err(error(Cause::AutoDownloadDisabled));
        }
        let endpoints = &self.http.endpoints;
        let sums_url = endpoints.asset_url(cua::CUA_REPO, &release.tag, cua::CHECKSUMS_ASSET);
        let sums = match self.http.text(&sums_url).await {
            Ok((text, _)) => text,
            Err(FetchError::NotFound) => {
                return Err(error(Cause::NoMatchingRelease(format!(
                    "upstream release {} does not exist or has no {}",
                    release.tag,
                    cua::CHECKSUMS_ASSET
                ))))
            }
            Err(FetchError::Unavailable(detail)) => return Err(error(Cause::Offline(detail))),
        };
        let expected =
            cua::parse_checksum(&sums, &asset).map_err(|e| error(Cause::NoMatchingRelease(e)))?;
        let url = endpoints.asset_url(cua::CUA_REPO, &release.tag, &asset);
        let checked = self
            .download_verified(&url, &expected, |path| {
                layout::inspect(path)
                    .map(|_| ())
                    .map_err(Cause::UnsupportedArchiveLayout)
            })
            .await
            .map_err(|cause| match cause {
                Cause::NoMatchingRelease(_) => error(Cause::NoMatchingRelease(format!(
                    "upstream release {} has no {asset}",
                    release.tag
                ))),
                cause => error(cause),
            })?;
        let path = self
            .cache
            .publish(CUA_COMPONENT, &version, arch, &asset, &checked, &expected)
            .map_err(|e| error(io_cause(e)))?;
        Ok(Obtained {
            path,
            sha256: expected,
            version,
        })
    }

    async fn resolve_channel(
        &self,
        channel: Channel,
        request: &BundleRequest,
        warnings: &mut Vec<String>,
    ) -> Result<CuaRelease, BundleError> {
        let arch = request.arch;
        let error = |cause| BundleError::new(Component::CuaDriver, channel.as_str(), arch, cause);
        if !request.auto_download {
            return self
                .newest_cached_cua(channel, arch)
                .ok_or_else(|| error(Cause::AutoDownloadDisabled));
        }
        let key = format!("cua/{}/{}", channel.as_str(), arch.as_str());
        if let Some(release) = self
            .cache
            .resolved(&key, self.ttl)
            .and_then(|tag| CuaRelease::from_tag(&tag))
        {
            return Ok(release);
        }
        match self.list(cua::CUA_REPO).await {
            Ok(releases) => {
                let release = cua::select(&releases, channel, arch).ok_or_else(|| {
                    error(Cause::NoMatchingRelease(format!(
                        "no cua-driver-rs release in channel {} publishes a Windows {} archive",
                        channel.as_str(),
                        arch.as_str()
                    )))
                })?;
                self.cache.remember(&key, &release.tag);
                Ok(release)
            }
            Err(fetch) => {
                let cause = offline(&fetch);
                match self.newest_cached_cua(channel, arch) {
                    Some(release) => {
                        warnings.push(format!(
                            "cannot reach GitHub ({cause}); using cached Cua {} for CuaVersion {}",
                            release.version,
                            channel.as_str()
                        ));
                        Ok(release)
                    }
                    None => Err(error(cause)),
                }
            }
        }
    }

    /// The newest verified cached driver version in `channel`.
    fn newest_cached_cua(&self, channel: Channel, arch: Arch) -> Option<CuaRelease> {
        let mut releases: Vec<CuaRelease> = self
            .cache
            .versions(CUA_COMPONENT, arch)
            .iter()
            .filter_map(|v| CuaRelease::from_version(v))
            .filter(|r| channel.accepts(r))
            .collect();
        releases.sort_by(|a, b| b.version.cmp(&a.version));
        releases.into_iter().find(|r| {
            self.cache
                .lookup(CUA_COMPONENT, &r.version.to_string(), arch, &r.asset(arch))
                .is_some()
        })
    }

    // ---- rdpilot-bridge ------------------------------------------------

    async fn bridge(
        &self,
        request: &BundleRequest,
        local: &Local,
        warnings: &mut Vec<String>,
    ) -> Result<Obtained, BundleError> {
        let arch = request.arch;
        if let Some(path) = &local.bridge {
            let sha256 = hash(path).await.map_err(|e| {
                BundleError::new(Component::Bridge, "from bundle_path", arch, io_cause(e))
            })?;
            return Ok(Obtained {
                path: path.clone(),
                sha256,
                version: "from bundle_path".to_owned(),
            });
        }
        let own = self.daemon_version.clone();
        let error =
            |version: &str, cause| BundleError::new(Component::Bridge, version, arch, cause);
        // A release is immutable: a verified cached copy of our own version
        // needs no network.
        if let Some(obtained) = self.cached_bridge(&own, arch) {
            return Ok(obtained);
        }
        if !request.auto_download {
            return Err(error(&own, Cause::AutoDownloadDisabled));
        }
        match self.fetch_bridge(&own, arch).await {
            Ok(obtained) => Ok(obtained),
            // Only a definite "no such release" makes this a development
            // build. A network failure must not pick another version.
            Err(BridgeFetch::Failed(cause)) => Err(error(&own, cause)),
            Err(BridgeFetch::NotFound) => {
                let version = self.newest_bridge_version(arch).await?;
                warnings.push(format!(
                    "rdpilot-bridge {version} differs from rdpilot-daemon {own}: this build has \
                     no release and bundle_path has no rdpilot-bridge.exe"
                ));
                if let Some(obtained) = self.cached_bridge(&version, arch) {
                    return Ok(obtained);
                }
                match self.fetch_bridge(&version, arch).await {
                    Ok(obtained) => Ok(obtained),
                    Err(BridgeFetch::NotFound) => Err(error(
                        &version,
                        Cause::NoMatchingRelease(format!(
                            "release {} has no SHA256SUMS",
                            bridge_release::release_tag(&version)
                        )),
                    )),
                    Err(BridgeFetch::Failed(cause)) => Err(error(&version, cause)),
                }
            }
        }
    }

    fn cached_bridge(&self, version: &str, arch: Arch) -> Option<Obtained> {
        self.cache
            .lookup(BRIDGE_COMPONENT, version, arch, BRIDGE_EXE_NAME)
            .map(|(path, sha256)| Obtained {
                path,
                sha256,
                version: version.to_owned(),
            })
    }

    async fn newest_bridge_version(&self, arch: Arch) -> Result<String, BundleError> {
        let own = &self.daemon_version;
        let error = |cause| BundleError::new(Component::Bridge, own, arch, cause);
        let key = "bridge/newest";
        if let Some(version) = self.cache.resolved(key, self.ttl) {
            return Ok(version);
        }
        let releases = self
            .list(bridge_release::RELEASE_REPO)
            .await
            .map_err(|e| error(offline(&e)))?;
        let version = bridge_release::newest(&releases)
            .ok_or_else(|| {
                error(Cause::NoMatchingRelease(format!(
                    "this development build has no release {}, and no published rdpilot \
                     release has rdpilot-bridge.exe; put a bridge built from this source \
                     in bundle_path",
                    bridge_release::release_tag(own)
                )))
            })?
            .to_string();
        self.cache.remember(key, &version);
        Ok(version)
    }

    async fn fetch_bridge(&self, version: &str, arch: Arch) -> Result<Obtained, BridgeFetch> {
        let tag = bridge_release::release_tag(version);
        let endpoints = &self.http.endpoints;
        let repo = bridge_release::RELEASE_REPO;
        let sums = match self
            .http
            .text(&endpoints.asset_url(repo, &tag, bridge_release::SHA256SUMS_ASSET))
            .await
        {
            Ok((text, _)) => text,
            Err(FetchError::NotFound) => return Err(BridgeFetch::NotFound),
            Err(FetchError::Unavailable(detail)) => {
                return Err(BridgeFetch::Failed(Cause::Offline(detail)))
            }
        };
        let expected = cua::parse_checksum(&sums, BRIDGE_EXE_NAME)
            .map_err(|e| BridgeFetch::Failed(Cause::NoMatchingRelease(e)))?;
        let url = endpoints.asset_url(repo, &tag, BRIDGE_EXE_NAME);
        let checked = self
            .download_verified(&url, &expected, |_| Ok(()))
            .await
            .map_err(|cause| match cause {
                Cause::NoMatchingRelease(_) => BridgeFetch::Failed(Cause::NoMatchingRelease(
                    format!("release {tag} has no {BRIDGE_EXE_NAME}"),
                )),
                cause => BridgeFetch::Failed(cause),
            })?;
        let path = self
            .cache
            .publish(
                BRIDGE_COMPONENT,
                version,
                arch,
                BRIDGE_EXE_NAME,
                &checked,
                &expected,
            )
            .map_err(|e| BridgeFetch::Failed(io_cause(e)))?;
        Ok(Obtained {
            path,
            sha256: expected,
            version: version.to_owned(),
        })
    }

    // ---- shared --------------------------------------------------------

    /// Download to a temporary file, compare with `expected`, run `check`,
    /// and return the temporary file. Nothing is left behind on failure.
    async fn download_verified(
        &self,
        url: &str,
        expected: &str,
        check: impl FnOnce(&Path) -> Result<(), Cause> + Send + 'static,
    ) -> Result<PathBuf, Cause> {
        let tmp = self.cache.download_path().map_err(io_cause)?;
        let result = match self.http.download(url, &tmp).await {
            Ok(actual) if actual == expected => {
                let path = tmp.clone();
                blocking(move || Ok(check(&path)))
                    .await
                    .map_err(Cause::Io)
                    .and_then(|r| r)
            }
            Ok(actual) => Err(Cause::ChecksumMismatch {
                expected: expected.to_owned(),
                actual,
            }),
            Err(FetchError::NotFound) => Err(Cause::NoMatchingRelease(String::new())),
            Err(FetchError::Unavailable(detail)) => Err(Cause::Offline(detail)),
        };
        match result {
            Ok(()) => Ok(tmp),
            Err(cause) => {
                let _ = std::fs::remove_file(&tmp);
                Err(cause)
            }
        }
    }

    /// Every release of `repo` (up to [`MAX_RELEASE_PAGES`] pages).
    async fn list(&self, repo: &str) -> Result<Vec<cua::ApiRelease>, FetchError> {
        let mut url = Some(self.http.endpoints.releases_url(repo));
        let mut releases = Vec::new();
        for _ in 0..MAX_RELEASE_PAGES {
            let Some(page) = url.take() else { break };
            let (text, next) = self.http.text(&page).await?;
            let mut batch: Vec<cua::ApiRelease> = serde_json::from_str(&text).map_err(|e| {
                FetchError::Unavailable(format!("unexpected release list from GitHub: {e}"))
            })?;
            releases.append(&mut batch);
            url = next;
        }
        Ok(releases)
    }

    /// Put the bridge, the Cua archive and a manifest into one directory
    /// named by the bundle id, reusing a verified existing one.
    async fn assemble(
        &self,
        cua: &Obtained,
        bridge: &Obtained,
        arch: Arch,
    ) -> Result<(PathBuf, String), BundleError> {
        let error = |cause| BundleError::new(Component::CuaDriver, &cua.version, arch, cause);
        let digest = hex(&Sha256::digest(format!("{}{}", bridge.sha256, cua.sha256)));
        let bundle_id = format!(
            "cua-driver-rs-v{}-{}",
            cua.version.replace('+', "_"),
            &digest[..16]
        );
        rdpilot_bridge_protocol::check_bundle_id(&bundle_id).map_err(|e| error(io_cause(e)))?;
        let dir = self.cache.bundle_dir(&bundle_id);
        let archive_name = cua
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error(Cause::MissingPath("archive file name".to_owned())))?
            .to_owned();
        let (bridge_path, cua_path, cua_version) =
            (bridge.path.clone(), cua.path.clone(), cua.version.clone());
        let (bridge_sha, cua_sha) = (bridge.sha256.clone(), cua.sha256.clone());
        let cache = self.cache.clone();
        let id = bundle_id.clone();
        let target = dir.clone();
        tokio::task::spawn_blocking(move || -> Result<(), Cause> {
            if verify_bundle(&target, &id) {
                return Ok(());
            }
            let files = layout::inspect(&cua_path).map_err(Cause::UnsupportedArchiveLayout)?;
            let manifest = BundleManifest {
                bundle_id: id.clone(),
                cua_version,
                archive_name: archive_name.clone(),
                archive_sha256: cua_sha,
                bridge_sha256: bridge_sha,
                files,
            };
            manifest
                .validate()
                .map_err(Cause::UnsupportedArchiveLayout)?;
            let parent = cache.root().join(cache::BUNDLES_DIR);
            std::fs::create_dir_all(&parent).map_err(io_cause)?;
            let tmp = parent.join(cache::tmp_name());
            let staged = (|| -> std::io::Result<()> {
                std::fs::create_dir(&tmp)?;
                link_or_copy(&bridge_path, &tmp.join(BRIDGE_EXE_NAME))?;
                link_or_copy(&cua_path, &tmp.join(&archive_name))?;
                let json = serde_json::to_vec_pretty(&manifest).map_err(std::io::Error::other)?;
                std::fs::write(tmp.join(MANIFEST_NAME), json)
            })();
            if let Err(e) = staged {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(io_cause(e));
            }
            if std::fs::rename(&tmp, &target).is_ok() {
                return Ok(());
            }
            // Another connect won, or a damaged directory is in the way.
            if verify_bundle(&target, &id) {
                let _ = std::fs::remove_dir_all(&tmp);
                return Ok(());
            }
            let _ = std::fs::remove_dir_all(&target);
            let renamed = std::fs::rename(&tmp, &target);
            if renamed.is_err() {
                let _ = std::fs::remove_dir_all(&tmp);
            }
            match renamed {
                Ok(()) => Ok(()),
                Err(_) if verify_bundle(&target, &id) => Ok(()),
                Err(e) => Err(io_cause(e)),
            }
        })
        .await
        .map_err(|e| error(io_cause(e)))?
        .map_err(error)?;
        Ok((dir, bundle_id))
    }
}

/// True when `dir` holds a complete bundle `id` whose files match its
/// manifest. A directory that does not is removed.
fn verify_bundle(dir: &Path, id: &str) -> bool {
    let Ok(bytes) = std::fs::read(dir.join(MANIFEST_NAME)) else {
        return false;
    };
    let ok = serde_json::from_slice::<BundleManifest>(&bytes)
        .ok()
        .filter(|m| m.bundle_id == id && m.validate().is_ok())
        .is_some_and(|m| {
            sha256_file(&dir.join(BRIDGE_EXE_NAME)).ok().as_deref() == Some(&m.bridge_sha256)
                && sha256_file(&dir.join(&m.archive_name)).ok().as_deref()
                    == Some(&m.archive_sha256)
        });
    if !ok {
        let _ = std::fs::remove_dir_all(dir);
    }
    ok
}

/// Read `bundle_path`: an optional `rdpilot-bridge.exe` and at most one Cua
/// archive for `arch`.
fn scan_local(dir: &Path, arch: Arch) -> Result<Local, BundleError> {
    let shown = dir.display().to_string();
    let error = |cause| BundleError::new(Component::BundlePath, &shown, arch, cause);
    let entries = std::fs::read_dir(dir).map_err(|e| {
        error(Cause::MissingPath(format!(
            "bundle_path is not a readable directory: {e}"
        )))
    })?;
    let suffix = format!("-windows-{}-binary.zip", arch.as_str());
    let mut local = Local::default();
    for entry in entries.filter_map(Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !entry.path().is_file() {
            continue;
        }
        if name == BRIDGE_EXE_NAME {
            local.bridge = Some(entry.path());
            continue;
        }
        let Some(version) = name
            .strip_prefix("cua-driver-rs-")
            .and_then(|rest| rest.strip_suffix(&suffix))
        else {
            continue;
        };
        let Some(release) = CuaRelease::from_version(version) else {
            continue;
        };
        if local.cua.is_some() {
            return Err(error(Cause::NoMatchingRelease(
                "bundle_path holds more than one Cua archive".to_owned(),
            )));
        }
        local.cua = Some((release, entry.path()));
    }
    Ok(local)
}

/// The production source: GitHub, the platform cache directory, and
/// `bundle_path` read from config.toml / the environment on every connect.
pub(crate) struct DaemonBundleSource {
    acquirer: Result<Acquirer, String>,
}

impl DaemonBundleSource {
    /// Build the HTTP client (reading proxy variables once) and sweep
    /// leftovers of interrupted downloads.
    pub(crate) fn from_environment() -> Self {
        let acquirer = (|| {
            let root = rdpilot_config::cache_dir()
                .ok_or_else(|| "no cache directory (no home directory)".to_owned())?;
            let http = Http::new(Endpoints::github(), true)?;
            let cache = Cache::new(root);
            cache.sweep();
            Ok(Acquirer::new(
                http,
                cache,
                env!("CARGO_PKG_VERSION").to_owned(),
            ))
        })();
        Self { acquirer }
    }
}

impl BundleSource for DaemonBundleSource {
    fn prepare(
        &self,
        request: BundleRequest,
    ) -> BoxFuture<'_, Result<PreparedBundle, BundleError>> {
        Box::pin(async move {
            let local_error = |cause| {
                BundleError::new(
                    Component::CuaDriver,
                    &request.cua_version,
                    request.arch,
                    cause,
                )
            };
            let acquirer = self
                .acquirer
                .as_ref()
                .map_err(|e| local_error(Cause::Io(e.clone())))?;
            let bundle_path = rdpilot_config::resolve()
                .map_err(|e| local_error(Cause::Io(format!("config.toml: {e}"))))?
                .bundle_path
                .map(PathBuf::from);
            acquirer.prepare(&request, bundle_path.as_deref()).await
        })
    }
}

#[cfg(test)]
mod tests;
