#!/usr/bin/env bash
# Run the Linux-hosted daemon RDP E2E gate against an explicitly supplied target.
set -euo pipefail

usage() {
  echo "usage: $0 /path/to/connection.json | --validate-connect-timeout" >&2
  exit 2
}

parse_connect_timeout() {
  local configured="${RDPILOT_CONNECT_TIMEOUT-60s}"
  [[ "$configured" =~ ^[1-9][0-9]*s$ ]] || {
    echo "RDPILOT_CONNECT_TIMEOUT must be an integer number of seconds (for example 60s)" >&2
    return 2
  }
  local seconds="${configured%s}"
  (( seconds >= 45 && seconds <= 120 )) || {
    echo "RDPILOT_CONNECT_TIMEOUT must be between 45s and 120s" >&2
    return 2
  }
  printf '%s\n' "$seconds"
}

if [[ "${1:-}" == "--validate-connect-timeout" ]]; then
  seconds="$(parse_connect_timeout)"
  printf 'connect_timeout_seconds=%s\n' "$seconds"
  exit 0
fi

connection_file="${1:-}"
[[ -n "$connection_file" ]] || usage
[[ -r "$connection_file" ]] || { echo "Connection file is not readable: $connection_file" >&2; exit 2; }
command -v python3 >/dev/null || { echo "python3 is required to validate the connection schema." >&2; exit 2; }
command -v timeout >/dev/null || { echo "timeout is required to bound the RDP E2E gate." >&2; exit 2; }

# Validate the schema without printing values. A fresh caller-supplied file is
# required; this intentionally never falls back to .secrets/connection.json.
python3 - "$connection_file" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as source:
        target = json.load(source)
    for key in ("host", "user", "password"):
        if not isinstance(target.get(key), str) or not target[key]:
            raise ValueError(f"missing non-empty string field: {key}")
    port = target.get("rdpPort", 3389)
    if not isinstance(port, int) or not 1 <= port <= 65535:
        raise ValueError("rdpPort must be an integer from 1 to 65535")
except (OSError, ValueError, json.JSONDecodeError) as error:
    print(f"Invalid connection file: {error}", file=sys.stderr)
    sys.exit(2)
PY

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
timeout_limit="${RDPILOT_E2E_TIMEOUT:-20m}"
connect_timeout_seconds="$(parse_connect_timeout)"

cd "$repo_root"
RDPILOT_LIVE=1 RDPILOT_CONNECTION_FILE="$connection_file" RDPILOT_CONNECT_TIMEOUT_SECS="$connect_timeout_seconds" \
  timeout --foreground "$timeout_limit" \
  cargo test -p rdpilot-daemon --test live_daemon_e2e -- --ignored --test-threads=1
