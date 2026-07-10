# Phase 1: Test Environment - Context

**Gathered:** 2026-06-04
**Status:** Ready for planning

<domain>
## Phase Boundary

Provision a disposable, reproducible Azure Windows test target — RDP/automation-ready — with scheduled auto-destroy, so every later phase has a real box to test against.

</domain>

<decisions>
## Implementation Decisions

### VM Image & Size
- **D-01:** Use **Windows Server 2022 Datacenter, Desktop Experience**. Frictionless pay-as-you-go Azure licensing (no Visual Studio subscription or AVD eligibility needed); real `explorer.exe` desktop and full UIA. Accepted caveat: Server Manager autostarts; no Store/UWP apps.
- **D-02:** VM size **Standard_B2ms** (2 vCPU / 8 GB, burstable). Cheapest size that comfortably runs WS2022 Desktop + RDP + a GUI app for short disposable sessions. If burst throttling causes screenshot/framebuffer latency noise in later phases, revisit Standard_D2s_v5.

### Sample Remote-Only Program
- **D-03:** Default to **7-Zip File Manager** (silent install, FOSS, LGPL-compatible). It has a non-trivial UIA tree (menus, toolbar, listview, modal dialogs) that realistically exercises window-list and accessibility extraction in later phases — especially the Phase 9 end-to-end "navigate a real program" demo — without writing any app code.

### Auto-Destroy Mechanism
- **D-04:** **Cloud-native scheduled safeguard that deletes the whole resource group** — an Azure Automation runbook (or scheduled Logic App) provisioned as part of the environment, running `az group delete` on a TTL/schedule. Chosen over VM auto-shutdown (stop-only) and a local scheduled task because the roadmap success criterion is full RG removal and the safeguard must fire even if the dev machine is off. Strongest cost guarantee.

### Network Access
- **D-05:** **Public IP on the VM + NSG allowlist scoped to the developer's current public IP**, detected at `up` time by the `.ps1`. Ports opened: **RDP 3389** and **WinRM (5986 HTTPS preferred; transport TBD — see open items)**, restricted to that single IP only. Rejected: Azure Bastion (cost/complexity, awkward for IronRDP to consume a direct socket) and fully-open 0.0.0.0/0 (global exposure). Accepted caveat: a changing dev IP mid-session requires re-running the allowlist step.

### Credentials & Connection Details
- **D-06:** **Generated at `up` time, written to a gitignored local file** (e.g. `.env` or a path under `.secrets/`). The `.ps1` generates a strong random admin/automation password at provision time and writes connection details locally; later phases read from that file. The chosen secrets path MUST be listed in `.gitignore` from the start. Rejected: Azure Key Vault (extra resource/cost/soft-delete quirks + az-auth dependency on every connect — too heavy for a solo throwaway box) and prompt-each-run (too painful across 9 phases of repeated test runs).

### Claude's Discretion

The following implementation details were left open intentionally — they are HOW-details for the researcher/planner to resolve, not undecided scope:

- **Azure region & subscription** — not pinned during discussion. Planner/researcher to choose (or confirm with user) a suitable region and target subscription.
- **WinRM transport** — HTTP 5985 vs HTTPS 5986, auth method (Negotiate/Kerberos/Basic), and whether a self-signed cert is provisioned. Lean toward HTTPS/5986.
- **96 DPI + `RemoteDesktop_SuppressWhenMinimized=2` enforcement mechanism** — registry keys vs a first-login script vs GPO; must apply to the automation user. The REQUIREMENT that these be set is locked by ENV-01; only the mechanism is open.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

Note: No external ADRs or specs exist yet for this phase.

### Phase Definition
- `.planning/ROADMAP.md` — Phase 1 goal and 5 success criteria; the authoritative phase boundary

### Requirements
- `.planning/REQUIREMENTS.md` — ENV-01 (Bicep provisions VM: RDP/NLA, WinRM, RemoteDesktop_SuppressWhenMinimized=2, 96 DPI, sample program), ENV-02 (.ps1 up/down), ENV-03 (scheduled auto-destroy)

### Stack & Architecture
- `.planning/PROJECT.md` — locked stack decisions (Azure, Bicep + .ps1, IronRDP, DVC, Windows-only v1)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- None — greenfield repo. Only `.planning/` docs and `CLAUDE.md` exist. No `Cargo.toml`, no `src/`, no existing IaC.

### Established Patterns
- None yet. Phase 1 establishes the first real artifacts: Bicep templates and `.ps1` scripts.

### Integration Points
- Phase 1 is infrastructure/IaC only — not Rust. The Rust SDK work begins in Phase 2. Later phases will read the gitignored secrets file produced by the `.ps1 up` command to connect to the VM.

</code_context>

<specifics>
## Specific Ideas

- 7-Zip File Manager is the concrete default for the "sample remote-only program" requirement — chosen for its non-trivial UIA tree (menus, toolbar, listview, modal dialogs). A deterministic known-tree app for test assertions can be added later if needed; not required now.
- The `.ps1` must detect the developer's current public IP at `up` time and scope the NSG rule to it automatically.
- The scheduled auto-destroy safeguard (Automation runbook or Logic App) must be provisioned as part of the environment — not a separate manual step.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 1-Test Environment*
*Context gathered: 2026-06-04*
