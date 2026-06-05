---
phase: 02-rdp-session-framebuffer-core
plan: 02
subsystem: session
tags: [rust, ironrdp, rustls, credssp, tokio, dvc, framebuffer, sdk]

# Dependency graph
requires:
  - phase: 02-rdp-session-framebuffer-core
    provides: "Plan 01 owned no-VM types (Error/Result, ConnectionConfig, Screenshot/Rect), the IronRDP dependency baseline, and the GNU x86_64 toolchain"
provides:
  - "Internal async connect path: TCP → connect_begin → tokio-rustls TLS upgrade (resumption disabled, D-15 cert policy) → mark_as_upgraded → connect_finalize (SESS-01)"
  - "RDPILOT_SENSOR DVC registration seam (DrdynvcClient on the connector before connect_begin) for Phase 4 (not exercised)"
  - "SDK-owned active-session loop: split reader/writer, DecodedImage(RgbA32), tokio::select! over read_pdu / input_rx / keepalive; GraphicsUpdate snapshot; DeactivateAll reactivation (SESS-02, CAP-01)"
  - "Internal SharedFrame (Arc<Mutex<FrameSnapshot>>) latest-frame snapshot, cloned by screenshot()"
  - "Automatic 60s zero-delta pointer-move keepalive (D-06)"
  - "Public Session handle: connect(), screenshot(), close(), Drop guard (D-04/D-07) — owned SDK types only (D-09)"
affects: [02-03-screenshot-wiring-live-suite, phase-3-input, phase-4-dvc-sensor]

# Tech tracking
tech-stack:
  added:
    - "rustls-native-certs 0.8.4 (platform-root store for the default validating cert path)"
  patterns:
    - "Dedicated background thread + current-thread Tokio runtime drives the session loop (sidesteps the &dyn PduHint HRTB non-Send limitation of tokio::spawn)"
    - "Type-erased UpgradedStream (Box<dyn AsyncReadWrite + Send + Sync + Unpin>) keeps rustls/tokio-rustls out of the internal seams"
    - "Snapshot-on-GraphicsUpdate: the loop copies the full RGBA frame into SharedFrame; readers clone, lock never held across .await (Pitfall 3)"
    - "Keepalive built as a direct PointerFlags::MOVE PDU (bypasses Database move-dedup) so a zero-delta tick still emits a well-formed null event"

key-files:
  created:
    - "crates/rdpilot/src/connect.rs"
    - "crates/rdpilot/src/framebuffer.rs"
    - "crates/rdpilot/src/keepalive.rs"
    - "crates/rdpilot/src/session_loop.rs"
    - "crates/rdpilot/src/session.rs"
  modified:
    - "crates/rdpilot/src/lib.rs"
    - "crates/rdpilot/Cargo.toml"
    - "Cargo.lock"

key-decisions:
  - "Enable ironrdp umbrella features (connector/session/graphics/input/dvc/svc) — the facade defaults to only core+pdu, so the connect/session types were otherwise inaccessible."
  - "Use ironrdp_tokio::reqwest::ReqwestNetworkClient (enable the ironrdp-tokio `reqwest` feature) instead of sspi's network_client — the umbrella sspi does not expose ReqwestNetworkClient without its own feature."
  - "Drive the session loop on a dedicated OS thread with a current-thread runtime, not tokio::spawn — the reactivation step holds a Sequence::next_pdu_hint() -> Option<&dyn PduHint> borrow across an .await, which the compiler cannot prove Send (HRTB limitation), so tokio::spawn rejects it."
  - "Add rustls-native-certs for the default (validating) cert path; the upstream example only demonstrates the no-op verifier."
  - "Rebuild the fast-path processor with bulk_decompressor: None on DeactivateAll reactivation — the server reinitializes compression after a Deactivate-All."

patterns-established:
  - "SDK owns the RDP state machine on a background thread; consumers only call connect/screenshot/close (D-04)."
  - "Owned-types boundary extends to internal seams: the upgraded stream is type-erased so no rustls/tokio-rustls type appears even in crate-internal signatures."
  - "Framebuffer integrity contract: full-frame snapshot on every GraphicsUpdate; screenshot() never touches the live DecodedImage."

requirements-completed: [SESS-01, SESS-02, CAP-01]

# Metrics
duration: ~70min
completed: 2026-06-05
---

# Phase 2 Plan 02: Live RDP Session + Framebuffer Core Summary

**The SDK now drives a full IronRDP connection to an authenticated active session over TLS/CredSSP, runs an SDK-owned background loop that pumps PDUs and snapshots the framebuffer on every GraphicsUpdate, keeps the session alive with an automatic 60s null-input keepalive, handles DeactivateAll reactivation, and exposes the user-signed-off `Session::connect/screenshot/close` async API with a Drop guard — compiling clean and green with 21 offline tests, no live VM required.**

## Performance

- **Duration:** ~70 min
- **Started:** 2026-06-05
- **Completed:** 2026-06-05
- **Tasks:** 3 (all `tdd="true"`, all autonomous)
- **Files:** 5 created, 3 modified

## Accomplishments

- **Task 1 — connect path (SESS-01):** Async `connect()` mirroring the canonical IronRDP sequence: TCP → `connect_begin` → `tokio-rustls` TLS upgrade → `mark_as_upgraded` → `connect_finalize` (ironrdp-tokio 0.9). Cert policy selected by `accept_invalid_certs` (default validates via `rustls-native-certs`; risk-named opt-out installs the no-op verifier, D-15). `Resumption::disabled()` on the rustls config (CredSSP/MS-CSSP, Pitfall 5). `RDPILOT_SENSOR` DVC seam: `DrdynvcClient` registered on the connector before `connect_begin`, with a commented Phase 4 insertion site. Server public key extracted via `x509-cert` DER decode. The TLS-upgraded stream is type-erased (`Box<dyn AsyncReadWrite + Send + Sync + Unpin>`) so no `rustls`/`tokio-rustls` type leaks into the SDK seams.
- **Task 2 — loop + framebuffer + keepalive (SESS-02, CAP-01, D-06):** `session_loop::run` splits the framed transport into independent reader/writer, owns the only mutating `DecodedImage(RgbA32)`, and `tokio::select!`s over `read_pdu()` / `input_rx.recv()` / the keepalive tick. Dispatches `ResponseFrame` (write back), `GraphicsUpdate` (snapshot the full RGBA frame into `SharedFrame`), `DeactivateAll` (run the reactivation sequence, rebuild the framebuffer at the new desktop size + fast-path processor + share_id — Pitfall 2, criterion #4), and `Terminate` (exit). `SharedFrame` is an `Arc<Mutex<FrameSnapshot>>` cloned by readers — the lock is never held across an `.await` (Pitfall 3). Keepalive is a direct zero-delta `PointerFlags::MOVE` PDU every 60s (never a keystroke, Pitfall 4).
- **Task 3 — Session handle (D-04/D-07):** `pub struct Session` runs the connect path, drives the loop on a dedicated background thread, and returns the handle. `screenshot()` clones the latest snapshot into an owned `Screenshot`. `close()` sends `RdpInputEvent::Close` (→ `graceful_shutdown`) and joins the loop via `spawn_blocking`. `Drop` closes the channel to signal the loop to exit (best-effort, no await). The public surface is owned SDK types only — no `ironrdp`/`image`/`rustls`/`tokio` leakage.
- **Green offline:** `cargo build`, `cargo clippy` (lib 0 warnings), and `cargo doc --no-deps` all exit 0; 21 unit tests pass with no live target.

## Task Commits

1. **Task 1: Connect + NLA/CredSSP over TLS, resumption disabled, DVC seam** — `7c80b4c` (feat)
2. **Task 2: Active-session loop + framebuffer snapshot + keepalive** — `962f737` (feat)
3. **Task 3: Session handle — connect/screenshot/close + Drop guard** — `3dc82e1` (feat)

_All three tasks are `tdd="true"`; for offline-testable code the unit tests (cert-policy branch, FrameSnapshot round-trip, keepalive shape, screenshot/Drop paths) live inline in each module and were authored alongside the implementation (RED/GREEN verified locally before each commit). The live connect/idle/keepalive behavior is exercised by Plan 03's gated suite, not here._

## Files Created/Modified

- `crates/rdpilot/src/connect.rs` (new) — connect path, TLS upgrade, cert policy, DVC seam, server-key extraction; cert-policy + seam unit tests
- `crates/rdpilot/src/framebuffer.rs` (new) — `FrameSnapshot` + `SharedFrame`; round-trip/latest-wins/clone-share unit tests
- `crates/rdpilot/src/keepalive.rs` (new) — `null_input_event()` + `KEEPALIVE_INTERVAL`; pointer-move/never-keystroke unit tests
- `crates/rdpilot/src/session_loop.rs` (new) — `run()` active-session pump + `reactivate()`; `RdpInputEvent` control enum; keepalive-source unit test
- `crates/rdpilot/src/session.rs` (new) — `pub struct Session` (connect/screenshot/close + Drop); screenshot/Drop unit tests
- `crates/rdpilot/src/lib.rs` — declared the new internal modules; `pub use session::Session`
- `crates/rdpilot/Cargo.toml` — enabled ironrdp features + ironrdp-tokio `reqwest`; added `rustls-native-certs`
- `Cargo.lock` — locked `rustls-native-certs 0.8.4` and its small transitive set (committed with the manifest)

## Decisions Made

- **Enable ironrdp umbrella features.** `ironrdp = "0.15"` defaults to only `core`+`pdu`; the connect state machine (`connector`), the active session (`session` → `ActiveStage`/`ActiveStageOutput`), the framebuffer (`graphics` → `DecodedImage`/`PixelFormat`), `input`, `dvc`, and `svc` are all behind features. Enabled exactly those six.
- **`ironrdp_tokio::reqwest::ReqwestNetworkClient`.** `connect_finalize` needs a `NetworkClient`; the umbrella `sspi` re-export does not expose `ReqwestNetworkClient` without its `network_client` feature. The supported seam is the `reqwest` feature on `ironrdp-tokio`.
- **Dedicated thread + current-thread runtime for the loop.** See Deviations — the load-bearing structural decision.
- **`rustls-native-certs` for the validating path.** The default (cert-validating) branch needs a real root store; the upstream example only shows the no-op verifier. Added the standard companion crate.
- **`bulk_decompressor: None` on reactivation.** After a Deactivate-All the server reinitializes compression; rebuilding the fast-path processor without a carried-over decompressor is the correct reset.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 — Blocking] Enabled ironrdp feature flags + the ironrdp-tokio `reqwest` feature**
- **Found during:** Task 1 (`cargo build`).
- **Issue:** The plan's bare `ironrdp = "0.15"` exposes only `core`+`pdu`, so `ironrdp::connector`, `::session`, `::graphics`, `::input`, `::dvc` did not resolve. Separately, `connect_finalize` needs a `NetworkClient`, but `sspi::network_client::reqwest_network_client` is gated behind sspi's `network_client` feature.
- **Fix:** Enabled `features = ["connector","session","graphics","input","dvc","svc"]` on `ironrdp`, and `features = ["reqwest"]` on `ironrdp-tokio` (providing `ironrdp_tokio::reqwest::ReqwestNetworkClient::new()`).
- **Files modified:** `crates/rdpilot/Cargo.toml`, `Cargo.lock` (committed in `7c80b4c`)
- **Verification:** `cargo build` exits 0; the connect path compiles against the real types.

**2. [Rule 3 — Blocking] Added `rustls-native-certs` for the default validating cert path**
- **Found during:** Task 1 (writing the cert-policy branch).
- **Issue:** The plan/example only demonstrate the no-op verifier; the default (validate) branch requires a platform root store, which rustls 0.23 does not ship built-in.
- **Fix:** `cargo add rustls-native-certs` (0.8.4); the default branch loads native roots into a `RootCertStore`. If no roots are available it returns `Error::Tls` (never panics).
- **Files modified:** `crates/rdpilot/Cargo.toml`, `Cargo.lock` (committed in `7c80b4c`)
- **Verification:** `cargo build`/`cargo test` green; cert-policy unit test covers both branches offline.

**3. [Rule 3 — Blocking] Drive the session loop on a dedicated thread instead of `tokio::spawn`**
- **Found during:** Task 3 (wiring `Session::connect`).
- **Issue:** `tokio::spawn` requires the future to be `Send`. The reactivation step calls `single_sequence_step_read`, which internally holds a `Sequence::next_pdu_hint() -> Option<&dyn PduHint>` borrow across an `.await`. The compiler cannot prove `&dyn PduHint` `Send` "for all lifetimes" (a known higher-ranked-lifetime limitation — the underlying `PduHint: Send + Sync` type *is* Send, but the HRTB inference fails), so `tokio::spawn` rejects the loop. Boxing as `Pin<Box<dyn Future + Send>>` did not help (same HRTB).
- **Fix:** Spawn a named OS thread (`rdpilot-session`) that builds a `new_current_thread` Tokio runtime and `block_on`s the loop. A current-thread runtime imposes no `Send` bound on its futures, so the loop runs cleanly. The constraint is fully contained inside `Session` — the public async API (`connect`/`screenshot`/`close`) is unchanged. `close()` joins the thread via `spawn_blocking`; `Drop` closes the channel to signal exit.
- **Files modified:** `crates/rdpilot/src/session.rs` (committed in `3dc82e1`)
- **Verification:** `cargo build`/`clippy`/`test` green; screenshot/Drop unit tests pass.

**4. [Rule 1 — Scope/clarity] Dropped the unused `RdpInputEvent::FastPath` variant**
- **Found during:** Task 3 (dead-code warning).
- **Issue:** The plan's Pattern 3 suggested pushing the keepalive null FastPath "through the same `input_rx`". Implementing keepalive as its own `select!` arm (cleaner — no self-send round-trip, no channel-capacity coupling) left `RdpInputEvent::FastPath` unconstructed, producing a dead-code warning.
- **Fix:** Removed the unused variant (Phase 2 only needs `Close`), documenting that Phase 3 input injection will reintroduce a fast-path channel event. Keepalive behavior is unchanged and correct.
- **Files modified:** `crates/rdpilot/src/session_loop.rs` (committed in `3dc82e1`)
- **Verification:** `cargo build` 0 warnings.

**5. [Rule 1 — Bug, trivial] clippy `identity_op` + redundant-deref fixes**
- **Found during:** Task 3 (clippy `--all-targets`).
- **Issue:** A framebuffer test used `vec![9; 1 * 1 * 4]` (`identity_op`); the reactivation call passed `&mut *activation` where deref coercion suffices.
- **Fix:** `vec![9; 4]` and `&mut activation`. clippy clean.
- **Files modified:** `crates/rdpilot/src/framebuffer.rs`, `crates/rdpilot/src/session_loop.rs` (committed in `3dc82e1`)
- **Verification:** `cargo clippy --all-targets` produces no warnings.

---

**Total deviations:** 5 (3 blocking dependency/structural, 2 trivial cleanup). The dedicated-thread decision (#3) is the only material structural change; it is fully encapsulated and does not alter the public API or any locked decision.

## Issues Encountered

- **`&dyn PduHint` non-Send across `.await`** — the central engineering obstacle. Diagnosed from the exact compiler note ("`Send` would have to be implemented for the type `&dyn PduHint` ... but is actually implemented for `&'0 dyn PduHint`, for some specific lifetime"). Resolved structurally (Deviation #3) rather than by weakening any bound or skipping the reactivation path.

## Resolved Open Questions / Assumptions

- **A3 (connect_finalize signature):** Verified against the cached `ironrdp-async` 0.9 source — the async `connect_finalize` argument order is identical to the blocking example (`upgraded, connector, &mut framed, &mut network_client, server_name.into(), server_public_key, None`).
- **Open Q2 (`enable_server_pointer`):** kept `false` (matches the headless example); `Pointer*` outputs are ignored in the loop.
- **A1 (keepalive sufficiency):** ships as a zero-delta pointer move; the documented ±1px-alternating fallback is noted in `keepalive.rs` for Plan 03's live idle test.

## Known Stubs

- **`RDPILOT_SENSOR` DVC seam** — intentional, plan-mandated. `DrdynvcClient::new()` is registered as the empty DRDYNVC host channel before `connect_begin`, with a commented Phase 4 insertion site. No sensor processor is built (Phase 4 owns it); the seam is not exercised in Phase 2. This is the documented Phase 4 hook, not an incomplete feature.
- No data-flow stubs: `screenshot()` is fully wired to the live snapshot path. It returns `Error::Session` only until the first `GraphicsUpdate` arrives (correct behavior, exercised by the live suite in Plan 03).

## Threat Flags

None — no security-relevant surface beyond the plan's threat model. The mitigations are implemented:
- **T-02-03** (server-cert trust): default validating verifier via `rustls-native-certs`; `accept_invalid_certs` risk-named, default off, installs the no-op verifier only.
- **T-02-04** (TLS downgrade/resumption): `Resumption::disabled()` on the rustls config.
- **T-02-02** (credential/cert logging): no credential or cert field is `tracing`-logged anywhere in `connect.rs`/`session*.rs`.
- **T-02-05** (stale/wrong-size framebuffer): snapshot-on-GraphicsUpdate + DeactivateAll reactivation rebuild.
- **T-02-06** (keepalive stray input / idle disconnect): zero-delta pointer move, never a keystroke; 60s interval.
- **T-02-07** (task/socket leak on drop): `Drop` closes the channel → loop exits → transport drops; `close()` is the clean awaited path.

## User Setup Required

None — no external service configuration. The live RDP target + `.secrets/connection.json` are only needed for Plan 03's gated suite (absent here, by design — D-18).

## Next Phase Readiness

- The full live machinery is in place and green offline: connect/auth, SDK-owned loop, framebuffer snapshot, keepalive, reactivation, and the `Session` handle.
- **Plan 03** can now wire `Screenshot` to a real connection and add the gated integration suite (SESS-01 connect, CAP-01 #2 RGB-correct, CAP-01 #3 crop, SESS-02 #4 stays-rendered-idle, #5 10-min keepalive). It should validate Assumption A1 (zero-delta keepalive) during the 10-min idle test and switch to the ±1px fallback if needed.
- **Phase 4** can register the `RDPILOT_SENSOR` `DvcProcessor` at the seam in `connect.rs` (before `connect_begin`) via `DrdynvcClient::new().with_dynamic_channel(...)`.

## Self-Check: PASSED

- All five created files exist on disk (`connect.rs`, `framebuffer.rs`, `keepalive.rs`, `session_loop.rs`, `session.rs`).
- All three task commits are present in git history: `7c80b4c`, `962f737`, `3dc82e1`.
- `cargo build`/`cargo clippy` (lib) exit 0 with no warnings; 21 offline tests pass; `cargo doc --no-deps` exits 0.

---
*Phase: 02-rdp-session-framebuffer-core*
*Completed: 2026-06-05*
