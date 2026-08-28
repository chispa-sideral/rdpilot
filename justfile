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

# Leases one short-lived shared-infrastructure Azure Windows VM, bootstraps
# missing official Rust/MSVC tools, then runs the Windows-only DACL gate.
windows-dacl:
    ./scripts/live/run-crabbox-windows-dacl.sh

# Same lease and bootstrap, without the DACL test; useful to prove image setup.
windows-build-host:
    RDPILOT_DACL_SETUP_ONLY=1 ./scripts/live/run-crabbox-windows-dacl.sh

# Requires a fresh caller-owned connection JSON; never falls back to .secrets.
rdp-e2e connection_file:
    ./scripts/live/run-rdp-e2e.sh "{{connection_file}}"
