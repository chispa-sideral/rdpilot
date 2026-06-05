---
phase: 2
slug: rdp-session-framebuffer-core
status: approved
nyquist_compliant: true
wave_0_complete: true
created: 2026-06-05
---

# Phase 2 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test harness (`#[test]` / `#[tokio::test]`); integration tests in `crates/rdpilot/tests/`. `serial_test` optional (live tests must not run concurrently against one VM). |
| **Config file** | `crates/rdpilot/Cargo.toml` `[dev-dependencies]` (`tokio` macros/rt, optional `serial_test`) |
| **Quick run command** | `cargo test -p rdpilot` (unit + crop math; live tests gated/skipped when no target — D-18) |
| **Full suite command** | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` (full 10-min idle included) |
| **Estimated runtime** | ~30 seconds quick; ~11+ minutes full (includes the canonical 10-min idle window) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p rdpilot` (unit + crop math; no VM cost)
- **After every plan wave:** Run `cargo test -p rdpilot` + a short-idle live smoke (parameterized idle, not full 10 min)
- **Before `/gsd-verify-work`:** Full suite with the full 10-min idle must be green: `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1`
- **Max feedback latency:** ~30 seconds for the quick run (offline)

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 2-01-01 | 01 | 1 | D-19 (toolchain) | T-02-SC | Rust toolchain present via scoop (Wave 0 prerequisite) | build | `cargo --version` | ✅ | ⬜ pending |
| 2-01-02 | 01 | 1 | SESS-01, CAP-01 | T-02-SC | Workspace resolves with corrected versions; `Cargo.lock` committed (auditable transitive graph) | build | `cargo build 2>&1 \| tail -5; test -f Cargo.lock && echo "Cargo.lock present"` | ✅ | ⬜ pending |
| 2-01-03 | 01 | 1 | CAP-01 | T-02-05 / API-01 | Bounds-checked crop returns `Error`, never panics; owned `Screenshot` type | unit | `cargo test -p rdpilot 2>&1 \| tail -15` | ✅ | ⬜ pending |
| 2-02-01 | 02 | 2 | SESS-01 | T-02-02/03/04 | Cert policy by flag; resumption disabled; no credential/cert logging; DVC seam present | unit | `cargo test -p rdpilot 2>&1 \| tail -15` | ✅ | ⬜ pending |
| 2-02-02 | 02 | 2 | SESS-02, CAP-01 | T-02-05/06 | Snapshot-on-GraphicsUpdate (no live image shared); DeactivateAll rebuild; keepalive is pointer-move (never keystroke) | unit | `cargo test -p rdpilot 2>&1 \| tail -15` | ✅ | ⬜ pending |
| 2-02-03 | 02 | 2 | SESS-02 | T-02-07 / API-01 | `close()` graceful + `Drop` guard aborts task; owned types only; no panics | unit | `cargo build -p rdpilot 2>&1 \| tail -5 && cargo test -p rdpilot 2>&1 \| tail -10` | ✅ | ⬜ pending |
| 2-03-01 | 03 | 3 | SESS-01, CAP-01 | — | Gated config loader skips when `.secrets/connection.json` absent (D-18); example compiles | integration (gated) | `cargo build -p rdpilot --examples 2>&1 \| tail -5 && cargo test -p rdpilot 2>&1 \| tail -10` | ✅ | ⬜ pending |
| 2-03-02 | 03 | 3 | SESS-01, SESS-02, CAP-01 | T-02-05 | Five `#[ignore]`'d criterion tests; default offline `cargo test` stays green (D-18); RGB known-pixel + behavioral idle-render | integration (gated) | `cargo test -p rdpilot 2>&1 \| tail -10 && cargo test -p rdpilot -- --include-ignored --list 2>&1 \| tail -20` | ✅ | ⬜ pending |
| 2-03-03 | 03 | 3 | SESS-01, SESS-02, CAP-01 | — | Canonical live run: 5 passed / 0 failed (NOT skipped) against the Phase 1 VM, full 10-min idle | integration (live, checkpoint) | `RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [x] Rust toolchain install (`scoop install rustup` → `rustup toolchain install stable`, verify `rustc --version` ≥ 1.78) — D-19 hard blocker; without it nothing builds (Plan 01 Task 1)
- [x] `crates/rdpilot/Cargo.toml` `[dev-dependencies]` — `tokio` (macros, rt), optionally `serial_test` (Plan 01 Task 2)
- [x] Offline crop/to_png unit tests — pure buffer math, runs without a VM (`tests/crop.rs` or `#[cfg(test)]` in `screenshot.rs`) (Plan 01 Task 3)
- [x] Shared gated test helper (`crates/rdpilot/tests/common/mod.rs`) — loads `.secrets/connection.json` into `ConnectionConfig`, returns `None`/skips when absent (D-18) (Plan 03 Task 1)
- [x] `crates/rdpilot/tests/live_session.rs` — `#[ignore]`'d gated integration suite covering SESS-01, CAP-01, SESS-02 (all 5 criteria) (Plan 03 Task 2)

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Canonical live validation against the Phase 1 VM (phase Nyquist gate) | SESS-01/SESS-02/CAP-01 | Requires a provisioned live Windows RDP target (cost/time); the only place criteria #1/#2/#4/#5 are meaningfully exercised | `infra/manage-env.ps1 up`, then `RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1`; confirm 5 passed / 0 failed (not skipped); optionally eyeball the saved PNG (Plan 03 Task 3, `checkpoint:human-verify`) |

*All offline behaviors (cert-policy branch, FrameSnapshot round-trip, crop math, PNG encode, public API shape, no-panic policy) have automated verification in the default `cargo test -p rdpilot` run.*

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references (toolchain, dev-deps, offline unit tests, gated loader, live suite)
- [x] No watch-mode flags
- [x] Feedback latency < 30s (quick offline run)
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** approved 2026-06-05
