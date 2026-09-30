//! `rdpilot-bridge install --generation N [--source DIR]`: verify and
//! install the served bundle under `%LOCALAPPDATA%\rdpilot`, then start the
//! installed copy with `run` and exit.
//!
//! `rdpilot-bridge run --generation N`: carry the Cua MCP stream for one RDP
//! session generation (started by `install`).
//!
//! `rdpilot-bridge cleanup`: stop this user's rdpilot processes in this
//! session and remove `%LOCALAPPDATA%\rdpilot` and
//! `%TEMP%\rdpilot-transfer-root`.

const USAGE: &str = "usage: rdpilot-bridge install --generation N [--source DIR] | \
                     run --generation N | cleanup";

#[cfg(windows)]
fn parse_generation(args: &mut std::vec::IntoIter<String>) -> std::io::Result<u64> {
    if args.next().as_deref() != Some("--generation") {
        return Err(std::io::Error::other(USAGE));
    }
    args.next()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|n| *n != 0)
        .ok_or_else(|| std::io::Error::other("invalid generation"))
}

#[cfg(windows)]
fn install(mut args: std::vec::IntoIter<String>) -> std::io::Result<()> {
    use rdpilot_bridge::install;
    let generation = parse_generation(&mut args)?;
    let source = match args.next().as_deref() {
        None => std::path::PathBuf::from(install::DEFAULT_SOURCE),
        Some("--source") => args
            .next()
            .map(std::path::PathBuf::from)
            .ok_or_else(|| std::io::Error::other(USAGE))?,
        Some(_) => return Err(std::io::Error::other(USAGE)),
    };
    if args.next().is_some() {
        return Err(std::io::Error::other(USAGE));
    }
    let dir = install::install(&source, &install::default_base()?, &generation.to_string())?;
    let mut run = std::process::Command::new(dir.join(rdpilot_bridge_protocol::BRIDGE_EXE_NAME));
    run.arg("run")
        .arg("--generation")
        .arg(generation.to_string())
        .current_dir(&dir);
    rdpilot_bridge::guest::spawn_detached(&mut run)
}

#[cfg(windows)]
fn run(mut args: std::vec::IntoIter<String>) -> std::io::Result<()> {
    let generation = parse_generation(&mut args)?;
    if args.next().is_some() {
        return Err(std::io::Error::other("unexpected bridge argument"));
    }
    // A repeated launch for the same generation exits quietly while the
    // first one runs.
    let Some(_mutex) = rdpilot_bridge::guest::acquire_generation(generation)? else {
        return Ok(());
    };
    let config = rdpilot_bridge::runtime::Config::adjacent(generation)?;
    let mut carrier = rdpilot_bridge::windows::open_carrier()?;
    // Bridge owns the runtime. Bound blocking-transfer teardown even if a remote
    // filesystem stops completing I/O; process exit releases remaining OS handles.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let (_, replacement) = tokio::sync::mpsc::channel(1);
    let incoming = std::mem::replace(&mut carrier.incoming, replacement);
    let result = runtime.block_on(rdpilot_bridge::runtime::run(
        config,
        incoming,
        carrier.outgoing.clone(),
    ));
    drop(carrier);
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

#[cfg(windows)]
fn cleanup(mut args: std::vec::IntoIter<String>) -> std::io::Result<()> {
    use rdpilot_bridge::{guest, install};
    if args.next().is_some() {
        return Err(std::io::Error::other(USAGE));
    }
    let base = install::default_base()?;
    let stopped = guest::stop_processes_under(&base)?;
    let transfer_root = std::env::temp_dir().join(install::TRANSFER_ROOT_NAME);
    let leftover = install::remove_footprint(&base, &transfer_root, &std::env::current_exe()?)?;
    if leftover.own_image.is_some() {
        guest::remove_after_exit(&base)?;
    }
    println!(
        "rdpilot-bridge: stopped {stopped} process(es); removed {}",
        base.display()
    );
    Ok(())
}

#[cfg(windows)]
fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };
    let args = args.into_iter();
    let result = match command.as_str() {
        "install" => install(args),
        "run" => run(args),
        "cleanup" => cleanup(args),
        _ => Err(std::io::Error::other(USAGE)),
    };
    if let Err(error) = result {
        eprintln!("rdpilot-bridge: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("rdpilot-bridge requires a Windows interactive RDP session ({USAGE})");
    std::process::exit(1);
}
