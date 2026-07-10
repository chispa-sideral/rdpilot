---
phase: 10-sdk-file-transfer-extension
plan: 04
subsystem: file-transfer
tags: [rust, sha2, sensor-protocol, path-traversal, checksum, dvc]

# Dependency graph
requires:
  - phase: 10-sdk-file-transfer-extension
    plan: 01
    provides: "Error::PathTraversal(String)/Error::ChecksumMismatch{expected,actual} variants + pub(crate) constructors, ConnectionConfig::share_root builder/getter, sha2 promoted to a direct dependency"
  - phase: 10-sdk-file-transfer-extension
    plan: 02
    provides: "Staged-write path (OpenEntry::WriteFile, handle_write/handle_set_information, atomic rename on Close) -- the RDPDR-side mechanics download_file relies on"
  - phase: 10-sdk-file-transfer-extension
    plan: 03
    provides: "MsgType::FileTransfer wire variant, C# FileTransfer.cs handler, error_kind=\"path_traversal\" sentinel producer, {op, remote_path, share_name} request / {bytes_transferred, sha256} reply shape"
provides:
  - "Public Session::upload_file(&self, local: &Path, remote_name: &str) -> Result<TransferOutcome> and Session::download_file(&self, remote_name: &str, local: &Path) -> Result<TransferOutcome>"
  - "pub struct TransferOutcome { bytes_transferred: u64, checksum: String } (owned, credential-free, D-09)"
  - "sensor_request()'s success:false branch now maps a reply error_kind==\"path_traversal\" to the distinct Error::PathTraversal, before falling through to Error::sensor_rejected -- the end-to-end BLOCKER fix producer"
  - "sha256_file(): bounded 64KiB streaming SHA-256 helper (sha2::Sha256), used to independently verify both upload and download directions against the sensor-reported digest"
  - "Session.share_root: Option<PathBuf> field (captured from ConnectionConfig::share_root at connect time) -- the local-side staging directory both new methods stage/retrieve bytes from with plain std::fs calls"
  - "TRANSFER_TIMEOUT_MS (30s) -- explicitly documented UNVALIDATED initial guess, live-tuning target for the 10-05 gate"
affects: [10-05-live-gate, file-transfer-cli-mcp-consumers]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Session-side local staging: upload_file copies the caller's local file into <share_root>/<sdk-generated-staging-name> via std::fs::copy (never remote_name) so the RDPDR-redirected RDPILOT drive can serve it to the sensor; download_file moves the sensor-written <share_root>/<staging-name> to the caller's local destination via std::fs::rename with a copy+remove EXDEV fallback -- no RDPDR round trip needed for the SDK's own side of the share root, only the remote Windows machine's side"
    - "unique_share_name(): nanos + process::id() + a monotonic counter (reusing next_req_id), mirroring rdpdr_backend.rs's own allocate_staging_path naming scheme -- no new crate dependency, deliberately distinct from any caller-supplied name to avoid collision"
    - "Two-stage checksum flow (D-10.5): sha256_file() is called independently on the LOCAL file on both sides (the source for upload, the moved-into-place destination for download) and compared case-insensitively against the sensor-reported sha256 (normalizing for C#'s Convert.ToHexString uppercase output) -- neither side trusts the other's reported hash without recomputing"
    - "error_kind-aware sensor_request(): the shared round-trip helper (not per-caller code) now inspects a structured error_kind field on a success:false reply BEFORE falling through to the generic Error::sensor_rejected -- WindowList/Uia/LaunchProcess never set error_kind so their behavior is provably unchanged (regression-tested)"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/session.rs
    - crates/rdpilot/src/lib.rs
    - crates/rdpilot/Cargo.toml

key-decisions:
  - "Task 1/Task 2 split required reconstructing an intermediate ('interface-first') version of upload_file/download_file that returns TransferOutcome{checksum: sensor_sha256} UNVERIFIED, then layering sha256_file()-based independent verification on top in a second commit -- because the plan's own two-task structure places wiring (Task 1) and SHA-256 verification (Task 2) as sequential, separately-committable units, but the natural single-pass implementation intertwines them in the same method bodies. Rebuilt this split by hand (not via git patch surgery) to guarantee both intermediate and final states compile and pass their own test subsets."
  - "Local share_root data-plane staging (fs::copy for upload, fs::rename+copy-fallback for download) was added as Rule-2 missing-critical-functionality: the plan's Task 1 action text describes only the DVC control-plane round trip (send FileTransfer, extract bytes/sha256) and is silent on how local bytes actually reach/leave the RDPDR-served share root. Without this step neither method would functionally transfer any bytes -- the sensor's own reply only confirms ITS side of the copy (real-remote-path <-> UNC path) completed; the Rust SDK's own side (UNC path <-> Session::upload_file/download_file's local/remote_name arguments) requires an explicit std::fs step this plan adds. This required adding a new Session.share_root field (populated from ConnectionConfig::share_root at connect time, mirroring desktop_size's static-capture pattern) since Session previously stored no config-derived state at all."
  - "tokio dev-dependency gained the \"test-util\" feature so the TRANSFER_TIMEOUT_MS (30s) offline timeout test can use #[tokio::test(start_paused = true)] instead of a real 30-second wall-clock wait -- proves upload_file passes the correct timeout constant into sensor_request without slowing the offline suite."
  - "Checksum expected/actual field convention: expected = sensor-reported digest, actual = locally-recomputed digest, applied consistently to BOTH upload_file and download_file (the plan's <action> text only specifies this ordering literally for download_file; extended it symmetrically to upload_file for a single, predictable Error::ChecksumMismatch shape across both directions)."
  - "share_name is a fresh SDK-generated staging name (unique_share_name()), never remote_name or any caller-supplied value -- offline-tested (assert_ne!) to guard against a future change accidentally reusing the caller's destination name for local staging, which would create an ambiguous/colliding local file path."

requirements-completed: [FILE-01, FILE-02]

# Metrics
duration: ~40min
completed: 2026-07-10
---

# Phase 10 Plan 04: Public upload_file/download_file API + SHA-256 Verification Summary

**`Session::upload_file`/`download_file` ride the existing async `sensor_request()` extension point exactly like `launch_process` (zero threading-model change), stage/retrieve bytes through the configured local `share_root` with plain `std::fs` calls, and independently verify SHA-256 on both sides -- with the BLOCKER-fix `error_kind=="path_traversal"` -> `Error::PathTraversal` mapping proven end-to-end by an offline test.**

## Performance

- **Duration:** ~40 min
- **Completed:** 2026-07-10
- **Tasks:** 2/2
- **Files modified:** 3

## Accomplishments

- Public `Session::upload_file(&self, local: &Path, remote_name: &str) -> Result<TransferOutcome>` and `Session::download_file(&self, remote_name: &str, local: &Path) -> Result<TransferOutcome>` — both async, riding `sensor_request(MsgType::FileTransfer, ...)` exactly like `launch_process`; `pub struct TransferOutcome { bytes_transferred: u64, checksum: String }` re-exported from `lib.rs` (owned, credential-free, D-09)
- **BLOCKER fix (D-10.4) — end-to-end `Error::PathTraversal` producer**: `sensor_request`'s shared `success:false` branch now inspects the reply's `error_kind` field and returns `Error::path_traversal(reason)` when it equals `"path_traversal"` — BEFORE falling through to the generic `Error::sensor_rejected(reason)`. Proven offline by a direct test (`success:false, error_kind:"path_traversal"` reply -> `Error::PathTraversal`, NOT `SensorRejected`) plus a regression test confirming a `success:false` reply with a different/absent `error_kind` still maps to `Error::SensorRejected` — the new branch is strictly additive, WindowList/Uia/LaunchProcess are provably unaffected
- SHA-256 integrity verification (D-10.5): a new `sha256_file()` helper streams any local file through `sha2::Sha256` with a bounded 64KiB buffer (never loads the whole file into memory, never a custom hash loop). Both `upload_file` (hashes the local source) and `download_file` (hashes the moved-into-place local destination) compare their independently-computed digest, case-insensitively, against the sensor-reported `sha256` and return the distinct `Error::ChecksumMismatch { expected: sensor_hash, actual: local_hash }` on divergence
- Local share-root data-plane wiring (Rule 2 addition, not explicit in the plan's action text but required for functional correctness): a new `Session.share_root: Option<PathBuf>` field, captured from `ConnectionConfig::share_root` at connect time; `upload_file` stages the local source into `<share_root>/<sdk-generated-name>` via `std::fs::copy` so the RDPDR-redirected `RDPILOT` drive can serve it to the sensor, and `download_file` moves the sensor-written staged file to the caller's `local` destination via `std::fs::rename` (with a copy+remove fallback for cross-device destinations)
- `TRANSFER_TIMEOUT_MS = 30_000` (30s) is explicitly documented as an UNVALIDATED initial guess, a live-tuning target for the 10-05 gate — its own offline timeout test uses Tokio's paused/mocked clock (`#[tokio::test(start_paused = true)]`, `tokio` `"test-util"` dev-feature added) so proving the correct timeout constant is used never costs a real 30-second wait
- 12 new offline tests added to `session.rs`'s existing `#[cfg(test)] mod tests` (129 total offline tests in the crate, up from 111 pre-Phase-10): `error_kind` mapping (2), config-missing precondition (2), request-emission + happy-path checksum verification (2), checksum-mismatch (2), malformed-reply (1), SHA-256 known-vector (1), and the paused-clock timeout test (1)

## Task Commits

Each task was committed atomically:

1. **Task 1: TransferOutcome type + upload_file/download_file over sensor_request** - `12038d4` (feat)
2. **Task 2: SHA-256 integrity verification + ChecksumMismatch branch** - `c61ea68` (feat)

_Note: both tasks carry `tdd="true"` in the plan frontmatter, mirroring 10-01/10-02/10-03's own documented exception — the `<behavior>` blocks describe acceptance behavior for functionality being built directly, not a separate RED/GREEN cycle for pre-existing code; the plan's own `<verify>` step is `cargo test`, and offline tests were written and passing before each task's commit. To keep the two commits genuinely atomic (Task 1 without SHA-256 verification, Task 2 layering it on) despite the natural single-pass implementation intertwining both, the Task 1 commit was constructed as a deliberate interface-first intermediate: `TransferOutcome.checksum` carries the sensor-reported digest UNVERIFIED, `sha256_file()` does not yet exist, and the three Task-2-specific tests (SHA-256 known-vector, upload checksum-mismatch, download checksum-mismatch) are absent — each intermediate and final state was independently built and fully tested before its own commit, not reconstructed via git patch surgery._

## Files Created/Modified

- `crates/rdpilot/src/session.rs` — `TRANSFER_TIMEOUT_MS` const; `Session.share_root: Option<PathBuf>` field + `connect()` wiring from `cfg.get_share_root()`; `pub struct TransferOutcome`; `sha256_file()` helper; `sensor_request()`'s `error_kind` branch; `pub async fn upload_file`/`download_file`; `unique_share_name()` helper; 12 new tests + 2 new test helpers (`test_share_root_dir`, `test_session_with_sensor_and_share_root`); all 9 pre-existing offline test `Session { .. }` literals updated with the new `share_root` field
- `crates/rdpilot/src/lib.rs` — `pub use session::{Session, TransferOutcome}`, public-surface doc comment updated
- `crates/rdpilot/Cargo.toml` — `tokio` dev-dependency gains the `"test-util"` feature (paused-clock timeout test only; no new crate)

## Decisions Made

See `key-decisions` in frontmatter for: the Task 1/Task 2 commit-split reconstruction, the local `share_root` staging addition (Rule 2), the `test-util` dev-feature addition, the expected/actual checksum field convention applied symmetrically to both directions, and the `unique_share_name()` non-collision guarantee.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 - Missing critical functionality] Local `share_root` data-plane staging (`std::fs::copy`/`std::fs::rename`) and a new `Session.share_root` field**
- **Found during:** Task 1 design (before writing `upload_file`/`download_file`)
- **Issue:** The plan's Task 1 `<action>` text describes only the DVC control-plane round trip — build the `{op, remote_path, share_name}` payload, call `sensor_request`, extract `bytes`/`sha256` from the reply — with no mention of how the caller's `local` file bytes actually reach the RDPDR-served share root (upload) or how the sensor-written staged bytes reach the caller's `local` destination (download). Per 10-RESEARCH's own architecture diagram and 10-03-SUMMARY's "Next Phase Readiness" note, the sensor's `FileTransfer.Transfer()` only copies between the validated real remote path and the UNC path `\\tsclient\RDPILOT\<share_name>` — it never touches the SDK caller's `local`/`remote_name` arguments directly. Without an explicit local `std::fs` step on the Rust side, neither method would transfer any actual file content: `upload_file` would send a request referencing a `share_name` that was never populated with the local file's bytes, and `download_file`'s caller-requested `local` path would never receive the bytes the sensor wrote to the staged share-root file. `Session` also had no existing field carrying the configured `share_root` at all (`Session::connect` never stored anything from `cfg` beyond what was already captured into other fields).
- **Fix:** Added a new `share_root: Option<PathBuf>` field to `Session`, populated from `cfg.get_share_root()` at connect time (mirrors `desktop_size`'s static-capture rationale). `upload_file` copies `local` into `<share_root>/<unique_share_name()>` via `fs::copy` before sending the request (with best-effort cleanup after, regardless of outcome); `download_file` moves `<share_root>/<unique_share_name()>` to `local` via `fs::rename` (with a `fs::copy`+`fs::remove_file` fallback for cross-device destinations) after a successful reply. Both fail fast with a new-but-not-novel `Error::Config` variant use (the enum variant + `category()` mapping already existed from Plan 10-01/connect.rs's existing usage; only a new call site) if no `share_root` was configured, before sending anything.
- **Files modified:** `crates/rdpilot/src/session.rs` (part of Task 1's normal implementation, not a separate commit).
- **Verification:** `upload_file_sends_filetransfer_request_and_verifies_checksum_on_success` and `download_file_sends_filetransfer_request_and_moves_staged_file_to_destination` offline tests directly exercise the full local-staging round trip (real temp-dir `share_root`, real file content, asserting the destination file's actual bytes); `upload_file_without_configured_share_root_returns_config_error_without_sending`/`download_file_without_configured_share_root_returns_config_error_without_sending` confirm the fail-fast precondition sends nothing.
- **Committed in:** `12038d4` (folded into Task 1's normal implementation).

**2. [Rule 3 - Blocking] `tokio` dev-dependency `"test-util"` feature addition, for a fast offline `TRANSFER_TIMEOUT_MS` proof**
- **Found during:** Task 1, writing the timeout-behavior test
- **Issue:** `TRANSFER_TIMEOUT_MS` is 30 seconds (a deliberately wide, UNVALIDATED bound per the plan's own `<output>` instruction). Proving `upload_file` actually passes this specific constant (not some other bound) into the shared `sensor_request` helper, mirroring the existing `set_foreground_window_times_out_and_removes_pending_entry` pattern (which uses the much shorter 500ms `ACTION_TIMEOUT_MS`), would otherwise require a real 30-second wall-clock wait per test run — unacceptable for an offline unit-test suite meant to run on every task commit.
- **Fix:** Added the `"test-util"` feature to the existing `tokio` dev-dependency (no new crate, no version change) and wrote `upload_file_times_out_after_transfer_timeout_and_removes_pending_entry` using `#[tokio::test(start_paused = true)]`, which auto-advances Tokio's mocked clock past idle periods — the test proves the full 30-second timeout-and-pending-map-cleanup behavior in well under a second of real time.
- **Files modified:** `crates/rdpilot/Cargo.toml` (part of Task 1's normal implementation, not a separate commit).
- **Verification:** The test passes in the same `cargo test` run as every other offline test (~3s total suite time for 129 tests); confirmed the test genuinely exercises the 30s bound by inspecting the assertion (`Error::Dvc` mentioning the timeout, pending map empty) — the same shape as the existing 500ms sibling test, just at the real configured constant.
- **Committed in:** `12038d4` (folded into Task 1's normal implementation).

**3. [Rule 3 - Blocking] `RUSTUP_TOOLCHAIN`/`--target` substitution for offline verification**
- **Found during:** Task 1 (initial `cargo build`/`cargo test` attempts)
- **Issue:** This host's default toolchain and `.cargo/config.toml` both pin `x86_64-pc-windows-gnu` (MinGW), not installed here — same established gap as Phases 6/7/9/10-01/10-02/10-03.
- **Fix:** Ran all `cargo build`/`cargo test`/`cargo clippy` invocations as `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> --target x86_64-unknown-linux-gnu`, matching the established substitution pattern (this plan's changes have no `cfg(windows)` branches, so the substitution is valid; the real windows-gnu build should still be re-confirmed on the pinned dev machine before the 10-05 live gate).
- **Files modified:** None (verification-only).
- **Verification:** All 129 offline tests pass (up from 111 pre-Phase-10, 118 after 10-02, 129 after this plan — 10-03 added no Rust-side tests beyond the wire-shape unit test already counted); `cargo clippy -p rdpilot --target x86_64-unknown-linux-gnu` (no `--all-targets`, matching prior Phase-10 plans' exact invocation) shows only the same two pre-existing, unrelated warnings already documented by 10-01/10-02/10-03 (an unused import in `input.rs`, a pre-existing `unnecessary_get_then_check` lint in `rdpdr_backend.rs`'s unrelated `handle_query_volume_information`), neither touched by this plan.
- **Committed in:** n/a (verification command choice, not a code change).

---

**Total deviations:** 3 auto-fixed (1 missing-critical-functionality addition required for the feature to functionally work at all, 1 blocking dev-tooling addition to keep the offline suite fast, 1 blocking verification-methodology substitution — established precedent)
**Impact on plan:** No scope creep beyond what correctness required. The local `share_root` staging (deviation 1) is the direct, necessary completion of what the plan's own Task 1 truths already claimed ("moves a local file to a named remote destination") — without it the public API would compile and pass a shallow request-shape test while never actually transferring any bytes. The `test-util` addition (deviation 2) is test-infrastructure-only, adds no new supply-chain node, and does not touch the public API or `TRANSFER_TIMEOUT_MS`'s production value.

## Issues Encountered

None beyond the three deviations above. One implementation subtlety worth flagging for Plan 10-05: the STRICT `expected_len`-match completeness rule flagged as an open question by 10-02-SUMMARY.md (does Windows always send `FILE_END_OF_FILE_INFORMATION` before `Close`?) is a live-gate-only-discoverable risk this plan's offline tests cannot exercise — this plan's `download_file` assumes the sensor's `success:true` DVC reply implies the RDPDR-side atomic rename already completed and the file is present at `<share_root>/<share_name>`; if the STRICT rule's assumption turns out to be wrong at the live gate, `download_file`'s `fs::rename`/`fs::copy` step would fail with a not-found error surfaced as `Error::Dvc`, which is a legible (if not maximally specific) failure mode, not silent corruption.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- **Live-gate confirmation required (Plan 10-05, BLOCKING for phase completion, non-blocking for this plan):**
  1. Real end-to-end `upload_file`/`download_file` against a live VM — proves the full DVC-plane + RDPDR-plane + local-staging round trip this plan only proved offline in pieces (DVC control-plane wiring offline-tested; RDPDR write path offline-tested in 10-02; local staging offline-tested here with simulated staged files, never a real sensor).
  2. `TRANSFER_TIMEOUT_MS` (30s) — UNVALIDATED initial guess, must be exercised against real multi-MB transfer latency and tuned if too tight or unnecessarily wide.
  3. The STRICT `expected_len`-match completeness rule (10-02's open question) — confirm Windows' real redirected-drive write sequence always issues `FILE_END_OF_FILE_INFORMATION` before `Close` for a normal `download_file` transfer; if not, `download_file` will see a missing staged file at the local-move step.
  4. FILE-03's `windows-drive-absolute` adversarial case (10-01/10-03's own annotated live-reconfirm items) — carried forward, no new work needed by this plan.
- `Session::upload_file`/`download_file` and `TransferOutcome` are the final public SDK surface Phase 13 (CLI `put`/`get`) and Phase 14 (MCP `put`/`get`) will call directly — both are ready to consume: `remote_name` is documented as relative to the sensor's FIXED `%TEMP%/rdpilot-transfer-root` (never an arbitrary absolute remote path), `Error::PathTraversal`/`Error::ChecksumMismatch` are distinct, legible error variants for those future CLI/MCP layers to map to user-facing messages.
- No blockers to the 10-05 live gate.

---
*Phase: 10-sdk-file-transfer-extension*
*Completed: 2026-07-10*

## Self-Check: PASSED

All claimed files exist on disk (`crates/rdpilot/src/session.rs`, `crates/rdpilot/src/lib.rs`, `crates/rdpilot/Cargo.toml`, this SUMMARY.md) and both task commit hashes (`12038d4`, `c61ea68`) are present in `git log --oneline --all`.
