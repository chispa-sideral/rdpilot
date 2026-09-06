#!/usr/bin/env bash
# Offline regression coverage for the wrapper-to-validator execution boundary.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
temporary_directory="$(mktemp -d)"
trap 'rm -rf "$temporary_directory"' EXIT

connection_file="$temporary_directory/connection.json"
cat >"$connection_file" <<'JSON'
{"host":"127.0.0.1","user":"test-user","password":"test-password","rdpPort":3389}
JSON

mkdir "$temporary_directory/bin"
cat >"$temporary_directory/bin/timeout" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == '--foreground' ]]
shift 2
exec "$@"
SH
cat >"$temporary_directory/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${RDPILOT_CONNECT_TIMEOUT:?}" >"$RDPILOT_TEST_RESULT"
SH
chmod 700 "$temporary_directory/bin/timeout" "$temporary_directory/bin/cargo"

run_valid() {
  local requested_timeout="$1"
  local expected_timeout="$2"
  local result_file="$temporary_directory/result-$expected_timeout"
  PATH="$temporary_directory/bin:$PATH" RDPILOT_CONNECT_TIMEOUT="$requested_timeout" \
    RDPILOT_TEST_RESULT="$result_file" bash "$repo_root/scripts/e2e/run-rdp-e2e.sh" "$connection_file"
  [[ "$(<"$result_file")" == "$expected_timeout" ]]
}

run_invalid() {
  local requested_timeout="$1"
  local result_file="$temporary_directory/invalid-result"
  if PATH="$temporary_directory/bin:$PATH" RDPILOT_CONNECT_TIMEOUT="$requested_timeout" \
    RDPILOT_TEST_RESULT="$result_file" bash "$repo_root/scripts/e2e/run-rdp-e2e.sh" "$connection_file" \
    >"$temporary_directory/stdout" 2>"$temporary_directory/stderr"; then
    return 1
  fi
  [[ ! -e "$result_file" ]]
  rg -q 'whole seconds with an s suffix from 45s through 120s' "$temporary_directory/stderr"
}

run_valid '' '60s'
run_valid '45s' '45s'
run_valid '120s' '120s'
run_invalid '44s'
run_invalid 'invalid'
