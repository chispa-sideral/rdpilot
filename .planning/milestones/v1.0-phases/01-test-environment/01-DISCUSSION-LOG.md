# Phase 1: Test Environment - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-06-04
**Phase:** 1-test-environment
**Areas discussed:** VM image & size, Sample remote-only program, Auto-destroy mechanism, Credentials & network access

---

## VM Image & Size

### Image

| Option | Description | Selected |
|--------|-------------|----------|
| Windows Server 2022 Datacenter (Desktop Experience) | Pay-as-you-go Azure licensing, real explorer.exe desktop, full UIA; Server Manager autostarts | ✓ |
| Windows 11 Enterprise | Closest to a real end-user desktop; requires Visual Studio subscription or AVD eligibility | |
| Windows Server 2025 | Newer, but less tested for IronRDP compatibility at this stage | |

**User's choice:** Windows Server 2022 Datacenter (Desktop Experience)
**Notes:** Accepted caveat that Server Manager autostarts and there are no Store/UWP apps; the benefit of frictionless licensing outweighed the minor shell difference.

### Size

| Option | Description | Selected |
|--------|-------------|----------|
| Standard_B2ms | 2 vCPU / 8 GB, burstable; cheapest that comfortably runs WS2022 Desktop + RDP + GUI app | ✓ |
| Standard_D2s_v5 | 2 vCPU / 8 GB, non-burstable; more consistent CPU but higher cost | |
| Standard_B2s | 2 vCPU / 4 GB; cheaper but tight on RAM for a Desktop Experience install | |

**User's choice:** Standard_B2ms
**Notes:** If burst throttling causes screenshot/framebuffer latency noise in later phases, revisit Standard_D2s_v5.

---

## Sample Remote-Only Program

| Option | Description | Selected |
|--------|-------------|----------|
| A real FOSS Win32 app (7-Zip File Manager) | Non-trivial UIA tree (menus, toolbar, listview, modal dialogs); FOSS/LGPL-compatible; silent install | ✓ |
| A bundled custom WinForms/WPF app | Fully controlled UIA tree; requires writing and maintaining app code | |
| Windows built-ins only (Notepad, Paint, etc.) | Zero install effort; UIA trees are too simple to exercise later phases meaningfully | |

**User's choice:** A real FOSS Win32 app — 7-Zip File Manager as concrete default
**Notes:** Chosen specifically for its non-trivial UIA tree that realistically exercises Phase 9's end-to-end "navigate a real program" demo. A deterministic known-tree app for test assertions can be added later if needed.

---

## Auto-Destroy Mechanism

| Option | Description | Selected |
|--------|-------------|----------|
| Self-deleting deployment via scheduled runbook / Logic App (full RG delete) | Cloud-native; fires even if dev machine is off; meets roadmap success criterion of full RG removal | ✓ |
| RG-tag + VM auto-shutdown | Stops the VM (cost reduction) but does not remove the resource group; does not satisfy ENV-03 | |
| Local scheduled task | Depends on dev machine being on; fragile, not cloud-native | |

**User's choice:** Cloud-native scheduled safeguard — Azure Automation runbook or scheduled Logic App provisioned as part of the environment, running `az group delete`
**Notes:** Strongest cost guarantee; the implementation detail (runbook vs. Logic App) is left for the planner to resolve based on simplicity of provisioning from Bicep.

---

## Credentials & Network Access

### Network Access

| Option | Description | Selected |
|--------|-------------|----------|
| Public IP + NSG allowlist to current dev IP | IP detected at `up` time; ports 3389 + WinRM scoped to single IP; no extra Azure resource | ✓ |
| Azure Bastion | No public IP; browser-based RDP; awkward for IronRDP to consume a direct socket; extra cost | |
| Fully open (0.0.0.0/0) | Simplest setup; unacceptable global exposure | |

**User's choice:** Public IP + NSG allowlist scoped to developer's current public IP
**Notes:** Accepted caveat that a changing dev IP mid-session requires re-running the allowlist step.

### Credentials Storage

| Option | Description | Selected |
|--------|-------------|----------|
| Generated at `up`, written to gitignored local file | Strong random password; secrets file in `.gitignore`; later phases read from it | ✓ |
| Azure Key Vault | Secure; adds extra resource, soft-delete quirks, az-auth dependency on every connect | |
| Prompt each run | No stored secrets; too painful across 9 phases of repeated test runs | |

**User's choice:** Generated at `up` time, written to a gitignored local file
**Notes:** The secrets path MUST be in `.gitignore` from the start. Azure Key Vault rejected as too heavy for a solo throwaway box.

---

## Final Gate

**User's choice:** Ready for context (proceed to write artifacts)
**Notes:** Region, WinRM transport, and DPI enforcement mechanism intentionally left open for the researcher/planner to resolve.

---

## Claude's Discretion

The following HOW-details were left open for the researcher/planner:
- **Azure region & subscription** — not pinned; planner to choose or confirm with user.
- **WinRM transport** — HTTP 5985 vs HTTPS 5986, auth method, self-signed cert provisioning. Lean HTTPS/5986.
- **96 DPI + `RemoteDesktop_SuppressWhenMinimized=2` enforcement** — registry keys vs first-login script vs GPO; must apply to the automation user. The requirement is locked (ENV-01); only the mechanism is open.

## Deferred Ideas

None — discussion stayed within phase scope. No scope-creep ideas arose.
