---
phase: 13-cli-surface
plan: 04
subsystem: daemon-dispatch
tags: [rust, daemon, dispatch, share-root, base64, wire-protocol]
dependency-graph:
  requires: [13-02, 13-03]
  provides: [live-operational-dispatch, share-root-config, base64-pinned-version]
  affects: [13-05, 13-06, 13-07]
tech-stack:
  added:
    - "base64 = \"0.22.1\" (crates.io, github.com/marshallpierce/rust-base64) — pinned in crates/rdpilot-daemon/Cargo.toml, reused verbatim by Plan 13-06's CLI addition"
  patterns:
    - "Registry::call(&session, |s| s.method(...)).await -> Result<T, DaemonError> -> WireResponse::Error(e.into()) on Err, mapped success variant on Ok"
    - "Wire<->SDK conversion via small, pure, per-type free functions (wire_*/sdk_* naming) in dispatch.rs, never duplicated elsewhere"
    - "SystemTime/Duration -> wire string/u64 conversion centralized in registry::iso8601_from_system_time (factored out of iso8601_now) — no duplicated civil-calendar math"
key-files:
  created: []
  modified:
    - crates/rdpilot-config/src/resolved.rs
    - crates/rdpilot-config/src/resolve.rs
    - crates/rdpilot-config/src/lib.rs
    - crates/rdpilot-config/src/paths.rs
    - crates/rdpilot-config/assets/config.toml.template
    - crates/rdpilot-daemon/Cargo.toml
    - crates/rdpilot-daemon/src/dispatch.rs
    - crates/rdpilot-daemon/src/seams.rs
    - crates/rdpilot-daemon/src/error_map.rs
    - crates/rdpilot-daemon/src/registry.rs
decisions:
  - "base64 pinned at 0.22.1 (live crates.io max_stable_version at plan-execution time, 2026-07-11) — the single shared pin for both this plan's daemon addition and Plan 13-06's CLI addition, per the developer-approved legitimacy checkpoint"
  - "share_root_or_default falls back to <platform-data-dir>/rdpilot/transfer-staging via directories::BaseDirs, further falling back to std::env::temp_dir() if no home directory resolves — never panics, always returns a usable path"
  - "resolve_share_root() in dispatch.rs calls rdpilot_config::resolve() with an all-None identity ResolvedConfig override — host/username/password/domain/accept_invalid_certs already arrive resolved on the wire; only share_root (daemon-local operational config) is sourced from config.toml/env at Connect time"
  - "Commit granularity: the rdpilot-config changes (Task 1's config-crate half) landed as one commit; the rdpilot-daemon changes (Task 1's Connect-arm share_root wiring + Task 3's full operational-verb wiring) landed as a second, combined commit — dispatch.rs was authored as one holistic rewrite and splitting it further would have required reconstructing an artificial intermediate state"
metrics:
  duration: ~70min
  completed: 2026-07-11
---

# Phase 13 Plan 04: Daemon Dispatch Wiring + share_root Fix Summary

Wired every operational `rdpilot-ipc::Request` verb in `rdpilot-daemon/src/dispatch.rs` to the live `rdpilot::Session` via `Registry::call` (replacing the Phase-12 `not_implemented_for` stub), and fixed the pre-existing Phase-12 gap where `Connect` never set `ConnectionConfig::share_root` — sourced now from a new `rdpilot-config::share_root_or_default` resolver with a documented platform-data-dir default.

## What Was Built

**Task 1 — `rdpilot-config` `share_root` field + resolver, wired into `Connect`:**

- `ResolvedConfig` gained `share_root: Option<String>` (`#[serde(default)]`; the struct still never derives `Serialize`, D-31 unchanged).
- `apply_overrides` carries `share_root` with the identical "explicit override wins, absent never clobbers a lower layer" semantics every other field already has.
- New `rdpilot_config::share_root_or_default(&ResolvedConfig) -> PathBuf`: returns the configured value verbatim when set; otherwise `<platform-data-dir>/rdpilot/transfer-staging` via `directories::BaseDirs`, with a `std::env::temp_dir()`-based fallback if even `BaseDirs::new()` cannot resolve a home directory — never panics, always returns a non-empty, usable path.
- `config.toml.template` documents the new `share_root` key (commented out, with the default-behavior note).
- `dispatch.rs`'s `Connect` arm now calls a new `resolve_share_root()` helper (uses `rdpilot_config::resolve()` with an all-`None` identity override, then `share_root_or_default`) and sets it via `cfg.share_root(...)` on the `ConnectionConfig` builder before calling `registry.open`. This closes research Pitfall 6 — `put`/`get` no longer fail with `Error::Config("...share_root...")`.

**Task 2 — base64 legitimacy checkpoint (developer pre-approved, mechanically verified this session):**

- Ran `slopcheck install base64` → `[OK]` verdict on crates.io.
- Confirmed package identity directly against the crates.io registry API: `base64`, `max_stable_version: 0.22.1`, `repository: https://github.com/marshallpierce/rust-base64`, `downloads: 1,317,480,750`, `created_at: 2015-12-04` — multi-year, top-tier, matches slopcheck's `[OK]`.
- Pinned `base64 = "0.22.1"` in `crates/rdpilot-daemon/Cargo.toml` (only after this verification, per the gate's sequencing). This exact pin is the one Plan 13-06's CLI addition must reuse.

**Task 3 — every operational dispatch arm wired to the live session:**

`dispatch.rs`'s exhaustive `match` over `Request` now routes every operational verb through `registry.call(&session, |s| s.method(...))`, converting wire DTOs (13-02) to/from the corresponding `rdpilot` SDK types (13-03's `ManagedSession` seam):

| Verb | `ManagedSession` method | `WireResponse` produced |
|---|---|---|
| `Ping` | `ping()` | `Ack` |
| `Screenshot` | `screenshot()` → `.to_png()` → base64 | `Screenshot { png_base64 }` |
| `WindowList` | `get_window_list()` → `wire_window_info` per element | `WindowList { windows }` |
| `ProcessList` | `get_process_tree()` → `wire_process_info` per element | `ProcessList { processes }` |
| `Uia` | `sdk_uia_scope` → `get_uia_tree(hwnd, scope)` → `wire_uia_element` per element | `Uia { elements }` |
| `WorldState` | `sdk_world_state_options` → `world_state(opts)` → `wire_world_state` (timestamp/capture_span/screenshot/window_list/uia conversion) | `WorldState { timestamp, capture_span_ms, screenshot, window_list, uia }` |
| `Mouse` | `sdk_mouse_action` → `send_mouse(action)` | `Ack` |
| `Key` | `sdk_key_action` (full 67-variant `WireKey` → `Key` match) → `send_key(action)` | `Ack` |
| `LaunchProcess` | `launch_process(exe, args, cwd)` | `Pid { pid }` |
| `SetForeground` | `set_foreground_window(hwnd)` | `Ack` |
| `Put` | `upload_file(PathBuf::from(local_path), remote_name)` → `wire_transfer_outcome` | `Transfer(TransferOutcome)` |
| `Get` | `download_file(remote_name, PathBuf::from(local_path))` → `wire_transfer_outcome` | `Transfer(TransferOutcome)` |

`Connect`/`List`/`Disconnect` are unchanged except `Connect`'s new `share_root` wiring (Task 1). The `not_implemented_for` function and every "not implemented" string are gone. The `Request` match remains exhaustive — no `_ =>` wildcard arm.

**Supporting changes:**

- `registry::iso8601_from_system_time(SystemTime) -> String` factored out of the existing `iso8601_now()` (which now delegates to it) so `dispatch.rs`'s `WorldState` arm reuses the exact same civil-calendar conversion rather than duplicating it, per the plan's explicit instruction.
- `seams::DaemonError` gained a `Config(String)` variant for `rdpilot-config` resolution failures (only reachable if the on-disk `config.toml` exists but fails to parse — `resolve()`'s file source is `.required(false)`, so a missing file is not an error), mapped to `WireErrorCode::Internal` in `error_map.rs`. Included in the D-31 planted-secret structural regression test.
- Dispatch's inline test module gained per-verb tests against the 13-03 canned `FakeSession`, plus:
  - a `RichPerceptionSession` fake (populated one-element window/process/UIA data) proving the `wire_window_info`/`wire_process_info`/`wire_uia_element` field-by-field conversions actually run, not just an empty-vec pass-through;
  - a `WithScreenshotSession` fake proving the `WorldState.screenshot` base64 conversion path and the exact ISO-8601 timestamp/millis conversion against a known reference point;
  - a `screenshot_returns_a_response_whose_png_base64_decodes_to_valid_png_bytes` test asserting the decoded bytes carry the PNG magic header;
  - confirmation that an unknown session still resolves to `SessionNotFound` for an operational verb after the rewrite (`Registry::call`'s existing guarantee).

## Verification

Ran on the substitute native-Linux target (fully offline environment; no Windows/live-RDP target available this session — the plan's own toolchain note anticipated this):

```
RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build --workspace --target x86_64-unknown-linux-gnu
RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --workspace --target x86_64-unknown-linux-gnu
```

- Full workspace build: clean (only one pre-existing, unrelated warning in `rdpilot::input` — not touched by this plan).
- Full workspace test suite: **129 + 9 + 66 + 0 + 1 + 3 + 2 + 1 + 38 = all passing, 0 failed** (28 `rdpilot` live-session tests correctly `ignored` offline — pre-existing, unrelated to this plan).
- `rdpilot-daemon` lib alone: 66/66 passing, including all 17 new/updated `dispatch::tests::*` per-verb tests.
- `rdpilot-config`: 9/9 passing, including the 3 new `share_root_or_default`/`apply_overrides` tests.
- Confirmed via `grep`: `dispatch.rs` contains no `not_implemented_for` function and no `"not implemented"` string literal; `registry.call` appears 12 times (once per operational arm); `.share_root(` appears in the `Connect` arm.
- Non-regression: `tests/thread_leak_soak.rs`'s non-soak variant (`thread_count_returns_to_baseline_after_a_few_cycles`), `tests/registry_concurrency.rs` (both N-way concurrency tests), and `tests/autostart_lifecycle.rs` (registered, correctly `ignored` by default — spawns a real child process, matching its pre-existing design) all still pass/register cleanly.
- `cargo clippy --lib -p rdpilot-daemon -p rdpilot-config` (no `--all-targets`, i.e. production code only): clean, zero warnings from this plan's code.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 — blocking] `assert_eq!(x, None)` on non-`PartialEq` wire types**
- **Found during:** Task 3, first test run.
- **Issue:** Two new `dispatch::tests` used `assert_eq!(window_list, None)`/`assert_eq!(uia, None)` against `Option<Vec<WireWindowInfo>>`/`Option<Vec<(u64, Vec<WireUiaElement>)>>` — `WireWindowInfo`/`WireUiaElement` (from `rdpilot-ipc`) do not derive `PartialEq`, so this failed to compile (E0369).
- **Fix:** Switched to `assert!(x.is_none(), ...)`.
- **Files modified:** `crates/rdpilot-daemon/src/dispatch.rs`.

**2. [Rule 1 — bug] Test assertions assumed a populated fake that was actually empty**
- **Found during:** Task 3, first test run.
- **Issue:** Initial `window_list`/`process_list`/`uia` dispatch tests asserted a one-element vec against the shared `FakeSession` (13-03), which actually returns `vec![]` for all three perception methods — the tests failed with `left: 0, right: 1`.
- **Fix:** Split into two test tiers: lightweight tests against the existing empty-vec `FakeSession` (proving the arm routes to the right method and response variant), plus a new `RichPerceptionSession` fake with one populated element per type (proving the field-by-field wire conversion itself is correct — the plan's acceptance criteria explicitly called for verifying "the fake's one-element vec").
- **Files modified:** `crates/rdpilot-daemon/src/dispatch.rs`.

**3. [Rule 3 — blocking] Unused imports in the production (non-test) compilation unit**
- **Found during:** Task 3, `cargo build --workspace` check.
- **Issue:** `SessionId`/`WireError`/`WireErrorCode` were only referenced by the test module (via `use super::*`), producing `unused_imports` warnings in the production build.
- **Fix:** Moved those three imports into the test module's own `use rdpilot_ipc::{...}` statement, leaving the top-level import list production-only.
- **Files modified:** `crates/rdpilot-daemon/src/dispatch.rs`.

No architectural (Rule 4) deviations — the plan's own research already specified the exact `Registry::call` pattern, the `ManagedSession` seam, and the `share_root_or_default` shape; this plan implemented them as designed.

### Toolchain Substitution (per plan instruction, not a deviation)

Ran on `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu` / `--target x86_64-unknown-linux-gnu` (native Linux substitute) rather than the eventual Windows target, per the plan's explicit "fully offline" toolchain note. Real Windows/live-RDP behavior for `put`/`get`'s `share_root` fix and every operational verb is deferred to the batched Phase 15 live gate, as the plan's `<success_criteria>` states.

## Known Stubs

None. Every arm this plan owns calls a real `rdpilot::Session` method through the live seam; no hardcoded/placeholder response remains for any operational verb.

## Threat Flags

None beyond what the plan's own `<threat_model>` already registered (T-13-10/T-13-11/T-13-12/T-13-SC) — no new network endpoint, auth path, or trust-boundary-crossing file-access pattern was introduced beyond what the plan described and pre-registered.

## Self-Check: PASSED

- `crates/rdpilot-config/src/resolved.rs` — FOUND, contains `share_root: Option<String>`.
- `crates/rdpilot-config/src/resolve.rs` — FOUND, contains `share_root_or_default`.
- `crates/rdpilot-daemon/src/dispatch.rs` — FOUND, contains `registry.call` (12 occurrences) and `.share_root(`; contains no `not_implemented_for` and no `"not implemented"` string.
- `crates/rdpilot-daemon/Cargo.toml` — FOUND, contains `base64 = "0.22.1"`.
- Commit `1826414` (`feat(13-04): add rdpilot-config share_root field + resolver`) — present in `git log --oneline`.
- Commit `6e23ab2` (`feat(13-04): wire every operational dispatch arm to the live session`) — present in `git log --oneline`.
- `cargo test --workspace` (substitute target): 0 failed, all suites green.
