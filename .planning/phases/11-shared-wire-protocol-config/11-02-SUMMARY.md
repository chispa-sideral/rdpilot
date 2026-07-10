---
phase: 11-shared-wire-protocol-config
plan: 02
subsystem: rdpilot-config (layered config resolution crate)
tags: [config, layered-resolution, credential-redaction, platform-paths]
dependency-graph:
  requires: [11-01 (workspace Cargo.toml member list precedent)]
  provides: [rdpilot-config-crate, ResolvedConfig, ConfigError, config_file_path, resolve, apply_overrides, CONFIG_TEMPLATE]
  affects: [Phase 12 daemon (ResolvedConfig -> rdpilot::ConnectionConfig conversion), Phase 13 CLI, Phase 14 MCP]
tech-stack:
  added: [rdpilot-config workspace crate (config 0.15.25 toml-only, directories 6.0.0, serde, thiserror)]
  patterns: [BaseDirs (not ProjectDirs) platform config path, Deserialize-only credential-free-by-construction config type, plain-Option override layer instead of config::Source]
key-files:
  created:
    - crates/rdpilot-config/Cargo.toml
    - crates/rdpilot-config/src/lib.rs
    - crates/rdpilot-config/src/resolved.rs
    - crates/rdpilot-config/src/paths.rs
    - crates/rdpilot-config/src/resolve.rs
    - crates/rdpilot-config/assets/config.toml.template
  modified:
    - Cargo.toml (workspace members += crates/rdpilot-config)
    - .gitignore (defensive config.toml / .rdpilot.toml entries)
decisions:
  - "ResolvedConfig derives ONLY Debug/Clone/Deserialize — no Serialize at all, extending D-31 one layer before the rdpilot-ipc wire boundary"
  - "Override layer (flag/MCP-init) implemented as a plain Option-based apply_overrides pass, not routed through config::Source (RESEARCH Alternatives)"
metrics:
  duration: "~30 minutes"
  completed: 2026-07-11
---

# Phase 11 Plan 02: rdpilot-config layered configuration crate Summary

New `rdpilot-config` workspace crate resolving target host + credentials through a deterministic file -> env -> flag/MCP-init precedence, backed by a platform-conventional TOML config file, with the resolved config type structurally incapable of being wire-serialized.

## What Was Built

- **`crates/rdpilot-config/`** — a new workspace member (`Cargo.toml` members list now `["crates/rdpilot", "crates/rdpilot-ipc", "crates/rdpilot-config"]`), pinned to `config = "0.15.25"` (`default-features = false, features = ["toml"]`) and `directories = "6.0.0"` — the exact D-26 versions, no bump. `cargo tree -p rdpilot-config` confirms neither `rdpilot` nor any `ironrdp*` crate appears in its dependency graph (Decision 1).
- **`ResolvedConfig` + `ConfigError`** (`resolved.rs`): `ResolvedConfig { host, port, username, password, domain, accept_invalid_certs }`, all `Option`/`bool`, deriving **only** `Debug, Clone, Deserialize` — no `Serialize` anywhere, so the type cannot be wire-serialized by accident even before the `rdpilot-ipc` boundary is considered (D-31, applied one layer earlier). `ConfigError` is an owned `thiserror` enum (`File`, `PathResolution`) — no `config::ConfigError`/`toml::*` type ever appears in a public signature (D-09).
- **`config_file_path()`** (`paths.rs`): `directories::BaseDirs::new().map(|b| b.config_dir().join("rdpilot").join("config.toml"))` — deliberately `BaseDirs`, not `ProjectDirs` (which would insert an unwanted extra `config` subfolder on Windows). The test asserts the path ends in exactly one `rdpilot`/`config.toml` pair with zero doubled `config` segments.
- **`CONFIG_TEMPLATE`** (`assets/config.toml.template`, exposed via `include_str!`): a fully-commented template documenting all six D-27 keys (`host`, `port`, `username`, `password`, `domain`, `accept_invalid_certs`), every key commented out so an unedited copy parses to an all-defaults `ResolvedConfig`.
- **`resolve()` / `apply_overrides()`** (`resolve.rs`): `resolve(overrides)` reads the platform config file (optional, `.required(false)`), layers `RDPILOT_`-prefixed env (`__` nesting separator, added last so env beats file), then applies `apply_overrides` as the third layer. `apply_overrides(base, overrides)` is a plain `Option`-based pass — deliberately NOT routed through `config::Source`, since Phase 13's `clap` and Phase 14's `rmcp` already hand over typed values.
- **`.gitignore`** gained defensive `config.toml`/`.rdpilot.toml` entries — the real config file lives at the platform config dir, structurally outside the repo tree; these entries only guard a contributor's local override copy.

## Deviations from Plan

None — plan executed exactly as written. The `config` crate's `Environment::default().source(Some(map))` explicit-source-injection API (verified live against Context7 docs before implementation) matched the RESEARCH doc's pattern exactly; no API-mismatch fixes were needed.

### Minor scope-internal adjustment

**[Rule 3 - blocking fix] `#[allow(dead_code)]` on `ConfigError::file`, `#[cfg(test)]` on the `empty_overrides` test helper**
- **Found during:** Task 1's clippy pass (`-D warnings`) flagged `ConfigError::file` as dead code (only reachable from the `#[cfg(test)]` module at that point in Task 1, before `paths.rs`/`resolve.rs` added their own non-test call sites in Tasks 2/3), and Task 3's clippy pass would have flagged `resolve.rs`'s `empty_overrides()` test-only builder the same way.
- **Fix:** Annotated `ConfigError::file` with `#[allow(dead_code)]` and a comment naming the future non-test call sites (Task 2/3), mirroring the existing `rdpilot` crate's own `#[allow(dead_code)] // Consumed by Plan ...` convention (`crates/rdpilot/src/error.rs`); gated `empty_overrides()` with `#[cfg(test)]` since it is genuinely test-only.
- **Files modified:** `crates/rdpilot-config/src/resolved.rs`, `crates/rdpilot-config/src/resolve.rs`
- **Commit:** `568410e` (Task 1), `b8b24ba` (Task 3)

## Verification

- `cargo +stable-x86_64-unknown-linux-gnu test -p rdpilot-config --target x86_64-unknown-linux-gnu` — 6 tests, all green (partial-TOML deserialize, platform config path convention, template parse + key-name coverage, CONFIG-01 layered precedence, `apply_overrides` no-clobber).
- `cargo +stable-x86_64-unknown-linux-gnu tree -p rdpilot-config --target x86_64-unknown-linux-gnu` — dependency graph contains only `serde`, `config`, `directories`, `thiserror` and their transitive deps (`toml`, `winnow`, `dirs-sys`, etc.); no `rdpilot`/`ironrdp*` node.
- `cargo +stable-x86_64-unknown-linux-gnu build --workspace --target x86_64-unknown-linux-gnu` — full three-crate workspace builds; only warning is the pre-existing unrelated unused import in `crates/rdpilot/src/input.rs`.
- `cargo +stable-x86_64-unknown-linux-gnu test --workspace --target x86_64-unknown-linux-gnu` — **129 rdpilot tests + 6 rdpilot-config tests + 13 rdpilot-ipc tests, all green; 28 pre-existing live-gate rdpilot tests correctly ignored** (RDPILOT_LIVE=1 gated, unrelated to this plan). The Plan 11-01 workspace-member edit did not regress the existing `rdpilot` crate's suite.
- `cargo +stable-x86_64-unknown-linux-gnu clippy -p rdpilot-config --target x86_64-unknown-linux-gnu -- -D warnings` — clean, zero warnings.
- Manual grep confirms: zero `.unwrap(`/`.expect(` in any file in `crates/rdpilot-config/src/`; zero `std::env::set_var` calls in the test code (CONFIG-01's precedence test uses `Environment::source(Some(map))` explicit injection instead, so it never touches the real ambient process environment or is subject to cross-test races); the override layer is not routed through `config::Source` anywhere in `resolve.rs`.

### Toolchain substitution (same as 11-01, established prior-phase pattern)

Identical substitution as documented in `11-01-SUMMARY.md`: this host only has `stable-x86_64-unknown-linux-gnu` installed (the workspace's pinned `stable-x86_64-pc-windows-gnu` is absent), so all verification ran via `cargo +stable-x86_64-unknown-linux-gnu ... --target x86_64-unknown-linux-gnu`. Legitimate for `rdpilot-config`: the crate is pure `serde`/`config`/`directories`/`thiserror` with zero `cfg(windows)` code. The genuine `x86_64-pc-windows-gnu` build remains unconfirmed on this host, consistent with every prior phase.

## Requirements Delivered

- **CONFIG-01:** satisfied — `layered_precedence` proves override > env > file for `host` with injected, deterministic sources (no real home dir or ambient env involved); `apply_overrides_none_does_not_clobber` proves a `None` override never destroys a lower layer's value.
- **CONFIG-02:** satisfied — `config_file_path_matches_platform_convention` proves the exact D-27 platform path (via `BaseDirs`, guarding the `ProjectDirs` doubled-`config`-segment pitfall); `template_parses_into_resolved_config` + `template_documents_every_d27_key` prove the shipped template is both valid and self-explanatory; defensive `.gitignore` entries are in place.

## Self-Check: PASSED

- `crates/rdpilot-config/Cargo.toml` — FOUND
- `crates/rdpilot-config/src/lib.rs` — FOUND
- `crates/rdpilot-config/src/resolved.rs` — FOUND
- `crates/rdpilot-config/src/paths.rs` — FOUND
- `crates/rdpilot-config/src/resolve.rs` — FOUND
- `crates/rdpilot-config/assets/config.toml.template` — FOUND
- Commit `568410e` (Task 1) — FOUND in `git log --oneline --all`
- Commit `69a596f` (Task 2) — FOUND in `git log --oneline --all`
- Commit `b8b24ba` (Task 3) — FOUND in `git log --oneline --all`
