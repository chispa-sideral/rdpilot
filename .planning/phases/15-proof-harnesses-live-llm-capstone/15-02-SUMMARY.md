---
phase: 15-proof-harnesses-live-llm-capstone
plan: 02
subsystem: rdpilot-cli (gated live proof/verb integration tests)
tags: [proof-02, cli-02, cli-03, offline-author, cargo-bin-exe, gated-live-test]
dependency_graph:
  requires: ["crates/rdpilot-cli/tests/cli_lifecycle.rs (CARGO_BIN_EXE_ + run_cli subprocess pattern)", "crates/rdpilot-cli/tests/cli_verbs.rs (CLI-02 offline verb assertions this extends live)", "crates/rdpilot-cli/tests/cli_errors.rs (D-28 exit-code taxonomy this re-asserts live)", "crates/rdpilot-cli/src/cli.rs + main.rs (verb/arg surface)", "crates/rdpilot/tests/common/mod.rs (RDPILOT_LIVE + .secrets/connection.json convention, re-implemented locally per D-17)"]
  provides: ["crates/rdpilot-cli/tests/live_proof.rs (PROOF-02 gated harness)", "crates/rdpilot-cli/tests/live_cli_verbs.rs (CLI-02/03 live re-exercise)"]
  affects: ["Plan 15-06 (runs live_cli_verbs.rs against the VM)", "Plan 15-07 (runs live_proof.rs against the VM)"]
tech_stack:
  added: []
  patterns: ["env!(\"CARGO_BIN_EXE_rdpilot\") + sibling rdpilot-daemon resolution, copied verbatim from cli_lifecycle.rs", "RDPILOT_DAEMON_TEST_CONNECTOR deliberately UNSET (opposite of the offline suites) so the auto-started daemon drives a real RDP session", "D-9.4 per-step PASS/FAIL trace + a final PROOF: PASS/FAIL line, mirroring examples/proof_harness.rs", "screenshot 'non-blank' proxy via before/after byte-variance instead of adding a PNG-decode dependency", "locally-duplicated .secrets/connection.json loader (rdpilot-cli must never depend on rdpilot, D-17)"]
key_files:
  created:
    - crates/rdpilot-cli/tests/live_proof.rs
    - crates/rdpilot-cli/tests/live_cli_verbs.rs
  modified: []
decisions:
  - "PROOF-02/CLI-02/CLI-03 live tests live as gated tests/*.rs in rdpilot-cli, NOT --example binaries -- env!(\"CARGO_BIN_EXE_rdpilot\") only resolves inside tests/benches targets (15-RESEARCH.md Pitfall 1, confirmed by this plan's own cargo test -- --list runs)."
  - "No new dependency added. The 'screenshot decodes to non-blank pixels' assertion (CLI-02) is satisfied via a before/after byte-variance + PNG-magic-number + minimum-size heuristic rather than adding a PNG-decoding crate -- rdpilot-cli has zero image-codec dependencies today and the plan's binding constraints preferred avoiding one. cargo tree -p rdpilot-cli confirmed unchanged and still free of ironrdp/rustls/rdpilot/rdpilot-daemon."
  - "The CLI-02 click-landing assertion targets whatever element in the 7-Zip::FM UIA children tree reports focusable == true (falling back to the first element if none do), rather than a hardcoded coordinate -- more robust to the real, unknown-until-live-run 7-Zip window layout than a fixed (x, y) guess would be."
  - "CLI-03's live payload is a deterministic ~8 MiB pseudo-random byte sequence (index-derived, no rand dependency), generated fresh per test run rather than a fixed fixture file, so the checksum assertion is never trivially satisfied by a stale cached artifact."
metrics:
  duration: "~55 min"
  completed: 2026-07-11
---

# Phase 15 Plan 02: PROOF-02 CLI Harness + CLI-02/03 Live Re-exercise Summary

Authored two gated `tests/*.rs` integration tests in `rdpilot-cli` -- `live_proof.rs` (PROOF-02's scripted CLI end-to-end proof harness) and `live_cli_verbs.rs` (the Phase-13-deferred CLI-02/03 live re-exercise) -- both spawning the REAL compiled `rdpilot`/`rdpilot-daemon` binaries against a real remote Windows target with the Phase 13 fake connector deliberately unset, compiled and verified offline on the Linux substitute target with no live run performed.

## What Was Built

**Task 1 -- `crates/rdpilot-cli/tests/live_proof.rs` (PROOF-02).** A single `#[ignore = "requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)"]` test, `cli_end_to_end_against_a_real_target`, that:

- Locally re-implements `crates/rdpilot/tests/common/mod.rs`'s `.secrets/connection.json` load/gate convention (`LIVE_ENV = "RDPILOT_LIVE"`, same JSON schema, same "unset env or missing file -> clean early return" D-18 semantics) -- `rdpilot-cli` cannot import that module directly since it never depends on `rdpilot` (D-17, thin-client invariant).
- Reuses `cli_lifecycle.rs`'s `env!("CARGO_BIN_EXE_rdpilot")` + sibling `rdpilot-daemon` resolution + real-file stdio capture (`Stdio::from(File)` + `.status()`, never `.output()`) verbatim, but with `RDPILOT_DAEMON_TEST_CONNECTOR` deliberately UNSET -- the opposite of every existing offline `rdpilot-cli` test -- so the auto-started daemon opens a genuine RDP session using `--host`/`--port`/`--username`/`--password`/`--accept-invalid-certs` sourced from the loaded live target.
- Drives the full black-box sequence: `connect --name proof-cli` (D-29 explicit name) -> `perceive screenshot --output <file>` (nonzero-size assertion) -> `input launch --exe "C:\Program Files\7-Zip\7zFM.exe"` -> a polled (up to 20x, 500ms apart) `perceive window list --json` until a `class_name == "7-Zip::FM"` entry appears -> `put` (parses the `--json` `{bytes_transferred, checksum}`) -> `get` (asserts the round-tripped checksum matches put's) -> `disconnect`.
- Asserts D-28's distinct-exit-code taxonomy end to end: `get --session does-not-exist ...` against a local destination that does not yet exist (so the CLI-side no-clobber short-circuit, exit 8, never fires first) must exit with `Some(2)` -- the `session-not-found` class -- never a generic `1`.
- Prints a D-9.4-style per-step `[PASS]`/`[FAIL]` trace (via a small `record` helper) plus a final `PROOF: PASS`/`PROOF: FAIL` line, mirroring `examples/proof_harness.rs`'s existing PROOF-01 style. The password is read into a local, handed straight to `Command::args`, and never appears in any `println!`/`format!`/`panic!` in this file (T-15-04).

**Task 2 -- `crates/rdpilot-cli/tests/live_cli_verbs.rs` (CLI-02/03 live re-exercise).** Two `#[ignore]` + `RDPILOT_LIVE`-gated tests, identically isolated/gated to `live_proof.rs` but kept in a separate file per the plan's binding constraint (so Plan 15-06's CLI-02/03 run and Plan 15-07's PROOF-02 run stay independently invokable):

- `cli_02_screenshot_and_click_landing_against_a_real_target` -- connects, launches 7-Zip File Manager, polls for its `7-Zip::FM` window (shared `connect_and_launch_7zip` helper), takes a "before" screenshot, fetches the window's UIA children (`perceive uia --scope children --json`), picks the first `focusable == true` element (falling back to the first element if none report focusable), computes its bounding-box center, clicks it (`input click --x --y`), re-fetches the UIA children and asserts some element now reports `focused == true`, then takes an "after" screenshot and asserts it differs byte-for-byte from the "before" capture (the realness proxy described below).
- `cli_03_multi_mb_put_get_round_trip_against_a_real_target` -- connects, generates a deterministic ~8 MiB local payload (`(i as u8).wrapping_mul(31).wrapping_add(7)`, no `rand` dependency), `put`s it, asserts the `--json` `bytes_transferred` equals the exact payload length, `get`s it back, asserts `bytes_transferred` again matches AND the checksum equals put's, and asserts the round-tripped file's actual on-disk size matches too (three independent size/checksum checks).
- Both print the same D-9.4 `record`-helper trace as `live_proof.rs`, ending in a `CLI-02 LIVE RE-EXERCISE: PASS/FAIL` / `CLI-03 LIVE RE-EXERCISE: PASS/FAIL` line.

**"Non-blank pixels" without a new dependency.** `rdpilot-cli` has no PNG/image-codec dependency, and the plan's binding constraints preferred not adding one just for a pixel-level sanity check. CLI-02's screenshot assertion instead checks (a) both captures start with the PNG magic number and exceed 1 KiB, and (b) the "before" and "after" captures are NOT byte-identical -- a fixed/canned image (like the offline `FakeTestSession`'s) could never satisfy (b); a genuinely live, changing desktop reliably does. This is documented in the file's own doc comment as a deliberate scope decision, not an oversight.

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build --workspace --target x86_64-unknown-linux-gnu` -- green (one pre-existing, unrelated warning in `crates/rdpilot/src/input.rs`, not touched by this plan).
- `cargo test -p rdpilot-cli --target x86_64-unknown-linux-gnu --test live_proof -- --list` -- lists `cli_end_to_end_against_a_real_target: test` (1 test, 0 benchmarks); compiles clean, no warnings.
- `cargo test -p rdpilot-cli --target x86_64-unknown-linux-gnu --test live_cli_verbs -- --list` -- lists both `cli_02_screenshot_and_click_landing_against_a_real_target: test` and `cli_03_multi_mb_put_get_round_trip_against_a_real_target: test` (2 tests, 0 benchmarks); compiles clean after fixing one `unused_assignments` warning.
- `cargo test -p rdpilot-cli --target x86_64-unknown-linux-gnu` (full offline suite, no `RDPILOT_LIVE`) -- all 9 unit tests + `cli_errors.rs` (3) + `cli_lifecycle.rs` (1) + `cli_verbs.rs` (1, +1 pre-existing `#[ignore]`) pass unchanged; `live_cli_verbs.rs`'s 2 tests and `live_proof.rs`'s 1 test all report `ignored, requires RDPILOT_LIVE=1 and a live Azure VM (.secrets/connection.json)` -- confirmed nothing new executes offline.
- `cargo tree -p rdpilot-cli --target x86_64-unknown-linux-gnu` -- unchanged from before this plan (no `[dev-dependencies]` section added, `Cargo.toml`/`Cargo.lock` untouched); grep for `ironrdp`/`rustls`/`rdpilot-daemon`/a bare `rdpilot` package found nothing -- thin-client invariant (D-17) intact.

**Toolchain substitution note:** built/tested via the native Linux substitute target (`RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu`, `--target x86_64-unknown-linux-gnu`) since the pinned `stable-x86_64-pc-windows-gnu` toolchain (per the workspace's `rust-toolchain.toml`, for the ARM64-Windows-host/no-MSVC situation described there) is not installed in this environment; only `stable-x86_64-unknown-linux-gnu` is available via `rustup toolchain list`. This mirrors the substitution already used and documented by 15-01.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 -- bug] Fixed an `unused_assignments` warning in `live_cli_verbs.rs`**
- **Found during:** Task 2 verification (`cargo test ... -- --list`)
- **Issue:** `click_detail`'s initial `String::new()` value was always overwritten before being read on every code path (every branch of the `if let Some(hwnd) = hwnd { ... } else { ... }` block sets it), so rustc flagged the initializer as dead.
- **Fix:** Added `#[allow(unused_assignments)]` on the `let mut click_detail = String::new();` line with a comment explaining the initializer is a defensive default only, never expected to survive to the final `record` call.
- **Files modified:** `crates/rdpilot-cli/tests/live_cli_verbs.rs`
- **Commit:** `c63fc75`

No other deviations -- both files were authored following the plan's action text and the research's PROOF-02 skeleton closely, with the CLI arg shapes (`--exe`, `--remote-name`, `--local`, etc.) taken directly from `cli.rs`'s actual clap definitions rather than the plan's loose shell-syntax example (`input launch ... "C:\...\7zFM.exe"` in prose vs. the real `--exe` flag) -- not a deviation from intent, just following the real, already-implemented CLI surface.

## Known Stubs

None -- both files are complete, gated, compile-checked test harnesses. No hardcoded empty values or placeholder rendering paths were introduced.

## Threat Flags

None beyond what the plan's own `<threat_model>` already names (T-15-04 password-never-printed, T-15-05 distinct-exit-code regression) -- both mitigations are implemented exactly as specified (no `Debug`/`Display` impl on `LiveTarget`, the D-28 exit-2 assertion in `live_proof.rs`). No new network endpoints, auth paths, or schema changes were introduced; this plan only adds test code that exercises existing, already-threat-modeled surfaces.

## What Remains (Live Runs)

- **Plan 15-06:** `RDPILOT_LIVE=1 cargo test -p rdpilot-cli --test live_cli_verbs -- --ignored` against the provisioned Azure VM -- exercises CLI-02 (real screenshot pixel variance + click landing verified via UIA) and CLI-03 (real ~8 MiB put/get, bytes_transferred + checksum match).
- **Plan 15-07:** `RDPILOT_LIVE=1 cargo test -p rdpilot-cli --test live_proof -- --ignored` against the same VM -- exercises PROOF-02's full connect -> screenshot -> launch -> window-list -> put -> get -> disconnect sequence plus the D-28 distinct-exit-code assertion.
- Neither test file has been executed against a live target yet; both were authored and compile-checked offline only, per this plan's OFFLINE-AUTHOR scope.

## Self-Check: PASSED

- FOUND: `crates/rdpilot-cli/tests/live_proof.rs`
- FOUND: `crates/rdpilot-cli/tests/live_cli_verbs.rs`
- FOUND commit `a3a9eef` (test(15-02): author PROOF-02 gated live CLI end-to-end proof harness)
- FOUND commit `c63fc75` (test(15-02): author CLI-02/03 live-deferred re-exercise)
