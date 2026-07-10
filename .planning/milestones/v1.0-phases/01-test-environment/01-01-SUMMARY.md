---
phase: 01-test-environment
plan: 01
subsystem: infra
tags: [scaffolding, gitignore, pester, secrets, validation-harness, wave-0]
one_liner: "Secrets-safe .gitignore + infra/ IaC root + Pester unit scaffold (mocked az) and ENV-01 post-deploy assertion skeleton — the no-Azure-cost test baseline every later Phase 1 plan consumes."
requires: []
provides:
  - ".gitignore excluding .secrets/ and .env (secrets-safe baseline)"
  - "infra/ IaC root with scripts/ and tests/ subdirectories"
  - "infra/README.md (up/down usage, az login pre-flight, secrets convention)"
  - "infra/tests/ManageEnv.Tests.ps1 (Pester unit scaffold, az/network mocked)"
  - "infra/tests/Validate-Target.ps1 (ENV-01 post-deploy assertion skeleton)"
affects:
  - "Plan 02 (cloud infra Bicep), Plan 03 (in-guest hardening), Plan 04 (manage-env.ps1) all build on this layout/harness"
tech_stack:
  added:
    - "Pester 5.7.1 (PowerShell test framework, already installed — no install needed)"
  patterns:
    - "infra/ chosen as the single IaC root (over tools/env/)"
    - ".secrets/connection.json is the only credential location; @secure() Bicep param; never echoed/logged/committed"
    - "Per-commit gate is no-Azure-cost: Invoke-Pester with az + Invoke-RestMethod mocked"
    - "Pester -Skip conditions computed at discovery time (top-level script body), not in BeforeAll"
    - "Skeleton-now / live-later test pattern: Plan-04-dependent assertions skipped with reason; assertion command text present for later activation"
key_files:
  created:
    - ".gitignore"
    - "infra/README.md"
    - "infra/scripts/.gitkeep"
    - "infra/tests/.gitkeep"
    - "infra/tests/ManageEnv.Tests.ps1"
    - "infra/tests/Validate-Target.ps1"
  modified: []
decisions:
  - "infra/ is the IaC root (not tools/env/) — locked here for all later Phase 1 artifacts"
  - "Cargo.lock policy deferred to Phase 2 (.gitignore only excludes /target/ and *.rs.bk for now)"
  - "Did NOT add a .gitkeep-removal step: infra/tests/.gitkeep kept alongside the two test files for a stable, explicit layout"
metrics:
  duration: "~12 min"
  completed: "2026-06-04"
  tasks: 3
  files: 6
  commits: 3
---

# Phase 1 Plan 01: Infrastructure Baseline & Validation Harness Summary

JWT-of-nothing here — this is **Wave 0 scaffolding**. It delivers no ENV-* requirement
directly (`requirements: []`); it lays the secrets-safe baseline, the `infra/` file
layout, and the Pester / assertion harness that Plans 02–04 consume. ENV-01 is delivered
by Plan 02 + 03; ENV-02/ENV-03 by Plan 04.

## What Was Built

- **`.gitignore` (repo root, the project's first):** excludes `.secrets/` and `.env`
  before any provisioning can run (Pitfall 7). `git check-ignore .secrets/connection.json`
  exits 0. Also excludes Rust build output (`/target/`, `*.rs.bk`) and editor/OS noise.
- **`infra/` IaC root:** `infra/scripts/` and `infra/tests/` created (each tracked via a
  documented `.gitkeep`). `infra/README.md` documents the `manage-env.ps1 -Action up|down`
  entry point (ENV-02), the mandatory authenticated `az login` pre-flight, the West Europe
  default region (D-05/A6, overridable), and the `.secrets/connection.json` secrets
  convention.
- **`infra/tests/ManageEnv.Tests.ps1` (Pester 5.7.1):** 3 LIVE assertions on a standalone
  crypto-RNG password generator (length ≥ 24, all four character classes, distinctness
  across runs) + 3 Plan-04-dependent tests (`ipify` IP parse, `az deployment group create`
  param wiring for `adminPassword=`/`allowedSourceIp=`, `-AllowedSourceIp` override) marked
  `-Skip` with reason "infra/manage-env.ps1 created in Plan 04". `az` and `Invoke-RestMethod`
  are mocked — zero Azure cost, zero network. Runs green: `Tests Passed: 3, Failed: 0, Skipped: 3`.
- **`infra/tests/Validate-Target.ps1`:** post-deploy ENV-01 assertion suite. In skeleton
  mode (no `.secrets/connection.json`) it prints one `PENDING` line per ENV-01 criterion and
  exits 0. The live path (run for real by Plan 04's phase gate) contains every required
  assertion command: `Test-NetConnection` for 3389/5986, NLA `UserAuthentication -eq 1`
  (verify, not set — Pitfall 2), default-hive `LogPixels=96`/`Win8DpiScaling=1` via
  load/read/unload (Pitfall 1), `RemoteDesktop_SuppressWhenMinimized=2` at HKLM + default
  hive, and `7zFM.exe`. WinRM sessions use `-SkipCACheck -SkipCNCheck -UseSSL` (Pitfall 3).
  Password is read into a secure credential only — never echoed.

## Task Commits

| Task | Name | Commit | Files |
| ---- | ---- | ------ | ----- |
| 1 | `.gitignore` + `infra/` layout | `9136e2d` | `.gitignore`, `infra/README.md`, `infra/scripts/.gitkeep`, `infra/tests/.gitkeep` |
| 2 | Pester scaffold for manage-env.ps1 logic | `6a35b86` | `infra/tests/ManageEnv.Tests.ps1` |
| 3 | Post-deploy ENV-01 assertion skeleton | `635a8af` | `infra/tests/Validate-Target.ps1` |

## Verification

- `git check-ignore .secrets/connection.json` → exit 0 (path ignored).
- `Invoke-Pester -Path infra/tests/ManageEnv.Tests.ps1 -CI` → exit 0 (3 passed, 3 skipped).
- `Validate-Target.ps1` standalone → exit 0 (skeleton mode, PENDING per check).
- No real Azure or network calls in any test run (all mocked / skeleton).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `infra/tests/` did not exist for Task 1's verify command**
- **Found during:** Task 1 (its verify asserts `Test-Path infra/tests`, but the tests dir
  is only populated by Tasks 2/3).
- **Fix:** Added a documented `infra/tests/.gitkeep` so the directory is tracked immediately
  and Task 1's verification passes. Harmless alongside the two test files added later.
- **Files modified:** `infra/tests/.gitkeep`
- **Commit:** `9136e2d`

**2. [Rule 1 - Bug] Pester `-Skip` condition evaluated null at discovery time**
- **Found during:** Task 2 (first Pester run: 3 expected-skipped tests FAILED with
  `CommandNotFoundException` instead of skipping).
- **Issue:** `$script:SkipUntilPlan04` was set in `BeforeAll` (run phase), but Pester 5
  evaluates `It -Skip:<cond>` at the earlier **discovery** phase, so the condition was
  unset and the tests ran instead of skipping.
- **Fix:** Moved the script-path / skip-condition computation to the top-level script body
  (discovery phase). Re-run: 3 passed, 3 skipped, exit 0.
- **Files modified:** `infra/tests/ManageEnv.Tests.ps1`
- **Commit:** `6a35b86`

### Scope-discipline note (not a deviation)
- Trimmed the `.gitignore` Rust section to exclude only build output (`/target/`,
  `*.rs.bk`) and deliberately left `Cargo.lock` out — the lockfile-commit policy for a
  library is a Phase 2 decision and was out of scope here.

## Known Stubs

The two test files are intentional **Wave 0 skeletons** (documented in the plan as
"skeletons activated/run-for-real by later plans"), not unintended stubs:

| File | Stub | Reason / Resolution |
| ---- | ---- | ------------------- |
| `infra/tests/ManageEnv.Tests.ps1` | 3 `It` blocks `-Skip`ped | Depend on `infra/manage-env.ps1`; activated when **Plan 04** creates it. |
| `infra/tests/Validate-Target.ps1` | skeleton mode (PENDING per check) | Requires a live `.secrets/connection.json`; real WinRM probes run by **Plan 04's** phase gate after `up`. |

Both are by design and do not block this scaffolding plan's goal (a green, no-cost harness).

## Threat Model Outcomes

- **T-01-01 (secrets committed) — mitigated:** `.gitignore` excludes `.secrets/` and `.env`,
  committed in Task 1 before any `up`; `git check-ignore` asserted exit 0.
- **T-01-02 (password in test output) — mitigated:** `ManageEnv.Tests.ps1` asserts on
  length/class only; `Validate-Target.ps1` reads the password into a secure credential and
  never writes it. No real/generated password is emitted anywhere.
- **T-01-03 (untrusted Pester) — accepted (as planned):** Pester 5.7.1 was already installed
  (PSGallery, first-party); no install was performed. Version pinned/recorded in the test header.

No new security surface introduced beyond the plan's threat model.

## Self-Check: PASSED

- FOUND: `.gitignore`
- FOUND: `infra/README.md`
- FOUND: `infra/scripts/.gitkeep`
- FOUND: `infra/tests/.gitkeep`
- FOUND: `infra/tests/ManageEnv.Tests.ps1`
- FOUND: `infra/tests/Validate-Target.ps1`
- FOUND commit: `9136e2d`
- FOUND commit: `6a35b86`
- FOUND commit: `635a8af`
