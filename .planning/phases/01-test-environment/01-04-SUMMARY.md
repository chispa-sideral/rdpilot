---
phase: 01-test-environment
plan: 04
status: complete
subsystem: infra
tags: [bicep, azure-automation, managed-identity, runbook, rbac, powershell, sas, custom-script-extension, pester, wave-3, env-02, env-03]
one_liner: "Auto-destroy (separate-management Automation Account + managed-identity runbook + daily schedule + RG-scoped Contributor) and manage-env.ps1 up/down (crypto password, dev-IP detection, CSE/runbook SAS publish, gitignored connection file) — authored + compiled + Pester-green MOCKED; live up→validate→down phase-gate (Task 4) PASSED 2026-06-04 (Standard_B2s_v2, IP 52.157.72.209, all ENV-01 assertions green, clean teardown)."

requires:
  - phase: 01-01
    provides: ".gitignore (.secrets/), Pester scaffold (skipped tests), Validate-Target.ps1 skeleton"
  - phase: 01-02
    provides: "main.bicep network+VM+CSE; scriptUri param + CSE fileUris contract"
  - phase: 01-03
    provides: "Configure-Target.ps1 (the CSE-published in-guest hardening script)"
provides:
  - "infra/scripts/Delete-ResourceGroup.ps1 — managed-identity auto-destroy runbook body (Connect-AzAccount -Identity, Remove-AzResourceGroup -Force)"
  - "infra/modules/autodestroy.bicep — Automation Account (SystemAssigned) + PowerShell runbook (publishContentLink) + daily schedule + jobSchedule, deployed into the persistent management RG"
  - "main.bicep auto-destroy wiring — module scoped to the management RG + Contributor roleAssignment scoped to the TEST RG only"
  - "infra/manage-env.ps1 — up/down driver: az auth pre-flight, dev-IP detection, crypto password, CSE+runbook SAS publish, deploy, gitignored .secrets/connection.json, teardown"
  - "infra/tests/ManageEnv.Tests.ps1 — all tests activated + scriptUri-publish assertions; 8 passed, 0 skipped, fully mocked"
affects:
  - "Phase 2+ consume .secrets/connection.json (host/user/password/ports) produced by manage-env.ps1 up"
  - "Live phase-gate (Task 4) PASSED 2026-06-04: Standard_B2s_v2, IP 52.157.72.209, all six ENV-01 assertions green, clean teardown — ENV-01/02/03 proven"
tech-stack:
  added:
    - "Azure Automation: automationAccounts@2023-11-01 (+ runbooks/schedules/jobSchedules), system-assigned managed identity"
    - "Microsoft.Authorization/roleAssignments@2022-04-01 (Contributor built-in role, GUID b24988ac-...)"
    - "Bicep module + cross-RG scope (module scope: resourceGroup(managementResourceGroupName))"
  patterns:
    - "separate-management auto-destroy topology: deleter lives in a persistent management RG, identity scoped Contributor over the TEST RG only"
    - "Runbook content pinned to repo source via a short-lived read-only SAS publishContentLink (not 'latest' off the internet)"
    - "manage-env.ps1 publishes BOTH Configure-Target.ps1 (CSE) and Delete-ResourceGroup.ps1 (runbook) to a private blob + single-blob read-only SAS"
    - "az auth pre-flight checks $LASTEXITCODE of `az account show` (no hard-coded subscription — uses the caller's active sub)"
    - "Pester mock resets $global:LASTEXITCODE=0 to simulate a successful native az call"
key-files:
  created:
    - "infra/scripts/Delete-ResourceGroup.ps1"
    - "infra/modules/autodestroy.bicep"
    - "infra/manage-env.ps1"
  modified:
    - "infra/main.bicep (auto-destroy params + module + role assignment)"
    - "infra/tests/ManageEnv.Tests.ps1 (activated + scriptUri-wiring tests)"
key-decisions:
  - "Auto-destroy topology = separate-management (user decision): persistent management RG holds the Automation Account; identity = Contributor over the TEST RG ONLY (tightest least-privilege). Avoids the self-delete job-status caveat (Pitfall 4)."
  - "Region = West Europe (default, confirmed)."
  - "Subscription NOT hard-coded: manage-env.ps1 uses the caller's currently-active az subscription (`az account show` pre-flight)."
  - "Runbook body delivered via publishContentLink to a per-deploy read-only SAS blob (pinned to repo source) rather than inline content — the well-supported ARM pattern; manage-env.ps1 mints the SAS."
  - "Management RG (default rdpilot-mgmt) is PERSISTENT — `up` ensures it exists, `down` leaves it in place (only the TEST RG is torn down)."
patterns-established:
  - "separate-management RBAC: roleAssignment in the target RG referencing a cross-RG module's principalId output"
  - "Per-up transient storage account (Standard_LRS, public access off) in the TEST RG for CSE+runbook script publishing, torn down with the RG"
requirements-completed: [ENV-01, ENV-02, ENV-03]  # Live phase-gate passed 2026-06-04: Standard_B2s_v2, IP 52.157.72.209, all assertions green.

duration: ~25min
completed: 2026-06-04
---

# Phase 1 Plan 04: Auto-Destroy + manage-env.ps1 Summary (COMPLETE — 4/4 tasks)

**Auto-destroy (separate-management Automation Account + managed-identity runbook + daily schedule + RG-scoped Contributor) and the `manage-env.ps1` up/down driver (crypto password, dev-IP detection, CSE/runbook SAS publish, gitignored connection file) were authored, compiled (`az bicep build` clean), Pester-green MOCKED, and the live `up`→`Validate-Target.ps1`→`down` phase-gate (Task 4) PASSED on 2026-06-04 with all six ENV-01 assertions green.**

> **STATUS: COMPLETE.** All 4 tasks done. ENV-01/02/03 proven via live gate: `Standard_B2s_v2` (note: `Standard_B2ms` was capacity-restricted in westeurope at run time), public IP 52.157.72.209, RDP 3389 + WinRM 5986 reachable, NLA enforced, 96 DPI, `SuppressWhenMinimized=2`, 7zFM.exe present. Clean teardown confirmed.

## Performance

- **Duration:** ~25 min (Tasks 1–3 authoring) + live gate run 2026-06-04
- **Started:** 2026-06-04T21:08:26Z
- **Completed:** 2026-06-04 (live gate passed)
- **Tasks:** 4 of 4 (all complete)
- **Files modified:** 5 (3 created, 2 modified) + post-run hardening commits (see below)

## Accomplishments

- **Task 1 (decision, resolved out-of-band):** auto-destroy topology = **separate-management**, region = **West Europe**, subscription = **caller's active az subscription (never hard-coded)**. Recorded; drives Task 2's RBAC scope and resource placement.
- **Task 2 — auto-destroy (ENV-03 authored):** `Delete-ResourceGroup.ps1` (managed-identity runbook body), `modules/autodestroy.bicep` (Automation Account with `SystemAssigned` identity + PowerShell runbook + daily `schedule` + `jobSchedule` passing the TEST RG name), and `main.bicep` wiring (module scoped to the management RG + a Contributor `roleAssignment` scoped to the TEST RG only). `az bicep build` clean.
- **Task 3 — manage-env.ps1 (ENV-02 authored):** `-Action up|down` (ValidateSet), `az account show` pre-flight, dev-IP detection (ipify) with `-AllowedSourceIp` IPv4-validated override, crypto-strong ≥24-char password (never echoed), per-`up` storage account + private `cse` container, upload of **both** `Configure-Target.ps1` and `Delete-ResourceGroup.ps1`, short-lived **read-only** single-blob SAS full-URIs fed as `scriptUri` + `deleteRunbookContentUri`, deploy, gitignored `.secrets/connection.json`, and `down` teardown of the TEST RG. Pester: **8 passed, 0 skipped**, fully mocked (zero Azure cost, zero network).

## Task Commits

1. **Task 1: Resolve auto-destroy topology** — no artifact (decision checkpoint; resolved by user as separate-management + West Europe + runtime subscription)
2. **Task 2: Auto-destroy runbook + Bicep** — `7adf236` (feat)
3. **Task 3: manage-env.ps1 + activate Pester tests** — `d30b2a4` (feat)
4. **Task 4: Live phase-gate** — PASSED 2026-06-04 (human run; no code commit — gate is a runtime validation)

**Post-run hardening commits** (applied after initial authoring, before the live gate run):
- `2a5db63` — safe password passing via params file (no cmd leak)
- `7797b25` — Validate-Target targets remote IP / fails on missing target
- `20af3d4` — up preflight: RG-state guard + SKU capacity check + `-VmSize`; down `-NoWait`/default-wait; verbose status

**Plan metadata:** committed separately (this SUMMARY + STATE + ROADMAP).

## Files Created/Modified

- `infra/scripts/Delete-ResourceGroup.ps1` (created) — runbook body: `param([string]$ResourceGroupName)`, `Disable-AzContextAutosave`, `Connect-AzAccount -Identity`, idempotent `Remove-AzResourceGroup -Force`. Managed identity only; no RunAs.
- `infra/modules/autodestroy.bicep` (created) — Automation Account (`SystemAssigned`) + runbook (`PowerShell`, `publishContentLink`) + daily `schedule` + `jobSchedule`; outputs `principalId`. Deployed into the management RG.
- `infra/main.bicep` (modified) — added params (`managementResourceGroupName`, `automationAccountName`, `autoDestroyStartTime`, `deleteRunbookContentUri`), the `autoDestroy` module (scoped to the management RG), and the Contributor `roleAssignment` scoped to the TEST RG.
- `infra/manage-env.ps1` (created) — the up/down orchestration driver (ENV-02).
- `infra/tests/ManageEnv.Tests.ps1` (modified) — un-skipped all tests; added a scriptUri-publish assertion and a publish-precedes-deploy ordering assertion.

## Decisions Made

See `key-decisions` frontmatter. Headlines: separate-management topology (RG-scoped Contributor, no self-delete caveat); West Europe; subscription is the caller's active az sub (never hard-coded); runbook content pinned via a per-deploy read-only SAS `publishContentLink`; the management RG is persistent.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Task 2 verify gate tripped on the literal "Owner" inside rationale comments**
- **Found during:** Task 2 (the gate `if ($b -match '(?i)Owner') { throw }` matches the literal anywhere, including prose).
- **Issue:** Comments reading "never subscription-Owner" tripped the no-`Owner` gate even though the role granted is Contributor (no `Owner` role anywhere). Same false-positive class as Plans 02/03 (`0.0.0.0/0`, `deploymentScripts`, `HKCU`).
- **Fix:** Reworded the three prose occurrences (`main.bicep` ×2, `Delete-ResourceGroup.ps1` ×1) to "the subscription full-access role" / "the full-access subscription role". No functional change — the role assignment uses the Contributor GUID `b24988ac-6180-42a0-ab88-20f7382dd24c`.
- **Files modified:** `infra/main.bicep`, `infra/scripts/Delete-ResourceGroup.ps1`
- **Verification:** Re-ran the Task 2 gate → `ok`.
- **Committed in:** `7adf236` (Task 2 commit).

**2. [Rule 1 - Bug] Pester mock left `$LASTEXITCODE` non-zero, tripping the `az account show` pre-flight**
- **Found during:** Task 3 (first Pester run: 5 tests failed with "Not authenticated to Azure").
- **Issue:** The driver's pre-flight checks `$LASTEXITCODE` after `az account show`. A `Mock az {}` does not reset the native `$LASTEXITCODE`, which retained a non-zero value from the un-logged-in CI environment, so the pre-flight threw before the mocked deploy was reached.
- **Fix:** The shared `az` mock now sets `$global:LASTEXITCODE = 0`, faithfully simulating a successful native `az` call. This is a test-only fix; the driver's pre-flight logic is correct for a real run.
- **Files modified:** `infra/tests/ManageEnv.Tests.ps1`
- **Verification:** Re-ran Pester → 8 passed, 0 failed, 0 skipped.
- **Committed in:** `d30b2a4` (Task 3 commit).

### Scope notes (not deviations)

- The plan allowed inline runbook content OR a `publishContentLink` URI ("planner's call, keep it pinned"). Chose `publishContentLink` to a per-deploy **read-only SAS** blob (minted by `manage-env.ps1` from the in-repo `Delete-ResourceGroup.ps1`) — the well-supported ARM pattern that keeps the runbook pinned to the repo source. `manage-env.ps1` publishes both scripts; `main.bicep` gains a `deleteRunbookContentUri` param.
- Added a `down`-persistent management RG (`-ManagementRg`, default `rdpilot-mgmt`); `up` ensures it exists, `down` deletes only the TEST RG. Required by the separate-management topology.

## Issues Encountered

None beyond the two auto-fixes above. Toolchain fully present: `az bicep` 0.43.8, Pester 5.7.1, PowerShell 7.5.4 + Windows PowerShell 5.1.

## Threat Model Outcomes

- **T-01-11 (over-privileged runbook identity) — mitigated:** Contributor scoped to the TEST RG only; no `Owner`/subscription-wide grant. Build gate enforces no `Owner` token.
- **T-01-12 (password leak) — mitigated (authored):** crypto RNG, never echoed, written only to gitignored `.secrets/connection.json`, passed to ARM as `@secure()`. (Live confirmation is part of the deferred gate.)
- **T-01-13 (runaway cost) — mitigated (authored):** daily auto-destroy runbook + on-demand `down`; fires even if the dev machine is off. (Live confirmation is part of the deferred gate.)
- **T-01-14 (spoofed IP) — mitigated:** `-AllowedSourceIp` override validated as IPv4; ipify over HTTPS; NSG scoped to a single source.
- **T-01-15 (self-delete job status) — N/A:** avoided entirely by the separate-management topology.
- **T-01-16 (SAS leak/over-scope) — mitigated:** SAS is read-only (`--permissions r`), single-blob, ~1h TTL, HTTPS-only; container is private (public access off); SAS never echoed; storage account torn down with the TEST RG.

## Task 4 — Live Phase-Gate: PASSED (2026-06-04)

The live `up → Validate-Target.ps1 → down` cycle was executed by the user on 2026-06-04.

**Deployment details:**
- VM SKU: `Standard_B2s_v2` (note: `Standard_B2ms` was capacity-restricted in westeurope at run time; `-VmSize` override used)
- Public IP: 52.157.72.209
- RGs: `rdpilot-test` (torn down) + `rdpilot-mgmt` (persistent, as designed)

**ENV-01 assertions — all six green:**
1. RDP port 3389 reachable
2. WinRM port 5986 reachable
3. NLA enforced (`UserAuthentication=1`)
4. Forced 96 DPI (`LogPixels=96` / `Win8DpiScaling=1` in default hive)
5. `RemoteDesktop_SuppressWhenMinimized=2` (HKLM + default hive)
6. 7zFM.exe present

**Teardown:** `manage-env.ps1 -Action down` removed `rdpilot-test` cleanly; `rdpilot-mgmt` persisted as designed.

Phase 1 is fully verified. ENV-01, ENV-02, and ENV-03 are proven.

## Known Stubs

None. All tasks complete; live phase-gate passed.

## Self-Check: PASSED

- FOUND: `infra/scripts/Delete-ResourceGroup.ps1`
- FOUND: `infra/modules/autodestroy.bicep`
- FOUND: `infra/manage-env.ps1`
- FOUND: `infra/main.bicep` (auto-destroy module + role assignment)
- FOUND: `infra/tests/ManageEnv.Tests.ps1` (8 passed, 0 skipped, mocked)
- FOUND commit: `7adf236` (Task 2)
- FOUND commit: `d30b2a4` (Task 3)
- VERIFIED: `az bicep build --file infra/main.bicep` compiles clean
- VERIFIED: `Invoke-Pester infra/tests/ManageEnv.Tests.ps1 -CI` → 8 passed, 0 failed, 0 skipped (zero Azure cost)
- VERIFIED: `.secrets/connection.json` gitignored; no `.secrets/` artifact created by the mocked test run
- VERIFIED live (2026-06-04): Task 4 phase-gate — `manage-env.ps1 -Action up -VmSize Standard_B2s_v2` deployed to 52.157.72.209; `Validate-Target.ps1` reported all ENV-01 checks green; `manage-env.ps1 -Action down` tore down cleanly.

---
*Phase: 01-test-environment (Plan 04 — COMPLETE, 4/4 tasks)*
*Completed: 2026-06-04 (live phase-gate)*
