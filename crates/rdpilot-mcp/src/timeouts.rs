//! Per-verb-class `tokio::time::timeout` bounds (MCP-06's Layer 3
//! non-blocking-isolation mechanism, D-14 research §Non-Blocking Isolation
//! Design). Every daemon round trip in [`crate::connect::round_trip_bounded`]
//! is wrapped in exactly one of these bounds — no tool handler is ever
//! allowed to await a daemon round trip unbounded.
//!
//! Values mirror the worst-case-latency table in
//! `14-RESEARCH.md`'s §Non-Blocking Isolation Design, Layer 3. Treat these
//! as a documented starting point, not immutable constants — a future plan
//! may make them configurable.

use std::time::Duration;

/// Fast perception/input verbs: `Ping`, `Screenshot`, `Mouse`, `Key`,
/// `SetForeground`, `WindowList`, `ProcessList`, `Uia`, `WorldState`,
/// `DesktopSize`. Each round trip is a single in-memory/DVC query against an
/// already-live session — no network file transfer, no new RDP handshake.
// Declared now (this plan's transport spine, 14-02); consumed once
// Plans 14-03/14-04 wire `#[tool]` methods that pass these bounds to
// `connect::round_trip_bounded`.
#[allow(dead_code)]
pub const FAST: Duration = Duration::from_secs(15);

/// `Connect`: establishing a fresh RDP + TLS handshake against the remote
/// host (plus sensor bootstrap) dominates this bound — the IPC round trip
/// itself is negligible next to the handshake it waits on.
#[allow(dead_code)] // see `FAST`'s note above
pub const CONNECT: Duration = Duration::from_secs(120);

/// `List`/`Disconnect`: daemon-local session-registry bookkeeping only, no
/// remote round trip at all — bounded tightly since a hang here indicates
/// the daemon itself is wedged, not that remote work is in flight.
#[allow(dead_code)] // see `FAST`'s note above
pub const LIFECYCLE: Duration = Duration::from_secs(10);

/// `Put`, `Get`, `LaunchProcess`: file-transfer duration scales with file
/// size and remote disk/network speed with no SDK-side cap.
/// `LaunchProcess` itself is fire-and-forget (the daemon returns a `Pid`
/// immediately, per `dispatch.rs`, not a wait-for-exit), so in practice this
/// class exists mainly for `Put`/`Get` headroom — 300s is a conservative
/// starting bound, not hard physics.
#[allow(dead_code)] // see `FAST`'s note above
pub const TRANSFER: Duration = Duration::from_secs(300);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_verb_class_bounds_are_strictly_ordered_lifecycle_fast_connect_transfer() {
        // Not a hard requirement of MCP-06, but documents the intended
        // relative shape: lifecycle (daemon-local) is tightest, transfer
        // (unbounded remote I/O) is loosest.
        assert!(LIFECYCLE < FAST);
        assert!(FAST < CONNECT);
        assert!(CONNECT < TRANSFER);
    }
}
