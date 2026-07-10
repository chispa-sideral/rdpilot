---
phase: 1
slug: test-environment
status: approved
nyquist_compliant: true
wave_0_complete: true
created: 2026-06-04
---

# Phase 1 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.
> IaC phase — "tests" are Pester logic tests + Bicep compile/what-if + post-deploy WinRM assertions, not a single unit-test framework (RESEARCH.md Validation Architecture L423-456).

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | **Pester** (PowerShell 5.x, pinned) for `.ps1` logic + post-deploy assertions; `az bicep build` + `az deployment group what-if` for Bicep |
| **Config file** | none — Wave 0 (Plan 01) creates `infra/tests/*.Tests.ps1` and installs Pester if absent |
| **Quick run command** | `az bicep build --file infra/main.bicep` (compile) + `Invoke-Pester -Path infra/tests/ManageEnv.Tests.ps1 -CI` (no Azure cost, `az`/network mocked) |
| **Full suite command** | `pwsh infra/manage-env.ps1 -Action up` → `pwsh infra/tests/Validate-Target.ps1` → `pwsh infra/manage-env.ps1 -Action down` (phase gate — short-lived Azure cost) |
| **Estimated runtime** | ~10 seconds (quick: Pester + bicep build); phase gate ~10-15 min (live provision) |

---

## Sampling Rate

- **After every task commit:** Run the quick command — `az bicep build --file infra/main.bicep` for any Bicep change AND `Invoke-Pester -Path infra/tests -CI` for any `.ps1` logic change (no Azure cost — `az`/`Invoke-RestMethod` mocked).
- **After every plan wave:** `az deployment group what-if -g <scratch-rg> --template-file infra/main.bicep --parameters ...` (no-cost preview) once `main.bicep` exists (Wave 2+).
- **Before `/gsd-verify-work`:** Full suite (phase gate) must be green — live `up` → `Validate-Target.ps1` (all ENV-01 green) → `down`.
- **Max feedback latency:** ~10 seconds (quick command).

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 01-01-T1 | 01 | 1 | ENV-02 (scaffold) | T-01-01 | `.secrets/`/`.env` gitignored before any `up` | static | `pwsh -NoProfile -Command "if ((Get-Content .gitignore -Raw) -notmatch '\.secrets/') { throw }; if (-not (Test-Path infra/tests) -or -not (Test-Path infra/scripts)) { throw }; 'ok'"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-01-T2 | 01 | 1 | ENV-02 (scaffold) | T-01-02 | password length/class only, never echoed | unit | `pwsh -NoProfile -Command "Invoke-Pester -Path infra/tests/ManageEnv.Tests.ps1 -CI"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-01-T3 | 01 | 1 | ENV-01 (scaffold) | T-01-02 | WinRM `-SkipCACheck -SkipCNCheck -UseSSL`; no password echo | integration (skeleton) | `pwsh -NoProfile -Command "& { . infra/tests/Validate-Target.ps1 }; if ($LASTEXITCODE -ne 0) { throw }; if ((Get-Content infra/tests/Validate-Target.ps1 -Raw) -notmatch 'UserAuthentication') { throw }; 'ok'"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-02-T1 | 02 | 2 | ENV-01 | T-01-04 / T-01-05 / T-01-06 | `@secure()` password; NSG scoped to dev IP, never `0.0.0.0/0` | static | `az bicep build --file infra/main.bicep && pwsh -NoProfile -Command "$c = Get-Content infra/main.bicep -Raw; if ($c -notmatch '@secure\(\)') { throw }; if ($c -match '0\.0\.0\.0/0') { throw }; if ($c -notmatch '2022-datacenter-g2') { throw }; 'ok'"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-02-T2 | 02 | 2 | ENV-01 | T-01-07 | CSE not `deploymentScripts`; wired to `Configure-Target.ps1` | static | `az bicep build --file infra/main.bicep && pwsh -NoProfile -Command "$c = Get-Content infra/main.bicep -Raw; if ($c -notmatch 'CustomScriptExtension') { throw }; if ($c -notmatch 'Configure-Target\.ps1') { throw }; if ($c -match 'deploymentScripts') { throw }; 'ok'"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-03-T1 | 03 | 2 | ENV-01 | T-01-08 / T-01-SC | 7-Zip URL + SHA-256 from official source, blocking checkpoint | manual (see Manual-Only) | — (blocking human-verify; no auto command) | n/a (checkpoint) | ⬜ pending |
| 01-03-T2 | 03 | 2 | ENV-01 | T-01-08 / T-01-10 | DPI/Suppress to default hive (not HKCU); SHA-256 gate before install; no reboot | static | `pwsh -NoProfile -Command "$c = Get-Content infra/scripts/Configure-Target.ps1 -Raw; foreach ($t in @('reg load','reg unload','LogPixels','Win8DpiScaling','RemoteDesktop_SuppressWhenMinimized','Get-FileHash','7zFM.exe','New-NetFirewallRule')) { if ($c -notmatch [regex]::Escape($t)) { throw } }; if ($c -match 'HKCU') { throw }; if ($c -match '(?i)Restart-Computer') { throw }; 'ok'"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-04-T1 | 04 | 3 | ENV-03 | T-01-11 / T-01-15 | auto-destroy topology + RBAC scope chosen with user | manual (see Manual-Only) | — (blocking decision; no auto command) | n/a (checkpoint) | ⬜ pending |
| 01-04-T2 | 04 | 3 | ENV-03 | T-01-11 / T-01-13 | managed-identity runbook; Contributor not Owner | static | `az bicep build --file infra/main.bicep && pwsh -NoProfile -Command "$b = Get-Content infra/main.bicep -Raw; foreach ($t in @('Microsoft.Automation/automationAccounts','runbooks','schedules','roleAssignments','SystemAssigned')) { if ($b -notmatch [regex]::Escape($t)) { throw } }; $r = Get-Content infra/scripts/Delete-ResourceGroup.ps1 -Raw; if ($r -notmatch 'Connect-AzAccount -Identity') { throw }; if ($r -notmatch 'Remove-AzResourceGroup') { throw }; if ($b -match '(?i)Owner') { throw }; 'ok'"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-04-T3 | 04 | 3 | ENV-02 | T-01-12 / T-01-14 | crypto password never echoed; written only to gitignored `.secrets/`; IP override validated | unit + static | `pwsh -NoProfile -Command "$s = Get-Content infra/manage-env.ps1 -Raw; foreach ($t in @('ValidateSet','az account show','az group create','az deployment group create','az group delete','connection.json','AllowedSourceIp')) { if ($s -notmatch [regex]::Escape($t)) { throw } }; 'ok'" && pwsh -NoProfile -Command "Invoke-Pester -Path infra/tests/ManageEnv.Tests.ps1 -CI"` | ❌ W0 (this task creates it) | ⬜ pending |
| 01-04-T4 | 04 | 3 | ENV-01 / ENV-02 / ENV-03 | all | live phase gate: all ENV-01 green, runbook+RBAC exist, RG removed | manual (see Manual-Only) | — (blocking human-verify; phase gate, see Full suite command) | live target | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

All Wave 0 gaps from RESEARCH.md (L452-456) are satisfied by **Plan 01** (Wave 1) before any Bicep is authored (Wave 2) or any `up` is run (Wave 3):

- [x] `.gitignore` — `.secrets/`, `.env` (Plan 01 Task 1) — repo had none; blocking for D-06 safety
- [x] `infra/tests/Validate-Target.ps1` — post-deploy WinRM assertion skeleton covering all ENV-01 criteria (Plan 01 Task 3)
- [x] `infra/tests/ManageEnv.Tests.ps1` — Pester tests for IP detection, password generation, `az` param wiring, all mocked (Plan 01 Task 2)
- [x] Pester install if absent — `Install-Module Pester -Scope CurrentUser -Force -SkipPublisherCheck`, pinned 5.x (Plan 01 Task 2)

*All MISSING/Wave-0 references in the Per-Task Verification Map are created within Plan 01 (Wave 1), ahead of every consumer.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| 7-Zip version/URL/SHA-256 are official and the computed hash matches the published hash | ENV-01 (T-01-08 / T-01-SC) | Supply-chain integrity cannot be statically verified at plan time (RESEARCH.md A3); must read the live official source. Blocking checkpoint, never auto-approved. | Plan 03 Task 1: retrieve installer from 7-zip.org / ip7z GitHub release, `Get-FileHash -Algorithm SHA256`, compare to published hash, approve the exact URL+hash pair. |
| Auto-destroy topology + RBAC scope decision | ENV-03 (T-01-11 / T-01-15) | Architectural decision (self-contained vs separate-management RG) drives RBAC scope and runbook placement; planner must not pick silently (PATTERNS.md Open Decision 1). | Plan 04 Task 1: choose `self-contained` or `separate-management`, confirm West Europe region + target subscription. |
| Live phase gate: all ENV-01 assertions green against the real VM, auto-destroy resources exist, teardown removes the RG | ENV-01 / ENV-02 / ENV-03 | Requires a live Azure subscription and short-lived VM cost; integration assertions only meaningful against a provisioned target. | Plan 04 Task 4: `up` → `Validate-Target.ps1` (RDP 3389 + WinRM 5986 reachable, NLA `UserAuthentication=1`, default-hive `LogPixels=96`/`Win8DpiScaling=1`, SuppressWhenMinimized=2, `7zFM.exe`) → confirm runbook + Contributor role → `down`. |

*Each manual verification is a blocking checkpoint; the live phase gate also corresponds to the "Full suite command" above.*

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or are documented blocking checkpoints (manual) with Wave 0 dependencies satisfied by Plan 01
- [x] Sampling continuity: no 3 consecutive tasks without automated verify — the only non-automated tasks (01-03-T1, 01-04-T1, 01-04-T4) are checkpoints, each adjacent to automated tasks (01-03-T2, 01-04-T2/T3)
- [x] Wave 0 covers all MISSING references (`.gitignore`, `Validate-Target.ps1`, `ManageEnv.Tests.ps1`, Pester install) — all in Plan 01
- [x] No watch-mode flags (`-CI` used for Pester; no `-Watch`)
- [x] Feedback latency < 10s for the quick command
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** approved 2026-06-04
