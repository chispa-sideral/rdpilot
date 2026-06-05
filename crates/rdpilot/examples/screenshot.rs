//! End-to-end example: load `.secrets/connection.json` → connect → screenshot →
//! write a full-desktop PNG (CAP-01).
//!
//! Run (with a provisioned Phase 1 target and `RDPILOT_LIVE` armed):
//!
//! ```text
//! RDPILOT_LIVE=1 cargo run -p rdpilot --example screenshot
//! ```
//!
//! It composes the PUBLIC API only — `Session::connect` → `Session::screenshot`
//! → `Screenshot::to_png` → `Session::close`. No `ironrdp`/`image`/`rustls` type
//! appears here (D-09). Output is written to `screenshot.png` in the current
//! directory; open it to eyeball the real remote desktop (e.g. the 7-Zip File
//! Manager installed by Phase 1) in correct color.
//!
//! # Security
//!
//! The example reads the gitignored `.secrets/connection.json` and hands the
//! credentials to [`rdpilot::ConnectionConfig`]; it NEVER prints the password
//! (the config's `Debug` redacts it). Do not log credentials from here.

use std::path::PathBuf;
use std::process::ExitCode;

use rdpilot::{ConnectionConfig, Session};

/// Where the captured PNG is written.
const OUTPUT_PATH: &str = "screenshot.png";

fn main() -> ExitCode {
    // A small current-thread runtime is enough to drive the async public API.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: could not start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // `message` is an app-level string; the SDK error types never leak
            // out of this example, and no credential is ever included.
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Compose the public API: load config → connect → screenshot → PNG → close.
async fn run() -> Result<(), String> {
    let cfg = load_config().map_err(|e| format!("config: {e}"))?;

    let session = Session::connect(&cfg)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;

    // `screenshot()` may fail until the first graphics update arrives; the active
    // session loop typically delivers one almost immediately after connect.
    let shot = session
        .screenshot()
        .await
        .map_err(|e| format!("screenshot failed: {e}"))?;

    let png = shot.to_png().map_err(|e| format!("encode failed: {e}"))?;

    std::fs::write(OUTPUT_PATH, &png).map_err(|e| format!("writing {OUTPUT_PATH}: {e}"))?;

    println!(
        "wrote {OUTPUT_PATH} ({}x{}, {} bytes)",
        shot.width,
        shot.height,
        png.len()
    );

    // Graceful teardown (D-07). Dropping would also work, but `close()` is the
    // clean awaited path.
    session
        .close()
        .await
        .map_err(|e| format!("close failed: {e}"))?;

    Ok(())
}

/// Load `.secrets/connection.json` into a [`ConnectionConfig`].
///
/// Mirrors the gating used by the integration suite's `tests/common` helper: the
/// example only connects against the disposable lab VM, so it sets
/// `accept_invalid_certs(true)` (D-15, test/example-only). The password is read
/// into a local and handed straight to the config; it is never printed.
fn load_config() -> Result<ConnectionConfig, String> {
    // `CARGO_MANIFEST_DIR` is `crates/rdpilot`; the secrets file is at the
    // workspace root, two levels up.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".secrets")
        .join("connection.json");

    if !path.exists() {
        return Err(format!(
            "{} not found — provision the target with `infra/manage-env.ps1 up` first",
            path.display()
        ));
    }

    let raw = std::fs::read_to_string(&path).map_err(|e| format!("reading connection.json: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("connection.json is not valid JSON: {e}"))?;

    let host = json
        .get("host")
        .and_then(|v| v.as_str())
        .ok_or("connection.json is missing a string `host`")?;
    let user = json
        .get("user")
        .and_then(|v| v.as_str())
        .ok_or("connection.json is missing a string `user`")?;
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .ok_or("connection.json is missing a string `password`")?;
    let port: u16 = json
        .get("rdpPort")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(3389);

    Ok(ConnectionConfig::new(host, user, password)
        .port(port)
        .accept_invalid_certs(true))
}
