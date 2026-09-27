//! Native RDP screenshot recovery; desktop observations belong to native Cua MCP.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rdpilot_ipc::{Request, WireResponse};

use crate::cli::ScreenshotArgs;
use crate::connect::round_trip;
use crate::exit_codes::CliError;
use crate::render::print_json;

/// `perceive screenshot --session <id> --output <path>` (D-13.1).
///
/// # Errors
///
/// [`CliError::Internal`] for an empty `--session`, a base64 decode
/// failure, or a failure writing `--output`; the daemon's own error
/// otherwise; or a transport/auto-start failure.
pub async fn screenshot(args: ScreenshotArgs, json: bool) -> Result<(), CliError> {
    let session = args.session.parse().map_err(CliError::Internal)?;
    match round_trip(Request::Screenshot { session }).await? {
        WireResponse::Screenshot { png_base64 } => {
            let bytes = decode_png(&png_base64)?;
            write_output(&args.output, &bytes)?;
            if json {
                print_json(
                    &serde_json::json!({ "output": args.output.display().to_string(), "bytes": bytes.len() }),
                )
            } else {
                println!("wrote {} ({} bytes)", args.output.display(), bytes.len());
                Ok(())
            }
        }
        WireResponse::Error(err) => Err(CliError::from(err)),
        other => Err(CliError::Internal(format!(
            "unexpected response to Screenshot: {other:?}"
        ))),
    }
}

/// Decode the native screenshot.
fn decode_png(png_base64: &str) -> Result<Vec<u8>, CliError> {
    STANDARD
        .decode(png_base64)
        .map_err(|e| CliError::Internal(format!("invalid base64 screenshot data: {e}")))
}

/// Write image bytes to a caller-local file.
fn write_output(output: &std::path::Path, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(output, bytes)
        .map_err(|e| CliError::Internal(format!("failed to write {}: {e}", output.display())))
}
