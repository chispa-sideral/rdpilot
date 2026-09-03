# Project workflow discovery. Recipes stay thin: their scripts own validation,
# credential handling, bounded execution, and cleanup.

default: help

help:
    @just --list --unsorted

# Formatting and offline compilation checks.
fmt:
    cargo fmt --all

check:
    cargo check --workspace --all-targets

# The daemon test suite needs a writable per-run runtime directory on Linux.
daemon-test:
    runtime_dir="$(mktemp -d)"; trap 'rm -rf "$runtime_dir"' EXIT; XDG_RUNTIME_DIR="$runtime_dir" cargo test -p rdpilot-daemon

install-local:
    ./scripts/install-local.sh

# Runs the Windows-only DACL gate on the GitHub-hosted Windows runner.
windows-dacl:
    pwsh -NoProfile -File ./scripts/ci/run-windows-dacl.ps1

# Creates, uses, and removes a bounded DevTest Labs target using workflow env.
rdp-e2e state_file:
    python3 ./scripts/e2e/devtest-rdp-e2e.py run --state "{{state_file}}"
