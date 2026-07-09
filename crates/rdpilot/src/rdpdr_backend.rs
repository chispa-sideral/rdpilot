//! The RDPDR (drive-redirection) `RdpilotDriveBackend` (05-02, SENSOR-02).
//!
//! Crate-internal only (D-09): nothing here is re-exported from `lib.rs`, and
//! no `ironrdp-rdpdr` type ever leaks into the public API. `ironrdp-rdpdr`
//! ships ZERO Windows filesystem backend on the pinned
//! `x86_64-pc-windows-gnu` target (`ironrdp-rdpdr-native` is *nix-cfg-gated
//! and empty there, 05-RESEARCH Pitfall 2) -- this module supplies the SDK's
//! own `std::fs`-based backend instead.
//!
//! [`RdpilotDriveBackend`] serves exactly ONE read-only file (the copied
//! `rdpilot-sensor.exe`, wired up by Plan 03's launch bootstrap): the hard
//! path allow-list in `handle_create` -- only the drive root or the one
//! served filename resolves, any other server-supplied path is rejected with
//! a not-found `NtStatus` and never touches `std::fs` -- IS the entire
//! security posture of this backend (T-05-04, ASVS V4). No
//! `unwrap`/`expect`/`panic!` outside `#[cfg(test)]` (API-01): every
//! `std::fs`/IO failure maps to an `NtStatus` response, never a panic
//! (T-05-05).
//!
//! Grounded directly in the pinned `ironrdp-rdpdr-0.6.0` source
//! (`src/pdu/efs.rs`, `src/backend/mod.rs`) at execution time -- see the
//! plan's 05-02-SUMMARY.md "efs.rs read resolution" for the exact
//! `ServerDriveIoRequest` variant list and `NtStatus` values this module
//! relies on (RESEARCH Open Question #1).

#[cfg(test)]
mod tests {
    use ironrdp_rdpdr::pdu::efs::{
        CreateDisposition, CreateOptions, DesiredAccess, DeviceCloseRequest, DeviceCreateRequest, DeviceIoRequest,
        DeviceReadRequest, FileAttributes, FileInformationClassLevel, MajorFunction, MinorFunction,
        ServerDriveIoRequest, ServerDriveQueryDirectoryRequest, SharedAccess,
    };

    use super::RdpilotDriveBackend;

    fn dev_io_req(file_id: u32, major: MajorFunction) -> DeviceIoRequest {
        DeviceIoRequest {
            device_id: 1,
            file_id,
            completion_id: 42,
            major_function: major,
            minor_function: MinorFunction::from(0),
        }
    }

    fn create_req(file_id: u32, path: &str) -> DeviceCreateRequest {
        DeviceCreateRequest {
            device_io_request: dev_io_req(file_id, MajorFunction::Create),
            desired_access: DesiredAccess::empty(),
            allocation_size: 0,
            file_attributes: FileAttributes::empty(),
            shared_access: SharedAccess::empty(),
            create_disposition: CreateDisposition::FILE_OPEN,
            create_options: CreateOptions::empty(),
            path: path.to_owned(),
        }
    }

    /// Constructing each of the four `ServerDriveIoRequest` variants this
    /// backend implements and passing it to `handle_drive_io_request` never
    /// panics and always returns `Ok(_)` (mirrors Phase 4's
    /// `malformed_payload_is_dropped_without_panic` style).
    #[test]
    fn all_four_irp_variants_return_ok_and_never_panic() {
        let mut backend = RdpilotDriveBackend::new(
            std::env::temp_dir().join("rdpilot-rdpdr-red-placeholder"),
            "served.bin".to_owned(),
        );

        let create = ServerDriveIoRequest::ServerCreateDriveRequest(create_req(1, "\\served.bin"));
        assert!(backend.handle_drive_io_request(create).is_ok());

        let close = ServerDriveIoRequest::DeviceCloseRequest(DeviceCloseRequest {
            device_io_request: dev_io_req(1, MajorFunction::Close),
        });
        assert!(backend.handle_drive_io_request(close).is_ok());

        let read = ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
            device_io_request: dev_io_req(1, MajorFunction::Read),
            length: 5,
            offset: 0,
        });
        assert!(backend.handle_drive_io_request(read).is_ok());

        let query_dir = ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(ServerDriveQueryDirectoryRequest {
            device_io_request: dev_io_req(1, MajorFunction::DirectoryControl),
            file_info_class_lvl: FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION,
            initial_query: 1,
            path: "\\*".to_owned(),
        });
        assert!(backend.handle_drive_io_request(query_dir).is_ok());
    }
}
