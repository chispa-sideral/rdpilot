# rdpilot

AI-driven computer use over RDP — a reusable Windows SDK that lets a local agent perceive and drive a remote Windows desktop as if it were local hardware.

## What This Is

`rdpilot` is a reusable **SDK/library** that opens and manages an RDP session to a remote **Windows** machine and exposes that session as a controllable, *perceivable* surface. It is the foundation; AI computer use is its first and most demanding consumer.

The library provides two layers of perception over a single managed remote desktop:

- **Pixels (RDP-native):** screenshots of the full desktop or an individual window, plus mouse/keyboard input injection mapped to remote coordinates.
- **Structured (helper-sourced):** the remote process tree, window list/geometry, and the UI Automation (accessibility) tree — data the RDP protocol does *not* carry natively, retrieved via a **thin remote "sensor" helper** (a dumb perception/control component, explicitly NOT the AI agent) running on the target over an RDP virtual channel and/or out-of-band Windows remote management (WinRM/WMI).

The deliberately rejected alternative is the "obvious" path of running the AI agent inside the RDP session. The whole point is that intelligence stays on the **local** workstation and operates the remote desktop through `rdpilot`.

## Core Value

A local AI agent can connect to a remote Windows desktop over RDP and **read/inspect a program that is only reachable via RDP** — navigating it and reporting what it sees — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.

## Context

- **Author / audience:** Solo author (Marc). **Personal tooling first** — built for real use, clean enough to open-source later (LGPL-3.0 leaning), but v1 does not carry the burden of public API stability or polished published-package docs.
- **Motivation:** Real recurring need to operate remote Windows environments from a local agent — checking something in a program only available over RDP, performing manual software updates, occasionally remote-assisting a person. Computer use for AI is nascent; doing it *over RDP* is unsolved.
- **Stance on prior art:** Assume nothing exists in this space (the author doubts it does). Prior-art discovery is an explicit research dimension, not an assumption.
- **v1 focus:** Read / inspect tasks — low blast radius, a safe and provable first loop. Write-heavy automation comes later.
- **v1 finish line:** The SDK, proven end-to-end by a **scripted/manual harness** that connects, screenshots, reads the UIA tree, and navigates a real remote-only program. No live LLM agent is required to call v1 "done."

## Requirements

### Validated

(None yet — ship to validate)

### Active

- [ ] Connect to and authenticate an RDP session to a Windows target; manage session lifecycle (open, keepalive, teardown)
- [ ] Capture screenshots of the full remote desktop and of an individual remote window
- [ ] Inject mouse and keyboard input mapped to remote desktop/window coordinates
- [ ] Enumerate the remote process tree
- [ ] Enumerate remote windows (list, titles, geometry, foreground/z-order)
- [ ] Retrieve the UI Automation / accessibility tree for the desktop or a specified window
- [ ] Launch and observe a remote process (e.g. `pwsh.exe`)
- [ ] Bootstrap/deploy the thin remote sensor helper and establish its transport (RDP virtual channel and/or out-of-band)
- [ ] Expose a clean SDK API surface that an AI computer-use consumer (later) can drive
- [ ] Scripted proof harness: connect → screenshot → read UIA tree → navigate a real remote-only program → report findings

### Out of Scope

- Running the AI agent on the remote session — explicitly rejected; intelligence stays local
- MCP server / Anthropic computer-use shim / CLI packaging — a later milestone; v1's first consumer is a scripted harness
- Non-Windows RDP targets (Linux/xrdp, macOS) — Windows-only v1 to lean on UIA/WinRM/WMI/RAIL
- Heavy write/destructive automation and its guardrails — beyond what read/inspect needs
- Published-package polish — public API stability guarantees, comprehensive docs, multi-registry distribution
- Multi-session orchestration / concurrency at scale
- Remote-assist co-driving UX

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| The SDK/library is the product; AI is its first consumer | Decouples core RDP perception/control from packaging (MCP/shim/CLI), which become later concerns | — Pending |
| A thin remote "sensor" helper is permitted on the target | Only viable way to get structured perception (process/UIA tree) over RDP; it is a dumb sensor, not the agent | — Pending |
| Windows-only for v1 | Lets the SDK use Windows-native APIs (UI Automation, WinRM, WMI, RAIL, pwsh) instead of lowest-common-denominator pixel scraping | — Pending |
| Pixels + structured perception (not vision-only) | Richer grounding for the agent than screenshots alone | — Pending |
| RDP stack and implementation language deferred to research | Tradeoffs of IronRDP (Rust) vs FreeRDP (C) vs Microsoft's own stack are unknown and decide the language | — Pending |
| v1 "done" = scripted proof, no live LLM | Isolates the genuinely hard problem (RDP perception fidelity) from agent/packaging work | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-06-04 after initialization*
