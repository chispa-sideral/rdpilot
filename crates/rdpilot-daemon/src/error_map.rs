//! `rdpilot::Error -> WireErrorCode` and `DaemonError -> WireError` mapping
//! — the Phase-11-deferred half of D-28 (rdpilot-ipc's error.rs doc
//! comment: "That mapping function lives in the Phase 12 daemon crate
//! instead, which is the only consumer that legitimately depends on both
//! `rdpilot` and `rdpilot-ipc`.").

use rdpilot_ipc::{WireError, WireErrorCode};

use crate::seams::DaemonError;

/// Map a `rdpilot::Error` onto the fixed D-28 wire error code set
/// (Decision 3 catch-all): `PathTraversal`/`ChecksumMismatch` map 1:1;
/// every other variant maps to `Internal`.
#[must_use]
pub fn wire_code_for_sdk_error(e: &rdpilot::Error) -> WireErrorCode {
    match e {
        rdpilot::Error::PathTraversal(_) => WireErrorCode::PathTraversal,
        rdpilot::Error::ChecksumMismatch { .. } => WireErrorCode::ChecksumMismatch,
        rdpilot::Error::SecureDesktopActive => WireErrorCode::SecureDesktopActive,
        rdpilot::Error::UacPromptNotActive => WireErrorCode::UacPromptNotActive,
        rdpilot::Error::UacResponseUnconfirmed(_) => WireErrorCode::UacResponseUnconfirmed,
        // Every other named `rdpilot::Error` variant maps to the Decision-3
        // `Internal` catch-all. Each is listed EXPLICITLY (not folded into
        // the trailing wildcard below) so this match documents, arm by
        // arm, that every currently-known variant was considered — the
        // trailing `_` arm exists ONLY to satisfy the compiler's
        // non-exhaustive-match rule for `rdpilot::Error`'s
        // `#[non_exhaustive]` attribute (matching a `#[non_exhaustive]`
        // enum from outside its defining crate REQUIRES a wildcard arm
        // even when every variant known today is listed — E0004 otherwise
        // — so a literal wildcard-free match, as the plan describes, is
        // not achievable against this enum's actual attribute; this is
        // the closest compiler-legal approximation of that forcing
        // function). Because every named variant is listed above the
        // wildcard, the wildcard arm is UNREACHABLE for any variant that
        // exists today — it only fires for a genuinely new variant added
        // upstream, at which point this file should gain an explicit arm
        // for it during that upstream change's review.
        rdpilot::Error::Connect(_)
        | rdpilot::Error::Tls(_)
        | rdpilot::Error::Decode(_)
        | rdpilot::Error::Encode(_)
        | rdpilot::Error::CropOutOfBounds { .. }
        | rdpilot::Error::Config(_)
        | rdpilot::Error::Session(_)
        | rdpilot::Error::CoordinateOutOfBounds { .. }
        | rdpilot::Error::Dvc(_)
        | rdpilot::Error::Bootstrap(_)
        | rdpilot::Error::SensorRejected(_) => WireErrorCode::Internal,
        _ => WireErrorCode::Internal,
    }
}

impl From<DaemonError> for WireError {
    fn from(err: DaemonError) -> Self {
        let message = err.to_string();
        let code = match &err {
            DaemonError::DuplicateSession(_) => WireErrorCode::DuplicateSession,
            DaemonError::SessionNotFound(_) | DaemonError::StillConnecting(_) => WireErrorCode::SessionNotFound,
            DaemonError::Sdk(e) => wire_code_for_sdk_error(e),
            DaemonError::Connect(_) => WireErrorCode::Internal,
            DaemonError::Io(_) => WireErrorCode::Internal,
            DaemonError::Config(_) => WireErrorCode::Internal,
            // No trailing wildcard: `DaemonError` is defined in THIS
            // crate, so — unlike `wire_code_for_sdk_error`'s match on the
            // externally-`#[non_exhaustive]` `rdpilot::Error` above — the
            // compiler does not force (and in fact rejects as
            // `unreachable_patterns`) a wildcard arm here once every
            // variant is listed. A future `DaemonError` variant added
            // without a matching arm here is therefore a genuine compile
            // error (T-12-05's forcing-function guarantee, fully realized
            // for this same-crate type).
        };
        WireError { code, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLANTED_SECRET: &str = "RDPILOT-PLANTED-SECRET-SENTINEL";

    #[test]
    fn path_traversal_maps_to_path_traversal_code() {
        let err = rdpilot::Error::PathTraversal("escapes share root".to_owned());
        assert_eq!(wire_code_for_sdk_error(&err), WireErrorCode::PathTraversal);
    }

    #[test]
    fn checksum_mismatch_maps_to_checksum_mismatch_code() {
        let err = rdpilot::Error::ChecksumMismatch {
            expected: "aaaa".to_owned(),
            actual: "bbbb".to_owned(),
        };
        assert_eq!(wire_code_for_sdk_error(&err), WireErrorCode::ChecksumMismatch);
    }

    #[test]
    fn every_other_sdk_error_variant_maps_to_internal() {
        let cases = [
            rdpilot::Error::Connect("x".to_owned()),
            rdpilot::Error::Tls("x".to_owned()),
            rdpilot::Error::Decode("x".to_owned()),
            rdpilot::Error::Encode("x".to_owned()),
            rdpilot::Error::CropOutOfBounds {
                rect_x: 0,
                rect_y: 0,
                rect_w: 1,
                rect_h: 1,
                image_w: 1,
                image_h: 1,
            },
            rdpilot::Error::Config("x".to_owned()),
            rdpilot::Error::Session("x".to_owned()),
            rdpilot::Error::CoordinateOutOfBounds {
                x: 0,
                y: 0,
                desktop_w: 1,
                desktop_h: 1,
            },
            rdpilot::Error::Dvc("x".to_owned()),
            rdpilot::Error::Bootstrap("x".to_owned()),
            rdpilot::Error::SensorRejected("x".to_owned()),
        ];
        for err in cases {
            assert_eq!(
                wire_code_for_sdk_error(&err),
                WireErrorCode::Internal,
                "expected Internal for {err}"
            );
        }
    }

    #[test]
    fn secure_desktop_active_maps_to_secure_desktop_active_code() {
        assert_eq!(wire_code_for_sdk_error(&rdpilot::Error::SecureDesktopActive), WireErrorCode::SecureDesktopActive);
    }

    #[test]
    fn uac_prompt_not_active_maps_to_uac_prompt_not_active_code() {
        assert_eq!(wire_code_for_sdk_error(&rdpilot::Error::UacPromptNotActive), WireErrorCode::UacPromptNotActive);
    }

    #[test]
    fn uac_response_unconfirmed_maps_to_uac_response_unconfirmed_code() {
        let err = rdpilot::Error::UacResponseUnconfirmed("still present".to_owned());
        assert_eq!(wire_code_for_sdk_error(&err), WireErrorCode::UacResponseUnconfirmed);
    }

    #[test]
    fn dvc_maps_to_internal() {
        // Explicit acceptance-criteria case: Dvc -> Internal.
        let err = rdpilot::Error::Dvc("channel closed".to_owned());
        assert_eq!(wire_code_for_sdk_error(&err), WireErrorCode::Internal);
    }

    #[test]
    fn daemon_error_duplicate_session_maps_to_duplicate_session_code() {
        let err = DaemonError::DuplicateSession("brave-otter".to_owned());
        let wire: WireError = err.into();
        assert_eq!(wire.code, WireErrorCode::DuplicateSession);
    }

    #[test]
    fn daemon_error_session_not_found_and_still_connecting_map_to_session_not_found_code() {
        let cases = [
            DaemonError::SessionNotFound("brave-otter".to_owned()),
            DaemonError::StillConnecting("brave-otter".to_owned()),
        ];
        for err in cases {
            let wire: WireError = err.into();
            assert_eq!(wire.code, WireErrorCode::SessionNotFound);
        }
    }

    #[test]
    fn daemon_error_io_maps_to_internal_code() {
        let err = DaemonError::Io("disk full".to_owned());
        let wire: WireError = err.into();
        assert_eq!(wire.code, WireErrorCode::Internal);
    }

    #[test]
    fn daemon_error_config_maps_to_internal_code() {
        let err = DaemonError::Config("bad toml".to_owned());
        let wire: WireError = err.into();
        assert_eq!(wire.code, WireErrorCode::Internal);
    }

    #[test]
    fn daemon_error_sdk_delegates_to_the_sdk_mapping() {
        let err = DaemonError::Sdk(rdpilot::Error::PathTraversal("x".to_owned()));
        let wire: WireError = err.into();
        assert_eq!(wire.code, WireErrorCode::PathTraversal);
    }

    /// No produced `WireError.message` contains a planted credential
    /// sentinel — every `DaemonError`/`rdpilot::Error` constructor above
    /// takes an id/host/reason string, never a credential field, so this
    /// is a structural regression guard (mirrors `rdpilot-ipc`'s CONFIG-03
    /// planted-secret test), not a live end-to-end trace.
    #[test]
    fn no_produced_wire_error_message_carries_the_planted_secret() {
        let cases: Vec<DaemonError> = vec![
            DaemonError::DuplicateSession("brave-otter".to_owned()),
            DaemonError::SessionNotFound("brave-otter".to_owned()),
            DaemonError::StillConnecting("brave-otter".to_owned()),
            DaemonError::Sdk(rdpilot::Error::Connect("host unreachable".to_owned())),
            DaemonError::Connect("host unreachable".to_owned()),
            DaemonError::Io("disk full".to_owned()),
            DaemonError::Config("bad toml".to_owned()),
        ];
        for err in cases {
            let wire: WireError = err.into();
            assert!(
                !wire.message.contains(PLANTED_SECRET),
                "leak found in WireError.message: {}",
                wire.message
            );
        }
    }
}
