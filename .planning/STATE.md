---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
status: executing
stopped_at: Completed 02-02-PLAN.md (live session + framebuffer core)
last_updated: "2026-06-05T13:27:20.356Z"
last_activity: Phase 2 Plan 02 complete — connect path + session loop + framebuffer snapshot + keepalive + Session handle; 21 offline tests green.
progress:
  total_phases: 9
  completed_phases: 1
  total_plans: 7
  completed_plans: 6
  percent: 86
---

# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-06-04)

**Core value:** A local AI agent can connect to a remote Windows desktop over RDP and read/inspect a program that is only reachable via RDP — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.
**Current focus:** Phase 2: RDP Session + Framebuffer Core

## Current Position

Phase: 2 of 9 (RDP Session + Framebuffer Core) — in progress
Plan: Phase 2 Plan 02 complete (2/3 plans done). The live RDP machinery: async connect (TCP → connect_begin → tokio-rustls TLS upgrade, resumption disabled, D-15 cert policy → connect_finalize) with the RDPILOT_SENSOR DVC seam; SDK-owned active-session loop (tokio::select! pump, RgbA32 DecodedImage, snapshot-on-GraphicsUpdate, DeactivateAll reactivation) on a dedicated thread; automatic 60s zero-delta keepalive; Session handle (connect/screenshot/close + Drop guard). 21 offline tests green; SESS-01/SESS-02/CAP-01 advanced. Next: Plan 03 (wire Screenshot to the live framebuffer + gated live integration suite).
Status: Executing (Phase 2 — Plans 01–02 done, Plan 03 pending)
Last activity: Phase 2 Plan 02 complete — connect path + session loop + framebuffer snapshot + keepalive + Session handle; 21 offline tests green.

Progress: [██████████] 86% (Phase 2 Plan 02 complete — 6/7 plans)

## Performance Metrics

**Velocity:**

- Total plans completed: 6
- Average duration: ~16 min
- Total execution time: ~1.6 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 1 | 4 | ~25+ min | ~8 min |
| 2 | 2/3 | ~105 min | ~52 min |

**Recent Trend:**

- Last plan: 02-02 (~70 min, 3 tasks, 8 files) — live session machinery: connect/loop/framebuffer/keepalive/Session
- Prior: 02-01 (~35 min, 3 tasks, 9 files) — toolchain blocker resolved (GNU x86_64), workspace + owned types

*Updated after each plan completion*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- Stack locked: IronRDP (Rust) + C# NativeAOT sensor + DVC transport on port 3389. **Corrected at Phase 2 Plan 01:** IronRDP versions are NOT uniform — umbrella `ironrdp = 0.15`, members at independent versions (tokio/connector 0.9, pdu 0.8, input/dvc 0.6, tls 0.2.1); the old uniform 0.14 pins do not resolve. `Cargo.lock` committed (D-02).
- **Build toolchain (Phase 2 Plan 01 architectural decision, checkpoint-approved):** build target is `x86_64-pc-windows-gnu` (MinGW-w64 gcc), NOT `*-msvc`. Host is ARM64 Windows with no MSVC/Windows SDK and VS was declined; x64 artifacts run under Windows-on-ARM x64 emulation. Toolchain pinned via `rust-toolchain.toml`; gcc linker pinned via `.cargo/config.toml`. Functionally equivalent for a pure-Rust RDP client.
- `ironrdp-tls` requires exactly one TLS backend feature — `rustls` selected (matches the planned hand-built rustls ClientConfig / D-15 custom verifier path).
- Public API exposes only owned SDK types (Error/ConnectionConfig/Screenshot/Rect); image/ironrdp/rustls/anyhow stay internal (D-09). No unwrap/expect/panic in library code (API-01). ConnectionConfig Debug redacts the password (D-14).
- DVC channel (RDPILOT_SENSOR) must be registered before connector.connect() completes — hard IronRDP constraint. **Implemented at Phase 2 Plan 02:** DrdynvcClient registered on the connector before connect_begin as the empty seam; Phase 4 adds the sensor processor via with_dynamic_channel.
- **Phase 2 Plan 02:** enabled ironrdp umbrella features (connector/session/graphics/input/dvc/svc — facade defaults to only core+pdu) and the ironrdp-tokio `reqwest` feature for ReqwestNetworkClient; added rustls-native-certs for the default validating cert path.
- **Phase 2 Plan 02 (structural):** the SDK-owned session loop runs on a dedicated OS thread with a current-thread Tokio runtime, NOT tokio::spawn — the reactivation step holds a Sequence::next_pdu_hint() -> Option<&dyn PduHint> borrow across .await, which the compiler cannot prove Send (HRTB limitation). Fully contained inside Session; public async API unchanged.
- RemoteDesktop_SuppressWhenMinimized=2 is a Phase 2 prerequisite, baked into Phase 1 VM provisioning
- Force 96 DPI on remote session; sensor sets DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2; emit both coordinate spaces
- v1 done = scripted harness proves end-to-end, no live LLM required
- Phase 1 provisions the Azure Windows target that all subsequent phases test against; auto-destroy prevents runaway cost
- `infra/` is the IaC root (chosen over `tools/env/`) — locked in Plan 01 for all later Phase 1 artifacts
- `.secrets/connection.json` is the only credential location; `.gitignore` excludes `.secrets/` and `.env` from the start (Pitfall 7)
- Cargo.lock commit policy RESOLVED in Phase 2 Plan 01: Cargo.lock IS committed (D-02), staged in the same commit as the manifests.
- Pester 5.7.1 is the PowerShell test framework; per-commit gate is no-Azure-cost (az + network mocked)
- main.bicep: single RG, subnet-level NSG association, StandardSSD_LRS OS disk, NLA left at Azure default (verified later); CSE invokes Configure-Target.ps1 via scriptUri param (Plan 03/04 fill the contract)
- Compiled Bicep ARM output (infra/*.json) is gitignored — source of truth is the .bicep
- 7-Zip pinned to 26.01 via blocking human-verify checkpoint: URL github.com/ip7z/7zip/releases/download/26.01/7z2601-x64.exe, SHA-256 d64a0468...94377d (computed == GitHub release asset digest); Configure-Target.ps1 throws on hash mismatch before install
- Configure-Target.ps1: per-user settings (96 DPI + SuppressWhenMinimized) go to the DEFAULT user hive via reg load/unload, never the current-user hive (Pitfall 1); no reboot; every mutation idempotent
- Forbidden-token verify gates match literals anywhere in a file (incl. comments) — keep rationale prose token-free (HKCU, 0.0.0.0/0, deploymentScripts, Owner)
- Auto-destroy topology = separate-management (user decision): persistent management RG (rdpilot-mgmt) holds the Automation Account; its managed identity = Contributor over the TEST RG (rdpilot-test) ONLY; West Europe; subscription = caller's active az sub (never hard-coded)
- manage-env.ps1 publishes BOTH Configure-Target.ps1 (CSE) and Delete-ResourceGroup.ps1 (runbook publishContentLink) to a per-up private blob + short-lived read-only single-blob SAS; storage account lives in the TEST RG so `down` cascades it; `down` leaves the management RG in place

### Pending Todos

- Phase 2 Plan 03: wire Screenshot to the live framebuffer snapshot + the gated live integration suite (D-16/D-17/D-18). Validate A1 (zero-delta keepalive) during the 10-min idle test; switch to the ±1px fallback if needed.
- Future agents on this machine must export the scoop rustup env (RUSTUP_HOME / CARGO_HOME / CARGO_HOME\bin on PATH) and have MinGW gcc on PATH for cargo to link.

### Blockers/Concerns

- Phase 5: AV/EDR environment on target is unknown — sensor binary hardening level TBD
- Phase 5: Drive redirection GPO policy on target is unknown — WinRM fallback may be required
- Phase 7/9: Target application UIA fidelity is unknown — identify and test before Phase 9 harness assertion design
- Phase 5: NativeAOT binary size unknown — benchmark during Phase 5 (5-30+ MB range)

## Deferred Items

None outstanding for Phase 1. All ENV-01/02/03 requirements satisfied.
Phase 2 Plan 02: pre-existing rustdoc intra-doc-link warnings in config.rs (Wave 1) logged in `.planning/phases/02-rdp-session-framebuffer-core/deferred-items.md` — out of scope, cargo doc still exits 0.

## Session Continuity

Last session: 2026-06-05T13:27:20.350Z
Stopped at: Completed 02-02-PLAN.md (live session + framebuffer core)
Resume file: None — ready for 02-03-PLAN.md
