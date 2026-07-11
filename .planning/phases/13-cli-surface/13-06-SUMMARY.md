---
phase: 13-cli-surface
plan: 06
subsystem: cli
tags: [rust, clap, cli, base64, wire-protocol, ipc]

requires:
  - phase: 13-cli-surface (13-02/13-03/13-04)
    provides: perception/input wire DTOs, live operational dispatch, base64=0.22.1 legitimacy-gated pin
  - phase: 13-cli-surface (13-05)
    provides: rdpilot-cli crate scaffold, round_trip transport client, exit_codes/CliError, render_table
provides:
  - "The full CLI-02 perception verb set: perceive screenshot/world-state/uia/window list/process list, each targeting a required --session"
  - "The full CLI-02 input+launch verb set: input click/scroll/drag/type/key/launch/foreground, each targeting a required --session"
  - "base64 dependency on rdpilot-cli, reusing the exact 0.22.1 pin Plan 13-04 legitimacy-gated on rdpilot-daemon"
  - "tests/cli_verbs.rs: the offline CLI-02 integration proof, round-tripping every verb against the real daemon binary's canned FakeTestSession"
affects: [13-07, phase-14-mcp-server, phase-15-proof-harnesses]

tech-stack:
  added:
    - "base64 = \"0.22.1\" on rdpilot-cli (verbatim reuse of Plan 13-04's legitimacy-gated pin on rdpilot-daemon, no second checkpoint)"
  patterns:
    - "Grouped-only clap subcommand families (Perceive/Input) per research D-13.1 -- no flat top-level spelling, unlike Session's flat+grouped duality"
    - "Every session-scoped leaf args struct carries a required session: String field (D-29, research Pattern 2) -- clap's own parse-error machinery enforces --session presence per-command"
    - "screenshot/world-state --output: base64-decode png_base64 then std::fs::write, binary bytes never printed (D-13.1) -- stdout carries only a text/JSON summary"
    - "Unknown --combo key names map to a legible CliError::Internal, never a panic (T-13-17)"
    - "Window/process/uia table-rendering helpers added to render/table.rs, reusing the existing hand-rolled render_table column printer"

key-files:
  created:
    - crates/rdpilot-cli/src/verbs/perceive.rs
    - crates/rdpilot-cli/src/verbs/input.rs
    - crates/rdpilot-cli/tests/cli_verbs.rs
  modified:
    - crates/rdpilot-cli/Cargo.toml
    - crates/rdpilot-cli/src/cli.rs
    - crates/rdpilot-cli/src/main.rs
    - crates/rdpilot-cli/src/verbs/mod.rs
    - crates/rdpilot-cli/src/render/table.rs
    - crates/rdpilot-cli/src/render/mod.rs

key-decisions:
  - "base64 pinned at 0.22.1 in crates/rdpilot-cli/Cargo.toml, verified identical to crates/rdpilot-daemon/Cargo.toml's Plan-13-04-approved pin -- no second legitimacy checkpoint opened"
  - "Perceive/Input are grouped-only clap subcommand families (no flat top-level rdpilot screenshot/click spelling), per research D-13.1"
  - "click gets a single leaf with a --double flag (not a separate double-click leaf) selecting WireMouseAction::DoubleClick vs Click"
  - "world-state's screenshot component only writes to disk when the caller passes BOTH --screenshot and --output; --screenshot alone with no --output reports \"captured\" without a write, and window_list/uia are rendered as tables/JSON when requested"
  - "--combo key names are matched case-insensitively against WireKey's variant spellings plus a few common aliases (esc/escape, del/delete, win/windows/super); an unrecognized token is a CliError::Internal, never a panic"

requirements-completed: [CLI-02]

duration: ~7min
completed: 2026-07-11
---

# Phase 13 Plan 06: CLI Perception + Input + Launch Verb Set Summary

**The full CLI-02 verb set (screenshot/world-state/uia/window+process list, click/scroll/drag/type/key/launch/foreground) wired into the clap tree against a required `--session`, base64-decoding screenshot bytes to `--output` only, proven end-to-end offline against the real daemon binary's canned fake session.**

## Performance

- **Duration:** ~7 min
- **Started:** 2026-07-11T08:37:18Z
- **Completed:** 2026-07-11T08:44:20Z
- **Tasks:** 3 (Task 1 was a non-blocking identification-only confirmation, folded into Task 2's commit)
- **Files modified:** 9 (3 created, 6 modified)

## Accomplishments

- Extended `crates/rdpilot-cli/src/cli.rs` with two grouped-only clap subcommand families: `perceive` (screenshot/world-state/uia/window list/process list) and `input` (click/scroll/drag/type/key/launch/foreground), each leaf requiring `--session` (D-29).
- `perceive screenshot --session S --output <path>` base64-decodes the daemon's `png_base64` and writes raw PNG bytes to disk only — verified live (subprocess test) that image bytes never appear on stdout.
- `perceive world-state` supports a-la-carte `--screenshot`/`--window-list`/`--uia-mode <none|foreground|all|hwnd>` (+ repeatable `--hwnd`) and an optional `--output` for the embedded screenshot.
- `perceive uia --hwnd N --scope <children|subtree>` (+ `--max-depth` for subtree) renders the UIA element tree.
- `perceive window list` / `perceive process list` render tables by default, JSON arrays under `--json`.
- `input click/scroll/drag/type/key` build the correct `WireMouseAction`/`WireKeyAction` and expect `Ack`; `input launch` prints the daemon-assigned pid; `input foreground` acks.
- Added `crates/rdpilot-cli/tests/cli_verbs.rs`: an offline integration test that spawns the compiled `rdpilot` binary as a subprocess, auto-starting the real `rdpilot-daemon` binary with `RDPILOT_DAEMON_TEST_CONNECTOR=1`, and round-trips every CLI-02 verb against the daemon's canned `FakeTestSession` — confirms the fake's exact canned rows (hwnd=1/"Notepad"/pid=1234 window, pid=1234/"notepad.exe" process, id="42"/"Button" UIA element, pid=4242 launch) and that a written screenshot file starts with real PNG magic bytes (`0x89 P N G`) decoded from base64, never the raw bytes on stdout.
- Confirmed base64 stays pinned at the exact `0.22.1` version Plan 13-04 legitimacy-gated on `rdpilot-daemon` — `crates/rdpilot-cli/Cargo.toml` and `crates/rdpilot-daemon/Cargo.toml` show the identical pin, no second human checkpoint.
- Re-verified the thin-client invariant after adding `base64`: `cargo tree -p rdpilot-cli` still shows zero `ironrdp`/`rustls`/`rdpilot-daemon` in the dependency graph.

## Task Commits

Each task was committed atomically (Task 1 folded into Task 2's commit — it produced no artifact of its own, only identified the pin for Task 2 to reuse verbatim):

1. **Task 1 + Task 2: base64 confirmation + perception verbs (screenshot/world-state/uia/window-list/process-list)** - `cfa9648` (feat)
2. **Task 3: input+launch verbs (click/scroll/drag/type/key/launch/foreground) + the offline CLI-02 integration proof** - `3793333` (feat)

**Plan metadata:** pending (this commit)

## Files Created/Modified

- `crates/rdpilot-cli/src/verbs/perceive.rs` - screenshot/world-state/uia/window-list/process-list handlers
- `crates/rdpilot-cli/src/verbs/input.rs` - click/scroll/drag/type/key/launch/foreground handlers + `--combo` key-name parser
- `crates/rdpilot-cli/tests/cli_verbs.rs` - offline CLI-02 integration proof (real daemon binary + canned fake session)
- `crates/rdpilot-cli/Cargo.toml` - added `base64 = "0.22.1"` (verbatim Plan 13-04 pin)
- `crates/rdpilot-cli/src/cli.rs` - `PerceiveCmd`/`InputCmd` grouped-only subcommand families + their leaf arg structs
- `crates/rdpilot-cli/src/main.rs` - dispatch arms for every new verb
- `crates/rdpilot-cli/src/verbs/mod.rs` - registered the new `perceive`/`input` modules
- `crates/rdpilot-cli/src/render/table.rs` - `render_window_table`/`render_process_table`/`render_uia_table` helpers
- `crates/rdpilot-cli/src/render/mod.rs` - re-exported the new table helpers

## Decisions Made

- base64 reused Plan 13-04's exact `0.22.1` pin verbatim — no second legitimacy checkpoint, per the plan's binding constraint.
- `perceive`/`input` are grouped-only (research D-13.1) — no flat top-level spelling like `session`/`put`/`get` have.
- `click` uses one leaf with a `--double` flag rather than a separate `double-click` leaf, keeping the button-selection logic (`--button`) shared between single and double clicks.
- `--combo` key-name matching is case-insensitive against `WireKey`'s variant spellings plus a handful of common aliases (`esc`/`escape`, `del`/`delete`, `win`/`windows`/`super`, digit names as either `digit0` or `0`).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] clap rejected negative `--dy` values as an unrecognized flag**
- **Found during:** Task 3 (`cli_verbs.rs` — `input scroll --dy -120` failed with `error: unexpected argument '-1' found`)
- **Issue:** clap's default argument parser treats a value starting with `-` as a potential short-flag combination, not a negative number, so `--dy -120` was misparsed.
- **Fix:** Added `allow_hyphen_values = true` to `ScrollArgs::dy` in `crates/rdpilot-cli/src/cli.rs`, which tells clap to accept the raw hyphen-prefixed token as this arg's value.
- **Files modified:** `crates/rdpilot-cli/src/cli.rs`
- **Verification:** `cargo test -p rdpilot-cli` — `cli_verbs.rs`'s `input scroll --dy -120` step passes.
- **Committed in:** `3793333` (Task 3 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Necessary for `input scroll` to accept negative wheel deltas (a normal, expected input) at all. No scope creep.

## Issues Encountered

- **Toolchain substitution (documented per this plan's binding constraint):** the environment has no real Windows target; all builds/tests ran via `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build/test --target x86_64-unknown-linux-gnu` (the native Linux substitute target). This is the same substitution Plans 13-01 through 13-05 used; not a deviation from those plans' established pattern.
- **Pre-existing, out-of-scope oddity noted but not fixed (scope boundary):** `crates/rdpilot-daemon/src/server.rs`'s `FakeTestSession::get_process_tree` canned `path` field literal contains an embedded raw newline character inside its `r"..."` raw string (`r"C:\Windows` + literal newline + `otepad.exe"`), evidently a pre-existing typo from Plan 13-04's authoring, not something this plan's tasks touch or introduce. `cli_verbs.rs`'s process-list assertion only checks `pid`/`name`, so this did not affect this plan's verification; flagged here for awareness, not auto-fixed (out of this plan's file scope per the Rule Priority scope boundary).

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- CLI-02 is now fully implemented and offline-proven: `rdpilot perceive {screenshot,world-state,uia,window list,process list}` and `rdpilot input {click,scroll,drag,type,key,launch,foreground}` all round-trip against the daemon's real dispatch/registry with a required `--session`.
- Plan 13-07 (CLI-03: `put`/`get` no-clobber + path absolutization + distinct error taxonomy/exit codes) can proceed — it shares `cli.rs`'s `SessionArg`/flat-vs-grouped pattern and `connect.rs`'s `round_trip` client, both unchanged by this plan.
- Real-Windows semantics (actual screenshot pixel content, genuine click-coordinate landing, real UIA tree shape) remain deferred to the Phase 15 batched live gate — `tests/cli_verbs.rs`'s `#[ignore]`-gated placeholder test documents exactly what belongs there.
- The pre-existing embedded-newline oddity in `FakeTestSession::get_process_tree`'s canned `path` (noted above under Issues Encountered) is a candidate for a small follow-up fix whenever `rdpilot-daemon/src/server.rs` is next touched — not blocking for CLI-02 or Phase 14/15.

---
*Phase: 13-cli-surface*
*Completed: 2026-07-11*
