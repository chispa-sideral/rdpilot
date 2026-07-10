---
phase: 10-sdk-file-transfer-extension
plan: 03
subsystem: sensor
tags: [csharp, rust, sensor-protocol, path-traversal, sha256, nativeaot, dispatch]

# Dependency graph
requires:
  - phase: 10-sdk-file-transfer-extension
    plan: 01
    provides: "Error::PathTraversal(String)/Error::ChecksumMismatch{expected,actual} variants (error.rs), share-root generalized RdpilotDriveBackend, resolve_under_root/looks_rooted validator pattern (the Rust-side half of D-10.2's symmetric defense-in-depth), sha2 promoted to a direct dependency"
provides:
  - "MsgType::FileTransfer wire variant, byte-identical (same ordinal position, bare-string serialization) on Rust sensor.rs and C# Envelope.cs"
  - "sensor/FileTransfer.cs: FileTransferOp/FileTransferRequest/FileTransferData/FileTransferResponse DTOs, ValidateRemotePath (GetRelativePath + host-independent LooksRooted ancestry check), Transfer (single-pass FileStream copy + IncrementalHash SHA-256)"
  - "Program.cs MsgType.FileTransfer dispatch arm + BuildFileTransferReplyEnvelope (never-throws-out-of-dispatch template)"
  - "The caller-facing error_kind=\"path_traversal\" producer (D-10.4 BLOCKER fix): FileTransferResponse.ErrorKind, a plain nullable string sentinel (\"path_traversal\"|\"io\"|null), wired end-to-end from ValidateRemotePath's rejection through BuildFileTransferReplyEnvelope, ready for Plan 10-04's sensor_request caller to map to Error::PathTraversal"
  - "FILE-03 BLOCKING C#-side adversarial selftest (--file-traversal-selftest Main arg), 6 cases, all passing offline"
affects: [10-04-session-api-and-checksum-verification, 10-05-live-gate]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Single MsgType::FileTransfer variant with an Op (Upload/Download) discriminant in the request payload, not two separate variants (10-CONTEXT Claude's Discretion) — reuses the existing generic req_id-keyed fulfilment path with zero dispatch/correlation framework change"
    - "C#-side error_kind is a plain nullable string sentinel (ErrorKind: string?), not a [JsonStringEnumConverter] enum — chosen so the wire value is literally \"path_traversal\"/\"io\" (snake_case, matching the intended Rust-side Error::PathTraversal mapping) rather than the PascalCase a typed enum would emit under the project's existing UseStringEnumConverter=true convention"
    - "LooksRooted() host-independent pre-check (leading separator OR ASCII-letter+colon drive prefix) run BEFORE any Path/GetFullPath call — the C#-side mirror of 10-01-SUMMARY.md's Rust looks_rooted() fix; Path.IsPathRooted alone only recognizes drive letters on an actual Windows runtime, so an offline linux-x64 host would otherwise silently nest a Windows-drive-absolute candidate under the share root instead of rejecting it"
    - "ValidateRemotePath treats ANY rooted-looking candidate as invalid outright (no legitimate remote_path is ever expected to be absolute) — the sensor's remote-side I/O is walled off to a single sensor-owned transfer root, symmetric with the Rust side's own D-10.1 share-root confinement, not a general-purpose arbitrary-path API"

key-files:
  created:
    - sensor/FileTransfer.cs
  modified:
    - crates/rdpilot/src/sensor.rs
    - sensor/Envelope.cs
    - sensor/EnvelopeJsonContext.cs
    - sensor/Program.cs

key-decisions:
  - "error_kind shape: plain nullable string sentinel (FileTransferResponse.ErrorKind, wire name error_kind), not a typed FileTransferErrorKind enum. The plan offered both options; a typed enum under this project's existing [JsonSourceGenerationOptions(UseStringEnumConverter = true)] convention would serialize as PascalCase (\"PathTraversal\"/\"Io\"), whereas the plan's own repeated literal wording (\"error_kind = \\\"path_traversal\\\"\") and the intended snake_case symmetry with the eventual Rust-side Error::PathTraversal mapping (Plan 10-04) both point to a literal snake_case string value. A plain string avoids introducing a second enum-naming convention divergence in the same file."
  - "Sensor-side fixed transfer root: Path.Combine(Path.GetTempPath(), \"rdpilot-transfer-root\") -- a directory under the current user's %TEMP%, created best-effort (Directory.CreateDirectory, idempotent) on every Transfer() call. This is a FIXED CONSTANT, not caller-configurable (WARNING-1 fold-in per the plan's <output> instruction): allowing the Rust SDK caller to dictate the root the validator checks the destination against would let an attacker/bug on that side simply supply a root that already contains the traversal target, defeating the entire point of \"validate under a root\". Phase 13 (CLI put/get) and Phase 14 (MCP put/get) must treat this sensor-side root as opaque -- callers pass a relative remote_path (a name/subpath under this root), never an absolute real-machine path elsewhere on the target disk. This is a narrower remote-write surface than a naive reading of D-10.4's Session::upload_file/download_file API might suggest; Plan 10-04 should document this constraint on the public API doc comments."
  - "Two-validator reason-mapping (BLOCKER fix, D-10.4, both halves): the C# FileTransfer.ValidateRemotePath rejection here is the SOLE authoritative producer of error_kind=\"path_traversal\" -- it is the sensor's own remote-destination validator rejecting untrusted input before any System.IO call. The Rust-side RdpilotDriveBackend::resolve_under_root rejection (Plan 10-01) protects the OTHER trust boundary (the local share root under the RDPDR-redirected drive) and already returns a structurally-distinct NtStatus::NO_SUCH_FILE at the IRP level; if that rejection fires during a transfer, it manifests here only as an ordinary FileStream open failure against \\\\tsclient\\RDPILOT\\<name>, reported as error_kind=\"io\" -- never a false path_traversal, so the two independent validators' rejections are never conflated."
  - "LooksRooted() explicit pre-check added beyond the plan's literal Path.GetRelativePath+rooted-result-check description -- necessary for the offline Windows-drive-absolute FILE-03 case to be deterministic on both this linux-x64 host and the real win-x64 target (Path.IsPathRooted alone only recognizes drive letters on an actual Windows runtime). This exactly mirrors the Rust-side looks_rooted() fix documented as Decision #1 in 10-01-SUMMARY.md -- not a new design, a direct C#-side application of the same already-established, already-proven-necessary fix."

requirements-completed: [FILE-01, FILE-02, FILE-03]

# Metrics
duration: ~35min
completed: 2026-07-10
---

# Phase 10 Plan 03: Sensor + C# Half of File Transfer Summary

**A single `MsgType::FileTransfer` wire variant rides the existing sensor extension point end-to-end: the C# sensor validates every destination path with a `Path.GetRelativePath` + host-independent rooted-check ancestry validator (never `StartsWith`), copies bytes with inline SHA-256, and surfaces a path-traversal rejection through a structured `error_kind="path_traversal"` field distinct from any other failure — the FILE-03 C# adversarial selftest proves all six required cases offline.**

## Performance

- **Duration:** ~35 min
- **Completed:** 2026-07-10
- **Tasks:** 3/3
- **Files modified:** 5 (1 created, 4 modified)

## Accomplishments

- `MsgType::FileTransfer` added as the last variant in both `crates/rdpilot/src/sensor.rs` (Rust) and `sensor/Envelope.cs` (C#), same ordinal position; a Rust unit test asserts the bare-string wire form `"FileTransfer"` (13 sensor:: tests pass, 112 offline `cargo test -p rdpilot` tests pass overall)
- `sensor/FileTransfer.cs` created: `FileTransferOp` (Upload/Download discriminant), `FileTransferRequest`/`FileTransferData`/`FileTransferResponse` DTOs (all registered in `EnvelopeJsonContext` for NativeAOT source-gen, zero reflection fallback), `ValidateRemotePath` (the C# half of D-10.2's symmetric defense-in-depth), and `Transfer` (single-pass `FileStream` copy with `IncrementalHash(SHA256)` fed inline, direction-aware between the validated real remote path and the RDPDR-redirected `\\tsclient\RDPILOT\<name>` UNC path)
- `Program.cs` gains the `MsgType.FileTransfer` dispatch arm and `BuildFileTransferReplyEnvelope`, following the exact `BuildLaunchProcessReplyEnvelope`/`BuildUiaTreeReplyEnvelope` never-throw-out-of-dispatch template (deserialize → handler → catch-all try/catch → `success:false`)
- The BLOCKER-fix `error_kind` field is wired end-to-end: `ValidateRemotePath` rejection → `Transfer` sets `ErrorKind = "path_traversal"` → `BuildFileTransferReplyEnvelope` propagates it verbatim onto the wire — ready for Plan 10-04's `sensor_request` caller to branch on distinctly from `Error::Dvc`/`Error::SensorRejected`
- The FILE-03 BLOCKING C# adversarial selftest (`--file-traversal-selftest` Main arg, `RunFileTraversalSelfTest`) covers all 6 required cases and passes 6/6 offline

## Task Commits

Each task was committed atomically:

1. **Task 1: Wire the FileTransfer message type (Rust sensor.rs + C# Envelope.cs + DTOs)** - `6ea71b1` (feat)
2. **Task 2: C# FileTransfer handler — path validator + FileStream copy-with-inline-SHA256** - `4962fdd` (feat)
3. **Task 3: FILE-03 BLOCKING adversarial selftest (C# validator, offline-runnable)** - `9fb86e8` (test)

## Files Created/Modified

- `crates/rdpilot/src/sensor.rs` - `MsgType::FileTransfer` variant (last position), doc-comment update, `file_transfer_envelope_serializes_with_bare_type_string` unit test
- `sensor/Envelope.cs` - `MsgType.FileTransfer` enum member (same ordinal position as the Rust side)
- `sensor/FileTransfer.cs` (new) - DTOs (`FileTransferOp`/`FileTransferRequest`/`FileTransferData`/`FileTransferResponse`), `ValidateRemotePath`, `Transfer`, `ShareRoot`/`EnsureShareRootExists`/`NormalizeSeparators`/`LooksRooted` helpers
- `sensor/EnvelopeJsonContext.cs` - registers `FileTransferRequest`/`FileTransferData`/`FileTransferResponse` with `[JsonSerializable]`
- `sensor/Program.cs` - `MsgType.FileTransfer` dispatch arm, `BuildFileTransferReplyEnvelope`, `--file-traversal-selftest` Main-arg hook + `RunFileTraversalSelfTest`

## The `error_kind` Shape (for Plan 10-04)

`FileTransferResponse.ErrorKind` is `string?` (wire name `error_kind`), NOT a `[JsonStringEnumConverter]`-backed enum. Values on the wire: `"path_traversal"` (a `ValidateRemotePath` rejection), `"io"` (any copy/IO failure, including a missing/malformed request payload or an exception escaping the handler), or absent/`null` on success. Plan 10-04's `sensor_request` caller should read this field out of the raw `serde_json::Value` reply and map `"path_traversal"` → `Error::PathTraversal`, anything else on failure → the existing `Error::SensorRejected`/`Error::Dvc` path.

## Two-Validator Reason-Mapping

| Validator | Trust boundary | Rejection surfaces as |
|---|---|---|
| C# `FileTransfer.ValidateRemotePath` (this plan) | Rust SDK → C# sensor's remote destination path | `error_kind = "path_traversal"` (authoritative caller-facing producer) |
| Rust `RdpilotDriveBackend::resolve_under_root` (Plan 10-01) | Remote Windows OS → local share root (RDPDR IRP level) | `NtStatus::NO_SUCH_FILE`; if it fires *during* a transfer, this manifests to the C# sensor only as an ordinary `FileStream` open failure against `\\tsclient\RDPILOT\<name>`, reported as `error_kind = "io"` — never a false `path_traversal` |

## Sensor-Side Transfer Root: Fixed Constant, Not Caller-Configurable

`FileTransfer.ShareRoot = Path.Combine(Path.GetTempPath(), "rdpilot-transfer-root")` — the current user's `%TEMP%` plus a fixed subdirectory name, created best-effort on every `Transfer()` call. This is deliberately a **fixed constant**, never something a `FileTransferRequest.RemotePath` value can influence: allowing the far-side (Rust SDK) caller to dictate the root the validator checks the destination against would let an attacker/bug on that side simply supply a root that already contains the traversal target, defeating the entire point of validating "under a root" in the first place. Consequently, **every legitimate `RemotePath` value is expected to be relative** (a name/subpath under this fixed root) — `ValidateRemotePath`'s `LooksRooted` pre-check rejects any candidate that looks absolute (POSIX-leading-separator or Windows-drive-letter-prefixed) outright, before any `Path.GetFullPath`/`GetRelativePath` computation. **Phases 13/14 (CLI/MCP `put`/`get`) must treat this as a real API constraint**: the sensor cannot write/read an arbitrary path anywhere on the remote disk — only within this sensor-owned transfer root, symmetric with the Rust-side D-10.1 share-root confinement on the local side.

## FILE-03 Selftest: Offline-Faithful vs Live-Reconfirmed

Run command (the plan's literal `dotnet run --project sensor/RdpilotSensor.csproj -c Release -- --file-traversal-selftest` fails on this host with `Exec format error`, because the csproj pins `RuntimeIdentifier=win-x64` — the RID must be explicitly overridden to run on a non-Windows host):

```bash
dotnet run --project sensor/RdpilotSensor.csproj -c Release -r linux-x64 --self-contained false -- --file-traversal-selftest
```

Result: exit 0, all 6 cases `PASS`:

| Case | Candidate | Expected | Faithfulness |
|---|---|---|---|
| trailing-dotdot-no-separator | `..` | reject | offline-faithful |
| mixed-separator-escape | `subdir\../../evil.txt` | reject | offline-faithful |
| sibling-directory-prefix | `../share-evil/secret.txt` | reject | offline-faithful |
| posix-absolute-as-relative | `/etc/passwd` | reject | offline-faithful |
| windows-drive-absolute | `C:\Windows\System32` | reject | **annotated live-reconfirm** (see below) — offline PASS backed by the explicit `LooksRooted()` check, not by `Path.IsPathRooted` alone |
| legitimate-nested-path (positive control) | `nested/legit.txt` | accept | offline-faithful |

Per the plan's own discipline (mirroring the FILE-04 chunk-size "confirm live" precedent), the Windows-drive-absolute case is still flagged for re-confirmation at the 10-05 live gate even though this plan's explicit `LooksRooted()` pre-check (mirroring 10-01-SUMMARY.md's Rust `looks_rooted()` fix) makes the offline result deterministic and platform-independent by construction, not incidental.

## Decisions Made

See `key-decisions` in frontmatter for: the plain-string `error_kind` shape choice over a typed enum, the fixed sensor-side transfer-root rationale (and its constraint on future CLI/MCP callers), the two-validator reason-mapping, and the `LooksRooted()` pre-check addition.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `dotnet run` RID override for offline verification**
- **Found during:** Task 3 verification
- **Issue:** `sensor/RdpilotSensor.csproj` pins `<RuntimeIdentifier>win-x64</RuntimeIdentifier>` (05-01, D-5.3). A bare `dotnet run --project sensor/RdpilotSensor.csproj -c Release -- --file-traversal-selftest` (the plan's literal verify command) builds a win-x64 binary and then attempts to execute it directly on this linux-x64 host, failing with `System.ComponentModel.Win32Exception (8): ... Exec format error` before any selftest logic runs.
- **Fix:** Ran `dotnet run --project sensor/RdpilotSensor.csproj -c Release -r linux-x64 --self-contained false -- --file-traversal-selftest`, overriding the RID for this one JIT-run verification only (no repo file changed — the csproj's `win-x64`/`PublishAot=true`/`SelfContained=true` properties, which govern the real deployed artifact, are untouched). This matches the plan's own BUILD/VERIFY NOTE ("substitute native x86_64-unknown-linux-gnu ... if the ... toolchain is unavailable") applied to the C# side, and mirrors the exact same-OS-substitution precedent 06-03-SUMMARY.md documented for the Rust toolchain and the linux-x64 NativeAOT surrogate publish.
- **Files modified:** None (verification-only).
- **Verification:** `dotnet run ... -r linux-x64 --self-contained false -- --file-traversal-selftest` exits 0, all 6 cases print `PASS`.
- **Committed in:** n/a (verification command choice, not a code change).

**2. [Rule 2 - Missing critical functionality] `LooksRooted()` explicit host-independent rooted-path pre-check**
- **Found during:** Task 2/Task 3 design (before writing `ValidateRemotePath`)
- **Issue:** The plan's literal `ValidateRemotePath` description (`Path.GetFullPath` the candidate, then `Path.GetRelativePath` + `".."`-prefix/`IsPathRooted`-result check) relies on `Path.IsPathRooted` to catch an absolute candidate. `Path.IsPathRooted` only recognizes a drive-letter prefix (`"C:"`) as rooted when the code is actually *running on Windows* — on this offline linux-x64 verification host, `Path.IsPathRooted("C:/Windows/System32")` is `false`, which would silently let the FILE-03 Windows-drive-absolute adversarial case be nested harmlessly under the share root instead of rejected (a real, empirically-confirmed-necessary gap, not a hypothetical one — this is the exact same platform asymmetry 10-01-SUMMARY.md's Decision #1 already diagnosed and fixed on the Rust side).
- **Fix:** Added an explicit `LooksRooted()` check (leading separator OR ASCII-letter-plus-colon drive prefix, evaluated as a plain string check independent of the host OS) run BEFORE any `Path`/`GetFullPath` call, mirroring the Rust-side `looks_rooted()` fix exactly.
- **Files modified:** `sensor/FileTransfer.cs` (part of Task 2's normal implementation, not a separate commit).
- **Verification:** The `windows-drive-absolute` FILE-03 selftest case passes offline (Task 3), proven deterministic by construction rather than by incidental directory non-existence.
- **Committed in:** `4962fdd` (folded into Task 2's normal implementation).

---

**Total deviations:** 2 auto-fixed (1 blocking verification-methodology substitution, 1 missing-critical-functionality fix carried over from the established 10-01 precedent)
**Impact on plan:** No scope creep — both deviations were either verification-only or a direct, previously-proven-necessary application of the sibling Rust plan's own already-documented fix to the equivalent C# code path.

## Issues Encountered

None beyond the two deviations above. `cargo clippy -p rdpilot --target x86_64-unknown-linux-gnu` shows only the same two pre-existing, unrelated warnings 10-01-SUMMARY.md already documented (`input.rs` unused import, `rdpdr_backend.rs` `unnecessary_get_then_check`) — neither in a file this plan touched, out of scope per the SCOPE BOUNDARY rule.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- Plan 10-04 can build `Session::upload_file`/`download_file` directly on this plan's `MsgType::FileTransfer` wire variant: call `sensor_request(MsgType::FileTransfer, Some(payload), TIMEOUT_MS)`, read `success`/`data`/`error`/`error_kind` out of the raw `serde_json::Value` reply, and map `error_kind == "path_traversal"` to the already-existing `Error::PathTraversal` (10-01) distinctly from a generic `Error::SensorRejected`/`Error::Dvc`.
- Plan 10-04 must construct the `FileTransferRequest.RemotePath` as a value RELATIVE to the sensor's fixed transfer root (see "Sensor-Side Transfer Root" above) — not an arbitrary absolute remote-machine path. This constraint should be documented on the public `upload_file`/`download_file` doc comments and is directly relevant to Phase 13/14's future CLI/MCP `put`/`get` verb design.
- The real `win-x64` NativeAOT publish/live-gate re-confirmation (including the `windows-drive-absolute` FILE-03 case) is deferred to the 10-05 live gate per the plan's own BUILD/VERIFY NOTE and this plan's own selftest annotation — no blocker for continuing to 10-04.
- No blockers.

---
*Phase: 10-sdk-file-transfer-extension*
*Completed: 2026-07-10*

## Self-Check: PASSED

All claimed files exist on disk (crates/rdpilot/src/sensor.rs, sensor/Envelope.cs, sensor/FileTransfer.cs, sensor/EnvelopeJsonContext.cs, sensor/Program.cs, this SUMMARY.md) and all 3 task commit hashes (6ea71b1, 4962fdd, 9fb86e8) are present in `git log --oneline --all`.
