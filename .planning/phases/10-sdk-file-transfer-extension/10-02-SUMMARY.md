---
phase: 10-sdk-file-transfer-extension
plan: 02
subsystem: file-transfer
tags: [rust, ironrdp-rdpdr, staged-write, atomic-rename, chunked-io]

# Dependency graph
requires:
  - phase: 10-sdk-file-transfer-extension
    plan: 01
    provides: "OpenEntry::File(PathBuf), resolve_under_root canonicalizing validator, RdpilotDriveBackend generalized to a configurable share root, connect.rs pre-creates <share_root>/.rdpilot-staging/"
provides:
  - "OpenEntry::WriteFile { staging, dest, expected_len } -- a staged-temp-file write handle granted on a create/overwrite CreateDisposition"
  - "handle_write: bounded per-IRP seek+write into the staged .part, no hardcoded chunk-size constant"
  - "handle_set_information: FILE_END_OF_FILE_INFORMATION records expected_len; all 5 decodable sub-variants return a well-formed ClientDriveSetInformationResponse"
  - "handle_close finalize_write: atomic fs::rename to the resolved destination ONLY when expected_len is Some and matches the staged file's actual length (STRICT completeness rule)"
  - "FILE-04 BLOCKING offline proxy: multi-IRP write reassembly (loop actually loops) + interrupted-transfer detection (stale .part, no destination file)"
affects: [10-04-checksum-and-session-api, 10-05-live-gate]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Staged-temp-file + atomic-rename write path (D-10.3): every write-disposition Create allocates a fresh <share_root>/.rdpilot-staging/<uuid>.part (uuid = nanos+pid+monotonic-counter, no new crate dependency), each DeviceWriteRequest does one bounded seek+write into it, and only a clean Close performs a single-syscall fs::rename to the resolve_under_root-validated destination"
    - "STRICT completeness rule: fs::rename fires ONLY when expected_len (set by a prior FILE_END_OF_FILE_INFORMATION SetInformation) is Some AND the staged file's on-disk length exactly equals it -- a Close with no SetInformation ever received, or with fewer bytes than declared, both leave the stale .part untouched rather than guessing at completeness"
    - "No per-IRP chunk-size constant anywhere: DeviceWriteRequest's offset/write_data.len() are taken as-is from the wire; FILE-04's offline test drives 3 sequential Write IRPs of DIFFERENT lengths specifically to prove no fixed chunk size is assumed"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/rdpdr_backend.rs

key-decisions:
  - "STRICT expected_len-match completeness rule (not 'any bytes written + clean close = complete'). handle_close's finalize_write only renames when OpenEntry::WriteFile.expected_len is Some (a FILE_END_OF_FILE_INFORMATION SetInformation was received) AND fs::metadata(staging).len() == expected_len exactly. A Close on a WriteFile handle that never received any SetInformation is treated as INCOMPLETE (no rename), even if bytes were written -- because without an authoritative end-of-file signal this backend cannot distinguish 'sensor finished writing' from 'transfer was cut off mid-stream'. This is deliberately more conservative than a 'bytes-accounted, no explicit signal needed' rule would be. It is explicitly flagged as a genuine unknown to confirm at the 10-05 live gate: does the real Windows redirected-drive write sequence always issue FILE_END_OF_FILE_INFORMATION before Close? 10-RESEARCH's own Code Examples section notes 'Windows' write sequence issues these' for SetInformation generally, which supports the assumption, but the exact ordering guarantee (always before Close, never after) is unverifiable offline and untested against a real Windows RDP client in this plan."
  - "Write-disposition detection: CreateDisposition::{FILE_CREATE, FILE_OPEN_IF, FILE_OVERWRITE_IF, FILE_SUPERSEDE} are treated as write intent (grant OpenEntry::WriteFile); plain FILE_OPEN and FILE_OVERWRITE (both of which require the target to already exist) fall through unchanged to the existing read-oriented OpenEntry::File path, preserving every pre-10-02 Read/QueryInformation/QueryDirectory behavior byte-for-byte for those two dispositions -- exactly the four values the plan's own action text named, no more."
  - "Staging filename scheme reused the existing test helpers' pattern (nanosecond SystemTime + process id) plus a new per-backend monotonic next_staging_id counter, avoiding a new uuid crate dependency -- collision-free for any realistic number of concurrent transfers within one backend instance's lifetime."
  - "Staging directory cleanup policy (CONTEXT's Claude's-Discretion item) remains explicitly DEFERRED, not implemented in this plan -- the plan's own action text said 'implement the sweep helper here or note it for connect.rs; do not block on it.' No orphaned-.part sweep exists yet; a future plan (likely alongside connect.rs's existing .rdpilot-staging pre-creation, 10-01) should add a best-effort cleanup of stale .part files at connect/first-use time."
  - "QueryInformation on a WriteFile handle now reports the CURRENT staged (in-progress) file size rather than rejecting -- added only to satisfy Rust's exhaustive-match requirement once the third OpenEntry variant existed, mirroring the existing OpenEntry::File arm's stat-failure-degrades-to-zero fallback. Not a stated requirement of this plan, but the minimal correct behavior for an otherwise-unhandled match arm (no test added for this specific behavior; it is exercised only indirectly)."

requirements-completed: [FILE-01, FILE-04]

# Metrics
duration: ~35min
completed: 2026-07-10
---

# Phase 10 Plan 02: Staged-Write Path (DeviceWriteRequest + SetInformation + Atomic Rename) Summary

**`DeviceWriteRequest` and `ServerDriveSetInformationRequest` are now implemented as a staged-temp-file + atomic-rename write path (D-10.3): bounded per-IRP seek+write into a `<uuid>.part` file, with a clean `Close` performing a single atomic `fs::rename` only when a prior `FILE_END_OF_FILE_INFORMATION` SetInformation's declared size exactly matches the staged bytes — proven offline against a synthetic 3-IRP-different-lengths write sequence (the chunked loop actually loops) and two independent interrupted-transfer scenarios (declared-but-short, and never-declared) that both leave a stale `.part` and never produce the destination file.**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-07-10T17:25:00Z (approx.)
- **Completed:** 2026-07-10T18:04:43Z
- **Tasks:** 2/2
- **Files modified:** 1

## Accomplishments

- `OpenEntry::WriteFile { staging, dest, expected_len }` generalizes the RDPDR file-handle table to a third kind of handle, granted by `handle_create` when the `CreateDisposition` signals create/overwrite intent (`FILE_CREATE`/`FILE_OPEN_IF`/`FILE_OVERWRITE_IF`/`FILE_SUPERSEDE`) — the destination is still resolved via 10-01's `resolve_under_root` (ancestry-validated) BEFORE any staging file is touched
- `handle_write`: one bounded `seek`+`write` per `DeviceWriteRequest` IRP into the staged `.part`, echoing the exact bytes-written count in `DeviceWriteResponse::length`; no per-IRP chunk-size constant assumed or hardcoded anywhere (10-RESEARCH Item 1) — proven by an offline test that drives 3 sequential IRPs of *different* lengths (`AAAA`/`BB`/`CCCCCC`) and asserts exact ordered reassembly
- `handle_set_information`: `FILE_END_OF_FILE_INFORMATION` records the authoritative `expected_len` on the handle; all 5 decodable `SetInformation` sub-variants (Basic/EndOfFile/Disposition/Rename/Allocation) return a well-formed `ClientDriveSetInformationResponse`, never `NOT_SUPPORTED` — Windows' standard write sequence issues this IRP as a matter of course
- `handle_close`'s new `finalize_write`: a single-syscall `fs::rename` from `.part` to the resolved destination fires ONLY when `expected_len` is `Some` and matches the staged file's exact on-disk length (the STRICT completeness rule, see Decisions below) — every other outcome (no `SetInformation` ever received, or a short/interrupted transfer) leaves the stale `.part` untouched; `Close` itself always still replies `SUCCESS` per MS-RDPEFS regardless of transfer outcome
- 6 new offline tests (17 total in `rdpdr_backend::tests`, 118 total in the crate) cover: multi-IRP reassembly, write access-control rejection, clean-transfer rename with exact content verification, declared-but-incomplete interrupted-transfer detection, never-declared-end-of-file non-rename, and SetInformation access-control rejection

## Task Commits

Each task was committed atomically:

1. **Task 1: Staged-write path — DeviceWriteRequest into `<uuid>.part`** - `47ad413` (feat)
2. **Task 2: SetInformation (end-of-file) + atomic rename on clean Close (FILE-04 interrupted-transfer detection)** - `2d3abdd` (feat)

_Note: both tasks carry `tdd="true"` in the plan frontmatter, but — mirroring 10-01's own noted exception — the `<behavior>` blocks describe acceptance behavior for functionality being built directly, not a separate RED/GREEN cycle for pre-existing code; the plan's own `<verify>` step is `cargo test`, executed after each task's implementation, and offline tests were written and passing before each task's commit._

## Files Created/Modified

- `crates/rdpilot/src/rdpdr_backend.rs` — `OpenEntry::WriteFile` variant; `is_write_disposition`/`allocate_staging_path` helpers; `handle_create` extended to grant `WriteFile` handles; `handle_write`/`write_bytes_at`; `handle_set_information`; `handle_close` extended with `finalize_write`; `handle_query_information`'s match extended for the new variant (exhaustiveness); dispatch wiring for `DeviceWriteRequest`/`ServerDriveSetInformationRequest`; 6 new tests in a new `write_path` test module

## The Transfer-Completeness Rule (for Plan 10-05's live-gate confirmation)

**Rule implemented: STRICT `expected_len` match.** A staged `.part` is renamed to its destination on `Close` if and only if:

1. A `FILE_END_OF_FILE_INFORMATION` `SetInformation` was received on that handle at some point before `Close` (`expected_len` is `Some`), AND
2. The staged file's actual on-disk byte length (`fs::metadata(staging).len()`) exactly equals that declared value.

Any other outcome — no `SetInformation` ever sent, or a length that doesn't match (short OR, defensively, long) — leaves the `.part` in staging untouched. `Close` itself always still replies `NtStatus::SUCCESS` regardless (MS-RDPEFS requires this); the caller/verifier must infer transfer success from the presence/absence of the destination file and the checksum (Plan 10-04), not from the `Close` completion status.

**This is a genuine unknown to confirm at the Plan 10-05 live gate**, exactly as 10-RESEARCH flagged: does the real Windows redirected-drive write sequence (driven by the C# sensor's `FileStream` copy against `\\tsclient\RDPILOT\<name>`) *always* issue `FILE_END_OF_FILE_INFORMATION` before `Close` for a normal, complete transfer? If it does not (e.g. Windows relies on `.NET`'s `FileStream.Dispose()`/`Close()` alone without an explicit end-of-file `SetInformation`), the STRICT rule as implemented would incorrectly treat every successful upload as "incomplete" and never rename — a live-gate-only-discoverable failure mode, not something the offline test suite can catch (the offline tests directly construct `ServerDriveSetInformationRequest` IRPs, so they always exercise the "SetInformation received" path when testing the clean case). **Action for Plan 10-05:** capture a wire trace of a real upload's IRP sequence and confirm `SetInformation(FILE_END_OF_FILE_INFORMATION)` reliably precedes `Close`; if it does not, this rule will need to fall back to "bytes-written-then-clean-close is sufficient" (accepting the corresponding loss of interrupted-transfer detection precision, or finding an alternative signal).

## Staging Cleanup Policy: Still Deferred

Per CONTEXT's own "Claude's Discretion" framing and this plan's action text ("implement the sweep helper here or note it for connect.rs; do not block on it"), **no orphaned-`.part` sweep was implemented in this plan.** Stale `.part` files from interrupted transfers (by design, per this plan's own FILE-04 detection mechanism) will accumulate under `<share_root>/.rdpilot-staging/` across sessions unless something eventually cleans them up. A future plan should add a best-effort removal of `.part` files at connect/first-use time, likely alongside `connect.rs`'s existing `.rdpilot-staging/` pre-creation step (10-01).

## Decisions Made

See `key-decisions` in frontmatter for: the STRICT completeness rule and its live-gate confirmation need, the exact four `CreateDisposition` values treated as write intent, the staging-filename scheme (no new crate dependency), the deferred cleanup policy, and the minimal `QueryInformation`-on-`WriteFile` fallback added only for match exhaustiveness.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `RUSTUP_TOOLCHAIN`/`--target` substitution for offline verification**
- **Found during:** Task 1 (initial `cargo build`/`cargo test` attempts)
- **Issue:** This host's default toolchain and `.cargo/config.toml` both pin `x86_64-pc-windows-gnu` (MinGW), not installed here — same established gap as Phases 6/7/9/10-01/10-03.
- **Fix:** Ran all `cargo build`/`cargo test`/`cargo clippy` invocations as `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> --target x86_64-unknown-linux-gnu`, matching the plan's own BUILD/VERIFY NOTE and 10-01's precedent (this plan's changes have no `cfg(windows)` branches, so the substitution is valid).
- **Files modified:** None (verification-only).
- **Verification:** All 118 offline tests pass (17 in `rdpdr_backend::tests`, up from the pre-existing 11); `cargo clippy -p rdpilot --target x86_64-unknown-linux-gnu` (no `--all-targets`, matching 10-01's exact invocation) shows only the same two pre-existing, unrelated warnings 10-01/10-03 already documented (`input.rs` unused import, `rdpdr_backend.rs` `unnecessary_get_then_check` at the unrelated `handle_query_volume_information` — neither touched by this plan). `cargo clippy --all-targets` surfaces 150+ pre-existing `clippy::expect_used` errors confined entirely to `session.rs`'s test module, verified via `git stash` to pre-exist independently of this plan's changes — out of scope per the SCOPE BOUNDARY rule (different file, unrelated to Write/SetInformation).
- **Committed in:** n/a (verification command choice, not a code change).

**2. [Rule 3 - Blocking] `handle_query_information`'s exhaustive `match` required a new arm for `OpenEntry::WriteFile`**
- **Found during:** Task 1 implementation (first `cargo build` attempt after adding the `WriteFile` variant)
- **Issue:** `handle_query_information`'s `file_size` computation matches on `&entry` with only `OpenEntry::Root`/`OpenEntry::File` arms and no wildcard — adding a third `OpenEntry` variant made this non-exhaustive, a hard compile error, not something the plan's action text explicitly called out.
- **Fix:** Added a `OpenEntry::WriteFile { staging, .. }` arm reporting the CURRENT staged (in-progress) file size via `fs::metadata(staging)`, mirroring the existing `File` arm's stat-failure-degrades-to-zero fallback exactly. This is the minimal correct behavior for an otherwise-unhandled match arm, not a new feature — no test was added specifically for this behavior (out of this plan's stated acceptance criteria), but it is exercised implicitly by every `write_path` test that performs a `Create` (which is followed by ordinary Windows `Create`→`QueryInformation` sequencing in a real client, per Plan 05's live-diagnosed finding, though these offline tests don't themselves issue a `QueryInformation` IRP).
- **Files modified:** `crates/rdpilot/src/rdpdr_backend.rs` (part of Task 1's normal implementation, not a separate commit).
- **Verification:** Compiles cleanly; all existing `query_information_succeeds_for_known_file_ids_and_rejects_unknown` tests still pass unchanged.
- **Committed in:** `47ad413` (folded into Task 1's normal implementation).

---

**Total deviations:** 2 auto-fixed (1 blocking verification-methodology substitution — established precedent, 1 blocking compile-error fix required by Rust's exhaustiveness checking, not a design choice)
**Impact on plan:** No scope creep — both deviations were either verification-only or a mechanically-required consequence of the plan's own `OpenEntry::WriteFile` design (D-10.3), not an independent feature addition.

## Issues Encountered

None beyond the two deviations above. One implementation subtlety worth flagging for future readers (Plan 10-04/10-05): `ClientDriveSetInformationResponse::new` returns `EncodeResult<Self>` (a `Result`, unlike the other response constructors in this file which are infallible) — `handle_set_information` handles the `Err` case (an unreachable-in-practice `cast_length!` overflow on `set_buffer.size()`) by falling back to a generic `DeviceCloseResponse`-shaped reject completion rather than propagating an error or panicking, consistent with this backend's `reject_unsupported` pattern for other genuinely-unreachable-but-must-be-handled cases.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- Plan 10-04 (checksum + `Session::upload_file`/`download_file` public API) can build directly on this plan's `OpenEntry::WriteFile` staged-write path — the RDPDR-side mechanics of a download-direction transfer (C# sensor writes to `\\tsclient\RDPILOT\<name>`, driving these Write/SetInformation/Close IRPs) are now fully implemented and offline-proven.
- **Live-gate confirmation required (Plan 10-05, non-blocking for 10-04):** the STRICT `expected_len`-match completeness rule's assumption that Windows always sends `FILE_END_OF_FILE_INFORMATION` before `Close` for a normal transfer. If this assumption is wrong, every real upload will appear "incomplete" and never rename — this must be the FIRST thing checked when the live gate is run, before investigating any other symptom.
- **Also deferred to a future plan:** orphaned-`.part` cleanup sweep (not blocking, not a stated FILE-01..04 requirement).
- No blockers to continuing to Plan 10-04.

---
*Phase: 10-sdk-file-transfer-extension*
*Completed: 2026-07-10*

## Self-Check: PASSED

All claimed files exist on disk (`crates/rdpilot/src/rdpdr_backend.rs`, this SUMMARY.md) and both task commit hashes (`47ad413`, `2d3abdd`) are present in `git log --oneline --all`.
