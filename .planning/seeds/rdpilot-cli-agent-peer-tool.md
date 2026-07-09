---
title: rdpilot CLI as an agent-driven peer tool (RDP path alongside crabbox's VNC path)
trigger_condition: When the v2 "consumer surface" work (CLI / MCP shim) is planned, or when crabbox-based dev work begins and an RDP/Windows target is needed
planted_date: 2026-07-09
status: planted
related_requirements: [API-01, API-02, v2-CLI-packaging]
---

# Seed: rdpilot CLI as an agent-driven peer tool

## Origin
Surfaced during a `/gsd-explore` session (2026-07-09). Marc is about to use **openclaw/crabbox** for dev work and wants rdpilot usable alongside it. Initial framing was "integrate rdpilot into crabbox / add a CLI." Research reframed the integration shape (below).

## What crabbox actually is (research findings, 2026-07-09)
- A **remote-execution / test-runner control plane** — "warm a box, sync the diff, run the suite." Go CLI + coordinator. Core transport is **SSH + rsync**, NOT a desktop protocol.
- **VNC is an optional "desktop lease bridge"** only: x11vnc (server) -> websockify -> noVNC (browser viewer). crabbox stands up the VNC **server** side of a box it leased; it does NOT embed an outward VNC client to drive arbitrary machines. It has its own desktop verbs: `crabbox desktop launch|click|paste|type|key|record|proof`.
- **External process plugins are NOT implemented.** Provider model is internal Go interfaces (`SSHLeaseBackend` / `DelegatedRunBackend`); docs explicitly warn against depending on an undocumented stdio plugin protocol. The "External" provider only handles box *provisioning*, not desktop *actions*.
- **Zero RDP support** anywhere in the repo. VNC-only.
- No crabbox-specific MCP server (MCP belongs to the separate parent `openclaw` product).

## Decided integration shape
NOT "crabbox drives rdpilot" (its external-plugin seam isn't built, and there's a VNC<->RDP impedance mismatch). Instead:

> **rdpilot and crabbox are PEER tools an agent chooses between** — crabbox's CLI for the VNC/Linux path, an **rdpilot CLI for the RDP/Windows path.** RDP is the more powerful path because it carries structured perception (UIA tree, process/window geometry) that VNC pixels cannot.

This is cleaner than integrating with crabbox internals: **no dependency on crabbox's unbuilt plugin API, and no VNC<->RDP bridge to build.** rdpilot just needs to be shell-drivable.

## Design implications for the future rdpilot CLI
- **Mirror crabbox's desktop verb ergonomics** — `screenshot / click / type / key / launch / read-tree` — so an agent fluent in crabbox is instantly fluent in rdpilot.
- **Thin surface over the Phase 8 public Session API + WorldState.** The CLI should be a shell wrapper, not a reimplementation.
- **Agent-friendly output**: structured JSON for the perception surface (WorldState, window list, UIA tree), clean stdout/stderr streaming. Consider JSON-lines to match agent-tool conventions. Screenshots should be returned as a file path / handle, NOT inlined into agent context.
- **No VNC bridge, no crabbox coupling.** rdpilot stays RDP-native and standalone.

## Feed-forward into Phase 8 (act on now)
This seed directly informs the Phase 8 **WorldState serialization + component-selection decisions**: a CLI/JSON consumer is now a concrete near-future reality, which (a) strengthens the case for adding `serde::Serialize` to the owned SDK types and `WorldState` in Phase 8, and (b) supports a fully à-la-carte WorldState where the caller opts into each component (screenshot / window list / foreground UIA / specific-hwnd UIA / all-top-level UIA) so an agent can avoid pulling a bulky screenshot into context when it only needs structured data.

## Scope note
CLI packaging is currently **deferred to v2** (REQUIREMENTS.md v2 list; PROJECT.md out-of-scope for v1). This seed does **NOT** pull the CLI into v1 — v1's finish line remains the Phase 9 scripted proof harness. Revisit when v2 "consumer surface" work is planned. Note: the Phase 9 harness itself could later be re-expressed as a CLI consumer, worth considering at that time.

## Sources
- openclaw/crabbox: docs/architecture.md, docs/provider-backends.md, docs/cli.md
- Security advisory GHSA-25gx-x37c-7pph (noVNC/x11vnc/websockify stack)