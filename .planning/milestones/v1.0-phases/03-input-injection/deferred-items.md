# Phase 3 — Deferred Items

## Pre-existing unused-import warning in `input.rs`

- **File:** `crates/rdpilot/src/input.rs:11` — `use crate::error::Error;`
- **Found during:** Plan 04 Task 3 (canonical live validation), while running `cargo clippy -p rdpilot --all-targets`.
- **Status:** Pre-existing — present identically in the very first clippy/test run of this checkpoint, before any Task-3 changes were made. Not caused by, or related to, this plan's changes (`tests/live_session.rs`, `infra/scripts/Configure-Target.ps1`).
- **Disposition:** Out of scope per the executor's scope boundary (only auto-fix issues directly caused by the current task's changes). Left unfixed; `cargo clippy` still exits 0 (warning only, not an error).
- **Suggested fix (for a future task that touches `input.rs`):** `cargo clippy --fix --lib -p rdpilot` removes the unused import.
