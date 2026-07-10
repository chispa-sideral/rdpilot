# Phase 2: RDP Session + Framebuffer Core - Pattern Map

**Mapped:** 2026-06-05
**Files analyzed:** 12 (new Rust artifacts + workspace config)
**Analogs found:** 0 / 12 (greenfield Rust — no in-repo Rust analogs exist; see "Greenfield Notice" + "Upstream Pattern Sources")

## Greenfield Notice

This phase introduces the **first Rust code in the repository**. Verified live (`git ls-files`, `**/*.rs`, `**/Cargo.toml`):

- **No `Cargo.toml`, no `crates/`, no `src/`, no `*.rs`** anywhere in the repo.
- The only existing code is **Phase 1's IaC** under `infra/` (PowerShell + Bicep) — a different language, a different role (Azure provisioning), and **not** an analog for an IronRDP SDK. `infra/` is the *test target provider* this phase consumes, not a pattern source.
- `.gitignore` is already Rust-ready: it excludes `/target/` and `**/*.rs.bk` (added in Phase 1, with a comment deferring the `Cargo.lock` policy to Phase 2 — now resolved by D-02: **commit `Cargo.lock`**).

Therefore there are **zero in-codebase analogs** for the files this phase creates. This document does **not** invent analogs. It does three things:

1. **Classifies** each new file by role + data flow.
2. **Points the planner at the canonical UPSTREAM templates** already captured in `02-RESEARCH.md` (IronRDP `screenshot.rs` and `ironrdp-client/src/rdp.rs`), which are the patterns to copy from in lieu of a local analog. The research has already fetched these live (2026-06-05) and extracted the load-bearing excerpts with line citations — the planner should treat those as the per-file "analog."
3. **Records the conventions this phase establishes** as the Rust baseline for every later crate (workspace layout, owned-SDK-types boundary, no-`unwrap` error policy, test gating).

**Precedent:** Phase 1's `01-PATTERNS.md` used exactly this "greenfield — follow cited external sources, establish conventions" structure. This continues it for the Rust side.

## File Classification

Derived from CONTEXT.md decisions (D-01..D-19), RESEARCH.md "Recommended Project Structure" (L222-244), and the Wave 0 gaps (L542-547). Paths follow the research-recommended single-crate workspace (`crates/rdpilot`, D-01). The internal module split is the research recommendation — the planner may consolidate small modules (e.g. fold `keepalive.rs`/`framebuffer.rs` into `session_loop.rs`/`session.rs`) as long as the public API surface (D-04/D-09) and the DVC seam (Phase 4) are preserved.

| New File | Role | Data Flow | Closest Analog | Match Quality |
|----------|------|-----------|----------------|---------------|
| `Cargo.toml` (workspace root) | config | n/a | none (greenfield) | no analog — establishes workspace baseline (D-01) |
| `Cargo.lock` (committed) | config | n/a | none | no analog — committed per D-02 |
| `crates/rdpilot/Cargo.toml` | config | n/a | none | no analog — dep table = RESEARCH.md "Installation" (L124-141) |
| `crates/rdpilot/src/lib.rs` | config (crate root / re-exports) | n/a | none | no analog — `pub use` of owned SDK types (D-09) |
| `crates/rdpilot/src/config.rs` | model (`ConnectionConfig`) | transform (params/env/file → typed config) | none | no analog — shape from D-12/D-13/D-15 |
| `crates/rdpilot/src/error.rs` | model (error enum) | n/a | none | no analog — `thiserror` enum, API-01 (no `unwrap`) |
| `crates/rdpilot/src/connect.rs` | service (connect/auth) | request-response (handshake) | none | no analog — **upstream:** `screenshot.rs` build_config/tls_upgrade + `rdp.rs` connect path (RESEARCH.md Pattern 1, L246-267) |
| `crates/rdpilot/src/session.rs` | service (`Session` handle) | event-driven (spawn task, lifecycle) | none | no analog — **upstream:** `rdp.rs` session ownership; teardown D-07 (Pattern 6, L340-343) |
| `crates/rdpilot/src/session_loop.rs` | service (active_session pump) | streaming (PDU pump / `tokio::select!`) | none | no analog — **upstream:** `ironrdp-client/src/rdp.rs::active_session` (Pattern 2, L269-301; Pattern 5 reactivation, L319-338) |
| `crates/rdpilot/src/framebuffer.rs` | model (latest-frame snapshot) | streaming (snapshot on `GraphicsUpdate`) | none | no analog — `Arc<Mutex<…>>`/`watch` snapshot (Pattern 2 + Pitfall 3, L380-385) |
| `crates/rdpilot/src/screenshot.rs` | utility (`Screenshot`: to_png/crop) | transform (RGBA → PNG / crop) | none | no analog — **upstream:** `screenshot.rs` PNG path (Pattern 4, L308-317) + crop math (L447-459) |
| `crates/rdpilot/src/keepalive.rs` | utility (synthetic null input) | event-driven (timer → FastPath) | none | no analog — `ironrdp-input` null pointer-move (Pattern 3, L303-306) |
| `crates/rdpilot/examples/screenshot.rs` | example (binary) | request-response (load cfg → connect → save png) | none | no analog — composes the public API (D-04/D-09) |
| `crates/rdpilot/tests/live_session.rs` | test (gated integration) | request-response (live VM) | none (precedent: Phase 1 Pester live tests) | no analog — gating per D-18 (Validation Architecture, L515-547) |
| `crates/rdpilot/tests/crop.rs` (or `#[cfg(test)]` in `screenshot.rs`) | test (unit, no VM) | transform (pure buffer math) | none | no analog — crop math, runs without a target |

> The 12-file count covers the workspace + crate scaffold (`Cargo.toml` ×2, `Cargo.lock`) and the 9 source/example/test artifacts. Internal `src/*.rs` module boundaries are advisory; the planner owns final granularity.

## Pattern Assignments

Each file below lists role/data-flow and the **UPSTREAM pattern source** (no local analog exists). Excerpts are NOT re-pasted here — they already live in `02-RESEARCH.md` with live-source line citations. The planner should copy from the research excerpts directly and verify exact signatures against `docs.rs` at implementation time (RESEARCH.md Open Questions 3, L494-497).

---

### `Cargo.toml` (workspace root) + `crates/rdpilot/Cargo.toml` (config) — DO FIRST

**Analog:** none. **Source:** RESEARCH.md "Recommended Project Structure" (L222-244) + corrected "Installation" dependency block (L124-141).

**Workspace root — copy structure (D-01):**
```toml
[workspace]
members = ["crates/rdpilot"]
resolver = "2"
```

**Crate deps — use the CORRECTED versions (RESEARCH.md L124-141, supersedes CLAUDE.md / STACK.md's "0.14" block which WILL NOT RESOLVE):**
```toml
ironrdp        = "0.15"
ironrdp-tokio  = "0.9"
ironrdp-tls    = "0.2"      # optional; or hand-build rustls ClientConfig
ironrdp-dvc    = "0.6"      # Phase 2: registration seam only — do NOT build the sensor
ironrdp-input  = "0.6"
ironrdp-pdu    = "0.8"
tokio          = { version = "1", features = ["full"] }
tokio-rustls   = "0.26"
rustls         = "0.23"
sspi           = "0.21"
image          = "0.25"
thiserror      = "2"
tracing        = "0.1"
x509-cert      = "0.2"      # if replicating the example's cert-key extraction
```
`[dev-dependencies]`: `tokio` (macros, rt), optionally `serial_test` (live tests must not run concurrently against one VM — Validation Architecture L520).

**Conventions established:** Cargo workspace with one member today (D-01); **commit `Cargo.lock`** (D-02); umbrella `ironrdp = "0.15"` facade + individually-pinned member crates (Pitfall 6, L398-401); re-verify versions and review the new transitive graph on first `cargo build` (Package Audit note, L175). Rust 1.78+ (D-03) — toolchain installed via `scoop install rustup` first (D-19; **Wave 0 blocker**, L509-511).

---

### `crates/rdpilot/src/error.rs` (model, error enum)

**Analog:** none. **Source:** Claude's Discretion (CONTEXT D / RESEARCH L43, L112) + API-01.

**Convention established:** `thiserror`-derived `Error` enum; public `Result<T, Error>` surface; **no `unwrap`/`expect`/`panic` in library code** (API-01). Crop out-of-bounds, malformed config, connect failure, and decode failure all return `Error`, never panic (Security V5, L559; threat "Crop/coords panic", L571). Do **not** leak `anyhow`/`image`/`ironrdp` types into the public error surface (Anti-Patterns, L348).

---

### `crates/rdpilot/src/config.rs` (model, `ConnectionConfig`)

**Analog:** none. **Source:** D-12/D-13/D-15; `screenshot.rs::build_config` for the IronRDP `connector::Config` mapping (RESEARCH.md Pattern 1 note, L267).

**Pattern:** owned typed `ConnectionConfig` (host, port, credentials, domain, requested width/height, color depth). Library is **env-agnostic** — no repo-specific secrets path hard-coded (D-12). Optional convenience loaders (env vars / config file) may live here but the source/path is the caller's choice (D-13). Expose a **risk-named** `accept_invalid_certs(true)` flag, default off (D-15); thumbprint pinning deferred. Map to IronRDP `connector::Config { credentials: Credentials::UsernamePassword{..}, enable_credssp: true, enable_tls: false /* frontend re-export, NOT the wire */, desktop_size, enable_server_pointer: false, .. }` (L267, L489-492).

**Conventions established:** Owned SDK types only at the API boundary (D-09); credentials are passed-through parameters — the SDK **never** touches OS credential machinery (D-14), never logs credential fields (Security V7, L561; threat L568).

---

### `crates/rdpilot/src/connect.rs` (service, connect/auth — SESS-01)

**Analog:** none. **Upstream source:** RESEARCH.md **Pattern 1** (L246-267) — async sequence from `ironrdp-client/src/rdp.rs`; rustls config + cert-key extraction from `screenshot.rs` (Code Examples, L417-434).

**Pattern (already excerpted in research — copy from there):** `TokioFramed::new` → `ClientConnector::new` → **[Phase 4 DVC seam: register `RDPILOT_SENSOR` `DvcProcessor` on the connector HERE, before `connect_begin`]** → `connect_begin` → TLS upgrade (hand-built rustls `ClientConfig`, custom verifier behind `accept_invalid_certs`, **`Resumption::disabled()`** per CredSSP/MS-CSSP) → extract server public key (`x509-cert` DER decode) → `mark_as_upgraded` → `connect_finalize` (sspi `ReqwestNetworkClient`).

**Conventions established / critical correctness:** **resumption MUST be disabled** (Pitfall 5, L392-396); default path validates certs, opt-out is risk-named (D-15); the **DVC registration seam is mandatory** — leave the hook before `connect_finalize`, do NOT exercise it (Anti-Pattern L350; Integration Points). Verify the exact `connect_finalize` arg order against `docs.rs/ironrdp-tokio/0.9` at impl time (Open Q3, L494-497; Assumption A3).

---

### `crates/rdpilot/src/session_loop.rs` (service, active_session pump — SESS-02)

**Analog:** none. **Upstream source:** RESEARCH.md **Pattern 2** (L269-301) — *mirrors* `ironrdp-client/src/rdp.rs::active_session` 1:1; **Pattern 5** (L319-338) for Deactivation-Reactivation.

> Module is named `session_loop` (NOT `loop`): `loop` is a Rust reserved keyword and `mod loop;` will not compile.

**Pattern (excerpted in research — copy from there):** `split_tokio_framed` → independent (reader, writer) → `DecodedImage::new(PixelFormat::RgbA32, w, h)` → `ActiveStage::new` → `loop { tokio::select! { reader.read_pdu(), input_rx.recv(), keepalive_interval.tick() } }` → dispatch `ActiveStageOutput::{ResponseFrame→write_all, GraphicsUpdate→snapshot, DeactivateAll→reactivation, Terminate→break}`; ignore `Pointer*` (`enable_server_pointer: false`).

**Conventions established / critical:** the loop **owns the only mutating `DecodedImage`**; `screenshot()` reads a *snapshot*, never the live image across `.await` (Pitfall 3, L380-385; Anti-Pattern L345). Must handle `DeactivateAll` → rebuild `image` + fast-path processor on `Finalized{desktop_size}` or criterion #4 breaks after a server resize (Pattern 5; Pitfall 2, L374-378).

---

### `crates/rdpilot/src/framebuffer.rs` (model, latest-frame snapshot — CAP-01)

**Analog:** none. **Upstream source:** the snapshot half of Pattern 2 (L292-293, L301) + Pitfall 3 (L380-385).

**Pattern:** on each `GraphicsUpdate`, copy `image.data()` (RGBA32, tightly packed `w*h*4`) + dims into shared state — `Arc<Mutex<FrameSnapshot>>` or `tokio::sync::watch`. `screenshot()` clones the latest snapshot. **Never** share the live `DecodedImage` across the API boundary.

**Convention established:** snapshot-on-`GraphicsUpdate` is the integrity contract for "current" perception (Security threat "Stale framebuffer", L572).

---

### `crates/rdpilot/src/screenshot.rs` (utility, `Screenshot` — CAP-01 #2/#3)

**Analog:** none. **Upstream source:** RESEARCH.md **Pattern 4** (L308-317) for PNG; crop math (Code Examples L447-459).

**Pattern (excerpted in research):** owned `Screenshot { width, height, rgba: Vec<u8> }` (D-09); `to_png()` via `image::ImageBuffer::<Rgba<u8>>::from_raw` → PNG bytes to an in-memory `Cursor<Vec<u8>>` (D-11; `image` stays internal); `crop(rect)` = row-major stride math on the retained RGBA buffer (no PNG re-decode), **bounds-checked → `Error`, never panic** (L458; API-01).

**Conventions established:** `PixelFormat::RgbA32` is correct-color **by construction** — no YUV→RGB conversion on the bitmap/RLE/RDP6/RemoteFX path (Pitfall 1, L368-372); do NOT opt into EGFX/H.264 in Phase 2. The `image` crate is an internal impl detail, never in the public API (D-09; Anti-Pattern L348). Caller writes PNG bytes to disk — the SDK only encodes (D-11).

---

### `crates/rdpilot/src/keepalive.rs` (utility, synthetic null input — SESS-02 #5)

**Analog:** none. **Upstream source:** RESEARCH.md **Pattern 3** (L303-306); `ironrdp-input` `Database`/`Operation` → `FastPathInputEvent`.

**Pattern:** `tokio::time::interval(Duration::from_secs(60))` arm (or a feeder task) pushes a well-formed **zero-delta pointer-move** `FastPathInputEvent` through `input_rx` → `process_fastpath_input`. Automatic, no opt-in (D-06).

**Conventions established:** 60 s interval (< any realistic idle timeout); **never build keepalive from keystrokes** (could leak chars into a focused remote field — Pitfall 4, L386-390; Anti-Pattern L349). If a zero-delta move proves insufficient, fall back to an alternating ±1px move (Assumption A1, L476).

---

### `crates/rdpilot/src/session.rs` (service, `Session` handle — SESS-02, D-04/D-07)

**Analog:** none. **Upstream source:** ownership pattern from `rdp.rs`; teardown = RESEARCH.md **Pattern 6** (L340-343).

**Pattern:** `Session::connect(&cfg).await` runs `connect.rs`, spawns the `session_loop.rs` task, returns the handle. `screenshot().await` reads the framebuffer snapshot. `close().await` sends `RdpInputEvent::Close` → `graceful_shutdown()` → await task exit → drop socket. **`Drop` guard** `abort()`s the `JoinHandle` + closes the channel if `close()` wasn't called (best-effort, cannot `await`).

**Conventions established:** SDK owns the loop; consumer never drives the state machine (D-04). One `Session` per process; explicit teardown + `Drop` fallback (D-07; Security V3, L557). Public API shape is the user-signed-off surface (CONTEXT L199-209) — keep it exactly.

---

### `crates/rdpilot/examples/screenshot.rs` (example, binary)

**Analog:** none. **Source:** composes the public API; load `.secrets/connection.json` → `ConnectionConfig` → `Session::connect` → `screenshot` → write PNG. 7-Zip File Manager (Phase 1) is the canonical remote-only program for an eyeball sanity check (CONTEXT L210-211).

**Convention established:** examples may use `anyhow` for app-level errors, but **must not** leak it back into the SDK's public API (RESEARCH L115, L348). **Do not `Read` `.secrets/connection.json` contents into context** (secrets rule; CONTEXT L164).

---

### `crates/rdpilot/tests/live_session.rs` + `tests/crop.rs` (test)

**Analog:** none in Rust. **Precedent:** Phase 1's live-infra Pester tests against the real Azure VM (CONTEXT "Established Patterns", L182-184) — same "validate against the live target" philosophy, now in Rust. **Source:** RESEARCH.md "Validation Architecture" (L515-547).

**Pattern:** integration tests in `tests/live_session.rs` covering all 5 criteria (SESS-01 connect, CAP-01 #2 RGB-correct known-pixel, CAP-01 #3 crop, SESS-02 #4 stays-rendered-idle, #5 10-min keepalive); marked `#[ignore]` and/or early-return when `RDPILOT_LIVE` / `.secrets/connection.json` is absent so default `cargo test` never hard-fails (D-18). Crop math lives in a no-VM unit test (`tests/crop.rs` or `#[cfg(test)]`), always in the default run.

**Conventions established:** gating mechanism = `#[ignore]` + env/secrets presence check (L535); quick run `cargo test -p rdpilot`; full run `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` (L523); a shared helper loads `.secrets/connection.json` and returns `None`/skips when absent (Wave 0, L546). CI-portable now, wired to CI later (D-17).

## Shared Patterns

Cross-cut multiple files. The planner should apply each to every relevant plan.

### Owned SDK types at the boundary
**Source:** D-09; Anti-Pattern L348.
**Apply to:** `lib.rs`, `config.rs`, `screenshot.rs`, `error.rs`, `session.rs`.
**Rule:** Public API exposes only `Session`, `ConnectionConfig`, `Screenshot`, `Rect`, `Error`. `image`, `ironrdp*`, `anyhow`, `rustls` types stay internal — clean semver + future PyO3/MCP marshalling.

### No-panic error policy (API-01)
**Source:** CONTEXT Discretion; RESEARCH L43/L112; Security V5 (L559).
**Apply to:** every `src/*.rs`.
**Rule:** `thiserror` enum + `Result`; **no `unwrap`/`expect`/`panic`** in library code. Bounds-check crop `Rect`, validate config — return `Error`.

### Framebuffer snapshot, never live-share
**Source:** Pattern 2/3; Pitfall 3 (L380-385); Anti-Pattern L345.
**Apply to:** `session_loop.rs`, `framebuffer.rs`, `session.rs`.
**Rule:** Snapshot RGBA on `GraphicsUpdate` into `Arc<Mutex<…>>`/`watch`. Never hold a lock across `.await`; never share the live `DecodedImage`.

### TLS/CredSSP correctness
**Source:** Pitfall 5 (L392-396); Security V6/V9 (L560,L562); MS-CSSP.
**Apply to:** `connect.rs`, `config.rs`.
**Rule:** `Resumption::disabled()`; verify the channel is TLS-upgraded before `connect_finalize`; default cert validation with a **risk-named, default-off** `accept_invalid_certs` opt-out (D-15); never log cert material or credentials.

### Stays-rendered by construction (no Suppress Output PDU)
**Source:** RESEARCH Summary correction #2 (L73); Anti-Pattern L346; D-05/D-08.
**Apply to:** `session_loop.rs` (never emit a Client Suppress Output PDU), `tests/live_session.rs`.
**Rule:** A headless IronRDP client satisfies criterion #4 by construction. `RemoteDesktop_SuppressWhenMinimized` is an mstsc-only key — **do NOT** read the remote registry, **do NOT** introduce WinRM in Phase 2's session path. Verify the *effect* behaviorally (D-08).

### Phase 4 DVC registration seam
**Source:** CONTEXT Integration Points (L189-190); D Discretion; Anti-Pattern L350.
**Apply to:** `connect.rs`.
**Rule:** Leave the `RDPILOT_SENSOR` `DvcProcessor` registration hook on the connector **before** `connect_finalize` (registration is impossible afterward — hard IronRDP constraint). Phase 2 leaves the seam; does NOT build the sensor.

### Committed lockfile
**Source:** D-02; project rule `lockfile-discipline.md`; Package Audit note (L175).
**Apply to:** `Cargo.toml` ×2, `Cargo.lock`.
**Rule:** Commit `Cargo.lock` together with `Cargo.toml` in the same commit. Review new transitive crates on first `cargo build`.

### Test gating (no hard-fail without a target)
**Source:** D-18; Validation Architecture (L535).
**Apply to:** `tests/live_session.rs`, the shared config-loader helper.
**Rule:** Live tests `#[ignore]` + early-skip when `RDPILOT_LIVE`/`.secrets/connection.json` absent. Crop math runs without a VM. Default `cargo test` stays green offline.

## No Analog Found

ALL files in this phase have no in-codebase analog — this is the **first Rust phase**. The planner must use the **UPSTREAM templates cited per file above** (already excerpted with live-source line citations in `02-RESEARCH.md`) as the pattern to copy from.

| File | Role | Data Flow | Reason | Pattern Source |
|------|------|-----------|--------|----------------|
| `Cargo.toml` (×2), `Cargo.lock` | config | n/a | No Rust workspace exists | RESEARCH L124-141, L222-244 |
| `src/lib.rs` | config | n/a | No crate exists | D-09 re-export surface |
| `src/config.rs` | model | transform | No Rust exists | D-12/13/15; `screenshot.rs::build_config` |
| `src/error.rs` | model | n/a | No Rust exists | API-01; `thiserror` |
| `src/connect.rs` | service | request-response | No Rust exists | **Upstream:** `screenshot.rs` + `rdp.rs` (Pattern 1) |
| `src/session.rs` | service | event-driven | No Rust exists | **Upstream:** `rdp.rs`; Pattern 6 |
| `src/session_loop.rs` | service | streaming | No Rust exists | **Upstream:** `ironrdp-client/src/rdp.rs::active_session` (Pattern 2/5) |
| `src/framebuffer.rs` | model | streaming | No Rust exists | Pattern 2 snapshot + Pitfall 3 |
| `src/screenshot.rs` | utility | transform | No Rust exists | **Upstream:** `screenshot.rs` (Pattern 4) + crop math |
| `src/keepalive.rs` | utility | event-driven | No Rust exists | Pattern 3; `ironrdp-input` |
| `examples/screenshot.rs` | example | request-response | No Rust exists | Composes public API |
| `tests/live_session.rs` | test | request-response | No Rust tests exist | RESEARCH L515-547; Phase 1 live-test precedent |
| `tests/crop.rs` | test | transform | No Rust tests exist | Crop math (L447-459) |

## Open Decisions the Planner Must Resolve (carried from RESEARCH.md)

These do not block pattern application but should be surfaced/verified during planning:
1. **Exact async `connect_finalize` signature** in `ironrdp-tokio` 0.9 (Open Q3 / Assumption A3, L494-497) — add a "verify against docs.rs" sub-step in `connect.rs`.
2. **Keepalive event sufficiency** (Assumption A1, L476) — zero-delta pointer move; fall back to ±1px alternating if the 10-min idle test fails.
3. **Does the lab VM have an idle timeout at all** (Open Q1, L484-487) — keepalive runs unconditionally regardless; the test still asserts liveness after 10 min.
4. **`enable_server_pointer`** (Open Q2, L489-492) — keep `false` for Phase 2 (matches the example).
5. **Internal module granularity** — the `src/*.rs` split is advisory; planner picks final boundaries while preserving the public API and the DVC seam.

## Metadata

**Analog search scope:** entire repo — `git status`, `**/*.rs` (Glob, 0 hits), `**/Cargo.toml` (Glob, 0 hits), root dir listing (`infra/`, `CLAUDE.md`, `testResults.xml` only), `.gitignore`, `infra/` contents, Phase 1 `01-PATTERNS.md`.
**Files scanned for analogs:** 0 Rust source files exist (confirmed). Phase 1 `infra/` is PowerShell/Bicep — different language and role, not an analog.
**Pattern extraction date:** 2026-06-05
**Upstream templates (the real "analogs"):** `Devolutions/IronRDP` `crates/ironrdp/examples/screenshot.rs` and `crates/ironrdp-client/src/rdp.rs` — excerpts already captured in `02-RESEARCH.md` (fetched live 2026-06-05).
