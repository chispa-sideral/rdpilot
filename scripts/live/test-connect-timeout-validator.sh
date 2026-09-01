#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
validator="$root/scripts/live/validate-connect-timeout.sh"
while IFS=':' read -r raw expected_status expected_output; do
  arg="$raw"
  [[ "$raw" == '<unset>' || "$raw" == '<empty>' ]] && arg=''
  set +e
  output="$($validator "$arg" 2>/dev/null)"
  status=$?
  set -e
  [[ "$status" == "$expected_status" ]] || { echo "unexpected status for fixture $raw: got $status expected ${expected_status:-<empty>}" >&2; exit 1; }
  [[ "$output" == "$expected_output" ]] || { echo "unexpected normalized timeout" >&2; exit 1; }
done <<'CASES'
<unset>:0:60
45s:0:45
60s:0:60
120s:0:120
44s:2:
121s:2:
60:2:
60.0s:2:
+60s:2:
 60s:2:
60ms:2:
CASES

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
connection_file="$tmpdir/connection.json"
printf '%s\n' '{"host":"192.0.2.1","user":"user","password":"password"}' > "$connection_file"
cat > "$tmpdir/cargo" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "${RDPILOT_CONNECT_TIMEOUT:-missing}" > "$RDPILOT_TIMEOUT_CAPTURE"
SH
chmod 700 "$tmpdir/cargo"

for raw in '<unset>' 45s 60s 120s; do
  capture="$tmpdir/capture"
  if [[ "$raw" == '<unset>' ]]; then
    env -u RDPILOT_CONNECT_TIMEOUT PATH="$tmpdir:$PATH" RDPILOT_TIMEOUT_CAPTURE="$capture" \
      bash "$root/scripts/live/run-rdp-e2e.sh" "$connection_file"
    expected=60s
  else
    PATH="$tmpdir:$PATH" RDPILOT_TIMEOUT_CAPTURE="$capture" RDPILOT_CONNECT_TIMEOUT="$raw" \
      bash "$root/scripts/live/run-rdp-e2e.sh" "$connection_file"
    expected="$raw"
  fi
  [[ "$(<"$capture")" == "$expected" ]] || { echo "wrapper passed wrong timeout for $raw" >&2; exit 1; }
done

capture="$tmpdir/invalid-capture"
set +e
PATH="$tmpdir:$PATH" RDPILOT_TIMEOUT_CAPTURE="$capture" RDPILOT_CONNECT_TIMEOUT=60 \
  bash "$root/scripts/live/run-rdp-e2e.sh" "$connection_file" >/dev/null 2>&1
status=$?
set -e
[[ $status -ne 0 && ! -e "$capture" ]] || { echo "bare timeout reached cargo" >&2; exit 1; }
