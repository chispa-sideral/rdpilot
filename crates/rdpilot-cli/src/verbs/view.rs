//! `view` — start the read-only live viewer and keep it running.
//!
//! Connects to an already-running daemon (never starts one), validates its
//! compatibility identity, sends `ViewerStart`, prints one URL per bound
//! address, and then holds the IPC connection. The daemon serves the viewer
//! only while this connection is open: Ctrl-C (or the terminal closing)
//! stops it. If the daemon exits, this command exits too.

use rdpilot_config::ViewerBind;
use rdpilot_ipc::{read_frame, Request, WireResponse, WireViewerBind};

use crate::cli::ViewArgs;
use crate::connect::{open_existing_stream, verified_request};
use crate::exit_codes::CliError;
use crate::render::print_json;

/// The access-boundary line printed with the URLs.
const BOUNDARY: &str = "Anyone with one of these URLs and network access to its address can \
view the sessions (read-only). Keep the URLs private. Press Ctrl-C to stop the viewer.";

/// `view [--bind] [--tailnet-address]`.
///
/// # Errors
///
/// [`CliError::DaemonUnreachable`] if no daemon runs or it exits while the
/// viewer runs; the daemon's error if it refuses to start the viewer;
/// [`CliError::MissingConfig`] for an invalid `[viewer]` configuration.
pub async fn view(args: ViewArgs, json: bool) -> Result<(), CliError> {
    let config =
        rdpilot_config::resolve_viewer().map_err(|e| CliError::MissingConfig(e.to_string()))?;
    let bind = match args.bind.unwrap_or(config.bind) {
        ViewerBind::Loopback => WireViewerBind::Loopback,
        ViewerBind::LoopbackAndTailnet => WireViewerBind::LoopbackAndTailnet,
    };
    let tailnet_address = args
        .tailnet_address
        .or(config.tailnet_address)
        .map(|a| a.to_string());

    let mut stream = open_existing_stream().await?;
    let response = verified_request(
        &mut stream,
        Request::ViewerStart {
            bind,
            tailnet_address,
            read_only: false,
            idle_timeout_secs: None,
        },
    )
    .await?;
    let (urls, notices) = match response {
        WireResponse::ViewerStarted {
            addresses,
            token,
            notices,
        } => (
            addresses
                .iter()
                .map(|a| format!("{a}?token={}", token.0))
                .collect::<Vec<_>>(),
            notices,
        ),
        WireResponse::Error(err) => return Err(CliError::from(err)),
        _ => {
            return Err(CliError::Internal(
                "unexpected response to ViewerStart".to_owned(),
            ))
        }
    };

    if json {
        print_json(&serde_json::json!({ "urls": urls, "notices": notices }))?;
    } else {
        println!("rdpilot live viewer (read-only) is running. Open one of:");
        for url in &urls {
            println!("  {url}");
        }
        for notice in &notices {
            println!("notice: {notice}");
        }
        println!("{BOUNDARY}");
    }

    // Hold the connection until Ctrl-C or daemon exit. The daemon sends
    // nothing more; a read returning means the daemon closed the stream.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            eprintln!("viewer stopped");
            Ok(())
        }
        _ = read_frame::<_, serde_json::Value>(&mut stream) => Err(CliError::DaemonUnreachable(
            "daemon exited; viewer stopped".to_owned(),
        )),
    }
}
