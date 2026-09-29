//! `config resolve <target>`: show what a target resolves to and why.

use rdpilot_config::hosts::{resolve_host, HostsInput, ResolvedHost, Row};

use crate::cli::TargetArgs;
use crate::exit_codes::CliError;
use crate::render::print_json;

/// Resolve `args` against the user's hosts file, `-F` and `-o` overrides.
///
/// # Errors
///
/// [`CliError::Config`] for any parse, target, option, file or permission error.
pub fn resolve_target(args: &TargetArgs) -> Result<ResolvedHost, CliError> {
    let config_dir = rdpilot_config::config_dir();
    let home = rdpilot_config::home_dir();
    resolve_host(&HostsInput {
        target: &args.target,
        options: &args.options,
        file: args.file.as_deref(),
        config_dir: config_dir.as_deref(),
        home: home.as_deref(),
    })
    .map_err(|e| CliError::Config(e.to_string()))
}

fn render_text(rows: &[Row]) -> String {
    let width = rows.iter().map(|r| r.keyword.len()).max().unwrap_or(0);
    let mut out = String::new();
    for r in rows {
        out.push_str(&format!(
            "{:<width$}  {}  ({})\n",
            r.keyword, r.value, r.source
        ));
    }
    out
}

/// `config resolve`: prints each setting with its source. Passwords are
/// redacted and a `PasswordCommand` is shown as written, never run.
///
/// # Errors
///
/// As [`resolve_target`].
pub fn resolve(args: TargetArgs, json: bool) -> Result<(), CliError> {
    let host = resolve_target(&args)?;
    let rows = host.rows();
    if json {
        let settings: Vec<_> = rows
            .iter()
            .map(
                |r| serde_json::json!({"keyword": r.keyword, "value": r.value, "source": r.source}),
            )
            .collect();
        print_json(&serde_json::json!({
            "target": host.target().to_string(),
            "settings": settings,
        }))
    } else {
        print!("{}", render_text(&rows));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_rendering_aligns_keywords() {
        let rows = vec![
            Row {
                keyword: "Port",
                value: "3389".to_owned(),
                source: "not set; rdpilot default".to_owned(),
            },
            Row {
                keyword: "PasswordCommand",
                value: "printenv X".to_owned(),
                source: "hosts:3".to_owned(),
            },
        ];
        let text = render_text(&rows);
        assert_eq!(
            text,
            "Port             3389  (not set; rdpilot default)\nPasswordCommand  printenv X  (hosts:3)\n"
        );
    }
}
