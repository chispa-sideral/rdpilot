---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Phase 2 context gathered
last_updated: "2026-06-05T10:51:39.336Z"
last_activity: Phase 1 complete — auto-destroy (ENV-03) validated live 2026-06-05; routing to Phase 2.
progress:
  total_phases: 9
  completed_phases: 1
  total_plans: 4
  completed_plans: 4
  percent: 11
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-06-04)

**Core value:** A local AI agent can connect to a remote Windows desktop over RDP and read/inspect a program that is only reachable via RDP — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.
**Current focus:** Phase 2: RDP Session + Framebuffer Core

## Current Position

Phase: 2 of 9 (RDP Session + Framebuffer Core) — Phase 1 complete
Plan: Phase 1 complete (4/4 plans done); ENV-01 satisfied via live gate (2026-06-04); ENV-02 satisfied via live gate (2026-06-04); ENV-03 validated live 2026-06-05 (schedule fired runbook unattended; MI reaped rdpilot-test; transient 403 absorbed by retry; activity log confirms MI completed the delete)
Status: Executing (Phase 2 not yet started)
Last activity: Phase 1 complete — auto-destroy (ENV-03) validated live 2026-06-05; routing to Phase 2.

Progress: [█░░░░░░░░░] 11% (Phase 1 complete — advancing to Phase 2)

## Performance Metrics

**Velocity:**

- Total plans completed: 4
- Average duration: ~8 min
- Total execution time: ~0.5 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 1 | 4 | ~25+ min | ~8 min |

**Recent Trend:**

- Last 5 plans: —
- Trend: —

*Updated after each plan completion*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- Stack locked: IronRDP 0.14 (Rust) + C# NativeAOT sensor + DVC transport on port 3389
- DVC channel (RDPILOT_SENSOR) must be registered before connector.connect() completes — hard IronRDP constraint
- RemoteDesktop_SuppressWhenMinimized=2 is a Phase 2 prerequisite, baked into Phase 1 VM provisioning
- Force 96 DPI on remote session; sensor sets DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2; emit both coordinate spaces
- v1 done = scripted harness proves end-to-end, no live LLM required
- Phase 1 provisions the Azure Windows target that all subsequent phases test against; auto-destroy prevents runaway cost
- `infra/` is the IaC root (chosen over `tools/env/`) — locked in Plan 01 for all later Phase 1 artifacts
- `.secrets/connection.json` is the only credential location; `.gitignore` excludes `.secrets/` and `.env` from the start (Pitfall 7)
- Cargo.lock commit policy deferred to Phase 2 (Plan 01 .gitignore only excludes /target/ + *.rs.bk)
- Pester 5.7.1 is the PowerShell test framework; per-commit gate is no-Azure-cost (az + network mocked)
- main.bicep: single RG, subnet-level NSG association, StandardSSD_LRS OS disk, NLA left at Azure default (verified later); CSE invokes Configure-Target.ps1 via scriptUri param (Plan 03/04 fill the contract)
- Compiled Bicep ARM output (infra/*.json) is gitignored — source of truth is the .bicep
- 7-Zip pinned to 26.01 via blocking human-verify checkpoint: URL github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe, SHA-256 d64a0468...94377d (computed == GitHub release asset digest); Configure-Target.ps1 throws on hash mismatch before install
- Configure-Target.ps1: per-user settings (96 DPI + SuppressWhenMinimized) go to the DEFAULT user hive via reg load/unload, never the current-user hive (Pitfall 1); no reboot; every mutation idempotent
- Forbidden-token verify gates match literals anywhere in a file (incl. comments) — keep rationale prose token-free (HKCU, 0.0.0.0/0, deploymentScripts, Owner)
- Auto-destroy topology = separate-management (user decision): persistent management RG (rdpilot-mgmt) holds the Automation Account; its managed identity = Contributor over the TEST RG (rdpilot-test) ONLY; West Europe; subscription = caller's active az sub (never hard-coded)
- manage-env.ps1 publishes BOTH Configure-Target.ps1 (CSE) and Delete-ResourceGroup.ps1 (runbook publishContentLink) to a per-up private blob + short-lived read-only single-blob SAS; storage account lives in the TEST RG so `down` cascades it; `down` leaves the management RG in place

### Pending Todos

- Begin Phase 2: RDP Session + Framebuffer Core (plan and execute IronRDP session + live screenshot capability).

### Blockers/Concerns

- Phase 5: AV/EDR environment on target is unknown — sensor binary hardening level TBD
- Phase 5: Drive redirection GPO policy on target is unknown — WinRM fallback may be required
- Phase 7/9: Target application UIA fidelity is unknown — identify and test before Phase 9 harness assertion design
- Phase 5: NativeAOT binary size unknown — benchmark during Phase 5 (5-30+ MB range)

## Deferred Items

None outstanding for Phase 1. All ENV-01/02/03 requirements satisfied.

## Session Continuity

Last session: 2026-06-05T10:51:39.328Z
Stopped at: Phase 2 context gathered
Resume file: .planning/phases/02-rdp-session-framebuffer-core/02-CONTEXT.md
