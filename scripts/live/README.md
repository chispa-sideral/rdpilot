# Live test gates

These scripts deliberately keep the two live gates separate.

`run-crabbox-windows-dacl.sh` leases a short-lived Windows Crabbox box, syncs
the checkout, and runs the DACL-only test. It uses the shared Crabbox Azure
infrastructure; it never invokes `infra/manage-env.ps1` or creates a resource
group. `Initialize-BuildHost.ps1` idempotently installs Rust stable (MSVC) and
the Visual Studio Build Tools C++ workload from their official installers if
they are absent, then imports the VS developer environment before the test.
Both downloads and installers are bounded; the installers are not retained.
It tags the owned lease with a unique slug and requires it to disappear from
Crabbox's JSON inventory before reporting success.

Use `just windows-dacl` for the full Windows-only test, or `just
windows-build-host` to validate only the ephemeral build host bootstrap.

`run-rdp-e2e.sh /path/to/connection.json` runs the Linux-hosted daemon E2E
test against a fresh, caller-supplied RDP target. It validates the file
without printing credentials, passes it through `RDPILOT_CONNECTION_FILE`,
and bounds the run with `RDPILOT_E2E_TIMEOUT` (default: 20 minutes). A normal
Crabbox Windows build lease is not an RDP E2E target.
