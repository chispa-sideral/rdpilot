#!/usr/bin/env bash
# scripts/install-local.sh — build, install, and dogfood rdpilot on this box.
#
# WHY a shell script and not a justfile: `just` is not guaranteed to be
# present on every box rdpilot might be dogfooded from; a POSIX/bash script
# has zero external dependency, so this is the primary install mechanism
# (research A2).
#
# WHY both binaries share one `--root`: rdpilot's daemon auto-start is a
# SIBLING-FILE lookup — `current_exe().with_file_name("rdpilot-daemon")` —
# NOT a `$PATH` search. If `rdpilot` and `rdpilot-daemon` ended up in two
# different (even both PATH-visible) directories, autostart would silently
# break with `DaemonUnreachable` (exit 3) even though `which rdpilot-daemon`
# succeeds. Installing both crates with `cargo install --root ~/.cargo`
# naturally puts both binaries in `~/.cargo/bin/`, satisfying the sibling
# constraint.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CARGO_ROOT="$HOME/.cargo"
CONFIG_DIR="$HOME/.config/rdpilot"
CONFIG_FILE="$CONFIG_DIR/config.toml"

# LOCAL-ONLY sensor staging (phase 999.8 dogfood decision): the win-x64
# NativeAOT sensor binary is never committed to git (C#/.NET8, can't be
# built on Linux — see the template comment below). If a previously-built
# binary exists on THIS box, this script stages it to a stable per-user
# runtime location and wires the daemon config to it. If it's absent, the
# install still succeeds — session-management-only (connect/list/
# disconnect) works; perceive/input/launch/put/get are inert until a
# sensor is built and configured.
SENSOR_SRC="${RDPILOT_SENSOR_SRC:-$REPO_ROOT/.secrets/sensor-build/rdpilot-sensor.exe}"
SENSOR_DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/rdpilot"
SENSOR_DEST="$SENSOR_DATA_DIR/rdpilot-sensor.exe"
SENSOR_STAGED=0
CONFIG_JUST_SEEDED=0

# Ensure the install destination is on PATH for this script's own smoke
# test below, regardless of whether the invoking shell already sourced
# `~/.cargo/env` (e.g. non-login/non-interactive shells).
export PATH="$CARGO_ROOT/bin:$PATH"

echo "==> [1/5] Using the workspace-selected stable host toolchain"
cargo --version

echo "==> [2/5] Building + installing rdpilot and rdpilot-daemon to $CARGO_ROOT/bin"
cargo install --path "$REPO_ROOT/crates/rdpilot-cli" --root "$CARGO_ROOT"
cargo install --path "$REPO_ROOT/crates/rdpilot-daemon" --root "$CARGO_ROOT"

echo "==> [3/5] Staging local sensor binary (local-only, never committed to git)"
if [ -f "$SENSOR_SRC" ]; then
  mkdir -p "$SENSOR_DATA_DIR"
  cp "$SENSOR_SRC" "$SENSOR_DEST"
  SENSOR_STAGED=1
  echo "    staged: $SENSOR_SRC -> $SENSOR_DEST"
else
  echo "    no local sensor binary found at $SENSOR_SRC — skipping (session-management-only install)"
fi

echo "==> [4/5] Seeding config (idempotent — never overwrites an existing file)"
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
if [ ! -f "$CONFIG_FILE" ]; then
  cp "$REPO_ROOT/crates/rdpilot-config/assets/config.toml.template" "$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"
  CONFIG_JUST_SEEDED=1
  echo "    seeded placeholder-only config at $CONFIG_FILE"
  if [ "$SENSOR_STAGED" -eq 1 ]; then
    {
      echo ""
      echo "# --- wired by scripts/install-local.sh (local sensor staging) ---"
      echo "sensor_binary_path = \"$SENSOR_DEST\""
    } >> "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
    echo "    wired sensor_binary_path = $SENSOR_DEST into freshly-seeded config"
  fi
else
  echo "    $CONFIG_FILE already exists — leaving it untouched"
  if [ "$SENSOR_STAGED" -eq 1 ] && ! grep -Eq '^[[:space:]]*sensor_binary_path[[:space:]]*=' "$CONFIG_FILE"; then
    echo "    NOTE: a local sensor is staged at $SENSOR_DEST but your existing"
    echo "    config has no active sensor_binary_path. Add it manually:"
    echo "      sensor_binary_path = \"$SENSOR_DEST\""
    echo "    or export RDPILOT_SENSOR_BINARY_PATH=\"$SENSOR_DEST\""
  fi
fi

echo "==> [5/5] Smoke test (D-8): confirming the CLI -> daemon IPC path works"
if ! rdpilot --version; then
  echo "FAILED: 'rdpilot --version' did not exit 0" >&2
  exit 1
fi
if ! rdpilot list >/dev/null; then
  echo "FAILED: 'rdpilot list' did not exit 0 (daemon auto-start / IPC path broken)" >&2
  exit 1
fi
if [ "$SENSOR_STAGED" -eq 1 ]; then
  if [ ! -f "$SENSOR_DEST" ]; then
    echo "FAILED: sensor was reported staged but $SENSOR_DEST is missing" >&2
    exit 1
  fi
  if [ "$CONFIG_JUST_SEEDED" -eq 1 ]; then
    WIRED_PATH="$(grep -E '^[[:space:]]*sensor_binary_path[[:space:]]*=' "$CONFIG_FILE" | sed -E 's/^[[:space:]]*sensor_binary_path[[:space:]]*=[[:space:]]*"([^"]*)".*/\1/')"
    if [ -z "$WIRED_PATH" ] || [ ! -f "$WIRED_PATH" ]; then
      echo "FAILED: freshly-seeded config's sensor_binary_path ('$WIRED_PATH') does not resolve to an existing file" >&2
      exit 1
    fi
  fi
fi

echo ""
echo "rdpilot installed successfully."
echo "Config file: $CONFIG_FILE"
if [ "$SENSOR_STAGED" -eq 1 ]; then
  echo "Sensor: staged at $SENSOR_DEST"
  if [ "$CONFIG_JUST_SEEDED" -eq 1 ]; then
    echo "        sensor_binary_path wired into the freshly-seeded config."
  else
    echo "        existing config left untouched — see NOTE above if wiring is needed."
  fi
else
  echo "Sensor: not found (looked at $SENSOR_SRC)."
  echo "        The sensor is C#/.NET 8 NativeAOT, win-x64 only, and cannot be"
  echo "        built on Linux. Build it on a Windows host (or the project's"
  echo "        Azure VM) via:"
  echo "          dotnet publish sensor/RdpilotSensor.csproj -c Release -r win-x64 \\"
  echo "            -p:PublishAot=true --self-contained"
  echo "        Then set sensor_binary_path in $CONFIG_FILE (or export"
  echo "        RDPILOT_SENSOR_BINARY_PATH) to the built rdpilot-sensor.exe path."
fi
echo "Next steps: edit $CONFIG_FILE (or export RDPILOT_HOST / RDPILOT_USERNAME /"
echo "RDPILOT_PASSWORD) with your OWN target host + credentials before running"
echo "'rdpilot connect'."
