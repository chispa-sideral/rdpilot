---
phase: 01-test-environment
plan: 03
subsystem: infra
tags: [powershell, in-guest-config, custom-script-extension, winrm, dpi, registry, 7zip, supply-chain, wave-2, env-01]
one_liner: "infra/scripts/Configure-Target.ps1 — the idempotent in-guest hardening the CSE invokes: WinRM HTTPS listener + firewall, 96 DPI + SuppressWhenMinimized=2 to the DEFAULT user hive (not the current-user hive), machine-wide SuppressWhenMinimized, and a SHA-256-gated 7-Zip 26.01 silent install using a human-approved pinned hash."
requires:
  - "infra/main.bicep CSE contract (Plan 02): no-arg invocation of Configure-Target.ps1 via scriptUri"
provides:
  - "infra/scripts/Configure-Target.ps1 — idempotent, no-reboot in-guest hardening (the in-guest half of ENV-01)"
  - "Pinned + human-verified 7-Zip 26.01 install (URL + SHA-256 hard-coded constants)"
  - "WinRM HTTPS (5986) self-signed listener + inbound firewall rule"
  - "Default-user-hive 96 DPI (LogPixels=96, Win8DpiScaling=1) + RemoteDesktop_SuppressWhenMinimized=2"
  - "Machine-wide RemoteDesktop_SuppressWhenMinimized=2 at HKLM + Wow6432Node"
affects:
  - "Plan 04 (manage-env.ps1) publishes this script + supplies scriptUri; its phase-gate Validate-Target.ps1 asserts all of these on the live box over WinRM"
  - "Phase 2 consumes RemoteDesktop_SuppressWhenMinimized + 96 DPI as RDP-session prerequisites"
tech_stack:
  added:
    - "Windows PowerShell 5.1-compatible in-guest script (#Requires -Version 5.1; runs under the CSE's powershell.exe)"
  patterns:
    - "Idempotency mandatory: every mutation guarded (listener presence check, reg add /f, firewall -ErrorAction SilentlyContinue, 7zFM.exe presence guard) so a CSE re-run is a no-op"
    - "Per-user registry → DEFAULT user hive via reg load/unload, never the current-user hive (Pitfall 1)"
    - "[gc]::Collect() + WaitForPendingFinalizers() in a finally block before reg unload (handles must be released or unload fails)"
    - "Pinned + SHA-256-verified binary download; hash mismatch throws BEFORE Start-Process; never 'latest'"
    - "No reboot anywhere in a CSE-invoked script"
    - "Forbidden-token gates match literals anywhere in the file — keep rationale prose token-free (carried from Plan 02)"
key_files:
  created:
    - "infra/scripts/Configure-Target.ps1"
  modified: []
decisions:
  - "7-Zip pinned to 26.01 (2026-04-27), URL https://github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe, SHA-256 d64a0468...94377d — approved via the blocking human-verify checkpoint (Task 1)"
  - "Pinned the official ip7z GitHub release URL (user's explicit choice over the 7-zip.org/a/ mirror, which serves a byte-for-byte identical binary)"
  - "Wrote both LogPixels=96 AND Win8DpiScaling=1 to be unambiguous about 100% scaling (RESEARCH.md Open Q1 / A4) — effective DPI verified later by Plan 04's phase gate"
  - "Did NOT mark ENV-01 complete: it spans Plans 02+03+04 and is only provable by Plan 04's live up → Validate-Target.ps1 phase gate (kept Pending)"
metrics:
  duration: "~2 min (Task 2 only; Task 1 checkpoint approval was out-of-band)"
  completed: "2026-06-04"
  tasks: 2
  files: 1
  commits: 1
---

# Phase 1 Plan 03: In-Guest Hardening Script (Configure-Target.ps1) Summary

`infra/scripts/Configure-Target.ps1` is the single idempotent script the Plan 02
`CustomScriptExtension` invokes (no-arg: `powershell -ExecutionPolicy Unrestricted
-File Configure-Target.ps1`). It performs all ENV-01 **in-guest** hardening in one
no-reboot pass: a WinRM HTTPS listener + firewall rule, 96 DPI and
`RemoteDesktop_SuppressWhenMinimized=2` written to the **default user hive** (so the
not-yet-existent automation user inherits them — Pitfall 1), machine-wide
SuppressWhenMinimized at HKLM + Wow6432Node, and a 7-Zip 26.01 silent install gated
by a **human-approved, pinned SHA-256**. This plan owns the in-guest half of ENV-01;
the cloud graph is Plan 02, and Plan 04 publishes the script, supplies `scriptUri`,
and runs the live phase-gate that proves all of this on a real box over WinRM.

## What Was Built

- **`infra/scripts/Configure-Target.ps1`** (`#Requires -Version 5.1`,
  `$ErrorActionPreference = 'Stop'`, header documenting the approved 7-Zip pin), four
  idempotent sections in order:
  1. **WinRM HTTPS + firewall** — guarded: if no `Transport=HTTPS` listener exists,
     `New-SelfSignedCertificate -DnsName $env:COMPUTERNAME` →
     `New-Item WSMan:\localhost\Listener -Transport HTTPS -Address * -CertificateThumbPrint <thumb> -Force`;
     **always** ensures `New-NetFirewallRule -DisplayName 'WinRM HTTPS' ... -LocalPort 5986 -Protocol TCP -Action Allow -ErrorAction SilentlyContinue`.
     Both listener and firewall are required (Pitfall 3).
  2. **Default-user-hive per-user settings** — `reg load HKLM\DEFAULT_USER C:\Users\Default\NTUSER.DAT`,
     then `reg add` `LogPixels=96`, `Win8DpiScaling=1` under `Control Panel\Desktop`
     and `RemoteDesktop_SuppressWhenMinimized=2` under `Software\Microsoft\Terminal Server Client`.
     In a `finally` block: `[gc]::Collect(); [gc]::WaitForPendingFinalizers(); reg unload HKLM\DEFAULT_USER`
     — handles released before unload or it fails. Never the current-user hive (Pitfall 1).
  3. **Machine-wide SuppressWhenMinimized** — `reg add /d 2` at
     `HKLM\Software\Microsoft\Terminal Server Client` and the `Wow6432Node` path.
  4. **7-Zip 26.01 silent install** — guarded by `if (-not (Test-Path 'C:\Program Files\7-Zip\7zFM.exe'))`.
     Inside: force TLS 1.2, `Invoke-WebRequest` the pinned URL to `$env:TEMP`,
     `Get-FileHash -Algorithm SHA256`, and `throw` on mismatch **before**
     `Start-Process -FilePath $installer -ArgumentList '/S' -Wait`. Cleans up the
     installer; re-asserts `7zFM.exe` exists post-install. No "latest" download.

  No `Restart-Computer`/reboot anywhere; every mutating section is a no-op on re-run.

### Approved 7-Zip pin (from Task 1 checkpoint)

| Field | Value |
| ----- | ----- |
| Version | 26.01 (released 2026-04-27) |
| URL | `https://github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe` |
| SHA-256 | `d64a0468f5b5b0b0fc5b2188450bcd655b70809d97b1c4535f2884635094377d` |
| Published-hash source | official `ip7z/7zip` GitHub release asset digest (`api.github.com/repos/ip7z/7zip/releases/tags/26.01`); computed == published; 7-zip.org/a/ mirror served a byte-for-byte identical file |

## Checkpoint (Task 1)

Task 1 was a **blocking human-verify** checkpoint (`gate="blocking-human"`, never
auto-approved). The executor fetched the current official x64 installer, downloaded it
from both official sources (GitHub release + 7-zip.org/a/ mirror), computed its SHA-256,
and confirmed it matched the GitHub release asset digest. The user reviewed and replied
**APPROVED**, pinning the GitHub release URL. Those exact constants were then hard-coded
in Task 2. No installer artifact was committed (by design — only the verified constants
land in the script).

## Task Commits

| Task | Name | Commit | Files |
| ---- | ---- | ------ | ----- |
| 1 | Confirm + verify pinned 7-Zip version/URL/SHA-256 | n/a (checkpoint — approval, no artifact) | none |
| 2 | Write the idempotent in-guest hardening script | `668646c` | `infra/scripts/Configure-Target.ps1` |

## Verification

PowerShell 7.5.4 available locally, so both gates ran for real (no Azure, no install):

- **Plan's automated gate** → `ok`: all required tokens present (`reg load`, `reg unload`,
  `LogPixels`, `Win8DpiScaling`, `RemoteDesktop_SuppressWhenMinimized`, `Get-FileHash`,
  `7zFM.exe`, `New-NetFirewallRule`); no `HKCU`; no `Restart-Computer`.
- **Syntax parse** (`[Parser]::ParseFile`) → `ok` (zero parse errors).
- **Constants + ordering gate** → `ok`: pinned SHA-256, URL, and version `26.01` present;
  the `SHA-256 mismatch` throw precedes `Start-Process` (hash check before install);
  no `latest .exe` download token.
- **No accidental deletions** in the commit; working tree clean after commit.

The live assertions (default-hive values, listener/firewall, `7zFM.exe`) over WinRM are
Plan 04's phase-gate responsibility (`Validate-Target.ps1` after a real `up`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Verify gate tripped on the literal "HKCU" inside rationale comments**
- **Found during:** Task 2 (first run of the plan's automated gate).
- **Issue:** The script writes nothing to the current-user hive, but its docstring/comments
  explained *why not* using the literal token "HKCU" (e.g. "NOT HKCU", "HKCU would write...").
  The gate (`if ($c -match 'HKCU') { throw }`) matches the literal anywhere in the file,
  including prose — the exact same class of false-positive Plan 02 hit with `0.0.0.0/0`
  and `deploymentScripts`.
- **Fix:** Reworded the three comment occurrences to "the current-user hive" while keeping
  the Pitfall-1 rationale intact. No functional change — the script never had an `HKCU:` write.
- **Files modified:** `infra/scripts/Configure-Target.ps1`
- **Commit:** `668646c`

### Scope notes (not deviations)
- Added `[gc]::WaitForPendingFinalizers()` alongside `[gc]::Collect()` and wrapped the
  hive edits in `try/finally` so the hive is always unloaded even if a `reg add` fails —
  a correctness hardening (Rule 2) for the load/unload pattern, fully within the plan's intent.
- Forced TLS 1.2 before the download for robustness on Windows PowerShell 5.1 defaults.
- ENV-01 left **Pending** in REQUIREMENTS.md: it is delivered jointly by Plans 02+03+04 and
  only provable by Plan 04's live phase gate — marking it complete now would be premature.

## Known Stubs

None. The script is complete and self-contained; the only forward dependency is that Plan 04
publishes it to a `scriptUri` and supplies that to the CSE — the documented Wave 2/3 contract,
not a stub.

## Threat Model Outcomes

- **T-01-08 / T-01-SC (7-Zip supply chain) — mitigated:** pinned official URL + SHA-256
  (confirmed via the blocking human-verify Task 1); `Get-FileHash` comparison throws before
  any installer runs; no "latest" download. The pin is a hard-coded constant matching the
  approved value.
- **T-01-10 (DPI/SuppressWhenMinimized written to the wrong hive) — mitigated:** values go
  to the loaded default user hive via `reg load`/`reg unload`; the build gate rejects any
  current-user (`HKCU`) per-user write (verified `ok`).
- **T-01-09 (self-signed WinRM cert MITM) — accepted (as planned):** self-signed listener is
  acceptable for a throwaway box scoped to one dev IP via the NSG; later-phase clients use
  `-SkipCACheck -SkipCNCheck` (documented in Plan 01's Validate-Target.ps1).

No new security surface introduced beyond the plan's threat model.

## Self-Check: PASSED

- FOUND: `infra/scripts/Configure-Target.ps1`
- FOUND commit: `668646c`
- VERIFIED: plan automated gate `ok`, syntax parse `ok`, constants+ordering `ok`
- VERIFIED: working tree clean, no accidental deletions
