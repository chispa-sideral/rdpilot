#!/usr/bin/env bash
# Canonical, non-secret parser for the per-Connect live E2E deadline.
set -euo pipefail

raw="${1:-}"
if [[ -z "$raw" ]]; then
  printf '60\n'
  exit 0
fi

if [[ ! "$raw" =~ ^[0-9]+s$ ]]; then
  echo 'RDPILOT_CONNECT_TIMEOUT must be whole seconds with an s suffix from 45s through 120s' >&2
  exit 2
fi
value=$((10#${raw%s}))
if (( value < 45 || value > 120 )); then
  echo 'RDPILOT_CONNECT_TIMEOUT must be whole seconds with an s suffix from 45s through 120s' >&2
  exit 2
fi
printf '%d\n' "$value"
