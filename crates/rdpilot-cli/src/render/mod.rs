//! Output rendering: the hand-rolled table printer for the human-readable
//! default, plus the `--json` machine-readable path. Each verb handler
//! destructures the SPECIFIC `WireResponse` variant it expects and renders
//! it directly — there is deliberately no global exhaustive `Render` match
//! over `WireResponse` here (an unexpected variant is a `CliError::Internal`
//! at the verb-handler call site instead).

pub mod table;

pub use table::render_table;

use crate::exit_codes::CliError;

/// Print `value` as pretty-printed JSON to stdout.
///
/// # Errors
///
/// Returns [`CliError::Internal`] if serialization fails (every wire DTO
/// this crate renders derives `Serialize` infallibly, so this is not
/// expected to trigger in practice).
pub fn print_json<T: serde::Serialize>(value: &T) -> Result<(), CliError> {
    let json =
        serde_json::to_string_pretty(value).map_err(|e| CliError::Internal(format!("failed to serialize JSON output: {e}")))?;
    println!("{json}");
    Ok(())
}
