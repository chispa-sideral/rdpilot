---
phase: 05-sensor-bootstrap-deployment
plan: 02
subsystem: rdp-transport
tags: [ironrdp-rdpdr, rdpdr, drive-redirection, ms-rdpefs, key-input, rust]

# Dependency graph
requires:
  - phase: 03-input-injection
    provides: "Key enum + scancode()/combo_operations() machinery this plan extends with Key::Win"
  - phase: 04-dvc-transport-channel
    provides: "ironrdp umbrella feature pattern (svc/core re-exports) this plan's RdpdrBackend leans on"
provides:
  - "Key::Win variant (Set-1 extended scancode 0x5B), Combo-expressible, terminal (not modifier) key"
  - "ironrdp-rdpdr = \"0.6\" dependency, resolved cleanly against the pinned ironrdp graph"
  - "RdpilotDriveBackend: a minimal, portable (std::fs-based) ironrdp_rdpdr::backend::RdpdrBackend serving exactly one read-only file (Create/Close/Read/QueryDirectory), with a hard path allow-list and typed rejection of the other 7 ServerDriveIoRequest variants"
affects: [05-03-PLAN, 05-04-PLAN, "any future RDPDR/drive-redirection extension"]

# Tech tracking
tech-stack:
  added: ["ironrdp-rdpdr 0.6.0 (Devolutions/IronRDP, same release train as sibling ironrdp-* crates)"]
  patterns:
    - "RdpdrBackend implemented as a pub(crate) struct with a hard server-supplied-path allow-list evaluated BEFORE any std::fs call (T-05-04) -- rejection is structural, not a runtime filter"
    - "Every std::fs/IO failure maps to a typed NtStatus response (never unwrap/expect/panic outside #[cfg(test)], API-01)"
    - "Unsupported IRP variants completed with a typed NOT_SUPPORTED DeviceCloseResponse, mirroring ironrdp-rdpdr's own handle_printer_io_request default -- never silently dropped, never a panic"

key-files:
  created:
    - crates/rdpilot/src/rdpdr_backend.rs
  modified:
    - crates/rdpilot/Cargo.toml
    - crates/rdpilot/src/input.rs
    - crates/rdpilot/src/lib.rs
    - Cargo.lock

key-decisions:
  - "Task 1 package-legitimacy checkpoint (ironrdp-rdpdr, gate=blocking-human) was pre-approved by the user before this session started (same Devolutions/IronRDP umbrella already used throughout the project) -- recorded here, not re-asked."
  - "efs.rs read_first resolution (RESEARCH Open Question #1): ServerDriveIoRequest has 11 variants, not the plan's assumed 4 -- ServerCreateDriveRequest, ServerDriveQueryInformationRequest, DeviceCloseRequest, ServerDriveQueryDirectoryRequest, ServerDriveNotifyChangeDirectoryRequest, ServerDriveQueryVolumeInformationRequest, DeviceControlRequest<AnyIoCtlCode>, DeviceReadRequest, DeviceWriteRequest, ServerDriveSetInformationRequest, ServerDriveLockControlRequest. The 4 planned variants (Create/Close/Read/QueryDirectory) are fully implemented; the other 7 are exhaustively matched and completed with a typed NOT_SUPPORTED DeviceCloseResponse (Rust match exhaustiveness forced this; treated as a normal, cheaply-fixed planning surprise per RESEARCH Assumption framing, not a blocker)."
  - "The not-found NtStatus this crate actually defines is NtStatus::NO_SUCH_FILE (STATUS_NO_SUCH_FILE, 0xC000_000F) -- there is no OBJECT_NAME_NOT_FOUND constant in efs.rs. Used for rejected Create paths."
  - "Read against an unknown/foreign file_id (never granted OpenEntry::File by handle_create) is rejected with NtStatus::ACCESS_DENIED before any std::fs call -- kept distinct from NtStatus::UNSUCCESSFUL (a genuine IO failure on the served file) so the two code paths are independently testable and the access-control boundary is structurally provable."
  - "RdpdrPdu::encode (pdu/mod.rs) writes a 4-byte SharedHeader (component/packet_id) before the inner PDU body -- the test-only byte decoder accounts for this offset when parsing SvcMessage::encode_unframed_pdu() output."
  - "QueryDirectory's InitialQuery byte (MS-RDPEFS 2.2.3.3.10) is used directly as the enumeration-complete signal: nonzero = first query (return the one served file's metadata), zero = subsequent query (NtStatus::NO_MORE_FILES, no buffer) -- no extra per-handle enumeration state needed."

patterns-established:
  - "Test-only SvcMessage byte decoding via ironrdp::core::ReadCursor + each response PDU's own DeviceIoResponse::decode, manually skipping RdpdrPdu's 4-byte SharedHeader prefix -- the sanctioned way to assert on RDPDR response content in offline unit tests without a public decode path (response PDUs are client-encode-only in this crate)."

requirements-completed: [SENSOR-02]

# Metrics
duration: ~15min
completed: 2026-07-09
---

# Phase 5 Plan 2: RDPDR Offline Foundation Summary

**Added `ironrdp-rdpdr` + `Key::Win` (Set-1 extended 0x5B) and a minimal `RdpilotDriveBackend` that serves exactly one read-only file over MS-RDPEFS with a hard path allow-list, laying the offline Rust foundation for the drive-redirection launch path.**

## Performance

- **Duration:** ~15 min (task-commit span 12:27:20+02:00 → 12:40:17+02:00)
- **Tasks:** 3 (1 checkpoint, 2 auto/TDD)
- **Files modified:** 4 (Cargo.toml, input.rs, lib.rs, Cargo.lock) + 1 created (rdpdr_backend.rs)

## Accomplishments

- `Key::Win` added to the input vocabulary (`(extended=true, 0x5B)`, Set-1 left Windows/GUI key), Combo-expressible and confirmed to make D-5.1's Win+R launch sequence constructible
- `ironrdp-rdpdr = "0.6"` resolves cleanly against the already-pinned `ironrdp-svc`/`ironrdp-pdu` graph (RESEARCH Assumption A1 confirmed) -- `ironrdp-rdpdr-native` deliberately absent
- `RdpilotDriveBackend` implements `ironrdp_rdpdr::backend::RdpdrBackend`: Create/Close/Read/QueryDirectory fully implemented, hard path allow-list enforced (T-05-04), other 7 IRP variants typed-rejected, no panics anywhere outside `#[cfg(test)]`

## Task Commits

Each task was committed atomically:

1. **Task 1: Package legitimacy gate for `ironrdp-rdpdr`** — no commit (checkpoint only; pre-approved by the user before this session, per the executor prompt's explicit instruction not to re-ask)
2. **Task 2: Add `ironrdp-rdpdr` dependency + `Key::Win` variant and scancode**
   - `95f31e4` (test) — RED: failing test referencing the not-yet-existing `Key::Win`
   - `c077aea` (feat) — GREEN: `Key::Win` variant + scancode arm, `ironrdp-rdpdr` dependency, `mod rdpdr_backend;` placeholder declaration
3. **Task 3: Implement `RdpilotDriveBackend` (Create/Close/Read/QueryDirectory)**
   - `7355f36` (test) — RED: failing test referencing the not-yet-existing `RdpilotDriveBackend`
   - `f383313` (feat) — GREEN: full `RdpdrBackend` implementation + 4 unit tests

_No REFACTOR commits needed — both GREEN implementations were clean on first pass after the two escape-sequence fixes described below (test-authoring mistakes, fixed before commit, not separate commits)._

## Files Created/Modified

- `crates/rdpilot/src/rdpdr_backend.rs` — new module: `RdpilotDriveBackend` (the minimal RDPDR drive backend) + 4 offline unit tests
- `crates/rdpilot/Cargo.toml` — added `ironrdp-rdpdr = "0.6"` dependency
- `crates/rdpilot/src/input.rs` — added `Key::Win` variant + `scancode()` match arm + 2 unit tests
- `crates/rdpilot/src/lib.rs` — added `mod rdpdr_backend;` (crate-internal, not re-exported per D-09)
- `Cargo.lock` — new `ironrdp-rdpdr` entry

## efs.rs read_first resolution (RESEARCH Open Question #1)

Read at execution time from the pinned `ironrdp-rdpdr-0.6.0` source
(`~/.cargo/registry/src/*/ironrdp-rdpdr-0.6.0/src/pdu/efs.rs` and
`src/backend/mod.rs`), resolving the plan's Task 3 open question:

- **`ServerDriveIoRequest` has 11 variants, not the plan's assumed 4:**
  `ServerCreateDriveRequest(DeviceCreateRequest)`,
  `ServerDriveQueryInformationRequest`, `DeviceCloseRequest`,
  `ServerDriveQueryDirectoryRequest`,
  `ServerDriveNotifyChangeDirectoryRequest`,
  `ServerDriveQueryVolumeInformationRequest`,
  `DeviceControlRequest<AnyIoCtlCode>`, `DeviceReadRequest`,
  `DeviceWriteRequest`, `ServerDriveSetInformationRequest`,
  `ServerDriveLockControlRequest`. Rust match exhaustiveness required handling
  all 11 in `handle_drive_io_request`; the 4 the plan asked for
  (Create/Close/Read/QueryDirectory) are fully implemented, the other 7 are
  rejected with a typed `NtStatus::NOT_SUPPORTED` `DeviceCloseResponse` —
  mirroring the crate's own `RdpdrBackend::handle_printer_io_request` default
  behavior (same fallback-PDU pattern, verified in `backend/mod.rs`).
- **`RdpdrBackend` trait signature** (`backend/mod.rs`): `handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>>` (an `SVC` message, not `DvcMessage` — RDPDR is a static virtual channel, unlike Phase 4's `RDPILOT_SENSOR` DVC). `handle_server_device_announce_response` and `handle_scard_call` return `PduResult<()>`; implemented as `Ok(())` no-ops per RESEARCH Code Examples.
- **`NtStatus` values available** (`efs.rs`, `impl NtStatus`): `SUCCESS`, `UNSUCCESSFUL`, `NOT_IMPLEMENTED`, `NO_MORE_FILES`, `OBJECT_NAME_COLLISION`, `ACCESS_DENIED`, `NOT_A_DIRECTORY`, `NO_SUCH_FILE`, `NOT_SUPPORTED`, `DIRECTORY_NOT_EMPTY`. **There is no `OBJECT_NAME_NOT_FOUND` constant** — the correct not-found status for a rejected `Create` path is `NtStatus::NO_SUCH_FILE` (`0xC000_000F`), used throughout this implementation.
- **Response-PDU field-level shapes used:**
  - `DeviceIoResponse { device_id, completion_id, io_status }`, built via `DeviceIoResponse::new(device_io_request, status)` (echoes `device_id`/`completion_id` from the inbound IRP).
  - `DeviceCreateResponse { device_io_reply, file_id, information }` — `information: Information::FILE_OPENED` on success, `Information::FILE_SUPERSEDED` (bit-value `0`) on rejection (the field is not meaningfully read by the peer when `io_status != SUCCESS`, but must still be a valid value).
  - `DeviceCloseResponse { device_io_response }`.
  - `DeviceReadResponse { device_io_reply, read_data: Vec<u8> }`.
  - `ClientDriveQueryDirectoryResponse { device_io_reply, buffer: Option<FileInformationClass> }` — `buffer: None` on `NO_MORE_FILES`; on the first query, `Some(FileInformationClass::{BothDirectory,FullDirectory,Directory,Names})` built from the requested `FileInformationClassLevel` via each struct's own `::new(creation_time, last_access_time, last_write_time, change_time, file_size, file_attributes, file_name)` constructor (times hardcoded to `0`, `file_attributes: FileAttributes::FILE_ATTRIBUTE_NORMAL`, `file_size` from `std::fs::metadata`).
  - `RdpdrPdu::encode` (`pdu/mod.rs`) writes a 4-byte `SharedHeader` (`component: u16`, `packet_id: u16`) **before** the inner response body — every `SvcMessage::encode_unframed_pdu()` output in this crate's own responses starts with those 4 bytes, then the `DeviceIoResponse` 12-byte prefix. This tripped up the test-only byte decoder on the first run (see Issues Encountered) and is now documented directly in the decoder's doc comment for future test authors.

## Decisions Made

See frontmatter `key-decisions` for the full list. Summary: the package-legitimacy checkpoint was pre-approved by the user (recorded, not re-asked); the plan's assumed 4-variant `ServerDriveIoRequest` enum is actually 11 variants (handled exhaustively, 7 typed-rejected); the correct not-found `NtStatus` is `NO_SUCH_FILE` (not `OBJECT_NAME_NOT_FOUND`, which does not exist in this crate); a foreign/unopened `file_id` on `Read` is rejected with `ACCESS_DENIED` (structurally distinct from `UNSUCCESSFUL` IO failures); `QueryDirectory`'s `InitialQuery` byte alone (no extra state) drives the single-entry-then-`NO_MORE_FILES` enumeration.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `ServerDriveIoRequest` has 11 variants, not the plan's assumed 4**
- **Found during:** Task 3 `read_first` of `efs.rs`
- **Issue:** The plan's `<action>` text described "the `ServerDriveIoRequest` enum's exactly-four variants"; the actual enum (verified by reading the pinned `ironrdp-rdpdr-0.6.0` source) has 11. Rust match exhaustiveness would not compile with only 4 arms.
- **Fix:** Implemented the 4 planned variants (Create/Close/Read/QueryDirectory) fully; added the remaining 7 as explicit match arms each calling a shared `reject_unsupported()` helper that returns a typed `NtStatus::NOT_SUPPORTED` `DeviceCloseResponse` — mirroring the crate's own `handle_printer_io_request` default fallback pattern (verified in `backend/mod.rs`), not an invented convention.
- **Files modified:** `crates/rdpilot/src/rdpdr_backend.rs`
- **Verification:** `cargo build -p rdpilot` compiles (exhaustive match, no wildcard `_` arm needed); `cargo test -p rdpilot rdpdr_backend::` green
- **Committed in:** `f383313` (Task 3 GREEN commit)

**2. [Rule 1 - Bug] Plan's `<verify>` grep for `ironrdp-rdpdr-native` absence would have false-positived on my own explanatory comment**
- **Found during:** Task 2, immediately after adding the dependency line
- **Issue:** My first Cargo.toml comment explaining why `ironrdp-rdpdr-native` was NOT added contained the literal substring `ironrdp-rdpdr-native`, which the plan's blunt `grep -c 'ironrdp-rdpdr-native' Cargo.toml` verify check would have counted as a match, failing the acceptance criterion `ironrdp-rdpdr-native` is absent.
- **Fix:** Reworded the comment to reference "the empty `-native` sibling crate" instead of spelling out the literal dependency name.
- **Files modified:** `crates/rdpilot/Cargo.toml`
- **Verification:** `grep -c 'ironrdp-rdpdr-native' crates/rdpilot/Cargo.toml` returns `0`
- **Committed in:** `c077aea` (Task 2 GREEN commit)

**3. [Rule 1 - Bug] Test-only byte decoder initially mis-parsed the `RdpdrPdu` `SharedHeader` prefix**
- **Found during:** Task 3, first test run after the GREEN implementation compiled
- **Issue:** `RdpdrPdu::encode` writes a 4-byte `SharedHeader` (component/packet_id) before the inner response body; the first version of the test-only byte decoder assumed the `DeviceIoResponse` prefix started at byte 0, causing 3 of 4 tests to fail with `io_status` decoding as the wrong field's bytes (observed as `NtStatus(0x2A)` — actually the test's own `completion_id` value, read 4 bytes early).
- **Fix:** Skip the 4-byte `SharedHeader` before decoding `DeviceIoResponse`, documented inline with the `RdpdrPdu::encode` source citation.
- **Files modified:** `crates/rdpilot/src/rdpdr_backend.rs` (test module only)
- **Verification:** All 4 `rdpdr_backend::` tests pass
- **Committed in:** `f383313` (Task 3 GREEN commit, fixed before commit — not a separate commit)

---

**Total deviations:** 3 auto-fixed (1 blocking enum-shape correction, 1 verify-script self-collision bug, 1 test-decoder bug)
**Impact on plan:** All three were necessary for correctness and for the plan's own acceptance criteria to pass as written. No scope creep — the 7 rejected IRP variants remain genuinely unimplemented (typed `NOT_SUPPORTED`, not silently accepted), matching the plan's stated "minimal backend" scope.

## Issues Encountered

- Two Rust string-literal escape-sequence authoring mistakes (single vs. double backslash in `\served.bin`/`\..\..\Windows\...` path literals) were introduced while drafting test code via an intermediate Python heredoc and caught immediately by the compiler (`error: unknown character escape`) before any commit — fixed inline, not a runtime bug, no separate deviation entry warranted.
- This sandbox is a native Fedora Linux VM, not the ARM64-Windows/scoop host `rust-toolchain.toml`/`.cargo/config.toml` target (established pattern from Phases 2-4). All verification in this plan used `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo {build,test,clippy} -p rdpilot --target x86_64-unknown-linux-gnu` — a session-local environment override; no committed toolchain file was touched. `cargo fmt`/`rustfmt` is not installed for the Linux toolchain in this sandbox (`rustup component add rustfmt` would be needed); code was hand-formatted to match the existing 4-space/module conventions but not machine-verified against `rustfmt.toml` in this session.

## Known Stubs

- `RdpilotDriveBackend` is currently constructed only by its own unit tests — `dead_code`/`unused` warnings on a plain `cargo build` are expected and will disappear once Plan 03 wires it into the connect path (`with_static_channel`) and the launch bootstrap, exactly as Phase 3's `input.rs` translation layer was flagged and later resolved. Not a stub in the sense of missing functionality: this plan's entire purpose was to build this backend as pure, standalone, offline-tested code, consumed by a later plan.

## Threat Flags

None beyond what the plan's own `<threat_model>` already covers (T-05-04, T-05-05, T-05-SC) — no new network endpoints, auth paths, or trust-boundary schema changes were introduced beyond the RDPDR IRP surface the plan explicitly scoped.

## User Setup Required

None — this plan is entirely offline Rust work; no external service configuration required.

## Next Phase Readiness

- Plan 03 can now wire `RdpilotDriveBackend::new(served_path, served_name)` into the connect path (registering it as the RDPDR static-channel processor) and use `Key::Win` + `Key::R` in a `KeyAction::Combo` to build the D-5.1 launch sequence.
- No blockers. `cargo build -p rdpilot` and `cargo test -p rdpilot` both exit 0 offline (63 unit/integration tests pass, 11 live tests correctly `#[ignore]`d); `ironrdp-rdpdr-native` confirmed absent from `Cargo.toml`.
- One open item carried forward (not a blocker for Plan 03, but worth noting for whoever wires the backend in): `RdpilotDriveBackend`'s `Create` handler currently does not inspect `create_options`/`desired_access` (e.g. `FILE_DIRECTORY_FILE`) when distinguishing a root-directory open from a file open — it infers this purely from the normalized path being empty vs. matching the served filename. This was sufficient for the offline unit tests and matches the plan's minimal scope, but Plan 03's live gate should confirm a real Windows RDP client's actual `Create` sequence against the redirected drive root behaves as expected.

## Self-Check: PASSED

All claimed files exist on disk (`crates/rdpilot/src/rdpdr_backend.rs`, `crates/rdpilot/Cargo.toml`, `crates/rdpilot/src/input.rs`, `crates/rdpilot/src/lib.rs`, this SUMMARY.md) and all 4 claimed commit hashes (`95f31e4`, `c077aea`, `7355f36`, `f383313`) are present in `git log --oneline --all`.

---
*Phase: 05-sensor-bootstrap-deployment*
*Completed: 2026-07-09*
