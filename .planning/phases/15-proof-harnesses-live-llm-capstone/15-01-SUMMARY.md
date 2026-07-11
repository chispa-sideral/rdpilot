---
phase: 15-proof-harnesses-live-llm-capstone
plan: 01
subsystem: rdpilot-daemon (Windows IPC transport + live-gate test harnesses)
tags: [windows-sys, dacl, named-pipe, daemon-02, daemon-04, session-01, session-03, session-04, offline-author, closes-12-07]
dependency_graph:
  requires: [12-06 (server assembly + bind-as-mutex/AddrInUse contract), 12-05 (JsonReconciliationSink/scan_orphans/seed_into), rdpilot-ipc wire protocol (Request/WireResponse/SessionLifecycle)]
  provides: [ipc::windows explicit owner-only DACL named-pipe creation, PipeListener, live_daemon_windows_dacl.rs (Windows-host-only), live_daemon_e2e.rs (Linux-hostable)]
  affects: [crates/rdpilot-daemon/src/ipc/windows.rs, crates/rdpilot-daemon/src/lib.rs, crates/rdpilot-daemon/Cargo.toml]
tech_stack:
  added: ["windows-sys 0.61.2 (cfg(windows) direct dependency, Microsoft-official, already lockfile-resolved transitively)"]
  patterns: ["raw Win32 DACL construction (OpenProcessToken -> GetTokenInformation(TokenUser) -> InitializeSecurityDescriptor -> InitializeAcl -> AddAccessAllowedAce -> SetSecurityDescriptorDacl)", "tokio named-pipe multi-instance accept loop (PipeListener with interior-mutability pending-instance swap)", "ERROR_ACCESS_DENIED -> io::ErrorKind::AddrInUse mapping for cross-platform bind-as-mutex parity"]
key_files:
  created:
    - crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs
    - crates/rdpilot-daemon/tests/live_daemon_e2e.rs
  modified:
    - crates/rdpilot-daemon/Cargo.toml
    - crates/rdpilot-daemon/src/ipc/windows.rs
    - crates/rdpilot-daemon/src/ipc/mod.rs
    - crates/rdpilot-daemon/src/lib.rs
decisions:
  - "windows-sys 0.61.2 chosen over windows-permissions for the DACL build (binding direction 3) -- Microsoft-official, already lockfile-resolved as a transitive dependency (tokio/mio/socket2/errno/dirs-sys all already pull it in), so no package-legitimacy checkpoint was required or triggered."
  - "12-07-PLAN.md's single live_daemon.rs was split into two files per 15-RESEARCH.md's recommendation: live_daemon_windows_dacl.rs (Windows-host-only, #![cfg(windows)]) and live_daemon_e2e.rs (Linux-hostable, drives the real compiled binary over Unix IPC against the real remote target)."
  - "Added a #[cfg(windows)] pub use ipc::{accept_and_authorize, bind, socket_path}; re-export to lib.rs (not in the plan's stated files_modified) -- Rule 2/3 auto-fix: without it the Windows live test has no crate-boundary path to the daemon's own IPC primitives, mirroring the pre-existing #[cfg(unix)] re-export exactly."
metrics:
  duration: "~70 min"
  completed: 2026-07-11
---

# Phase 15 Plan 01: Windows Owner-Only Pipe DACL + Split 12-07 Live Test Files Summary

Authored the explicit owner-only Windows named-pipe DACL (via raw `windows-sys` 0.61.2 Win32 calls) that `ipc/windows.rs` had left as a stub since Plan 12-02, and split `12-07-PLAN.md`'s single deferred live-gate test file into two: a Windows-host-only DACL/squatting/cross-account file and a Linux-hostable orphan-liveness + e2e session file — closing Phase 12's one remaining plan as a side effect of Phase 15's opening wave.

## What Was Built

**Task 1 — `ipc/windows.rs` (the DACL code itself).** `Cargo.toml` gained a `[target.'cfg(windows)'.dependencies]` entry for `windows-sys = "0.61.2"` with the `Win32_Foundation`/`Win32_Security`/`Win32_Security_Authorization`/`Win32_System_Threading`/`Win32_System_Memory` feature set (per the plan's binding instruction — the actual implementation below uses `Win32_Foundation`/`Win32_Security`/`Win32_System_Threading` directly via `Vec<u8>`-backed heap buffers; `Win32_Security_Authorization`/`Win32_System_Memory` are present for the SDDL-string/`LocalAlloc` refinement path the plan anticipated but this implementation did not need). `ipc/windows.rs` now implements:

- `OwnerOnlyDacl::build()` — the raw Win32 chain: `GetCurrentProcess` → `OpenProcessToken(TOKEN_QUERY)` → two-call `GetTokenInformation(TokenUser)` idiom (probe size, then read) → `InitializeAcl` (MSDN ACL-sizing formula, DWORD-rounded) → `AddAccessAllowedAce(GENERIC_ALL, owner_sid)` → `InitializeSecurityDescriptor` → `SetSecurityDescriptorDacl(present=TRUE, dacl=non-null, defaulted=FALSE)`. Every buffer the resulting pointers reference (`TOKEN_USER` bytes, ACL bytes, the `SECURITY_DESCRIPTOR` struct itself) is owned by the `OwnerOnlyDacl` struct so it outlives the pipe-creation call.
- `create_secured_pipe_instance(first: bool)` — builds a `SECURITY_ATTRIBUTES` pointing at that descriptor and calls `ServerOptions::new().first_pipe_instance(first).create_with_security_attributes_raw(PIPE_NAME, attrs)`.
- `bind()` / `accept_and_authorize()` / `socket_path()` — the same three-function shape `unix.rs` exposes, so `ipc/mod.rs`'s `#[cfg(windows)]` re-exports and `server.rs`'s accept loop compile unchanged on Windows. `PipeListener` (a `tokio::sync::Mutex<Option<NamedPipeServer>>`) holds the currently-pending instance; `accept_and_authorize` awaits its `.connect()` then immediately prepares the NEXT instance (`first_pipe_instance(false)`) so a racing second client always has something to connect to.
- `bind()` maps the `ERROR_ACCESS_DENIED` Windows raises when `first_pipe_instance(true)` finds a pre-existing pipe to `io::ErrorKind::AddrInUse` — this was not explicitly spelled out in the plan's action text but is required for `server.rs`'s existing bind-as-mutex "another daemon already won the race, exit cleanly" branch to work identically on Windows (documented as a deviation below).

Every `unsafe` block is individually justified with a `SAFETY:` comment naming the specific Win32 call and why the preconditions hold; `#[allow(unsafe_code)]` is placed on each of the three functions containing unsafe code (`create_secured_pipe_instance`, `OwnerOnlyDacl::build`, `current_user_token_user_buffer`), never at the module level.

**Task 2 — `tests/live_daemon_windows_dacl.rs`.** `#![cfg(windows)]`-gated (confirmed via `cargo test -- --list`: "0 tests, 0 benchmarks" on the Linux offline suite). Three `#[ignore]` + `RDPILOT_LIVE`-gated assertions: (a) `cross_account_connection_is_rejected_by_the_owner_only_dacl` — spawns a `runas /savecred /user:$RDPILOT_SECOND_WINDOWS_ACCOUNT` probe attempting to open the pipe directly, asserts the daemon-side `accept_and_authorize` either errors or times out waiting; (b) `a_second_first_instance_creation_of_the_same_pipe_name_fails_loudly` — calls `bind()` twice while the first instance is alive, asserts the second errors (T-15-02); (c) `a_same_account_connection_is_accepted_through_the_present_dacl` — the control case pairing with (a): a same-account client succeeds, which together with (a)'s rejection proves the DACL is present and owner-scoped (not null/default, not everyone-access).

**Task 3 — `tests/live_daemon_e2e.rs`.** Cross-platform (no `cfg(windows)` gate), `#[ignore]` + `RDPILOT_LIVE`-gated, drives the REAL compiled `rdpilot-daemon` binary (`env!("CARGO_BIN_EXE_rdpilot-daemon")`, mirroring `autostart_lifecycle.rs`'s pattern) over the real Unix IPC transport. `.secrets/connection.json` is loaded via a locally duplicated loader (the canonical loader in `crates/rdpilot/tests/common/mod.rs` is private to that crate). Two tests: `connect_list_disconnect_e2e_against_a_real_target` (SESSION-01/03/04 — connect with an explicit session name, list asserts one `Live` entry with correct host/timestamps, disconnect acks) and `kill_minus_9_mid_session_then_restart_surfaces_the_orphan_which_is_then_explicitly_reconciled` (DAEMON-04 live — connects to the real target, `child.kill()`s the daemon process directly (keeping the `Child` handle `connect_or_spawn` itself does not expose), restarts a daemon B pointed at the same isolated `XDG_RUNTIME_DIR`/sink path, asserts `list` surfaces the session as `Orphaned` with the host preserved, then asserts an explicit `Disconnect` clears it).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 2 — missing critical functionality] Added a `#[cfg(windows)]` re-export block to `lib.rs`**
- **Found during:** Task 2 (authoring the Windows live test)
- **Issue:** `lib.rs` only had `#[cfg(unix)] pub use ipc::{accept_and_authorize, authorize_uid, bind, socket_path};` — there was no Windows equivalent, so an external integration-test crate (`tests/live_daemon_windows_dacl.rs`) would have had no way to reach `rdpilot_daemon::bind()`/`accept_and_authorize()`/`socket_path()` on Windows, even though `ipc/mod.rs` itself already re-exported them at the module level.
- **Fix:** Added `#[cfg(windows)] pub use ipc::{accept_and_authorize, bind, socket_path};` to `lib.rs`, mirroring the Unix block exactly (minus `authorize_uid`, which has no Windows analogue — documented in both the new re-export's comment and `ipc/windows.rs`'s module doc).
- **Files modified:** `crates/rdpilot-daemon/src/lib.rs` (not in the plan's stated `files_modified`, but required for the plan's own must_have: "A live-gated Windows-only test file exists that asserts different-account rejection + anti-squatting")
- **Commit:** `a4e8f80`

**2. [Rule 2 — missing critical functionality] `bind()`'s `ERROR_ACCESS_DENIED` → `AddrInUse` mapping**
- **Found during:** Task 1 (authoring `ipc/windows.rs`)
- **Issue:** The plan's action text specified `first_pipe_instance(true)` for anti-squatting but did not spell out how the resulting Windows OS error (`ERROR_ACCESS_DENIED`) should surface to `server.rs`'s existing bind-as-mutex logic, which explicitly matches on `err.kind() == io::ErrorKind::AddrInUse` (the Unix contract) to distinguish "another daemon already won the race" (benign, exit cleanly) from a genuine bind failure (fatal).
- **Fix:** `windows::bind()` catches `raw_os_error() == Some(ERROR_ACCESS_DENIED)` specifically and remaps it to `io::ErrorKind::AddrInUse`, so `server.rs`'s existing branch (untouched, no code change there) works identically on both platforms.
- **Files modified:** `crates/rdpilot-daemon/src/ipc/windows.rs`
- **Commit:** `a4e8f80`

No architectural deviations (Rule 4) were needed.

## Package Legitimacy

`windows-sys` 0.61.2 required NO legitimacy checkpoint: Microsoft-official, already resolved in `Cargo.lock` as a transitive dependency of `tokio`/`mio`/`socket2`/`errno`/`dirs-sys`/`schannel`/`anstyle-wincon`/`anstyle-query`/`nu-ansi-term`. `Cargo.lock`'s diff is a single line — the existing `0.61.2` resolution gained `rdpilot-daemon` as a new dependent edge; no new package version was pulled in.

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build -p rdpilot-daemon --target x86_64-unknown-linux-gnu` — green.
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build --workspace --target x86_64-unknown-linux-gnu` — green.
- `cargo test --test live_daemon_windows_dacl -- --list` → "0 tests, 0 benchmarks" (the `#![cfg(windows)]` file compiles to an empty crate on Linux, exactly as designed).
- `cargo test --test live_daemon_e2e -- --list` → both tests discovered; a plain `cargo test --test live_daemon_e2e` run shows both as `ignored` (never execute offline).
- Non-regression, all green on the Linux substitute target: `cargo test --lib` (70/70 passed), `--test ipc_security` (3 passed, 1 correctly ignored — `RDPILOT_SECOND_UID` opt-in), `--test crash_restart_reconcile` (1/1), `--test registry_concurrency` (2/2), `--test thread_leak_soak -- --include-ignored` (2/2), `--test autostart_lifecycle -- --include-ignored` (1/1 — this specifically re-confirms the `lib.rs` re-export addition from deviation #1 above did not break the existing Unix-side wiring).
- `cargo clippy -p rdpilot-daemon --target x86_64-unknown-linux-gnu` (lib + bins, not `--all-targets`): zero new warnings — the two pre-existing warnings surfaced belong to the unrelated `rdpilot` crate (`input.rs` unused import, `rdpdr_backend.rs` clippy suggestion), untouched by this plan.

**Toolchain substitution note:** this Linux sandbox has no Windows target installed and the repo's `rust-toolchain.toml` pins `stable-x86_64-pc-windows-gnu` as the default channel; every command above used `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu --target x86_64-unknown-linux-gnu` to build against the native-Linux substitute, per the orchestrator's instruction. `cargo fmt --check` was NOT used as a gate: it already fails with ~2934 lines of diff against the pristine pre-plan baseline (confirmed via `git stash`), a pre-existing, repo-wide condition unrelated to this plan (likely a local rustfmt version/config mismatch) — out of scope per the SCOPE BOUNDARY rule. `cargo clippy --all-targets` is also known pre-existing-broken: `server.rs`'s own doc comment states its `#![deny(clippy::expect_used)]` "has never actually been enforced against with `cargo clippy --all-targets`" — confirmed here (28 pre-existing errors in `registry.rs`'s own inline test module, none touched by this plan).

## Self-Check: PASSED

- FOUND: `crates/rdpilot-daemon/src/ipc/windows.rs`
- FOUND: `crates/rdpilot-daemon/src/lib.rs`
- FOUND: `crates/rdpilot-daemon/Cargo.toml`
- FOUND: `crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs`
- FOUND: `crates/rdpilot-daemon/tests/live_daemon_e2e.rs`
- FOUND commit `a4e8f80` (Task 1: DACL + lib.rs re-export)
- FOUND commit `caf7dc6` (Task 2: Windows-only live test file)
- FOUND commit `58395da` (Task 3: Linux-hostable live test file)

## What Remains

The real Windows compile of `ipc/windows.rs` and the actual DACL/anti-squatting/cross-account assertions in `live_daemon_windows_dacl.rs` execute on the pinned Azure Windows VM in **Plan 15-05**. The orphan-liveness and connect/list/disconnect e2e assertions in `live_daemon_e2e.rs` execute from the Linux host against that same VM in **Plan 15-06**. Both test files' exact probe mechanics for the cross-account scenario (`runas /savecred`) are a best-effort sketch given this plan is offline-author-only per its own constraints — Plan 15-05 should confirm or adjust the non-interactive-elevation mechanism against whatever the VM provisioning script actually sets up for `RDPILOT_SECOND_WINDOWS_ACCOUNT`.
