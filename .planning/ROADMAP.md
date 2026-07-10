# Roadmap: rdpilot

## Milestones

- ✅ **v1.0 MVP** — Phases 1-9 (shipped 2026-07-10)

## Phases

Phase details for shipped milestones are archived. Full phase directories now live under `.planning/milestones/v1.0-phases/`.

<details>
<summary>✅ v1.0 MVP (Phases 1-9) — SHIPPED 2026-07-10</summary>

- [x] Phase 1: Test Environment (4/4 plans) — completed 2026-06-05
- [x] Phase 2: RDP Session + Framebuffer Core (3/3 plans) — completed 2026-06-05
- [x] Phase 3: Input Injection (4/4 plans) — completed 2026-07-08
- [x] Phase 4: DVC Transport Channel (3/3 plans) — completed 2026-07-09
- [x] Phase 5: Sensor Bootstrap + Deployment (4/4 plans) — completed 2026-07-09
- [x] Phase 6: Window + Process Perception (5/5 plans) — completed 2026-07-09
- [x] Phase 7: UIA Tree Module (5/5 plans) — completed 2026-07-09
- [x] Phase 8: Public SDK API + WorldState (3/3 plans) — completed 2026-07-09
- [x] Phase 9: Scripted Proof Harness (4/4 plans) — completed 2026-07-10

Full phase goals, success criteria, and plan-by-plan detail: `.planning/milestones/v1.0-ROADMAP.md`.

</details>

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|-----------------|--------|-----------|
| 1. Test Environment | v1.0 | 4/4 | Complete | 2026-06-05 |
| 2. RDP Session + Framebuffer Core | v1.0 | 3/3 | Complete | 2026-06-05 |
| 3. Input Injection | v1.0 | 4/4 | Complete | 2026-07-08 |
| 4. DVC Transport Channel | v1.0 | 3/3 | Complete | 2026-07-09 |
| 5. Sensor Bootstrap + Deployment | v1.0 | 4/4 | Complete | 2026-07-09 |
| 6. Window + Process Perception | v1.0 | 5/5 | Complete | 2026-07-09 |
| 7. UIA Tree Module | v1.0 | 5/5 | Complete | 2026-07-09 |
| 8. Public SDK API + WorldState | v1.0 | 3/3 | Complete | 2026-07-09 |
| 9. Scripted Proof Harness | v1.0 | 4/4 | Complete | 2026-07-10 |

## Backlog

### Phase 999.1: Harden live RDP test suite against frame-timing races (BACKLOG)

**Goal:** Replace fixed-time settles / blank-frame tolerances in the live integration suite with deterministic frame-readiness signals (poll-until-shell-ready, wait-for-first-real-paint), reducing flakiness like the screenshot_is_rgb_correct failure observed during Phase 2 live validation (fixed in commit 03302a0 as a band-aid).
**Requirements:** TBD
**Plans:** 3/3 plans complete

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.2: subagent tool grants — ensure file-producing agents have Write (BACKLOG)

**Goal:** Audit GSD subagent definitions so agents expected to produce files (researchers, planners, doc-writers, etc.) are granted Write/Edit tools. Observed failure: a researcher subagent burned ~6 turns trying to write a file via fs/bash/pwsh/python workarounds because it lacked Write().
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.3: Auto-surface the Azure live-test environment infra (local skill) (BACKLOG)

**Goal:** Make the existing live-test infrastructure DISCOVERABLE and self-surfacing so any agent proactively provisions it when a phase needs live RDP validation — instead of treating live testing as unavailable or forgetting the infra exists. Phase 1 already built the full stack: `infra/` Bicep (network/NSG/WS2022 VM + CustomScriptExtension), in-guest `Configure-Target.ps1` (WinRM HTTPS, 96 DPI, SuppressWhenMinimized, SHA-verified 7-Zip), `manage-env.ps1 up`/`down`, and a scheduled auto-destroy runbook. The gap is DISCOVERABILITY, not capability.

**Motivation:** During Phase 3 (Input Injection) execution, live validation (Wave 4 / 03-04) needs a running Windows target, but nothing in the executor's default context points at `manage-env.ps1 up`. Live-testing capability should announce itself at the moment of need.

**Proposed approach:** Author a local project skill (e.g. `.claude/skills/live-test-env/SKILL.md`) that documents: (a) the infra exists and where (`infra/`, `manage-env.ps1`), (b) how to spin it up/down (`manage-env.ps1 up` → connection details in `.secrets/connection.json`; `down` to tear down; auto-destroy as backstop), (c) the known gotchas (VM size `Standard_B2s_v2` in westeurope since `Standard_B2ms` is SkuNotAvailable; scoop rustup + MinGW gcc env for the Windows build host; RDPILOT_LIVE env gating for the gated suite), and (d) a trigger note so agents consult it whenever a phase's success criteria require a live remote Windows session. Cross-reference from CLAUDE.md if useful.

**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.4: Remote Assistance / Shadowing (BACKLOG)

**Goal:** We should support session shadowing (with and without control) so that an agent can offer assistance to a user in need.
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)