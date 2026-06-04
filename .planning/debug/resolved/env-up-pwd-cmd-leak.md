---
status: resolved
trigger: "manage-env.ps1 up printed 'No se esperaba pfYem5 en este momento' and empty Public IP; Validate-Target.ps1 probed local interfaces instead of remote VM"
created: 2026-06-04T00:00:00Z
updated: 2026-06-04T00:00:00Z
resolved: 2026-06-04
resolution_verified_by: live phase-gate pass (all ENV-01 assertions green, IP 52.157.72.209, clean teardown)
---

## Current Focus

hypothesis: Bug1 - adminPassword passed as bareword `adminPassword=$adminPwd` to `az deployment group create`; PowerShell hands `az` (a .cmd batch shim) the arg, cmd.exe re-parses special chars in the password -> "No se esperaba X en este momento". Deploy fails/partial -> publicIp output never read -> empty host. Bug2 - Validate-Target reads empty host, Test-NetConnection with empty/null host enumerates local adapters.
test: read code (done), inspect connection.json (host empty - confirmed), query live az for real IP/RG/deployment outputs
expecting: az shows a deployed public IP; deployment outputs has publicIp.value
next_action: run read-only az queries for public-ip list and deployment show outputs

## Symptoms

expected: `manage-env.ps1 up` deploys VM, captures public IP, writes it to connection.json, prints it. Validate-Target probes the REMOTE public IP.
actual: cmd parse error leaking password fragment 'pfYem5'; Public IP printed empty; connection.json host empty; Validate-Target probed local interfaces (fe80::, 192.168.1.29, 100.124.225.39, 172.28.160.1, fd7a:115c:)
errors: "No se esperaba pfYem5 en este momento." (cmd.exe: "X was not expected at this time")
reproduction: pwsh infra/manage-env.ps1 -Action up ; then pwsh infra/tests/Validate-Target.ps1
started: first real run of the env

## Eliminated

## Evidence

- timestamp: phase0
  checked: .secrets/connection.json non-secret fields
  found: host=EMPTY, user/password/rdpPort/winrmPort present
  implication: deployment-output capture failed OR az call errored before writing; Validate-Target then got empty host

- timestamp: phase1
  checked: manage-env.ps1 lines 252-261 az deployment group create
  found: adminPassword=$adminPwd passed as a single bareword token in a multiline arg list to az (a .cmd shim). Password with cmd-special chars (& | < > ( ) ^ space) breaks cmd parsing. Secret also visible on process command line.
  implication: ROOT CAUSE bug1. Fix: pass via --parameters @paramfile.json (temp) so cmd never re-parses and secret never hits cmdline.

- timestamp: phase1
  checked: Validate-Target.ps1 lines 54-87
  found: targetHost = $conn.host (empty); Test-NetConnection $targetHost with empty host falls back to local; no guard against empty host
  implication: ROOT CAUSE bug2. Fix: -PublicIp/-TargetIp param + fail-fast on empty host.

- timestamp: ground-truth
  checked: live az read-only queries against rdpilot-test
  found: deployment group list = [] (NO deployment); resource list = only storage acct rdpilotcse181zetf0; public-ip list = []; RG rdpilot-test & rdpilot-mgmt both exist (westeurope, Succeeded)
  implication: the cmd parse error killed `az deployment group create` BEFORE submission -> no VM, no public IP ever created. Bug1 is the hard blocker, not cosmetic. Validation cannot exercise in-guest ENV-01 until user re-runs `up` with the fix.

- timestamp: verify
  checked: ran `pwsh infra/tests/Validate-Target.ps1` live with current (empty-host) connection.json
  found: exits 1 with "No target IP - is the env up?" — no local-interface probing. Bug2 fix confirmed behaving correctly.
  implication: validator now fails fast on missing target instead of nonsense local probes.

## Resolution

root_cause: |
  bug1: manage-env.ps1:252-261 passed adminPassword (and other params) as bareword
  `adminPassword=$adminPwd` to `az deployment group create`. On Windows `az` is a
  .cmd shim, so cmd.exe re-parses special chars in the password -> "No se esperaba X
  en este momento" and the command never reaches az. Live az ground truth: NO
  deployment exists in rdpilot-test, NO public IP, only the CSE storage account
  -> deploy died at cmd parse. Empty publicIp -> connection.json host empty.
  bug2: Validate-Target.ps1 set $targetHost = $conn.host (empty) and called
  Test-NetConnection with no guard -> empty host enumerates LOCAL adapters.
fix: |
  bug1 (manage-env.ps1 ~252-300): serialise ALL deploy params into an ARM
  parameters JSON file via ConvertTo-Json (no shell interpolation, no cmd re-parse,
  secret never on command line); pass with `--parameters @$paramsFile`; delete the
  file in finally; check $LASTEXITCODE and throw on failure; throw if publicIp still
  empty before writing connection.json.
  bug2 (Validate-Target.ps1): add -PublicIp/-TargetIp param; resolve targetHost from
  param else conn.host; FAIL-FAST throw on empty or non-IPv4 host (never probe local
  interfaces); guard credential build + WinRM session when conn absent.
verification: both scripts parse clean; ran validator live (see Evidence)
files_changed:
  - infra/manage-env.ps1
  - infra/tests/Validate-Target.ps1
