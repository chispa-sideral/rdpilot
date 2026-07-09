//! Shared test helper: load `.secrets/connection.json` into a
//! [`ConnectionConfig`] and gate the live suite (D-18).
//!
//! The library itself is environment-agnostic (D-12): it never reads a secrets
//! path or env var. That policy lives here, in TEST code only.
//!
//! Gating rule (D-18): the live integration tests must NOT hard-fail when no
//! target is present. [`load_config`] returns `None` — so each live test can
//! early-return cleanly — unless BOTH conditions hold:
//!
//! 1. `RDPILOT_LIVE` is set (the explicit opt-in), and
//! 2. `.secrets/connection.json` exists and parses.
//!
//! # Security
//!
//! This helper parses credentials out of the gitignored `.secrets/connection.json`
//! and hands them to [`ConnectionConfig`]. It NEVER prints, logs, or otherwise
//! surfaces the parsed `password` (threat T-02-08). Callers must not log the
//! returned config's secret fields either (the `Debug` impl already redacts the
//! password).

use std::path::PathBuf;

use rdpilot::ConnectionConfig;

/// Name of the opt-in env var that arms the live suite (D-18).
pub const LIVE_ENV: &str = "RDPILOT_LIVE";

/// Name of the env var that parameterises the idle/keepalive duration, in
/// seconds. Defaults to a short value in dev; the canonical run sets `600`.
pub const IDLE_SECS_ENV: &str = "RDPILOT_IDLE_SECS";

/// Default idle duration (seconds) when [`IDLE_SECS_ENV`] is unset — short so a
/// developer's armed run stays quick. The canonical phase-gate run overrides
/// this to the full 600s (10 min).
pub const DEFAULT_IDLE_SECS: u64 = 5;

/// Locate `.secrets/connection.json` relative to the workspace root.
///
/// `CARGO_MANIFEST_DIR` points at `crates/rdpilot`; the secrets file lives at the
/// workspace root, two levels up. Returns the candidate path regardless of whether
/// it exists (existence is checked by [`load_config`]).
fn connection_file() -> PathBuf {
    // crates/rdpilot -> crates -> <workspace root>
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".secrets")
        .join("connection.json")
}

/// Load the live target configuration, or `None` if the suite is not armed.
///
/// Returns `None` (the test should skip) when either:
/// - `RDPILOT_LIVE` is unset, or
/// - `.secrets/connection.json` is absent.
///
/// Returns `Some(cfg)` only when the suite is armed AND the file parses. A
/// present-but-malformed file panics with a descriptive message (a deliberate
/// armed-run misconfiguration is a hard error, not a silent skip — threat
/// T-02-10), but the panic message NEVER includes the password.
///
/// For the disposable self-signed lab VM, `accept_invalid_certs(true)` is set on
/// the produced config (D-15). This opt-out is test-only and risk-named.
pub fn load_config() -> Option<ConnectionConfig> {
    // Gate 1: explicit opt-in. Unset -> skip (default `cargo test` stays green).
    // `?` returns `None` early when the var is absent.
    std::env::var_os(LIVE_ENV)?;

    // Gate 2: secrets file presence. Absent -> skip (no target provisioned).
    let path = connection_file();
    if !path.exists() {
        return None;
    }

    // Parse the connection file. The schema is written by Phase 1's
    // `infra/manage-env.ps1 up`:
    //   { "host": ..., "user": ..., "password": ..., "rdpPort": 3389, "winrmPort": 5986 }
    let raw = std::fs::read_to_string(&path)
        .expect("RDPILOT_LIVE is set and .secrets/connection.json exists but could not be read");
    let json: serde_json::Value = serde_json::from_str(&raw)
        .expect(".secrets/connection.json is not valid JSON");

    // Pull non-secret fields explicitly; never interpolate the password into a
    // message. We read the password into a local only to hand it straight to
    // `ConnectionConfig` (whose `Debug` redacts it) — it is never printed.
    let host = json
        .get("host")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `host`");
    let user = json
        .get("user")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `user`");
    let password = json
        .get("password")
        .and_then(|v| v.as_str())
        .expect(".secrets/connection.json is missing a string `password`");
    // Port is optional; default to the standard RDP port (3389).
    let port: u16 = json
        .get("rdpPort")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(3389);

    let cfg = ConnectionConfig::new(host, user, password)
        .port(port)
        // Lab VM uses a self-signed cert; accept it ONLY in test config (D-15).
        .accept_invalid_certs(true);

    Some(cfg)
}

/// Name of the env var that overrides the default published sensor exe
/// location for [`sensor_exe_path`] — lets a live-gate run point at a
/// different publish output without editing test code.
pub const SENSOR_EXE_ENV: &str = "RDPILOT_SENSOR_EXE";

/// Locate the published NativeAOT `rdpilot-sensor.exe` (Plan 01 Task 3),
/// honoring [`SENSOR_EXE_ENV`] if set, otherwise defaulting to the standard
/// `dotnet publish -r win-x64` output path relative to the workspace root —
/// the same default `deploy-winrm.ps1`'s `-SensorExe` param uses.
///
/// Returns the candidate path regardless of whether it exists; callers
/// (`sensor_rdpdr_deploy_and_ping_within_1s`) check existence themselves so
/// the failure message can name the exact missing path.
pub fn sensor_exe_path() -> PathBuf {
    if let Some(over) = std::env::var_os(SENSOR_EXE_ENV) {
        return PathBuf::from(over);
    }
    // crates/rdpilot -> crates -> <workspace root> -> sensor/bin/...
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sensor")
        .join("bin")
        .join("Release")
        .join("net8.0")
        .join("win-x64")
        .join("publish")
        .join("rdpilot-sensor.exe")
}

/// Idle duration (in seconds) for the idle/keepalive tests, from
/// [`IDLE_SECS_ENV`] or [`DEFAULT_IDLE_SECS`]. The canonical run sets `600`.
pub fn idle_secs() -> u64 {
    std::env::var(IDLE_SECS_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_SECS)
}
