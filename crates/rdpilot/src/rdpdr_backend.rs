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

use std::collections::HashMap;
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::PathBuf;

use ironrdp::core::impl_as_any;
use ironrdp::pdu::PduResult;
use ironrdp::svc::SvcMessage;
use ironrdp_rdpdr::backend::RdpdrBackend;
use ironrdp_rdpdr::pdu::RdpdrPdu;
use ironrdp_rdpdr::pdu::efs::{
    ClientDriveQueryDirectoryResponse, DeviceCloseRequest, DeviceCloseResponse, DeviceControlRequest,
    DeviceCreateRequest, DeviceCreateResponse, DeviceIoRequest, DeviceIoResponse, DeviceReadRequest,
    DeviceReadResponse, FileAttributes, FileBothDirectoryInformation, FileDirectoryInformation,
    FileFullDirectoryInformation, FileInformationClass, FileInformationClassLevel, FileNamesInformation, Information,
    NtStatus, ServerDeviceAnnounceResponse, ServerDriveIoRequest, ServerDriveQueryDirectoryRequest,
};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};

/// What a previously-granted RDPDR file id refers to: the served drive root
/// (a directory) or the one served file. `handle_read` only ever streams
/// bytes for [`OpenEntry::File`] -- a `Read` against a root/directory handle
/// (or an unknown handle) is rejected before any `std::fs` call (T-05-04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenEntry {
    Root,
    File,
}

/// A minimal, portable (`std::fs`-based) [`RdpdrBackend`] that serves
/// exactly ONE read-only file over MS-RDPEFS (05-02, SENSOR-02).
///
/// Constructed once per connection (Plan 03 wires this into the connect
/// path) with the local path of the file to serve and the name it should
/// appear under in the redirected drive (e.g. under
/// `\\tsclient\RDPILOT\<served_name>`).
#[derive(Debug)]
pub(crate) struct RdpilotDriveBackend {
    served_path: PathBuf,
    served_name: String,
    open_files: HashMap<u32, OpenEntry>,
    next_file_id: u32,
}

impl_as_any!(RdpilotDriveBackend);

impl RdpilotDriveBackend {
    /// Build a backend serving `served_path` under the name `served_name`.
    /// `served_path` is read lazily (on each `Read` IRP) -- the file does
    /// not need to exist yet at construction time.
    pub(crate) fn new(served_path: PathBuf, served_name: impl Into<String>) -> Self {
        Self {
            served_path,
            served_name: served_name.into(),
            open_files: HashMap::new(),
            next_file_id: 1,
        }
    }

    /// Strip the leading backslash(es) from an RDPDR wire path so the drive
    /// root (`""`/`"\"`) and a bare filename (`"\served.bin"`) both compare
    /// cleanly against [`RdpilotDriveBackend::served_name`].
    fn normalize(path: &str) -> &str {
        path.trim_start_matches('\\')
    }

    /// A fresh, never-zero file id for a newly granted `Create`.
    /// `wrapping_add` (not `+=`) so an exhausted counter never panics on
    /// overflow (API-01) -- purely defensive, this backend never opens
    /// anywhere close to `u32::MAX` handles in practice.
    fn allocate_file_id(&mut self) -> u32 {
        let id = self.next_file_id;
        self.next_file_id = self.next_file_id.wrapping_add(1);
        id
    }

    /// [`ServerDriveIoRequest::ServerCreateDriveRequest`]: the hard path
    /// allow-list (T-05-04) -- only the drive root or the one served
    /// filename resolves; every other server-supplied path is rejected with
    /// `NtStatus::NO_SUCH_FILE` (the not-found status this crate's `efs.rs`
    /// actually defines -- RESEARCH Open Question #1) and never reaches
    /// `std::fs`.
    fn handle_create(&mut self, req: DeviceCreateRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceCreateRequest {
            device_io_request, path, ..
        } = req;
        let normalized = Self::normalize(&path);

        let entry = if normalized.is_empty() {
            Some(OpenEntry::Root)
        } else if normalized.eq_ignore_ascii_case(&self.served_name) {
            Some(OpenEntry::File)
        } else {
            None
        };

        let Some(entry) = entry else {
            let response = DeviceCreateResponse {
                device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::NO_SUCH_FILE),
                file_id: 0,
                information: Information::FILE_SUPERSEDED,
            };
            return Ok(vec![SvcMessage::from(RdpdrPdu::DeviceCreateResponse(response))]);
        };

        let file_id = self.allocate_file_id();
        self.open_files.insert(file_id, entry);
        let response = DeviceCreateResponse {
            device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
            file_id,
            information: Information::FILE_OPENED,
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::DeviceCreateResponse(response))])
    }

    /// [`ServerDriveIoRequest::DeviceCloseRequest`]: drop the handle. Always
    /// succeeds, including for an already-closed/unknown file id (closing
    /// twice is not an error worth surfacing to the remote peer).
    fn handle_close(&mut self, req: DeviceCloseRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceCloseRequest { device_io_request } = req;
        self.open_files.remove(&device_io_request.file_id);
        let response = DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::DeviceCloseResponse(response))])
    }

    /// [`ServerDriveIoRequest::DeviceReadRequest`]: stream bytes from
    /// [`RdpilotDriveBackend::served_path`] via `std::fs` -- portable across
    /// the windows-gnu target and any Linux test host (NOT
    /// `ironrdp-rdpdr-native`). A `file_id` that was never granted a
    /// [`OpenEntry::File`] by `handle_create` (unknown id, or the root's
    /// directory handle) is rejected with `NtStatus::ACCESS_DENIED` *before*
    /// any `std::fs` call -- the access-control boundary this backend exists
    /// to enforce (T-05-04) is structural: no server-supplied string ever
    /// reaches `Read`, only a `file_id` this backend itself allocated after
    /// validating the `Create` path.
    fn handle_read(&self, req: DeviceReadRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceReadRequest {
            device_io_request,
            length,
            offset,
        } = req;

        if !matches!(self.open_files.get(&device_io_request.file_id), Some(OpenEntry::File)) {
            let response = DeviceReadResponse {
                device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::ACCESS_DENIED),
                read_data: Vec::new(),
            };
            return Ok(vec![SvcMessage::from(RdpdrPdu::DeviceReadResponse(response))]);
        }

        let (status, read_data) = match self.read_served_bytes(offset, length) {
            Ok(bytes) => (NtStatus::SUCCESS, bytes),
            // Any IO failure (e.g. the served file having disappeared since
            // Create) maps to a typed NtStatus response, never a panic
            // (API-01, T-05-05).
            Err(_io_error) => (NtStatus::UNSUCCESSFUL, Vec::new()),
        };

        let response = DeviceReadResponse {
            device_io_reply: DeviceIoResponse::new(device_io_request, status),
            read_data,
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::DeviceReadResponse(response))])
    }

    /// Read up to `length` bytes of [`RdpilotDriveBackend::served_path`]
    /// starting at `offset`. `Read::take` bounds the read to `length`
    /// without ever preallocating a `length`-sized buffer up front (a
    /// server-supplied `length` up to `u32::MAX` would otherwise be an
    /// unbounded-allocation DoS vector, T-05-05) -- short reads near EOF
    /// simply yield fewer bytes, never an error.
    fn read_served_bytes(&self, offset: u64, length: u32) -> std::io::Result<Vec<u8>> {
        let mut file = fs::File::open(&self.served_path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = Vec::new();
        file.take(u64::from(length)).read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// [`ServerDriveIoRequest::ServerDriveQueryDirectoryRequest`]: on the
    /// first query for a directory handle (nonzero `InitialQuery`, MS-RDPEFS
    /// 2.2.3.3.10), return the one served file's metadata (size/name) so the
    /// remote `copy`/Explorer enumeration sees it; every subsequent query on
    /// the same handle (`InitialQuery == 0`) reports `NO_MORE_FILES` with no
    /// buffer -- this backend never enumerates more than the single served
    /// entry.
    fn handle_query_directory(&self, req: ServerDriveQueryDirectoryRequest) -> PduResult<Vec<SvcMessage>> {
        let ServerDriveQueryDirectoryRequest {
            device_io_request,
            file_info_class_lvl,
            initial_query,
            ..
        } = req;

        if initial_query == 0 {
            let response = ClientDriveQueryDirectoryResponse {
                device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::NO_MORE_FILES),
                buffer: None,
            };
            return Ok(vec![SvcMessage::from(RdpdrPdu::ClientDriveQueryDirectoryResponse(
                response,
            ))]);
        }

        // A `stat` failure (served file not present yet/anymore) degrades to
        // a zero-size entry rather than a hard error -- Plan 03's launch
        // bootstrap copies the file before enumerating it, so this is a
        // defensive fallback, not the expected path.
        let file_size = fs::metadata(&self.served_path)
            .map(|meta| i64::try_from(meta.len()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let buffer = Some(Self::directory_entry(
            file_info_class_lvl,
            file_size,
            self.served_name.clone(),
        ));

        let response = ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
            buffer,
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::ClientDriveQueryDirectoryResponse(
            response,
        ))])
    }

    /// Build the single directory-listing entry in the shape the server
    /// asked for. `ServerDriveQueryDirectoryRequest::decode` (efs.rs, read at
    /// execution time) only ever admits these four
    /// [`FileInformationClassLevel`] values, so the wildcard arm is
    /// unreachable in practice but still needs a safe, non-panicking default
    /// (API-01).
    fn directory_entry(level: FileInformationClassLevel, file_size: i64, file_name: String) -> FileInformationClass {
        let attrs = FileAttributes::FILE_ATTRIBUTE_NORMAL;
        match level {
            FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION => FileInformationClass::BothDirectory(
                FileBothDirectoryInformation::new(0, 0, 0, 0, file_size, attrs, file_name),
            ),
            FileInformationClassLevel::FILE_FULL_DIRECTORY_INFORMATION => FileInformationClass::FullDirectory(
                FileFullDirectoryInformation::new(0, 0, 0, 0, file_size, attrs, file_name),
            ),
            FileInformationClassLevel::FILE_DIRECTORY_INFORMATION => FileInformationClass::Directory(
                FileDirectoryInformation::new(0, 0, 0, 0, file_size, attrs, file_name),
            ),
            // FILE_NAMES_INFORMATION, and the unreachable default above.
            _ => FileInformationClass::Names(FileNamesInformation::new(file_name)),
        }
    }

    /// Every MS-RDPEFS request this minimal backend does not implement
    /// (`efs.rs`'s `ServerDriveIoRequest` has 11 variants, not the plan's
    /// originally-assumed 4 -- RESEARCH Open Question #1 resolution) is
    /// completed with a typed `NOT_SUPPORTED` response, mirroring this
    /// crate's own `handle_printer_io_request` default (`DeviceCloseResponse`
    /// reused as a generic "reject" completion body) -- never a silently
    /// dropped IRP, never a panic (API-01).
    fn reject_unsupported(device_io_request: DeviceIoRequest) -> PduResult<Vec<SvcMessage>> {
        let response = DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(device_io_request, NtStatus::NOT_SUPPORTED),
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::DeviceCloseResponse(response))])
    }
}

impl RdpdrBackend for RdpilotDriveBackend {
    fn handle_server_device_announce_response(&mut self, _pdu: ServerDeviceAnnounceResponse) -> PduResult<()> {
        Ok(())
    }

    fn handle_scard_call(&mut self, _req: DeviceControlRequest<ScardIoCtlCode>, _call: ScardCall) -> PduResult<()> {
        Ok(())
    }

    /// Dispatch on the exactly-11-variant `ServerDriveIoRequest` enum
    /// (`efs.rs`, read at execution time): the four variants this backend
    /// implements (Create/Close/Read/QueryDirectory) plus the seven it
    /// rejects with a typed `NOT_SUPPORTED` completion.
    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        match req {
            ServerDriveIoRequest::ServerCreateDriveRequest(r) => self.handle_create(r),
            ServerDriveIoRequest::DeviceCloseRequest(r) => self.handle_close(r),
            ServerDriveIoRequest::DeviceReadRequest(r) => self.handle_read(r),
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) => self.handle_query_directory(r),
            ServerDriveIoRequest::ServerDriveQueryInformationRequest(r) => {
                Self::reject_unsupported(r.device_io_request)
            }
            ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(r) => {
                Self::reject_unsupported(r.device_io_request)
            }
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) => {
                Self::reject_unsupported(r.device_io_request)
            }
            ServerDriveIoRequest::DeviceControlRequest(r) => Self::reject_unsupported(r.header),
            ServerDriveIoRequest::DeviceWriteRequest(r) => Self::reject_unsupported(r.device_io_request),
            ServerDriveIoRequest::ServerDriveSetInformationRequest(r) => {
                Self::reject_unsupported(r.device_io_request)
            }
            ServerDriveIoRequest::ServerDriveLockControlRequest(r) => Self::reject_unsupported(r.device_io_request),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use ironrdp::core::ReadCursor;
    use ironrdp::svc::SvcMessage;
    use ironrdp_rdpdr::backend::RdpdrBackend as _;
    use ironrdp_rdpdr::pdu::efs::{
        CreateDisposition, CreateOptions, DesiredAccess, DeviceCloseRequest, DeviceCreateRequest, DeviceIoRequest,
        DeviceIoResponse, DeviceReadRequest, FileAttributes, FileInformationClassLevel, MajorFunction, MinorFunction,
        NtStatus, ServerDriveIoRequest, ServerDriveQueryDirectoryRequest, SharedAccess,
    };

    use super::RdpilotDriveBackend;

    /// Write `bytes` to a fresh temp file and return its path -- the local
    /// "served file" backing store for these tests. Every caller removes it
    /// again once done (no `tempfile` crate dependency needed for this
    /// narrow, offline-only use).
    fn write_temp_file(bytes: &[u8]) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!("rdpilot-rdpdr-test-{}-{nanos}", std::process::id()));
        let mut file = std::fs::File::create(&path).expect("temp file creates");
        file.write_all(bytes).expect("temp file writes");
        path
    }

    /// Decode the `DeviceIoResponse` every RDPDR completion PDU this backend
    /// sends leads with (device_id/completion_id/io_status), plus whatever
    /// raw bytes follow it -- the minimal shared decoding both
    /// `create_response_fields` and `read_response_fields` build on.
    ///
    /// `RdpdrPdu::encode` (pdu/mod.rs, read at execution time) writes the
    /// 4-byte `SharedHeader` (component/packet_id) *before* the inner PDU
    /// body, so the `DeviceIoResponse` prefix starts at byte offset 4, not 0.
    fn decode_io_status_and_tail(msg: &SvcMessage) -> (NtStatus, Vec<u8>) {
        let bytes = msg.encode_unframed_pdu().expect("message encodes");
        let mut cursor = ReadCursor::new(&bytes);
        let _shared_header = cursor.read_slice(4);
        let io_response = DeviceIoResponse::decode(&mut cursor).expect("decodes a DeviceIoResponse prefix");
        let tail = cursor.read_slice(cursor.len()).to_vec();
        (io_response.io_status, tail)
    }

    /// `(io_status, file_id)` from a `DeviceCreateResponse`-wrapped message.
    fn create_response_fields(msg: &SvcMessage) -> (NtStatus, u32) {
        let (status, tail) = decode_io_status_and_tail(msg);
        let mut cursor = ReadCursor::new(&tail);
        let file_id = cursor.read_u32();
        (status, file_id)
    }

    /// `(io_status, read_data)` from a `DeviceReadResponse`-wrapped message.
    fn read_response_fields(msg: &SvcMessage) -> (NtStatus, Vec<u8>) {
        let (status, tail) = decode_io_status_and_tail(msg);
        let mut cursor = ReadCursor::new(&tail);
        let length = cursor.read_u32() as usize;
        let data = cursor.read_slice(length).to_vec();
        (status, data)
    }

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

    /// The drive root and the one served filename both succeed; any other
    /// server-supplied path is rejected with `NtStatus::NO_SUCH_FILE` (the
    /// not-found status `efs.rs` actually defines) and grants no file id --
    /// the hard path allow-list IS the entire security posture of this
    /// backend (T-05-04).
    #[test]
    fn create_accepts_root_and_served_file_but_rejects_other_paths() {
        let served_path = write_temp_file(b"payload");
        let mut backend = RdpilotDriveBackend::new(served_path.clone(), "served.bin".to_owned());

        let root = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(1, "")))
            .expect("root create returns Ok");
        let (status, _file_id) = create_response_fields(&root[0]);
        assert_eq!(status, NtStatus::SUCCESS);

        let served = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(
                2,
                "\\served.bin",
            )))
            .expect("served-file create returns Ok");
        let (status, file_id) = create_response_fields(&served[0]);
        assert_eq!(status, NtStatus::SUCCESS);
        assert_ne!(file_id, 0);

        let other = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(
                3,
                "\\..\\..\\Windows\\System32\\config\\SAM",
            )))
            .expect("rejected create still returns Ok, never propagates an Err");
        let (status, file_id) = create_response_fields(&other[0]);
        assert_eq!(status, NtStatus::NO_SUCH_FILE);
        assert_eq!(file_id, 0);

        let _ = std::fs::remove_file(&served_path);
    }

    /// A `Read` against a `file_id` that was never granted `OpenEntry::File`
    /// (unknown/foreign id) is rejected with `NtStatus::ACCESS_DENIED`
    /// *before* any `std::fs` call. `served_path` deliberately points at a
    /// nonexistent file: if the rejection ever fell through to `std::fs`, it
    /// would surface as the distinct `NtStatus::UNSUCCESSFUL` IO-error status
    /// asserted against in `read_returns_exact_served_bytes_at_offset`, not
    /// `ACCESS_DENIED` -- the two different statuses are the proof that the
    /// access-control path and the IO-error path are structurally distinct
    /// (T-05-04).
    #[test]
    fn read_for_unopened_file_id_is_rejected_without_touching_filesystem() {
        let served_path = std::env::temp_dir().join("rdpilot-rdpdr-test-does-not-exist");
        let mut backend = RdpilotDriveBackend::new(served_path, "served.bin".to_owned());

        let read = ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
            device_io_request: dev_io_req(99, MajorFunction::Read),
            length: 4,
            offset: 0,
        });
        let out = backend
            .handle_drive_io_request(read)
            .expect("rejected read still returns Ok");
        let (status, data) = read_response_fields(&out[0]);
        assert_eq!(status, NtStatus::ACCESS_DENIED);
        assert!(data.is_empty());
    }

    /// A `Read` at a given offset/length returns exactly those bytes from
    /// the served file.
    #[test]
    fn read_returns_exact_served_bytes_at_offset() {
        let served_path = write_temp_file(b"0123456789");
        let mut backend = RdpilotDriveBackend::new(served_path.clone(), "served.bin".to_owned());

        let created = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(
                1,
                "\\served.bin",
            )))
            .expect("create returns Ok");
        let (status, file_id) = create_response_fields(&created[0]);
        assert_eq!(status, NtStatus::SUCCESS);

        let read = ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
            device_io_request: dev_io_req(file_id, MajorFunction::Read),
            length: 4,
            offset: 3,
        });
        let out = backend.handle_drive_io_request(read).expect("read returns Ok");
        let (status, data) = read_response_fields(&out[0]);
        assert_eq!(status, NtStatus::SUCCESS);
        assert_eq!(data, b"3456");

        let _ = std::fs::remove_file(&served_path);
    }
}
