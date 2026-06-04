# Phase 1: Test Environment - Pattern Map

**Mapped:** 2026-06-04
**Files analyzed:** 9 (new artifacts)
**Analogs found:** 0 / 9 (greenfield — no in-repo analogs exist; see "No Analog Found" + "Conventions to Establish")

## Greenfield Notice

This is rdpilot's FIRST implementation phase. The repository contains only `.planning/` documentation and `CLAUDE.md` — verified via `git ls-files`:

```
.planning/...   CLAUDE.md
```

There is **no `Cargo.toml`, no `src/`, no `infra/`, no `.gitignore`, no existing `.ps1` or `.bicep`**. Therefore there are **zero in-codebase analogs** for the planner to copy from. This document does **not** invent analogs. Instead it does two things:

1. **Classifies** each file this phase creates by role + data flow.
2. **Records the conventions the planner should establish as the baseline** (file layout, naming, idempotency, secrets handling), plus the **external verified pattern source** for each file (from `01-RESEARCH.md`, which cites first-party Microsoft samples). The planner should treat those research excerpts as the "pattern to copy from" in lieu of a local analog, and should treat the conventions below as binding for every later IaC/PowerShell artifact in the project.

## File Classification

Derived from CONTEXT.md decisions (D-01..D-06), RESEARCH.md "Recommended Project Structure", and the Wave 0 gaps. Paths use the research-recommended `infra/` root (planner may substitute `tools/env/` but should pick one and keep it consistent).

| New File | Role | Data Flow | Closest Analog | Match Quality |
|----------|------|-----------|----------------|---------------|
| `.gitignore` | config | n/a | none (greenfield) | no analog — establishes baseline |
| `infra/main.bicep` | config (IaC template) | declarative provision | none | no analog — pattern source = MS `vm-simple-windows` quickstart |
| `infra/modules/network.bicep` (optional split) | config (IaC module) | declarative provision | none | no analog — pattern source = MS quickstart NSG/VNet |
| `infra/modules/vm.bicep` (optional split) | config (IaC module) | declarative provision | none | no analog — pattern source = MS quickstart VM + CSE |
| `infra/modules/autodestroy.bicep` (optional split) | config (IaC module) | event-driven (scheduled) | none | no analog — pattern source = MS Automation runbook + managed identity |
| `infra/scripts/Configure-Target.ps1` | config (in-guest provisioning) | transform / batch (idempotent host config) | none | no analog — pattern source = MS `vm-winrm-windows` + default-hive technique |
| `infra/scripts/Delete-ResourceGroup.ps1` | utility (runbook body) | event-driven (scheduled delete) | none | no analog — pattern source = MS "runbook with managed identity" |
| `infra/manage-env.ps1` | utility (orchestration driver) | request-response (CLI up/down) | none | no analog — pattern source = synthesized D-05/D-06 + `az deployment` |
| `infra/tests/Validate-Target.ps1` | test (post-deploy assertions) | request-response (WinRM probes) | none | no analog — Pester / `Test-NetConnection` over WinRM |
| `infra/tests/ManageEnv.Tests.ps1` | test (unit, mocked `az`) | request-response | none | no analog — Pester `Mock` of `az` |
| `infra/README.md` | config (docs) | n/a | none | no analog — document `up`/`down` usage |

> Note: `infra/modules/*` are optional per RESEARCH.md ("split if main.bicep grows large"). Default to a single `main.bicep` unless the planner judges it too large; if split, the module excerpts below apply.

## Pattern Assignments

Each file below lists the **role/data-flow**, the **external pattern source** (no local analog exists), and the **concrete excerpt** the planner should copy/adapt. Line citations point at `01-RESEARCH.md`.

---

### `.gitignore` (config) — DO FIRST

**Analog:** none. This is Pitfall 7 (RESEARCH.md L294-296) — the repo has no `.gitignore` and `.secrets/connection.json` (D-06) could be committed.

**Pattern to copy (minimum content):**
```gitignore
.secrets/
.env
```

**Convention established:** All generated credentials/connection artifacts live under `.secrets/` (gitignored). No secret ever enters Bicep parameter defaults or a committed `*.parameters.json`. This must be created and committed **before** any `up` is ever run.

---

### `infra/main.bicep` (config, declarative provision)

**Analog:** none. Pattern source = MS `vm-simple-windows` quickstart (RESEARCH.md Sources L486) + the verified resource/API table (L65-77).

**Image reference — copy exactly** (RESEARCH.md L80-87):
```bicep
imageReference: {
  publisher: 'MicrosoftWindowsServer'
  offer: 'WindowsServer'
  sku: '2022-datacenter-g2'   // Gen2 Desktop Experience. NOT '-core', NOT '-azure-edition'
  version: 'latest'
}
```

**Secure password param + sizing params** (D-02/D-06):
```bicep
@secure()
param adminPassword string          // passed from manage-env.ps1; never defaulted, never logged
param allowedSourceIp string        // dev public IP, detected at up-time (D-05)
param vmSize string = 'Standard_B2ms'   // param so D2s_v5 switch is one flag (Pitfall 6, L289-292)
```

**Pinned API versions to use** (RESEARCH.md L65-77):
- `Microsoft.Compute/virtualMachines@2024-07-01`
- `Microsoft.Compute/virtualMachines/extensions@2024-07-01`
- `Microsoft.Network/publicIPAddresses@2024-05-01` (SKU `Standard`, `Static`)
- `Microsoft.Network/networkSecurityGroups@2024-05-01`
- `Microsoft.Network/virtualNetworks@2024-05-01`
- `Microsoft.Network/networkInterfaces@2024-05-01`
- `Microsoft.Automation/automationAccounts@2023-11-01` (+ `/runbooks`, `/schedules`, `/jobSchedules`)
- `Microsoft.Authorization/roleAssignments@2022-04-01`

**Convention established:** One `main.bicep` deploys network + VM + one CustomScriptExtension + Automation account. Pin every `@apiVersion` explicitly (no "latest" API). All resources land in a single RG so teardown cascades (Pitfall 5).

---

### NSG rule (in `main.bicep` or `modules/network.bicep`) (config, declarative provision)

**Analog:** none. Pattern source = RESEARCH.md "NSG rule scoped to the dev IP" (L300-328) — copy directly:
```bicep
@description('Developer public IP, detected at up-time by manage-env.ps1')
param allowedSourceIp string

resource nsg 'Microsoft.Network/networkSecurityGroups@2024-05-01' = {
  name: 'rdpilot-nsg'
  location: location
  properties: {
    securityRules: [
      { name: 'allow-rdp', properties: {
          priority: 1000, access: 'Allow', direction: 'Inbound', protocol: 'Tcp'
          sourcePortRange: '*', destinationPortRange: '3389'
          sourceAddressPrefix: allowedSourceIp, destinationAddressPrefix: '*' } }
      { name: 'allow-winrm-https', properties: {
          priority: 1010, access: 'Allow', direction: 'Inbound', protocol: 'Tcp'
          sourcePortRange: '*', destinationPortRange: '5986'
          sourceAddressPrefix: allowedSourceIp, destinationAddressPrefix: '*' } }
    ]
  }
}
```
**Convention established:** Never `0.0.0.0/0` (anti-pattern, L240). Only 3389 + 5986, only from `allowedSourceIp`.

---

### CustomScriptExtension (in `main.bicep` or `modules/vm.bicep`) (config, declarative provision)

**Analog:** none. Pattern source = RESEARCH.md "CustomScriptExtension invoking the config script" (L330-350) — copy directly:
```bicep
resource config 'Microsoft.Compute/virtualMachines/extensions@2024-07-01' = {
  parent: vm
  name: 'Configure-Target'
  location: location
  properties: {
    publisher: 'Microsoft.Compute'
    type: 'CustomScriptExtension'
    typeHandlerVersion: '1.10'
    autoUpgradeMinorVersion: true
    settings: { fileUris: [ scriptUri ] }
    protectedSettings: {
      commandToExecute: 'powershell -ExecutionPolicy Unrestricted -File Configure-Target.ps1'
    }
  }
}
```
**Convention established:** Exactly ONE CSE running ONE idempotent script does all in-guest hardening (Pattern 1, L180-183). Do NOT use `deploymentScripts` (runs in a container, can't touch the guest — L93). No reboots inside the extension (anti-pattern L242).

---

### `infra/scripts/Configure-Target.ps1` (config, transform/batch — in-guest)

**Analog:** none. Pattern source = RESEARCH.md Pattern 1 (L184-213), verified against MS `vm-winrm-windows` + default-user-hive docs.

**Core idempotent pattern — copy structure** (every change guarded; this is the load-bearing technique):
```powershell
# WinRM HTTPS listener (self-signed) — guarded
if (-not (Get-ChildItem WSMan:\localhost\Listener | Where-Object { $_.Keys -match 'Transport=HTTPS' })) {
  $cert = New-SelfSignedCertificate -DnsName $env:COMPUTERNAME -CertStoreLocation Cert:\LocalMachine\My
  New-Item -Path WSMan:\localhost\Listener -Transport HTTPS -Address * `
    -CertificateThumbPrint $cert.Thumbprint -Force
}
New-NetFirewallRule -DisplayName 'WinRM HTTPS' -Direction Inbound -LocalPort 5986 `
  -Protocol TCP -Action Allow -ErrorAction SilentlyContinue

# Per-user DPI + SuppressWhenMinimized into the DEFAULT user hive (Pitfall 1 — load/edit/unload)
$hive = 'HKLM\DEFAULT_USER'
reg load $hive C:\Users\Default\NTUSER.DAT | Out-Null
reg add "$hive\Control Panel\Desktop" /v LogPixels      /t REG_DWORD /d 96 /f | Out-Null
reg add "$hive\Control Panel\Desktop" /v Win8DpiScaling /t REG_DWORD /d 1  /f | Out-Null
reg add "$hive\Software\Microsoft\Terminal Server Client" /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
[gc]::Collect(); reg unload $hive | Out-Null   # MUST release handles before unload

# Machine-wide SuppressWhenMinimized (HKLM + Wow6432Node)
reg add "HKLM\Software\Microsoft\Terminal Server Client" /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
reg add "HKLM\Software\Wow6432Node\Microsoft\Terminal Server Client" /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null

# 7-Zip silent install (AFTER SHA-256 verification of $installer)
if (-not (Test-Path 'C:\Program Files\7-Zip\7zFM.exe')) {
  Start-Process -FilePath $installer -ArgumentList '/S' -Wait
}
```
**Conventions established:**
- **Idempotency is mandatory** — CSE may re-run on reboot (L183). Guard every mutation with `if (-not ...)` / `/f` overwrite semantics.
- **Per-user registry → default-user hive, never HKCU** (Pitfall 1, L259-264; anti-pattern L237). The automation user has no profile at provision time.
- **7-Zip: pin version URL + verify SHA-256 before install** (Package Audit L113-117; anti-pattern L238). Do not download "latest".
- No in-script reboot.

---

### `infra/scripts/Delete-ResourceGroup.ps1` (utility, event-driven — runbook body)

**Analog:** none. Pattern source = RESEARCH.md "Auto-destroy runbook body" (L352-359), MS "runbook with managed identity":
```powershell
param([string]$ResourceGroupName)
Disable-AzContextAutosave -Scope Process | Out-Null
Connect-AzAccount -Identity                       # system-assigned managed identity (NOT RunAs — retired)
Remove-AzResourceGroup -Name $ResourceGroupName -Force
```
**Conventions established:**
- **Managed identity only** — RunAs accounts are retired (State of the Art, L366/L371).
- Identity needs an explicit `roleAssignments` grant (Contributor over the target RG) or it fails `AuthorizationFailed` (Pitfall 4, L278-282).
- **Self-delete topology is an OPEN QUESTION** (RESEARCH.md Open Q2, L394-397; Assumption A5). Planner MUST resolve with the user: Automation Account inside the disposable RG (messy self-delete, job status may not report) vs a separate persistent management RG (cleaner). Do not silently pick one.

---

### `infra/manage-env.ps1` (utility, request-response — orchestration driver, ENV-02)

**Analog:** none. Pattern source = RESEARCH.md Pattern 2 (L215-234) — copy structure:
```powershell
param([ValidateSet('up','down')][string]$Action, [string]$Rg='rdpilot-test', [string]$Location='westeurope')

if ($Action -eq 'up') {
  $devIp = (Invoke-RestMethod 'https://api.ipify.org?format=json').ip          # public-IP detection (D-05)
  $pwd   = ...cryptographic RNG...                                            # strong random >=24 chars (D-06)
  az group create -n $Rg -l $Location | Out-Null
  az deployment group create -g $Rg --template-file infra/main.bicep `
    --parameters adminPassword=$pwd allowedSourceIp=$devIp | Out-Null
  $ip = az network public-ip list -g $Rg --query "[0].ipAddress" -o tsv
  New-Item -ItemType Directory -Force .secrets | Out-Null
  @{ host=$ip; user='rdpadmin'; password=$pwd; winrmPort=5986; rdpPort=3389 } |
    ConvertTo-Json | Set-Content .secrets/connection.json                      # gitignored (D-06)
}
elseif ($Action -eq 'down') { az group delete -n $Rg --yes --no-wait }
```
**Conventions established:**
- **Password generation: use a cryptographic RNG**, not the hand-rolled char-shuffler shown in the draft. RESEARCH.md "Don't Hand-Roll" (L251) mandates `[System.Web.Security.Membership]::GeneratePassword()` or a crypto RNG, >=24 chars, meeting Azure's 12-char minimum.
- **Never echo `$pwd`** to console/logs (secrets note L234; project rule `secrets-handling`). `@secure()` param keeps it out of deployment history.
- **Add a pre-flight `az account show` check** before `up` (Environment Availability L419; Assumption A7) — fail fast with a clear auth error.
- **Provide `-AllowedSourceIp` override param** as fallback if ipify lookup fails (L421/L501).
- **`-Action` validated via `ValidateSet`**; validate the IP param (ASVS V5, L468).
- Teardown is `az group delete` — Bicep has no destroy (Pitfall 5).

---

### `infra/tests/Validate-Target.ps1` (test, request-response — post-deploy assertions)

**Analog:** none. Pattern source = RESEARCH.md "Phase Requirements → Test Map" (L436-445). One assertion per ENV-01 criterion, run over WinRM after `up`:
```powershell
Test-NetConnection <ip> -Port 3389        # RDP reachable
Test-NetConnection <ip> -Port 5986        # WinRM reachable
# Over WinRM (New-PSSessionOption -SkipCACheck -SkipCNCheck + -UseSSL):
#   (Get-ItemProperty '...WinStations\RDP-Tcp').UserAuthentication -eq 1   # NLA enforced (Pitfall 2)
#   default-hive LogPixels=96 / Win8DpiScaling=1                          # 96 DPI
#   HKLM + default-hive RemoteDesktop_SuppressWhenMinimized -eq 2
#   Test-Path 'C:\Program Files\7-Zip\7zFM.exe'                           # 7-Zip installed
```
**Conventions established:**
- WinRM clients use `New-PSSessionOption -SkipCACheck -SkipCNCheck` + `-UseSSL` (self-signed cert, Pitfall 3, L272-275).
- NLA is a **verification**, not a configuration step (Pitfall 2, L266-269).
- This is the phase-gate suite: `up` → `Validate-Target.ps1` (all green) → `down` (L450).

---

### `infra/tests/ManageEnv.Tests.ps1` (test, unit — mocked `az`)

**Analog:** none. Pattern source = Pester with `Mock` of `az` (RESEARCH.md Wave 0, L455). Cover: IP detection, password generation entropy/length, param wiring — all without Azure cost.
**Conventions established:**
- **Pester** is the test framework for `.ps1` logic (Validation Architecture, L429-433).
- Pin Pester version if installing (`Install-Module Pester -Scope CurrentUser`, L456).
- Per-commit gate: `bicep build infra/main.bicep` + `Invoke-Pester infra/tests` (no Azure cost, L448).

---

## Shared Patterns

These cross-cut multiple files. The planner should apply them to every relevant plan.

### Idempotency (in-guest config)
**Source:** RESEARCH.md Pattern 1 (L180-213), CSE re-run blog (L489).
**Apply to:** `Configure-Target.ps1` — and any future in-guest script.
**Rule:** Guard every mutation; assume the script runs more than once; never reboot mid-script.

### Secrets handling
**Source:** D-06; RESEARCH.md L234; project rule `secrets-handling.md`.
**Apply to:** `.gitignore`, `manage-env.ps1`, `main.bicep`.
**Rule:** Generated password lives only in memory + `.secrets/connection.json` (gitignored). `@secure()` Bicep param. Never echo, never log, never default, never commit. Create `.gitignore` before first run.

### Least-privilege RBAC
**Source:** Pitfall 4 (L278-282); ASVS V4 (L467).
**Apply to:** `autodestroy.bicep` / runbook role assignment.
**Rule:** Grant the runbook identity exactly Contributor over the target RG (Owner only if locks/role-assignments require it). Never subscription-Owner "to be safe" (anti-pattern L239).

### Pinned versions everywhere
**Source:** RESEARCH.md Standard Stack (L65-88), Package Audit (L113-117).
**Apply to:** all `.bicep` (API versions), `Configure-Target.ps1` (7-Zip URL + SHA-256), Pester install.
**Rule:** No "latest" API versions, no "latest" binary downloads. Pin + verify.

### NSG least-exposure
**Source:** D-05; L300-328; ASVS V14 (L471).
**Apply to:** NSG resource.
**Rule:** Only 3389 + 5986, only from `allowedSourceIp`. Never `0.0.0.0/0`.

### Single-RG teardown
**Source:** Pitfall 5 (L284-287); Don't Hand-Roll (L248).
**Apply to:** `main.bicep` (everything in one RG), `manage-env.ps1 down`, runbook.
**Rule:** All resources in one RG so `az group delete` / `Remove-AzResourceGroup -Force` cascades. No per-resource delete loops.

## No Analog Found

ALL files in this phase have no in-codebase analog — this is the greenfield first phase. The planner should use the **external pattern sources cited per file above** (all first-party MS samples / synthesized-from-decisions patterns in `01-RESEARCH.md`) instead of a local analog.

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| `.gitignore` | config | n/a | Repo has none (Pitfall 7) |
| `infra/main.bicep` | config | declarative | No IaC exists yet |
| `infra/modules/*.bicep` | config | declarative | No IaC exists yet |
| `infra/scripts/Configure-Target.ps1` | config | transform/batch | No `.ps1` exists yet |
| `infra/scripts/Delete-ResourceGroup.ps1` | utility | event-driven | No `.ps1` exists yet |
| `infra/manage-env.ps1` | utility | request-response | No `.ps1` exists yet |
| `infra/tests/Validate-Target.ps1` | test | request-response | No tests exist yet |
| `infra/tests/ManageEnv.Tests.ps1` | test | request-response | No tests exist yet |
| `infra/README.md` | config | n/a | No docs structure yet |

## Open Decisions the Planner Must Resolve (carried from RESEARCH.md)

These block clean pattern application — surface to user during planning:
1. **Auto-destroy topology** (Open Q2 / A5) — self-deleting RG vs separate management RG. Drives RBAC scope and runbook placement.
2. **Azure region + subscription** (Open Q3 / A6) — propose West Europe (EU-first); confirm subscription before first `up`.
3. **`Win8DpiScaling` semantics** (Open Q1 / A4) — set both `LogPixels=96` + `Win8DpiScaling=1`, verify effective DPI post-login.
4. **Exact 7-Zip version URL + SHA-256** (A3) — confirm against 7-zip.org at plan time; add a `checkpoint:human-verify` or automated hash check.
5. **`infra/` vs `tools/env/` root** — pick one; this map assumes `infra/`.

## Metadata

**Analog search scope:** entire repo (`git ls-files`, `**/*.ps1`, `**/*.bicep`, `**/.gitignore`).
**Files scanned:** 15 tracked files — all under `.planning/` plus `CLAUDE.md`. Zero source/IaC files.
**Pattern extraction date:** 2026-06-04
