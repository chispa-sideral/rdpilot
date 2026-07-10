---
phase: 07-uia-tree-module
plan: 03
subsystem: sensor
tags: [csharp, dotnet8, nativeaot, com-interop, generatedcominterface, uiautomation, live-gate, azure, bstr]

# Dependency graph
requires:
  - phase: 07-uia-tree-module (07-02)
    provides: "sensor/UiaInterop.cs's four hand-authored [GeneratedComInterface] UIA interfaces + --smoke-test-uia AOT risk-gate entry point (offline-clean, behaviorally unvalidated)"
provides:
  - "LIVE-VERIFIED proof that IUIAutomation + [GeneratedComInterface] + NativeAOT marshals correctly on real Windows, resolving D-7.5's spike-first risk gate"
  - "UiaInterop.ReadName() -- leak-safe manual BSTR decode (Marshal.PtrToStringBSTR/FreeBSTR), the live-diagnosed fix for a STATUS_HEAP_CORRUPTION crash in CurrentName's implicit StringMarshalling.Utf16 path"
  - "Per-step Console tracing (RunUiaAotSmokeTest's Step() helper, explicit Flush()) as a durable diagnostic aid for any future native-crash risk gate"
  - "Empirical resolution of RESEARCH Assumptions A1 (SAFEARRAY, PASS unmodified), A2 (BSTR, FAILED as researched -> fixed), A3 (BOOL, PASS unmodified), and A4 (vtable/GUID correctness, PASS unmodified)"
affects: [07-04-uia-handler, 07-05-live-gate]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "A COM BSTR return from a [GeneratedComInterface]-attributed member must NEVER be declared as a plain C# `string` under StringMarshalling.Utf16 -- the implicit marshaller frees it with CoTaskMemFree instead of the allocator-matched SysFreeString, corrupting the OLE Automation heap. Declare [PreserveSig] + out nint raw pointer, decode with Marshal.PtrToStringBSTR, free with Marshal.FreeBSTR (both exist on .NET 8's Marshal, unlike the removed SafeArrayGet* family)."
    - "A native crash that bypasses the managed try/catch (e.g. STATUS_HEAP_CORRUPTION in ntdll.dll) also bypasses Console.Out's default buffered-when-redirected StreamWriter unless each diagnostic line is explicitly Flush()'d immediately after WriteLine -- add a Step()-style tracer with inline Flush() around every risky native call in any future COM/AOT smoke test, not just a single try/catch wrapper."
    - "Running a Windows exe launched from az vm run-command lands in Session 0 (no desktop) and cannot exercise UIA at all -- the sensor's OWN LaunchProcess handler (already running in the interactive session via the proven Win+R/RDPDR deploy_and_launch path) must be used to spawn the smoke-test binary as a child process, with cmd.exe /c ... > file 2>&1 redirecting output to a fixed absolute path that a follow-up Session-0 run-command CAN read back (file reads, unlike window/COM enumeration, are not session-restricted)."
    - "A fresh Windows VM's FIRST RDP login shows a modal 'network discoverable' Settings flyout that captures keyboard focus and silently swallows Win+R -- deploy_and_launch's Win+R injection will appear to do nothing (no error) until this one-time dialog is dismissed via a mouse click."

key-files:
  created: []
  modified:
    - sensor/UiaInterop.cs
    - sensor/Program.cs

key-decisions:
  - "GetCurrentName's interface-level StringMarshalling.Utf16 attribute was removed entirely from IUIAutomationElement (it was BSTR's only consumer) and replaced with the same [PreserveSig] + raw-pointer + manual-decode shape already proven by GetRuntimeId's SAFEARRAY handling -- keeping exactly one marshalling discipline (manual, explicit, leak-safe-via-finally) for every COM type this phase found risky, rather than mixing implicit-marshaller and manual-decode styles."
  - "The live-gate driver (RDP connect -> deploy_and_launch -> LaunchProcess(cmd /c smoke-test > file) -> az vm run-command Get-Content) was implemented as a throwaway crates/rdpilot/examples/*.rs file, run via cargo, then deleted -- NOT committed, since the plan's files_modified scope is sensor/UiaInterop.cs + sensor/Program.cs only. The mechanism itself (documented here) is reusable for 07-05's live gate if a similar in-session capture need arises."

requirements-completed: [PERC-03]

# Metrics
duration: ~1h50min (VM up through VM down, including two live-diagnosed rebuild/redeploy cycles)
completed: 2026-07-09
---

# Phase 7 Plan 03: UIA COM-Interop Live Risk Gate Summary

**THE D-7.5 risk gate PASSED on a real Windows VM: `[smoke-test-uia] PASS: root name='Desktop 1' enabled=True rect=0,0,1920,1080 runtimeId.len=2 children=3`, after live-diagnosing and fixing a `STATUS_HEAP_CORRUPTION` crash in `IUIAutomationElement.CurrentName`'s BSTR marshalling (RESEARCH Assumption A2) — SAFEARRAY (A1), BOOL (A3), RECT, and `FindAll` all passed on the first live attempt with zero changes required.**

## Performance

- **Duration:** ~1h50min (VM `up` through VM `down`)
- **Tasks:** 2/2 completed. Task 1 (AOT-publish + iterate until PASS) executed and committed in-session; Task 2 (`checkpoint:human-verify`, `gate="blocking"`) was reached, reported without self-approval, and subsequently **APPROVED by the coordinator** ("risk gate is accepted... D-7.5 is now empirically retired — 07-04 ... is cleared to proceed").
- **Files modified:** 2 (`sensor/UiaInterop.cs`, `sensor/Program.cs`)

## Feasibility Pre-Check

`az`/`pwsh`/`dotnet` present, `az account show` authenticated to subscription "Chispa Sideral", `rdpilot-test` RG clear, `rdpilot-mgmt` RG present as expected. Pre-check PASSED — proceeded to provision.

## Live-Gate Narrative

1. **Provisioned** the disposable VM: `infra/manage-env.ps1 up -VmSize Standard_B2s_v2`.
2. **Installed** .NET 8 SDK 8.0.422 (to `C:\dotnet8`, fresh VM had none) and VC++ Build Tools (`Microsoft.VisualStudio.Workload.VCTools`, required for the NativeAOT linker) on the VM via `az vm run-command invoke` — same WinRM-unavailable-from-this-host substitution Phase 6 established.
3. **Built** `dotnet publish -c Release -r win-x64 -p:PublishAot=true --self-contained` ON the VM via `az vm run-command invoke` (AOT cannot cross-compile from this Linux host) — clean, zero ILC/trim/source-gen warnings, both before and after the fix. Sensor source embedded as a base64 tarball in the run-command script body (~31 KB); the built exe relayed back via a short-lived Azure Storage blob (account-key SAS, `rwcl`, 3h expiry, in a throwaway `relay` container inside the existing `rdpilotcsevld85fbx` storage account) — the first upload attempt with a **user-delegation** SAS failed `AuthorizationPermissionMismatch` (the delegating identity lacked a data-plane RBAC role); switching to an **account-key** SAS resolved it immediately.
4. **Discovered UIA requires the interactive session**, which `az vm run-command` (Session 0) cannot provide. Drove an actual RDP connection from this Linux host via the Rust SDK's own public API (`Session::connect` → `deploy_and_launch()` to get the base sensor running in-session → `Session::launch_process("cmd.exe", Some("/c \"%TEMP%\\rdpilot-sensor.exe\" --smoke-test-uia > C:\\Users\\Public\\smoke-test-uia-out.txt 2>&1"), None)` to spawn the smoke-test binary AS A CHILD of the already-interactive-session sensor process, redirecting output to a fixed path a follow-up Session-0 `run-command` could read back). This driver was a throwaway `crates/rdpilot/examples/*.rs` file (built/run via `cargo run -p rdpilot --example ... --target x86_64-unknown-linux-gnu`, `RUSTUP_TOOLCHAIN=stable` overriding the repo's ARM64-Windows-pinned `rust-toolchain.toml`) — deleted after use, never committed (plan scope is `sensor/*.cs` only).
5. **First `deploy_and_launch()` attempt failed** ("sensor did not respond after 3 launch attempts"). Screenshot diagnosis (`session.screenshot()`, retry-polled) revealed the FIRST-EVER RDP login to this fresh VM landed on a modal "Do you want to allow your PC to be discoverable" Settings flyout that silently captured keyboard focus, swallowing every injected Win+R. A single `send_mouse(MouseAction::Click { .. })` on "No" cleared it permanently; every subsequent `deploy_and_launch()` succeeded normally (~20ms ping).
6. **First `--smoke-test-uia` run produced a 0-byte output file** — no PASS, no FAIL, no exception text. `Get-WinEvent -LogName Application` (via `run-command`, Session 0 — file/event-log reads are NOT session-restricted, unlike window/COM enumeration) surfaced `Application Error` Event ID 1000: `Exception code: 0xc0000374` (`STATUS_HEAP_CORRUPTION`), faulting in `ntdll.dll`, faulting process `rdpilot-sensor.exe`. Added per-step `Console.WriteLine` + explicit `Console.Out.Flush()` tracing (`Step()` helper) around every risky call in `RunUiaAotSmokeTest` so a native crash — which bypasses the managed `try/catch` entirely and would otherwise lose all buffered/unflushed output — still shows every step completed before it. Rebuild + rerun pinpointed the crash to exactly `GetCurrentName()`.
7. **Root-caused and fixed** (see Deviations below): `StringMarshalling.Utf16`'s implicit BSTR marshaller frees the returned string with `CoTaskMemFree` instead of the allocator-matched `SysFreeString`, corrupting the OLE Automation heap. Applied RESEARCH's own documented fallback.
8. **Rebuilt, redeployed, reran** — full `[smoke-test-uia] PASS` captured (see below). Cleaned up the temporary `relay` blob container, tore the VM down, confirmed `az group exists -n rdpilot-test` returns `false` (`rdpilot-mgmt` persists as designed).

## The PASS Line

```
[smoke-test-uia] PASS: root name='Desktop 1' enabled=True rect=0,0,1920,1080 runtimeId.len=2 children=3
```

Per-step trace leading to it (post-fix run):

```
[smoke-test-uia] step: about to GetRootAutomation()
[smoke-test-uia] step: GetRootAutomation() OK
[smoke-test-uia] step: GetDesktopWindow() OK hwnd=0x10010
[smoke-test-uia] step: ElementFromHandle() OK
[smoke-test-uia] step: about to ReadRuntimeId()
[smoke-test-uia] step: ReadRuntimeId() OK len=2
[smoke-test-uia] step: about to ReadName()
[smoke-test-uia] step: ReadName() OK name='Desktop 1'
[smoke-test-uia] step: about to GetCurrentIsEnabled()
[smoke-test-uia] step: GetCurrentIsEnabled() OK enabled=True
[smoke-test-uia] step: about to GetCurrentBoundingRectangle()
[smoke-test-uia] step: GetCurrentBoundingRectangle() OK rect=0,0,1920,1080
[smoke-test-uia] step: about to CreateTrueCondition()
[smoke-test-uia] step: CreateTrueCondition() OK
[smoke-test-uia] step: about to FindAll()
[smoke-test-uia] step: FindAll() OK
[smoke-test-uia] step: about to GetLength()
[smoke-test-uia] step: GetLength() OK count=3
```

## RESEARCH Assumptions — Resolution

| Assumption | Path | Result | Fix Required |
|---|---|---|---|
| A1 | `GetRuntimeId` SAFEARRAY(int) decode (`[PreserveSig]` + manual `Marshal.SafeArrayGet*`) | **PASS** | None — first-attempt code (from 07-02) worked unmodified |
| A2 | `CurrentName` BSTR (`StringMarshalling.Utf16` + plain `string` return) | **FAILED** (`STATUS_HEAP_CORRUPTION`) | Fixed: `[PreserveSig]` + raw `out nint` + `Marshal.PtrToStringBSTR`/`FreeBSTR` |
| A3 | `CurrentIsEnabled`/`CurrentIsKeyboardFocusable`/`CurrentHasKeyboardFocus`/`CurrentIsOffscreen` BOOL (`[MarshalAs(UnmanagedType.Bool)]`) | **PASS** | None — only `CurrentIsEnabled` was exercised by the smoke test; the other three BOOL-returning slots share the identical marshalling declaration so this result generalizes to them (07-04 will exercise them directly) |
| A4 | vtable slot order / GUID correctness (`CoCreateInstance`, `ElementFromHandle`, `FindAll`) | **PASS** | None — no `E_NOINTERFACE`/`COMException`, every call landed on the correct native method |
| — | `CurrentBoundingRectangle` RECT-by-out-pointer (Open Question #2) | **PASS** | None — `Rect32` reused verbatim as RESEARCH recommended |

## win-x64 NativeAOT Publish Result

Built ON the Azure VM via `az vm run-command invoke` (WinRM unavailable from this Linux host, same Phase 6 substitution). **Zero ILC/trimming/source-generator warnings** on both the pre-fix and post-fix publish. Final (post-fix) exe: 2,973,696 bytes, SHA256 `e57554b382d6fbdb418dc6ef78691123d96cfd9f42b9e8789ef356ef0dda896b` — verified byte-identical between the VM build and the locally-relayed copy used to drive the live-session launch.

## Task Commits

Task 1 (AOT-publish + iterate until PASS) was completed and committed:

1. **Live-diagnosed BSTR marshalling fix** — `44c2ac5` (fix) — `sensor/UiaInterop.cs`, `sensor/Program.cs`

Task 2 (`checkpoint:human-verify`, `gate="blocking"`) — VM teardown (one of the checkpoint's `how-to-verify` steps) was performed as directed automation ahead of the human review; the PASS line, fix scope, and teardown were then reported to the coordinator without self-approval. The coordinator subsequently reviewed and returned the resume-signal **"approved"**, confirming: the smoke-test PASS on the real win-x64 AOT binary, the A2 BSTR heap-corruption root-cause/fix (in-scope, commit `44c2ac5`), the clean AOT publish, and the VM teardown. D-7.5 is now empirically retired; 07-04 is cleared to proceed.

## Files Created/Modified

- `sensor/UiaInterop.cs` — `IUIAutomationElement.GetCurrentName` changed from a plain `string` property (implicit `StringMarshalling.Utf16` BSTR marshalling) to `[PreserveSig] int GetCurrentName(out nint name)`; new `UiaInterop.ReadName()` leak-safe manual BSTR decode (`Marshal.PtrToStringBSTR`/`Marshal.FreeBSTR`, `finally`-guarded); interface-level `StringMarshalling.Utf16` attribute removed (was BSTR's only consumer).
- `sensor/Program.cs` — `RunUiaAotSmokeTest` gained a `Step()` diagnostic tracer (`Console.WriteLine` + explicit `Flush()`) around every risky COM call; the `GetCurrentName()` call site now calls `UiaInterop.ReadName(root)`.

## Decisions Made

See frontmatter `key-decisions`. Summary: kept exactly one marshalling discipline (manual/explicit/leak-safe) for every risky COM type in this phase rather than mixing implicit-marshaller and manual-decode styles; the RDP-driven live-gate harness was throwaway/uncommitted tooling, not new SDK surface.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `StringMarshalling.Utf16` BSTR marshalling crashes the win-x64 AOT binary with `STATUS_HEAP_CORRUPTION`**
- **Found during:** Task 1, first live `--smoke-test-uia` run on the VM (0-byte output file, no exception text — root-caused via `Get-WinEvent` Application-log `Event ID 1000` + added per-step tracing to pinpoint the exact call)
- **Issue:** `IUIAutomationElement.GetCurrentName()` declared as a plain C# `string` return under the interface-level `[GeneratedComInterface(StringMarshalling = StringMarshalling.Utf16)]` attribute (07-02's first-attempt strategy per RESEARCH Assumption A2) crashed the process outright — `Exception code: 0xc0000374` (`STATUS_HEAP_CORRUPTION`), faulting in `ntdll.dll`, the instant the call executed. Root cause: a COM `BSTR` is allocated by the OLE Automation allocator (`SysAllocString`) and must be freed with `SysFreeString`; the source generator's implicit `StringMarshalling.Utf16` BSTR handling instead frees the returned pointer with `CoTaskMemFree`, corrupting the OLE Automation heap the moment the mismatched allocator/deallocator pair collide. This is a native-level crash that bypasses the managed `try/catch` in `RunUiaAotSmokeTest` entirely — it does not surface as a catchable `COMException`.
- **Fix:** Declared the slot as `[PreserveSig] int GetCurrentName(out nint name)` (raw HRESULT + raw BSTR pointer, identical shape to `GetRuntimeId`'s already-proven SAFEARRAY fallback), added `UiaInterop.ReadName()` to decode via `Marshal.PtrToStringBSTR` and free via `Marshal.FreeBSTR` in a `finally` block (leak-safe, same discipline as `ReadRuntimeId`/T-07-03) — both of these `Marshal` members exist on .NET 8 (unlike the removed `SafeArrayGet*` family), so no hand-rolled P/Invoke was needed. Removed the now-unused interface-level `StringMarshalling.Utf16` attribute.
- **Files modified:** `sensor/UiaInterop.cs`, `sensor/Program.cs` (smoke-test call site updated to `UiaInterop.ReadName(root)`).
- **Verification:** Rebuilt win-x64 AOT on the VM (zero ILC/trim/source-gen warnings), redeployed, reran `--smoke-test-uia` — full `[smoke-test-uia] PASS` captured, including a correctly-decoded `name='Desktop 1'`.
- **Committed in:** `44c2ac5`.

**2. [Rule 3 - Blocking issue] `deploy_and_launch()` failed on the VM's first-ever RDP login (network-discoverable dialog swallowed Win+R)**
- **Found during:** first live-gate attempt — `deploy_and_launch()` exhausted all 3 launch attempts/60 ping polls with no pong.
- **Issue:** A fresh Windows VM's very first RDP login surfaces a modal Settings flyout ("Do you want to allow your PC to be discoverable by other PCs and devices on this network?") that captures keyboard focus and silently swallows the injected Win+R sequence — no error, no visible symptom other than the Run dialog never appearing (confirmed via a retry-polled `session.screenshot()` diagnostic that is NOT part of the committed plan scope, see Decisions).
- **Fix:** A single `session.send_mouse(MouseAction::Click { .. })` on the dialog's "No" button (the SDK's already-public `Session::send_mouse`/`MouseAction` API — no new code) cleared it permanently; every subsequent `deploy_and_launch()` on the same VM succeeded normally. This is a one-time first-login condition, not a recurring one — not added as permanent SDK/handler code, since it is orthogonal to this phase's UIA-interop scope and Phase 1-6 VMs never hit it in prior live gates (their first-login dialog dismissal, if any, was not previously diagnosed/documented).
- **Files modified:** None (diagnosis-only, using existing public `Session` API from an uncommitted throwaway driver).
- **Verification:** `deploy_and_launch()` succeeded (~20ms ping) on every subsequent attempt in this session.
- **Committed in:** N/A (no code change — operational finding, documented here for future live-gate sessions against fresh VM images).

---

**Total deviations:** 2 (1 auto-fixed code bug — Rule 1; 1 auto-resolved operational blocker — Rule 3). Both are genuine, empirically-discovered facts the offline 07-02 gate could not have caught (RESEARCH explicitly flagged BSTR/A2 as LOW-MEDIUM confidence pending exactly this live gate; the first-login dialog is a VM-provisioning-time condition, not a code defect).
**Impact on plan:** No scope change. The BSTR fix is confined to `sensor/UiaInterop.cs`/`sensor/Program.cs` exactly as the plan's `<action>` requires ("Keep every edit confined to UiaInterop.cs / Program.cs"). SAFEARRAY (A1), BOOL (A3), RECT, and vtable/GUID correctness (A4) all required zero changes — RESEARCH's HIGH-confidence assessment of those paths held.

## Issues Encountered

- The first blob-upload attempt used a **user-delegation SAS** (`az storage container generate-sas --auth-mode login --as-user`), which failed `AuthorizationPermissionMismatch` — the delegating Azure AD identity lacked a data-plane RBAC role (e.g. Storage Blob Data Contributor) on the storage account, even though it is a subscription Owner (Azure Storage data-plane permissions are separate from control-plane RBAC). Switched to an **account-key SAS** (`--account-key`, using `az storage account keys list`), which succeeded immediately — this is the durable pattern for any future blob-relay use against this same storage account until/unless a data-plane role is explicitly assigned.
- `rust-toolchain.toml`/`.cargo/config.toml` pin `stable-x86_64-pc-windows-gnu` with a Windows-only linker path (written for the project owner's separate ARM64 Windows dev machine) — invalid on this Linux execution host. Worked around per-invocation with `RUSTUP_TOOLCHAIN=stable ... --target x86_64-unknown-linux-gnu` (no repo files modified) to compile/run the throwaway live-gate driver natively; this is environment-specific to this execution session, not a project configuration bug, and is not something this plan's scope authorizes changing.

## User Setup Required

None beyond the pre-authorized live-gate execution itself. The user's Azure session (`az account show` — subscription "Chispa Sideral") was already active.

## Next Phase Readiness

- **D-7.5's risk gate is RETIRED.** The novel `IUIAutomation` + `[GeneratedComInterface]` + NativeAOT combination is now empirically proven working on real Windows, with the one marshalling assumption that failed (A2/BSTR) fixed and re-verified live. 07-04 (the real `UiaTree` handler) can now be built directly on top of `sensor/UiaInterop.cs` with confidence in all four hand-authored interfaces' vtable/GUID correctness and every exercised marshalling path.
- 07-04 should reuse `UiaInterop.ReadName()` for `CurrentName` (do NOT reintroduce a plain `string GetCurrentName()` property) and `UiaInterop.ReadRuntimeId()` for `id` (D-7.2) exactly as this gate proved them.
- 07-04 should be aware that `CurrentHasKeyboardFocus`/`CurrentIsKeyboardFocusable`/`CurrentIsOffscreen` (the three BOOL-returning slots NOT directly exercised by this smoke test, only `CurrentIsEnabled` was) share the identical `[MarshalAs(UnmanagedType.Bool)]` declaration pattern that passed here — expected to work, but 07-04's own handler exercise is the first DIRECT live confirmation for those three specific members.
- No known stubs introduced by this plan (it is a pure risk-gate/fix plan, no new handler surface).
- **Task 2 (the plan's `checkpoint:human-verify`, `gate="blocking"`) is APPROVED** — the coordinator confirmed the PASS, the fix scope, and the VM teardown. Plan 07-03 is COMPLETE; 07-04 (the real `UiaTree` handler) is cleared to proceed.

## Threat Flags

None beyond what 07-03-PLAN's `<threat_model>` already covers. T-07-02 (live vtable dispatch tampering) is directly addressed by this gate's own outcome — the one bad marshalling declaration surfaced HERE, on a throwaway binary against the desktop root, exactly as the threat register's mitigation plan described. T-07-03 (SAFEARRAY DoS via leak) — `ReadRuntimeId`'s `finally`-guarded `SafeArrayDestroy` was exercised on the live path with no leak observed; `ReadName`'s new `finally`-guarded `Marshal.FreeBSTR` follows the identical discipline. T-07-04 (cost/credential leak from a left-up VM) — mitigated: VM torn down, `rdpilot-test` RG confirmed absent before this SUMMARY was written.

## Self-Check: PASSED

- FOUND: commit `44c2ac5` in `git log --oneline --all`
- FOUND: `sensor/UiaInterop.cs` contains `ReadName` / `PtrToStringBSTR` / `FreeBSTR`
- FOUND: `sensor/Program.cs` contains `Step(` tracer calls and `UiaInterop.ReadName(root)` call site
- CONFIRMED: `az group exists -n rdpilot-test` → `false`
- CONFIRMED: `az group exists -n rdpilot-mgmt` → `true`
- CONFIRMED: no throwaway `crates/rdpilot/examples/*.rs` driver files remain (`git status --short` shows only the two scoped `sensor/*.cs` files as tracked changes, now committed)

---
*Phase: 07-uia-tree-module*
*Completed: 2026-07-09*
