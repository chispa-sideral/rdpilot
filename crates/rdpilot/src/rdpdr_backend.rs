//! The RDPDR (drive-redirection) `RdpilotDriveBackend` (05-02, SENSOR-02).
//!
//! Crate-internal only (D-09): nothing here is re-exported from `lib.rs`, and
//! no `ironrdp-rdpdr` type ever leaks into the public API. `ironrdp-rdpdr`
//! ships ZERO Windows filesystem backend on the pinned
//! `x86_64-pc-windows-gnu` target (`ironrdp-rdpdr-native` is *nix-cfg-gated
//! and empty there, 05-RESEARCH Pitfall 2) -- this module supplies the SDK's
//! own `std::fs`-based backend instead.
//!
//! [`RdpilotDriveBackend`] serves the sensor exe read-only (the copied
//! `rdpilot-sensor.exe`, wired up by Plan 03's launch bootstrap) and, when a
//! share root is configured (D-10.1, 10-01), every path under it that
//! resolves via [`RdpilotDriveBackend::resolve_under_root`] -- a
//! canonicalize and component-wise ancestry check (D-10.2), never substring
//! matching, run against every RDPDR-supplied path other than the fixed
//! sensor exe name. Any path that fails to resolve is rejected with a
//! not-found `NtStatus` and never touches `std::fs` -- this validator IS the
//! entire security posture of this backend (T-05-04/T-10-01, ASVS V4). No
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
use std::path::{Path, PathBuf};

use ironrdp::core::impl_as_any;
use ironrdp::pdu::PduResult;
use ironrdp::svc::SvcMessage;
use ironrdp_rdpdr::backend::RdpdrBackend;
use ironrdp_rdpdr::pdu::RdpdrPdu;
use ironrdp_rdpdr::pdu::efs::{
    Boolean, Characteristics, ClientDriveQueryDirectoryResponse, ClientDriveQueryInformationResponse,
    ClientDriveQueryVolumeInformationResponse, DeviceCloseRequest, DeviceCloseResponse, DeviceControlRequest,
    DeviceCreateRequest, DeviceCreateResponse, DeviceIoRequest, DeviceIoResponse, DeviceReadRequest,
    DeviceReadResponse, FileAttributeTagInformation, FileAttributes, FileBasicInformation,
    FileBothDirectoryInformation, FileDirectoryInformation, FileFsAttributeInformation, FileFsDeviceInformation,
    FileFsFullSizeInformation, FileFsSizeInformation, FileFsVolumeInformation, FileFullDirectoryInformation,
    FileInformationClass, FileInformationClassLevel, FileNamesInformation, FileStandardInformation,
    FileSystemAttributes, FileSystemInformationClass, FileSystemInformationClassLevel, Information, NtStatus,
    ServerDeviceAnnounceResponse, ServerDriveIoRequest, ServerDriveQueryDirectoryRequest,
    ServerDriveQueryInformationRequest, ServerDriveQueryVolumeInformationRequest,
};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};

/// What a previously-granted RDPDR file id refers to: the drive root (a
/// directory) or a resolved, absolute file path -- either the sensor exe
/// (the pre-Phase-10 read-only special case) or a file under the configured
/// share root, already validated by [`RdpilotDriveBackend::resolve_under_root`]
/// at `Create` time (D-10.1/D-10.2). `handle_read` only ever streams bytes
/// for [`OpenEntry::File`] -- a `Read` against a root/directory handle (or
/// an unknown handle) is rejected before any `std::fs` call (T-05-04).
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenEntry {
    Root,
    File(PathBuf),
}

/// A `std::fs`-based [`RdpdrBackend`] that serves the sensor exe read-only
/// (the pre-Phase-10 bootstrap special case) AND, when a share root is
/// configured, an allow-listed share root generalized for bidirectional file
/// transfer (05-02/10-01, SENSOR-02, D-10.1).
///
/// Constructed once per connection (`connect.rs`) with the local path of the
/// sensor exe, the name it should appear under in the redirected drive
/// (e.g. under `\\tsclient\RDPILOT\<sensor_name>`), and an optional share
/// root. When `share_root` is `None`, every non-sensor-exe path is rejected
/// -- the connect path is byte-for-byte the pre-Phase-10 behavior.
#[derive(Debug)]
pub(crate) struct RdpilotDriveBackend {
    sensor_path: PathBuf,
    sensor_name: String,
    /// The configured share root, if any (D-10.1). Canonicalized fresh on
    /// every [`RdpilotDriveBackend::resolve_under_root`] call rather than
    /// cached at construction time -- cheap for a per-IRP validator, and
    /// avoids `new()` needing to return a `Result` for a directory that may
    /// not exist yet at construction (the caller, `connect.rs`, pre-creates
    /// it before calling `new`, but this backend does not assume that).
    share_root: Option<PathBuf>,
    open_files: HashMap<u32, OpenEntry>,
    next_file_id: u32,
}

impl_as_any!(RdpilotDriveBackend);

impl RdpilotDriveBackend {
    /// Build a backend serving `sensor_path` read-only under `sensor_name`,
    /// plus (when `share_root` is `Some`) every path under that root that
    /// [`RdpilotDriveBackend::resolve_under_root`] accepts. `sensor_path` is
    /// read lazily (on each `Read` IRP) -- the file does not need to exist
    /// yet at construction time.
    pub(crate) fn new(sensor_path: PathBuf, sensor_name: impl Into<String>, share_root: Option<PathBuf>) -> Self {
        Self {
            sensor_path,
            sensor_name: sensor_name.into(),
            share_root,
            open_files: HashMap::new(),
            next_file_id: 1,
        }
    }

    /// Strip the leading backslash(es) from an RDPDR wire path so the drive
    /// root (`""`/`"\"`) and a bare filename (`"\sensor.exe"`) both compare
    /// cleanly against [`RdpilotDriveBackend::sensor_name`].
    fn normalize(path: &str) -> &str {
        path.trim_start_matches('\\')
    }

    /// `true` if `normalized` would be treated as an absolute/rooted path on
    /// EITHER Unix (leading `/`) or Windows (a drive-letter prefix, e.g.
    /// `C:`) -- checked as our OWN host-independent string test, never via
    /// `Path::is_absolute()`/`Path::join`'s absolute-replace semantics, which
    /// are `cfg(windows)`-conditional and would NOT treat `"C:/Windows"` as
    /// rooted on this offline Linux test host (a second, drive-letter-shaped
    /// gap alongside 10-RESEARCH Pitfall 1's separator issue -- deliberately
    /// closed here so the offline FILE-03 suite is a faithful proxy for the
    /// real Windows target on this specific adversarial case too).
    fn looks_rooted(normalized: &str) -> bool {
        if normalized.starts_with('/') {
            return true;
        }
        let bytes = normalized.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    }

    /// Resolve an untrusted RDPDR-supplied relative path under the
    /// configured share root, canonicalizing and ancestry-checking it before
    /// any further `std::fs` call reaches it (D-10.2, FILE-03 BLOCKING).
    ///
    /// Returns `Err(())` for: no share root configured; a rooted input
    /// (leading `/` or a drive-letter prefix, [`Self::looks_rooted`]); a
    /// parent directory that does not exist/cannot be canonicalized under
    /// the root (10-RESEARCH Pitfall 2 -- `std::fs::canonicalize` requires
    /// existence, so the PARENT is canonicalized, never the full untrusted
    /// leaf, which may not exist yet for a brand-new upload); or a
    /// canonicalized parent that escapes the root (`Path::starts_with` is
    /// component-aware, so this is immune to the C# `string.StartsWith`
    /// sibling-directory prefix bug, 10-RESEARCH Pitfall 3, and is proven as
    /// a Rust-side regression guard in Task 3's adversarial suite).
    ///
    /// `untrusted` MUST already have any leading RDPDR backslash stripped
    /// (by [`Self::normalize`]) before being passed here; every remaining
    /// `\` is normalized to `/` UNCONDITIONALLY, regardless of host OS,
    /// BEFORE any `Path`/`PathBuf` construction (10-RESEARCH Pitfall 1) --
    /// this is what makes a `\`-bearing adversarial test case faithful on
    /// this Linux host, which does not otherwise treat `\` as a separator.
    fn resolve_under_root(&self, untrusted: &str) -> std::result::Result<PathBuf, ()> {
        let root = self.share_root.as_ref().ok_or(())?;
        let root_canonical = fs::canonicalize(root).map_err(|_| ())?;

        let normalized = untrusted.replace('\\', "/");
        if Self::looks_rooted(&normalized) {
            return Err(());
        }

        let candidate = root_canonical.join(&normalized);
        // The parent, not the (possibly not-yet-existing) full candidate, is
        // what gets canonicalized (Pitfall 2). A candidate whose last
        // component is `..` (the trailing-`..`-no-separator adversarial
        // case, CVE-2025-48817's off-by-one class) has NO `file_name()` --
        // caught by the check below, not silently swallowed by `parent()`'s
        // purely-syntactic last-component strip.
        let parent = candidate.parent().ok_or(())?;
        let parent_canonical = fs::canonicalize(parent).map_err(|_| ())?;
        if !parent_canonical.starts_with(&root_canonical) {
            return Err(());
        }
        let leaf = candidate.file_name().ok_or(())?;
        Ok(parent_canonical.join(leaf))
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

    /// [`ServerDriveIoRequest::ServerCreateDriveRequest`]: the drive root
    /// always resolves; the sensor exe name resolves to its fixed read-only
    /// path (the pre-Phase-10 bootstrap special case, unchanged); every
    /// other name is routed through [`Self::resolve_under_root`] (D-10.2) --
    /// an escaping/unresolvable path is rejected with `NtStatus::NO_SUCH_FILE`
    /// (the not-found status this crate's `efs.rs` actually defines --
    /// RESEARCH Open Question #1) and never reaches `std::fs` (T-05-04).
    fn handle_create(&mut self, req: DeviceCreateRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceCreateRequest {
            device_io_request, path, ..
        } = req;
        let normalized = Self::normalize(&path);

        let entry = if normalized.is_empty() {
            Some(OpenEntry::Root)
        } else if normalized.eq_ignore_ascii_case(&self.sensor_name) {
            Some(OpenEntry::File(self.sensor_path.clone()))
        } else {
            self.resolve_under_root(normalized).ok().map(OpenEntry::File)
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

    /// [`ServerDriveIoRequest::DeviceReadRequest`]: stream bytes from the
    /// per-handle resolved [`OpenEntry::File`] path via `std::fs` -- portable
    /// across the windows-gnu target and any Linux test host (NOT
    /// `ironrdp-rdpdr-native`). A `file_id` that was never granted an
    /// [`OpenEntry::File`] by `handle_create` (unknown id, or the root's
    /// directory handle) is rejected with `NtStatus::ACCESS_DENIED` *before*
    /// any `std::fs` call -- the access-control boundary this backend exists
    /// to enforce (T-05-04) is structural: no server-supplied string ever
    /// reaches `Read`, only a `file_id` this backend itself allocated after
    /// validating (and, for share-root paths, canonicalizing/ancestry-
    /// checking, D-10.2) the `Create` path.
    fn handle_read(&self, req: DeviceReadRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceReadRequest {
            device_io_request,
            length,
            offset,
        } = req;

        let Some(OpenEntry::File(path)) = self.open_files.get(&device_io_request.file_id) else {
            let response = DeviceReadResponse {
                device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::ACCESS_DENIED),
                read_data: Vec::new(),
            };
            return Ok(vec![SvcMessage::from(RdpdrPdu::DeviceReadResponse(response))]);
        };

        let (status, read_data) = match Self::read_bytes_at(path, offset, length) {
            Ok(bytes) => (NtStatus::SUCCESS, bytes),
            // Any IO failure (e.g. the file having disappeared since
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

    /// Read up to `length` bytes of `path` starting at `offset`.
    /// `Read::take` bounds the read to `length` without ever preallocating a
    /// `length`-sized buffer up front (a server-supplied `length` up to
    /// `u32::MAX` would otherwise be an unbounded-allocation DoS vector,
    /// T-05-05) -- short reads near EOF simply yield fewer bytes, never an
    /// error.
    fn read_bytes_at(path: &Path, offset: u64, length: u32) -> std::io::Result<Vec<u8>> {
        let mut file = fs::File::open(path)?;
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

        // The one entry this backend lists for ANY directory handle:
        // the per-handle resolved `OpenEntry::File` path if the queried
        // handle is one (share-root file or sensor exe), else the sensor
        // exe entry (preserves pre-Phase-10 behavior byte-for-byte when the
        // handle is the drive root or unrecognized -- true share-root
        // directory enumeration is out of this plan's scope). A `stat`
        // failure (file not present yet/anymore) degrades to a zero-size
        // entry rather than a hard error -- Plan 03's launch bootstrap
        // copies the sensor exe before enumerating it, so this is a
        // defensive fallback, not the expected path.
        let (entry_path, entry_name) = match self.open_files.get(&device_io_request.file_id) {
            Some(OpenEntry::File(path)) => (path.clone(), Self::display_name(path, &self.sensor_name)),
            _ => (self.sensor_path.clone(), self.sensor_name.clone()),
        };
        let file_size = fs::metadata(&entry_path)
            .map(|meta| i64::try_from(meta.len()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let buffer = Some(Self::directory_entry(file_info_class_lvl, file_size, entry_name));

        let response = ClientDriveQueryDirectoryResponse {
            device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
            buffer,
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::ClientDriveQueryDirectoryResponse(
            response,
        ))])
    }

    /// [`ServerDriveIoRequest::ServerDriveQueryInformationRequest`]: part of
    /// Windows' STANDARD `Create`-then-`QueryInformation` sequence when
    /// opening ANY path (including the drive root) -- live-diagnosed as a
    /// hard requirement, not an optional extra (Plan 04 live gate,
    /// D-5.6/SC2 blocking bug): a real Windows RDP client always issues this
    /// immediately after a successful `Create`, and rejecting it with
    /// `NOT_SUPPORTED` (this backend's prior behavior, inherited from the
    /// generic 7-variant reject list) makes the ENTIRE redirected drive
    /// unusable -- Explorer/`cmd`'s `dir`/`copy` all fail with "The device
    /// is not connected" the moment they try to stat the root they just
    /// opened. A known/open `file_id` (root, the sensor exe, or a resolved
    /// share-root file) always succeeds; an unknown id is rejected with
    /// `NtStatus::ACCESS_DENIED` (mirrors `handle_read`'s access-control
    /// discipline, T-05-04) with no buffer, per this response's own doc
    /// comment ("if io_status has an io_status besides SUCCESS, buffer can
    /// be omitted").
    fn handle_query_information(&self, req: ServerDriveQueryInformationRequest) -> PduResult<Vec<SvcMessage>> {
        let ServerDriveQueryInformationRequest {
            device_io_request,
            file_info_class_lvl,
        } = req;

        let Some(entry) = self.open_files.get(&device_io_request.file_id).cloned() else {
            let response = ClientDriveQueryInformationResponse {
                device_io_response: DeviceIoResponse::new(device_io_request, NtStatus::ACCESS_DENIED),
                buffer: None,
            };
            return Ok(vec![SvcMessage::from(RdpdrPdu::ClientDriveQueryInformationResponse(
                response,
            ))]);
        };

        let is_dir = entry == OpenEntry::Root;
        // A `stat` failure degrades to a zero-size entry rather than a hard
        // error -- mirrors `handle_query_directory`'s defensive fallback
        // (the file may not exist yet/anymore); only meaningful for
        // `OpenEntry::File`, the root has no backing `std::fs` metadata.
        let file_size = match &entry {
            OpenEntry::Root => 0,
            OpenEntry::File(path) => fs::metadata(path)
                .map(|meta| i64::try_from(meta.len()).unwrap_or(i64::MAX))
                .unwrap_or(0),
        };
        let attrs = if is_dir {
            FileAttributes::FILE_ATTRIBUTE_DIRECTORY
        } else {
            FileAttributes::FILE_ATTRIBUTE_NORMAL
        };

        let buffer = match file_info_class_lvl {
            FileInformationClassLevel::FILE_BASIC_INFORMATION => {
                Some(FileInformationClass::Basic(FileBasicInformation {
                    creation_time: 0,
                    last_access_time: 0,
                    last_write_time: 0,
                    change_time: 0,
                    file_attributes: attrs,
                }))
            }
            FileInformationClassLevel::FILE_STANDARD_INFORMATION => {
                Some(FileInformationClass::Standard(FileStandardInformation {
                    allocation_size: file_size,
                    end_of_file: file_size,
                    number_of_links: 1,
                    delete_pending: Boolean::False,
                    directory: if is_dir { Boolean::True } else { Boolean::False },
                }))
            }
            FileInformationClassLevel::FILE_ATTRIBUTE_TAG_INFORMATION => {
                Some(FileInformationClass::AttributeTag(FileAttributeTagInformation {
                    file_attributes: attrs,
                    reparse_tag: 0,
                }))
            }
            // Any other level this minimal backend does not model: succeed
            // with no buffer rather than fail the whole Create/stat sequence
            // (T-05-01 -- never turn an unrecognized-but-benign request into
            // a hard client-visible error).
            _ => None,
        };

        let response = ClientDriveQueryInformationResponse {
            device_io_response: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
            buffer,
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::ClientDriveQueryInformationResponse(
            response,
        ))])
    }

    /// [`ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest`]:
    /// part of Windows' STANDARD directory-listing sequence (`dir`/Explorer
    /// query the volume label/size/filesystem type before or alongside
    /// enumerating files) -- live-diagnosed as a second hard requirement
    /// alongside `QueryInformation` (Plan 04 live gate, D-5.6/SC2 blocking
    /// bug, found immediately after fixing the first one: `dir` progressed
    /// past "The device is not connected" to "The request could not be
    /// performed because of an I/O device error", the generic client-side
    /// surfacing of this backend's `NOT_SUPPORTED` reply). Always succeeds
    /// with small, plausible fixed values (this backend serves exactly one
    /// read-only file -- there is no real volume to report on); an unknown
    /// `file_id` is rejected the same way `handle_query_information` is.
    fn handle_query_volume_information(
        &self,
        req: ServerDriveQueryVolumeInformationRequest,
    ) -> PduResult<Vec<SvcMessage>> {
        let ServerDriveQueryVolumeInformationRequest {
            device_io_request,
            fs_info_class_lvl,
        } = req;

        if self.open_files.get(&device_io_request.file_id).is_none() {
            let response = ClientDriveQueryVolumeInformationResponse {
                device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::ACCESS_DENIED),
                buffer: None,
            };
            return Ok(vec![SvcMessage::from(
                RdpdrPdu::ClientDriveQueryVolumeInformationResponse(response),
            )]);
        }

        // Fixed, plausible-but-arbitrary volume metadata -- this backend
        // serves one read-only file, not a real filesystem, so there is no
        // meaningful free-space/serial-number to report. 512-byte sectors, 1
        // sector per allocation unit: the smallest self-consistent, nonzero
        // values a Windows client would accept without misbehaving.
        let buffer = match fs_info_class_lvl {
            FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION => {
                Some(FileSystemInformationClass::FileFsVolumeInformation(FileFsVolumeInformation {
                    volume_creation_time: 0,
                    volume_serial_number: 0x1234_5678,
                    supports_objects: Boolean::False,
                    volume_label: "RDPILOT".to_owned(),
                }))
            }
            FileSystemInformationClassLevel::FILE_FS_SIZE_INFORMATION => {
                Some(FileSystemInformationClass::FileFsSizeInformation(FileFsSizeInformation {
                    total_alloc_units: 1,
                    available_alloc_units: 0,
                    sectors_per_alloc_unit: 1,
                    bytes_per_sector: 512,
                }))
            }
            FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION => {
                Some(FileSystemInformationClass::FileFsAttributeInformation(FileFsAttributeInformation {
                    file_system_attributes: FileSystemAttributes::FILE_CASE_SENSITIVE_SEARCH,
                    max_component_name_len: 255,
                    file_system_name: "RDPILOTFS".to_owned(),
                }))
            }
            FileSystemInformationClassLevel::FILE_FS_FULL_SIZE_INFORMATION => {
                Some(FileSystemInformationClass::FileFsFullSizeInformation(FileFsFullSizeInformation {
                    total_alloc_units: 1,
                    caller_available_alloc_units: 0,
                    actual_available_alloc_units: 0,
                    sectors_per_alloc_unit: 1,
                    bytes_per_sector: 512,
                }))
            }
            FileSystemInformationClassLevel::FILE_FS_DEVICE_INFORMATION => {
                Some(FileSystemInformationClass::FileFsDeviceInformation(FileFsDeviceInformation {
                    // FILE_DEVICE_DISK (0x00000007) -- the standard Windows
                    // DDK device-type constant for a disk-like volume.
                    device_type: 0x0000_0007,
                    characteristics: Characteristics::FILE_REMOTE_DEVICE,
                }))
            }
            // `decode` (efs.rs) already rejects any level besides the 5
            // handled above at the wire-parsing stage, so this arm is
            // unreachable in practice -- still a safe, non-panicking default
            // (API-01) rather than an unwrap/expect.
            _ => None,
        };

        let response = ClientDriveQueryVolumeInformationResponse {
            device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
            buffer,
        };
        Ok(vec![SvcMessage::from(
            RdpdrPdu::ClientDriveQueryVolumeInformationResponse(response),
        )])
    }

    /// The display filename for a resolved [`OpenEntry::File`] path: its
    /// final path component if it has one (share-root files), else
    /// `fallback` (the sensor exe, whose display name is the fixed
    /// `sensor_name`, not derived from its local on-disk path).
    fn display_name(path: &Path, fallback: &str) -> String {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| fallback.to_owned())
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
            ServerDriveIoRequest::ServerDriveQueryInformationRequest(r) => self.handle_query_information(r),
            ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(r) => {
                Self::reject_unsupported(r.device_io_request)
            }
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) => {
                self.handle_query_volume_information(r)
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
        DeviceIoResponse, DeviceReadRequest, FileAttributes, FileInformationClassLevel, FileSystemInformationClassLevel,
        MajorFunction, MinorFunction, NtStatus, ServerDriveIoRequest, ServerDriveQueryDirectoryRequest,
        ServerDriveQueryInformationRequest, ServerDriveQueryVolumeInformationRequest, SharedAccess,
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
            None,
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
        let mut backend = RdpilotDriveBackend::new(served_path.clone(), "served.bin".to_owned(), None);

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
        let mut backend = RdpilotDriveBackend::new(served_path, "served.bin".to_owned(), None);

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

    /// `QueryInformation` on a known `file_id` (root or the served file)
    /// always succeeds -- the live-diagnosed bug fix (Plan 04 live gate,
    /// D-5.6/SC2): without this, Windows' standard Create-then-
    /// QueryInformation sequence fails and the entire redirected drive is
    /// unusable ("The device is not connected").
    #[test]
    fn query_information_succeeds_for_known_file_ids_and_rejects_unknown() {
        let served_path = write_temp_file(b"0123456789");
        let mut backend = RdpilotDriveBackend::new(served_path.clone(), "served.bin".to_owned(), None);

        let root_created = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(1, "")))
            .expect("root create returns Ok");
        let (status, root_file_id) = create_response_fields(&root_created[0]);
        assert_eq!(status, NtStatus::SUCCESS);

        let file_created = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(
                2,
                r"\served.bin",
            )))
            .expect("served-file create returns Ok");
        let (status, file_file_id) = create_response_fields(&file_created[0]);
        assert_eq!(status, NtStatus::SUCCESS);

        for level in [
            FileInformationClassLevel::FILE_BASIC_INFORMATION,
            FileInformationClassLevel::FILE_STANDARD_INFORMATION,
            FileInformationClassLevel::FILE_ATTRIBUTE_TAG_INFORMATION,
        ] {
            let query_root = ServerDriveIoRequest::ServerDriveQueryInformationRequest(
                ServerDriveQueryInformationRequest {
                    device_io_request: dev_io_req(root_file_id, MajorFunction::QueryInformation),
                    file_info_class_lvl: level.clone(),
                },
            );
            let out = backend
                .handle_drive_io_request(query_root)
                .expect("query on root returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&out[0]);
            assert_eq!(status, NtStatus::SUCCESS, "root QueryInformation({level}) must succeed");

            let query_file = ServerDriveIoRequest::ServerDriveQueryInformationRequest(
                ServerDriveQueryInformationRequest {
                    device_io_request: dev_io_req(file_file_id, MajorFunction::QueryInformation),
                    file_info_class_lvl: level.clone(),
                },
            );
            let out = backend
                .handle_drive_io_request(query_file)
                .expect("query on served file returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&out[0]);
            assert_eq!(status, NtStatus::SUCCESS, "file QueryInformation({level}) must succeed");
        }

        let query_unknown = ServerDriveIoRequest::ServerDriveQueryInformationRequest(
            ServerDriveQueryInformationRequest {
                device_io_request: dev_io_req(9999, MajorFunction::QueryInformation),
                file_info_class_lvl: FileInformationClassLevel::FILE_BASIC_INFORMATION,
            },
        );
        let out = backend
            .handle_drive_io_request(query_unknown)
            .expect("rejected query still returns Ok, never propagates an Err");
        let (status, _tail) = decode_io_status_and_tail(&out[0]);
        assert_eq!(status, NtStatus::ACCESS_DENIED);

        let _ = std::fs::remove_file(&served_path);
    }

    /// `QueryVolumeInformation` on a known `file_id` always succeeds for
    /// every `FileSystemInformationClassLevel` this backend models -- the
    /// second live-diagnosed bug fix (Plan 04 live gate, D-5.6/SC2), found
    /// immediately after `QueryInformation`: Windows' `dir`/Explorer
    /// sequence queries volume metadata too, and rejecting it with
    /// `NOT_SUPPORTED` surfaced as a generic "I/O device error" on the
    /// client.
    #[test]
    fn query_volume_information_succeeds_for_known_file_id_and_rejects_unknown() {
        let served_path = write_temp_file(b"0123456789");
        let mut backend = RdpilotDriveBackend::new(served_path.clone(), "served.bin".to_owned(), None);

        let root_created = backend
            .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(1, "")))
            .expect("root create returns Ok");
        let (status, root_file_id) = create_response_fields(&root_created[0]);
        assert_eq!(status, NtStatus::SUCCESS);

        for level in [
            FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION,
            FileSystemInformationClassLevel::FILE_FS_SIZE_INFORMATION,
            FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION,
            FileSystemInformationClassLevel::FILE_FS_FULL_SIZE_INFORMATION,
            FileSystemInformationClassLevel::FILE_FS_DEVICE_INFORMATION,
        ] {
            let query = ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(
                ServerDriveQueryVolumeInformationRequest {
                    device_io_request: dev_io_req(root_file_id, MajorFunction::QueryVolumeInformation),
                    fs_info_class_lvl: level.clone(),
                },
            );
            let out = backend
                .handle_drive_io_request(query)
                .expect("query volume info returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&out[0]);
            assert_eq!(status, NtStatus::SUCCESS, "QueryVolumeInformation({level:?}) must succeed");
        }

        let query_unknown = ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(
            ServerDriveQueryVolumeInformationRequest {
                device_io_request: dev_io_req(9999, MajorFunction::QueryVolumeInformation),
                fs_info_class_lvl: FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION,
            },
        );
        let out = backend
            .handle_drive_io_request(query_unknown)
            .expect("rejected query still returns Ok, never propagates an Err");
        let (status, _tail) = decode_io_status_and_tail(&out[0]);
        assert_eq!(status, NtStatus::ACCESS_DENIED);

        let _ = std::fs::remove_file(&served_path);
    }

    /// A `Read` at a given offset/length returns exactly those bytes from
    /// the served file.
    #[test]
    fn read_returns_exact_served_bytes_at_offset() {
        let served_path = write_temp_file(b"0123456789");
        let mut backend = RdpilotDriveBackend::new(served_path.clone(), "served.bin".to_owned(), None);

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

    /// FILE-03 BLOCKING adversarial path-traversal suite (10-01 Task 3,
    /// D-10.2). Drives `handle_drive_io_request(ServerCreateDriveRequest)`
    /// against a real temp share root for the exact required matrix --
    /// trailing `..` with no separator, mixed `/`+`\` separators,
    /// absolute-path-as-relative, and the sibling-directory-prefix
    /// regression guard (10-RESEARCH Pitfall 3, mirrored on the Rust side as
    /// a regression guard even though `Path::starts_with` is not vulnerable
    /// to it) -- plus one positive control proving the suite rejects
    /// attacks WITHOUT over-rejecting a legitimate nested path (Pitfall 2
    /// warning sign).
    mod path_traversal {
        use super::*;

        /// A fresh temp share root for one test: `<base>/share` is the
        /// configured root, pre-seeded with a `subdir` directory (so the
        /// trailing-`..`-no-separator case's PARENT genuinely canonicalizes
        /// -- proving the rejection comes from the trailing-`..` `file_name()`
        /// check, D-10.2, not merely from a nonexistent directory) and a
        /// `sub/dir` directory (for the positive-control nested-path case).
        /// `<base>/share-evil` is a SIBLING directory holding `secret.txt`
        /// (Pitfall 3's sibling-prefix regression guard: `share` is a strict
        /// string prefix of `share-evil`, which a raw `StartsWith` -- but
        /// not Rust's component-aware `Path::starts_with` -- would conflate).
        /// Every caller removes `base` (and everything under it) once done.
        fn setup() -> (RdpilotDriveBackend, std::path::PathBuf) {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let base = std::env::temp_dir().join(format!(
                "rdpilot-rdpdr-traversal-{}-{nanos}",
                std::process::id()
            ));
            let root = base.join("share");
            std::fs::create_dir_all(root.join("subdir")).expect("share/subdir creates");
            std::fs::create_dir_all(root.join("sub/dir")).expect("share/sub/dir creates");
            let sibling = base.join("share-evil");
            std::fs::create_dir_all(&sibling).expect("sibling share-evil creates");
            std::fs::write(sibling.join("secret.txt"), b"top secret").expect("sibling secret writes");

            let backend = RdpilotDriveBackend::new(
                std::env::temp_dir().join("rdpilot-rdpdr-traversal-sensor-placeholder"),
                "served.bin".to_owned(),
                Some(root),
            );
            (backend, base)
        }

        /// Assert `path` is rejected: `NtStatus::NO_SUCH_FILE`, file id 0,
        /// never granted a handle -- and that `handle_drive_io_request`
        /// itself still returns `Ok` (a rejection is a typed response, never
        /// a propagated `Err`, API-01).
        fn assert_rejected(backend: &mut RdpilotDriveBackend, path: &str) {
            let out = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(1, path)))
                .unwrap_or_else(|e| panic!("rejected create for {path:?} must still return Ok, got Err: {e}"));
            let (status, file_id) = create_response_fields(&out[0]);
            assert_eq!(status, NtStatus::NO_SUCH_FILE, "path {path:?} must be rejected with NO_SUCH_FILE");
            assert_eq!(file_id, 0, "path {path:?} must not be granted a file id");
        }

        /// Assert `path` is accepted: `NtStatus::SUCCESS`, a nonzero file id
        /// granted -- the positive control proving the validator does not
        /// over-reject a legitimate nested path.
        fn assert_accepted(backend: &mut RdpilotDriveBackend, path: &str) {
            let out = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(1, path)))
                .unwrap_or_else(|e| panic!("accepted create for {path:?} must return Ok, got Err: {e}"));
            let (status, file_id) = create_response_fields(&out[0]);
            assert_eq!(status, NtStatus::SUCCESS, "legitimate nested path {path:?} must be accepted");
            assert_ne!(file_id, 0, "legitimate nested path {path:?} must be granted a nonzero file id");
        }

        /// Case (1): trailing `..` with NO following separator -- the exact
        /// FreeRDP `contains_dotdot()` off-by-one class (CVE-2025-48817,
        /// GHSA-3xpj-m4hx-8vmx). `subdir` genuinely exists under the share
        /// root (see `setup`), so this proves rejection comes from
        /// `resolve_under_root`'s `candidate.file_name() == None` check on a
        /// trailing `..` component, not from a missing directory.
        #[test]
        fn rejects_trailing_dotdot_with_no_separator() {
            let (mut backend, base) = setup();
            assert_rejected(&mut backend, "\\subdir\\..");
            assert_rejected(&mut backend, "..");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// Case (2): mixed `/`+`\` separators. Unconditional `\`-to-`/`
        /// normalization BEFORE any `Path`/`PathBuf` construction (Pitfall
        /// 1) makes these faithful on this Linux test host, which does not
        /// otherwise treat `\` as a separator.
        #[test]
        fn rejects_mixed_separators() {
            let (mut backend, base) = setup();
            assert_rejected(&mut backend, "..\\../Windows\\System32");
            assert_rejected(&mut backend, "foo/..\\..\\bar");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// Case (3): absolute-path-as-relative inputs -- rejected by
        /// `resolve_under_root`'s own host-independent `looks_rooted` check
        /// (leading `/` or a drive-letter prefix), not by relying on
        /// `Path::is_absolute()`'s `cfg(windows)`-conditional semantics
        /// (which would NOT treat a drive-letter string as rooted on this
        /// Linux test host).
        #[test]
        fn rejects_absolute_path_as_relative() {
            let (mut backend, base) = setup();
            assert_rejected(&mut backend, "\\C:\\Windows");
            assert_rejected(&mut backend, "/etc/passwd");
            assert_rejected(&mut backend, "C:\\Windows\\System32");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// Case (4): sibling-directory-prefix regression guard (10-RESEARCH
        /// Pitfall 3). The share root is named `share`; the resolved
        /// candidate lands in the SIBLING `share-evil` directory, whose name
        /// is a strict string prefix match of `share` -- a raw
        /// `string.StartsWith`-style check would wrongly accept this, but
        /// Rust's component-aware `Path::starts_with` correctly rejects it.
        #[test]
        fn rejects_sibling_directory_prefix() {
            let (mut backend, base) = setup();
            assert_rejected(&mut backend, "..\\share-evil\\secret.txt");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// Positive control: a legitimate nested path is ACCEPTED -- proves
        /// the suite rejects attacks WITHOUT over-rejecting valid nested
        /// writes (Pitfall 2 warning sign).
        #[test]
        fn accepts_legitimate_nested_path() {
            let (mut backend, base) = setup();
            assert_accepted(&mut backend, "\\sub\\dir\\ok.bin");
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
