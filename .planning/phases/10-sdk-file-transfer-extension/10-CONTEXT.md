# Phase 10: SDK File-Transfer Extension - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

The SDK can move files in both directions between local disk and the remote target through the already-proven RDPDR channel plus new sensor-mediated commands — safely, and without touching `Session`'s threading model.

**Depends on:** Nothing new (extends the v1.0 SDK: `RdpilotDriveBackend`, the DVC sensor protocol).
**Requirements:** FILE-01, FILE-02, FILE-03, FILE-04.

**Success Criteria** (from ROADMAP.md, verbatim):
1. A local file uploads to a named remote destination (`Session::upload_file`) and is verified present on the target (FILE-01).
2. A remote file downloads to a named local destination (`Session::download_file`) with matching size/checksum (FILE-02).
3. **[BLOCKING]** An adversarial path-traversal test suite — trailing `..` with no separator, mixed `/`/`\` separators, and absolute-path-as-relative inputs — is rejected by full canonicalized ancestry validation under the share root (never substring matching), with no `Write`/`Create` IRP escaping the share root (FILE-03; Pitfall 8 / FreeRDP GHSA-3xpj-m4hx-8vmx / CVE-2025-48817).
4. A file larger than one MS-RDPEFS per-IRP chunk transfers correctly (the chunked read/write loop actually loops), and an interrupted transfer surfaces a clean, detectable failure (staged-and-renamed) rather than silent corruption (FILE-04).

**Out of scope:** CLIPRDR-based transfer (deferred clipboard feature); the wire protocol crate for daemon/CLI/MCP put-get verbs (Phase 11+); the CLI `put`/`get` commands themselves (Phase 13); the MCP put/get tool (Phase 14). This phase only extends the SDK/library layer.

</domain>

<decisions>
## Implementation Decisions

### Inherited / Already-Decided (carried forward, do not re-open)

- **D-20 (PROJECT.md):** Bidirectional file transfer generalizes the existing RDPDR path, NOT CLIPRDR. RDPDR already deploys the sensor; CLIPRDR is clipboard-shaped and reserved for the deferred clipboard feature.
- **Hybrid mechanism is structurally required, not a preference:** RDPDR channel carries the file bytes; a NEW sensor-mediated trigger command drives each transfer. RDPDR IRPs are server(Windows)-initiated only — there is no way to make Windows open/write/read a file on demand without something on the remote side telling it to. A sensor trigger is mandatory regardless of which mechanism might otherwise be preferred.
- **`Session`'s threading model is UNTOUCHABLE:** dedicated OS thread + current-thread Tokio runtime, never `tokio::spawn` (`session.rs` ~line 206-210, documented rationale: the reactivation step holds a `!Send` type across an await point). The new file command rides the existing async `sensor_request()` extension point (`session.rs` ~line 508), exactly like `Uia`/`LaunchProcess` were added in Phases 6/7. This requires zero sensor-framework change beyond a new `MsgType` enum variant (Rust `sensor.rs`) + a new C# dispatch arm (`sensor/Program.cs`).
- **Security posture carries verbatim from T-05-04 (ASVS V4):** hard allow-list, never substring match, reject before touching `std::fs`.
- **API-01 no-panic discipline:** no `unwrap`/`expect`/`panic` in Rust library or C# sensor code; typed `Result`/`NtStatus` everywhere. Enforced today via `lib.rs` inner `#![deny(unsafe_code)]` / `clippy::unwrap_used` / `clippy::expect_used` gates (08-01) — this phase's new code must satisfy the same gates.
- **D-09 crate-internal-only:** no `ironrdp-rdpdr` types leak into the public API; new public methods expose only owned SDK types.

### File-Transfer Architecture

- **D-10.1 — Share-root generalization: SINGLE configured share root.**
  Generalize `OpenEntry::File` (in `RdpilotDriveBackend`, `rdpdr_backend.rs`) to carry a resolved `PathBuf` under one configured share root; canonicalize + ancestry-check each resolved path. Reuse the existing per-connection RDPDR registration (`connect.rs` line ~127-131: `Rdpdr::new(Box::new(drive_backend), "rdpilot".to_owned()).with_drives(Some(vec![(0, "RDPILOT".to_owned())]))`, gated behind `cfg.get_sensor_binary_path().is_some()`); special-case the sensor exe name (`SENSOR_EXE_NAME` = `"rdpilot-sensor.exe"`, `connect.rs` line 64) inside the SAME backend. Add a share-root builder field on `ConnectionConfig` (precedent: `sensor_binary_path()` / `get_sensor_binary_path()`, `config.rs` lines 129-183).
  **Rejected:** second-drive registration (duplicates channel management); per-transfer ephemeral backend (conflicts with static-channel registration timing — the drive is registered once at connect time, not per-transfer).

- **D-10.2 — Canonicalization location: BOTH SIDES (symmetric defense-in-depth).**
  Rust `RdpilotDriveBackend` validates every RDPDR-supplied path against the LOCAL share root using `std::fs::canonicalize` + `Path` ancestry check. C# sensor validates every destination path against the REMOTE share root using `Path.GetFullPath` + StartsWith-on-canonical-form (NEVER `Contains("..")`). The FILE-03 BLOCKING adversarial suite (trailing `..` with no separator, mixed `/`+`\` separators, absolute-path-as-relative) runs against BOTH validators independently.
  **Rationale:** the hybrid mechanism crosses two trust boundaries — remote Windows OS → Rust backend (protects local disk) and Rust SDK → C# sensor (protects remote disk). Single-sided validation leaves one direction attacker/bug-controlled.

- **D-10.3 — Chunking + write path: STAGED TEMP FILE + ATOMIC RENAME.**
  Implement the currently-`NOT_SUPPORTED` `DeviceWriteRequest` (`rdpdr_backend.rs` line 507) and `ServerDriveSetInformationRequest` (`rdpdr_backend.rs` line 508, for end-of-file/size) in `RdpilotDriveBackend`. Offset-keyed writes go into `<share_root>/.rdpilot-staging/<uuid>.part` per IRP (seek+write, bounded per-IRP — never buffer whole file in memory, preserving the T-05-05 bounded-IO discipline and avoiding unbounded-alloc DoS). Atomic rename to the final destination only on a clean `Close` after all bytes are accounted for.
  FILE-04's "interrupted transfer surfaces a clean, detectable failure" comes free from this design: an interrupted transfer never renames, leaving a stale `.part` in staging.
  Rely on the remote Windows OS issuing sequential multi-offset Read/Write IRPs for the "large file / loop actually loops" behavior (confirm the per-IRP chunk bound during research — flagged as an open item below).

- **D-10.4 — Public API shape + error taxonomy: TWO NAMED METHODS.**
  ```
  Session::upload_file(&self, local: &Path, remote_name: &str) -> Result<()>
  ```
  (or returning a `TransferOutcome` — planner may refine, keep symmetric with download)
  ```
  Session::download_file(&self, remote_name: &str, local: &Path) -> Result<TransferOutcome>
  // TransferOutcome { bytes_transferred, checksum }
  ```
  Matches the roadmap's own `Session::upload_file`/`download_file` naming and the downstream CLI `put`/`get` + MCP `put`/`get` verbs (Phase 13/14).
  New narrow `Error` variants: `Error::PathTraversal(String)` and `Error::ChecksumMismatch { expected, actual }`, DISTINCT from `Error::SensorRejected`/`Error::Dvc` (`error.rs` lines 91, 106) so a path rejection is never conflated with a transient transport failure (CLI-03, one phase later, requires "distinct, legible errors"). Follow the existing `error.rs` Display/category pattern (precedent: `Error::CoordinateOutOfBounds`, line 76; category strings at line ~187-190; constructors like `Error::dvc()`/`Error::sensor_rejected()` at lines 153/168).

- **D-10.5 — Checksum: SHA-256 both sides.**
  Add the `sha2` crate to `crates/rdpilot/Cargo.toml` (Rust); use BCL `System.Security.Cryptography.SHA256` in C# (zero new NuGet dependency, NativeAOT-safe — matches the no-COM/no-third-party-AOT-risk discipline established in Phases 5/6/7). Used to satisfy FILE-02's size/checksum verification.
  **Rejected:** BLAKE3 (unvetted C# AOT NuGet); CRC32/xxHash (non-cryptographic, inappropriate for a security-adjacent transfer).

### Claude's Discretion

- Exact `Result<()>` vs `Result<TransferOutcome>` return type for `upload_file` — keep symmetric with `download_file`, planner may refine.
- Internal naming/shape of the new `MsgType` variant(s) for the sensor-mediated trigger (e.g., a single `FileTransfer` variant with an upload/download discriminant in the payload, vs. two separate variants) — follow the existing `sensor.rs` envelope pattern (`{version, req_id, type, payload}`) and Program.cs dispatch style (`else if (msg.Type == MsgType.X) { ... BuildXReplyEnvelope(...) }`).
- Exact staging directory cleanup policy for orphaned `.part` files from prior interrupted transfers (not a stated success criterion; researcher/planner may propose a simple approach, e.g. best-effort cleanup on next connect).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Roadmap / Requirements / Decisions
- `.planning/ROADMAP.md` §"Phase 10: SDK File-Transfer Extension" (lines 40-49) — goal, success criteria, requirement IDs
- `.planning/REQUIREMENTS.md` FILE-01, FILE-02, FILE-03, FILE-04 (lines 41-44, 85-88)
- `.planning/PROJECT.md` D-20 (line 109) — RDPDR-not-CLIPRDR decision and rationale
- `.planning/STATE.md` Phase 5 section (line 137) — RDPDR live-gate history: the two previously-`NOT_SUPPORTED` IRPs (`QueryInformation`/`QueryVolumeInformation`/`QueryDirectory`) that were implemented in Phase 5, `rdpsnd` join-only stub requirement, and other RDPDR channel timing findings relevant to extending this backend

### Security precedent
- T-05-04 (ASVS V4 allow-list posture) — referenced in `RdpilotDriveBackend` doc comments (`rdpdr_backend.rs`)
- T-05-05 (bounded-IO discipline) — applies to the new staged-write path
- Pitfall 8 / FreeRDP GHSA-3xpj-m4hx-8vmx / CVE-2025-48817 — the path-traversal CVE class this phase's FILE-03 blocking criterion directly guards against

### API/error conventions
- `crates/rdpilot/src/error.rs` — existing `Error` enum, Display/category pattern, constructor style (`dvc()`, `sensor_rejected()`, `coordinate_out_of_bounds()`)

No further external specs — requirements fully captured in decisions above.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets

- **`crates/rdpilot/src/rdpdr_backend.rs`** — `RdpilotDriveBackend` struct (fields `served_path: PathBuf`, `served_name: String`, ~lines 68-70). 5 of 11 `ServerDriveIoRequest` variants implemented; `DeviceWriteRequest` (line 507) and `ServerDriveSetInformationRequest` (line 508) are currently typed-`NOT_SUPPORTED` via `reject_unsupported()` (line 468-474). Hardcoded single `served_path`/`served_name` — THE file to generalize for a configurable share root + staged writes.
- **`crates/rdpilot/src/sensor.rs`** — `RdpilotSensorProcessor`/`SensorShared`; generic `{version, req_id, type, payload}` envelope, `req_id`-keyed fulfilment. A new `MsgType` variant is a zero-framework-change addition (same pattern as `Uia`/`LaunchProcess`).
- **`crates/rdpilot/src/session.rs`** — `sensor_request()` async helper (~line 508) is the extension point every new sensor-mediated call rides (see `get_window_list`/`get_process_tree`/`get_uia_tree`/`set_foreground_window`/`launch_process` at lines 586, 611, 656-657, 791-792, 820-821 for the calling convention). Threading model (dedicated OS thread + current-thread Tokio, ~line 206-210) is UNTOUCHABLE — do not add `tokio::spawn`.
- **`crates/rdpilot/src/connect.rs`** — `RDPILOT_SENSOR` (line 58) / `SENSOR_EXE_NAME` (line 64) consts; connect-time RDPDR registration gated on `cfg.get_sensor_binary_path()` (lines 127-131), drive named `"RDPILOT"`. File transfer piggybacks on this SAME registration — no new gating needed since sensor deploy is already a hard prerequisite for any sensor-mediated feature.
- **`crates/rdpilot/src/config.rs`** — `ConnectionConfig` builder; `sensor_binary_path()` (line 129) / `get_sensor_binary_path()` (line 182) is the exact precedent pattern for adding a new share-root builder field.
- **`crates/rdpilot/src/error.rs`** — `Error` enum (line 15); add `PathTraversal(String)` and `ChecksumMismatch { expected, actual }` here, following the `CoordinateOutOfBounds` (line 76) / `Dvc` (line 91) / `SensorRejected` (line 106) pattern including category strings (~lines 187-190) and `pub(crate)` constructors (lines 153, 168).
- **`sensor/Program.cs`** (+ existing handler files, e.g. `ProcessLaunch.cs`) — dispatch pattern is a flat `else if (msg.Type == MsgType.X) { WriteEnvelope(handle, BuildXReplyEnvelope(msg.ReqId, msg.Payload)); }` chain (lines 338-365), with each `BuildXReplyEnvelope` following: deserialize DTO → handler → catch-all try/catch → `Success = false` on any exception, NEVER throw out of the dispatch (see `BuildLaunchProcessReplyEnvelope`, lines 475-499, and `BuildUiaTreeReplyEnvelope`-equivalent at ~line 501+). Template for the new file-transfer sensor handler.

### Established Patterns

- Sensor round-trip calling convention: `self.sensor_request(crate::sensor::MsgType::X, Some(payload), TIMEOUT_MS).await` → typed success/data vs `success:false` → `Error::SensorRejected` branching (D-6.4 precedent).
- No COM/no third-party NativeAOT-risky NuGet in the sensor — BCL-only where possible (`System.Security.Cryptography.SHA256` satisfies this for D-10.5).
- `[LibraryImport]`-only P/Invoke surface in the sensor where Win32 calls are needed (not relevant unless the file-transfer trigger needs any Win32 API beyond `System.IO`).

### Integration Points

- New `MsgType` variant in `sensor.rs` (Rust) + matching C# enum value + new dispatch arm in `Program.cs`.
- New `ConnectionConfig` share-root builder method in `config.rs`, consumed by `connect.rs`'s existing `RdpilotDriveBackend::new(...)` call site (currently `RdpilotDriveBackend::new(sensor_path.to_path_buf(), SENSOR_EXE_NAME)`, line 129 — will need a third argument or restructuring to also carry the share root).
- New public `Session::upload_file`/`download_file` methods in `session.rs`, alongside the existing public perception/input/launch methods.

</code_context>

<specifics>
## Specific Ideas

No UI/UX specifics — this is a pure SDK/library API extension. The public method signatures in D-10.4 (`Session::upload_file`, `Session::download_file`) are the concrete "shape" the user specified, matching the roadmap's own naming and anticipating the Phase 13 CLI `put`/`get` verbs and Phase 14 MCP `put`/`get` tool.

</specifics>

<deferred>
## Deferred Ideas

- CLIPRDR-based transfer / clipboard file copy-paste — explicitly out of scope per D-20; reserved for a future deferred clipboard feature.
- Wire protocol types for daemon/CLI/MCP file-transfer requests — belongs to Phase 11 (`rdpilot-ipc`), which ROADMAP.md notes "must cover the file-transfer request/response variants" defined here.
- CLI `put`/`get` commands and MCP `put`/`get` tool — Phase 13 and Phase 14 respectively; this phase only builds the SDK methods they will call.

### Open Research Items (not user decisions — flagged for gsd-phase-researcher)

1. **Confirm the MS-RDPEFS per-IRP max read/write chunk size** against the pinned `ironrdp-rdpdr-0.6.0` source/spec — do not assume a number. FILE-04's "larger than one chunk" test depends on knowing what one chunk actually is.
2. **Confirm the RDPDR channel stays registered only when `cfg.get_sensor_binary_path()` is `Some`** — file transfer piggybacks on the same connect-time registration; sensor deploy is a hard prerequisite anyway, so no new gating should be needed, but confirm no edge case breaks this assumption.
3. **Determine how the C# sensor reads/writes remote-side bytes for the sensor-triggered legs** and how the remote path references the RDPDR-redirected `RDPILOT` drive (precedent: `session.rs`'s `deploy_and_launch`/`launch_command`, though post-bootstrap transfers should use sensor DVC commands, not injected keystrokes).

**Reviewed Todos (not folded):** None — no pending todos in STATE.md overlap this phase's scope.

</deferred>

---

*Phase: 10-SDK File-Transfer Extension*
*Context gathered: 2026-07-10*
