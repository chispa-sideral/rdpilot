//! Native recovery diagnostic: connect, open Run, type notepad, capture each stage.
use rdpilot::{ConnectionConfig, Key, KeyAction, Session};
use std::{path::PathBuf, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let output = PathBuf::from(std::env::args().nth(1).ok_or("output directory required")?);
    std::fs::create_dir_all(&output)?;
    let mut cfg = ConnectionConfig::new(
        std::env::var("PROBE_HOST")?,
        std::env::var("PROBE_USERNAME")?,
        std::env::var("PROBE_PASSWORD")?,
    )
    .accept_invalid_certs(std::env::var("PROBE_ACCEPT_INVALID_CERTS").as_deref() == Ok("1"));
    if let Ok(port) = std::env::var("PROBE_PORT") {
        cfg = cfg.port(port.parse()?);
    }
    // Optional: serve a directory on the RDPILOT drive (no bridge launch).
    if let Ok(bundle) = std::env::var("PROBE_BUNDLE_DIR") {
        cfg = cfg.bundle_path(bundle);
    }
    let s = Session::connect(&cfg).await?;
    tokio::time::sleep(Duration::from_secs(10)).await;
    capture(&s, &output, "ready").await?;
    s.send_key(KeyAction::Combo(vec![Key::Win, Key::R])).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    capture(&s, &output, "run").await?;
    s.send_key(KeyAction::Combo(vec![Key::Ctrl, Key::A]))
        .await?;
    for c in "notepad".chars() {
        s.send_key(KeyAction::Type(c.to_string())).await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    capture(&s, &output, "typed").await?;
    s.send_key(KeyAction::Combo(vec![Key::Enter])).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    capture(&s, &output, "entered").await?;
    s.close().await?;
    Ok(())
}
async fn capture(
    s: &Session,
    root: &std::path::Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(
        root.join(format!("{name}.png")),
        s.screenshot().await?.to_png()?,
    )?;
    eprintln!("native probe stage: {name}");
    Ok(())
}
