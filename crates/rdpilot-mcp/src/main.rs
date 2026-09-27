//! Target-bound transparent Cua MCP endpoint. Protocol stdout contains only Cua JSON.
#![deny(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
mod proxy;
use clap::Parser;
use rdpilot_ipc::{connect_or_spawn, socket_path, SessionId};

#[derive(Parser)]
#[command(
    version,
    about = "Expose native Cua MCP for one existing rdpilot connection"
)]
struct Args {
    /// Existing named RDP connection. Cua session labels never change this binding.
    #[arg(long)]
    session: SessionId,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("rdpilot-mcp: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run(args));
    // Tokio's stdin uses a blocking read that cannot be cancelled. Bound runtime
    // teardown so a dead target also exits when the caller leaves stdin open.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rdpilot-mcp: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let socket = socket_path()?;
    let daemon = std::env::current_exe()?.with_file_name(if cfg!(windows) {
        "rdpilot-daemon.exe"
    } else {
        "rdpilot-daemon"
    });
    let mut stream = connect_or_spawn(&socket, &daemon).await?;
    proxy::attach(&mut stream, args.session).await?;
    proxy::forward(
        stream,
        tokio::io::BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await?;
    Ok(())
}
