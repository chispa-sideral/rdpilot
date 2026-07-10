# Phase 9: Scripted Proof Harness - Context

**Gathered:** 2026-07-10
**Status:** Ready for planning

<domain>
## Phase Boundary

A scripted, no-LLM harness connects to a REAL remote-only Windows program over RDP and runs the full loop connect -> screenshot -> get_window_list -> get_uia_tree -> navigate -> verify, exiting with a documented pass/fail report. This is the terminal v1 phase — success closes the milestone.

**Requirements:** PROOF-01.

**Success Criteria** (what must be TRUE):
1. The harness connects, authenticates, and produces a screenshot of a real remote-only Windows program (not a toy or localhost target).
2. The harness reads the UIA tree for the target program's main window and asserts specific named elements are present with valid bounding boxes.
3. The harness injects a navigation action (e.g. menu open, button click, or text entry) and verifies the result via a follow-up screenshot or UIA query.
4. The harness completes the full loop (connect -> screenshot -> get_windows -> get_uia_tree -> navigate -> verify) and exits with a pass/fail report, all assertions documented.

**Explicitly NOT in this phase** (redirect scope creep):
- New public `Session`/SDK API surface. The full public API already exists and is live-verified (Phase 8) — Phase 9 is pure composition + assertion authoring, UNLESS the D-9.1 fidelity spike proves 7-Zip's UIA tree needs a capability the SDK doesn't have (e.g. deeper-than-children tree walking), in which case that's a scoped, flagged addition — not a redesign.
- Any CLI/MCP consumer surface — still v2/out-of-scope (PROJECT.md, seeded separately).
- JSON report schema or report files — v1 report is stdout-only (D-9.4).
- Editing `Configure-Target.ps1` / infra scope — UI seeding happens at harness runtime (D-9.6), not by changing installed infra.

</domain>

<decisions>
## Implementation Decisions

### Target program + fidelity gate (was the standing blocker)
- **D-9.1:** The harness targets **7-Zip File Manager** (`C:\Program Files\7-Zip\7zFM.exe`) — the sample remote-only program already SHA-256-pinned and installed by Phase 1 infra (`Configure-Target.ps1`). All prior live UIA validation used Notepad, never 7-Zip. BEFORE any assertion code is written, Phase 9 runs a throwaway live spike (mirroring the proven Phase 7 D-7.5 risk-gate pattern): launch `7zFM.exe` on a disposable Azure VM, call `get_uia_tree(hwnd)`, dump the flat `UiaElement` list, and manually inspect role/name/bbox fidelity of the menu bar, toolbar, and listview. Fall back to Notepad (or a purpose-built sample app) ONLY if 7-Zip's tree proves genuinely inadequate (e.g. opaque/unlabeled toolbar controls with no distinguishing name/role). Rationale: 7-Zip preserves the PROJECT.md Core Value narrative ("read/inspect a program only reachable via RDP") that Notepad (first-party, locally trivial) would weaken.

### Navigation action (deferred, coupled to D-9.1)
- **D-9.2:** The specific navigation action (SC#3) is chosen from what the D-9.1 spike surfaces — whichever target element returns clean, distinguishable UIA name/role/bbox. Menu-open (e.g. click "File" in 7zFM's menu bar, verify submenu via UIA/screenshot) is the safe default recommendation if the spike gives no reason to prefer a toolbar-button click or address-bar text entry.

### Harness form-factor — BOTH example and gated test
- **D-9.3:** A single shared harness function/module, called by BOTH: (a) a new `examples/proof_harness.rs` (`main() -> ExitCode`, current-thread tokio runtime, modeled on `examples/screenshot.rs` — the human-facing v1 demo deliverable producing the report + exit code that SC#4's wording implies), AND (b) a thin gated `#[ignore]` test in `crates/rdpilot/tests/live_session.rs` (reusing `require_target!()` / `common::load_config()` so it stays inside the canonical `RDPILOT_LIVE` live-gate command, parity with Phases 2-8). Any harness code that lives in the library crate obeys D-09 (owned SDK types only) + API-01 (no `unwrap`/`expect`/`unsafe`).

### Report shape — stdout only
- **D-9.4:** Plain stdout step-by-step trace (connect check, screenshot check, uia check with element count, navigate check, verify check) plus a final PASS/FAIL summary and a non-zero exit code on failure. No JSON schema, no report file — no consumer exists yet (CLI/MCP is v2/out-of-scope); matches SC#4 literally with v1-minimalism precedent.

### Navigation-timing robustness — poll with timeout
- **D-9.5:** The click-then-verify step uses a bounded poll-with-timeout on the verifying UIA query/screenshot, reusing the existing `live_session.rs` poll-loop idiom (bounded attempts + `eprintln` progress, cf. `launch_notepad_and_find_window`). Explicitly NOT a fixed sleep (the anti-pattern backlog 999.1 was filed against). Phase 9 does NOT depend on 999.1 being promoted — it stays unpromoted; the local poll is sufficient.

### UI seeding — runtime, by the harness
- **D-9.6:** To make 7-Zip's UI deterministic (its default file view may be empty/variable), the harness seeds state at runtime — launch `7zFM.exe` pointed at a known path (e.g. `C:\Program Files`) or navigate to a known built-in location — rather than editing `Configure-Target.ps1`. Zero new infra scope. If D-9.2 lands on always-present chrome (menu bar/toolbar), seeding may be unnecessary; decide alongside the spike.

### Live infra reuse — no change
- **D-9.7:** Phase 9 reuses the established live-gate infra unchanged — Azure disposable VM via `infra/manage-env.ps1 up`/`down`, `RDPILOT_LIVE` env gate + `.secrets/connection.json` (host/user/password/rdpPort/winrmPort). No discoverability skill exists (backlog 999.3 unbuilt, 0 plans) — Phase 9 RESEARCH/CONTEXT must document the `manage-env.ps1 up`/`down` path explicitly since nothing else surfaces it.

### Claude's / planner's discretion
- Exact stdout trace formatting (symbols, exact wording) as long as it's a step-by-step trace ending in a clear PASS/FAIL summary (D-9.4).
- Exact number of poll attempts / timeout duration for D-9.5 (reason offline from prior phases' timing constants, live-tune empirically — established project methodology).
- Exact shape of `launch_7zip_and_find_window` (or equivalent) — mirror `launch_notepad_and_find_window`'s structure.
- Whether the harness module lives as a new `src`-adjacent test-support module, a `tests/common/` addition, or an `examples/`-local module shared via `include!`/path — as long as D-9.3's "single shared function, two callers" holds.
- Which specific UIA elements (name/role) are asserted for SC#2, and the exact wording of assertion failure messages — determined by the D-9.1 spike output.

</decisions>

<specifics>
## Specific Ideas

- The D-9.1 spike is explicitly modeled on the proven Phase 7 D-7.5 risk-gate pattern (spike-before-code, dump-and-inspect, fallback criteria defined up front) — do not invent a new spike methodology.
- "Reason offline, live-tune empirically" (Phase 8's D-8.2 wording) applies again here for D-9.5's poll/timeout tuning.
- The harness is the human-facing v1 demo deliverable — `examples/proof_harness.rs` should read like a narrated walkthrough of the Core Value narrative (README.md / PROJECT.md), since it's likely what gets pointed to as "proof v1 works."

</specifics>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope & requirements
- `.planning/ROADMAP.md` §"Phase 9: Scripted Proof Harness" — goal, 4 success criteria, requirement mapping (PROOF-01).
- `.planning/REQUIREMENTS.md` — PROOF-01 full text.
- `.planning/PROJECT.md` — Core Value narrative ("read/inspect a program only reachable via RDP"); v1 done = scripted harness, no live LLM; D-09 owned-types-only public API.
- `.planning/STATE.md` — accumulated decisions and current position.

### Prior phase context (inherited constraints — do not re-litigate)
- `.planning/phases/08-public-sdk-api-worldstate/08-CONTEXT.md` — full public `Session` API surface (D-8.1..D-8.4); `world_state()` composite call; owned-type/serde split.
- `.planning/phases/07-uia-tree-module/07-CONTEXT.md` — D-7.4 (`TreeScope_Children`-only scope — must be re-assessed against 7-Zip's tree depth during the D-9.1 spike); D-7.5 risk-gate spike pattern (the template for D-9.1); `UiaElement` field set/shape.
- `.planning/phases/06-window-process-perception/06-CONTEXT.md` — `WindowInfo`/`ProcessInfo` shapes; success/degrade contract.
- `.planning/phases/04-dvc-transport-channel/04-CONTEXT.md` — wire envelope + sensor protocol (stays INTERNAL; API-01 hides it; harness never touches it directly).

### Infra & live-test environment (no discoverability skill exists — document explicitly)
- `infra/manage-env.ps1` — sole live-target bootstrap path: `up` provisions the disposable Azure VM, `down` tears it down.
- `.secrets/connection.json` — live connection details (host/user/password/rdpPort/winrmPort) produced by `manage-env.ps1 up`.
- `infra/scripts/Configure-Target.ps1` (~lines 22-176) — installs 7-Zip 26.01 (SHA-256 verified) to `C:\Program Files\7-Zip\7zFM.exe`.
- `infra/tests/Validate-Target.ps1` (lines 11, 43, 168-172) — asserts 7-Zip's presence (ENV-01); confirms the target is already infra-guaranteed, no new infra work needed.
- `.planning/ROADMAP.md` §"Phase 999.3" — documents the infra exists but has no discoverability skill yet; do not depend on 999.3 being promoted.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `crates/rdpilot/tests/live_session.rs` (~1647 lines) — canonical gated-live pattern: `#[ignore]` gating, `require_target!()` skip macro, `common::load_config()`, `--test-threads=1` canonical run, existing `launch_notepad_and_find_window()` poll helper (direct template for a `launch_7zip_and_find_window` equivalent).
- `crates/rdpilot/tests/common/mod.rs` — `load_config()` / `LIVE_ENV` / `IDLE_SECS_ENV` gating helpers, reusable verbatim.
- `crates/rdpilot/src/session.rs` — full public `Session` API: `connect`, `screenshot`/`screenshot_window`, `get_window_list`, `get_uia_tree(hwnd)`, `world_state(opts)`, `send_mouse`/`send_key`, `set_foreground_window`, `launch_process`, `deploy_and_launch`, `close`. The harness likely does `deploy_and_launch` (sensor bootstrap) THEN `launch_process("7zFM.exe", ...)`, mirroring `launch_notepad_and_find_window`.
- `crates/rdpilot/src/worldstate.rs` — `WorldStateOptions`/`UiaMode`/`WorldState`; `UiaMode::Foreground` already does the titled-only min-z-order "main window" heuristic — may simplify SC#2's window targeting.
- `crates/rdpilot/examples/screenshot.rs` — example-binary shape (`main() -> ExitCode`, current-thread tokio, connect->action->close, no ironrdp/image leak) — direct template for `examples/proof_harness.rs`.

### Established Patterns
- Owned-SDK-types-only public API (D-09); harness code touching the library crate stays inside this boundary.
- No `unwrap`/`expect`/`unsafe` in library code (API-01) — applies to any harness code that lands inside `crates/rdpilot/src` or `tests/`; `examples/` binaries are conventionally more lenient but should still follow the pattern per D-9.3.
- Bounded poll-loop with `eprintln` progress (not fixed sleep) for any state that needs to settle — `launch_notepad_and_find_window`'s idiom, reused for D-9.5.
- "Reason offline, live-tune empirically" for timing constants (established Phases 2-8).

### Integration Points
- `examples/proof_harness.rs` and the new `#[ignore]` test in `tests/live_session.rs` both call into one shared harness function/module (D-9.3) — this is the phase's primary new code surface.
- The harness consumes ONLY existing public types (`Session`, `WorldState`, `UiaElement`, `WindowInfo`) — confirmed via `crates/rdpilot/src/lib.rs`'s re-export list. No new public types are expected unless the D-9.1 spike forces one.

</code_context>

<deferred>
## Deferred Ideas

- **CLI/MCP consumer surface** — still v2/out-of-scope (PROJECT.md, REQUIREMENTS.md v2 list).
- **JSON report schema / report file output** — no consumer exists yet; stdout-only for v1 (D-9.4).
- **Backlog 999.1 (harden live suite against frame-timing races)** — Phase 9 does not depend on it; the local D-9.5 poll is sufficient standalone.
- **Backlog 999.3 (auto-surface live-test-env skill)** — still unbuilt (0 plans); Phase 9 documents the `manage-env.ps1` path explicitly in RESEARCH/CONTEXT instead of relying on a skill.
- **Deeper-than-children UIA tree walking** — only in scope if the D-9.1 spike proves `TreeScope_Children` (D-7.4) is insufficient for 7-Zip's meaningful elements (listview rows, menu items). If needed, the planner must flag it as a scoped capability addition, not silently absorb it.
- **`value`/`ValuePattern` on `UiaElement`** — still backlog (inherited from Phase 7/8).

</deferred>

---

## Settled — do not re-litigate

- **Full public Session SDK API** — locked Phase 8 (D-8.1..D-8.4); Phase 9 composes it, does not extend it (barring the D-9.1 spike exception).
- **Owned-types-only public API (D-09)** — locked Phase 2.
- **Coordinate contract** (physical virtual-desktop pixels, shared `Rect`) — locked Phases 6/7/8.
- **Wire envelope + sensor/DVC protocol** — locked Phase 4; stays hidden behind `Session`.
- **UIA scope is `TreeScope_Children`-only (D-7.4)** — locked Phase 7, but explicitly FLAGGED for re-assessment against 7-Zip's depth needs during the D-9.1 spike (see Deferred). Not silently overridden — any change is a planner-flagged scoped addition.
- **CLI/MCP packaging is v2** — PROJECT.md/REQUIREMENTS.md.
- **Live infra (`infra/manage-env.ps1`, disposable Azure VM, `RDPILOT_LIVE` gate)** — built Phase 1, reused unchanged (D-9.7).

---
*Phase: 09-scripted-proof-harness*
*Context gathered: 2026-07-10*
