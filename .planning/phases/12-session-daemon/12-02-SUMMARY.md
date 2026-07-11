---
phase: 12-session-daemon
plan: 02
subsystem: rdpilot-daemon
tags: [daemon, scaffold, seams, error-mapping]
dependency-graph:
  requires:
    - rdpilot_ipc::Request::Connect/List/Disconnect (12-01)
    - rdpilot_ipc::WireErrorCode::DuplicateSession (12-01)
  provides:
    - crates/rdpilot-daemon (crate scaffold, workspace member)
    - rdpilot_daemon::{SessionConnector, ManagedSession, ReconciliationSink, SessionEntry, DaemonError}
    - rdpilot_daemon::seams::RealConnector
    - rdpilot_daemon::error_map::wire_code_for_sdk_error
    - "impl From<DaemonError> for WireError"
  affects:
    - crates/rdpilot-daemon/src/registry.rs (Plan 12-03, consumes seams)
    - crates/rdpilot-daemon/src/dispatch.rs (Plan 12-04, consumes error_map)
tech-stack:
  added:
    - "rdpilot-daemon crate: tokio 1 (full), serde/serde_json, directories 6.0.0, thiserror 2, libc 0.2.186 (unix), sysinfo 0.39.6 (dev)"
  patterns:
    - "Manual boxed-future trait methods (Pin<Box<dyn Future<Output=T> + 'a>>, no async-trait dep) for dyn-object-safe async traits"
    - "BoxFuture deliberately NOT +Send — rdpilot::Session::connect's future is provably non-Send (HRTB PduHint limitation reaches the connect path, not only the reactivation loop)"
key-files:
  created:
    - crates/rdpilot-daemon/Cargo.toml
    - crates/rdpilot-daemon/src/main.rs
    - crates/rdpilot-daemon/src/lib.rs
    - crates/rdpilot-daemon/src/seams.rs
    - crates/rdpilot-daemon/src/error_map.rs
    - crates/rdpilot-daemon/src/{registry,dispatch,reconcile,lifecycle,autostart,server}.rs
    - crates/rdpilot-daemon/src/ipc/{mod,unix,windows,framing}.rs
  modified:
    - Cargo.toml (workspace members)
decisions:
  - "BoxFuture (seams.rs) drops the +Send bound — rdpilot::Session::connect's future is not Send; downstream registry/dispatch (Plans 12-03/04/06) must drive session-touching async code on a single-threaded tokio::task::LocalSet + spawn_local, never bare tokio::spawn"
metrics:
  duration: "~50m"
  completed: "2026-07-11"
---

# Phase 12 Plan 02: Daemon Crate Scaffold + Seams + Error Mapping Summary

Scaffolded the `rdpilot-daemon` crate (binary + lib, added to the workspace), delivered the full module skeleton for Plans 12-03 through 12-07, and implemented the two REAL, tested interfaces every downstream plan builds against: the session/reconciliation seams (`SessionConnector`/`ManagedSession`/`ReconciliationSink`/`SessionEntry`/`DaemonError`) and the `rdpilot::Error -> WireErrorCode` / `DaemonError -> WireError` mapping (the Phase-11-deferred half of D-28).

## What was built

**Task 1 — crate scaffold:** `crates/rdpilot-daemon` added to the workspace `members`. `Cargo.toml` pins exactly the research-verified dependencies via the tokio-native transport path (no `interprocess`, no `windows-permissions` — both correctly deferred): `rdpilot`/`rdpilot-ipc`/`rdpilot-config` (path deps), `tokio` (`features = ["full"]`), `serde`/`serde_json`, `directories = "6.0.0"`, `thiserror = "2"`, `libc = "0.2.186"` (unix-only), `sysinfo = "0.39.6"` (dev-only). `main.rs` is a thin `#[tokio::main]` wrapper. `lib.rs` mirrors `rdpilot-ipc`'s crate-lint header (`deny(unsafe_code)`/`unwrap_used`/`expect_used`) and declares all ten modules (`registry`, `dispatch`, `reconcile`, `lifecycle`, `autostart`, `server`, `ipc::{unix,windows,framing}`) as minimal compiling stubs, re-exporting `server::run` and the seam types.

**Task 2 — seams (`seams.rs`):** `SessionConnector`/`ManagedSession` are manual boxed-future traits (no `async-trait` dependency added — native async-fn-in-traits are not `dyn`-object-safe without hand-boxing the returned future, and the registry needs `Arc<dyn SessionConnector>`/`Box<dyn ManagedSession>`). `ReconciliationSink` is synchronous by design (no lock held across `.await` in teardown paths). `RealConnector` delegates to `rdpilot::Session::connect`; `impl ManagedSession for rdpilot::Session` delegates `close()` to `Session::close` (joins the OS thread) and `describe()` returns `SessionLifecycle::Live` as the Plan-02 baseline. `SessionEntry` has `Connecting`/`Live`/`Orphaned` variants plus a `to_status()` render method into `rdpilot_ipc::SessionStatus`. `DaemonError` (`thiserror`, `#[non_exhaustive]`) covers `DuplicateSession`/`SessionNotFound`/`StillConnecting`/`Sdk`/`Connect`/`Io`, with `Display` verified credential-free by a planted-sentinel regression test.

**Task 3 — error mapping (`error_map.rs`):** `wire_code_for_sdk_error` maps `rdpilot::Error::PathTraversal`/`ChecksumMismatch` 1:1 and every one of the other 11 named variants (`Connect`, `Tls`, `Decode`, `Encode`, `CropOutOfBounds`, `Config`, `Session`, `CoordinateOutOfBounds`, `Dvc`, `Bootstrap`, `SensorRejected`) explicitly to `Internal`. `impl From<DaemonError> for WireError` maps `DuplicateSession -> DuplicateSession`, `SessionNotFound`/`StillConnecting -> SessionNotFound`, `Sdk(e) -> wire_code_for_sdk_error(e)`, `Connect`/`Io -> Internal`.

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build -p rdpilot-daemon --target x86_64-unknown-linux-gnu` — succeeds, zero warnings in this crate's own code.
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu` — 13/13 tests green (seams + error_map).
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo clippy -p rdpilot-daemon --target x86_64-unknown-linux-gnu --all-targets` — zero warnings in this crate's own code (pre-existing, out-of-scope `rdpilot` warnings unrelated to this plan).
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build --workspace --target x86_64-unknown-linux-gnu` — full workspace builds clean.
- `crates/rdpilot-daemon/Cargo.toml` contains neither `interprocess` nor `windows-permissions`.

## Deviations from Plan

**1. [Rule 3 — blocking issue] `BoxFuture` drops the `+ Send` bound the plan's literal async-fn-style trait signatures implied.**

- **Found during:** Task 2, while implementing `RealConnector::connect`.
- **Issue:** Wrapping `rdpilot::Session::connect(&cfg).await` inside `Box::pin(async move { ... })` with a `+ Send` bound on the boxed future produced a hard compile error: `implementation of Send is not general enough` / `Send would have to be implemented for &dyn ironrdp_pdu::PduHint`. A compile probe (`assert_send::<rdpilot::Session>()` — passed; a probe wrapping `Session::connect(&cfg)` directly in `assert_send` — failed) confirmed the *future* returned by `Session::connect` is not `Send`, while the `Session` struct itself is. This is the same higher-ranked-lifetime `&dyn PduHint`-across-`.await` limitation `crates/rdpilot/src/session.rs`'s own doc comment documents for the session loop's reactivation step — it turns out to reach the initial connect handshake too, not only the ongoing loop. Since a `dyn Future` trait object erases auto-trait information unless declared on the object type, requiring `+ Send` in the trait's return type made `RealConnector` (the production implementation) impossible to write at all — a hard blocker for a Task 2 acceptance criterion (`impl SessionConnector for RealConnector` must exist and compile).
- **Fix:** Removed `+ Send` from the `BoxFuture<'a, T>` type alias used by both `SessionConnector::connect` and `ManagedSession::close`. The trait supertrait bounds (`SessionConnector: Send + Sync + 'static`, `ManagedSession: Send + 'static`) are unchanged and remain satisfiable (`RealConnector` is a trivially-Send/Sync ZST; `rdpilot::Session` itself is confirmed `Send` by the probe) — only the *future returned by an async operation* needed the relaxation, not the connector/session values themselves.
- **Consequence documented on the type alias and in the commit message:** any async code path that awaits a `SessionConnector::connect`/`ManagedSession::close` call — directly, or via the registry's `open`/`close` (Plan 12-03) — must run on a single-threaded Tokio context (`tokio::task::LocalSet` + `spawn_local`), never a bare `tokio::spawn` on the default multi-threaded runtime (which requires `Send` futures). This affects how Plan 12-03's concurrency test (SC#1) and Plan 12-04/12-06's dispatch/server assembly must be structured. This is a workload-appropriate constraint (research: "human-scale connect/disconnect/list traffic, not thousands of req/s") and mirrors `rdpilot::Session`'s own established pattern of sidestepping the identical HRTB limitation via a dedicated single-threaded execution context rather than fighting it — not a new architectural direction, but an existing one now known to apply one layer higher than originally scoped.
- **Files modified:** `crates/rdpilot-daemon/src/seams.rs` (doc comment + type alias only; no additional files).
- **Commit:** `bfaae58`

**2. [Rule 1 — bug/compiler-fact correction] `wire_code_for_sdk_error`'s match on `rdpilot::Error` requires a trailing wildcard arm, not the plan's literal "no wildcard" spec.**

- **Found during:** Task 3.
- **Issue:** `rdpilot::Error` is `#[non_exhaustive]` (an existing, intentional Phase-2/Phase-8 decision — see `crates/rdpilot/src/error.rs`'s doc comment). Rust's compiler REQUIRES a wildcard arm when matching a `#[non_exhaustive]` enum from outside its defining crate, even when every currently-known variant is listed explicitly (E0004 otherwise) — this is a hard language rule, not a style preference, so the plan's literal "EXHAUSTIVE match (no wildcard)" instruction for this specific match is not achievable as written.
- **Fix:** Kept every one of the 13 variants listed explicitly, arm by arm (11 explicit `=> Internal` arms plus the two 1:1 mappings), and added a trailing `_ => WireErrorCode::Internal` wildcard with a doc comment explaining it is unreachable for every variant that exists today and exists solely to satisfy the compiler's non-exhaustive-match rule — the closest compiler-legal approximation of the plan's forcing-function intent for this particular (cross-crate, `#[non_exhaustive]`) match. For `impl From<DaemonError> for WireError`'s match on `DaemonError` (a same-crate type, so NOT subject to the same forced-wildcard rule), the wildcard was correctly omitted — `cargo build` in fact warned `unreachable pattern` when one was present, confirming the same-crate match is genuinely exhaustive without it, fully realizing T-12-05's forcing-function guarantee for that match.
- **Files modified:** `crates/rdpilot-daemon/src/error_map.rs`.
- **Commit:** `64460cc`

## Known Stubs

The six not-yet-implemented modules (`registry.rs`, `dispatch.rs`, `reconcile.rs`, `lifecycle.rs`, `autostart.rs`, `server.rs`, `ipc::{mod,unix,windows,framing}`) are intentional interface-first stubs per the plan's explicit scope ("Do NOT put real logic in the stub bodies — they are filled by Plans 03–06"). `server::run()` returns `Ok(())` unconditionally — this is the ONLY behavior these stubs have, and it is correct: the plan's own acceptance criterion is `cargo build -p rdpilot-daemon` succeeding, not any runtime behavior. Each stub file's doc comment names its owning future plan.

## Threat Flags

None — this plan's `<threat_model>` (T-12-04, T-12-05, T-12-SC) is fully covered as described; the Rule 3 BoxFuture deviation does not introduce new surface (it only affects which Tokio scheduling primitive downstream code must use, not any wire-facing or credential-handling behavior).

## Self-Check: PASSED

- FOUND: crates/rdpilot-daemon/Cargo.toml (contains `directories = "6.0.0"`, `sysinfo = "0.39.6"`, `tokio` with `features = ["full"]`, no `interprocess`)
- FOUND: crates/rdpilot-daemon/src/lib.rs (contains `pub use server::run` + 10 `mod` declarations)
- FOUND: crates/rdpilot-daemon/src/seams.rs (contains `trait SessionConnector`, `trait ManagedSession`, `trait ReconciliationSink`, `enum SessionEntry`, `enum DaemonError`, `struct RealConnector`)
- FOUND: crates/rdpilot-daemon/src/error_map.rs (contains `fn wire_code_for_sdk_error`, `impl From<DaemonError> for WireError`)
- FOUND: root Cargo.toml `members` array contains `crates/rdpilot-daemon`
- FOUND commit d528a26 (Task 1: crate scaffold)
- FOUND commit bfaae58 (Task 2: seams)
- FOUND commit 64460cc (Task 3: error mapping)
