---
phase: 01-test-environment
plan: 02
subsystem: infra
tags: [bicep, iac, azure, vm, nsg, custom-script-extension, wave-2, env-01]
one_liner: "infra/main.bicep — the ENV-01 cloud resource graph: VNet/NSG (IP-scoped RDP 3389 + WinRM 5986), Standard/Static public IP, NIC, a WS2022 Datacenter Gen2 Desktop Standard_B2ms VM with a @secure() admin password, and one CustomScriptExtension that invokes the (Plan 03) Configure-Target.ps1."
requires:
  - "infra/ IaC root + .gitignore (Plan 01)"
provides:
  - "infra/main.bicep — declarative network + VM + NSG + CustomScriptExtension provisioning (compiles clean)"
  - "@secure() adminPassword param (no default) — keeps the credential out of ARM deployment history"
  - "NSG scoped to allowedSourceIp for 3389/5986 (never the open internet)"
  - "CSE -> Configure-Target.ps1 contract (filename, no-arg invocation, scriptUri param) that Plan 03 implements"
  - "Bicep outputs publicIp/adminUsername/rdpPort/winrmPort for manage-env.ps1 + Validate-Target.ps1"
affects:
  - "Plan 03 (Configure-Target.ps1) implements the CSE contract defined here"
  - "Plan 04 (manage-env.ps1) supplies adminPassword/allowedSourceIp/scriptUri params and consumes the outputs"
tech_stack:
  added:
    - "Bicep template targeting resourceGroup scope (az bicep 0.43.8)"
    - "Pinned Azure API versions: Network @2024-05-01, Compute @2024-07-01"
  patterns:
    - "Single main.bicep, single RG so `az group delete` cascades (no per-resource teardown)"
    - "Every @apiVersion pinned explicitly — no 'latest' API"
    - "@secure() param with no default for the admin password (D-06)"
    - "NSG least-exposure: only 3389 + 5986, only from allowedSourceIp (D-05)"
    - "One CustomScriptExtension runs one idempotent in-guest script (CSE over the container-bound deployment-script type)"
    - "NLA left at the Azure default — verified post-deploy, never configured (Pitfall 2)"
key_files:
  created:
    - "infra/main.bicep"
  modified:
    - ".gitignore (added infra/*.json — compiled Bicep ARM output)"
decisions:
  - "VNet 10.0.0.0/16 with a single 10.0.0.0/24 'default' subnet; NSG associated at the subnet (covers the NIC)"
  - "OS disk StandardSSD_LRS (cheap, adequate for a short-lived disposable burstable VM)"
  - "Subnet-level NSG association (not NIC-level) — simpler graph, same effect for a single-NIC VM"
  - "Compiled main.json is a transient build artifact — gitignored (infra/*.json), source of truth is main.bicep"
metrics:
  duration: "~10 min"
  completed: "2026-06-04"
  tasks: 2
  files: 2
  commits: 2
---

# Phase 1 Plan 02: Core Infrastructure Bicep Summary

`infra/main.bicep` is the **ENV-01 cloud resource graph** for the disposable Windows
Server 2022 test target. One template, one resource group: VNet + subnet, an NSG that
opens RDP 3389 and WinRM-HTTPS 5986 **only** from the developer's detected public IP, a
Standard/Static public IP, a NIC, the `Standard_B2ms` WS2022 Datacenter **Gen2 Desktop**
VM with a `@secure()` admin password, and a single `CustomScriptExtension` that invokes
the in-guest `Configure-Target.ps1` (authored separately in Plan 03). It compiles clean
under `az bicep build`. This plan owns the cloud graph and **defines** the CSE→script
contract; Plan 03 owns the script body, Plan 04 supplies the deployment params and the
auto-destroy resources.

## What Was Built

- **`infra/main.bicep` (targetScope `resourceGroup`):**
  - **Params:** `@secure() adminPassword` (no default — passed by manage-env.ps1, never
    logged/committed; `@secure()` keeps it out of ARM deployment history, T-01-05);
    `adminUsername = 'rdpadmin'`; `allowedSourceIp` (dev public IP, D-05);
    `vmSize = 'Standard_B2ms'` (one-flag switch to `Standard_D2s_v5`, Pitfall 6);
    `location = resourceGroup().location`; `scriptUri` (where Configure-Target.ps1 is
    published at deploy time, supplied by Plan 04).
  - **`rdpilot-nsg`** (`networkSecurityGroups@2024-05-01`) — exactly two inbound allow
    rules: `allow-rdp` (priority 1000, TCP, dest 3389) and `allow-winrm-https`
    (priority 1010, TCP, dest 5986), both `sourceAddressPrefix: allowedSourceIp`,
    `destinationAddressPrefix: '*'`. The file contains no open-internet CIDR (T-01-06).
  - **`rdpilot-vnet`** (`virtualNetworks@2024-05-01`) — `10.0.0.0/16` with a single
    `default` subnet `10.0.0.0/24`, NSG associated at the subnet.
  - **`rdpilot-pip`** (`publicIPAddresses@2024-05-01`) — SKU `Standard`, allocation `Static`.
  - **`rdpilot-nic`** (`networkInterfaces@2024-05-01`) — binds the subnet + public IP.
  - **`rdpilot-vm`** (`virtualMachines@2024-07-01`) — `hardwareProfile.vmSize: vmSize`;
    image `MicrosoftWindowsServer` / `WindowsServer` / **`2022-datacenter-g2`** / `latest`
    (D-01 Gen2 Desktop — not `-core`, not `-azure-edition`); `osProfile.windowsConfiguration`
    with `provisionVMAgent` + `enableAutomaticUpdates` (NLA left at the Azure default,
    verified later — Pitfall 2); `StandardSSD_LRS` OS disk.
  - **`Configure-Target` CSE** (`virtualMachines/extensions@2024-07-01`, parented to the VM)
    — publisher `Microsoft.Compute`, type `CustomScriptExtension`, handler `1.10`,
    `autoUpgradeMinorVersion`. `settings.fileUris` = `[ scriptUri ]`;
    `protectedSettings.commandToExecute` = `powershell -ExecutionPolicy Unrestricted -File Configure-Target.ps1`.
    A leading comment documents that the script body lives in `infra/scripts/Configure-Target.ps1`
    (Plan 03) and that the deployment-script container type was deliberately rejected (it
    cannot reach the guest registry/WinRM).
  - **Outputs:** `publicIp`, `adminUsername`, `rdpPort` (3389), `winrmPort` (5986).

## Task Commits

| Task | Name | Commit | Files |
| ---- | ---- | ------ | ----- |
| 1 | Network + VM Bicep (VNet, NSG, public IP, NIC, VM) | `1581c69` | `infra/main.bicep`, `.gitignore` |
| 2 | Wire CustomScriptExtension to Configure-Target.ps1 | `df3b1ec` | `infra/main.bicep` |

## Verification

Toolchain available locally — `az bicep` **0.43.8** verified present, so the compile gate
was run for real (no Azure deployment, no cost).

- `az bicep build --file infra/main.bicep` → compiles clean, no errors (both tasks).
- Content gate (Task 1): `@secure()` present; no open-internet CIDR; image sku is
  `2022-datacenter-g2` → `ok`.
- Content gate (Task 2): contains `CustomScriptExtension`; wired to `Configure-Target.ps1`;
  no deployment-script resource token → `ok`.
- Full acceptance scan: `@secure()` decorates `adminPassword` (no default); public IP SKU
  `Standard` + allocation `Static`; CSE handler `1.10`; all four outputs present; no
  `Microsoft.Resources/deploymentScripts` resource — all True.

`az deployment group validate` / `what-if` against a scratch RG is intentionally deferred
to Plan 04's phase gate (it needs sample params + an authenticated subscription) per the
plan's verification note.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Verify gate tripped on the literal "0.0.0.0/0" inside a Bicep comment**
- **Found during:** Task 1 (the verify command and threat model T-01-06 fail the file if it
  contains `0.0.0.0/0` *anywhere*, including prose).
- **Issue:** An `@description` comment read "...never `0.0.0.0/0`." The substring tripped the
  build gate even though no NSG rule used it.
- **Fix:** Reworded the comment to "...this single source only — never the open internet."
  No functional change to the resource graph.
- **Files modified:** `infra/main.bicep`
- **Commit:** `1581c69`

**2. [Rule 3 - Blocking] Verify gate tripped on the literal "deploymentScripts" inside a comment**
- **Found during:** Task 2 (the verify command fails the file if it contains `deploymentScripts`
  anywhere).
- **Issue:** A comment explaining *why* CSE was chosen used the word "deploymentScripts",
  tripping the no-`deploymentScripts` gate even though no such resource exists.
- **Fix:** Reworded to "the deployment-script resource type" (hyphenated prose), preserving
  the rationale without the forbidden token.
- **Files modified:** `infra/main.bicep`
- **Commit:** `df3b1ec`

**3. [Rule 3 - Blocking] `az bicep build` emits an untracked `infra/main.json`**
- **Found during:** Task 1 (post-build `git status` showed an untracked `infra/main.json`).
- **Issue:** `az bicep build` writes the compiled ARM JSON next to the source. It is a
  transient build artifact, not a source file, and should never be committed.
- **Fix:** Added `infra/*.json` to `.gitignore` (committed with Task 1). The source of truth
  is `main.bicep`; the JSON is regenerated on every build.
- **Files modified:** `.gitignore`
- **Commit:** `1581c69`

## Known Stubs

None functionally — but one **forward reference is expected and by design:** the CSE's
`commandToExecute` invokes `Configure-Target.ps1`, and `scriptUri` points at where that
script is published at deploy time. The script itself is authored in **Plan 03**, and
`scriptUri` is supplied by **manage-env.ps1 in Plan 04**. This is the documented Wave 2
CSE→script contract, not an unresolved stub.

## Threat Model Outcomes

- **T-01-05 (adminPassword in deployment history) — mitigated:** `adminPassword` is a
  `@secure()` param with no default. Verified by the build gate (`@secure()` required, blocks otherwise).
- **T-01-06 (over-broad NSG / open internet) — mitigated:** both inbound rules use
  `sourceAddressPrefix: allowedSourceIp`; the build gate fails the file on any open-internet
  CIDR. Verified clean.
- **T-01-04 (RDP brute-force) — mitigated (this plan's share):** RDP 3389 scoped to
  `allowedSourceIp` only; NLA left at the Azure default for later verification (Pitfall 2);
  strong random password is Plan 04's responsibility.
- **T-01-07 (wrong CSE tool / unguarded script) — mitigated (this plan's share):** uses
  `CustomScriptExtension` (not the container-bound deployment-script type); the idempotent,
  no-reboot guarded script is the Plan 03 contract this template defines.

No new security surface introduced beyond the plan's threat model.

## Self-Check: PASSED

- FOUND: `infra/main.bicep`
- FOUND commit: `1581c69`
- FOUND commit: `df3b1ec`
- VERIFIED: `az bicep build --file infra/main.bicep` compiles clean
- VERIFIED: `infra/main.json` is gitignored (working tree clean after commits)
