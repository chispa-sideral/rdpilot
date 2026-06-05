---
id: SEED-001
status: dormant
planted: 2026-06-05
planted_during: v1.0 / Phase 02
trigger_when: when relevant
scope: unknown
---

# SEED-001: there should be an option to see the RDP connection window (as view only, or with control)

## Why This Matters

_To be filled in. Run `/gsd-capture --seed --enrich SEED-001` to add context._

## When to Surface

**Trigger:** when relevant

This seed will surface during `/gsd-new-milestone` when the milestone scope matches.

## Scope Estimate

**Unknown** — run `/gsd-capture --seed --enrich SEED-001` to estimate effort.

## Breadcrumbs

- `crates/rdpilot/src/framebuffer.rs` — stores the live RGBA32 `DecodedImage` updated each `GraphicsUpdate`; the raw data a viewer window would consume
- `crates/rdpilot/src/screenshot.rs` — `Screenshot` / `Rect` public types; `to_png()` and `crop()` encode the framebuffer for display — a viewer would poll or subscribe to this
- `crates/rdpilot/src/session.rs` — `Session::screenshot()` is the current pull API; a viewer window would need either a push/subscribe path or a shared framebuffer Arc for live rendering
- `crates/rdpilot/src/session_loop.rs` — the inner Tokio loop that fires on `GraphicsUpdate`; the integration point for a frame-ready notification channel
- `crates/rdpilot/src/session.rs` (`input_tx` / `RdpInputEvent`) — input injection channel already exists; "control" mode would route mouse/keyboard from the viewer window through this channel
- `.planning/ROADMAP.md` — Phase 02 goal explicitly mentions "stays rendered while the local window is minimized or hidden", signaling the viewer window concept is adjacent to planned work
- `.planning/phases/02-rdp-session-framebuffer-core/02-02-PLAN.md` — live session loop + framebuffer snapshot machinery built here; viewer would sit on top of this foundation

## Notes

_Captured via one-shot seed capture. Enrich with trigger, why, and scope at your convenience._
