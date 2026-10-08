//! Acquisition tests against an in-process HTTP stub. No test reaches the
//! network. Versions are synthetic.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

const DAEMON: &str = "5.0.0";
const STABLE: &str = "7.3.4";
const NIGHTLY: &str = "7.3.5-nightly.20990102.6";

type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

/// A tiny HTTP/1.1 server: path -> (status, body); unknown paths are 404.
struct Stub {
    base: String,
    routes: Routes,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Routes = Arc::default();
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let (r, h) = (routes.clone(), hits.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (r, h) = (r.clone(), h.clone());
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match socket.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let text = String::from_utf8_lossy(&request);
                    let path = text.split_whitespace().nth(1).unwrap_or("").to_owned();
                    h.lock().unwrap().push(path.clone());
                    let (status, body) = r
                        .lock()
                        .unwrap()
                        .get(&path)
                        .cloned()
                        .unwrap_or((404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        Self { base, routes, hits }
    }

    fn set(&self, path: &str, status: u16, body: impl Into<Vec<u8>>) {
        self.routes
            .lock()
            .unwrap()
            .insert(path.to_owned(), (status, body.into()));
    }

    /// Every route answers 500 from now on.
    fn fail_everything(&self) {
        for value in self.routes.lock().unwrap().values_mut() {
            *value = (500, Vec::new());
        }
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }

    fn clear_hits(&self) {
        self.hits.lock().unwrap().clear();
    }
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn cua_zip(version: &str) -> Vec<u8> {
    layout::tests::make_zip(&[
        ("cua-driver.exe", format!("MZ driver {version}").as_bytes()),
        ("cua_driver_sdk.dll", b"sdk"),
    ])
}

fn cua_tag(version: &str) -> String {
    CuaRelease::from_version(version).unwrap().tag
}

fn cua_asset(version: &str) -> String {
    format!("cua-driver-rs-{version}-windows-x86_64-binary.zip")
}

fn bridge_exe(version: &str) -> Vec<u8> {
    format!("MZ bridge {version}").into_bytes()
}

/// Upstream Cua releases (stable and nightly) and the given bridge
/// releases, served by one stub.
struct World {
    stub: Stub,
    cache_dir: tempfile::TempDir,
}

impl World {
    async fn new(bridge_releases: &[&str]) -> Self {
        let stub = Stub::start().await;
        let mut list = Vec::new();
        for version in [STABLE, NIGHTLY] {
            let tag = cua_tag(version);
            let asset = cua_asset(version);
            let zip = cua_zip(version);
            stub.set(
                &format!("/trycua/cua/releases/download/{tag}/checksums.txt"),
                200,
                format!("## SHA256 Checksums\n```\n{}  {asset}\n```\n", sha(&zip)),
            );
            stub.set(
                &format!("/trycua/cua/releases/download/{tag}/{asset}"),
                200,
                zip,
            );
            list.push(serde_json::json!({
                "tag_name": tag, "draft": false, "prerelease": true,
                "assets": [{"name": "checksums.txt"}, {"name": asset}],
            }));
        }
        stub.set(
            "/repos/trycua/cua/releases?per_page=100",
            200,
            serde_json::to_vec(&list).unwrap(),
        );
        let mut ours = Vec::new();
        for version in bridge_releases {
            let exe = bridge_exe(version);
            let prefix = format!("/chispa-sideral/rdpilot/releases/download/v{version}");
            stub.set(
                &format!("{prefix}/SHA256SUMS"),
                200,
                format!("{}  rdpilot-bridge.exe\n", sha(&exe)),
            );
            stub.set(&format!("{prefix}/rdpilot-bridge.exe"), 200, exe);
            ours.push(serde_json::json!({
                "tag_name": format!("v{version}"), "draft": false, "prerelease": false,
                "assets": [{"name": "rdpilot-bridge.exe"}, {"name": "SHA256SUMS"}],
            }));
        }
        stub.set(
            "/repos/chispa-sideral/rdpilot/releases?per_page=100",
            200,
            serde_json::to_vec(&ours).unwrap(),
        );
        Self {
            stub,
            cache_dir: tempfile::tempdir().unwrap(),
        }
    }

    fn acquirer(&self) -> Acquirer {
        self.acquirer_at(&self.stub.base)
    }

    fn acquirer_at(&self, base: &str) -> Acquirer {
        let endpoints = Endpoints {
            api: base.to_owned(),
            web: base.to_owned(),
        };
        Acquirer::new(
            Http::new(endpoints, false).unwrap(),
            Cache::new(self.cache_dir.path().to_path_buf()),
            DAEMON.to_owned(),
        )
    }

    fn cached(&self, component: &str) -> Vec<String> {
        let mut v =
            Cache::new(self.cache_dir.path().to_path_buf()).versions(component, Arch::X86_64);
        v.sort();
        v
    }
}

/// A base URL where nothing listens.
async fn closed_port() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    base
}

fn request(cua_version: &str, auto_download: bool) -> BundleRequest {
    BundleRequest {
        cua_version: cua_version.to_owned(),
        auto_download,
        arch: Arch::X86_64,
    }
}

fn manifest(bundle: &PreparedBundle) -> BundleManifest {
    serde_json::from_slice(&std::fs::read(bundle.dir.join(MANIFEST_NAME)).unwrap()).unwrap()
}

#[tokio::test]
async fn release_build_downloads_its_own_bridge_and_latest_dev_cua() {
    let world = World::new(&["4.0.0", DAEMON]).await;
    let bundle = world
        .acquirer()
        .prepare(&request("latest-dev", true), None)
        .await
        .unwrap();
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    let m = manifest(&bundle);
    assert_eq!(m.cua_version, NIGHTLY);
    assert_eq!(m.bridge_sha256, sha(&bridge_exe(DAEMON)));
    assert_eq!(m.bundle_id, bundle.bundle_id);
    assert!(m.validate().is_ok());
    assert!(m.files.contains_key("cua-driver.exe"));
    assert_eq!(
        std::fs::read(bundle.dir.join("rdpilot-bridge.exe")).unwrap(),
        bridge_exe(DAEMON)
    );
    assert!(bundle.dir.join(&m.archive_name).is_file());
    assert_eq!(world.cached(BRIDGE_COMPONENT), vec![DAEMON.to_owned()]);
    assert_eq!(world.cached(CUA_COMPONENT), vec![NIGHTLY.to_owned()]);
    // The release lookup is a download URL, never the bridge release list.
    assert!(!world
        .stub
        .hits()
        .iter()
        .any(|h| h.starts_with("/repos/chispa-sideral")));
}

#[tokio::test]
async fn latest_and_explicit_tags() {
    let world = World::new(&[DAEMON]).await;
    let a = world.acquirer();
    let latest = a.prepare(&request("latest", true), None).await.unwrap();
    assert_eq!(manifest(&latest).cua_version, STABLE);
    world.stub.clear_hits();
    let tag = cua_tag(NIGHTLY);
    let explicit = a.prepare(&request(&tag, true), None).await.unwrap();
    assert_eq!(manifest(&explicit).cua_version, NIGHTLY);
    // An explicit tag needs no release list.
    assert!(!world.stub.hits().iter().any(|h| h.starts_with("/repos/")));
    let err = a
        .prepare(&request("cua-driver-rs-v7.9.9", true), None)
        .await
        .unwrap_err();
    assert!(matches!(err.cause, Cause::NoMatchingRelease(_)), "{err}");
    let err = a.prepare(&request("bogus", true), None).await.unwrap_err();
    assert!(matches!(err.cause, Cause::NoMatchingRelease(_)), "{err}");
}

#[tokio::test]
async fn cua_checksum_mismatch_is_rejected_and_nothing_is_cached() {
    let world = World::new(&[DAEMON]).await;
    let tag = cua_tag(STABLE);
    world.stub.set(
        &format!("/trycua/cua/releases/download/{tag}/{}", cua_asset(STABLE)),
        200,
        cua_zip("tampered"),
    );
    let err = world
        .acquirer()
        .prepare(&request("latest", true), None)
        .await
        .unwrap_err();
    assert_eq!(err.component, Component::CuaDriver);
    assert!(matches!(err.cause, Cause::ChecksumMismatch { .. }), "{err}");
    assert!(world.cached(CUA_COMPONENT).is_empty());
    assert!(!world.cache_dir.path().join(cache::BUNDLES_DIR).exists());
}

#[tokio::test]
async fn bridge_checksum_mismatch_is_rejected_and_nothing_is_cached() {
    let world = World::new(&[DAEMON]).await;
    world.stub.set(
        &format!("/chispa-sideral/rdpilot/releases/download/v{DAEMON}/rdpilot-bridge.exe"),
        200,
        b"MZ evil".to_vec(),
    );
    let err = world
        .acquirer()
        .prepare(&request("latest", true), None)
        .await
        .unwrap_err();
    assert_eq!(err.component, Component::Bridge);
    assert!(matches!(err.cause, Cause::ChecksumMismatch { .. }), "{err}");
    assert!(world.cached(BRIDGE_COMPONENT).is_empty());
}

#[tokio::test]
async fn unsupported_archive_layouts_fail_closed_and_are_not_cached() {
    let world = World::new(&[DAEMON]).await;
    let tag = cua_tag(STABLE);
    let asset = cua_asset(STABLE);
    for bad in [
        layout::tests::make_zip(&[("../cua-driver.exe", b"MZ")]),
        layout::tests::make_zip(&[("bin/cua-driver.exe", b"MZ")]),
        layout::tests::make_zip(&[("other.exe", b"MZ")]),
    ] {
        world.stub.set(
            &format!("/trycua/cua/releases/download/{tag}/checksums.txt"),
            200,
            format!("{}  {asset}\n", sha(&bad)),
        );
        world.stub.set(
            &format!("/trycua/cua/releases/download/{tag}/{asset}"),
            200,
            bad,
        );
        let err = world
            .acquirer()
            .prepare(&request(&tag, true), None)
            .await
            .unwrap_err();
        assert!(
            matches!(err.cause, Cause::UnsupportedArchiveLayout(_)),
            "{err}"
        );
        assert!(world.cached(CUA_COMPONENT).is_empty());
    }
}

#[tokio::test]
async fn cache_hits_need_no_network() {
    let world = World::new(&[DAEMON]).await;
    let first = world
        .acquirer()
        .prepare(&request("latest-dev", true), None)
        .await
        .unwrap();
    world.stub.fail_everything();
    world.stub.clear_hits();
    // Fresh resolved channel, cached Cua and cached release bridge.
    let second = world
        .acquirer()
        .prepare(&request("latest-dev", true), None)
        .await
        .unwrap();
    assert_eq!(first.bundle_id, second.bundle_id);
    assert!(second.warnings.is_empty());
    assert!(world.stub.hits().is_empty(), "{:?}", world.stub.hits());
    // Same through a closed port.
    let offline = world
        .acquirer_at(&closed_port().await)
        .prepare(&request(&cua_tag(NIGHTLY), true), None)
        .await
        .unwrap();
    assert_eq!(offline.bundle_id, first.bundle_id);
}

#[tokio::test]
async fn offline_channel_after_ttl_uses_newest_cached_version_with_a_warning() {
    let world = World::new(&[DAEMON]).await;
    let a = world.acquirer();
    a.prepare(&request("latest", true), None).await.unwrap();
    a.prepare(&request("latest-dev", true), None).await.unwrap();
    let mut offline = world.acquirer_at(&closed_port().await);
    offline.ttl = Duration::ZERO;
    let dev = offline
        .prepare(&request("latest-dev", true), None)
        .await
        .unwrap();
    assert_eq!(manifest(&dev).cua_version, NIGHTLY);
    assert!(
        dev.warnings
            .iter()
            .any(|w| w.contains("cannot reach GitHub") && w.contains(NIGHTLY)),
        "{:?}",
        dev.warnings
    );
    let latest = offline
        .prepare(&request("latest", true), None)
        .await
        .unwrap();
    assert_eq!(
        manifest(&latest).cua_version,
        STABLE,
        "nightly is not latest"
    );
}

#[tokio::test]
async fn offline_with_an_empty_cache_fails_closed() {
    let world = World::new(&[DAEMON]).await;
    let offline = world.acquirer_at(&closed_port().await);
    for version in ["latest-dev".to_owned(), cua_tag(STABLE)] {
        let err = offline
            .prepare(&request(&version, true), None)
            .await
            .unwrap_err();
        assert!(matches!(err.cause, Cause::Offline(_)), "{err}");
        let text = err.to_string();
        for part in [
            "Cua driver",
            "x86_64",
            "offline",
            "CuaEnabled=no",
            "CuaVersion=<tag>",
            "bundle_path",
            "restore network access",
        ] {
            assert!(text.contains(part), "{part}: {text}");
        }
    }
}

#[tokio::test]
async fn auto_download_off_uses_only_the_cache() {
    let world = World::new(&[DAEMON]).await;
    let err = world
        .acquirer()
        .prepare(&request("latest", false), None)
        .await
        .unwrap_err();
    assert_eq!(err.cause, Cause::AutoDownloadDisabled);
    assert!(world.stub.hits().is_empty(), "{:?}", world.stub.hits());
    world
        .acquirer()
        .prepare(&request("latest", true), None)
        .await
        .unwrap();
    world.stub.clear_hits();
    let cached = world
        .acquirer()
        .prepare(&request("latest-dev", false), None)
        .await
        .unwrap();
    assert_eq!(manifest(&cached).cua_version, STABLE, "newest cached");
    assert!(world.stub.hits().is_empty(), "{:?}", world.stub.hits());
}

#[tokio::test]
async fn development_build_prefers_the_bundle_path_bridge_and_still_downloads_cua() {
    // No release v5.0.0 exists: this daemon is a development build.
    let world = World::new(&["4.0.0"]).await;
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("rdpilot-bridge.exe"), b"MZ local bridge").unwrap();
    let bundle = world
        .acquirer()
        .prepare(&request("latest", true), Some(local.path()))
        .await
        .unwrap();
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    let m = manifest(&bundle);
    assert_eq!(m.bridge_sha256, sha(b"MZ local bridge"));
    assert_eq!(m.cua_version, STABLE);
    assert!(world.cached(BRIDGE_COMPONENT).is_empty());
    assert!(!world
        .stub
        .hits()
        .iter()
        .any(|h| h.contains("chispa-sideral")));
}

#[tokio::test]
async fn development_build_without_a_local_bridge_uses_the_newest_release_with_a_warning() {
    let world = World::new(&["4.0.0", "4.2.0"]).await;
    let bundle = world
        .acquirer()
        .prepare(&request("latest", true), None)
        .await
        .unwrap();
    assert_eq!(manifest(&bundle).bridge_sha256, sha(&bridge_exe("4.2.0")));
    assert!(
        bundle
            .warnings
            .iter()
            .any(|w| w.contains("rdpilot-bridge 4.2.0 differs from rdpilot-daemon 5.0.0")),
        "{:?}",
        bundle.warnings
    );
    let none = World::new(&[]).await;
    let err = none
        .acquirer()
        .prepare(&request("latest", true), None)
        .await
        .unwrap_err();
    assert_eq!(err.component, Component::Bridge);
    assert!(matches!(err.cause, Cause::NoMatchingRelease(_)), "{err}");
}

#[tokio::test]
async fn a_network_failure_on_the_release_lookup_never_falls_back_to_another_release() {
    let world = World::new(&["4.2.0", DAEMON]).await;
    world.stub.set(
        &format!("/chispa-sideral/rdpilot/releases/download/v{DAEMON}/SHA256SUMS"),
        503,
        Vec::new(),
    );
    let err = world
        .acquirer()
        .prepare(&request("latest", true), None)
        .await
        .unwrap_err();
    assert_eq!(err.component, Component::Bridge);
    assert_eq!(err.version, DAEMON);
    assert!(matches!(err.cause, Cause::Offline(_)), "{err}");
    let hits = world.stub.hits();
    assert!(!hits.iter().any(|h| h.contains("v4.2.0")), "{hits:?}");
    assert!(!hits.iter().any(|h| h.starts_with("/repos/chispa-sideral")));
}

#[tokio::test]
async fn a_bundle_path_archive_fixes_the_cua_version() {
    let world = World::new(&[DAEMON]).await;
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join(cua_asset("7.0.1")), cua_zip("7.0.1")).unwrap();
    let bundle = world
        .acquirer()
        .prepare(&request(&cua_tag(STABLE), true), Some(local.path()))
        .await
        .unwrap();
    assert_eq!(manifest(&bundle).cua_version, "7.0.1");
    assert!(
        bundle.warnings.iter().any(|w| w.contains("ignored")),
        "{:?}",
        bundle.warnings
    );
    assert!(
        world.cached(CUA_COMPONENT).is_empty(),
        "local files are not cached"
    );
    let missing = local.path().join("absent");
    let err = world
        .acquirer()
        .prepare(&request("latest", true), Some(&missing))
        .await
        .unwrap_err();
    assert_eq!(err.component, Component::BundlePath);
    assert!(matches!(err.cause, Cause::MissingPath(_)), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_prepares_share_one_valid_cache_entry() {
    let world = Arc::new(World::new(&[DAEMON]).await);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let world = world.clone();
        tasks.push(tokio::spawn(async move {
            world
                .acquirer()
                .prepare(&request("latest-dev", true), None)
                .await
                .map(|b| (b.dir, b.bundle_id))
        }));
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap().unwrap());
    }
    assert!(results.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(world.cached(CUA_COMPONENT), vec![NIGHTLY.to_owned()]);
    assert_eq!(world.cached(BRIDGE_COMPONENT), vec![DAEMON.to_owned()]);
    let bundles: Vec<_> = std::fs::read_dir(world.cache_dir.path().join(cache::BUNDLES_DIR))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert_eq!(bundles.len(), 1, "{bundles:?}");
    assert!(verify_bundle(&results[0].0, &results[0].1));
}

#[tokio::test]
async fn a_damaged_bundle_directory_is_rebuilt() {
    let world = World::new(&[DAEMON]).await;
    let a = world.acquirer();
    let first = a.prepare(&request("latest", true), None).await.unwrap();
    let m = manifest(&first);
    std::fs::remove_file(first.dir.join(&m.archive_name)).unwrap();
    std::fs::write(first.dir.join(&m.archive_name), b"damaged").unwrap();
    let second = a.prepare(&request("latest", true), None).await.unwrap();
    assert_eq!(first.bundle_id, second.bundle_id);
    assert!(verify_bundle(&second.dir, &second.bundle_id));
}
