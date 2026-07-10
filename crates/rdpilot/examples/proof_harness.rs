//! v1 demo binary: a narrated walkthrough of the Core Value narrative — "a
//! local AI agent can connect to a remote Windows desktop over RDP and
//! read/inspect a program that is only reachable via RDP" (PROJECT.md) —
//! proven here WITHOUT any LLM, as a fully scripted loop (PROOF-01).
//!
//! Composes the PUBLIC API only (`Session::connect` -> `deploy_and_launch` ->
//! the shared [`proof_harness::run_proof_harness`]) plus the shared
//! `tests/support/proof_harness.rs` module (D-9.3, wired in below via
//! `#[path]`). No `ironrdp`/`image`/`rustls` type appears here (D-09).
//!
//! Run (with a provisioned Phase 1 target, a published `rdpilot-sensor.exe`,
//! and `RDPILOT_LIVE` armed):
//!
//! ```text
//! RDPILOT_LIVE=1 cargo run -p rdpilot --example proof_harness
//! ```
//!
//! Prints a step-by-step PASS/FAIL trace to stdout (D-9.4 — no JSON schema,
//! no report file) and exits non-zero if any step failed.
//!
//! # Security
//!
//! The example reads the gitignored `.secrets/connection.json` and hands the
//! credentials to [`rdpilot::ConnectionConfig`]; it NEVER prints the password
//! (the config's `Debug` redacts it). Do not log credentials from here.

use std::path::PathBuf;
use std::process::ExitCode;

use rdpilot::{ConnectionConfig, Session};

// D-9.3: the shared harness function/module, reused verbatim by the gated
// `proof_harness_end_to_end` live test in `tests/live_session.rs` — see that
// file's own `#[path = "support/proof_harness.rs"] mod proof_harness;` decl.
// Pitfall 5: this compiles the SAME source file as a SEPARATE module tree
// inside THIS example binary crate; see proof_harness.rs's own top doc
// comment for why it never reaches the SDK via a bare `crate::` path.
#[path = "../tests/support/proof_harness.rs"]
mod proof_harness;

fn main() -> ExitCode {
    // A small current-thread runtime is enough to drive the async public API
    // (mirrors examples/screenshot.rs).
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
        Ok(passed) => {
            if passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(message) => {
            // `message` is an app-level string; the SDK error types never leak
            // out of this example, and no credential is ever included.
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Compose the public API: load config -> resolve the sensor exe -> connect
/// -> deploy the sensor -> run the shared proof harness -> print the D-9.4
/// stdout trace -> close. Returns `Ok(report.passed)` so `main` can map it to
/// an exit code.
async fn run() -> Result<bool, String> {
    let cfg = load_config().map_err(|e| format!("config: {e}"))?;

    let sensor_exe = sensor_exe_path();
    if !sensor_exe.exists() {
        return Err(format!(
            "published rdpilot-sensor.exe not found at {sensor_exe:?} — publish it first \
             (`dotnet publish -r win-x64 -p:PublishAot=true --self-contained` on a Windows host \
             with the .NET 8 SDK)"
        ));
    }
    let cfg = cfg.sensor_binary_path(sensor_exe);

    println!("=== rdpilot proof harness (PROOF-01) ===");
    println!("connecting...");
    let session = Session::connect(&cfg)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    println!("connected — deploying sensor...");

    session
        .deploy_and_launch()
        .await
        .map_err(|e| format!("deploy_and_launch failed: {e}"))?;
    println!("sensor answering — running the proof loop (screenshot -> launch 7-Zip -> deeper UIA walk -> navigate -> verify)...");

    let report = proof_harness::run_proof_harness(&session)
        .await
        .map_err(|e| format!("proof harness failed: {e}"))?;

    for (name, passed, detail) in &report.steps {
        println!("  [{}] {name}: {detail}", if *passed { "PASS" } else { "FAIL" });
    }
    println!("PROOF: {}", if report.passed { "PASS" } else { "FAIL" });

    session
        .close()
        .await
        .map_err(|e| format!("close failed: {e}"))?;

    Ok(report.passed)
}

/// Load `.secrets/connection.json` into a [`ConnectionConfig`].
///
/// Mirrors `examples/screenshot.rs`'s `load_config` exactly: the example only
/// connects against the disposable lab VM, so it sets
/// `accept_invalid_certs(true)` (D-15, test/example-only). The password is
/// read into a local and handed straight to the config; it is never printed.
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

/// Name of the env var that overrides the default published sensor exe
/// location — mirrors `tests/common/mod.rs`'s `SENSOR_EXE_ENV`/
/// `sensor_exe_path`, reimplemented locally because an `examples/` crate root
/// cannot reach `mod common;` (Pitfall 6 — `tests/common/` is only visible to
/// `tests/*.rs` integration-test crate roots, not `examples/*.rs` binaries).
const SENSOR_EXE_ENV: &str = "RDPILOT_SENSOR_EXE";

/// Locate the published NativeAOT `rdpilot-sensor.exe`, honoring
/// [`SENSOR_EXE_ENV`] if set, otherwise defaulting to the standard
/// `dotnet publish -r win-x64` output path relative to the workspace root —
/// the same default `tests/common::sensor_exe_path` and `deploy-winrm.ps1`'s
/// `-SensorExe` param use.
fn sensor_exe_path() -> PathBuf {
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
