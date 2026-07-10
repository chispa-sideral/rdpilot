---
phase: 3
slug: input-injection
status: approved
nyquist_compliant: true
wave_0_complete: true
created: 2026-07-08
---

# Phase 3 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test harness (`#[test]` / `#[tokio::test]`); no external framework (same as Phase 2). Offline unit tests live inline (`#[cfg(test)] mod tests`) in `input.rs`/`session.rs`/`session_loop.rs`/`error.rs`; gated integration tests in `crates/rdpilot/tests/live_session.rs`. |
| **Config file** | None new — `crates/rdpilot/Cargo.toml` `[dev-dependencies]` only (`tokio` macros/rt-multi-thread, `serde_json`, optional `serial_test`); reused from Phase 2. No `Cargo.toml` change (no new packages — RESEARCH Package Legitimacy Audit). |
| **Quick run command** | `cargo test -p rdpilot` (offline unit tests only; live suite `#[ignore]`'d by default, D-18) |
| **Full suite command** | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` (requires a provisioned VM + `.secrets/connection.json`, per `tests/common/mod.rs`) |
| **Estimated runtime** | ~30 seconds quick (offline); several minutes full (live input round-trips + settle windows against one VM, single-threaded) |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p rdpilot` (offline unit + translation tests; no VM cost)
- **After every plan wave:** Run `cargo test -p rdpilot` + `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored` where a live VM is available (developer's local gated run, same D-18 pattern as Phase 2)
- **Before `/gsd-verify-work`:** Full suite green (offline + one canonical live run against a freshly-provisioned VM) must pass: `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1`
- **Max feedback latency:** ~30 seconds for the quick offline run

### Nyquist Compliance

`nyquist_compliant: true`. Justification (confirmed against RESEARCH § "Validation Architecture"):

- **Offline behaviors** (vocabulary shape, translation ordering, the Pitfall 1 wheel-magnitude split, the Pitfall 5 MouseMove-before-Wheel rule, D-3.5 modifier ordering, the coordinate bounds check, Session wiring/dispatch) can change on **every task commit**, and are sampled at exactly that rate by the offline unit suite (~30s latency). Sampling rate ≥ change rate → no aliasing.
- **Live-only behaviors** (SC#1 menu activation, SC#2 all mouse action types observably affecting the desktop, SC#3 typed text + combos received) have **no ack path — the framebuffer is the only observable** (RESEARCH Architecture step 7, D-3.4). They can only change when the input surface itself changes (a wave merge or a code change to `send_mouse`/`send_key`), and are sampled at the wave-merge and phase-gate live runs, which is at least as often as they can change.
- **Empirical LIVE-VERIFY items** (D-3.7 double-click gap, D-3.8 drag step count) are, by design, unresolvable offline (RESEARCH §4 — `GetDoubleClickTime` and `SM_CXDRAG` are per-VM/empirical). They are sampled at the canonical live run (Plan 04 Task 3), the only rate at which their true value is observable. No higher sampling rate exists, so no undersampling is possible.

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement / SC | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|------------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 3-01-01 | 01 | 1 | INPUT-01/02 (foundation), SC#4 | T-03-04 | Owned enums (D-3.1) with no `ironrdp`/`FastPathInputEvent`/`Scancode` leak (D-09); `Error::CoordinateOutOfBounds` mirrors `CropOutOfBounds` (D-3.2); no `unwrap`/`expect`/`panic` in library code (API-01) | unit | `cargo test -p rdpilot 2>&1 \| tail -15` | ✅ | ⬜ pending |
| 3-01-02 | 01 | 1 | INPUT-01/02 (foundation), SC#2, SC#3 | T-03-01, T-03-02, T-03-03, T-03-04 | Pure translation: Scroll splits `\|dy\|>255` before encode (Pitfall 1), always emits MouseMove before WheelRotations (Pitfall 5), routes keys through `Operation` never hand-built `KeyboardEvent` (Pitfall 2), D-3.5 reverse-release modifier ordering, degenerate Combo never panics (API-01) | unit | `cargo test -p rdpilot 2>&1 \| tail -20` | ✅ | ⬜ pending |
| 3-02-01 | 02 | 2 | INPUT-01, SC#4 | T-03-05, T-03-06, T-03-07 | `check_bounds` returns `Error::CoordinateOutOfBounds` before any PDU (D-3.2, SC#4 enforced); `Mutex<Database>` scoped to synchronous `apply()`, never held across `.await`, poison mapped to `Error::Session`; no `tokio::time::sleep` added to `session_loop.rs` (Pitfall 3) | unit | `cargo test -p rdpilot 2>&1 \| tail -20` | ✅ | ⬜ pending |
| 3-02-02 | 02 | 2 | INPUT-01, SC#2 | T-03-05, T-03-06, T-03-07, T-03-08 | `send_mouse` bounds-checks first (nothing sent on reject); all double-click/drag timing is `tokio::time::sleep` on the caller's context, never in the loop (Pitfall 3); owned-types-only public signature (D-09); coordinates/content never logged (Security V5) | unit (offline drain) | `cargo test -p rdpilot 2>&1 \| tail -20` | ✅ | ⬜ pending |
| 3-03-01 | 03 | 3 | INPUT-02, SC#3 | T-03-09, T-03-10, T-03-11 | `send_key` routes Type (Unicode) + Combo (scancode) entirely through `Operation`/`Database`, no hand-built `KeyboardEvent` (Pitfall 2); typed content never logged beyond a redacted summary (Security V5, D-14 parity); lock only for `apply()`; degenerate Combo returns Ok without panic (API-01) | unit (offline drain) | `cargo test -p rdpilot 2>&1 \| tail -20` | ✅ | ⬜ pending |
| 3-04-01 | 04 | 4 | INPUT-01, SC#1, SC#2, SC#4 | T-03-13 | `#[ignore]` + None-early-return gating keeps default offline `cargo test` green (D-18); public API only, no `ironrdp`/`image` import (D-09); screenshot-diff is the only observable (D-3.4); standard-integrity reversible targets only (Pitfall 4) | integration (gated) | `cargo test -p rdpilot 2>&1 \| tail -10 && cargo test -p rdpilot -- --include-ignored --list 2>&1 \| tail -30` | ✅ | ⬜ pending |
| 3-04-02 | 04 | 4 | INPUT-02, SC#3 | T-03-14 | Gated keyboard tests assert typed text only via screenshot-diff — never print/log the string (Security V5); public API only (D-09); default offline `cargo test` stays green (D-18); reversible standard-integrity targets, Alt+F4 dialog cancelled with Esc | integration (gated) | `cargo test -p rdpilot 2>&1 \| tail -10 && cargo test -p rdpilot -- --include-ignored --list 2>&1 \| tail -30` | ✅ | ⬜ pending |
| 3-04-03 | 04 | 4 | INPUT-01/02, SC#1, SC#2, SC#3, SC#4 | T-03-12, T-03-15, T-03-16 | Canonical live run: every Phase-3 input test passes (NOT skipped) against a real VM; double-click gap (D-3.7) + drag step-count (D-3.8) empirically confirmed (RESEARCH §4); all targets standard-integrity (Pitfall 4); VM torn down (`manage-env.ps1 down`) | integration (live, checkpoint) | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*
*File Exists: the automated command's target file/module is defined by the owning plan (`input.rs` is created in Plan 01; `session.rs`/`session_loop.rs`/`error.rs`/`lib.rs`/`live_session.rs`/`common/mod.rs` already exist from Phases 1-2).*

---

## Wave 0 Requirements

Phase 3 reuses Phase 2's test infrastructure verbatim — no new framework, config, or fixture is required (RESEARCH § "Wave 0 Gaps"). The Wave 0 preconditions are therefore already satisfied:

- [x] Rust test harness + `[dev-dependencies]` (`tokio` macros/rt, `serde_json`, optional `serial_test`) — established in Phase 2, unchanged
- [x] Shared gated test helper `crates/rdpilot/tests/common/mod.rs` (`load_config` / `RDPILOT_LIVE` / `idle_secs`) — reused as-is (D-3.4 says "extend," not replace)
- [x] Gated integration suite `crates/rdpilot/tests/live_session.rs` (`block_on`, `capture_when_ready`, `require_target!`, `is_blank`, `is_uniform_grey`) — extended by Plan 04, not recreated
- [x] Offline unit-test convention — inline `#[cfg(test)] mod tests` in `input.rs`/`session.rs`/`session_loop.rs`/`error.rs` (standard crate pattern; no new file, no new fixture)

No MISSING references: every offline behavior has an inline unit test in its owning plan; every live behavior has a gated integration test plus the canonical checkpoint. No package installs are introduced (`ironrdp-input`/`ironrdp-pdu` already pinned in `Cargo.lock`).

---

## Manual-Only Verifications

The Plan 04 checkpoint (`03-04` Task 3, `checkpoint:human-verify`, `gate="blocking"`) is the phase Nyquist gate. It covers the behaviors that cannot be proven offline plus the two empirical live-verify tuning items.

| Behavior | Requirement / SC | Why Manual / Live-Only | Test Instructions |
|----------|------------------|------------------------|-------------------|
| Canonical live input validation against the Phase 1 VM | INPUT-01/02, SC#1/#2/#3/#4 | Requires a provisioned live Windows RDP target; SC#1/#2/#3 have no ack path — the framebuffer (screenshot-diff) is the only observable (D-3.4) | `infra/manage-env.ps1 up`, then `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1`; confirm every Phase-3 input test **passed (not skipped)**: `mouse_click_activates_menu`, `mouse_action_types_all_work`, `coordinate_contract_enforced`, `keyboard_typed_text_is_received`, `keyboard_combos_are_received`; then `infra/manage-env.ps1 down` |
| **Double-click gap (D-3.7) empirical confirmation** | INPUT-01, SC#2 | `GetDoubleClickTime` default is 500ms but is per-VM/user-configurable; whether a ~100ms-spaced two-batch DoubleClick registers as a double-click (not two singles) is only observable live (RESEARCH §4 — flagged LOW/unresolvable offline by design) | During the canonical run, confirm a DoubleClick reliably triggers double-click behavior on the VM. If flaky, increase `DOUBLE_CLICK_GAP` in `session.rs`, re-run, and record the final tuned value in the SUMMARY |
| **Drag step-count (D-3.8) empirical confirmation** | INPUT-01, SC#2 | `SM_CXDRAG` is a per-move distance threshold (not an event count) and is empirical per VM; whether ~5 interpolated moves register a drag is only observable live (RESEARCH §4 — flagged LOW/unresolvable offline by design) | During the canonical run, confirm a Drag visibly registers (icon moves / selection rectangle draws). If not, increase the drag step count in `input.rs` / the `DRAG_STEP_GAP` in `session.rs`, re-run, and record the final tuned value in the SUMMARY |

*All offline behaviors (owned-type shape, translation ordering, wheel-magnitude split, MouseMove-before-Wheel, D-3.5 modifier ordering, the coordinate bounds check, Session `send_mouse`/`send_key` channel wiring) have automated verification in the default `cargo test -p rdpilot` run. SC#4's rejection half is proven offline (bounds-check unit test, 3-02-01) and its `desktop_size()` physical-pixel half is confirmed live (3-04-01).*

---

## Validation Sign-Off

- [x] All tasks have an `<automated>` verify (or, for the live checkpoint, a `<human-check>` command) or a satisfied Wave 0 dependency
- [x] Sampling continuity: no 3 consecutive tasks without automated verify (every offline task runs `cargo test -p rdpilot`)
- [x] Wave 0 covers all references (Phase-2 harness reused; no MISSING items; no new packages/config/fixture)
- [x] No watch-mode flags
- [x] Feedback latency < 30s (quick offline run)
- [x] Live-only behaviors (SC#1/#2/#3) and the two empirical LIVE-VERIFY items (D-3.7/D-3.8) sampled at the phase-gate canonical run — the only rate at which they are observable
- [x] Every entry mapped to the 4 ROADMAP success criteria and to INPUT-01/INPUT-02
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** approved 2026-07-08
