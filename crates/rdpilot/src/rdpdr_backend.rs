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
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use tracing::trace;

use ironrdp::core::impl_as_any;
use ironrdp::pdu::PduResult;
use ironrdp::svc::SvcMessage;
use ironrdp_rdpdr::backend::RdpdrBackend;
use ironrdp_rdpdr::pdu::RdpdrPdu;
use ironrdp_rdpdr::pdu::efs::{
    Boolean, Characteristics, ClientDriveQueryDirectoryResponse, ClientDriveQueryInformationResponse,
    ClientDriveQueryVolumeInformationResponse, ClientDriveSetInformationResponse, CreateDisposition,
    DeviceCloseRequest, DeviceCloseResponse, DeviceControlRequest, DeviceCreateRequest, DeviceCreateResponse,
    DeviceIoRequest, DeviceIoResponse, DeviceReadRequest, DeviceReadResponse, DeviceWriteRequest,
    DeviceWriteResponse, FileAttributeTagInformation, FileAttributes, FileBasicInformation,
    FileBothDirectoryInformation, FileDirectoryInformation, FileEndOfFileInformation, FileFsAttributeInformation,
    FileFsDeviceInformation, FileFsFullSizeInformation, FileFsSizeInformation, FileFsVolumeInformation,
    FileFullDirectoryInformation, FileInformationClass, FileInformationClassLevel, FileNamesInformation,
    FileStandardInformation, FileSystemAttributes, FileSystemInformationClass, FileSystemInformationClassLevel,
    Information, NtStatus, ServerDeviceAnnounceResponse, ServerDriveIoRequest, ServerDriveQueryDirectoryRequest,
    ServerDriveQueryInformationRequest, ServerDriveQueryVolumeInformationRequest, ServerDriveSetInformationRequest,
};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};

/// What a previously-granted RDPDR file id refers to: the drive root (a
/// directory), a resolved read-only file path, or an in-progress staged
/// write (10-02, D-10.3).
///
/// [`OpenEntry::File`] is either the sensor exe (the pre-Phase-10 read-only
/// special case) or a file under the configured share root, already
/// validated by [`RdpilotDriveBackend::resolve_under_root`] at `Create`
/// time (D-10.1/D-10.2). `handle_read` only ever streams bytes for
/// [`OpenEntry::File`] -- a `Read` against a root/directory/write handle
/// (or an unknown handle) is rejected before any `std::fs` call (T-05-04).
///
/// [`OpenEntry::WriteFile`] is granted when `handle_create` sees a
/// create/overwrite `CreateDisposition` (D-10.3): `dest` is the
/// `resolve_under_root`-validated, not-yet-existing final destination;
/// `staging` is a fresh `<share_root>/.rdpilot-staging/<uuid>.part` path
/// this backend allocates and creates itself (never server-derived, so no
/// additional validation is needed on it); `expected_len` is `None` until a
/// `FILE_END_OF_FILE_INFORMATION` `SetInformation` records the
/// authoritative total-size signal (`handle_set_information`) -- the STRICT
/// completeness rule `handle_close`'s `finalize_write` enforces: a clean
/// Close only renames `staging` to `dest` when `expected_len` is `Some` AND
/// the staged file's actual length matches it exactly; any other outcome
/// (no `SetInformation` ever received, or a short/interrupted transfer)
/// leaves the stale `.part` in staging -- a clean, detectable failure
/// (FILE-04) rather than a partially-written destination file. See
/// 10-02-SUMMARY.md for why this rule was chosen and what remains a
/// live-gate confirmation item (real Windows Close/SetInformation
/// ordering).
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpenEntry {
    Root,
    File(PathBuf),
    WriteFile {
        staging: PathBuf,
        dest: PathBuf,
        expected_len: Option<u64>,
    },
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
    /// A fresh counter (10-02, D-10.3) mixed into every staged `.part`
    /// filename alongside a nanosecond timestamp and the process id --
    /// guarantees a unique staging name per `Create` within one backend's
    /// lifetime without pulling in a `uuid` crate dependency (mirrors the
    /// existing test helper's `nanos`+pid scheme).
    next_staging_id: u64,
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
            next_staging_id: 1,
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

    /// `true` for the four `CreateDisposition` values that signal
    /// create/overwrite WRITE intent (D-10.3 Task 1 action): `FILE_CREATE`,
    /// `FILE_OPEN_IF`, `FILE_OVERWRITE_IF`, `FILE_SUPERSEDE`. Plain
    /// `FILE_OPEN` (and `FILE_OVERWRITE`, which -- like `FILE_OPEN` --
    /// requires the target to already exist) fall through to the existing
    /// read-oriented [`OpenEntry::File`] path unchanged, preserving every
    /// pre-10-02 Read/QueryInformation/QueryDirectory behavior byte-for-byte
    /// for those dispositions.
    fn is_write_disposition(disposition: CreateDisposition) -> bool {
        disposition == CreateDisposition::FILE_CREATE
            || disposition == CreateDisposition::FILE_OPEN_IF
            || disposition == CreateDisposition::FILE_OVERWRITE_IF
            || disposition == CreateDisposition::FILE_SUPERSEDE
    }

    /// Allocate and create (empty, truncated) a fresh
    /// `<share_root>/.rdpilot-staging/<uuid>.part` file for a new
    /// [`OpenEntry::WriteFile`] handle (D-10.3). The "uuid" is a
    /// nanosecond-timestamp + process-id + monotonic-counter combination
    /// (mirrors the existing test helpers' scheme; no new crate dependency)
    /// -- collision-free for any realistic number of concurrent transfers
    /// within one backend's lifetime. Returns `None` (never panics, API-01)
    /// if no share root is configured, the root fails to canonicalize, or
    /// the staging file fails to create (e.g. `.rdpilot-staging/` missing --
    /// `connect.rs` is responsible for pre-creating it, 10-01) -- any of
    /// which reject the `Create` with `NtStatus::NO_SUCH_FILE`, same as an
    /// unresolvable share-root path.
    fn allocate_staging_path(&mut self) -> Option<PathBuf> {
        let root = self.share_root.as_ref()?;
        let root_canonical = fs::canonicalize(root).ok()?;

        let id = self.next_staging_id;
        self.next_staging_id = self.next_staging_id.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let staging = root_canonical
            .join(".rdpilot-staging")
            .join(format!("{nanos}-{}-{id}.part", std::process::id()));

        fs::File::create(&staging).ok()?;
        Some(staging)
    }

    /// [`ServerDriveIoRequest::ServerCreateDriveRequest`]: the drive root
    /// always resolves; the sensor exe name resolves to its fixed read-only
    /// path (the pre-Phase-10 bootstrap special case, unchanged); a
    /// create/overwrite disposition ([`Self::is_write_disposition`], D-10.3)
    /// against any other name resolves the destination via
    /// [`Self::resolve_under_root`] (ancestry-validated BEFORE any staging
    /// file is created) and, on success, allocates a fresh staged
    /// [`OpenEntry::WriteFile`] handle ([`Self::allocate_staging_path`]);
    /// every other (read-oriented) name is routed through
    /// [`Self::resolve_under_root`] (D-10.2) into the existing
    /// [`OpenEntry::File`] path unchanged. An escaping/unresolvable path, or
    /// a staging-file allocation failure, is rejected with
    /// `NtStatus::NO_SUCH_FILE` (the not-found status this crate's `efs.rs`
    /// actually defines -- RESEARCH Open Question #1) and never reaches
    /// `std::fs` for the destination (T-05-04).
    fn handle_create(&mut self, req: DeviceCreateRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceCreateRequest {
            device_io_request,
            create_disposition,
            path,
            ..
        } = req;
        let normalized = Self::normalize(&path);

        let entry = if normalized.is_empty() {
            Some(OpenEntry::Root)
        } else if normalized.eq_ignore_ascii_case(&self.sensor_name) {
            Some(OpenEntry::File(self.sensor_path.clone()))
        } else if Self::is_write_disposition(create_disposition) {
            match self.resolve_under_root(normalized) {
                Ok(dest) => self.allocate_staging_path().map(|staging| OpenEntry::WriteFile {
                    staging,
                    dest,
                    expected_len: None,
                }),
                Err(()) => None,
            }
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
    /// twice is not an error worth surfacing to the remote peer). Closing an
    /// [`OpenEntry::WriteFile`] handle additionally runs
    /// [`Self::finalize_write`] (10-02, D-10.3/FILE-04) BEFORE this
    /// completion is sent -- a clean, complete transfer atomically renames
    /// the staged `.part` to its destination; an interrupted one leaves the
    /// stale `.part` in staging. Either outcome still replies
    /// `NtStatus::SUCCESS` -- a `Close` always succeeds per MS-RDPEFS
    /// regardless of the transfer's own outcome; FILE-04's "clean, detectable
    /// failure" is the ABSENCE of the destination file plus the PRESENCE of
    /// the stale `.part`, not an error status on this response.
    fn handle_close(&mut self, req: DeviceCloseRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceCloseRequest { device_io_request } = req;
        if let Some(OpenEntry::WriteFile {
            staging,
            dest,
            expected_len,
        }) = self.open_files.remove(&device_io_request.file_id)
        {
            Self::finalize_write(&staging, &dest, expected_len);
        }
        let response = DeviceCloseResponse {
            device_io_response: DeviceIoResponse::new(device_io_request, NtStatus::SUCCESS),
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::DeviceCloseResponse(response))])
    }

    /// Complete a [`OpenEntry::WriteFile`] handle's staged transfer on
    /// `Close` (10-02, D-10.3, FILE-04). Atomically `fs::rename`s `staging`
    /// to `dest` -- a single syscall on the target filesystem, so no reader
    /// can ever observe a partially-written `dest` (Don't Hand-Roll,
    /// 10-RESEARCH) -- ONLY when BOTH: (1) `expected_len` is `Some` (a prior
    /// `FILE_END_OF_FILE_INFORMATION` `SetInformation` was received,
    /// [`Self::handle_set_information`]) AND (2) the staged file's actual
    /// on-disk length exactly matches it. This is the STRICT
    /// completeness rule this plan adopts (see 10-02-SUMMARY.md): it
    /// deliberately does NOT treat "no `SetInformation` ever sent, but bytes
    /// were written" as complete, because without an authoritative signal
    /// this backend cannot distinguish "the sensor finished writing" from
    /// "the transfer was cut off mid-stream" -- the real Windows
    /// Close/SetInformation ordering (does the OS always send
    /// `FILE_END_OF_FILE_INFORMATION` before `Close` for a redirected-drive
    /// write? RESEARCH flags this in the "Windows' write sequence issues
    /// these" note) is a genuine live-gate confirmation item (Plan 10-05),
    /// not something assumable offline.
    ///
    /// Any other outcome (`expected_len` still `None`, a length mismatch --
    /// FILE-04's interrupted-transfer case -- or the `fs::rename` call
    /// itself failing) leaves `staging` untouched: a stale `.part` file is
    /// the clean, detectable failure signal FILE-04 requires, never a
    /// partially-written `dest`. Never panics (API-01): every `std::fs`
    /// failure here is silently absorbed into "stay staged, do not rename",
    /// not propagated as an error -- `Close` itself always still succeeds
    /// (see [`Self::handle_close`]'s doc comment).
    fn finalize_write(staging: &Path, dest: &Path, expected_len: Option<u64>) {
        let Some(expected_len) = expected_len else {
            return;
        };
        let Ok(metadata) = fs::metadata(staging) else {
            return;
        };
        if metadata.len() != expected_len {
            return;
        }
        let _ = fs::rename(staging, dest);
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

        // 10-05 live-gate diagnostic only (Rule 2 addition, 10-RESEARCH Open
        // Question 1): records the real per-IRP offset/length the remote
        // Windows OS actually requests -- zero cost when unsubscribed
        // (`tracing`'s whole design point), never asserted on in production
        // code, only observed by the live-gate test's own subscriber.
        trace!(offset, len = length, "rdpdr_read_irp");

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

    /// [`ServerDriveIoRequest::DeviceWriteRequest`] (10-02, D-10.3): one
    /// bounded `seek`+`write` per IRP into the handle's staged `.part` file
    /// -- never buffers/accumulates the whole transfer in memory (T-05-05).
    /// `offset`/`write_data.len()` come entirely from the IRP; no per-IRP
    /// chunk-size constant is assumed or hardcoded anywhere (10-RESEARCH
    /// Item 1) -- the OS-chosen real chunk size is only observable at the
    /// 10-05 live gate, and this loop-per-IRP design already handles
    /// whatever size it turns out to be.
    ///
    /// A `file_id` that is unknown, or was granted [`OpenEntry::Root`]/
    /// [`OpenEntry::File`] (read-only, including the sensor exe) rather than
    /// [`OpenEntry::WriteFile`], is rejected with `NtStatus::ACCESS_DENIED`
    /// *before* any `std::fs` call -- mirrors [`Self::handle_read`]'s
    /// access-control discipline (T-05-04). Any IO failure on the actual
    /// `seek`+`write` maps to `NtStatus::UNSUCCESSFUL`, never a panic
    /// (API-01).
    fn handle_write(&self, req: DeviceWriteRequest) -> PduResult<Vec<SvcMessage>> {
        let DeviceWriteRequest {
            device_io_request,
            offset,
            write_data,
        } = req;

        let Some(OpenEntry::WriteFile { staging, .. }) = self.open_files.get(&device_io_request.file_id) else {
            let response = DeviceWriteResponse {
                device_io_reply: DeviceIoResponse::new(device_io_request, NtStatus::ACCESS_DENIED),
                length: 0,
            };
            return Ok(vec![SvcMessage::from(RdpdrPdu::DeviceWriteResponse(response))]);
        };

        // 10-05 live-gate diagnostic only (Rule 2 addition, 10-RESEARCH Open
        // Question 1): records the real per-IRP offset/length the remote
        // Windows OS actually requests -- zero cost when unsubscribed, never
        // asserted on in production code.
        trace!(offset, len = write_data.len() as u64, "rdpdr_write_irp");

        let (status, length) = match Self::write_bytes_at(staging, offset, &write_data) {
            Ok(written) => (NtStatus::SUCCESS, written),
            // Any IO failure maps to a typed NtStatus response, never a
            // panic (API-01, T-05-05).
            Err(_io_error) => (NtStatus::UNSUCCESSFUL, 0),
        };

        let response = DeviceWriteResponse {
            device_io_reply: DeviceIoResponse::new(device_io_request, status),
            length,
        };
        Ok(vec![SvcMessage::from(RdpdrPdu::DeviceWriteResponse(response))])
    }

    /// `seek` to `offset` and write exactly `data` into `path` (the staged
    /// `.part` file) -- one bounded write, no unbounded preallocation
    /// (T-05-05, D-10.3). Returns the number of bytes written (always
    /// `data.len()` on success, per `DeviceWriteResponse::length`'s "MUST
    /// echo the number of bytes actually written" contract -- RESEARCH Code
    /// Examples); `u32::try_from` degrades to `u32::MAX` rather than
    /// panicking in the (practically unreachable, IRPs are not that large)
    /// event `data.len()` exceeds `u32::MAX` (API-01).
    fn write_bytes_at(path: &Path, offset: u64, data: &[u8]) -> std::io::Result<u32> {
        let mut file = fs::OpenOptions::new().write(true).open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(data)?;
        Ok(u32::try_from(data.len()).unwrap_or(u32::MAX))
    }

    /// [`ServerDriveIoRequest::ServerDriveSetInformationRequest`] (10-02,
    /// D-10.3): `efs.rs`'s `decode()` only ever admits 5
    /// `FileInformationClassLevel` sub-variants for this IRP (Basic/
    /// EndOfFile/Disposition/Rename/Allocation, RESEARCH Code Examples) --
    /// Windows' standard write sequence issues this IRP as a matter of
    /// course, so ALL 5 must return a well-formed
    /// `ClientDriveSetInformationResponse`, never `NOT_SUPPORTED`.
    ///
    /// Only [`FileInformationClass::EndOfFile`] carries semantic weight for
    /// this minimal backend: its `end_of_file` value is stored as the
    /// [`OpenEntry::WriteFile`] handle's `expected_len` -- the authoritative
    /// "expected total bytes" signal [`Self::finalize_write`] later checks
    /// on `Close` (D-10.3, FILE-04's completeness rule -- see that fn's doc
    /// comment for the STRICT rule chosen and why). The other 4 sub-variants
    /// reply success with no side effect: this backend does not model
    /// rename/disposition/allocation-size semantics, only end-of-file.
    ///
    /// A `file_id` that is unknown, or was granted [`OpenEntry::Root`]/
    /// [`OpenEntry::File`] (read-only) rather than [`OpenEntry::WriteFile`],
    /// is rejected with `NtStatus::ACCESS_DENIED` (mirrors
    /// [`Self::handle_write`]'s access-control discipline, T-05-04) --
    /// still via a well-formed `ClientDriveSetInformationResponse`, per its
    /// own doc comment ("length MUST be equal to the Length field in the
    /// ... Request" regardless of the reported `io_status`).
    /// `ClientDriveSetInformationResponse::new`'s own `cast_length!` can, in
    /// principle, fail to encode (an oversized `set_buffer.size()`) -- that
    /// case falls back to a generic reject completion rather than
    /// unwrapping/panicking (API-01).
    fn handle_set_information(&mut self, req: ServerDriveSetInformationRequest) -> PduResult<Vec<SvcMessage>> {
        let file_id = req.device_io_request.file_id;

        let status = match self.open_files.get_mut(&file_id) {
            Some(OpenEntry::WriteFile { expected_len, .. }) => {
                if let FileInformationClass::EndOfFile(FileEndOfFileInformation { end_of_file }) = &req.set_buffer {
                    if let Ok(len) = u64::try_from(*end_of_file) {
                        *expected_len = Some(len);
                    }
                }
                NtStatus::SUCCESS
            }
            _ => NtStatus::ACCESS_DENIED,
        };

        match ClientDriveSetInformationResponse::new(&req, status) {
            Ok(resp) => Ok(vec![SvcMessage::from(RdpdrPdu::ClientDriveSetInformationResponse(resp))]),
            // Encoding failure maps to a typed reject completion, never a
            // panic (API-01) -- practically unreachable for the 5 small,
            // fixed-size FileInformationClass payloads this IRP admits.
            Err(_encode_error) => Ok(vec![SvcMessage::from(RdpdrPdu::DeviceCloseResponse(DeviceCloseResponse {
                device_io_response: DeviceIoResponse::new(req.device_io_request, NtStatus::UNSUCCESSFUL),
            }))]),
        }
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
            // Report the CURRENT staged size for an in-progress WriteFile
            // handle (10-02) -- the same defensive stat-failure-degrades-
            // to-zero fallback as the File arm above.
            OpenEntry::WriteFile { staging, .. } => fs::metadata(staging)
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
    /// (`efs.rs`, read at execution time): the eight variants this backend
    /// implements -- Create/Close/Read/QueryDirectory/QueryInformation/
    /// QueryVolumeInformation (Plan 05/10-01) plus Write/SetInformation
    /// (10-02, D-10.3) -- plus the remaining variants it rejects with a
    /// typed `NOT_SUPPORTED` completion.
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
            ServerDriveIoRequest::DeviceWriteRequest(r) => self.handle_write(r),
            ServerDriveIoRequest::ServerDriveSetInformationRequest(r) => self.handle_set_information(r),
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
        DeviceIoResponse, DeviceReadRequest, DeviceWriteRequest, FileAttributes, FileEndOfFileInformation,
        FileInformationClass, FileInformationClassLevel, FileSystemInformationClassLevel, MajorFunction,
        MinorFunction, NtStatus, ServerDriveIoRequest, ServerDriveQueryDirectoryRequest,
        ServerDriveQueryInformationRequest, ServerDriveQueryVolumeInformationRequest, ServerDriveSetInformationRequest,
        SharedAccess,
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

    /// 10-02 Task 1: staged-write path (`DeviceWriteRequest` into
    /// `<uuid>.part`, D-10.3) offline tests -- FILE-04's "chunked loop
    /// actually loops" proxy plus the write-side access-control regression
    /// guard mirroring `read_for_unopened_file_id_is_rejected_without_touching_filesystem`.
    mod write_path {
        use super::*;

        /// A fresh temp share root, WITH `.rdpilot-staging/` pre-created
        /// (mirrors `connect.rs`'s real pre-Create setup, 10-01-SUMMARY.md)
        /// -- without it, `allocate_staging_path` would fail to create the
        /// `.part` file and every write-disposition `Create` in these tests
        /// would be rejected.
        fn setup() -> (RdpilotDriveBackend, std::path::PathBuf) {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let base = std::env::temp_dir().join(format!(
                "rdpilot-rdpdr-write-{}-{nanos}",
                std::process::id()
            ));
            let root = base.join("share");
            std::fs::create_dir_all(root.join(".rdpilot-staging")).expect("share/.rdpilot-staging creates");

            let backend = RdpilotDriveBackend::new(
                std::env::temp_dir().join("rdpilot-rdpdr-write-sensor-placeholder"),
                "served.bin".to_owned(),
                Some(root),
            );
            (backend, base)
        }

        fn create_write_req(file_id: u32, path: &str, disposition: CreateDisposition) -> DeviceCreateRequest {
            DeviceCreateRequest {
                create_disposition: disposition,
                ..create_req(file_id, path)
            }
        }

        fn write_req(file_id: u32, offset: u64, data: &[u8]) -> DeviceWriteRequest {
            DeviceWriteRequest {
                device_io_request: dev_io_req(file_id, MajorFunction::Write),
                offset,
                write_data: data.to_vec(),
            }
        }

        /// `(io_status, length)` from a `DeviceWriteResponse`-wrapped message.
        fn write_response_fields(msg: &SvcMessage) -> (NtStatus, u32) {
            let (status, tail) = decode_io_status_and_tail(msg);
            let mut cursor = ReadCursor::new(&tail);
            let length = cursor.read_u32();
            (status, length)
        }

        /// The single staged `.part` file's on-disk bytes -- there is
        /// exactly one entry under `.rdpilot-staging/` per test in this
        /// module (one `Create` each), so reading the sole directory entry
        /// is a faithful, encapsulation-respecting way to inspect what
        /// `handle_write` actually wrote without exposing `OpenEntry`'s
        /// private `staging` field to tests.
        fn read_sole_staged_file(root: &std::path::Path) -> Vec<u8> {
            let staging_dir = root.join(".rdpilot-staging");
            let mut entries: Vec<_> = std::fs::read_dir(&staging_dir)
                .expect("staging dir reads")
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .collect();
            assert_eq!(entries.len(), 1, "expected exactly one staged .part file");
            std::fs::read(entries.remove(0)).expect("staged file reads")
        }

        /// Three sequential `DeviceWriteRequest` IRPs at increasing offsets
        /// (0, N, 2N with DIFFERENT chunk lengths -- deliberately NOT a
        /// fixed/hardcoded chunk size, 10-RESEARCH Item 1) reassemble into
        /// one staged file with the concatenated bytes in the exact order
        /// written; each `DeviceWriteResponse` echoes the exact per-IRP
        /// bytes-written length -- the chunked write loop actually loops
        /// (FILE-04's offline proxy).
        #[test]
        fn multi_irp_write_reassembles_in_order() {
            let (mut backend, base) = setup();
            let root = base.join("share");

            let created = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_write_req(
                    1,
                    "\\upload.bin",
                    CreateDisposition::FILE_OPEN_IF,
                )))
                .expect("write-disposition create returns Ok");
            let (status, file_id) = create_response_fields(&created[0]);
            assert_eq!(status, NtStatus::SUCCESS);
            assert_ne!(file_id, 0);

            // Three chunks of DIFFERENT lengths -- proves no assumed/fixed
            // chunk-size constant is baked into the write path.
            let chunks: [&[u8]; 3] = [b"AAAA", b"BB", b"CCCCCC"];
            let mut offset = 0u64;
            let mut expected = Vec::new();
            for chunk in chunks {
                let out = backend
                    .handle_drive_io_request(ServerDriveIoRequest::DeviceWriteRequest(write_req(
                        file_id, offset, chunk,
                    )))
                    .expect("write returns Ok");
                let (status, length) = write_response_fields(&out[0]);
                assert_eq!(status, NtStatus::SUCCESS);
                assert_eq!(length as usize, chunk.len(), "response must echo bytes actually written");
                offset += chunk.len() as u64;
                expected.extend_from_slice(chunk);
            }

            let staged_bytes = read_sole_staged_file(&root);
            assert_eq!(staged_bytes, expected, "staged file must reassemble the writes in exact order");

            let _ = std::fs::remove_dir_all(&base);
        }

        /// A `Write` against a file id that was never granted an
        /// `OpenEntry::WriteFile` -- unknown id, OR a plain read-only
        /// `OpenEntry::File` handle (the sensor exe) -- is rejected with
        /// `NtStatus::ACCESS_DENIED` and touches no `std::fs` write (mirrors
        /// `read_for_unopened_file_id_is_rejected_without_touching_filesystem`'s
        /// access-control discipline, T-05-04).
        #[test]
        fn write_against_read_only_or_unknown_handle_is_rejected() {
            let (mut backend, base) = setup();

            // Unknown file id entirely.
            let unknown = backend
                .handle_drive_io_request(ServerDriveIoRequest::DeviceWriteRequest(write_req(9999, 0, b"x")))
                .expect("rejected write still returns Ok");
            let (status, length) = write_response_fields(&unknown[0]);
            assert_eq!(status, NtStatus::ACCESS_DENIED);
            assert_eq!(length, 0);

            // A read-only OpenEntry::File handle (the sensor exe special
            // case, FILE_OPEN disposition) must also reject a Write.
            let sensor_created = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_req(
                    1,
                    "\\served.bin",
                )))
                .expect("sensor-exe create returns Ok");
            let (status, sensor_file_id) = create_response_fields(&sensor_created[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let rejected = backend
                .handle_drive_io_request(ServerDriveIoRequest::DeviceWriteRequest(write_req(
                    sensor_file_id,
                    0,
                    b"x",
                )))
                .expect("rejected write still returns Ok");
            let (status, length) = write_response_fields(&rejected[0]);
            assert_eq!(status, NtStatus::ACCESS_DENIED);
            assert_eq!(length, 0);

            let _ = std::fs::remove_dir_all(&base);
        }

        /// `SetInformation(FILE_END_OF_FILE_INFORMATION)` + `Close` on a
        /// WriteFile handle whose staged bytes fully account for the
        /// declared `end_of_file` (10-02 Task 2, D-10.3, FILE-04 clean
        /// path): the `.part` is atomically renamed to the resolved
        /// destination; the destination exists with the exact full content;
        /// no stale `.part` remains in staging.
        #[test]
        fn set_information_end_of_file_then_close_renames_on_complete_transfer() {
            let (mut backend, base) = setup();
            let root = base.join("share");
            let root_canonical = std::fs::canonicalize(&root).expect("root canonicalizes");
            let dest = root_canonical.join("upload.bin");

            let created = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_write_req(
                    1,
                    "\\upload.bin",
                    CreateDisposition::FILE_OPEN_IF,
                )))
                .expect("write-disposition create returns Ok");
            let (status, file_id) = create_response_fields(&created[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let payload = b"the-quick-brown-fox";
            let write_out = backend
                .handle_drive_io_request(ServerDriveIoRequest::DeviceWriteRequest(write_req(file_id, 0, payload)))
                .expect("write returns Ok");
            let (status, length) = write_response_fields(&write_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);
            assert_eq!(length as usize, payload.len());

            let set_info = ServerDriveIoRequest::ServerDriveSetInformationRequest(ServerDriveSetInformationRequest {
                device_io_request: dev_io_req(file_id, MajorFunction::SetInformation),
                set_buffer: FileInformationClass::EndOfFile(FileEndOfFileInformation {
                    end_of_file: payload.len() as i64,
                }),
            });
            let set_info_out = backend
                .handle_drive_io_request(set_info)
                .expect("set-information returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&set_info_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let close = ServerDriveIoRequest::DeviceCloseRequest(DeviceCloseRequest {
                device_io_request: dev_io_req(file_id, MajorFunction::Close),
            });
            let close_out = backend.handle_drive_io_request(close).expect("close returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&close_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            assert!(dest.exists(), "clean transfer must rename staging to the destination");
            assert_eq!(std::fs::read(&dest).expect("destination reads"), payload);

            let staging_dir = root.join(".rdpilot-staging");
            let remaining: Vec<_> = std::fs::read_dir(&staging_dir)
                .expect("staging dir reads")
                .filter_map(|e| e.ok())
                .collect();
            assert!(remaining.is_empty(), "no stale .part should remain after a clean rename");

            let _ = std::fs::remove_dir_all(&base);
        }

        /// A `Close` on a WriteFile handle that is INCOMPLETE -- fewer
        /// staged bytes than the declared `end_of_file` (10-02 Task 2,
        /// D-10.3, FILE-04 interrupted-transfer detection) -- does NOT
        /// rename: the destination name never appears, and the stale
        /// `.part` remains in staging for detection. This is FILE-04's
        /// "clean, detectable failure" proven offline.
        #[test]
        fn close_on_incomplete_transfer_never_renames_and_leaves_stale_part() {
            let (mut backend, base) = setup();
            let root = base.join("share");
            let root_canonical = std::fs::canonicalize(&root).expect("root canonicalizes");
            let dest = root_canonical.join("upload.bin");

            let created = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_write_req(
                    1,
                    "\\upload.bin",
                    CreateDisposition::FILE_OPEN_IF,
                )))
                .expect("write-disposition create returns Ok");
            let (status, file_id) = create_response_fields(&created[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            // Declare a total size of 20 bytes but only ever write 5 --
            // the interrupted-transfer case.
            let set_info = ServerDriveIoRequest::ServerDriveSetInformationRequest(ServerDriveSetInformationRequest {
                device_io_request: dev_io_req(file_id, MajorFunction::SetInformation),
                set_buffer: FileInformationClass::EndOfFile(FileEndOfFileInformation { end_of_file: 20 }),
            });
            let set_info_out = backend
                .handle_drive_io_request(set_info)
                .expect("set-information returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&set_info_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let write_out = backend
                .handle_drive_io_request(ServerDriveIoRequest::DeviceWriteRequest(write_req(file_id, 0, b"first")))
                .expect("write returns Ok");
            let (status, _length) = write_response_fields(&write_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let close = ServerDriveIoRequest::DeviceCloseRequest(DeviceCloseRequest {
                device_io_request: dev_io_req(file_id, MajorFunction::Close),
            });
            let close_out = backend.handle_drive_io_request(close).expect("close returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&close_out[0]);
            assert_eq!(status, NtStatus::SUCCESS, "Close itself always succeeds per MS-RDPEFS");

            assert!(!dest.exists(), "an interrupted transfer must NEVER produce the destination file");

            let staging_dir = root.join(".rdpilot-staging");
            let remaining: Vec<_> = std::fs::read_dir(&staging_dir)
                .expect("staging dir reads")
                .filter_map(|e| e.ok())
                .collect();
            assert_eq!(remaining.len(), 1, "the stale .part must remain in staging for detection");

            let _ = std::fs::remove_dir_all(&base);
        }

        /// A `Close` on a WriteFile handle that NEVER received a
        /// `SetInformation(FILE_END_OF_FILE_INFORMATION)` also does NOT
        /// rename (the STRICT completeness rule, see `finalize_write`'s doc
        /// comment) -- this specific ordering (does real Windows always send
        /// end-of-file before Close?) is the live-gate confirmation item
        /// this plan explicitly flags, not assumed here.
        #[test]
        fn close_without_any_set_information_never_renames() {
            let (mut backend, base) = setup();
            let root = base.join("share");
            let root_canonical = std::fs::canonicalize(&root).expect("root canonicalizes");
            let dest = root_canonical.join("upload.bin");

            let created = backend
                .handle_drive_io_request(ServerDriveIoRequest::ServerCreateDriveRequest(create_write_req(
                    1,
                    "\\upload.bin",
                    CreateDisposition::FILE_OPEN_IF,
                )))
                .expect("write-disposition create returns Ok");
            let (status, file_id) = create_response_fields(&created[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let write_out = backend
                .handle_drive_io_request(ServerDriveIoRequest::DeviceWriteRequest(write_req(file_id, 0, b"data")))
                .expect("write returns Ok");
            let (status, _length) = write_response_fields(&write_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            let close = ServerDriveIoRequest::DeviceCloseRequest(DeviceCloseRequest {
                device_io_request: dev_io_req(file_id, MajorFunction::Close),
            });
            let close_out = backend.handle_drive_io_request(close).expect("close returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&close_out[0]);
            assert_eq!(status, NtStatus::SUCCESS);

            assert!(!dest.exists(), "no SetInformation ever received means no authoritative completeness signal");

            let _ = std::fs::remove_dir_all(&base);
        }

        /// `SetInformation` on an unknown/read-only handle is rejected with
        /// `NtStatus::ACCESS_DENIED`, never a panic (mirrors
        /// `write_against_read_only_or_unknown_handle_is_rejected`).
        #[test]
        fn set_information_against_read_only_or_unknown_handle_is_rejected() {
            let (mut backend, base) = setup();

            let unknown = ServerDriveIoRequest::ServerDriveSetInformationRequest(ServerDriveSetInformationRequest {
                device_io_request: dev_io_req(9999, MajorFunction::SetInformation),
                set_buffer: FileInformationClass::EndOfFile(FileEndOfFileInformation { end_of_file: 4 }),
            });
            let out = backend
                .handle_drive_io_request(unknown)
                .expect("rejected set-information still returns Ok");
            let (status, _tail) = decode_io_status_and_tail(&out[0]);
            assert_eq!(status, NtStatus::ACCESS_DENIED);

            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
