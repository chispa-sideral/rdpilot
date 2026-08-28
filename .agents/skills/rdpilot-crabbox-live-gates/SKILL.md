---
name: rdpilot-crabbox-live-gates
description: Run or improve rdpilot's live Windows build and RDP test gates through Crabbox Azure leases. Use for project testing automation, not ordinary local Rust development or user-managed RDP hosts.
---

# Rdpilot Crabbox Live Gates

Use the system `$crabbox` and `$crabbox-azure-leasing` skills for the provider contract. Use `$rdpilot` only after a real credentialed RDP endpoint is supplied or provisioned.

## Project boundary

For ticket-level live testing, lease the existing shared Crabbox Azure infrastructure. Do **not** invoke `infra/manage-env.ps1` to create resource groups or VMs unless the user explicitly requests testing the project-owned infrastructure itself. That script has a different ownership and permission model.

Keep the two gates separate:

- The Windows DACL gate needs a Windows lease with the Rust/MSVC build environment and a temporary local test account. It verifies ACL-sensitive behavior on Windows; it is not an RDP connectivity test.
- The RDP E2E gate needs an actual reachable RDP host plus host, user, password, and port configuration. A normal Crabbox Windows lease does not by itself provide those credentials or expose a public RDP endpoint.

## Lease safely

Before leasing, check `crabbox doctor --provider azure` and inspect existing leases without changing unrelated ones. Load the project operator environment so assignment-only values are exported:

```bash
set -a
source /home/marc/.config/crabbox/.env
set +a
export CRABBOX_AZURE_LOCATION=westeurope
```

For the standard Azure Windows gate, explicitly select Azure, Windows normal mode, a short TTL/idle timeout, and the supported v7 type `Standard_D2als_v7`. Do not rely on Crabbox's default class/type.

Never print credential values, copied connection JSON, or Azure secrets. Do not turn a stale local connection file into a test target.

## Execute and close the loop

Use the project entry points instead of reconstructing commands:

- `just windows-build-host` checks the ephemeral Windows bootstrap only.
- `just windows-dacl` bootstraps and runs the Windows DACL gate.
- `just rdp-e2e /path/to/connection.json` runs the Linux-hosted RDP gate against a caller-supplied target.

`scripts/live/Initialize-BuildHost.ps1` is the approved, idempotent build-host bootstrap. It installs missing Rust MSVC and Visual Studio Build Tools from official installers, imports the VS developer environment, uses bounded waits, and removes installer binaries. Do not replace it with an ad hoc package-manager setup.

1. Sync the checked-out source through Crabbox, then use the project bootstrap and narrow Windows gate. Generic Windows images expose built-in `powershell`, not necessarily `pwsh`.
2. Collect only non-secret diagnostics from the narrow Windows gate.
3. Create only temporary test accounts and remove them even when the test fails.
4. Stop the lease promptly, then independently confirm the VM no longer exists. Treat a nonzero cleanup result as a failure to report, not as permission to delete shared resources directly.

Windows archive synchronization has previously stalled before any remote command ran. Treat a sync-quiet/watchdog failure as a Crabbox failure, not a bootstrap or test result: stop only the owned lease, verify its slug is absent from provider inventory and `inspect`, and preserve unrelated leases.

For an RDP E2E run, validate the supplied target with rdpilot before the full suite. A client stuck in `Connecting` must be treated as a disconnect/cancellation regression: record the observable state and endpoint metadata without secrets, then clean up the local daemon/session. Do not claim the E2E gate passed unless it used a fresh, credentialed target.

## When improving automation

Prefer small, project-local scripts that make these prerequisites explicit: lease parameters, image readiness, source sync, temporary-account lifecycle, target configuration, and teardown verification. Keep Azure provisioning out of the scripts unless the task explicitly covers the project-owned `infra/` path.
