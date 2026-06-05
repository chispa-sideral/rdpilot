---
phase: 02-rdp-session-framebuffer-core
plan: 01
subsystem: infra
tags: [rust, cargo, ironrdp, rustls, image, thiserror, mingw, windows-gnu, sdk]

# Dependency graph
requires:
  - phase: 01-environment
    provides: ".secrets/connection.json (consumed at test time by later plans), the live Windows RDP target"
provides:
  - "Cargo workspace at repo root (resolver 2, member crates/rdpilot)"
  - "Committed Cargo.lock freezing the 389-crate transitive graph (D-02)"
  - "rdpilot crate manifest with the CORRECTED IronRDP versions (umbrella 0.15 + members at independent versions)"
  - "Owned public types: Error (+ Result alias), ConnectionConfig, Screenshot, Rect"
  - "Screenshot::to_png() (image internal) and bounds-checked Screenshot::crop()"
  - "GNU x86_64 cross-build toolchain config (rust-toolchain.toml + .cargo/config.toml) for a non-MSVC ARM64 host"
affects: [02-02-session, 02-03-screenshot-wiring-live-suite, phase-3-input, phase-4-dvc-sensor]

# Tech tracking
tech-stack:
  added:
    - "ironrdp 0.15 (umbrella) + ironrdp-tokio 0.9, -tls 0.2.1 (rustls backend), -dvc 0.6, -input 0.6, -pdu 0.8"
    - "tokio 1, tokio-rustls 0.26, rustls 0.23 (aws-lc-rs default provider), sspi 0.21"
    - "image 0.25 (internal PNG encode), thiserror 2, tracing 0.1, x509-cert 0.2"
    - "dev: serde_json 1, serial_test 3"
    - "MinGW-w64 gcc 16.1.0 (scoop) + rustup target/toolchain x86_64-pc-windows-gnu"
  patterns:
    - "Owned SDK types at the public boundary (no image/ironrdp/rustls in pub signatures)"
    - "No-panic library code (thiserror Result; zero unwrap/expect/panic outside #[cfg(test)])"
    - "Redacted Debug for credential-bearing config types"
    - "Committed Cargo.lock with same-commit manifest discipline"

key-files:
  created:
    - "Cargo.toml (workspace root)"
    - "Cargo.lock"
    - "crates/rdpilot/Cargo.toml"
    - "crates/rdpilot/src/lib.rs"
    - "crates/rdpilot/src/error.rs"
    - "crates/rdpilot/src/config.rs"
    - "crates/rdpilot/src/screenshot.rs"
    - ".cargo/config.toml"
    - "rust-toolchain.toml"
  modified: []

key-decisions:
  - "Build target switched from *-pc-windows-msvc to x86_64-pc-windows-gnu — MSVC Build Tools/Windows SDK unavailable and user declined Visual Studio. Functionally equivalent for an RDP client (pure-Rust IronRDP + rustls, no MSVC-specific deps)."
  - "Pin the GNU toolchain as the project toolchain via rust-toolchain.toml (not the global rustup default) so the workspace builds on a non-MSVC host without changing global state."
  - "Enable the rustls backend feature on ironrdp-tls (the crate fails to compile without exactly one TLS backend selected; rustls matches the hand-built ClientConfig path planned for connect)."
  - "Screenshot::crop rejects zero-area rectangles in addition to out-of-bounds ones, returning Error::CropOutOfBounds."

patterns-established:
  - "Owned SDK types at the boundary: image/ironrdp/rustls/anyhow stay internal."
  - "No-panic policy (API-01): library code returns Error, never unwrap/expect/panic."
  - "Credential redaction: ConnectionConfig has a custom Debug that prints <redacted> for the password."
  - "Committed lockfile: Cargo.lock staged in the same commit as the manifests."

requirements-completed: [SESS-01, CAP-01]

# Metrics
duration: ~35min
completed: 2026-06-05
---

# Phase 2 Plan 01: Workspace + No-VM Foundation Types Summary

**First Rust code in the repo: a Cargo workspace, the rdpilot crate with corrected IronRDP 0.15 deps building via a GNU x86_64 cross-toolchain, and the owned no-VM types (Error, ConnectionConfig, Screenshot/Rect with to_png + bounds-checked crop) — green offline with 8 passing unit tests.**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-06-05 (continuation of a checkpoint-paused plan)
- **Completed:** 2026-06-05
- **Tasks:** 3 (Task 1 checkpoint resolved by user decision; Tasks 2 & 3 executed)
- **Files modified:** 9 created

## Accomplishments
- Resolved the Task 1 toolchain blocker: installed MinGW-w64 + the `x86_64-pc-windows-gnu` Rust toolchain and pinned it for the workspace; a trivial cargo build links and produces a runnable x86-64 PE.
- Scaffolded the Cargo workspace and `rdpilot` crate; `cargo build` exits 0 resolving the corrected IronRDP graph (umbrella 0.15 + members 0.9/0.8/0.6/0.2) — the non-resolving uniform 0.14 pins (Pitfall 6) avoided. Committed `Cargo.lock` (389 crates).
- Implemented the owned public surface: `Error`/`Result` (thiserror, no third-party leakage, no-panic), `ConnectionConfig` (env-agnostic, risk-named `accept_invalid_certs` default-off, password-redacting Debug), and `Screenshot`/`Rect` (`to_png` with `image` internal, bounds-checked `crop`).
- 8 offline unit tests pass (crop dims/sub-pixels/out-of-bounds, to_png magic bytes, config defaults/builders/redaction, buffer-length validation); clippy clean.

## Task Commits

Each task was committed atomically:

1. **Task 1: Install the Rust toolchain (D-19 blocker)** — resolved via the checkpoint user decision (GNU toolchain). Toolchain config committed as part of Task 2 (the approved scaffold exception).
2. **Task 2: Workspace + crate manifest with corrected IronRDP versions** — `4dc3225` (feat)
3. **Task 3: Error, ConnectionConfig, Screenshot no-VM types** — `b0b6ab0` (feat)

**Plan metadata:** see final docs commit below.

_Note: Task 3 is `tdd="true"`; tests and implementation landed in a single commit (the test module lives inline in `screenshot.rs`/`config.rs` and was authored alongside the types — RED/GREEN verified locally before commit)._

## Files Created/Modified
- `Cargo.toml` — workspace root (resolver 2, member `crates/rdpilot`)
- `Cargo.lock` — committed lockfile, 389-crate transitive graph (D-02)
- `crates/rdpilot/Cargo.toml` — crate manifest, corrected IronRDP versions, `rustls` feature on `ironrdp-tls`, dev-deps
- `crates/rdpilot/src/lib.rs` — crate root, re-exports the owned public surface
- `crates/rdpilot/src/error.rs` — `Error` enum (thiserror) + `Result` alias, no-panic, source-erased variants
- `crates/rdpilot/src/config.rs` — `ConnectionConfig` owned type, builders, redacted Debug
- `crates/rdpilot/src/screenshot.rs` — `Screenshot`/`Rect`, `to_png()`, bounds-checked `crop()`, unit tests
- `.cargo/config.toml` — default `x86_64-pc-windows-gnu` target + pinned MinGW gcc linker
- `rust-toolchain.toml` — pins the GNU x86_64 toolchain for the workspace

## Decisions Made
- **GNU x86_64 toolchain instead of MSVC** (see Deviations) — the load-bearing decision; everything else followed from it.
- **Pin the toolchain per-project** (`rust-toolchain.toml`) rather than flipping the global rustup default, keeping the change local and reproducible.
- **`ironrdp-tls` rustls backend** — required to compile; matches the planned hand-built rustls `ClientConfig` path.
- **`crop` rejects zero-area rects** — a zero-width/height crop is meaningless and would yield an empty buffer; treating it as `CropOutOfBounds` is safer and is covered by a test.

## Deviations from Plan

### Auto-fixed Issues

**1. [Architectural — checkpoint-approved] Build target switched from `*-pc-windows-msvc` to `x86_64-pc-windows-gnu`**
- **Found during:** Task 1 (toolchain) — prior executor hit a human-action checkpoint; user resolved it with this decision.
- **Issue:** The host is ARM64 Windows with no MSVC linker / Windows SDK, and the user declined to install Visual Studio. The locked stack assumed `*-msvc`.
- **Fix:** Installed MinGW-w64 (`scoop install mingw`, gcc 16.1.0 targeting `x86_64-w64-mingw32`), added the `x86_64-pc-windows-gnu` Rust target, and force-installed the `stable-x86_64-pc-windows-gnu` host toolchain (its x64 `rustc.exe` and the x64 host build-scripts/proc-macros run under Windows-on-ARM x64 emulation, a tier-1 target). Pinned the toolchain in `rust-toolchain.toml` and the gcc linker in `.cargo/config.toml`. Functionally equivalent for an RDP client (pure-Rust IronRDP + rustls; no MSVC-specific dependencies). A linker smoke test produced and ran a PE32+ x86-64 exe before proceeding.
- **Files modified:** `rust-toolchain.toml`, `.cargo/config.toml` (committed in `4dc3225`)
- **Verification:** smoke-test exe ran ("Hello, world!"); full `cargo build` exits 0; `cargo test` runs the x64 test binary under emulation (8 passed).
- **Committed in:** `4dc3225` (Task 2 commit)

**2. [Rule 3 — Blocking] Enabled the `rustls` TLS backend feature on `ironrdp-tls`**
- **Found during:** Task 2 (`cargo build`).
- **Issue:** `ironrdp-tls` 0.2.1 emits `compile_error!` unless exactly one TLS backend feature (`rustls`/`native-tls`/`stub`) is selected; the plan's bare `ironrdp-tls = "0.2"` did not compile.
- **Fix:** Pinned `ironrdp-tls = { version = "0.2", default-features = false, features = ["rustls"] }` to match the planned hand-built rustls `ClientConfig` / custom cert verifier path (D-15).
- **Files modified:** `crates/rdpilot/Cargo.toml` (committed in `4dc3225`)
- **Verification:** `cargo build` exits 0 after the change.
- **Committed in:** `4dc3225` (Task 2 commit)

**3. [Rule 1 — Bug, trivial] Removed a clippy `identity_op` (`1 * width`) in a test**
- **Found during:** Task 3 (clippy).
- **Issue:** A crop sub-pixel test computed an index with `1 * cropped.width`, tripping `clippy::identity_op`.
- **Fix:** Rewrote with explicit `(cx, cy)` coordinates; clippy clean.
- **Files modified:** `crates/rdpilot/src/screenshot.rs` (committed in `b0b6ab0`)
- **Verification:** `cargo clippy --all-targets` produces no warnings; tests still pass.
- **Committed in:** `b0b6ab0` (Task 3 commit)

---

**Total deviations:** 3 (1 architectural/checkpoint-approved, 1 blocking, 1 trivial bug)
**Impact on plan:** The target switch is the only material change and was explicitly approved at the checkpoint; it does not alter the public API, the version pins, or any behavior. The `ironrdp-tls` feature is mandatory for compilation. No scope creep — the toolchain files are the pre-approved buildability exception.

## Issues Encountered
- **Initial build failed at host build-script/proc-macro linking.** With only the `x86_64-pc-windows-gnu` *target* added (host still `aarch64-pc-windows-msvc`), build scripts/proc-macros compiled for the MSVC host and invoked a non-existent `link.exe` (the MSYS coreutils `link` shadowed it, surfacing "extra operand"). Resolved by force-installing the full `stable-x86_64-pc-windows-gnu` *host* toolchain so host artifacts also build as x64-gnu and run under emulation. This is the correct fix for a non-MSVC ARM64 host, not a workaround.

## Package Audit (D-02 / first-build lockfile review)
- Resolved IronRDP versions match RESEARCH.md exactly: `ironrdp 0.15.0`, `-connector/-tokio 0.9.0`, `-pdu 0.8.0`, `-input/-dvc 0.6.0`, `-tls 0.2.1` (plus internal `ironrdp-core 0.2`, `-async 0.9`, `-svc 0.7`, `-error 0.2`).
- 389 total crates. TLS/crypto providers present: `rustls 0.23.40` with `aws-lc-rs`/`aws-lc-sys` (rustls 0.23 default provider) and `ring` (pulled transitively). Nothing unexpected or slop; all in line with an RDP + image + TLS stack.

## Known Stubs
- The `Error::Session` variant and several IronRDP member crates (`ironrdp-tokio`, `-dvc`, `-input`, `-pdu`, `sspi`, `tokio-rustls`, `x509-cert`) are declared in the manifest but **not yet referenced** by library code. This is intentional: they are the dependency baseline for Plan 02 (session loop / connect) and Plan 03 (live wiring). Rust does not warn on unused manifest deps, and the build/tests are green. No stub renders into a user-visible surface in this plan.

## Threat Flags
None — no new security-relevant surface beyond the plan's threat model. The mitigations for T-02-01 (crop bounds → `Error`, never panic), T-02-02 (redacted `Debug`, no credential logging, env-agnostic), and T-02-SC (verified versions + committed `Cargo.lock`) are all implemented. T-02-03 (`accept_invalid_certs` risk-named, default-off) is defined here; its enforcement at the connect path lands in Plan 02.

## User Setup Required
None — no external service configuration required. (Toolchain prerequisites — MinGW-w64 via scoop and the `x86_64-pc-windows-gnu` toolchain — were installed during execution and are pinned via `rust-toolchain.toml`/`.cargo/config.toml`.)

## Next Phase Readiness
- Workspace, dependency baseline, and the owned no-VM type surface are in place and green offline.
- Plan 02 (session) can build the connect path + SDK-owned async loop against `ConnectionConfig`/`Error`, and Plan 03 wires `Screenshot` to the live framebuffer + the gated live suite.
- Build environment is reproducible on this non-MSVC ARM64 host via the committed toolchain config; any future agent must export the scoop rustup env (`RUSTUP_HOME`/`CARGO_HOME`/`CARGO_HOME/bin` on PATH) and have MinGW gcc on PATH for cargo to link.

## Self-Check: PASSED

All created files exist on disk and both task commits (`4dc3225`, `b0b6ab0`) are present in git history.

---
*Phase: 02-rdp-session-framebuffer-core*
*Completed: 2026-06-05*
