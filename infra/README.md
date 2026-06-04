# rdpilot — Infrastructure (`infra/`)

This directory is the **IaC root** for rdpilot's disposable Azure Windows test target.
Phase 1 provisions a throwaway Windows Server 2022 VM (RDP + WinRM ready) that every
later phase tests against, with a cloud-native scheduled auto-destroy so a forgotten
box can never run up a bill.

> **Convention:** `infra/` is the single IaC root (chosen over `tools/env/`). All
> Infrastructure-as-Code, provisioning scripts, and post-deploy assertions live here.

## Layout

```
infra/
  main.bicep            # Network + VM + CustomScriptExtension + Automation (auto-destroy). Created by Plan 02/03.
  manage-env.ps1        # Entry point: `up` / `down` driver (ENV-02). Created by Plan 04.
  scripts/
    Configure-Target.ps1     # In-guest idempotent hardening (NLA verify, 96 DPI, SuppressWhenMinimized, 7-Zip). Plan 03.
    Delete-ResourceGroup.ps1 # Auto-destroy runbook body (managed identity). Plan 02/03.
  tests/
    ManageEnv.Tests.ps1   # Pester unit tests (mocked `az`, no Azure cost). This plan (01).
    Validate-Target.ps1   # Post-deploy WinRM assertions for ENV-01 criteria. This plan (01).
  README.md             # This file.
```

> Files marked "created by Plan NN" do not exist yet — this plan (01) lays down the
> directory skeleton, the secrets-safe `.gitignore`, and the test/assertion harness.
> The Bicep and `manage-env.ps1` are authored by later Phase 1 plans.

## Entry point: `manage-env.ps1 -Action up|down` (ENV-02)

The single driver for the test environment lifecycle:

```powershell
# Provision the environment (detects your public IP, generates a strong password,
# deploys the RG, writes connection details to .secrets/connection.json):
pwsh ./infra/manage-env.ps1 -Action up

# Tear everything down (deletes the whole resource group):
pwsh ./infra/manage-env.ps1 -Action down
```

`up` also exposes `-AllowedSourceIp <ip>` to override automatic public-IP detection
(used when the ipify lookup is unavailable or your egress IP differs).

## Pre-flight: authenticated `az login` required

`up`/`down` operate against your Azure subscription via the `az` CLI. Before running
either, you **must** have an authenticated session against the intended subscription:

```powershell
az login
az account show          # confirm the active subscription is the one you intend to deploy into
az account set --subscription "<subscription-id-or-name>"   # if needed
```

`manage-env.ps1 up` performs a fast-fail `az account show` pre-flight and aborts with a
clear error if you are not logged in — it will not silently deploy into the wrong place.

## Default region

The default deployment region is **West Europe** (`westeurope`) — EU-first, per the
project's privacy-by-design / EU-FOSS posture. This is Claude's discretion per decision
D-05/A6 and may be overridden (the region is a parameter on the driver; confirm the
region and subscription before your first `up`).

## Secrets convention

Credentials and connection details live in **exactly one place**: the gitignored
`.secrets/connection.json`, written by `manage-env.ps1 up`. It contains the host, admin
user, generated password, and RDP/WinRM ports; later phases read it to connect.

- The admin password is generated with a cryptographic RNG at `up` time and is **never
  echoed to the console, never logged, and never committed**.
- `.secrets/` and `.env` are excluded by the repo-root `.gitignore` (created in this
  plan, before any `up` ever runs — Pitfall 7).
- No secret is ever placed in a Bicep parameter default or a committed
  `*.parameters.json`; the Bicep `adminPassword` is a `@secure()` parameter.

If you suspect a secret was committed, treat it as compromised: rotate it and scrub it
from history. Prevention is the `.gitignore` here plus the `@secure()` parameter.

## Testing

Two layers, both no-Azure-cost where possible:

- `tests/ManageEnv.Tests.ps1` — Pester unit tests for the driver's logic (password
  entropy, public-IP parsing, `az` parameter wiring) with `az` and the network mocked.
  Run: `Invoke-Pester -Path infra/tests/ManageEnv.Tests.ps1 -CI`.
- `tests/Validate-Target.ps1` — post-deploy assertion suite covering every ENV-01
  criterion (RDP 3389 / WinRM 5986 reachability, NLA enforced, 96 DPI,
  `RemoteDesktop_SuppressWhenMinimized=2`, 7-Zip installed). It runs as a no-op
  **skeleton** (printing a `PENDING` line per check, exit 0) until a live
  `.secrets/connection.json` exists, at which point it executes the real WinRM probes.
  Run: `pwsh ./infra/tests/Validate-Target.ps1`.

The phase gate is: `up` → `Validate-Target.ps1` all green → `down`.
