# Phase 8 Discussion Log

**Date:** 2026-07-09
**Phase:** 08-public-sdk-api-worldstate
**Mode:** discuss (interactive, orchestrated)

## Gray areas presented (user selected all four)
1. WorldState UIA scope
2. Snapshot shape & 500 ms timing
3. Coordinate representation
4. Serialize + strict lints

## Area 1 — WorldState composition
- Options presented: foreground-only opt-in (recommended) / caller-hwnd opt-in / all-top-level opt-in.
- User response: pushed back — raised a screenshot context-bloat concern and proposed a FULLY a-la-carte model (caller selects screenshot / window list / foreground UIA / all-top-level UIA / specific-hwnd UIA).
- Resolution -> D-8.1: a-la-carte `WorldStateOptions`; defaults = screenshot + window list (SC#2-compliant); UIA modes None/Foreground/Hwnd/AllTopLevel. Context-bloat answered: the screenshot is SDK-struct bytes, not agent context; the consumer layer marshals it as a file handle; a-la-carte lets callers omit it.

## Area 2 — Snapshot shape & timing
- Options: options-struct + best-effort/report-elapsed (recommended) / options-struct + hard-fail / bare + best-effort.
- User selected: options-struct + best-effort/report-elapsed.
- Resolution -> D-8.2.

## Area 3 — Coordinates
- Options: single-pixel-space-now (recommended) / both-fields-logical==pixel / full-dual-scale-factor.
- User selected: single physical-pixel space now, defer dual.
- Resolution -> D-8.3.

## Area 4 — Serialize + strict lints
- Options: serde + lints (recommended) / serde-only / lints-only / neither.
- User selected: add `serde::Serialize` now + strict-lint gates.
- Resolution -> D-8.4. Informed by the mid-session crabbox exploration seed (a JSON CLI consumer is now concrete).

## Deferred ideas captured
- Dual pixel+logical coords -> backlog (D-8.3).
- rdpilot CLI/MCP -> v2 seed `.planning/seeds/rdpilot-cli-agent-peer-tool.md` (created mid-session via /gsd-explore, commit f027da1).
- AllTopLevel UIA latency -> future per-window cap if needed.

## Notes
- Mid-discussion the user ran /gsd-explore on openclaw/crabbox, producing the CLI peer-tool seed, which fed forward into D-8.1 (a-la-carte) and D-8.4 (serde now).
