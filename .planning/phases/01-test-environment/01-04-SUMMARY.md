---
phase: 01-test-environment
plan: 04
status: partial
subsystem: infra
tags: [bicep, azure-automation, managed-identity, runbook, rbac, powershell, sas, custom-script-extension, pester, wave-3, env-02, env-03]
one_liner: "Auto-destroy (separate-management Automation Account + managed-identity runbook + daily schedule + RG-scoped Contributor) and manage-env.ps1 up/down (crypto password, dev-IP detection, CSE/runbook SAS publish, gitignored connection file) — authored + compiled + Pester-green MOCKED; the live up→validate→down phase-gate (Task 4) is DEFERRED to a human run."

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
  - "The DEFERRED live phase-gate (Task 4) must be run by a human to prove ENV-01 green and close Phase 1"
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
requirements-completed: []  # ENV-02/ENV-03 are AUTHORED but NOT yet proven — the live phase-gate (Task 4) is DEFERRED. Do not mark complete until the human run passes.

duration: ~25min
completed: 2026-06-04
---

# Phase 1 Plan 04: Auto-Destroy + manage-env.ps1 Summary (PARTIAL — 3/4 tasks)

**Auto-destroy (separate-management Automation Account + managed-identity runbook + daily schedule + RG-scoped Contributor) and the `manage-env.ps1` up/down driver (crypto password, dev-IP detection, CSE/runbook SAS publish, gitignored connection file) were authored, compiled (`az bicep build` clean), and proven Pester-green MOCKED — the live `up`→`Validate-Target.ps1`→`down` phase-gate (Task 4) is DEFERRED to a human run by explicit user decision.**

> **STATUS: PARTIAL.** Tasks 1–3 are complete and committed on `develop`. **Task 4 (the live Azure phase-gate) was NOT executed** — no real deployment, no `az group create`, no `az deployment`, no `manage-env.ps1 -Action up` against a live subscription was performed. Phase 1 is therefore **NOT yet fully verified**: ENV-02/ENV-03 are authored but their live proof is pending the human-run gate documented below. Do not treat this plan or Phase 1 as complete.

## Performance

- **Duration:** ~25 min (Tasks 1–3 only)
- **Started:** 2026-06-04T21:08:26Z
- **Completed (authoring):** 2026-06-04
- **Tasks:** 3 of 4 (Task 4 deferred)
- **Files modified:** 5 (3 created, 2 modified)

## Accomplishments

- **Task 1 (decision, resolved out-of-band):** auto-destroy topology = **separate-management**, region = **West Europe**, subscription = **caller's active az subscription (never hard-coded)**. Recorded; drives Task 2's RBAC scope and resource placement.
- **Task 2 — auto-destroy (ENV-03 authored):** `Delete-ResourceGroup.ps1` (managed-identity runbook body), `modules/autodestroy.bicep` (Automation Account with `SystemAssigned` identity + PowerShell runbook + daily `schedule` + `jobSchedule` passing the TEST RG name), and `main.bicep` wiring (module scoped to the management RG + a Contributor `roleAssignment` scoped to the TEST RG only). `az bicep build` clean.
- **Task 3 — manage-env.ps1 (ENV-02 authored):** `-Action up|down` (ValidateSet), `az account show` pre-flight, dev-IP detection (ipify) with `-AllowedSourceIp` IPv4-validated override, crypto-strong ≥24-char password (never echoed), per-`up` storage account + private `cse` container, upload of **both** `Configure-Target.ps1` and `Delete-ResourceGroup.ps1`, short-lived **read-only** single-blob SAS full-URIs fed as `scriptUri` + `deleteRunbookContentUri`, deploy, gitignored `.secrets/connection.json`, and `down` teardown of the TEST RG. Pester: **8 passed, 0 skipped**, fully mocked (zero Azure cost, zero network).

## Task Commits

1. **Task 1: Resolve auto-destroy topology** — no artifact (decision checkpoint; resolved by user as separate-management + West Europe + runtime subscription)
2. **Task 2: Auto-destroy runbook + Bicep** — `7adf236` (feat)
3. **Task 3: manage-env.ps1 + activate Pester tests** — `d30b2a4` (feat)

**Plan metadata:** committed separately (this SUMMARY + STATE + ROADMAP) — marked PARTIAL.

_Task 4 (live phase-gate) has NO commit — it is deferred._

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

## DEFERRED: Task 4 — Live Phase-Gate (human-run)

Task 4 is the live `up → Validate-Target.ps1 → down` cycle. It incurs real (short-lived) Azure cost and was **deliberately deferred by the user**. It MUST be run by a human against an authenticated subscription to prove ENV-01/02/03 and close Phase 1. Exact sequence:

```bash
# 0. Authenticate; the driver uses whatever subscription is active (no hard-coded sub).
az login
az account set --subscription <your-subscription-id>   # optional — only if not already active
az account show                                          # confirm the intended subscription

# 1. Bring the environment up (creates rdpilot-mgmt + rdpilot-test, publishes scripts, deploys).
pwsh infra/manage-env.ps1 -Action up
#    Expect: .secrets/connection.json written (gitignored — verify: git check-ignore .secrets/connection.json).
#    Expect: az group show -n rdpilot-test  -> succeeds.
#    Expect: az group show -n rdpilot-mgmt  -> succeeds (persistent management RG).

# 2. Validate ENV-01 over WinRM (proves the CSE fetched + ran the published Configure-Target.ps1).
pwsh infra/tests/Validate-Target.ps1
#    Expect ALL green: RDP 3389 + WinRM 5986 reachable; NLA UserAuthentication=1;
#    default-hive LogPixels=96 / Win8DpiScaling=1; SuppressWhenMinimized=2 (HKLM + default hive);
#    7zFM.exe present.

# 3. Confirm the auto-destroy resources exist (separate-management).
az automation runbook show -g rdpilot-mgmt --automation-account-name rdpilot-autodestroy -n Delete-ResourceGroup
az role assignment list --scope $(az group show -n rdpilot-test --query id -o tsv) --query "[?roleDefinitionName=='Contributor']"
#    Expect: the runbook exists; the Automation identity holds Contributor over rdpilot-test ONLY.

# 4. Tear down the TEST RG (management RG persists).
pwsh infra/manage-env.ps1 -Action down
az group show -n rdpilot-test    # Expect: non-zero exit (RG gone / deleting).
az group show -n rdpilot-mgmt    # Expect: still present.

# 5. (Optional) verify the scheduled runbook itself deletes a scratch RG.
```

On a successful gate, mark ENV-02 + ENV-03 (and the ENV-01 live proof) complete in REQUIREMENTS.md, check the `01-04-PLAN.md` box in ROADMAP.md, and mark Phase 1 complete.

## Known Stubs

None in the authored code. The only outstanding item is the **deferred live phase-gate (Task 4)** above — an intentional, user-decided deferral, not a code stub.

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
- NOT DONE (deferred by user): Task 4 live phase-gate — no live deployment performed

---
*Phase: 01-test-environment (Plan 04 — PARTIAL, 3/4 tasks)*
*Completed (authoring): 2026-06-04*
