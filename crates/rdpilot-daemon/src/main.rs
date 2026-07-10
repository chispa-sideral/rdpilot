//! `rdpilot-daemon` binary entry point.
//!
//! A thin wrapper around [`rdpilot_daemon::run`] — all real assembly
//! (config load, registry construction, IPC listener bind, lifecycle tasks)
//! lives in `server.rs` (Plan 12-06). This file's only job is to start the
//! Tokio runtime and translate a top-level error into a nonzero exit code.

#[tokio::main]
async fn main() {
    if let Err(e) = rdpilot_daemon::run().await {
        eprintln!("rdpilot-daemon: {e}");
        std::process::exit(1);
    }
}
