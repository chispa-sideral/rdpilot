#!/usr/bin/env bash
# Lease a short-lived Windows build box and run only the DACL gate.
# The remote gate hydrates missing Rust/MSVC prerequisites from official
# installers, with bounded installs and no package-manager dependency.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
env_file="${CRABBOX_ENV_FILE:-$HOME/.config/crabbox/.env}"
lease_slug="rdpilotdacl$(date +%s)$RANDOM"

if ! command -v crabbox >/dev/null; then
  echo "crabbox is required for the Windows DACL gate." >&2
  exit 2
fi
if ! command -v python3 >/dev/null; then
  echo "python3 is required to verify Crabbox teardown." >&2
  exit 2
fi
if [[ ! -r "$env_file" ]]; then
  echo "Crabbox environment file is not readable: $env_file" >&2
  exit 2
fi

# Assignment-only values in the operator file must be exported for crabbox.
set -a
# shellcheck disable=SC1090
source "$env_file"
set +a
export CRABBOX_AZURE_LOCATION="${RDPILOT_CRABBOX_LOCATION:-westeurope}"

cd "$repo_root"
crabbox doctor --provider azure

# Generic Azure Windows images include Windows PowerShell; do not require
# PowerShell 7 merely to bootstrap the build host.
remote_command=(powershell -NoProfile -ExecutionPolicy Bypass -File scripts/live/Run-WindowsDacl.ps1)
if [[ "${RDPILOT_DACL_SETUP_ONLY:-0}" == "1" ]]; then
  remote_command+=(-SetupOnly)
fi

# `run` syncs this checkout and owns the newly-created lease. `always` makes
# teardown run after both a passing and a failing test; the remote script does
# the analogous cleanup for its temporary Windows account and scheduled task.
# The generated slug is this script's ownership boundary: it is passed to
# `inspect --id` and compared exactly against the provider-backed list output.
set +e
crabbox run \
  --provider azure \
  --target windows \
  --windows-mode normal \
  --type Standard_D2als_v7 \
  --ttl 90m \
  --idle-timeout 20m \
  --slug "$lease_slug" \
  --stop-after always \
  -- "${remote_command[@]}"
run_status=$?
set -e

# `list --json` is the machine-readable provider inventory contract. Do not
# accept the run exit code as teardown proof: the owned slug must be absent.
if ! inventory_json="$(crabbox list --provider azure --json)"; then
  echo "Unable to verify teardown for owned Crabbox lease '$lease_slug'." >&2
  exit 1
fi

set +e
python3 -c '
import json
import sys

slug = sys.argv[1]
try:
    inventory = json.load(sys.stdin)
except json.JSONDecodeError:
    sys.exit(2)

def contains(value):
    if isinstance(value, dict):
        return any(contains(item) for item in value.values())
    if isinstance(value, list):
        return any(contains(item) for item in value)
    return value == slug

sys.exit(1 if contains(inventory) else 0)
' "$lease_slug" <<<"$inventory_json"
inventory_status=$?
set -e

if [[ "$inventory_status" -eq 1 ]]; then
  echo "Owned Crabbox lease '$lease_slug' is still present after teardown." >&2
  exit 1
fi
if [[ "$inventory_status" -ne 0 ]]; then
  echo "Crabbox returned invalid JSON while verifying teardown for '$lease_slug'." >&2
  exit 1
fi

# `inspect` takes a lease ID or slug and must not resolve our just-released
# target. It is an additional exact-target check, independent of run's result.
if crabbox inspect --provider azure --id "$lease_slug" --json >/dev/null 2>&1; then
  echo "Owned Crabbox lease '$lease_slug' is still inspectable after teardown." >&2
  exit 1
fi

if [[ "$run_status" -ne 0 ]]; then
  exit "$run_status"
fi
