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
AGENTS_SKILL_DIR="$HOME/.agents/skills/rdpilot"
CLAUDE_SKILLS_DIR="$HOME/.claude/skills"
CLAUDE_SKILL_LINK="$CLAUDE_SKILLS_DIR/rdpilot"

# Ensure the install destination is on PATH for this script's own smoke
# test below, regardless of whether the invoking shell already sourced
# `~/.cargo/env` (e.g. non-login/non-interactive shells).
export PATH="$CARGO_ROOT/bin:$PATH"

echo "==> [1/5] Detecting host target triple"
# This box's rust-toolchain.toml + .cargo/config.toml are pinned for the
# project's original ARM64-Windows dev machine (stable-x86_64-pc-windows-gnu
# channel, forced Windows-GNU cross target). A bare `cargo build`/`cargo
# install` fails outright here (toolchain-channel error) or cross-compiles
# to Windows. Both the `+stable` toolchain override AND an explicit
# `--target` override are REQUIRED on every cargo invocation below.
HOST_TRIPLE="$(rustc +stable -vV | awk '/^host:/ {print $2}')"
echo "    host triple: $HOST_TRIPLE"

echo "==> [2/5] Building + installing rdpilot and rdpilot-daemon to $CARGO_ROOT/bin"
cargo +stable install --path "$REPO_ROOT/crates/rdpilot-cli" \
  --target "$HOST_TRIPLE" --root "$CARGO_ROOT"
cargo +stable install --path "$REPO_ROOT/crates/rdpilot-daemon" \
  --target "$HOST_TRIPLE" --root "$CARGO_ROOT"

echo "==> [3/5] Seeding config (idempotent — never overwrites an existing file)"
mkdir -p "$CONFIG_DIR"
chmod 700 "$CONFIG_DIR"
if [ ! -f "$CONFIG_FILE" ]; then
  cp "$REPO_ROOT/crates/rdpilot-config/assets/config.toml.template" "$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"
  echo "    seeded placeholder-only config at $CONFIG_FILE"
else
  echo "    $CONFIG_FILE already exists — leaving it untouched"
fi

echo "==> [4/5] Installing the skill to both discovery surfaces"
mkdir -p "$AGENTS_SKILL_DIR"
cp "$REPO_ROOT/.claude/skills/rdpilot/SKILL.md" "$AGENTS_SKILL_DIR/SKILL.md"
mkdir -p "$CLAUDE_SKILLS_DIR"
ln -sf "$AGENTS_SKILL_DIR" "$CLAUDE_SKILL_LINK"
echo "    real copy: $AGENTS_SKILL_DIR/SKILL.md"
echo "    symlink:   $CLAUDE_SKILL_LINK -> $AGENTS_SKILL_DIR"

echo "==> [5/5] Smoke test (D-8): confirming the CLI -> daemon IPC path works"
if ! rdpilot --version; then
  echo "FAILED: 'rdpilot --version' did not exit 0" >&2
  exit 1
fi
if ! rdpilot list >/dev/null; then
  echo "FAILED: 'rdpilot list' did not exit 0 (daemon auto-start / IPC path broken)" >&2
  exit 1
fi

echo ""
echo "rdpilot installed successfully."
echo "Config file: $CONFIG_FILE"
echo "Next steps: edit $CONFIG_FILE (or export RDPILOT_HOST / RDPILOT_USERNAME /"
echo "RDPILOT_PASSWORD) with your OWN target host + credentials before running"
echo "'rdpilot connect'."
