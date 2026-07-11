---
phase: 14-mcp-server-surface
plan: 03
subsystem: rdpilot-mcp
tags: [mcp, computer-use, coordinate-bridge, dispatch]
dependency-graph:
  requires: [14-01, 14-02]
  provides: [computer-tool, scale-to-native, computer-action-schema]
  affects: [14-04, 14-05]
tech-stack:
  added: []
  patterns:
    - "pure coordinate-bridge function, independently unit-tested against edges/corners/near-corners without a live session"
    - "internally-tagged serde enum mirroring an external vendor schema's field names verbatim, plus an extra required top-level field via #[serde(flatten)]"
    - "explicit tool-error rejection for unsupported actions instead of silent no-op"
key-files:
  created:
    - crates/rdpilot-mcp/src/computer/mod.rs
    - crates/rdpilot-mcp/src/computer/scale.rs
    - crates/rdpilot-mcp/src/computer/dispatch.rs
    - crates/rdpilot-mcp/tests/scale_to_native.rs
  modified:
    - crates/rdpilot-mcp/src/handler.rs
    - crates/rdpilot-mcp/src/main.rs
    - crates/rdpilot-mcp/src/error.rs
decisions:
  - "scale_to_native rounds via plain f64::round() (half away from zero) then clamps to [0, native_dim-1] as the LAST step before the u16 cast — the bridge itself, not Session::check_bounds, is the safety net (Pitfall 2)."
  - "Added McpError::InvalidArgument (Rule 2): none of the existing WireErrorCode-derived variants correctly classed a client-local validation failure (empty session, unsupported action, duration-cap breach, unknown key token)."
  - "tests/scale_to_native.rs pulls computer/scale.rs in via #[path] rather than adding a [lib] target to this bin-only crate — scale_to_native has zero crate-internal dependencies, so this keeps the plan's exact files_modified footprint."
  - "Click-action `text` (modifier-key) field is accepted for Anthropic field-name parity (Pitfall 3/A2) but not wired to any WireMouseAction equivalent — documented no-op, not one of the 3 named gaps, matching research's own Open Questions framing."
  - "Scroll's `coordinate` field is Option<[u32;2]> in the schema (parity with Anthropic) but required in practice by dispatch — no cursor-position tracking exists to default to (a direct consequence of the cursor_position gap)."
metrics:
  duration: ~45 min
  completed: 2026-07-11
---

# Phase 14 Plan 03: Computer-Use Mega-Tool + Coordinate Bridge Summary

Implemented the Anthropic `computer_20250124`-compatible `computer` mega-tool: the BLOCKING `scale_to_native` pure coordinate bridge (MCP-04), the schema-discriminated `ComputerAction` enum whose field names mirror Anthropic's reference verbatim, and the full action-to-`Request` dispatch table with the three genuine SDK-capability gaps rejected explicitly.

## What Was Built

**`crates/rdpilot-mcp/src/computer/scale.rs`** — `scale_to_native(x, y, native_w, native_h) -> (u16, u16)`, a pure function with zero dependency on any session/handler/rmcp/rdpilot-ipc type. Bridges the FIXED advertised 1280x800 (`ADVERTISED_WIDTH`/`ADVERTISED_HEIGHT`, D-14.2 LOCKED) space to a session's native 96-DPI pixels: floating-point scale factors, `f64::round()` (round-half-away-from-zero — the required tie-break), then `.clamp(0.0, native_dim - 1.0)` as the last step before the `u16` cast, with `saturating_sub` guarding the degenerate `native_dim == 0` case from a clamp-min-greater-than-max panic.

**`crates/rdpilot-mcp/tests/scale_to_native.rs`** — the MCP-04 BLOCKING integration test. Pulls `computer/scale.rs` in via `#[path]` (this crate has no `[lib]` target) and asserts: zero corner, the corrected exact-tie near-corner vector `scale_to_native(1279, 799, 1920, 1080) == (1919, 1079)`, the symmetric top-right/bottom-left near-corners, center sanity, the exclusive advertised-edge clamp (`(1280, 800)` never lands on/past native width/height) across four native resolutions, and square-scale identity.

**`crates/rdpilot-mcp/src/computer/mod.rs`** — `ComputerAction` (16 `computer_20250124` variants, `#[serde(tag = "action", rename_all = "snake_case")]`, field names — `coordinate`, `start_coordinate`, `text`, `scroll_direction`, `scroll_amount`, `duration` — mirroring Anthropic's reference verbatim), `ScrollDirection`, and `ComputerArgs { session: String, #[serde(flatten)] action: ComputerAction }` (D-29: `session` is an extra required top-level field this server's own schema adds).

**`crates/rdpilot-mcp/src/computer/dispatch.rs`** — `RdpilotMcpHandler::dispatch_computer`, the full action→`Request` mapping: coordinate-bearing actions first round-trip `Request::DesktopSize` to source real native dims, then scale through `scale_to_native`, then issue `Request::Mouse`/`Request::Key`/`Request::Screenshot` via `round_trip_bounded(.., timeouts::FAST)`. `triple_click` is three sequential `Click` round trips ~100ms apart (no atomic primitive). `key`/`hold_key` split `"+"`-joined combos through the canonical `rdpilot_ipc::parse_wire_key` table. `screenshot` passes the base64 PNG straight through as an rmcp image content block (no decode/re-encode).

**`crates/rdpilot-mcp/src/handler.rs`** — registers `computer` as a `#[tool]` method: a thin adapter extracting `Parameters<ComputerArgs>`, calling `dispatch_computer`, and mapping `McpError` to `rmcp::ErrorData` (D-28).

## The Three Explicit Gap Rejections (T-14-10, tested by name)

1. **`left_mouse_down`/`left_mouse_up`** — `WireMouseAction` has no press-only/release-only primitive. Rejected with a message naming both the action and `left_click_drag` as the supported alternative.
2. **`cursor_position`** — no SDK pointer-position getter exists. Rejected with a message pointing at `screenshot` as the way to observe the pointer.
3. **Horizontal `scroll_direction` (`left`/`right`)** — rdpilot's `Scroll` wire verb carries only vertical `dy` (D-3.3). Rejected with a message naming the vertical-only limit.

None of these round-trip to the daemon before rejecting — proven by tests that construct a bare `RdpilotMcpHandler::new()` with no live session and assert each rejection by name.

`hold_key`'s multi-second hold semantics are approximated as an atomic combo press+release (research A3) — a documented limitation, not a rejection. `wait`/`hold_key` `duration` is capped at 100s (Pitfall 5); over-cap values are rejected before any daemon round trip.

## Verification

```
cargo test -p rdpilot-mcp --test scale_to_native   # 14 passed (BLOCKING)
cargo test -p rdpilot-mcp                          # 31 unit + 14 integration passed
cargo test --workspace                              # all green, no regressions
cargo clippy -p rdpilot-mcp --all-targets            # clean
cargo tree -p rdpilot-mcp                            # no ironrdp/rustls/rdpilot/rdpilot-daemon/interprocess
```

The corrected tie vector: `scale_to_native(1279, 799, 1920, 1080) == (1919, 1079)` — `1279 * 1.5 = 1918.5` is an exact `.5` tie, `f64::round()` rounds it up to 1919 (round-half-away-from-zero), matching the plan's `4736a3b` correction.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - blocking compile fix] `scroll_dy`'s `i32::from(u32)` does not exist**
- **Found during:** Task 3 build
- **Issue:** `i32::from(scroll_amount.min(...))` — `From<u32> for i32` is not implemented (u32 doesn't always fit in i32).
- **Fix:** Widened the intermediate arithmetic to `i64` (`i64::from(scroll_amount)`, capped and clamped, then cast down to `i16`).
- **Files modified:** `crates/rdpilot-mcp/src/computer/dispatch.rs`
- **Commit:** d6900e4

**2. [Rule 2 - missing critical functionality] Added `McpError::InvalidArgument`**
- **Found during:** Task 3 — needed a way to render client-local validation failures (empty session, unsupported-action gap rejections, duration-cap breach, unknown key token) with a correctly-classed, legible error code.
- **Issue:** None of the existing `McpError` variants (`Wire`, `Timeout`, `DaemonUnreachable`, `Transport`) fit this shape; reusing `Wire(WireErrorCode::SessionNotFound)` for e.g. a `cursor_position` rejection would have been semantically wrong.
- **Fix:** Added `McpError::InvalidArgument(String)` + `McpError::invalid_argument(...)` constructor + `"invalid-argument"` discriminant in `code_str_for`, with its own unit tests.
- **Files modified:** `crates/rdpilot-mcp/src/error.rs` (not in the plan's `files_modified` list — a small, self-contained addition within this same crate, not a sibling-crate change).
- **Commit:** d6900e4

**3. [Rule 3 - blocking, clippy hygiene] `expect_err` flagged by crate-wide `#![deny(clippy::expect_used)]`**
- **Found during:** post-build `cargo clippy --all-targets` pass
- **Issue:** `.expect_err(...)` calls in `dispatch.rs`'s test module tripped the crate-wide deny lint (compiler warnings didn't catch this; `cargo test` alone doesn't run clippy).
- **Fix:** Added `#[allow(clippy::expect_used)]` on the test module, mirroring `rdpilot-daemon`'s own established precedent (`server.rs`/`registry.rs`/`lifecycle.rs`) that this crate-wide deny is a production-code guard, not a test-ergonomics one. Also fixed an unrelated `manual_range_contains` clippy suggestion in `check_duration_cap`.
- **Files modified:** `crates/rdpilot-mcp/src/computer/dispatch.rs`
- **Commit:** d6900e4 (folded into the same task commit; caught before the commit landed)

**4. [Environment] Offline substitute build target**
- Per the executor's own instructions, all `cargo build`/`test`/`clippy`/`tree` invocations used `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu ... --target x86_64-unknown-linux-gnu` (the project's default toolchain channel is Windows-targeted, `stable-x86_64-pc-windows-gnu`, appropriate for the eventual Windows sensor helper but not for this offline verification pass). No functional code depends on the substitution; it only affects the local build/test invocation.

### Design choices not treated as deviations (matched research's own framing)

- Click-action `text` (modifier-key) fields are deserialized for Anthropic field-name parity (Pitfall 3/A2) but not wired to a `WireMouseAction` equivalent — research's own Open Questions section flags this as unresolved and out of this plan's BLOCKING/must-have scope; documented as a no-op in the field's own doc comment, not fabricated as a 4th gap-rejection.
- `Scroll`'s `coordinate` is `Option<[u32; 2]>` in the schema (parity with Anthropic's own optional field) but dispatch treats a missing coordinate as a rejection, since no cursor-position tracking exists to fall back on (a direct, honest consequence of the `cursor_position` gap, not an invented requirement).

## Self-Check

- [x] `crates/rdpilot-mcp/src/computer/scale.rs` — FOUND
- [x] `crates/rdpilot-mcp/src/computer/mod.rs` — FOUND
- [x] `crates/rdpilot-mcp/src/computer/dispatch.rs` — FOUND
- [x] `crates/rdpilot-mcp/tests/scale_to_native.rs` — FOUND
- [x] `crates/rdpilot-mcp/src/handler.rs` (modified) — FOUND
- [x] `crates/rdpilot-mcp/src/main.rs` (modified) — FOUND
- [x] `crates/rdpilot-mcp/src/error.rs` (modified) — FOUND
- [x] Commit `5eb3389` (Task 1+2: scale + schema) — present in `git log`
- [x] Commit `d6900e4` (Task 3: dispatch + gap rejections + tool registration) — present in `git log`

## Known Stubs

None — every `computer_20250124` action either maps to a real `Request` or is an explicit, tested rejection.

## Threat Flags

None beyond the plan's own `<threat_model>` (T-14-07/08/09/10), all of which are directly mitigated by this plan's own implementation (see Verification above).
