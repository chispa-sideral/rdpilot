#[cfg(windows)]
fn main() {
    let result = (|| -> std::io::Result<()> {
        let mut args = std::env::args().skip(1);
        if args.next().as_deref() != Some("--generation") {
            return Err(std::io::Error::other(
                "usage: rdpilot-bridge --generation NUMBER",
            ));
        }
        let generation = args
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|n| *n != 0)
            .ok_or_else(|| std::io::Error::other("invalid generation"))?;
        if args.next().is_some() {
            return Err(std::io::Error::other("unexpected bridge argument"));
        }
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
    })();
    if let Err(error) = result {
        eprintln!("rdpilot-bridge: {error}");
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("rdpilot-bridge requires a Windows interactive RDP session");
    std::process::exit(1);
}
