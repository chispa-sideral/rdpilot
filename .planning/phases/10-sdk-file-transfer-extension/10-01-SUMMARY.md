---
phase: 10-sdk-file-transfer-extension
plan: 01
subsystem: file-transfer
tags: [rust, ironrdp-rdpdr, path-traversal, canonicalization, sha2, ConnectionConfig]

# Dependency graph
requires:
  - phase: 05-sensor-bootstrap-rdpdr-drive
    provides: RdpilotDriveBackend (single read-only served file), Rdpdr static-channel registration in connect.rs
provides:
  - "Error::PathTraversal(String) and Error::ChecksumMismatch{expected,actual} variants + pub(crate) constructors + category strings"
  - "ConnectionConfig::share_root builder + get_share_root getter"
  - "RdpilotDriveBackend generalized: OpenEntry::File(PathBuf), resolve_under_root canonicalizing validator, per-handle Create/Read/QueryDirectory/QueryInformation"
  - "connect.rs pre-creates <share_root>/.rdpilot-staging/ before constructing the backend"
  - "FILE-03 BLOCKING Rust-side adversarial path-traversal test suite (5 tests, all passing)"
affects: [10-02-staged-write-path, 10-03-checksum-and-session-api, file-transfer-cli-mcp-consumers]

# Tech tracking
tech-stack:
  added: ["sha2 0.11 (promoted from transitive to direct dependency)"]
  patterns:
    - "resolve_under_root: canonicalize the PARENT dir (never the untrusted leaf, which may not exist), ancestry-check via Path::starts_with (component-aware), reject any candidate whose file_name() is None (trailing '..')"
    - "Explicit host-independent looks_rooted() check (leading '/' or drive-letter prefix) run BEFORE any Path/PathBuf construction, alongside unconditional backslash-to-forward-slash normalization"
    - "Backend construction takes Option<PathBuf> share_root; None preserves pre-Phase-10 sensor-only behavior byte-for-byte"

key-files:
  created: []
  modified:
    - crates/rdpilot/src/error.rs
    - crates/rdpilot/src/config.rs
    - crates/rdpilot/Cargo.toml
    - crates/rdpilot/src/rdpdr_backend.rs
    - crates/rdpilot/src/connect.rs

key-decisions:
  - "Deviated from 10-RESEARCH's resolve_under_root code example: added an explicit looks_rooted() string check (leading '/' or ASCII-letter+colon drive prefix) instead of relying solely on trim-and-join. Path::join's absolute-path-replace behavior is cfg(windows)-conditional -- a Windows drive-letter string like 'C:/Windows' is NOT recognized as absolute on this offline Linux test host, so without the explicit check the offline FILE-03 suite would only reject that adversarial case by coincidence (the literal 'C:/Windows/System32' subdir not existing under the temp share root) rather than by the intended defense. Verified this concern by mutation-testing: neutering looks_rooted() still left all 5 tests passing (proving the gap is real but currently masked by directory non-existence); the explicit check closes it regardless of host or directory layout."
  - "share_root stored as Option<PathBuf> (uncanonicalized) on RdpilotDriveBackend and canonicalized fresh on every resolve_under_root call, rather than cached at construction time. Keeps new() infallible (no Result return, matching pre-Phase-10 signature style) -- connect.rs is responsible for pre-creating the directory before constructing the backend, mapping any create failure to Error::Config."
  - "handle_query_directory/handle_query_information generalized to stat the per-handle OpenEntry::File(path) with a fallback to the sensor exe entry when the handle is Root/unknown -- true share-root directory enumeration is out of this plan's scope (not a stated FILE-01..04 requirement), preserving pre-Phase-10 listing behavior exactly for the sensor-only configuration."

requirements-completed: [FILE-01, FILE-03]

# Metrics
duration: ~20min
completed: 2026-07-10
---

# Phase 10 Plan 01: Rust-Side Path-Traversal-Guarded Share Root Foundation Summary

**Canonicalizing `resolve_under_root` validator (parent-canonicalize + component-aware ancestry check + host-independent rooted-path rejection) generalizes `RdpilotDriveBackend` from one hardcoded served file to an allow-listed, configurable share root, proven against the full FILE-03 adversarial matrix offline.**

## Performance

- **Duration:** ~20 min
- **Completed:** 2026-07-10T19:39:14+02:00
- **Tasks:** 3/3
- **Files modified:** 5

## Accomplishments
- New `Error::PathTraversal(String)` / `Error::ChecksumMismatch{expected,actual}` variants (D-10.4), with category strings, `pub(crate)` constructors, and unit test coverage mirroring the existing `SensorRejected`/`Bootstrap` pattern
- `ConnectionConfig::share_root` builder + `get_share_root()` getter (D-10.1), mirroring `sensor_binary_path`/`get_sensor_binary_path` exactly; `sha2` promoted to a direct dependency
- `RdpilotDriveBackend` generalized: `OpenEntry::File` now carries a resolved `PathBuf`; a new `resolve_under_root` validator canonicalizes and ancestry-checks every non-sensor-exe path under the configured share root (D-10.2) before any further `std::fs` call; `handle_create`/`handle_read`/`handle_query_directory`/`handle_query_information` all generalized to the per-handle resolved path
- `connect.rs` pre-creates `<share_root>/.rdpilot-staging/` before constructing the backend, mapping a create failure to `Error::Config`
- FILE-03 BLOCKING adversarial suite: 5 new tests (4 rejection cases covering the full required matrix + 1 positive control), all passing against a real temp share root

## Task Commits

Each task was committed atomically:

1. **Task 1: New error taxonomy + configurable share root + promote sha2** - `a956d92` (feat)
2. **Task 2: Generalize RdpilotDriveBackend to a canonicalized share root** - `b51cca8` (feat)
3. **Task 3: FILE-03 BLOCKING adversarial path-traversal suite** - `c5aded7` (test)

_Note: no TDD gate applies to this plan (Task 2 has `tdd="true"` in frontmatter but the `<behavior>` block describes acceptance behavior for the validator being built directly, not a separate RED/GREEN cycle for a pre-existing feature; the plan's own `<verify>` step is `cargo test`, executed after each task's implementation)._

## Files Created/Modified
- `crates/rdpilot/src/error.rs` - `Error::PathTraversal`/`Error::ChecksumMismatch` variants, constructors, category strings, unit tests
- `crates/rdpilot/src/config.rs` - `share_root` field, builder, getter, Debug field, unit test
- `crates/rdpilot/Cargo.toml` - `sha2 = "0.11"` promoted to a direct dependency
- `crates/rdpilot/src/rdpdr_backend.rs` - `OpenEntry::File(PathBuf)`, `resolve_under_root`/`looks_rooted`, generalized `handle_create`/`handle_read`/`handle_query_directory`/`handle_query_information`, `read_bytes_at` (renamed from `read_served_bytes`), `display_name` helper, `path_traversal` test module (5 tests), all 6 existing test call sites adapted to the new 3-arg `RdpilotDriveBackend::new` signature
- `crates/rdpilot/src/connect.rs` - pre-creates the share root + staging dir, passes `cfg.get_share_root()` into `RdpilotDriveBackend::new`

## Final `RdpilotDriveBackend::new` Signature (for Plan 10-02)

```rust
pub(crate) fn new(
    sensor_path: PathBuf,
    sensor_name: impl Into<String>,
    share_root: Option<PathBuf>,
) -> Self
```

`share_root: None` preserves pre-Phase-10 behavior byte-for-byte (only the sensor exe and the drive root resolve). `share_root: Some(root)` additionally routes every non-sensor-exe `Create` path through `resolve_under_root`, which canonicalizes `root` fresh on each call (not cached at construction) and only succeeds if the candidate's PARENT directory exists and canonicalizes to a path under the canonical root.

## `OpenEntry::File(PathBuf)` Contract for Plan 10-02

```rust
enum OpenEntry {
    Root,
    File(PathBuf), // the RESOLVED, absolute, canonicalized-parent-joined path
}
```

- Every `OpenEntry::File(path)` stored in `open_files` has ALREADY passed either the sensor-exe-name special case or `resolve_under_root`'s canonicalize+ancestry check at `Create` time -- `handle_read`, `handle_query_directory`, and `handle_query_information` never re-validate, they only look up the handle and operate on `path` directly.
- `path` is always an absolute path on the LOCAL filesystem (under the canonical share root, or the sensor exe's configured path) -- never a raw RDPDR wire string.
- Plan 10-02's staged-write path (`DeviceWriteRequest`/`ServerDriveSetInformationRequest`) should extend `handle_create` to also grant `OpenEntry::File` handles for `<share_root>/.rdpilot-staging/<uuid>.part` destinations (already pre-created as a directory by this plan's `connect.rs` change) using the SAME `resolve_under_root` validator -- no new validation logic needed, only a new `CreateDisposition`-aware branch (`FILE_CREATE`/`FILE_OPEN_IF`) in `handle_create`.

## Decisions Made

1. **Explicit `looks_rooted()` check, deviating from 10-RESEARCH's code example.** The research code example does a naive `root_canonical.join(untrusted.trim_start_matches('/'))` and relies entirely on the ancestry check afterward. On the real Windows target, `Path::join` recognizes a drive-letter-prefixed string (e.g. `"C:/Windows"`) as absolute and replaces the whole path, so the ancestry check alone would still catch it there. But on THIS offline Linux test host, `Path::is_absolute()`/`Path::join` do NOT recognize `"C:/Windows"` as absolute (no leading `/`), so it gets silently nested under the share root instead of replacing it -- meaning the naive approach's rejection of `"C:\Windows\System32"` in the FILE-03 suite would only happen by coincidence (that literal subdirectory not existing under the temp root), not by the intended defense. Verified via a targeted, reverted mutation test (neutering `looks_rooted()` to always return `false`): all 5 path-traversal tests still passed, confirming this gap is real and masked, not merely theoretical. Added an explicit, host-independent `looks_rooted()` string check (leading `/` OR an ASCII-letter-plus-colon drive prefix) that runs BEFORE any `Path`/`PathBuf` construction, closing the gap regardless of host OS or incidental directory layout.
2. **`share_root` stored uncanonicalized, canonicalized fresh per call.** Keeps `RdpilotDriveBackend::new` infallible (no `Result` return), matching the existing constructor style; `connect.rs` is responsible for pre-creating the directory (mapping any failure to `Error::Config`) before constructing the backend.
3. **Directory-listing generalization kept minimal.** `handle_query_directory`/`handle_query_information` were generalized to stat the per-handle resolved path with a fallback to the sensor exe entry for `Root`/unknown handles -- genuine share-root directory enumeration (listing multiple files under a subdirectory) is out of this plan's scope; FILE-01/02/03/04 do not require it, and the existing tests (which never exercised true directory listing beyond the single served file) pass unchanged.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `RUSTUP_TOOLCHAIN`/`--target` substitution for offline verification**
- **Found during:** Task 1 (initial `cargo test` attempt)
- **Issue:** This host's default toolchain and `.cargo/config.toml` both pin `x86_64-pc-windows-gnu` (MinGW), which is not installed here (established gap, STATE.md Phase 6/7/9 precedent).
- **Fix:** Ran all `cargo test`/`cargo clippy` invocations as `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> --target x86_64-unknown-linux-gnu`, matching the established substitution pattern (this plan's changes have no `cfg(windows)` branches, so the substitution is valid; the real windows-gnu build should still be re-confirmed on the pinned dev machine before any live gate).
- **Files modified:** None (verification-only).
- **Verification:** All 111 offline tests pass; `cargo clippy` shows only the two pre-existing, unrelated warnings (an unused import in `input.rs` and a pre-existing `unnecessary_get_then_check` lint in `query_information`, neither touched by this plan).
- **Committed in:** n/a (verification command choice, not a code change).

---

**Total deviations:** 1 auto-fixed (1 blocking, tooling substitution only)
**Impact on plan:** No code-level deviation from the plan's stated file scope or acceptance criteria beyond the deliberately-documented `resolve_under_root` shape change (see Decisions #1), which is explicitly anticipated by the plan's own `<output>` instruction to record any such deviation.

## Issues Encountered

None beyond the toolchain substitution above. One implementation subtlety worth flagging for future readers: `Path::file_name()` returns `None` for any path whose last component is `..` (verified empirically) -- this is the exact mechanism `resolve_under_root` relies on to reject the trailing-`..`-no-separator adversarial case (CVE-2025-48817's off-by-one class), and it works correctly even when the `..`'s parent directory genuinely exists (confirmed by pre-creating a `subdir` directory in the test setup specifically so the rejection is proven to come from this check, not from a missing directory).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- `OpenEntry::File(PathBuf)` and `resolve_under_root` are ready for Plan 10-02's staged-write path (`DeviceWriteRequest`/`ServerDriveSetInformationRequest`) to build on directly -- no new validation logic needed, only a `CreateDisposition`-aware branch in `handle_create` for `.rdpilot-staging/<uuid>.part` destinations.
- `sha2` is a direct dependency, ready for Plan 10-04's checksum verification.
- `Error::PathTraversal`/`Error::ChecksumMismatch` are ready to be surfaced by the future `Session::upload_file`/`download_file` public API (D-10.4).
- No blockers. The real `x86_64-pc-windows-gnu` build/live-gate re-confirmation (this plan's code has no `cfg(windows)` branches, so risk is low) should happen before any live VM gate, per established project practice.

---
*Phase: 10-sdk-file-transfer-extension*
*Completed: 2026-07-10*

## Self-Check: PASSED

All claimed files exist on disk (error.rs, config.rs, Cargo.toml, rdpdr_backend.rs, connect.rs, this SUMMARY.md) and all 3 task commit hashes (a956d92, b51cca8, c5aded7) are present in `git log --oneline --all`.
