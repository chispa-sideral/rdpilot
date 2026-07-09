//! Minimal no-op "rdpsnd" (audio) static-channel stub (05-04 live-gate bug
//! fix, D-5.6/SC2).
//!
//! Crate-internal only (D-09): never re-exported from `lib.rs`.
//!
//! Live-diagnosed root cause: `ironrdp-rdpdr`'s own doc comment on
//! [`ironrdp_rdpdr::Rdpdr`] states the RDPDR channel "must always be
//! advertised with the 'rdpsnd' channel in order for the server to send
//! anything back to it" (MS-RDPEFS Appendix A footnote <1>). Live trace
//! diagnostics (Plan 04 live gate) confirmed this exactly: the client
//! correctly requested and successfully joined the `rdpdr` static channel
//! (`ChannelJoinConfirm` received for channel_id 1004), but the server never
//! sent a `Server Announce Request` — the PDU that starts the RDPDR protocol
//! — because our connect path registered `rdpdr` and `drdynvc` but never
//! `rdpsnd`. Every `deploy_and_launch` attempt then correctly typed and ran
//! `cmd /c copy \\tsclient\RDPILOT\...`, which failed with "The network name
//! cannot be found" — `\\tsclient\RDPILOT` was never mounted because the
//! server-side RDPDR driver had nothing to announce it with.
//!
//! This stub only needs to be PRESENT and successfully joined as a static
//! channel — it does not need to implement real audio playback (out of
//! scope for this SDK) or even reply to the server's Server Audio Formats
//! PDU; channel presence alone is what termsrv's RDPDR-gate checks for.
//! [`RdpsndStub::process`] therefore drops every inbound payload
//! unconditionally (never panics, mirrors the "drop-never-panic" discipline
//! used throughout this crate, T-05-01) and sends nothing back.

use ironrdp_pdu::PduResult;
use ironrdp_pdu::gcc::ChannelName;
use ironrdp_svc::{CompressionCondition, SvcClientProcessor, SvcMessage, SvcProcessor, impl_as_any};

/// A static channel processor that only exists to satisfy the MS-RDPEFS
/// Appendix A<1> "rdpdr requires rdpsnd to be present" server-side gate. See
/// the module doc comment for the live-diagnosed root cause.
#[derive(Debug, Default)]
pub(crate) struct RdpsndStub;

impl_as_any!(RdpsndStub);

impl RdpsndStub {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl SvcProcessor for RdpsndStub {
    fn channel_name(&self) -> ChannelName {
        // "rdpsnd" + 2 NUL bytes, matching the 8-byte fixed-width channel
        // name convention every other static channel here uses (e.g.
        // `ironrdp_rdpdr::Rdpdr::NAME` = b"rdpdr\0\0\0").
        ChannelName::from_static(b"rdpsnd\0\0")
    }

    fn compression_condition(&self) -> CompressionCondition {
        CompressionCondition::Never
    }

    fn process(&mut self, _payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        // Deliberately a no-op: never parses, never replies, never panics
        // (T-05-01). Channel presence is the only thing this stub needs to
        // provide (see module doc comment).
        Ok(Vec::new())
    }
}

impl SvcClientProcessor for RdpsndStub {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_name_is_rdpsnd() {
        let stub = RdpsndStub::new();
        assert_eq!(stub.channel_name(), ChannelName::from_static(b"rdpsnd\0\0"));
    }

    #[test]
    fn process_ignores_arbitrary_bytes_without_panic() {
        let mut stub = RdpsndStub::new();
        let out = stub.process(&[0xFF, 0x00, 0x01, 0x02]).expect("never errors");
        assert!(out.is_empty(), "the stub never replies");

        // Empty input must not panic either.
        let out = stub.process(&[]).expect("never errors");
        assert!(out.is_empty());
    }
}
