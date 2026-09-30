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

# Ensure the install destination is on PATH for this script's own smoke
# test below, regardless of whether the invoking shell already sourced
# `~/.cargo/env` (e.g. non-login/non-interactive shells).
export PATH="$CARGO_ROOT/bin:$PATH"

# Stop only the invoking user's exact daemon process before replacing either
# sibling binary. A partial paired install must leave no old live daemon that
# can silently accept newer client frames. This is intentionally bounded:
# TERM gets five seconds, KILL gets one further second, and a surviving
# process is a visible installer failure rather than a best-effort warning.
stop_rdpilot_daemon() {
  local uid status
  uid="$(id -u)"

  if ! command -v pkill >/dev/null 2>&1 || ! command -v pgrep >/dev/null 2>&1; then
    echo "FAILED: pkill and pgrep are required to stop rdpilot-daemon safely" >&2
    return 1
  fi

  if pkill -TERM -u "$uid" -x rdpilot-daemon; then
    :
  else
    status=$?
    if [ "$status" -ne 1 ]; then
      echo "FAILED: could not send TERM to this user's rdpilot-daemon (pkill exit $status)" >&2
      return 1
    fi
  fi

  if wait_for_rdpilot_daemon_exit 50; then
    return 0
  else
    status=$?
  fi
  if [ "$status" -ne 1 ]; then
    return "$status"
  fi

  echo "    rdpilot-daemon did not exit after TERM; sending KILL" >&2
  if pkill -KILL -u "$uid" -x rdpilot-daemon; then
    :
  else
    status=$?
    if [ "$status" -ne 1 ]; then
      echo "FAILED: could not send KILL to this user's rdpilot-daemon (pkill exit $status)" >&2
      return 1
    fi
  fi
  if wait_for_rdpilot_daemon_exit 10; then
    return 0
  else
    status=$?
  fi
  if [ "$status" -eq 1 ]; then
    echo "FAILED: this user's rdpilot-daemon survived TERM and KILL" >&2
    return 1
  fi
  return "$status"
}

# Poll at 100ms intervals. Return 0 when absent, 1 when still present after
# the requested number of attempts, and 2 for an unexpected pgrep failure.
wait_for_rdpilot_daemon_exit() {
  local attempts="$1" uid attempt status
  uid="$(id -u)"
  for ((attempt = 0; attempt < attempts; attempt += 1)); do
    if pgrep -u "$uid" -x rdpilot-daemon >/dev/null; then
      sleep 0.1
    else
      status=$?
      if [ "$status" -eq 1 ]; then
        return 0
      fi
      echo "FAILED: could not inspect this user's rdpilot-daemon (pgrep exit $status)" >&2
      return 2
    fi
  done
  return 1
}

echo "==> [1/6] Using the workspace-selected stable host toolchain"
cargo --version

echo "==> [2/6] Stopping this user's running rdpilot-daemon"
stop_rdpilot_daemon

echo "==> [3/6] Building + installing rdpilot and rdpilot-daemon to $CARGO_ROOT/bin"
cargo install --path "$REPO_ROOT/crates/rdpilot-cli" --root "$CARGO_ROOT"
cargo install --path "$REPO_ROOT/crates/rdpilot-daemon" --root "$CARGO_ROOT"

echo "==> [4/6] Stopping this user's daemon again after the paired install"
stop_rdpilot_daemon

cargo install --path "$REPO_ROOT/crates/rdpilot-mcp" --root "$CARGO_ROOT"

# The daemon downloads the Cua bundle on demand; nothing is staged here.
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
if [ ! -f "$CONFIG_FILE" ]; then
  cp "$REPO_ROOT/crates/rdpilot-config/assets/config.toml.template" "$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"
  echo "Seeded placeholder config at $CONFIG_FILE"
fi
"$CARGO_ROOT/bin/rdpilot" --version
"$CARGO_ROOT/bin/rdpilot" list >/dev/null
"$CARGO_ROOT/bin/rdpilot-mcp" --help >/dev/null
echo "Installed rdpilot, rdpilot-daemon and rdpilot-mcp."
