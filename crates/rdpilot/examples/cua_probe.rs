//! RDP-only integration probe. Credentials are read from environment and never printed.
//! Usage: cargo run -p rdpilot --example cua_probe -- BUNDLE_DIR REQUESTS_JSONL OUTPUT_JSONL
//! REQUESTS_JSONL contains native Cua requests; initialize is performed automatically.
use rdpilot::{ConnectionConfig, Session};
use serde_json::json;
use std::{io::Write, path::PathBuf, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: cua_probe BUNDLE REQUESTS OUTPUT".into());
    }
    let output = PathBuf::from(&args[3]);
    let root = output.with_extension("share");
    std::fs::create_dir_all(&root)?;
    let mut cfg = ConnectionConfig::new(
        std::env::var("RDPILOT_HOST")?,
        std::env::var("RDPILOT_USERNAME")?,
        std::env::var("RDPILOT_PASSWORD")?,
    )
    .bundle_path(&args[1])
    .share_root(&root)
    .accept_invalid_certs(std::env::var("RDPILOT_ACCEPT_INVALID_CERTS").as_deref() == Ok("1"));
    if let Ok(port) = std::env::var("RDPILOT_PORT") {
        cfg = cfg.port(port.parse()?);
    }
    let session = Session::connect(&cfg).await?;
    eprintln!("RDP connected; deploying pinned bundle");
    {
        let deploy = session.deploy_and_launch();
        tokio::pin!(deploy);
        let mut snapshots = tokio::time::interval(Duration::from_secs(3));
        let mut count = 0;
        loop {
            tokio::select! {
                result=&mut deploy=>{result?;break;},
                _=snapshots.tick()=>{if let Ok(shot)=session.screenshot().await{std::fs::write(format!("{}.deploy-{count}.png",args[3]),shot.to_png()?)?;}count+=1;}
            }
        }
    }
    eprintln!("bridge ready; attaching native Cua");
    let mut cua = session.attach_cua().await?;
    let mut log = std::fs::File::create(output)?;
    cua.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"rdpilot-live-proof","version":"1"}}})).await?;
    let initialized = tokio::time::timeout(Duration::from_secs(30), cua.recv())
        .await??
        .ok_or("initialize EOF")?;
    writeln!(log, "{}", initialized)?;
    cua.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await?;
    for line in std::fs::read_to_string(&args[2])?.lines() {
        let request: serde_json::Value = serde_json::from_str(line)?;
        let id = request.get("id").cloned();
        cua.send(request).await?;
        if let Some(id) = id {
            loop {
                let response = tokio::time::timeout(Duration::from_secs(65), cua.recv())
                    .await??
                    .ok_or("Cua EOF")?;
                writeln!(log, "{}", response)?;
                log.flush()?;
                if response.get("id") == Some(&id) && response.get("method").is_none() {
                    break;
                }
            }
        }
    }
    eprintln!("Cua exchange complete; native screenshot and independent heartbeat");
    std::fs::write(
        args[3].clone() + ".png",
        session.screenshot().await?.to_png()?,
    )?;
    session.ping().await?;
    cua.close().await?;
    session.close().await?;
    Ok(())
}
