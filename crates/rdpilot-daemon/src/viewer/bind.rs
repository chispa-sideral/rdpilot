//! Bind-set selection for the live viewer.
//!
//! The viewer binds `127.0.0.1`, plus at most one Tailscale IPv4 address.
//! `100.64.0.0/10` is also carrier-grade NAT space, so the range alone never
//! qualifies an address: automatic detection also requires an interface
//! whose name identifies Tailscale, and an explicit override must be present
//! on a local interface. No wildcard, LAN or public address is ever bound.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use rdpilot_ipc::WireViewerBind;
use tokio::net::TcpListener;

/// The only loopback address the viewer binds.
pub(crate) const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

/// Attempts to find one port free on both the tailnet and loopback address.
const PORT_PAIR_ATTEMPTS: usize = 5;

/// One local interface address.
#[derive(Debug, Clone)]
pub(crate) struct Interface {
    pub(crate) name: String,
    pub(crate) addr: IpAddr,
}

/// The outcome of tailnet address selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TailnetSelection {
    /// The bind set is loopback only.
    Disabled,
    /// Bind this tailnet address in addition to loopback.
    Selected(Ipv4Addr),
    /// No Tailscale interface address was found.
    NotFound,
    /// Several candidates; the operator must choose one.
    Ambiguous(Vec<Ipv4Addr>),
    /// The explicit override was refused.
    OverrideRejected(Ipv4Addr, &'static str),
}

impl TailnetSelection {
    pub(crate) fn address(&self) -> Option<Ipv4Addr> {
        match self {
            TailnetSelection::Selected(addr) => Some(*addr),
            _ => None,
        }
    }

    /// The notice to print when the viewer falls back to loopback only.
    pub(crate) fn notice(&self) -> Option<String> {
        match self {
            TailnetSelection::Disabled | TailnetSelection::Selected(_) => None,
            TailnetSelection::NotFound => {
                Some("tailnet address not found; serving on loopback only".to_owned())
            }
            TailnetSelection::Ambiguous(candidates) => Some(format!(
                "several tailnet candidates ({}); set viewer.tailnet_address or \
                 --tailnet-address; serving on loopback only",
                candidates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            TailnetSelection::OverrideRejected(addr, reason) => Some(format!(
                "tailnet address {addr} refused ({reason}); serving on loopback only"
            )),
        }
    }
}

/// `100.64.0.0/10` (Tailscale's IPv4 range, shared with carrier-grade NAT).
pub(crate) fn is_tailnet_range(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (b & 0xC0) == 64
}

/// Interface names Tailscale uses: `tailscale0` (Linux), `Tailscale`
/// (Windows), `utun*` (macOS).
fn is_tailscale_interface(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("tailscale") || lower.starts_with("utun")
}

/// Pure selection over a supplied interface list (platform-independent).
pub(crate) fn select_tailnet_address(
    interfaces: &[Interface],
    bind: WireViewerBind,
    override_addr: Option<Ipv4Addr>,
) -> TailnetSelection {
    if bind == WireViewerBind::Loopback {
        return TailnetSelection::Disabled;
    }
    if let Some(addr) = override_addr {
        if !is_tailnet_range(addr) {
            return TailnetSelection::OverrideRejected(addr, "not in 100.64.0.0/10");
        }
        if !interfaces.iter().any(|i| i.addr == IpAddr::V4(addr)) {
            return TailnetSelection::OverrideRejected(addr, "not present on a local interface");
        }
        return TailnetSelection::Selected(addr);
    }
    let mut candidates: Vec<Ipv4Addr> = interfaces
        .iter()
        .filter(|i| is_tailscale_interface(&i.name))
        .filter_map(|i| match i.addr {
            IpAddr::V4(v4) if is_tailnet_range(v4) => Some(v4),
            _ => None,
        })
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [] => TailnetSelection::NotFound,
        [one] => TailnetSelection::Selected(*one),
        _ => TailnetSelection::Ambiguous(candidates),
    }
}

/// The host's interface addresses (empty when enumeration fails).
pub(crate) fn local_interfaces() -> Vec<Interface> {
    if_addrs::get_if_addrs()
        .map(|list| {
            list.into_iter()
                .map(|i| Interface {
                    addr: i.ip(),
                    name: i.name,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Listeners bound for one viewer start.
pub(crate) struct Bound {
    pub(crate) listeners: Vec<TcpListener>,
    /// Bound socket addresses, loopback first.
    pub(crate) addrs: Vec<SocketAddr>,
    pub(crate) notices: Vec<String>,
}

/// Bind `127.0.0.1` and, when given, `tailnet` on the same ephemeral port.
/// If the tailnet bind fails for any reason, serve on loopback only and add
/// a notice. Only a loopback bind failure is an error.
pub(crate) async fn bind_listeners(tailnet: Option<Ipv4Addr>) -> io::Result<Bound> {
    let mut notices = Vec::new();
    if let Some(tailnet) = tailnet {
        if is_tailnet_range(tailnet) {
            for _ in 0..PORT_PAIR_ATTEMPTS {
                let tail = match TcpListener::bind((tailnet, 0)).await {
                    Ok(listener) => listener,
                    Err(e) => {
                        notices.push(format!(
                            "tailnet address {tailnet} could not be bound ({}); serving on \
                             loopback only",
                            e.kind()
                        ));
                        break;
                    }
                };
                let port = tail.local_addr()?.port();
                match TcpListener::bind((LOOPBACK, port)).await {
                    Ok(lo) => {
                        let addrs = vec![lo.local_addr()?, tail.local_addr()?];
                        return Ok(Bound {
                            listeners: vec![lo, tail],
                            addrs,
                            notices,
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::AddrInUse => {}
                    Err(e) => return Err(e),
                }
            }
            if notices.is_empty() {
                notices.push(format!(
                    "no port free on both 127.0.0.1 and {tailnet}; serving on loopback only"
                ));
            }
        } else {
            notices.push(format!(
                "{tailnet} is not a tailnet address; serving on loopback only"
            ));
        }
    }
    let lo = TcpListener::bind((LOOPBACK, 0)).await?;
    let addrs = vec![lo.local_addr()?];
    Ok(Bound {
        listeners: vec![lo],
        addrs,
        notices,
    })
}
