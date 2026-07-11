---
phase: 13-cli-surface
plan: 07
subsystem: rdpilot-cli
tags: [cli, file-transfer, error-taxonomy, exit-codes, no-clobber, path-absolutization]
dependency-graph:
  requires: [13-05, 13-06]
  provides: [CLI-03]
  affects: [Phase 15 (batched live gate re-exercises put/get through the CLI)]
tech-stack:
  added: []
  patterns:
    - "Path absolutization via std::env::current_dir() join (not std::path::absolute — workspace pins rust-version 1.78)"
    - "CLI-side no-clobber check runs strictly before the wire round trip (no wasted request on a refusal the CLI can already see locally)"
    - "--json error rendering mirrors success-path shape: {\"error\":{\"code\":\"<kebab>\",\"message\":\"...\"}}"
key-files:
  created:
    - crates/rdpilot-cli/src/verbs/file.rs
    - crates/rdpilot-cli/tests/cli_errors.rs
  modified:
    - crates/rdpilot-cli/src/cli.rs
    - crates/rdpilot-cli/src/main.rs
    - crates/rdpilot-cli/src/verbs/mod.rs
    - crates/rdpilot-cli/src/exit_codes.rs
decisions:
  - "Asymmetric no-clobber shipped exactly as scoped: get fully CLI-side enforced (exists() check + --force), put's remote no-clobber deferred to backlog Phase 999.5 — documented in put --help, never silently under-delivered"
  - "daemon-unreachable proven offline by copying the compiled rdpilot binary into an isolated dir with no sibling rdpilot-daemon binary, rather than mocking connect_or_spawn — exercises the real spawn-failure path"
  - "--json error output printed to stdout (mirrors the success-path convention), not stderr, so a scripted caller never branches its parser on exit status alone to find the payload"
metrics:
  duration: "~55 minutes"
  completed: 2026-07-11
---

# Phase 13 Plan 07: CLI put/get file transfer + finalized error taxonomy Summary

`rdpilot put`/`get` transfer files over the daemon's shared filesystem (no bytes on the wire) with asymmetric no-clobber — `get` fully CLI-side enforced, `put`'s remote overwrite-refusal a documented known gap — and the CLI-03 error taxonomy (session-not-found/daemon-unreachable/no-clobber/...) now maps to eight distinct, legible exit codes proven offline against the real compiled daemon binary.

## What Was Built

**Task 1 — `put`/`get` verbs (path absolutization, get no-clobber, put asymmetry documentation):**

- Extended the clap tree (`crates/rdpilot-cli/src/cli.rs`) with flat `Put`/`Get` leaves and a grouped `File` subcommand family (`rdpilot file put|get`), sharing one `PutArgs`/`GetArgs` leaf struct each per the flat+grouped duality pattern already established for `session` (D-13.1).
- `crates/rdpilot-cli/src/verbs/file.rs` (new): both `put` and `get` absolutize `--local` via a private `absolutize()` helper — an already-absolute path passes through unchanged; a relative path is joined onto `std::env::current_dir()`. Deliberately NOT the stdlib helper that stabilized in Rust 1.79 (this workspace pins `rust-version = "1.78"`).
- `get` checks the absolutized local destination's `exists()` BEFORE sending any request. If it exists and `--force` was not passed, returns `CliError::NoClobber` (exit 8) with a message naming the refused path — the daemon is never even contacted for a refused no-clobber `get`.
- `put` performs NO remote existence check (out of scope this phase — the sensor's Upload handler has no overwrite-refusal and extending it is explicitly excluded). `--force` on `put` is accepted for forward-compatibility only and changes nothing about wire behavior today. `put --help` documents this explicitly: *"Accepted for forward-compat only — remote overwrite is NOT prevented this phase (see backlog: symmetric remote no-clobber, Phase 999.5)"*.
- Both verbs wired into `main.rs`'s exhaustive `dispatch` match (flat and grouped spellings call the identical handler function).
- Successful transfers print `transferred <n> bytes (checksum <sha256>)` (table) or the full `TransferOutcome` struct (`--json`).

**Task 2 — Finalized CLI-03 error taxonomy + offline proof:**

- `exit_codes.rs`'s `code_for` mapping (already laid down substantially complete in Plan 13-05) confirmed total and panic-free: `SessionNotFound`→2, `DaemonUnreachable`→3 (client-synthesized, never wire-transmitted), `TransferFailed`→4, `PathTraversal`→5, `ChecksumMismatch`→6, `DuplicateSession`→7, `NoClobber`→8 (CLI-local), everything else (`Internal`, `MissingConfig`, `Transport`)→1.
- Added `exit_codes::code_str_for` — the kebab-case wire-style code string for every `CliError` class (for `Wire` variants, exactly the wire's own `#[serde(rename_all = "kebab-case")]` string; client-local classes get their own fixed spelling: `daemon-unreachable`, `no-clobber`, `missing-config`, `internal`, `transport`).
- Wired `code_str_for` into `main.rs`'s error path: `--json` now emits `{"error":{"code":"<kebab>","message":"..."}}` to stdout (verified manually: `rdpilot disconnect --session ghost --json` → `{"error":{"code":"session-not-found","message":"SessionNotFound: no such session \"ghost\""}}`, exit 2).
- New `crates/rdpilot-cli/tests/cli_errors.rs` (3 tests, all passing, none `#[ignore]`-gated):
  1. `session_not_found_maps_to_exit_code_2_and_names_the_session` — `disconnect --session ghost-session --json` against a freshly auto-started, empty-registry daemon → exit 2, `--json` payload asserts the exact `session-not-found` code.
  2. `daemon_unreachable_maps_to_exit_code_3` — the compiled `rdpilot` binary copied into an isolated directory with NO sibling `rdpilot-daemon` binary, pointed at a socket with no listener → `connect_or_spawn`'s spawn attempt fails immediately (never reaches the bounded backoff loop) → exit 3.
  3. `get_no_clobber_refuses_without_force_and_succeeds_with_force` — a real `connect` establishes a live session against the canned fake connector; `get` against a pre-existing local file without `--force` → exit 8, file untouched; the identical request WITH `--force` → exit 0, proceeds against `FakeTestSession::download_file`.

## Deviations from Plan

None — plan executed exactly as written. `exit_codes.rs`'s numeric taxonomy (`code_for`) was already substantially complete from Plan 13-05's forward-looking scaffold (including the `NoClobber` placeholder variant); this plan's Task 2 finalized the remaining piece research/planning identified as outstanding — the `--json` machine-readable error shape and `code_str_for` — plus the offline integration proof.

### Toolchain substitution (environment note, not a deviation from scope)

Fully offline execution: built and tested against the native Linux substitute target per the assignment's toolchain note —
```
RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build/test/clippy --target x86_64-unknown-linux-gnu
```
— rather than the project's actual Windows cross-compilation target. No source changes were made to accommodate this; it only affects which host triple the toolchain resolves against.

## Verification

- `cargo build --workspace` (substitute target): green, no errors (one pre-existing, unrelated warning in `rdpilot`'s `input.rs` — out of this plan's scope boundary, not touched).
- `cargo test -p rdpilot-cli` (substitute target): 9 unit tests + 3 `cli_errors` integration tests + 1 `cli_lifecycle` test + 1 `cli_verbs` test (1 `#[ignore]`d) — all green.
- `cargo test --workspace` (substitute target): 129 passed, 0 failed (plus 28 intentionally `#[ignore]`d heavy/live-gate tests across the workspace) across every crate.
- `cargo clippy -p rdpilot-cli --all-targets` (substitute target): clean, zero warnings — including the crate's own `#![deny(clippy::unwrap_used)]`/`#![deny(clippy::expect_used)]` gates (the new `verbs/file.rs` inline unit tests use `Result`-returning `#[test]` functions with `?`, not `.expect()`/`.unwrap()`, to satisfy this; `tests/cli_errors.rs`, a separate integration-test crate root, follows this codebase's established convention of using `.expect()`/`.unwrap()` freely there, matching `cli_lifecycle.rs`/`cli_verbs.rs`).
- `cargo tree -p rdpilot-cli` (substitute target): confirmed free of `ironrdp`, `rustls`, `rdpilot`, `rdpilot-daemon` — full dependency list is `base64`, `clap` (+ its own deps), `rdpilot-config`, `rdpilot-ipc`, `serde`/`serde_json`, `tokio` (thin-client invariant D-17 preserved).
- `put --help` / `get --help` manually inspected: `put`'s `--force` help text names the known gap and points at the backlog item verbatim; `get`'s `--force` help text is unambiguous about overwrite semantics.
- Manual `--json` error smoke test against the real compiled daemon (fake connector): confirmed the exact `{"error":{"code":"session-not-found","message":"..."}}` shape.

Real multi-MB `put`/`get` transfer against a live Windows target remains `#[ignore]`-deferred to the batched Phase 15 live gate — FILE-01/02/04 were already live-verified in Phase 10; the CLI path re-exercises the same transfer semantics through a new client surface, it does not re-prove them.

## Known Gaps (documented, not stubs)

- **`put`'s remote no-clobber** is not enforced this phase (developer-accepted asymmetry). Tracked as backlog Phase 999.5 (`.planning/ROADMAP.md`): extend the C# sensor's `FileTransfer` Upload handler with a `no_clobber` flag + `File.Exists` check, thread it through `Request::Put`, add a live gate. Surfaced in `put --help` and here — never silently under-delivered.

## Self-Check: PASSED

- `crates/rdpilot-cli/src/verbs/file.rs` — FOUND
- `crates/rdpilot-cli/tests/cli_errors.rs` — FOUND
- `crates/rdpilot-cli/src/cli.rs` — FOUND (modified, `PutArgs`/`GetArgs`/`FileCmd` present)
- `crates/rdpilot-cli/src/exit_codes.rs` — FOUND (modified, `code_str_for` present)
- Commit `762833d` (`feat(13-07): put/get verbs — path absolutization, get no-clobber, put asymmetry`) — FOUND in `git log`
- Commit `6b1b9eb` (`test(13-07): finalize the CLI-03 error taxonomy, prove distinct exits offline`) — FOUND in `git log`
