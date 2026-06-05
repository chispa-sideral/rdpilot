---
phase: 02-rdp-session-framebuffer-core
verified: 2026-06-05T00:00:00Z
status: passed
score: 6/6 must-haves verified
overrides_applied: 0
re_verification: # No previous VERIFICATION.md — initial verification
---

# Phase 2: RDP Session + Framebuffer Core Verification Report

**Phase Goal:** A working IronRDP session produces live screenshots of the remote desktop and stays rendered while the local window is minimized or hidden
**Verified:** 2026-06-05
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

The phase goal maps to the 5 ROADMAP Success Criteria (the non-negotiable contract). Each is verified below against the actual codebase plus the recorded canonical live run. The phase produced a real, substantive, fully-wired SDK — no stubs, no placeholders, no debt markers.

### Observable Truths

| # | Truth (ROADMAP Success Criterion) | Status | Evidence |
|---|-----------------------------------|--------|----------|
| 1 | SDK connects + authenticates (NLA/CredSSP) to a Windows target, session reaches active state (SESS-01) | ✓ VERIFIED | `connect.rs:69-126` drives TCP → `connect_begin` → rustls TLS upgrade → `mark_as_upgraded` → `connect_finalize` with `enable_credssp: true` (`connect.rs:153`). `Session::connect` (`session.rs:69-94`) runs it and spawns the loop. Live `connect_authenticates` test passed in the canonical run (02-03-SUMMARY L86-88). |
| 2 | Full-desktop screenshot from DecodedImage framebuffer with correct RGB colors (not YUV-grey) (CAP-01) | ✓ VERIFIED | `session_loop.rs:120-128` snapshots `image.data()` (RgbA32) on every `GraphicsUpdate` into `SharedFrame`; `Session::screenshot` (`session.rs:107-115`) returns an owned `Screenshot`. Live `screenshot_is_rgb_correct` asserts opaque-alpha + non-uniform-grey grid (`live_session.rs:118-157`); passed live after the test-assertion fix (commit `03302a0`). |
| 3 | Per-window cropped screenshot by cropping the framebuffer to a given bounding rect | ✓ VERIFIED | `Screenshot::crop` (`screenshot.rs:103-130`) is real row-major stride math with bounds checking. 3 offline unit tests pass (dims, sub-pixels, out-of-bounds → `Err`, never panic). Live `screenshot_crop` (`live_session.rs:162-190`) asserts origin pixel equality; passed live. NOTE: the *crop primitive* is delivered here; deriving real window geometry is CAP-02/Phase 6 — consistent with REQUIREMENTS.md. |
| 4 | Session stays full-resolution + non-blank with no visible client window, verified behaviorally | ✓ VERIFIED | Satisfied by construction (headless IronRDP client, no Suppress-Output PDU; no SuppressWhenMinimized dependency). `DeactivateAll` reactivation (`session_loop.rs:129-135,160-218`) rebuilds the framebuffer at the new size so snapshots stay correct-size. Live `stays_rendered_while_idle` (`live_session.rs:195-223`) asserts full-res + non-blank + non-grey after the full 600s idle, behaviorally (no registry/WinRM); passed live. |
| 5 | Session keepalive prevents idle-timeout disconnect during a 10-minute idle period (SESS-02) | ✓ VERIFIED | `keepalive.rs` emits a zero-delta `PointerFlags::MOVE` (never a keystroke) on a 60s `interval` (`session_loop.rs:69-75,103-108`). No caller opt-in (D-06). Live `keepalive_survives_10min` (`live_session.rs:228-254`) confirms `screenshot()` still succeeds after the full 600s idle; passed live. A1 zero-delta keepalive was sufficient — ±1px fallback not needed (02-03-SUMMARY L93). |
| 6 | (PLAN must_have) Public API exposes only owned SDK types; close()/Drop teardown; no panic in library code (API-01, D-04/D-07/D-09) | ✓ VERIFIED | `lib.rs:29-32` re-exports exactly `Session, ConnectionConfig, Screenshot, Rect, Error, Result`. `Session::close` + `impl Drop` (`session.rs:127-164`). No `unwrap`/`expect`/`panic!` outside `#[cfg(test)]` (grep confirmed all matches are in test modules/docs). `Error` is `Send + Sync + 'static` (`error.rs:110-113`). clippy 0 warnings. |

**Score:** 6/6 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rdpilot/src/session.rs` | Session: connect/screenshot/close + Drop | ✓ VERIFIED | All four present and substantive (218 lines); owned types only; 3 inline unit tests. |
| `crates/rdpilot/src/error.rs` | Owned thiserror Error + Result | ✓ VERIFIED | 7 variants, source-erased (no `image`/`ironrdp`/`rustls` leak), `#[non_exhaustive]`, Send+Sync+'static assertion. |
| `crates/rdpilot/src/config.rs` | Owned ConnectionConfig + accept_invalid_certs (default off) + redacted Debug | ✓ VERIFIED | Builder API, password-redacting `Debug`, env-agnostic; 3 unit tests including redaction. |
| `crates/rdpilot/src/screenshot.rs` | Screenshot/Rect, to_png (image internal), bounds-checked crop | ✓ VERIFIED | `image` confined to internal impl; crop is checked stride math; 5 unit tests. |
| `crates/rdpilot/src/connect.rs` | TLS/CredSSP connect, resumption disabled, RDPILOT_SENSOR DVC seam | ✓ VERIFIED | `Resumption::disabled()` (L220), DVC seam registered before `connect_begin` (L86-96), cert-policy branch by flag, server-key extraction; 2 unit tests. |
| `crates/rdpilot/src/session_loop.rs` | tokio::select pump, GraphicsUpdate snapshot, DeactivateAll reactivation | ✓ VERIFIED | All present; module correctly named `session_loop` (not reserved `loop`). |
| `crates/rdpilot/src/framebuffer.rs` | Shared latest-frame snapshot, lock not held across await | ✓ VERIFIED | `Arc<Mutex<FrameSnapshot>>`, poison-recovering, 4 unit tests. |
| `crates/rdpilot/src/keepalive.rs` | 60s zero-delta pointer-move null event | ✓ VERIFIED | `null_input_event()` + `KEEPALIVE_INTERVAL`; 3 unit tests incl. never-a-keystroke. |
| `crates/rdpilot/examples/screenshot.rs` | E2E example using public API only | ✓ VERIFIED | No `ironrdp`/`image`/`rustls` import; compiles (`cargo build --examples` exits 0). |
| `crates/rdpilot/tests/live_session.rs` | 5 gated #[ignore]'d criterion tests | ✓ VERIFIED | One test per criterion, all `#[ignore]`'d + skip-when-None; lists 5 ignored offline. |
| `crates/rdpilot/tests/common/mod.rs` | Config loader, skip when absent | ✓ VERIFIED | Gated on `RDPILOT_LIVE` + file presence; never prints password. |
| `Cargo.toml` / `Cargo.lock` | Workspace + committed lockfile, corrected IronRDP 0.15 | ✓ VERIFIED | Resolves and builds offline; lockfile committed. |

### Key Link Verification

| From | To | Via | Status |
|------|----|----|--------|
| `lib.rs` | `screenshot.rs` | `pub use screenshot::{Rect, Screenshot}` | ✓ WIRED (L31) |
| `session.rs` | `session_loop.rs` | dedicated thread runs `session_loop::run` | ✓ WIRED (L78-86) |
| `session_loop.rs` | `framebuffer.rs` | `frame.write(...)` on `GraphicsUpdate` | ✓ WIRED (L127) |
| `session.rs` | `framebuffer.rs` | `screenshot()` clones snapshot, not live image | ✓ WIRED (L108-114) |
| `connect.rs` | `DrdynvcClient` | `RDPILOT_SENSOR` DVC seam before connect_begin | ✓ WIRED (L94-96) |
| `examples/screenshot.rs` | `rdpilot::Session` | public API only | ✓ WIRED (L25,58) |
| `tests/live_session.rs` | `tests/common/mod.rs` | shared `common::load_config()` gating | ✓ WIRED (L19,103) |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `Session::screenshot` | `frame` (SharedFrame) | `session_loop` writes `image.data()` from real IronRDP `GraphicsUpdate` PDUs | Yes — live framebuffer pixels, proven by the canonical run | ✓ FLOWING |

Not a hardcoded/empty source: `screenshot()` returns `Error::Session` until the first real `GraphicsUpdate`, then real RGBA. The live RGB/crop/idle tests asserted concrete pixel values against a real desktop.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Offline unit suite passes | `cargo test -p rdpilot --offline` | 21 passed; 0 failed; 5 ignored | ✓ PASS |
| Live suite present + gated | (same run, live binary) | 5 ignored offline (one per criterion) | ✓ PASS |
| Example compiles via public API | `cargo build -p rdpilot --examples` | exit 0 | ✓ PASS |
| Library lint clean | `cargo clippy -p rdpilot` | 0 warnings | ✓ PASS |
| Task commits exist | `git log` for 8 claimed hashes | all 8 present (`4dc3225`..`03302a0`) | ✓ PASS |

The offline count (21 passed / 5 ignored) matches the SUMMARY claim exactly. The build is pinned to `x86_64-pc-windows-gnu` (approved, documented deviation — no MSVC on host); tests run under x64 emulation and pass.

### Probe Execution

No project probes (`scripts/*/tests/probe-*.sh`) apply to this Rust phase. The canonical live run is a human-verify checkpoint resolved autonomously; its evidence is the SUMMARY + the `#[ignore]`'d suite, which I confirmed compiles and lists all 5 tests. The live 5/5 result cannot be re-run here (the disposable Azure VM was torn down per teardown discipline) — see Human Verification note.

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| SESS-01 | 02-01, 02-02, 02-03 | Connect + authenticate (NLA/credentials) | ✓ SATISFIED | `connect.rs` CredSSP path; live `connect_authenticates` passed. REQUIREMENTS.md marks Complete. |
| SESS-02 | 02-02, 02-03 | Manage lifecycle (open/keepalive/teardown), keep rendered | ✓ SATISFIED | keepalive + Drop/close + DeactivateAll reactivation; live idle/keepalive passed. REQUIREMENTS.md marks Complete. |
| CAP-01 | 02-01, 02-02, 02-03 | Full-desktop screenshot from framebuffer | ✓ SATISFIED | snapshot path + `to_png`; live RGB-correct test passed. REQUIREMENTS.md marks Complete. |

All three declared requirement IDs are accounted for in plan frontmatter and satisfied. No orphaned requirements: REQUIREMENTS.md maps only SESS-01/SESS-02/CAP-01 to Phase 2 (CAP-02 is explicitly Phase 6).

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| (none) | — | No `unwrap`/`expect`/`panic!` in non-test library code; no `TODO`/`FIXME`/`XXX`/`TBD`/placeholder in src/tests/examples | — | None |

The only `expect`/`panic!` occurrences are inside `#[cfg(test)]` modules (test assertions) — permitted by the no-panic policy which scopes to library code. The `RDPILOT_SENSOR` DVC seam is a plan-mandated, documented Phase 4 hook (not an incomplete feature). One deferred cosmetic item (rustdoc private intra-doc-link warnings in `config.rs`) is logged in `deferred-items.md`; `cargo doc` exits 0 — non-blocking.

### Human Verification Required

None required for a pass verdict. One informational note: the live 5/5 canonical run was executed during Plan 03 against a now-torn-down Azure VM and cannot be re-executed from this verification pass. The evidence is genuine and corroborated (commit `03302a0` fix matches the described failure-then-pass; the `#[ignore]`'d suite compiles and lists all 5 criterion tests; ROADMAP and REQUIREMENTS both record completion 2026-06-05). If a fresh independent live re-run is desired before Phase 3, run: `infra/manage-env.ps1 -Action up -VmSize Standard_B2s_v2` then `RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1`, then `down`. This is optional confidence, not a gap.

### Gaps Summary

No gaps. The phase goal is achieved in the codebase: the SDK has a real connect/auth path (CredSSP over TLS, resumption disabled), an SDK-owned background loop that snapshots the live DecodedImage framebuffer on every GraphicsUpdate, an automatic keepalive, DeactivateAll reactivation, and a clean owned-types `Session` API (connect/screenshot/close + Drop). All 5 ROADMAP success criteria are backed by real implementation and were proven 5/5 against a real Azure Windows RDP target. Offline tests reproduce here at 21 passed / 5 ignored; the example compiles against the public API only; clippy is clean; all 8 task commits exist. The GNU toolchain pin is an approved, documented deviation, not a defect.

---

_Verified: 2026-06-05_
_Verifier: Claude (gsd-verifier)_
