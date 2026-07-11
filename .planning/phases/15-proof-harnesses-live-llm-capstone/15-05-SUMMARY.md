---
phase: 15-proof-harnesses-live-llm-capstone
plan: 05
subsystem: daemon-ipc-windows
tags: [azure, live-gate, windows-sys, dacl, named-pipe, daemon-02, rdpilot-ipc]

# Dependency graph
requires:
  - phase: 15-proof-harnesses-live-llm-capstone
    plan: 01
    provides: "windows-sys owner-only DACL implementation (ipc/windows.rs) + live_daemon_windows_dacl.rs test file, offline-authored, never real-compiled"
provides:
  - "Single live Azure VM (rdpilot-vm, Standard_B2s_v2, westeurope, RG rdpilot-test) provisioned and held UP for Plans 15-06/15-07/15-08"
  - "Byte-verified C# sensor exe (SHA-256 a92a6bd6b29e515f0f734607b13e097329b9acb26b79e21d7b139a8c903725c3, 3,239,936 bytes) saved to .secrets/sensor-build/rdpilot-sensor.exe"
  - "DAEMON-02 Windows half LIVE-VERIFIED: owner-only DACL present, anti-squatting fails loudly, genuine cross-account rejection (ERROR_ACCESS_DENIED) all PASS"
  - "First real Windows compile of ipc/windows.rs (windows-sys 0.61.2) -- 2 genuine API-path bugs found and fixed"
  - "rdpilot-ipc's transport framing (read_frame/write_frame/TransportError) fixed from wrongly-Unix-only to cross-platform"
  - "3 daemon test files (autostart_lifecycle.rs, ipc_security.rs, live_daemon_e2e.rs) correctly #![cfg(unix)]-gated for the first time"
  - "Scheduled-Task-based cross-account probe pattern (reusable for future Windows live gates needing a genuinely-different-account probe from a non-interactive SYSTEM execution context)"
affects: [15-06, 15-07, 15-08]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Detached-background-process pattern for az vm run-command builds expected to exceed ~5 minutes: Start-Process -WindowStyle Hidden running a wrapper .ps1 that redirects output to a log file and writes a sentinel done-file, then poll cheaply via short separate az vm run-command invoke calls -- az vm run-command invoke itself has an internal ~80-90 minute execution timeout after which it force-kills the remote script independently of whether the invoking local az CLI process is still attached."
    - "Scheduled-Task-based cross-account Windows probe: runas cannot work from az vm run-command's NT AUTHORITY\\SYSTEM / non-interactive Session 0 execution context (requires an interactive window station, fails immediately with exit 1 without ever attempting the probe -- a false-positive PASS risk). A one-shot schtasks /create + /run as the target account, with results written to a shared result file (not stdout, which schtasks cannot relay back), is the working non-interactive mechanism. Requires: (a) the target account has SeBatchLogonRight (e.g. via Backup Operators membership), (b) explicit icacls grants on any file the task needs to read/write (this VM's hardened image scopes C:\\Windows\\Temp writes by SYSTEM to Administrators/SYSTEM only), (c) stripping the UTF-8 BOM PowerShell's Out-File -Encoding utf8 writes before string-prefix assertions."
    - "windows-sys 0.61.2 live-compile-confirmed API surface: OpenProcessToken lives in Win32::System::Threading (not Win32::Security); SECURITY_DESCRIPTOR_REVISION lives in Win32::System::SystemServices (not Win32::Security, needs its own Cargo.toml feature); HANDLE is now type-aliased to *mut c_void (was an integer type in older windows-sys), so a bare 0 literal no longer type-checks as a null handle."

key-files:
  created: []
  modified:
    - crates/rdpilot-daemon/src/ipc/windows.rs
    - crates/rdpilot-daemon/Cargo.toml
    - crates/rdpilot-ipc/src/lib.rs
    - crates/rdpilot-ipc/src/transport.rs
    - crates/rdpilot-ipc/Cargo.toml
    - crates/rdpilot-daemon/tests/autostart_lifecycle.rs
    - crates/rdpilot-daemon/tests/ipc_security.rs
    - crates/rdpilot-daemon/tests/live_daemon_e2e.rs
    - crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs
    - .secrets/connection.json (gitignored, not committed)
    - .secrets/sensor-build/rdpilot-sensor.exe (gitignored, not committed)

key-decisions:
  - "windows-sys 0.61.2's real API surface differs from what offline authoring (Plan 15-01) assumed at 3 points -- OpenProcessToken's module, SECURITY_DESCRIPTOR_REVISION's module, and HANDLE's underlying type. All fixed with no legitimacy-checkpoint implications: windows-sys itself needed no version change, only import paths and one Cargo.toml feature flag."
  - "rdpilot-ipc's transport module's #[cfg(unix)] gate was scoped too broadly (whole-module instead of per-item) since its original authoring (Plan 13-01) predates any Windows IPC consumer. Fixed to unblock rdpilot-daemon's Windows build; the framing functions (read_frame/write_frame) were always meant to be cross-platform per their own doc comments, just never actually compiled that way until this plan."
  - "The cross-account DACL test's runas-based probe (authored offline in Plan 15-01, with the file's own doc anticipating a fallback might be needed) was live-diagnosed as non-functional in this project's actual execution topology (az vm run-command running as SYSTEM in Session 0). Replaced with the scheduled-task fallback the original doc had already named, independently verified via a manual whoami check before committing to the Rust rewrite -- confirming the mechanism genuinely runs as the second account before trusting its result."
  - "Second Windows account 'rdpilot2' was added to the Backup Operators local group (grants SeBatchLogonRight by default local security policy) purely to enable the Scheduled-Task probe mechanism -- this does not weaken the DACL test itself, since Windows named-pipe DACL enforcement is scoped to the specific user SID, independent of group/privilege membership."

requirements-completed: [DAEMON-02]

# Metrics
duration: ~7h (provisioning + sensor build ~30min; Rust toolchain install + first real Windows compile + 4 rounds of live-diagnosed fixes ~6h, dominated by az vm run-command's build-time discovery process and its ~80-90min per-invocation timeout before the detached-process pattern was adopted)
completed: 2026-07-11
---

# Phase 15 Plan 05: Live-Run — Windows-DACL Gate on the Azure VM Summary

**Provisioned the single shared Azure VM for the rest of Phase 15, built and byte-verified the C# sensor on it, and closed DAEMON-02's Windows half live — the first real Windows compile of `ipc/windows.rs` surfaced and fixed 2 genuine `windows-sys` API-path bugs, and the cross-account rejection test's `runas`-based probe was live-diagnosed as fundamentally broken in this project's execution topology and replaced with a working Scheduled-Task mechanism, independently verified before trusting its result.**

## Preflight Result

**PASSED.** Azure CLI authenticated (subscription "Chispa Sideral", `e0f87f51-4d5b-4d6f-9a5e-345283edc4a5`). `infra/manage-env.ps1` + Bicep templates present. Management RG `rdpilot-mgmt` exists (persistent, holds the auto-destroy Automation Account). No missing prerequisites — provisioning proceeded autonomously per binding direction 4.

## VM Provisioned (Task 1)

| Property | Value |
|----------|-------|
| Name | `rdpilot-vm` |
| Resource Group | `rdpilot-test` |
| Size | `Standard_B2s_v2` |
| Region | `westeurope` |
| Public IP | `20.86.100.173` |
| Connection file | `.secrets/connection.json` (fresh, gitignored) |
| Auto-destroy backstop | Confirmed armed: `Delete-ResourceGroup` runbook `Published`, daily schedule `Enabled` (next run 2026-07-12) |
| **Status** | **UP — held for Plans 15-06/15-07/15-08. Do NOT tear down before 15-08's authorized checkpoint.** |

## Sensor Build (Task 2) — COMPLETE

- `.NET 8 SDK 8.0.422` + VS Build Tools (VCTools workload) installed on the fresh WS2022 VM via `az vm run-command invoke` (neither preinstalled — confirms the 10-05 finding still holds).
- C# sensor built on the VM (`dotnet publish -c Release -r win-x64 -p:PublishAot=true --self-contained true`), source embedded as base64 directly in the run-command script body.
- Relayed back via a short-lived Storage blob SAS (container `relay` in the existing `rdpilotcseh86c08l9` storage account, both a write SAS for VM→host and a read SAS for host→VM used through this plan's multiple build cycles).
- **SHA-256 byte-identity confirmed**: `a92a6bd6b29e515f0f734607b13e097329b9acb26b79e21d7b139a8c903725c3` (3,239,936 bytes), VM-built vs locally-relayed copy. Saved to `.secrets/sensor-build/rdpilot-sensor.exe` for the perception-dependent waves (15-06 onward).

## Windows-DACL Live Gate (Task 3) — COMPLETE, DAEMON-02 CLOSED

### The first real Windows compile

Installed Rust (`rustup`, `stable-x86_64-pc-windows-msvc`, rustc 1.97.0) on the VM, relayed the full workspace source via Storage blob SAS, and ran `cargo test -p rdpilot-daemon --no-run` — the first time `ipc/windows.rs`'s `#[cfg(windows)]` DACL code (authored offline in Plan 15-01 against `windows-sys` 0.61.2's documented API surface, never real-compiled) hit a genuine Windows toolchain.

**Compile errors found and fixed** (all `windows-sys`/cfg-gating issues, none an architectural problem — `windows-sys` itself needed no version change or legitimacy re-checkpoint):

1. `OpenProcessToken` and `SECURITY_DESCRIPTOR_REVISION` were imported from `Win32::Security`, but in windows-sys 0.61.2 they actually live in `Win32::System::Threading` and `Win32::System::SystemServices` respectively (confirmed by grepping the real vendored crate source on the VM). Fixed imports; added the `Win32_System_SystemServices` Cargo.toml feature.
2. `let mut token: HANDLE = 0;` no longer type-checks — `HANDLE` is `*mut c_void` in this windows-sys version (was an integer type historically). Fixed to `ptr::null_mut()`.
3. `rdpilot_ipc::transport::{read_frame, write_frame}` (used unconditionally by `rdpilot-daemon/src/ipc/mod.rs`'s cross-platform `serve_connection`) failed to resolve because the entire `transport` module in `rdpilot-ipc` was `#[cfg(unix)]`-gated at the whole-module level (a leftover from before any Windows IPC consumer existed). Fixed: only the genuinely Unix-only items (`socket_dir`/`socket_path`/`connect_or_spawn`) keep `#[cfg(unix)]`, scoped per-item; `tokio` moved from a Unix-only to an unconditional dependency in `rdpilot-ipc/Cargo.toml`.
4. Three pre-existing daemon test files (`autostart_lifecycle.rs`, `ipc_security.rs`, `live_daemon_e2e.rs`) use Unix-only APIs (`UnixStream`, `std::os::unix::fs::MetadataExt`, `libc::geteuid`) with no cfg gate at all — never real-compiled on Windows before this plan. Added `#![cfg(unix)]` to each, mirroring `live_daemon_windows_dacl.rs`'s existing `#![cfg(windows)]` convention.
5. `live_daemon_windows_dacl.rs`'s own `a_same_account_connection_is_accepted_through_the_present_dacl` test tried `tokio::join!`-ing `ClientOptions::open()`, which is **synchronous** (`io::Result<NamedPipeClient>`, not a `Future`) — a genuine test-authoring bug (E0277 × several macro-expansion duplicates). Fixed to a plain sequential call followed by an awaited accept.

After fix 3, the daemon library and the four cfg-clean test files compiled and ran cleanly (BUILD_EXIT=0, all confirming test binaries built). Fix 5 closed the last compile error in `live_daemon_windows_dacl.rs` itself.

### The `runas` false-positive (live-diagnosed, not assumed)

The very first successful test run reported all 3 DACL tests PASS — but the cross-account test's PASS came from a `tokio::time::timeout` branch, which cannot on its own distinguish "genuinely rejected by the DACL" from "the second-account probe never ran at all". Rather than trust a self-reported PASS on a **critical** severity threat (T-15-12), an independent manual verification was run: starting the real `rdpilot-daemon.exe` and manually invoking the exact `runas` command the test used, with full output captured.

**Result: `runas.exe` exited immediately with code 1 and the probe never ran at all** — because `runas` internally requires an interactive window station to create its new logon session, and this entire test suite executes via `az vm run-command invoke` as `NT AUTHORITY\SYSTEM` in the non-interactive services session (Session 0), which has none. The original test's own module doc had already anticipated this risk and named the fallback ("a scheduled task trigger is the documented fallback") — this plan implemented it.

**Fix: replaced the `runas` probe with a one-shot Scheduled Task** (`schtasks /create` + `/run` as the second account), with results written to a shared file (Task Scheduler has no direct stdout relay). Getting this working end-to-end required three further live-diagnosed fixes:

- `std::env::temp_dir()` resolves to SYSTEM's own profile-scoped temp directory under this execution context — the second account has no ACL access to it at all. Switched to `C:\Windows\Temp`.
- Even `C:\Windows\Temp` was insufficient on this hardened VM image: `Configure-Target.ps1`'s in-guest hardening leaves files SYSTEM writes there with a DACL scoped to `Administrators`/`SYSTEM` only (verified live via `icacls`) — not world-writable as on a stock Windows image. Added explicit `icacls /grant` calls scoped to just the two files the probe creates (the inner script, read+execute; the result file, modify).
- The second account also needed `SeBatchLogonRight` ("Log on as a batch job") to run a Scheduled Task at all — granted via Backup Operators group membership (does not weaken the DACL test itself: pipe DACL enforcement is scoped to the specific SID, independent of group/privilege membership).
- PowerShell's `Out-File -Encoding utf8` writes a leading UTF-8 BOM (U+FEFF), which `str::trim_start()` does not strip (not whitespace) — the probe's genuine "Access Denied" result was failing a naive `starts_with("denied:")` check. Stripped the BOM explicitly before the assertion.

### Final live results — all genuine

```
running 3 tests
test a_same_account_connection_is_accepted_through_the_present_dacl ... [PASS] a same-account connection was accepted through the present, non-null, owner-scoped DACL
test a_second_first_instance_creation_of_the_same_pipe_name_fails_loudly ... [PASS] a second first-instance creation of the same pipe name failed loudly: AddrInUse ("a daemon is already listening on \\.\pipe\rdpilot-daemon (first-instance pipe creation was denied)")
test cross_account_connection_is_rejected_by_the_owner_only_dacl ... [PASS] cross-account connection rejected at the pipe boundary: denied: Exception calling "Open" with "3" argument(s): "Access to the path '\\.\pipe\rdpilot-daemon' is denied."
             [PASS] daemon-side accept_and_authorize never completed (timed out waiting, consistent with rejection)

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.21s
```

The cross-account rejection was independently confirmed to be genuine: the scheduled task's `whoami` output during manual verification confirmed it ran as `rdpilot-vm\rdpilot2` (not SYSTEM, not the primary `rdpadmin` account), and the pipe open attempt received a real `ERROR_ACCESS_DENIED` from Windows.

## Package Legitimacy

No new package/crate was added or version-changed. `windows-sys` stays pinned at 0.61.2 (per binding direction 3, already lockfile-resolved, no legitimacy checkpoint needed) — only its correct import paths were discovered live. The `windows-permissions` fallback trigger from the plan's threat model did **not** fire.

## Task Commits

Each fix committed atomically, all on `develop`:

1. **`534bec4`** — `fix(15-05): correct windows-sys 0.61.2 API paths in DACL code` (OpenProcessToken/SECURITY_DESCRIPTOR_REVISION module paths, HANDLE null-pointer fix)
2. **`96728b3`** — `fix(15-05): make rdpilot-ipc transport framing cross-platform` (cfg(unix) gate scoped per-item instead of whole-module; tokio dependency unconditional)
3. **`4447224`** — `fix(15-05): gate Unix-only daemon integration tests behind cfg(unix)` (autostart_lifecycle.rs, ipc_security.rs, live_daemon_e2e.rs)
4. **`0dcf28c`** — `fix(15-05): fix live-VM-diagnosed bugs in the Windows-DACL live gate` (tokio::join! misuse, runas→Scheduled-Task replacement, icacls grants, BOM strip)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] windows-sys 0.61.2 API-path mismatches (OpenProcessToken, SECURITY_DESCRIPTOR_REVISION, HANDLE type)**
- **Found during:** Task 3, first real Windows compile of `ipc/windows.rs`
- **Fix:** Corrected import module paths; added `Win32_System_SystemServices` Cargo.toml feature; changed `HANDLE = 0` to `HANDLE = ptr::null_mut()`
- **Files modified:** `crates/rdpilot-daemon/src/ipc/windows.rs`, `crates/rdpilot-daemon/Cargo.toml`
- **Verification:** Compiles cleanly on the real Windows VM; offline Linux build unaffected (file is `#[cfg(windows)]`-gated)
- **Committed in:** `534bec4`

**2. [Rule 1 - Bug] rdpilot-ipc's transport module wrongly whole-module `#[cfg(unix)]`-gated**
- **Found during:** Task 3, second compile error round
- **Issue:** `rdpilot-daemon/src/ipc/mod.rs`'s cross-platform `serve_connection` unconditionally imports `read_frame`/`write_frame`, but the entire `rdpilot_ipc::transport` module (including these platform-agnostic framing functions) was gated `#[cfg(unix)]`
- **Fix:** Split the gate per-item: framing (`TransportError`/`read_frame`/`write_frame`) unconditional; socket-path/connect-or-spawn (`socket_dir`/`socket_path`/`connect_or_spawn`/`try_connect`) stay `#[cfg(unix)]`. Moved `tokio` from a Unix-only to unconditional dependency
- **Files modified:** `crates/rdpilot-ipc/src/lib.rs`, `crates/rdpilot-ipc/src/transport.rs`, `crates/rdpilot-ipc/Cargo.toml`
- **Verification:** Full offline workspace test suite (all crates) passes; VM compile confirms the Windows path now resolves
- **Committed in:** `96728b3`

**3. [Rule 1 - Bug] Three daemon test files missing `#![cfg(unix)]`**
- **Found during:** Task 3, third compile error round
- **Issue:** `autostart_lifecycle.rs`, `ipc_security.rs`, `live_daemon_e2e.rs` use `UnixStream`/`MetadataExt`/`libc::geteuid` unconditionally — never real-compiled on Windows before this plan
- **Fix:** Added `#![cfg(unix)]` to each file
- **Files modified:** the three test files listed above
- **Verification:** Offline Linux suite unaffected (still compiles/runs identically); VM compile confirms these are now correctly stripped on Windows
- **Committed in:** `4447224`

**4. [Rule 1 - Bug] `tokio::join!` misuse in the same-account DACL control-case test**
- **Found during:** Task 3, fourth compile error round
- **Issue:** `ClientOptions::open()` is synchronous, not a `Future` — cannot be `tokio::join!`-ed
- **Fix:** Sequential synchronous call followed by an awaited accept
- **Files modified:** `crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs`
- **Verification:** Compiles and the control-case test passes live
- **Committed in:** `0dcf28c`

**5. [Rule 1 - Bug, live-diagnosed] `runas`-based cross-account probe never actually ran**
- **Found during:** Task 3, independent manual verification after the first "PASS" run (not trusted at face value given the critical severity of T-15-12)
- **Issue:** `runas` requires an interactive window station; this test suite's execution context (`az vm run-command` → `NT AUTHORITY\SYSTEM`, Session 0) has none, so `runas` failed immediately (exit 1) without ever attempting the probe — the daemon-side timeout alone could not distinguish this from a genuine rejection
- **Fix:** Replaced with a one-shot Scheduled Task (the mechanism the file's own original doc had already named as the fallback), with three supporting fixes (world-writable-assumption temp-dir bug, hardened-image ACL gap via explicit `icacls` grants, UTF-8 BOM stripping) discovered iteratively via further live diagnosis
- **Files modified:** `crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs`
- **Verification:** Manually verified via a standalone `whoami` probe that the Scheduled Task genuinely runs as the second account BEFORE trusting the Rust test's rewritten result; then re-verified the actual `cargo test` run reports the same genuine `ERROR_ACCESS_DENIED`
- **Committed in:** `0dcf28c`

**6. [Rule 2 - Missing critical functionality] `RDPILOT_SECOND_WINDOWS_PASSWORD` env var added**
- **Found during:** Task 3, implementing the Scheduled-Task probe
- **Issue:** `schtasks /create` requires the target account's password (`/rp`); the original test only consumed `RDPILOT_SECOND_WINDOWS_ACCOUNT`
- **Fix:** Added a second env var, `RDPILOT_SECOND_WINDOWS_PASSWORD`, consumed alongside the existing account-name var
- **Files modified:** `crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs`
- **Committed in:** `0dcf28c`

**Total deviations:** 6 (all Rule 1/2, all directly required for the live gate to compile/run/pass at all — no scope creep). The `runas`→Scheduled-Task replacement (deviation 5) is the single most consequential finding: without independently verifying the first "PASS", a genuine gap in the cross-account rejection proof (the critical T-15-12 threat) would have shipped undetected.

## Issues Encountered

- **`az vm run-command invoke`'s internal ~80-90 minute execution timeout** repeatedly force-killed early build attempts (the first cold `cargo build` genuinely took that long, compiling the full `ironrdp`+`tokio-full`+`image` dependency tree on a 2-vCPU VM with Windows Defender real-time scanning enabled and no exclusions). Two mitigations applied: (1) Defender exclusions added for `C:\rdpilot-src`/`.cargo`/`.rustup` and the `cargo.exe`/`rustc.exe`/`link.exe` processes (throwaway VM, safe); (2) switched to a detached-background-process pattern (`Start-Process -WindowStyle Hidden` + log file + sentinel done-file + cheap polling) for every subsequent build/test invocation, decoupling the actual compile/run duration from the `az vm run-command` request/response cycle. Once both were in place, incremental rebuilds completed in 5-7 minutes.
- Orphaned `cargo`/`rustc` processes from earlier interrupted attempts were found still running on the VM (confirmed via `Get-Process`/`Get-CimInstance Win32_Process` diagnostics) and had to be explicitly killed before a clean rebuild could proceed — the local `az` CLI process dying does not kill the remote VM-side process.

## User Setup Required

None beyond the plan's own stated Azure `az` CLI authentication prerequisite (already satisfied).

## Next Phase Readiness

- **DAEMON-02 requirement is fully Complete** (both Unix and Windows halves live-verified).
- Plan 15-06 (orphan-liveness + e2e + CLI-02/03 live + MCP-04 live, from the Linux host against the same VM) can proceed immediately — the VM is UP, the sensor is byte-verified and saved locally, and `.secrets/connection.json` is fresh.
- No blockers. No windows-permissions legitimacy checkpoint fired (windows-sys held, as the plan hoped).
- **Reminder for 15-06/07/08:** the VM stays UP until 15-08's authorized teardown checkpoint. If any of those plans need another Windows-side build/test cycle, reuse the detached-background-process + Scheduled-Task-probe patterns documented here rather than rediscovering them.

---
*Phase: 15-proof-harnesses-live-llm-capstone*
*Completed: 2026-07-11*

## Self-Check: PASSED

All claimed files exist on disk (`crates/rdpilot-daemon/src/ipc/windows.rs`, `crates/rdpilot-daemon/Cargo.toml`, `crates/rdpilot-ipc/src/lib.rs`, `crates/rdpilot-ipc/src/transport.rs`, `crates/rdpilot-ipc/Cargo.toml`, `crates/rdpilot-daemon/tests/autostart_lifecycle.rs`, `crates/rdpilot-daemon/tests/ipc_security.rs`, `crates/rdpilot-daemon/tests/live_daemon_e2e.rs`, `crates/rdpilot-daemon/tests/live_daemon_windows_dacl.rs`, `.secrets/connection.json`, `.secrets/sensor-build/rdpilot-sensor.exe`, this SUMMARY.md), and all four task commit hashes (`534bec4`, `96728b3`, `4447224`, `0dcf28c`) are present in `git log --oneline --all`.
