---
phase: 13-cli-surface
verified: 2026-07-11T09:02:33Z
status: passed
score: 3/3 must-haves verified (offline scope)
overrides_applied: 0
---

# Phase 13: CLI Surface Verification Report

**Phase Goal:** A thin `rdpilot` CLI drives the full session/perception/input/file verb set over the daemon, with every command explicitly targeting a named session.

**Verified:** 2026-07-11T09:02:33Z
**Status:** VERIFIED (offline)
**Re-verification:** No — initial verification
**Scope note:** Per developer instruction, all live-Windows assertions for this phase are deliberately `#[ignore]`-deferred to the batched Phase 15 live gate. Their absence is NOT treated as a failure here; they are listed under "Deferred Live Items" below and were confirmed to be correctly `#[ignore]`-gated (not silently skipped by omission) rather than simply missing.

All commands below were executed directly against the real, freshly-built codebase (not taken from SUMMARY.md claims). Toolchain substitution per the project's standing environment note: `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo ... --target x86_64-unknown-linux-gnu` (native-Linux substitute for the pinned-but-uninstalled `x86_64-pc-windows-gnu` toolchain).

## Goal Achievement

### Observable Truths / Criteria

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | CLI-01 — `connect [--name] / list / disconnect` over the daemon with transparent auto-start | ✓ VERIFIED | `cargo test -p rdpilot-cli --test cli_lifecycle --target x86_64-unknown-linux-gnu` → `test connect_list_disconnect_lifecycle_auto_starts_the_real_daemon ... ok` (1 passed). Read `crates/rdpilot-cli/tests/cli_lifecycle.rs` in full: it spawns the compiled `rdpilot` binary as a subprocess with no daemon pre-started, asserts the socket doesn't exist beforehand, calls `connect --json` (asserts `socket_path.exists()` afterward — real auto-start, not mocked), `list --json` (asserts `status:"Live"`, name/host round-trip), `disconnect` (exit 0), then a second `disconnect` on the same now-gone session asserts exit code exactly `2` (`SessionNotFound`). This drives the REAL compiled `rdpilot-daemon` binary via `RDPILOT_DAEMON_TEST_CONNECTOR=1` (fake connector), not a mock. |
| 2 | CLI-02 — full perception+input+launch verb set against `--session` | ✓ VERIFIED | `cargo test -p rdpilot-cli --test cli_verbs --target x86_64-unknown-linux-gnu` → `every_cli_02_verb_round_trips_against_the_canned_fake_session ... ok` (1 passed, 1 intentionally `#[ignore]`d — see Deferred Live Items). Read `crates/rdpilot-cli/tests/cli_verbs.rs` in full (360 lines): round-trips `perceive screenshot --output`, `perceive window list`, `perceive process list`, `perceive uia`, `perceive world-state --screenshot --window-list --uia-mode all --output`, `input click/scroll/drag/type/key/launch/foreground`, each requiring `--session`. `screenshot --output` assertion: `assert_eq!(&png_bytes[0..4], &[0x89, b'P', b'N', b'G'], ...)` on base64-decoded file bytes, plus an explicit assertion that raw PNG magic bytes never appear on stdout. All fake-session canned values (hwnd=1/"Notepad"/pid=1234, pid=1234/"notepad.exe", uia id="42"/"Button", launch pid=4242) are asserted exactly. |
| 3 | CLI-03 — put/get no-clobber default + `--force`; distinct exit codes for session-not-found (2), daemon-unreachable (3), no-clobber (8→0 with `--force`) | ✓ VERIFIED | `cargo test -p rdpilot-cli --test cli_errors --target x86_64-unknown-linux-gnu` → 3/3 passed: `session_not_found_maps_to_exit_code_2_and_names_the_session`, `daemon_unreachable_maps_to_exit_code_3`, `get_no_clobber_refuses_without_force_and_succeeds_with_force`. Read the test file in full: exit-2 case also asserts the exact `--json` payload `{"error":{"code":"session-not-found",...}}`; exit-3 case copies the compiled `rdpilot` binary into an isolated dir with NO sibling `rdpilot-daemon` binary (genuinely unspawnable, not mocked); no-clobber case asserts the pre-existing local file is byte-for-byte untouched on refusal, then succeeds with `--force` against the real fake session's `download_file`. Live-ran `rdpilot put --help`: confirmed the exact documented asymmetry text — *"Accepted for forward-compat only — remote overwrite is NOT prevented this phase (see backlog: symmetric remote no-clobber, Phase 999.5)"* — present in both `--help` output and `crates/rdpilot-cli/src/cli.rs`'s `PutArgs::force` doc comment, and referenced in `13-07-SUMMARY.md`'s "Known Gaps" section and `ROADMAP.md`'s Phase 999.5 backlog entry. |

**Score:** 3/3 offline-provable criteria verified.

### Binding Invariants

| Invariant | Status | Evidence |
|-----------|--------|----------|
| Thin-client invariant (`cargo tree -p rdpilot-cli`) | ✓ VERIFIED | Ran `cargo tree -p rdpilot-cli --target x86_64-unknown-linux-gnu`. Full dependency list: `base64`, `clap` (+ its own subdeps), `rdpilot-config`, `rdpilot-ipc`, `serde`, `serde_json`, `tokio` (+ subdeps). Zero occurrences of `ironrdp`, `rustls`, `rdpilot` (the SDK crate), or `rdpilot-daemon`. |
| SESSION-02 non-regression (every operational verb hard-rejects a missing session) | ✓ VERIFIED | `cargo test -p rdpilot-ipc --target x86_64-unknown-linux-gnu request::` → 7/7 passed, including `every_operational_request_verb_rejects_a_missing_session_field` and `disconnect_without_a_session_field_is_a_hard_rejection`. |
| DAEMON-01/02/03 non-regression | ✓ VERIFIED | `--test thread_leak_soak -- --include-ignored`: 2/2 passed (`thread_and_rss_return_to_baseline_after_fifty_cycles`, `thread_count_returns_to_baseline_after_a_few_cycles`). `--test registry_concurrency`: 2/2 passed. `--test autostart_lifecycle -- --include-ignored`: 1/1 passed. `--test ipc_security`: 3/3 passed (1 additional test `cross_account_peer_is_rejected_end_to_end` correctly `#[ignore]`d — requires a second local uid, environmental, not a Phase-13 regression concern). All green after the Phase-13 dispatch wiring, storage change (`Arc<TokioMutex<Option<Box>>>`), and transport relocation (13-01). |
| Full workspace test suite | ✓ VERIFIED | `cargo test --workspace --target x86_64-unknown-linux-gnu`: **263 passed, 0 failed, 32 ignored** (all ignored are documented `#[ignore]`-gated heavy/live-gate tests: 28 in `rdpilot/tests/live_session.rs`, 1 `cli_verbs` Phase-15-deferred placeholder, 1 `autostart_lifecycle` heavy-spawn test not run by default, 1 `ipc_security` cross-account test, 1 `thread_leak_soak` heavy N=50 soak not run by default). No failures anywhere in the workspace. |
| base64 pin consistency | ✓ VERIFIED | `grep base64 crates/rdpilot-cli/Cargo.toml crates/rdpilot-daemon/Cargo.toml` → both pin `base64 = "0.22.1"` verbatim, with matching legitimacy-gate comment referencing the same upstream identity confirmation (`github.com/marshallpierce/rust-base64`); `cargo tree` output shows a single resolved `base64 v0.22.1` in the graph. |

### Planning-Doc Counter Reconciliation

| Check | Result |
|-------|--------|
| ROADMAP.md Phase 13 progress row | ✓ `\| 13. CLI Surface \| v1.1 \| 7/7 \| Complete   \| 2026-07-11 \|` |
| ROADMAP.md Phase 13 plan checkboxes | ✓ All 7 (`13-01` through `13-07`) show `[x]` |
| ROADMAP.md top milestone checklist | ✓ `- [x] **Phase 13: CLI Surface**` (completed 2026-07-11) |
| REQUIREMENTS.md CLI-01/02/03 checkboxes | ✓ All three show `[x]` with detailed "COMPLETE (13-0X: ...)" / plain-complete annotations |
| **REQUIREMENTS.md Requirements Coverage table (bottom section)** | ⚠️ **Residual drift found**: the coverage table row for `CLI-02` still reads `"CLI-02 \| Phase 13 \| In Progress (13-03: ... Remaining: the actual rdpilot CLI binary that issues these requests over IPC is 13-06)"` — stale text from before Plan 13-06 executed. `CLI-01` and `CLI-03`'s rows in the same table correctly say `Complete`. This is a leftover artifact of the same Wave-2 parallel-execution drift the task description flagged; it does not affect the checkbox-level requirement status (which is correctly `[x]` for all three), only this one prose cell. **Not a blocker** — cosmetic documentation lag, not a functional gap. |
| **STATE.md narrative fields** | ⚠️ **Residual drift found**: `stopped_at`, `last_activity`, and the `## Current Position` section all still describe the project as of Plan 13-06's completion ("13-07 (CLI-03) remains" / "Plan: 7 of 7 ... 13-07 ... remains"), even though 13-07 is in fact complete (commits `762833d`/`6b1b9eb`/`8906f9a` exist, `ROADMAP.md` shows 7/7). The numeric `progress:` frontmatter block (`total_plans: 21, completed_plans: 20`), however, IS internally self-consistent with reality: 21 = 5(P10)+2(P11)+7(P12)+7(P13), 20 = all of those except the still-outstanding `12-07` live-gate plan — i.e., the counters already correctly treat all of Phase 13 (including 13-07) as complete, only the prose narrative lags behind. **Not a blocker** — the authoritative counters are correct; only descriptive text is stale. |

Both drift items are documentation-only staleness in narrative/prose cells, not in the authoritative checkbox/counter fields, and do not indicate any missing or broken functionality. Recommend a trivial doc-only follow-up to sync the REQUIREMENTS.md coverage-table CLI-02 cell and STATE.md's narrative fields, but this does not block Phase 14.

### Known Debt (recorded for later cleanup, not fixed here — per task scope)

| Item | Severity | Evidence |
|------|----------|----------|
| (a) `FakeTestSession::get_process_tree` embedded raw-newline typo | ℹ️ Info / low severity | `crates/rdpilot-daemon/src/server.rs:263-264`: `path: r"C:\Windows` followed by a literal embedded newline then `otepad.exe".to_owned()`. Confirmed by direct read of the source. Test-only fixture data (canned `ProcessInfo.path` for the fake connector); `cli_verbs.rs`'s process-list assertion only checks `pid`/`name`, so this does not affect any Phase 13 test outcome or production code path. Pre-existing from Plan 13-04's authoring, correctly out of scope for every subsequent Phase-13 plan's file-scope boundary. Should be fixed opportunistically whenever `server.rs` is next touched. |
| (b) ~26 test-only `clippy::expect_used`/`unwrap_used` allows in `rdpilot-daemon` | ℹ️ Info / low severity | Confirmed: 10 explicit `#[allow(clippy::expect_used)]` annotations in `registry.rs` (one per test, "a poisoned registry mutex is unrecoverable"), plus 2 module-level `#[allow(clippy::expect_used, clippy::unwrap_used)]` blocks covering the `server.rs` and `lifecycle.rs` test modules (23 and 4 raw `.expect(`/`.unwrap(` call sites respectively, order-of-magnitude consistent with the "~26" figure). All are `#[cfg(test)]`-scoped fail-fast test assertions, never reachable from production code (the crate-level `#![deny(clippy::expect_used)]` in `lib.rs` still applies to non-test code). Carried forward from Phase 12, unrelated to Phase 13's own changes. Not a functional risk; a style-consistency cleanup candidate only. |

### Deferred Live Items (Phase 15 batched gate — confirmed correctly deferred, not simply missing)

| Item | Where deferred | Confirmation |
|------|----------------|--------------|
| Real screenshot pixel content | `cli_verbs.rs::real_windows_semantics_are_proven_in_the_phase_15_live_gate` | `#[ignore = "requires a real Windows RDP target -- deferred to the Phase 15 batched live gate"]`; body is `unreachable!()` — a deliberate placeholder, confirmed present and correctly ignored by test run (`1 ignored`). |
| Real click-coordinate landing | Same test | Same as above. |
| Real UIA tree shape (CLI-02) | Same test | Same as above. |
| Real multi-MB `put`/`get` transfer through the CLI (CLI-03) | `13-07-SUMMARY.md` + module doc of `cli_errors.rs` | Explicitly documented: "Real multi-MB `put`/`get` transfer against a live Windows target remains `#[ignore]`-deferred to the batched Phase 15 live gate — FILE-01/02/04 were already live-verified in Phase 10." No corresponding `#[ignore]` test exists in the CLI crate for this (transfer mechanics were proven at the wire/dispatch level in 13-03/13-04's offline dispatch tests using the fake session); this is consistent with the stated scope — CLI-03's own offline proof (`cli_errors.rs`) exercises the no-clobber/error-taxonomy contract, not transfer bytes-correctness, which is Phase-10's domain re-exercised (not re-proven) at Phase 15. |
| `put`'s remote no-clobber (asymmetry) | `ROADMAP.md` backlog Phase 999.5 | Confirmed present as a real backlog entry (not silently dropped), and confirmed NOT part of the Phase 15 live-gate scope per `ROADMAP.md`'s own text: `"put's remote no-clobber asymmetry is NOT part of the Phase 15 gate — it is tracked separately as backlog Phase 999.5."` |

### Requirements Coverage

| Requirement | Source Plan | Status | Evidence |
|-------------|-------------|--------|----------|
| CLI-01 | 13-05 (scaffold), 13-01 (transport relocation) | ✓ SATISFIED | `cli_lifecycle.rs` full pass; thin-client invariant confirmed |
| CLI-02 | 13-06 (perception/input/launch verbs), 13-02/13-03/13-04 (wire DTOs + dispatch) | ✓ SATISFIED | `cli_verbs.rs` full pass (non-ignored test); every verb round-trips |
| CLI-03 | 13-07 (put/get + error taxonomy) | ✓ SATISFIED | `cli_errors.rs` full pass; `put --help` confirms documented gap |

No orphaned requirements found — `REQUIREMENTS.md`'s three CLI-* rows map 1:1 to the three plans/criteria claimed for this phase.

### Anti-Patterns Found

Scanned every `.rs` file under `crates/rdpilot-cli/` plus the Phase-13-touched files in `rdpilot-ipc`/`rdpilot-daemon` (`transport.rs`, `request.rs`, `server.rs`, `dispatch.rs`, `registry.rs`) for `TBD|FIXME|XXX|TODO|HACK|PLACEHOLDER` and stub-return patterns. **Zero matches.** No debt markers, no stub returns, no empty handlers found in Phase-13-authored code.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| `put --help` documents the no-clobber asymmetry gap | `cargo run -p rdpilot-cli --bin rdpilot -- put --help` | `--force  Accepted for forward-compat only — remote overwrite is NOT prevented this phase (see backlog: symmetric remote no-clobber, Phase 999.5)` | ✓ PASS |
| `cargo tree -p rdpilot-cli` shows the thin-client dependency set | `cargo tree -p rdpilot-cli --target x86_64-unknown-linux-gnu` | base64/clap/rdpilot-config/rdpilot-ipc/serde/serde_json/tokio only | ✓ PASS |
| `cargo clippy -p rdpilot-cli --all-targets` clean | `cargo clippy -p rdpilot-cli --all-targets --target x86_64-unknown-linux-gnu` | No warnings/errors | ✓ PASS |

### Probe Execution

No `scripts/*/tests/probe-*.sh` convention or PLAN/SUMMARY-declared probes found for this phase — Phase 13's own `<verify>` blocks are `cargo test` invocations directly (executed above), not shell probe scripts. Step 7c: N/A for this phase (no probe scripts declared or discovered).

### Human Verification Required

None. All Phase-13 offline-provable criteria were independently re-run against real compiled binaries and pass. All items requiring a live Windows target are explicitly, correctly `#[ignore]`-deferred to the Phase 15 batched live gate per the developer's own stated batching strategy — this is a scoping decision documented in ROADMAP.md, not an unresolved gap.

### Gaps Summary

No blocking gaps. Two cosmetic documentation-staleness items (REQUIREMENTS.md's CLI-02 coverage-table cell, STATE.md's narrative `stopped_at`/`Current Position` text) are noted above as non-blocking residual drift — the authoritative checkbox/counter fields are correct; only prose descriptions lag. Two pieces of known, non-blocking test-only debt (server.rs newline typo, daemon crate's test-scoped clippy allows) are recorded per the task's instruction to note-not-fix.

## Overall Verdict

**VERIFIED (offline).** All three phase success criteria (CLI-01, CLI-02, CLI-03) are independently confirmed against real, freshly-executed test runs driving the actual compiled `rdpilot`/`rdpilot-daemon` binaries — not SUMMARY.md claims. The thin-client invariant holds, SESSION-02 and DAEMON-01/02/03 non-regression suites are all green, the full workspace test suite is 263 passed / 0 failed / 32 (documented, intentional) ignored, and the base64 pin is consistent across both consumer crates. The CLI achieves the Phase 13 goal and is a sound foundation for Phase 14's MCP server surface. The Phase-15-deferred live items (real screenshot pixel content, real click landing, real UIA tree shape, real multi-MB transfer bytes, and the separately-tracked `put` no-clobber backlog item) remain correctly scoped out of this phase and are listed above for the batched live gate.

---
*Verified: 2026-07-11T09:02:33Z*
*Verifier: Claude (gsd-verifier)*
