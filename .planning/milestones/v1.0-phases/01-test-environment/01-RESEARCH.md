# Phase 1: Test Environment - Research

**Researched:** 2026-06-04
**Domain:** Azure IaC (Bicep + PowerShell) — disposable Windows Server 2022 RDP/automation test target with scheduled auto-destroy
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Use **Windows Server 2022 Datacenter, Desktop Experience**. Frictionless pay-as-you-go Azure licensing; real `explorer.exe` desktop and full UIA. Accepted caveat: Server Manager autostarts; no Store/UWP apps.
- **D-02:** VM size **Standard_B2ms** (2 vCPU / 8 GB, burstable). Cheapest size that comfortably runs WS2022 Desktop + RDP + a GUI app for short disposable sessions. If burst throttling causes screenshot/framebuffer latency noise in later phases, revisit Standard_D2s_v5.
- **D-03:** Sample remote-only program = **7-Zip File Manager** (silent install, FOSS, LGPL-compatible). Non-trivial UIA tree (menus, toolbar, listview, modal dialogs).
- **D-04:** **Cloud-native scheduled safeguard that deletes the whole resource group** — an Azure Automation runbook (or scheduled Logic App) provisioned as part of the environment, running `az group delete` on a TTL/schedule. Must fire even if the dev machine is off. Strongest cost guarantee.
- **D-05:** **Public IP on the VM + NSG allowlist scoped to the developer's current public IP**, detected at `up` time by the `.ps1`. Ports opened: **RDP 3389** and **WinRM (5986 HTTPS preferred)**, restricted to that single IP only. Rejected: Azure Bastion and fully-open 0.0.0.0/0. Caveat: a changing dev IP mid-session requires re-running the allowlist step.
- **D-06:** **Credentials generated at `up` time, written to a gitignored local file** (e.g. `.env` or under `.secrets/`). The `.ps1` generates a strong random admin/automation password at provision time. The chosen secrets path MUST be in `.gitignore` from the start. Rejected: Azure Key Vault and prompt-each-run.

### Claude's Discretion
- **Azure region & subscription** — not pinned. Planner/researcher to choose (or confirm with user) a suitable region and target subscription.
- **WinRM transport** — HTTP 5985 vs HTTPS 5986, auth method, self-signed cert provisioning. Lean toward HTTPS/5986.
- **96 DPI + `RemoteDesktop_SuppressWhenMinimized=2` enforcement mechanism** — registry keys vs first-login script vs GPO; must apply to the automation user. The REQUIREMENT that these be set is locked by ENV-01; only the mechanism is open.

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| ENV-01 | Bicep template provisions an Azure Windows VM: RDP/NLA reachable, WinRM enabled, `RemoteDesktop_SuppressWhenMinimized=2`, 96 DPI (100%) for the automation user, sample remote-only program installed | Standard Stack (Bicep VM template), WinRM HTTPS via CustomScriptExtension, DPI/SuppressWhenMinimized via **default-user hive** edit (see Pitfall 1), 7-Zip silent install. Mechanism choices documented. |
| ENV-02 | A PowerShell `.ps1` brings the environment up and tears it down | `up`/`down` script pattern: `az group create` + `az deployment group create`, public-IP detection, secrets file generation, `az group delete` on `down`. |
| ENV-03 | A scheduled auto-destroy safeguard automatically tears down the VM / resource group | Azure Automation Account + system-assigned managed identity + PowerShell runbook + schedule, all in the Bicep. Self-deletion subtlety documented (see Pitfall 4). |
</phase_requirements>

## Summary

Phase 1 is **pure Azure IaC** — no Rust. It produces a Bicep template (or template set) plus a PowerShell `up`/`down` wrapper that provisions a single disposable Windows Server 2022 Datacenter (Desktop Experience) VM, hardens it for RDP automation, installs 7-Zip as the test application, and wires in a cloud-native scheduled auto-destroy of the entire resource group. Every later phase consumes the gitignored secrets/connection file this phase writes.

The Microsoft `vm-simple-windows` quickstart is the canonical skeleton (verified: `Microsoft.Compute/virtualMachines`, `publicIPAddresses`, `networkSecurityGroups`, `virtualNetworks`, `networkInterfaces`; `imageReference` publisher `MicrosoftWindowsServer` / offer `WindowsServer` / sku `2022-datacenter-azure-edition` — **note D-01 wants the non-Azure-edition `2022-datacenter-g2` SKU**, see Standard Stack). NLA is the default for Azure Windows VMs (no action needed; verification needed). The hard parts are: (1) per-user registry settings (96 DPI, optionally SuppressWhenMinimized) that must be applied to the **default user hive** because the automation user profile does not exist at provision time; (2) WinRM HTTPS listener configuration via a CustomScriptExtension self-signed cert; and (3) an auto-destroy runbook whose managed identity must have rights to delete the RG that **contains the runbook itself**.

**Primary recommendation:** Build one `main.bicep` that deploys network + VM + a single post-deploy `CustomScriptExtension` running an idempotent `Configure-Target.ps1` (WinRM HTTPS, default-user-hive DPI + SuppressWhenMinimized, 7-Zip install), plus an Azure Automation Account with a managed-identity PowerShell runbook on a daily TTL schedule that runs `Remove-AzResourceGroup -Force`. Wrap deploy/teardown in `manage-env.ps1 -Action up|down`, which detects the dev public IP, generates the admin password, deploys, and writes `.secrets/connection.json`. Add `.gitignore` (the repo has none yet) as the very first task.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| VM + network provisioning | IaC (Bicep, ARM control plane) | — | Declarative infra; Bicep is the locked tool |
| Public-IP detection + NSG scoping | Local orchestration (`.ps1`) | IaC (NSG resource) | IP is dynamic, known only at `up` time; `.ps1` passes it as a Bicep param |
| Credential generation + secrets file | Local orchestration (`.ps1`) | — | D-06: generated locally, written to gitignored file; never in Bicep state |
| In-guest hardening (WinRM, DPI, SuppressWhenMinimized, 7-Zip) | Guest config (CustomScriptExtension) | — | Per-machine/per-user OS state; runs inside the VM post-boot |
| Scheduled auto-destroy | Cloud control plane (Automation runbook + managed identity) | — | D-04: must fire independent of the dev machine |
| Teardown on demand | Local orchestration (`.ps1 down`) | Cloud control plane | `az group delete` from the wrapper; runbook is the safety net |

## Standard Stack

### Core
| Tool | Version | Purpose | Why Standard |
|------|---------|---------|--------------|
| Azure CLI (`az`) | 2.86.0 (installed, verified) | Deploy/teardown driver, public-IP-free control plane | Locked by project; `az deployment group create`, `az group delete` |
| Bicep CLI | 0.43.8 (installed, verified) | IaC authoring/compile | Locked by project (Bicep + .ps1) |
| PowerShell | 7.5.4 (installed, verified) | `up`/`down` wrapper + in-guest config scripts | Locked by project; cross-version with Windows PowerShell 5.1 in-guest |

### Supporting (Azure resource types — API versions verified against MS quickstart, current 2026-01)
| Resource | API Version | Purpose | Notes |
|----------|-------------|---------|-------|
| `Microsoft.Compute/virtualMachines` | `2024-07-01` (use current; quickstart shows `2022-03-01`) | The VM | `osProfile.windowsConfiguration` for WinRM, `imageReference` for WS2022 |
| `Microsoft.Compute/virtualMachines/extensions` | `2024-07-01` | CustomScriptExtension post-deploy config | publisher `Microsoft.Compute`, type `CustomScriptExtension`, handler `1.10` |
| `Microsoft.Network/publicIPAddresses` | `2024-05-01` | Public IP (D-05) | SKU `Standard`, `Static` allocation |
| `Microsoft.Network/networkSecurityGroups` | `2024-05-01` | RDP 3389 + WinRM 5986 allowlist | `sourceAddressPrefix` = dev IP param |
| `Microsoft.Network/virtualNetworks` | `2024-05-01` | VNet + subnet | NSG associated at subnet or NIC |
| `Microsoft.Network/networkInterfaces` | `2024-05-01` | NIC | binds public IP + subnet |
| `Microsoft.Automation/automationAccounts` | `2023-11-01` | Auto-destroy host (D-04) | system-assigned identity |
| `Microsoft.Automation/automationAccounts/runbooks` | `2023-11-01` | The delete-RG PowerShell runbook | type `PowerShell` |
| `Microsoft.Automation/automationAccounts/schedules` + `jobSchedules` | `2023-11-01` | TTL schedule | daily recurrence |
| `Microsoft.Authorization/roleAssignments` | `2022-04-01` | Grant runbook identity rights to delete the RG | scope subtlety — see Pitfall 4 |

**Image reference for D-01 (WS2022 Datacenter, Desktop Experience):**
```bicep
imageReference: {
  publisher: 'MicrosoftWindowsServer'
  offer: 'WindowsServer'
  sku: '2022-datacenter-g2'   // Gen2; full Desktop Experience. NOT '...-core' (no desktop), NOT '...-azure-edition' (Hotpatch/Azure-tuned — D-01 specifies plain Datacenter)
  version: 'latest'
}
```
`[VERIFIED: MS quickstart allowed-values list]` — `2022-datacenter-g2` is the Gen2 Desktop image; `*-core` variants are Server Core (no GUI, disqualified by D-01); `*-azure-edition` is a distinct SKU. **[ASSUMED]** that `2022-datacenter-g2` (rather than `2022-datacenter` Gen1) is the right pick — Gen2 is the modern default and required for Trusted Launch; confirm region availability at deploy time.

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| CustomScriptExtension | Bicep `deploymentScripts` | `deploymentScripts` run in a *container*, not in-guest — cannot touch the VM's registry/WinRM. Wrong tool for in-guest config. |
| CustomScriptExtension | `runCommands` (Run Command v2) | Run Command **does not re-run on reboot** (better idempotency) but is newer and slightly more verbose in Bicep. Viable alternative; CSE is the better-documented path with the official WinRM sample. |
| Automation runbook (D-04 locked) | VM auto-shutdown | Stop-only — does NOT delete the RG. Disqualified by D-04 (criterion is full RG removal). |
| Automation runbook (D-04 locked) | Logic App schedule | Equivalent; runbook chosen — richer PowerShell + managed identity story. Logic App acceptable per D-04 ("or scheduled Logic App"). |
| Self-signed WinRM cert via CSE | Key Vault cert + VM `secrets` block | Key Vault explicitly rejected (D-06 rationale). Self-signed is correct for a throwaway box; client uses `-SkipCACheck`. |

**Installation (driver tooling — already present on dev machine):**
```powershell
# Verified installed: az 2.86.0, bicep 0.43.8, pwsh 7.5.4. No installs needed.
az version ; az bicep version ; pwsh --version
```

## Package Legitimacy Audit

> This phase installs **no language packages** (no npm/PyPI/crates). External artifacts are: Azure resource providers (first-party Microsoft control plane) and one downloaded application binary (7-Zip).

| Artifact | Source | Disposition |
|----------|--------|-------------|
| Azure resource providers (`Microsoft.*`) | Azure control plane (first-party) | Approved — not a package registry |
| Azure CLI / Bicep / PowerShell | Pre-installed, verified versions | Approved |
| 7-Zip installer | **Must pin to official `https://www.7-zip.org/` download + verify** | See note below |

**7-Zip download integrity [WARNING]:** The CustomScriptExtension will download the 7-Zip installer at provision time. Pin a specific version URL (e.g. `https://www.7-zip.org/a/7z<version>-x64.exe`) and **verify the SHA-256** against the published hash before silent-installing, or vendor the installer into a storage account / repo. Downloading "latest" from a redirect at runtime is a supply-chain risk on a box that later phases trust. `[ASSUMED]` exact current 7-Zip version/URL — confirm at plan time (7-zip.org publishes per-version `.exe` URLs and a hashes page).

slopcheck was not run — there are no registry packages in scope, so the gate is N/A for this phase. The one downloaded binary (7-Zip) is gated by the SHA-256 verification requirement above; the planner SHOULD add a `checkpoint:human-verify` or an automated hash-check task for it.

## Architecture Patterns

### System Architecture Diagram

```
                        DEV MACHINE (local)
  ┌──────────────────────────────────────────────────────────────┐
  │  manage-env.ps1  -Action up | down                            │
  │    1. detect dev public IP  ──(GET)──►  api.ipify.org (HTTPS) │
  │    2. generate random admin password (in-memory)             │
  │    3. az group create                                        │
  │    4. az deployment group create  ──► (params: devIp, pwd)   │
  │    5. write .secrets/connection.json  (gitignored)          │
  └───────────────┬──────────────────────────────────────────────┘
                  │ ARM control plane
                  ▼
        ┌───────────────────── Resource Group (TTL-tagged) ─────────────────────┐
        │                                                                        │
        │   main.bicep deploys:                                                  │
        │                                                                        │
        │   VNet ── Subnet ──┬── NSG (allow 3389, 5986  FROM devIp ONLY)         │
        │                    │                                                   │
        │              NIC ──┴── Public IP (Standard, Static)  ◄── RDP/WinRM in  │
        │                    │                                                   │
        │              ┌─────▼──────────────────────────────────┐               │
        │              │  WS2022 VM (Standard_B2ms)              │               │
        │              │   osProfile.windowsConfiguration:      │               │
        │              │     - WinRM HTTPS listener (cert)      │               │
        │              │     - NLA on (default)                 │               │
        │              │   CustomScriptExtension → Configure-   │               │
        │              │   Target.ps1 (idempotent):            │               │
        │              │     • WinRM HTTPS self-signed cert     │               │
        │              │     • load Default NTUSER.DAT hive:    │               │
        │              │         LogPixels=96, Win8DpiScaling=1 │               │
        │              │     • RemoteDesktop_SuppressWhenMin=2  │               │
        │              │     • install 7-Zip (verified SHA)     │               │
        │              └────────────────────────────────────────┘              │
        │                                                                        │
        │   Automation Account (system-assigned identity)                       │
        │     └─ Runbook: Remove-AzResourceGroup -Force  ◄── daily schedule     │
        │          identity has Contributor/Owner over the RG (Pitfall 4)       │
        └────────────────────────────────────────────────────────────────────────┘
```

### Recommended Project Structure
```
infra/                          # or tools/env/ — planner's choice
├── main.bicep                  # network + VM + extension + automation account
├── modules/                    # (optional) split if main.bicep grows large
│   ├── network.bicep
│   ├── vm.bicep
│   └── autodestroy.bicep
├── scripts/
│   ├── Configure-Target.ps1    # in-guest: runs via CustomScriptExtension
│   └── Delete-ResourceGroup.ps1# runbook body (or inline in Bicep)
├── manage-env.ps1              # the up/down driver (ENV-02)
└── README.md                   # how to run up/down
.gitignore                      # MUST exist — add .secrets/ and .env
.secrets/                       # gitignored; connection.json written here at `up`
```

### Pattern 1: One idempotent in-guest config script via CustomScriptExtension
**What:** A single `Configure-Target.ps1` invoked by one `CustomScriptExtension` resource does all in-guest hardening.
**When to use:** Always for this phase — keeps guest config in one auditable, re-runnable place.
**Why:** CSE may re-run on reboot (verified behavior), so the script **must be idempotent** — guard every change (`if not already set`). Avoid in-script reboots.
```powershell
# Source: pattern synthesized from MS Custom Script Extension docs + default-user-hive technique
# Configure-Target.ps1 (excerpt) — idempotent guards throughout

# --- WinRM HTTPS listener (self-signed) ---
if (-not (Get-ChildItem WSMan:\localhost\Listener | Where-Object { $_.Keys -match 'Transport=HTTPS' })) {
  $cert = New-SelfSignedCertificate -DnsName $env:COMPUTERNAME -CertStoreLocation Cert:\LocalMachine\My
  New-Item -Path WSMan:\localhost\Listener -Transport HTTPS -Address * `
    -CertificateThumbPrint $cert.Thumbprint -Force
}
New-NetFirewallRule -DisplayName 'WinRM HTTPS' -Direction Inbound -LocalPort 5986 `
  -Protocol TCP -Action Allow -ErrorAction SilentlyContinue

# --- Per-user DPI + SuppressWhenMinimized into the DEFAULT user hive (applies to future automation user) ---
$hive = 'HKLM\DEFAULT_USER'
reg load $hive C:\Users\Default\NTUSER.DAT | Out-Null
reg add "$hive\Control Panel\Desktop" /v LogPixels      /t REG_DWORD /d 96 /f | Out-Null
reg add "$hive\Control Panel\Desktop" /v Win8DpiScaling /t REG_DWORD /d 1  /f | Out-Null
reg add "$hive\Software\Microsoft\Terminal Server Client" /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
[gc]::Collect(); reg unload $hive | Out-Null   # must release handles before unload

# --- Machine-wide SuppressWhenMinimized (covers the RDP CLIENT side too; harmless on server) ---
reg add "HKLM\Software\Microsoft\Terminal Server Client" /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null
reg add "HKLM\Software\Wow6432Node\Microsoft\Terminal Server Client" /v RemoteDesktop_SuppressWhenMinimized /t REG_DWORD /d 2 /f | Out-Null

# --- 7-Zip silent install (after SHA-256 verification of $installer) ---
if (-not (Test-Path 'C:\Program Files\7-Zip\7zFM.exe')) {
  Start-Process -FilePath $installer -ArgumentList '/S' -Wait
}
```

### Pattern 2: `up`/`down` driver detects IP, generates secret, writes connection file
**What:** `manage-env.ps1` is the single human entry point (ENV-02).
```powershell
# Source: synthesized from D-05/D-06 + az deployment patterns
param([ValidateSet('up','down')][string]$Action, [string]$Rg='rdpilot-test', [string]$Location='westeurope')

if ($Action -eq 'up') {
  $devIp = (Invoke-RestMethod 'https://api.ipify.org?format=json').ip          # public-IP detection (D-05)
  $pwd   = -join ((48..57)+(65..90)+(97..122)+(33,35,37,42) | Get-Random -Count 24 | % {[char]$_})  # strong random (D-06)
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
> **Secrets-handling note (project rule):** the generated password lives only in `$pwd` and the gitignored `.secrets/connection.json`. Do **not** echo it to the console or into logs. `az deployment` receives it as a `@secure()` param so it is not stored in deployment history.

### Anti-Patterns to Avoid
- **Setting DPI/SuppressWhenMinimized in HKCU during provisioning:** the automation user has no profile yet; HKCU edits hit the provisioning context, not the future user. Use the default-user hive (Pitfall 1).
- **Downloading 7-Zip "latest" with no hash check:** supply-chain risk on a trusted target. Pin + verify SHA-256.
- **Granting the runbook identity subscription-Owner "to be safe":** over-privileged. Scope to exactly what's needed to delete the RG (Pitfall 4).
- **Opening 3389/5986 to `0.0.0.0/0`:** explicitly rejected (D-05). Scope to the detected dev IP.
- **Putting the admin password in a Bicep parameter default or a committed `.parameters.json`:** must be generated at `up` time and gitignored (D-06).
- **Reboot inside the CSE script:** breaks idempotency/resume; let extensions complete without rebooting.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Resource teardown | Custom per-resource delete loop | `Remove-AzResourceGroup -Force` / `az group delete` | RG delete cascades; everything provisioned lives in one RG |
| Scheduled cost safeguard | Local cron/Task Scheduler | Azure Automation runbook + schedule (D-04) | Must fire when dev machine is off |
| WinRM HTTPS listener | Hand-written WSMan SOAP config | `New-SelfSignedCertificate` + `WSMan:\localhost\Listener` (MS sample `vm-winrm-windows`) | Official, tested pattern |
| Random strong password | Custom char shuffler with weak entropy | `[System.Web.Security.Membership]::GeneratePassword()` or a cryptographic RNG | Avoid bias/weak generation; meet the 12-char Azure minimum |
| Public IP detection | Parse `ipconfig`/router | `api.ipify.org` (or `ifconfig.me`) over HTTPS | Returns the *public* egress IP, which is what the NSG needs |
| VM base image hardening | Build a custom image | Marketplace WS2022 + CSE config | Throwaway box; image-building is overkill |

**Key insight:** Everything in this phase has a first-party Azure or built-in PowerShell primitive. The only genuinely custom code is the *orchestration glue* (`manage-env.ps1`) and the *idempotent guards* in `Configure-Target.ps1`.

## Common Pitfalls

### Pitfall 1: Per-user settings (96 DPI, SuppressWhenMinimized) applied to the wrong hive
**What goes wrong:** `LogPixels`/`Win8DpiScaling` and the user-scope `RemoteDesktop_SuppressWhenMinimized` are **HKCU** (per-user) values. At provision time the automation user's profile doesn't exist yet, so writing HKCU writes the *SYSTEM/provisioning* user's hive — the automation user logs in later at default 96+scaling-off or inherits nothing.
**Why it happens:** Tutorials show `Set-ItemProperty HKCU:\...` which only works *as that user, after login*.
**How to avoid:** Load the **default user hive** (`C:\Users\Default\NTUSER.DAT`) with `reg load HKU\X ...`, write the values, then `[gc]::Collect()` and `reg unload` (handles must be released first or unload fails). New profiles inherit it on first login. `[VERIFIED: MS CopyProfile/default-hive docs]`
**Warning signs:** Later phases observe non-100% scaling or DPI-mismatched coordinates; minimized RDP stalls rendering despite the "set" registry value.
**Note:** 96 DPI also requires `Win8DpiScaling=1` *or* leaving it 0 (sources conflict — see Open Questions). Set both `LogPixels=96` and `Win8DpiScaling=1` to be unambiguous, and verify post-login. A sign-out/sign-in (or the first login itself) is required to apply DPI.

### Pitfall 2: NLA is on by default — verify, don't configure
**What goes wrong:** Plan adds NLA-enabling steps that are redundant, or assumes NLA is off.
**Why it happens:** NLA (`UserAuthenticationRequired=1`) is the Azure Windows VM default.
**How to avoid:** Treat NLA as a **verification** step (success criterion 2), not a configuration step. The success criterion "reachable over RDP with NLA + provisioned credentials" is satisfied by the default config plus the NSG allowing the dev IP. `[ASSUMED — verify on the box]`
**Warning signs:** Connecting works but you can't tell if NLA was actually enforced — verify with `(Get-ItemProperty 'HKLM:\System\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp').UserAuthentication -eq 1`.

### Pitfall 3: WinRM HTTPS cert/trust and firewall mismatch
**What goes wrong:** Listener created but port 5986 blocked by Windows Firewall, or client rejects the self-signed cert.
**Why it happens:** CSE configures WinRM but forgets the inbound firewall rule; clients don't `-SkipCACheck`.
**How to avoid:** In `Configure-Target.ps1` add the 5986 inbound firewall rule **and** ensure the NSG allows 5986 from the dev IP (D-05). Document that later-phase WinRM clients must use `New-PSSessionOption -SkipCACheck -SkipCNCheck` + `-UseSSL` (self-signed). `[VERIFIED: MS vm-winrm-windows sample]`
**Warning signs:** `Enter-PSSession ... -UseSSL` times out (firewall/NSG) vs. throws a cert error (trust).

### Pitfall 4: Auto-destroy runbook can't delete the RG that contains it
**What goes wrong:** The runbook calls `Remove-AzResourceGroup` but its managed identity has no rights, OR it tries to delete the RG it lives in and the job aborts mid-delete.
**Why it happens:** A fresh system-assigned identity has **zero** RBAC. And deleting the RG kills the Automation Account running the job.
**How to avoid:** (a) Add a `Microsoft.Authorization/roleAssignments` granting the runbook's identity **Contributor** (or Owner, needed if any resource has its own locks/role assignments) over the target RG. (b) For full self-deletion, scope the identity at the **subscription** level (or a parent), or have the runbook delete the RG with `-Force -AsJob` semantics understanding the job host vanishes — accept that the final job status may not report back. Simplest robust pattern: a **separate** "management" RG/Automation Account that deletes the *test* RG, so the deleter outlives its target. `[ASSUMED — confirm desired topology with user]`
**Warning signs:** Runbook job fails with `AuthorizationFailed`; or schedule fires but RG persists.

### Pitfall 5: Bicep deletes nothing — `down` and the runbook must call delete explicitly
**What goes wrong:** Expecting a Bicep redeploy or `complete` mode to remove resources.
**Why it happens:** Bicep has no Terraform-style `destroy`.
**How to avoid:** Teardown is `az group delete` (in `manage-env.ps1 down`) and `Remove-AzResourceGroup` (in the runbook). `[VERIFIED: MS delete-resource-group docs]`

### Pitfall 6: Burstable B-series throttling distorts later-phase latency
**What goes wrong:** Standard_B2ms exhausts CPU credits during sustained framebuffer/screenshot polling in Phase 2+, adding latency noise.
**Why it happens:** B-series banks credits; sustained load drains them.
**How to avoid:** Accepted per D-02 (revisit Standard_D2s_v5 if it bites). Make `vmSize` a Bicep param so switching is one flag. `[CITED: Azure B-series docs]`

### Pitfall 7: `.gitignore` does not exist yet — secrets could be committed
**What goes wrong:** The repo has **no `.gitignore`** (verified). The `.secrets/connection.json` (D-06) could be staged accidentally.
**How to avoid:** First task of the phase: create `.gitignore` with `.secrets/` and `.env`. `[VERIFIED: ls of repo root]`

## Code Examples

### NSG rule scoped to the dev IP (RDP + WinRM)
```bicep
// Source: synthesized from MS quickstart NSG + D-05
@description('Developer public IP, detected at up-time by manage-env.ps1')
param allowedSourceIp string

resource nsg 'Microsoft.Network/networkSecurityGroups@2024-05-01' = {
  name: 'rdpilot-nsg'
  location: location
  properties: {
    securityRules: [
      {
        name: 'allow-rdp', properties: {
          priority: 1000, access: 'Allow', direction: 'Inbound', protocol: 'Tcp'
          sourcePortRange: '*', destinationPortRange: '3389'
          sourceAddressPrefix: allowedSourceIp, destinationAddressPrefix: '*'
        }
      }
      {
        name: 'allow-winrm-https', properties: {
          priority: 1010, access: 'Allow', direction: 'Inbound', protocol: 'Tcp'
          sourcePortRange: '*', destinationPortRange: '5986'
          sourceAddressPrefix: allowedSourceIp, destinationAddressPrefix: '*'
        }
      }
    ]
  }
}
```

### CustomScriptExtension invoking the config script
```bicep
// Source: MS Custom Script Extension for Windows docs
resource config 'Microsoft.Compute/virtualMachines/extensions@2024-07-01' = {
  parent: vm
  name: 'Configure-Target'
  location: location
  properties: {
    publisher: 'Microsoft.Compute'
    type: 'CustomScriptExtension'
    typeHandlerVersion: '1.10'
    autoUpgradeMinorVersion: true
    settings: {
      fileUris: [ scriptUri ]   // raw URL or storage-account blob to Configure-Target.ps1
    }
    protectedSettings: {
      commandToExecute: 'powershell -ExecutionPolicy Unrestricted -File Configure-Target.ps1'
    }
  }
}
```

### Auto-destroy runbook body (PowerShell, managed identity)
```powershell
# Source: MS "PowerShell runbook with managed identity" + delete-resource-group docs
param([string]$ResourceGroupName)
Disable-AzContextAutosave -Scope Process | Out-Null
Connect-AzAccount -Identity                       # system-assigned managed identity
Remove-AzResourceGroup -Name $ResourceGroupName -Force
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| ARM JSON templates | Bicep | ~2020+ | Locked tool; concise, type-safe |
| "RunAs accounts" for Automation auth | System-assigned managed identity | RunAs retired Sep 2023 | Use managed identity — no cert rotation |
| Basic SKU public IP | Standard SKU (Static) | Basic IP retired (2025) | Use `Standard`/`Static` |
| Key Vault VM cert injection for WinRM | Self-signed via CSE (for throwaway) | n/a | D-06 rejects Key Vault; self-signed fits disposable box |

**Deprecated/outdated:**
- Automation **RunAs accounts**: retired — do not use; managed identity only.
- **Basic** public IP SKU: retired — use Standard.
- `makecert.exe` for the WinRM cert: use `New-SelfSignedCertificate`.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `2022-datacenter-g2` is the correct SKU for "WS2022 Datacenter Desktop Experience" (vs Gen1 `2022-datacenter`) | Standard Stack | Wrong image variant; Gen1 vs Gen2 / Trusted Launch mismatch. Verify region availability. |
| A2 | NLA is on by default and needs only verification | Pitfall 2 | If off, success criterion 2 fails until configured |
| A3 | Exact current 7-Zip version/download URL + SHA-256 | Package Audit | Stale URL breaks install; unverified binary is a supply-chain risk |
| A4 | Setting both `LogPixels=96` and `Win8DpiScaling=1` reliably yields 100% scaling for new users | Pitfall 1 / Open Q | Sources conflict on whether Win8DpiScaling must be 1; may need post-login verification |
| A5 | Desired auto-destroy topology (self-deleting RG vs separate management RG) | Pitfall 4 | Affects RBAC scope + whether final job reports success |
| A6 | Region = West Europe and target subscription | (discretion) | Cost/latency/availability; user must confirm subscription |
| A7 | `manage-env.ps1` may call `az` non-interactively (dev already `az login`'d) | Pattern 2 | If not logged in, `up` fails fast with a clear auth error |

## Open Questions

1. **`Win8DpiScaling` value for 96 DPI**
   - What we know: `LogPixels=96` = 100%. One source says `Win8DpiScaling=1` enables non-default LogPixels; another says you can leave it 0 when LogPixels=96.
   - What's unclear: whether `Win8DpiScaling` must be 1 specifically when targeting the *default* 96.
   - Recommendation: set both `LogPixels=96` + `Win8DpiScaling=1`, and add a post-login verification step in a later phase (or a CSE re-check) that reads effective DPI.

2. **Auto-destroy topology (Pitfall 4)**
   - What we know: a runbook can delete its own RG but the job host dies mid-delete; cleanest is a separate management scope.
   - What's unclear: does the user want the Automation Account *inside* the disposable RG (deleted with it — fine, but self-delete is messy) or in a persistent management RG?
   - Recommendation: confirm with user. Default to Automation Account inside the test RG with subscription-scoped identity if a separate RG is unwanted; document the "job status may not report" caveat.

3. **Azure region + subscription**
   - What we know: discretion item; B2ms + WS2022-g2 availability varies by region.
   - Recommendation: propose West Europe (EU-first per user profile) and confirm subscription before first `up`.

4. **`RemoteDesktop_SuppressWhenMinimized` scope semantics**
   - What we know: this is fundamentally an **RDP client** setting (keeps the *client* from suppressing render when minimized). The target VM is the *server* here.
   - What's unclear: the requirement (ENV-01) places it on the VM. Setting it on the target is harmless and correct for nested/onward RDP and consistency, but the *operative* instance for rdpilot is the IronRDP **client** (rdpilot itself), which is Phase 2.
   - Recommendation: set it on the target (satisfies ENV-01 literally) AND note for Phase 2 that the rdpilot client must apply equivalent behavior. Flag this to the user so it isn't mistaken for the only place it matters.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Azure CLI (`az`) | All deploy/teardown | ✓ | 2.86.0 | — |
| Bicep CLI | Template compile | ✓ | 0.43.8 | `az` bundles bicep |
| PowerShell 7 | `manage-env.ps1` | ✓ | 7.5.4 | Windows PowerShell 5.1 |
| Active `az login` + subscription | `up`/`down` | ✗ (must verify) | — | `az login` interactively before run |
| `Az` PowerShell module (in runbook) | Auto-destroy runbook | n/a (cloud) | Automation default | Az modules pre-installed in Automation, or import |
| Internet egress to `api.ipify.org` | Public-IP detection | ✓ (assumed) | — | hardcode IP param fallback |

**Missing dependencies with no fallback:** None blocking — but an authenticated `az` session against the chosen subscription is required before the first `up`. The planner should add a pre-flight check (`az account show`) in `manage-env.ps1`.

**Missing dependencies with fallback:** Public-IP detection service — allow `-AllowedSourceIp` override param if the lookup fails.

## Validation Architecture

> `nyquist_validation` is enabled. This is an IaC phase — "tests" are deployment + post-deploy assertions, not a unit-test framework.

### Test Framework
| Property | Value |
|----------|-------|
| Framework | **Pester** (PowerShell test framework) for `.ps1` logic + post-deploy assertions; `az deployment group validate` / `what-if` for Bicep |
| Config file | none yet — Wave 0 creates `infra/tests/*.Tests.ps1` |
| Quick run command | `az deployment group what-if -g <rg> --template-file infra/main.bicep --parameters ...` (no-cost preview) + `bicep build infra/main.bicep` (lint/compile) |
| Full suite command | `manage-env.ps1 -Action up` → run `infra/tests/Validate-Target.ps1` → `manage-env.ps1 -Action down` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| ENV-01 | Bicep compiles + passes what-if | static | `bicep build infra/main.bicep`; `az deployment group what-if ...` | ❌ Wave 0 |
| ENV-01 | RDP reachable + NLA enforced | integration | `Test-NetConnection <ip> -Port 3389`; remote `Get-ItemProperty ...UserAuthentication` via WinRM | ❌ Wave 0 |
| ENV-01 | WinRM 5986 reachable | integration | `Test-NetConnection <ip> -Port 5986`; `Enter-PSSession -UseSSL -SkipCACheck` | ❌ Wave 0 |
| ENV-01 | `RemoteDesktop_SuppressWhenMinimized=2` set | integration | WinRM: read both HKLM keys + default-hive value | ❌ Wave 0 |
| ENV-01 | 96 DPI for automation user | integration | WinRM: read default-hive `LogPixels`/`Win8DpiScaling` (or post-login effective DPI) | ❌ Wave 0 |
| ENV-01 | 7-Zip installed | integration | WinRM: `Test-Path 'C:\Program Files\7-Zip\7zFM.exe'` | ❌ Wave 0 |
| ENV-02 | `up` provisions + writes connection file; `down` removes RG | integration | run `up`, assert `.secrets/connection.json` + `az group show` succeeds; run `down`, assert `az group show` fails | ❌ Wave 0 |
| ENV-03 | Runbook + schedule provisioned; identity has RBAC | static + integration | `az automation runbook show`; `az role assignment list`; (optionally trigger runbook once and confirm RG deleted) | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `bicep build infra/main.bicep` (compile) + `Invoke-Pester infra/tests` for any `.ps1` logic changed (no Azure cost).
- **Per wave merge:** `az deployment group what-if` against a scratch RG (no-cost preview).
- **Phase gate:** full `up` → `Validate-Target.ps1` (all ENV-01 assertions green) → `down` on a real subscription before `/gsd-verify-work`. This incurs short-lived VM cost — keep the box up only for the assertion run.

### Wave 0 Gaps
- [ ] `.gitignore` — add `.secrets/`, `.env` (repo has none — blocking for D-06 safety)
- [ ] `infra/tests/Validate-Target.ps1` — post-deploy assertion script covering all ENV-01 criteria over WinRM
- [ ] `infra/tests/ManageEnv.Tests.ps1` — Pester tests for IP detection, password generation, param wiring (mock `az`)
- [ ] Pester install if absent: `Install-Module Pester -Scope CurrentUser` (verify on npmjs-equivalent — PSGallery; pin version)

## Security Domain

> `security_enforcement` enabled, ASVS level 1, block on high. This phase provisions infrastructure and handles credentials/network exposure.

### Applicable ASVS Categories
| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | NLA-enforced RDP; strong generated admin password (≥24 chars, D-06); WinRM over HTTPS with credentials |
| V3 Session Management | no | No app sessions in this phase |
| V4 Access Control | yes | NSG allowlist to single dev IP (D-05); runbook managed-identity RBAC scoped least-privilege (Pitfall 4) |
| V5 Input Validation | partial | `manage-env.ps1` params (`-Action`, IP) validated; `ValidateSet`/regex on IP |
| V6 Cryptography | yes | TLS for WinRM (self-signed, throwaway); password via cryptographic RNG — never hand-roll |
| V7 Secrets / Data Protection | yes | Credentials in gitignored file only; `@secure()` Bicep param; `.gitignore` mandatory before first run; never log the password |
| V14 Configuration | yes | No public 0.0.0.0/0; ports limited to 3389/5986; auto-destroy limits exposure window |

### Known Threat Patterns for Azure throwaway Windows VM
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| RDP brute-force / exposed 3389 | Spoofing / Elevation | NSG scoped to dev IP only; NLA; strong random password; auto-destroy TTL |
| Credential leak via committed secrets | Information Disclosure | `.gitignore` `.secrets/`; `@secure()` param; no console echo |
| Over-privileged runbook identity | Elevation of Privilege | Least-privilege role assignment scoped to the RG (or subscription only if self-delete required) |
| Supply-chain (tampered 7-Zip binary) | Tampering | Pin version + verify SHA-256 before install |
| Runaway cost / forgotten box (availability of budget) | Denial of Service (financial) | Scheduled auto-destroy (ENV-03) + on-demand `down` |
| Self-signed WinRM MITM | Tampering / Spoofing | Acceptable for throwaway box scoped to one IP; documented `-SkipCACheck` is a known tradeoff, not for production |

## Sources

### Primary (HIGH confidence)
- [MS: Quickstart Bicep Windows VM (`vm-simple-windows`)](https://learn.microsoft.com/en-us/azure/virtual-machines/windows/quick-create-bicep) — full template, API versions, WS2022 SKU allowed-values, `@secure()` adminPassword
- [MS sample: vm-winrm-windows](https://learn.microsoft.com/en-us/samples/azure/azure-quickstart-templates/vm-winrm-windows/) — WinRM HTTPS self-signed via CustomScriptExtension; client connect pattern
- [MS: Custom Script Extension for Windows](https://learn.microsoft.com/en-us/azure/virtual-machines/extensions/custom-script-windows) — CSE structure, protectedSettings
- [Azure VM Runtime Team: when does CSE re-execute](https://devblogs.microsoft.com/azure-vm-runtime/when-will-customscript-extension-re-execute-my-script/) — idempotency requirement
- [MS: Delete resource group](https://learn.microsoft.com/en-us/azure/azure-resource-manager/management/delete-resource-group) — no Bicep destroy; `az group delete`
- [MS: PowerShell runbook with managed identity](https://learn.microsoft.com/en-us/azure/automation/learn/powershell-runbook-managed-identity) — Connect-AzAccount -Identity
- [MS: DPI-related APIs and registry settings](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/dpi-related-apis-and-registry-settings) — LogPixels / Win8DpiScaling
- [MS: Customize default user profile (CopyProfile)](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/customize-the-default-user-profile-by-using-copyprofile) — default-hive editing rationale

### Secondary (MEDIUM confidence)
- [MS Q&A / TechNet: RemoteDesktop_SuppressWhenMinimized](https://learn.microsoft.com/en-us/answers/questions/2196906/how-to-prevent-minimized-rdp-sessions-from-freezin) — value 2, HKLM/Wow6432Node/HKCU scopes
- [Stephen Wagner: modify default user registry hive](https://www.stephenwagner.com/2024/05/22/modify-add-new-default-user-registry/) — `reg load`/`reg unload` + handle-release technique
- [Stellium / arnav.au: Azure Automation auto-clean resource groups](https://arnav.au/2022/09/16/auto-clean-azure-resources-using-azure-automation/) — TTL runbook pattern

### Tertiary (LOW confidence — verify at plan time)
- Public-IP detection via `api.ipify.org` — standard but external dependency; provide override param
- Exact 7-Zip version/URL/SHA-256 — must be confirmed against 7-zip.org at plan time

## Metadata

**Confidence breakdown:**
- Standard stack (Bicep VM/network/automation): HIGH — verified against current MS quickstarts + installed tool versions
- WinRM HTTPS via CSE: HIGH — official MS sample
- Per-user DPI/SuppressWhenMinimized mechanism: MEDIUM-HIGH — default-hive technique verified; exact `Win8DpiScaling` semantics has a documented conflict (Open Q1)
- Auto-destroy runbook: HIGH on mechanism, MEDIUM on self-delete topology (needs user decision)
- Image SKU + 7-Zip pinning: MEDIUM — verify region availability + current binary hash at plan time

**Research date:** 2026-06-04
**Valid until:** 2026-07-04 (Azure API versions/SKUs are stable; re-verify 7-Zip URL and region SKU availability at deploy time)
