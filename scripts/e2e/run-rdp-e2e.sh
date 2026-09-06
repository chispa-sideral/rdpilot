#!/usr/bin/env bash
# Execute the daemon RDP E2E gate against an explicit, owner-only connection file.
set -euo pipefail

connection_file="${1:-${RDPILOT_CONNECTION_FILE:-}}"
[[ -n "$connection_file" && -r "$connection_file" ]] || {
  echo 'An explicit readable RDPILOT_CONNECTION_FILE is required.' >&2
  exit 2
}
command -v python3 >/dev/null || { echo 'python3 is required.' >&2; exit 2; }
command -v timeout >/dev/null || { echo 'timeout is required.' >&2; exit 2; }

python3 - "$connection_file" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding='utf-8') as source:
        target = json.load(source)
    for key in ('host', 'user', 'password'):
        if not isinstance(target.get(key), str) or not target[key]:
            raise ValueError(f'missing non-empty string field: {key}')
    if not isinstance(target.get('rdpPort', 3389), int):
        raise ValueError('rdpPort must be an integer')
except (OSError, ValueError, json.JSONDecodeError) as error:
    print(f'Invalid connection file: {error}', file=sys.stderr)
    raise SystemExit(2)
PY

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
timeout_limit="${RDPILOT_E2E_TIMEOUT:-20m}"
connect_timeout="$(bash "$repo_root/scripts/e2e/validate-connect-timeout.sh" "${RDPILOT_CONNECT_TIMEOUT:-}")"

cd "$repo_root"
RDPILOT_LIVE=1 RDPILOT_CONNECTION_FILE="$connection_file" \
RDPILOT_CONNECT_TIMEOUT="${connect_timeout}s" RDPILOT_CONNECT_TIMEOUT_SECS="$connect_timeout" \
  timeout --foreground "$timeout_limit" \
  cargo test -p rdpilot-daemon --test live_daemon_e2e -- --ignored --test-threads=1
