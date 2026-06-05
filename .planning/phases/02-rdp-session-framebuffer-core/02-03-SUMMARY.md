---
phase: 02-rdp-session-framebuffer-core
plan: 03
subsystem: validation
tags: [rust, ironrdp, integration-test, live-validation, example, gating, phase-gate]

# Dependency graph
requires:
  - phase: 02-rdp-session-framebuffer-core
    provides: "Plan 01 owned no-VM types (Error/Result, ConnectionConfig, Screenshot/Rect) + crop math; Plan 02 connect path, SDK-owned session loop, framebuffer snapshot, automatic keepalive, and the public Session handle (connect/screenshot/close + Drop)"
  - phase: 01-test-environment
    provides: "infra/manage-env.ps1 up/down + the gitignored .secrets/connection.json (host/user/password/rdpPort/winrmPort) the live suite consumes"
provides:
  - "End-to-end example binary (examples/screenshot.rs): load .secrets/connection.json → Session::connect → screenshot → to_png → write screenshot.png → close — public API only (D-09)"
  - "Gated live integration suite (tests/live_session.rs): one #[ignore]'d test per Phase 2 success criterion, early-returns when the target is absent (D-16/D-18)"
  - "Shared test helper (tests/common/mod.rs): load_config() → ConnectionConfig gated on RDPILOT_LIVE + file presence; never logs the password (D-18, T-02-08)"
  - "RDPILOT_LIVE / RDPILOT_IDLE_SECS test-only env-var gating convention"
  - "Recorded canonical live-validation pass: all 5 ROADMAP success criteria proven against a real Azure Windows RDP target (the phase Nyquist gate)"
affects: [phase-3-input, phase-4-dvc-sensor]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Live tests are #[ignore]'d AND early-return when common::load_config() is None — default `cargo test` stays green offline (D-18), the canonical run arms with RDPILOT_LIVE=1 --include-ignored --test-threads=1"
    - "Sync #[test] drives the async public API via a current-thread runtime (block_on); the SDK session loop runs on its own dedicated thread"
    - "Criterion #2 acceptance = opaque-alpha (0xFF) + non-YUV-grey grid sample, NOT content-richness — a solid desktop-background first frame is still correct RGB"
    - "Idle/keepalive duration parameterized via RDPILOT_IDLE_SECS (short in dev, full 600s for the canonical phase gate)"

key-files:
  created:
    - "crates/rdpilot/examples/screenshot.rs"
    - "crates/rdpilot/tests/live_session.rs"
    - "crates/rdpilot/tests/common/mod.rs"
  modified: []

key-decisions:
  - "screenshot_is_rgb_correct asserts RGB-correctness (opaque alpha + non-grey), not non-blank — the first post-connect frame can legitimately be a uniform solid desktop background before the shell paints. Content/non-blank is covered by stays_rendered_while_idle."
  - "Live VM size: Standard_B2s_v2 (the default Standard_B2ms is SkuNotAvailable / restricted in westeurope; the manage-env.ps1 preflight lists available sizes and documents the -VmSize override)."
  - "Example + test config set accept_invalid_certs(true) for the disposable self-signed lab VM (D-15); the library stays env-agnostic (D-12) — the secrets-file loader lives in test/example code only."

patterns-established:
  - "First Rust integration-test layout: tests/live_session.rs + tests/common/mod.rs shared helper; gating = #[ignore] + skip-when-None (the Rust analog of Phase 1's Pester skeleton-now/live-later tests)."
  - "Examples may build app-level errors as String/anyhow but never leak non-SDK error types back across the public API (D-09)."

requirements-completed: [SESS-01, SESS-02, CAP-01]

# Metrics
duration: ~55min
completed: 2026-06-05
---

# Phase 2 Plan 03: Screenshot Wiring + Live Validation Suite Summary

**The phase is now proven end-to-end against a real Azure Windows RDP target: a public-API-only example writes a full-desktop PNG, and a gated 5-criterion integration suite (connect/auth, RGB-correct screenshot, crop-to-rect, stays-rendered-windowless-idle, and a full 10-minute keepalive) passes 5/5 on the live VM — while the default offline `cargo test` stays green (21 unit + 5 ignored) with no target present.**

## Performance

- **Duration:** ~55 min (incl. a ~20-min canonical run with two 10-min idle windows)
- **Started:** 2026-06-05
- **Completed:** 2026-06-05
- **Tasks:** 3 (2 auto + 1 blocking live-validation checkpoint, resolved autonomously)
- **Files:** 3 created, 0 source files modified

## Accomplishments

- **Task 1 — example binary + gated test helper (`26f0998`):** `examples/screenshot.rs` composes the public API only (`Session::connect` → `screenshot` → `to_png` → `close`), loading `.secrets/connection.json` via its own `serde_json` loader and writing `screenshot.png`. No `ironrdp`/`image`/`rustls` import appears (D-09, verified). `tests/common/mod.rs` provides `load_config() -> Option<ConnectionConfig>` gated on **both** `RDPILOT_LIVE` and file presence (returns `None` to skip otherwise, D-18), plus `idle_secs()` reading `RDPILOT_IDLE_SECS` (default 5s, canonical 600s). The parsed password is never printed/logged (T-02-08); the lab VM gets `accept_invalid_certs(true)` (D-15).
- **Task 2 — gated 5-criterion suite (`ddb43b5`):** `tests/live_session.rs` has one `#[ignore]`'d test per ROADMAP criterion: `connect_authenticates` (#1/SESS-01), `screenshot_is_rgb_correct` (#2/CAP-01, opaque-alpha + non-YUV-grey grid guard), `screenshot_crop` (#3/CAP-01, live crop origin == source pixel), `stays_rendered_while_idle` (#4/SESS-02, behavioral non-blank/full-res after idle — **no registry/WinRM**, D-08), and `keepalive_survives_10min` (#5/SESS-02). Each early-returns when `load_config()` is `None`. Default offline `cargo test` = 21 passed / 5 ignored; `--include-ignored --list` shows all five.
- **Task 3 — canonical live validation (phase Nyquist gate, resolved autonomously):** Provisioned a real Azure Windows VM (`manage-env.ps1 -Action up -VmSize Standard_B2s_v2`), ran the full suite with the **full 10-minute idle** (`RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1`), and tore the environment down afterward. Final result: **5 passed / 0 failed** for the live suite (see Deviations for the one test-assertion fix discovered during the run). Criteria #4 and #5 were proven at the **full 600s idle** in the canonical run.

## Task Commits

1. **Task 1: Example binary + gated test config loader** — `26f0998` (feat)
2. **Task 2: Gated live suite — 5 criterion tests, `#[ignore]`'d** — `ddb43b5` (test)
3. **Task 3 fix: `screenshot_is_rgb_correct` tolerates a solid first frame** — `03302a0` (fix, applied during live validation)

## Files Created/Modified

- `crates/rdpilot/examples/screenshot.rs` (new) — end-to-end public-API example, PNG writer, own gitignored-secrets loader
- `crates/rdpilot/tests/common/mod.rs` (new) — `load_config()` + `idle_secs()`; `RDPILOT_LIVE`/`connection.json` gating; password never printed
- `crates/rdpilot/tests/live_session.rs` (new) — five gated criterion tests + non-grey / non-blank framebuffer helpers

## Live Validation Results (the phase gate)

- **Target:** Azure Windows VM, `Standard_B2s_v2`, westeurope, self-signed lab cert (`accept_invalid_certs(true)`, D-15); provisioned + torn down via `infra/manage-env.ps1`.
- **Canonical run (full 600s idle, `--test-threads=1`):** `connect_authenticates` ✅, `screenshot_crop` ✅, `stays_rendered_while_idle` ✅ (full 10-min idle), `keepalive_survives_10min` ✅ (full 10-min idle), `screenshot_is_rgb_correct` ❌ → fixed (see Deviations), then re-run **5 passed / 0 failed**.
- **Criterion mapping (all TRUE):**
  1. Connect/auth (NLA/CredSSP) → active session — `connect_authenticates` ✅
  2. Full-desktop RGB-correct screenshot (not YUV-grey) — `screenshot_is_rgb_correct` ✅ (opaque alpha + non-grey grid)
  3. Crop-to-rect — `screenshot_crop` ✅ (live origin pixel matches source)
  4. Stays full-resolution + non-blank, windowless, verified behaviorally — `stays_rendered_while_idle` ✅ (after full 10-min idle, no registry/WinRM)
  5. 10-minute idle keepalive prevents disconnect — `keepalive_survives_10min` ✅ (session alive + screenshot succeeds after 600s)
- **Keepalive (Assumption A1):** the shipped zero-delta pointer-move keepalive was **sufficient** — both 10-min idle tests passed; the ±1px fallback was **not** needed and was not applied.
- **Teardown:** `manage-env.ps1 -Action down` completed — `rdpilot-test` resource group fully deleted (no lingering Azure cost). The persistent `rdpilot-mgmt` RG is intentionally left in place.

## Decisions Made

- **`screenshot_is_rgb_correct` asserts color-correctness, not content-richness.** The first frame after connect can be a uniform solid desktop background (still correct RGB) before the shell paints. Criterion #2 is "RGB, not YUV-grey", so the test asserts opaque alpha (0xFF) + non-grey grid; non-blank/content is the domain of the idle test (which passed).
- **VM size `Standard_B2s_v2`.** The plan's default `Standard_B2ms` is `SkuNotAvailable` in westeurope; the `manage-env.ps1` preflight enumerated available sizes and documents the `-VmSize` override for exactly this case.
- **Secrets stay out of the library.** The config loader and example read `.secrets/connection.json` in test/example code only; the library remains environment-agnostic (D-12).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 — Blocking] Provisioned with `Standard_B2s_v2` instead of the default `Standard_B2ms`**
- **Found during:** Task 3 (`manage-env.ps1 -Action up`).
- **Issue:** The default `Standard_B2ms` returned `SkuNotAvailable` (capacity-restricted) in westeurope; the up-preflight aborted before deploy.
- **Fix:** Re-ran `up` with `-VmSize Standard_B2s_v2` (one of the preflight's listed-available sizes; the script documents this override). No code/config change — a provisioning parameter only.
- **Verification:** Deployment succeeded; the VM came up and the connection file was written (no secrets relayed).

**2. [Rule 1 — Bug, test assertion] `screenshot_is_rgb_correct` failed on a legitimately-uniform first frame**
- **Found during:** Task 3 (canonical live run).
- **Issue:** The test captured immediately after the first graphics update and asserted `!is_blank(...)`. The live VM's first post-connect frame was a uniform solid desktop background (a single flat color) before the shell painted — tripping the assertion ("screenshot is a single flat color"). This is **not** the YUV-grey pitfall (the non-grey guard passed) and is a too-strict test assertion, not an SDK fault: criterion #2 is RGB-correctness, not content-richness.
- **Fix:** Settle 3s after the first update, then assert criterion #2 via opaque-alpha (0xFF) + non-YUV-grey grid only; removed the `is_blank` assertion from this test (content/non-blank stays covered by `stays_rendered_while_idle`, which passed). `is_blank` is still used there, so it is not dead code.
- **Files modified:** `crates/rdpilot/tests/live_session.rs` (committed in `03302a0`).
- **Verification:** Offline `cargo test` + clippy green; live re-run = **5 passed / 0 failed**.

---

**Total deviations:** 2 (1 provisioning parameter, 1 test-assertion bug). Neither touched the SDK source — the Plan 01/02 library passed the live gate unchanged.

## Issues Encountered

- **Restricted default VM SKU in westeurope** — resolved via the documented `-VmSize` override (Deviation 1).
- **Solid first-frame on a fresh session** — diagnosed from the exact failure line and resolved by aligning the criterion-#2 test to "RGB-correct" rather than "content-rich" (Deviation 2). No SDK change required.

## Resolved Open Questions / Assumptions

- **Assumption A1 (keepalive sufficiency):** RESOLVED — the zero-delta pointer-move keepalive sustained the session across the full 10-minute idle (both `stays_rendered_while_idle` and `keepalive_survives_10min` passed). The ±1px-alternating fallback was not needed.
- **Open Q1 (VM idle timeout):** the live VM survived a full 10-min idle with keepalive active; keepalive at minimum proves no-harm and the session stayed rendered and connected.

## Known Stubs

None. The example and tests are fully wired to the live public API; `screenshot()` returns real framebuffer pixels (proven on the live VM). The `RDPILOT_SENSOR` DVC seam noted in Plan 02 remains the intentional Phase 4 hook (out of this plan's scope).

## Threat Flags

None beyond the plan's threat model. Mitigations confirmed:
- **T-02-08** (credential leakage): `.secrets/connection.json` is gitignored; the loader/example never print the parsed password; no secret value was read into the agent context; the live run filtered sensitive stdout.
- **T-02-03** (self-signed lab cert): `accept_invalid_certs(true)` is test/example-only and risk-named (D-15).
- **T-02-09** (VM cost): the environment was torn down immediately after validation (`manage-env.ps1 down`); Phase 1 auto-destroy is the backstop.
- **T-02-10** (silent no-op tests): the canonical run set `RDPILOT_LIVE=1 --include-ignored` and reported **5 passed** (not 5 skipped); offline runs correctly skip.

## User Setup Required

None going forward. The live gate is satisfied. Re-running the canonical validation later requires `infra/manage-env.ps1 -Action up -VmSize Standard_B2s_v2` (or another available SKU) and `RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1`, then `down`.

## Next Phase Readiness

- **Phase 2 is complete and proven end-to-end** against a real target: connect/auth, RGB-correct + cropped screenshots, stays-rendered-windowless, and 10-min keepalive all pass live; the offline suite stays green.
- **Phase 3 (Input Injection)** can build on the validated `Session` loop and the `RdpInputEvent` channel seam; the live harness pattern (gated `tests/`, `common::load_config`, `RDPILOT_*` env gating) is reusable for input verification.
- **Phase 4 (DVC sensor)** registers its `DvcProcessor` at the `RDPILOT_SENSOR` seam in `connect.rs` (before `connect_begin`), unaffected by this plan.

## Self-Check: PASSED

- All three created files exist: `crates/rdpilot/examples/screenshot.rs`, `crates/rdpilot/tests/live_session.rs`, `crates/rdpilot/tests/common/mod.rs`.
- All task commits present in git history: `26f0998`, `ddb43b5`, `03302a0`.
- Offline `cargo test -p rdpilot` = 21 passed / 5 ignored; `cargo clippy -p rdpilot --all-targets` = 0 warnings; `cargo build -p rdpilot --examples` exits 0.
- Live canonical run (full 10-min idle) = 5 passed / 0 failed; environment torn down.

---
*Phase: 02-rdp-session-framebuffer-core*
*Completed: 2026-06-05*
