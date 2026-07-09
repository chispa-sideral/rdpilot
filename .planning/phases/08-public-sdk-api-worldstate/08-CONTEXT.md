# Phase 8: Public SDK API + WorldState - Context

**Gathered:** 2026-07-09
**Status:** Ready for planning

<domain>
## Phase Boundary

A clean typed `Session` struct hides all IronRDP internals and sensor protocol details, and a `WorldState` snapshot combines framebuffer screenshot, window list, and optional UIA tree in one timestamped structure with a single enforced coordinate space.

**Requirements:** API-01 (clean typed SDK API surface), API-02 (coherent `WorldState` correlating screenshot + window list + UIA snapshot in one coordinate space).

**Success Criteria** (what must be TRUE):
1. A consumer can drive a full read/inspect workflow using only the public `Session` API without importing any IronRDP types or sensor protocol details.
2. `Session::world_state()` returns a `WorldState` struct containing a screenshot, window list, and optional UIA tree captured within 500 ms of each other.
3. All coordinate values in `WorldState` (screenshot dimensions, window rects, UIA bounding boxes, mouse input targets) are in the same virtual-desktop pixel space with no silent scaling.
4. The API compiles clean under strict Rust settings (no `unsafe` in public surface, no `unwrap` in library code).

**Explicitly NOT in this phase** (redirect scope creep):
- The rdpilot **CLI / MCP consumer surface** — deferred to v2 (REQUIREMENTS.md v2 list; PROJECT.md out-of-scope). Captured as seed `.planning/seeds/rdpilot-cli-agent-peer-tool.md`; do NOT build in Phase 8.
- Dual pixel + logical/DPI-scaled coordinate representation (API-02's "both coordinates") — backlog; unnecessary while the session is forced to 96 DPI (logical==physical). See Deferred.
- The scripted end-to-end proof harness — Phase 9 (PROOF-01).
- New perception capabilities (clipboard, file transfer, etc.) — v2.
</domain>

<decisions>
## Implementation Decisions

### WorldState composition — a-la-carte
- **D-8.1:** `Session::world_state(WorldStateOptions)` is fully a-la-carte: each component is individually selectable. `screenshot` (default ON), `window_list` (default ON), and `uia` as a mode — `None` (default) / `Foreground` / `Hwnd(u64)` (one or a set) / `AllTopLevel`. A default `WorldStateOptions` yields screenshot + window list + no UIA, satisfying ROADMAP SC#2's "contains a screenshot, window list, and optional UIA tree." Rationale: lets a consumer (esp. the seeded CLI/MCP agent) request only what it needs — avoiding pulling a bulky screenshot into an agent's context when it only wants structured data — while keeping the default call SC#2-compliant. Generalizes the "optional UIA" requirement to "optional everything, sensible defaults."

### Snapshot shape and the 500 ms bound
- **D-8.2:** `world_state()` returns a `WorldState` carrying ONE batch timestamp plus the measured capture span (elapsed across the sequenced component fetches). The SC#2 "within 500 ms of each other" bound is BEST-EFFORT: if the span exceeds 500 ms the call still returns and surfaces the measured elapsed — it does NOT hard-fail. Rationale: matches the project's consistent "reason offline, live-tune empirically" methodology (every `session.rs` timing constant was reasoned offline then verified live); a hard error on a single transient slow sensor round-trip would make the primary snapshot API brittle. SC#2 is validated empirically at a live gate, as in every prior phase. Timestamp type (monotonic `Instant` vs wall-clock `SystemTime`) is planner's discretion — no `chrono`/`time` dependency exists today.

### Coordinate representation — single pixel space now
- **D-8.3:** `WorldState` uses a SINGLE physical virtual-desktop pixel space — the existing shared `crate::Rect` (`u32 x/y/w/h`) already reused by `Screenshot` dims, `WindowInfo.rect`, and `UiaElement.bbox` — with no silent scaling (SC#3). Logical==physical is documented as a consequence of the forced 96 DPI (100%) session contract. API-02's "emit BOTH pixel and logical/DPI-scaled coordinates" is DEFERRED to backlog for future non-96-DPI targets (nothing tracks a DPI scale factor today, and it would be 1.0 for every v1 target). Rationale: honors ROADMAP SC#3's "single space, no silent scaling"; matches v1-minimalism; avoids inventing a scale-factor abstraction with zero v1 benefit.

### Serialization + strictness enforcement
- **D-8.4:** Add `serde::Serialize` to all owned public types (`Rect`, `WindowInfo`, `WindowState`, `ProcessInfo`, `UiaElement`) and the new `WorldState`, WITHOUT pulling `image`/`ironrdp` types into the public surface (mirror the existing owned-type/wire-type split, D-09). `Screenshot` serializes DIMS-ONLY (`width`/`height`); the raw RGBA buffer is excluded from serde output — PNG bytes remain available via the existing `to_png()`. Additionally add strict lint gates to compiler-ENFORCE API-01 / SC#4: `#![deny(unsafe_code)]` plus clippy `unwrap_used` / `expect_used` gates (these currently hold by convention + tests only). Rationale: the just-seeded CLI/MCP consumer will need JSON `WorldState` output, so `Serialize` is now concretely justified rather than speculative; turning "strict by convention" into "strict by compiler" locks SC#4 permanently. Screenshot RGBA is excluded from JSON to keep the structured payload lean (the context-bloat concern) — screenshots cross the consumer boundary as a file/handle, not inline.

### Claude's / planner's discretion
- Exact `WorldStateOptions` field/enum names and whether the `uia` `Hwnd` variant takes a single `u64` or a `Vec<u64>` (D-8.1) — planner's call, as long as the four selection modes are expressible.
- Timestamp type (`Instant` vs `SystemTime`) and whether the capture span is a single `Duration` field or per-component timestamps (D-8.2).
- Exact clippy lint set beyond `unwrap_used`/`expect_used` (e.g. `panic`, `missing_docs`) and whether the gates live in `lib.rs` attributes or a `[lints]` table in `Cargo.toml` (D-8.4).
- Whether `world_state()` sequences the existing public methods (`screenshot`/`get_window_list`/`get_uia_tree`) or funnels the sensor-backed parts through the private `sensor_request()` helper — an implementation detail.
- Exact serde representation of the `WindowState` enum and any `#[serde(rename)]` conventions.
</decisions>

<specifics>
## Specific Ideas
- The public `Session` API (API-01) is already ~90% delivered: `lib.rs` re-exports only owned types, no IronRDP/rustls/image/sensor-protocol leaks, and grep-confirmed zero `unwrap`/`expect`/`unsafe` in non-test library code. Phase 8 mostly AUDITS + DOCUMENTS + HARDENS that surface (adds the deny/clippy gates) and adds the genuinely new `WorldState` + `world_state()`.
- `WorldState` is a data/API surface, not a visual UI — ROADMAP's "UI hint: yes" refers to the SDK API being the consumer's "interface." No `/gsd:ui-phase` visual contract is needed; the API ergonomics ARE the surface.
- Context-bloat guard (from discussion): screenshots must never be inlined into a downstream agent's context; the a-la-carte options (D-8.1) + dims-only `Screenshot` serde (D-8.4) are the mechanisms that keep structured payloads lean.
</specifics>

<canonical_refs>
## Canonical References
**Downstream agents MUST read these before planning or implementing.**

### Phase scope & requirements
- `.planning/ROADMAP.md` §"Phase 8: Public SDK API + WorldState" — goal, 4 success criteria, requirement mapping (API-01, API-02).
- `.planning/REQUIREMENTS.md` — API-01 and API-02 full text (note API-02's "both pixel and logical/DPI-scaled" — RESOLVED to single-space-now per D-8.3, dual deferred).
- `.planning/PROJECT.md` — locked stack decisions; D-09 owned-types-only public API; CLI/MCP explicitly v2/out-of-scope.
- `.planning/STATE.md` — accumulated decisions and current position.

### New seed (informs the deferred CLI, not this phase)
- `.planning/seeds/rdpilot-cli-agent-peer-tool.md` — the crabbox peer-tool insight; the reason D-8.1/D-8.4 lean toward a-la-carte + serde now. Do NOT build the CLI in Phase 8.

### Prior phase context (inherited constraints — do not re-litigate)
- `.planning/phases/07-uia-tree-module/07-CONTEXT.md` — `UiaElement` field set/shape; coordinate space FIXED as physical virtual-desktop pixels matching `crate::Rect`; owned-type/wire-type split (D-09).
- `.planning/phases/06-window-process-perception/06-CONTEXT.md` — D-6.3 field-richness pattern; D-6.4 success/degrade contract; `WindowInfo`/`ProcessInfo` shapes.
- `.planning/phases/04-dvc-transport-channel/04-CONTEXT.md` — wire envelope + sensor protocol (stays INTERNAL; API-01 hides it).

### Current code surface (from scout — read directly)
- `crates/rdpilot/src/lib.rs` — authoritative public re-export list (the D-09 boundary).
- `crates/rdpilot/src/session.rs` — all public `Session` methods + the private `sensor_request()` helper (the seam `world_state()` composes).
- `crates/rdpilot/src/screenshot.rs` — `Screenshot { width, height, rgba }`, `Rect { x,y,w,h }`, `to_png()`, `crop()`.
- `crates/rdpilot/src/perception.rs` — `WindowInfo`/`ProcessInfo`/`UiaElement` owned types + their `*Wire` deserialize-only counterparts.
- `crates/rdpilot/src/error.rs` — `Error` (`#[non_exhaustive]`, thiserror), `Result`.
</canonical_refs>

<code_context>
## Existing Code Insights

### Already true (act on these — do not re-derive)
- **API-01 largely satisfied:** `lib.rs` exports only owned types (`ConnectionConfig`, `Error`/`Result`, input vocab, `ProcessInfo`/`UiaElement`/`WindowInfo`/`WindowState`, `Rect`/`Screenshot`, `Session`). `connect`/`framebuffer`/`keepalive`/`session_loop` are private mods. No IronRDP/rustls/image/anyhow leak at the boundary. Grep-confirmed: zero `unwrap`/`expect`/`unsafe` in non-test library code.
- **Coordinate space already unified & enforced:** one `Rect` (defined in `screenshot.rs`) is reused verbatim by `WindowInfo.rect` and `UiaElement.bbox`; `desktop_size()` and `check_bounds()` (mouse input) share it. D-8.3 does NOT need to build a unification layer — it exists end-to-end. SC#3 is largely already true.
- **No timestamp/time concept exists** anywhere (`Screenshot` is `{width,height,rgba}` only); no `chrono`/`time` in `Cargo.toml`. D-8.2's batch timestamp + span is greenfield.
- **No owned type currently derives `Serialize`** (only internal `*Wire` structs derive `Deserialize`). D-8.4's `Serialize` derives are new ground — add them to owned types + `WorldState` only, never to `image`/`ironrdp` types.
- **No `#![deny]` / `[lints]` config today** — D-8.4's strict gates are additive; expect to fix any latent warnings they surface.

### Reusable assets for `world_state()`
- `Session::screenshot()` — clones the latest framebuffer snapshot (client-side, no sensor round-trip).
- `Session::get_window_list()` / `get_uia_tree(hwnd)` — sensor round-trips via the private `sensor_request()` helper (handshake-check -> req_id -> oneshot -> timeout). `world_state()` sequences these.
- `Session::desktop_size()` — `(u32,u32)` in the same pixel space; validate/attach to WorldState's single coordinate space.
- The owned-type/`*Wire` split in `perception.rs` — the template if any new serde-facing shape is needed.

### Established patterns (reuse verbatim)
- Owned-SDK-types-only public API (D-09); internal types stay `pub(crate)`.
- No `unwrap`/`expect`/`panic!` in library code (API-01) — applies to all new `WorldState`/`world_state()` code.
- Physical virtual-desktop pixels, `u32 x/y/w/h`, single space (SC#3).
- "Reason offline, live-tune empirically" for timing (D-8.2's 500 ms bound).

### Integration points
- `world_state()` is a NEW public `Session` method — the first composite/aggregating call (all prior methods are single-purpose). It sequences existing methods under a capture-span measurement.
- `WorldStateOptions` + `WorldState` are new public types added to `lib.rs`'s re-export list.
- Adding `Serialize` + strict lints touches every owned-type module (`screenshot.rs`, `perception.rs`, `error.rs`) plus `lib.rs`/`Cargo.toml` — a crate-wide but mechanical pass.
</code_context>

<deferred>
## Deferred Ideas
- **Dual pixel + logical/DPI-scaled coordinates** (API-02 literal) — backlog. Only meaningful for non-96-DPI targets; logical==physical for all v1 targets. Revisit if a target ever runs at non-100% scaling.
- **rdpilot CLI / MCP consumer surface** — v2, seeded at `.planning/seeds/rdpilot-cli-agent-peer-tool.md`. The peer-to-crabbox agent-driven CLI. Informs Phase 8 (a-la-carte + serde) but is NOT built here.
- **`AllTopLevel` UIA latency** — the D-8.1 `AllTopLevel` mode may risk the 500 ms span on many-window desktops; per D-8.2 it degrades gracefully (best-effort + reported elapsed) rather than failing. If it proves consistently slow, a per-window cap is a future refinement.
- **`value`/`ValuePattern` on `UiaElement`** — still backlog (inherited D-7.1).
</deferred>

---

## Settled — do not re-litigate
- **Owned-types-only public API (D-09)** — locked Phase 2; API-01 audits/documents it, does not re-decide it.
- **Coordinate contract** (physical virtual-desktop pixels, shared `Rect`) — locked Phases 6/7; WorldState reuses it.
- **Wire envelope + sensor/DVC protocol** — locked Phase 4; API-01 HIDES it behind the typed `Session`, does not change it.
- **CLI/MCP packaging is v2** — PROJECT.md/REQUIREMENTS.md; the seed does not change v1 scope.

---
*Phase: 08-public-sdk-api-worldstate*
*Context gathered: 2026-07-09*
