---
phase: 03-input-injection
verified: 2026-07-08T00:00:00Z
status: passed
score: 4/4 must-haves verified
overrides_applied: 0
---

# Phase 3: Input Injection Verification Report

**Phase Goal:** Mouse and keyboard actions are delivered to the remote session at the correct coordinates, and the DPI coordinate contract is defined and enforced for all future phases.
**Verified:** 2026-07-08
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (ROADMAP Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | A mouse click sent to a known remote coordinate activates the target element (e.g. a menu opens) | ✓ VERIFIED | `crates/rdpilot/src/session.rs::send_mouse` implements bounds-check → translate → apply → send for `MouseAction::Click`. Gated test `mouse_click_activates_menu` (`tests/live_session.rs:332`) right-clicks the desktop center and asserts `region_changed` over a 400x500 menu rect. SUMMARY 03-04 records this test **passed** in the canonical live run against a real Azure VM (10/10 pass, re-run after fixing an infra gap). Test source, not just SUMMARY prose, was read and confirms the assertion is a real screenshot-diff, not a stub. |
| 2 | All mouse action types work: move, left/right/middle click, double-click, scroll, drag | ✓ VERIFIED (with one honestly-scoped carve-out) | `MouseAction` enum (`input.rs:383`) has all 6 variants; `mouse_operations()` translates each; `Session::send_mouse` (`session.rs:194`) dispatches all 6 with correct batching/timing (`DOUBLE_CLICK_GAP`, `DRAG_STEP_GAP`). Gated test `mouse_action_types_all_work` exercises all 6 live. Move/Click(L/M/R)/right-click-menu are asserted via screenshot-diff or Ok+alive; DoubleClick, Drag, and Scroll are round-trip-only on this VM image (no double-clickable icon, no lasting drag trace, no scrollable Start overflow — see Flag below). The underlying `send_mouse` mechanism for all 6 variants is unconditionally proven (offline unit tests assert exact operation sequences for every variant; live run proves the 3 fully-observable variants and non-erroring round-trip for the other 3). |
| 3 | Typed text and key combinations (e.g. Ctrl+A, Alt+F4) are received by the remote application | ✓ VERIFIED | `KeyAction::Type`/`Combo` (`input.rs:466`), `key_operations()` (D-3.5 modifier ordering), `Session::send_key` (`session.rs:253`). Gated tests `keyboard_typed_text_is_received` (Start search box, screenshot-diff) and `keyboard_combos_are_received` (Ctrl+Esc opens Start — diff observed; Ctrl+A — round-trip only, documented trade-off; Alt+F4 opens then Esc-cancels the Shut Down Windows dialog — diff observed). SUMMARY 03-04 records both tests passed live. |
| 4 | The coordinate contract is documented and enforced: 96 DPI, physical virtual-desktop pixels | ✓ VERIFIED | `Session::desktop_size()` (`session.rs:155`) exposes the connect-time-captured negotiated size; `check_bounds()` (`session.rs:162`) rejects any out-of-range coordinate with `Error::CoordinateOutOfBounds` **before** any `Operation`/PDU is built — mirrors the `CropOutOfBounds` precedent exactly (`error.rs:76`, `:118`). Offline unit test `check_bounds_rejects_out_of_range_and_accepts_in_range` and `send_mouse_rejects_out_of_range_coordinate_before_sending` (asserts nothing reaches the channel on rejection). Gated live test `coordinate_contract_enforced` confirms `desktop_size()` reports 1920x1080 physical pixels and that a subsequent in-bounds click still succeeds after a rejection (proves per-call, not session-level, enforcement). 96 DPI itself is a Phase 1 concern (VM-level), correctly out of this phase's scope per 03-CONTEXT.md. |

**Score:** 4/4 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rdpilot/src/input.rs` | Owned `MouseAction`/`KeyAction`/`Button`/`Key` + pure translation to `ironrdp_input::Operation` batches | ✓ VERIFIED | 762 lines, 16 unit tests, all passing. No `ironrdp`/`Scancode`/`FastPathInputEvent` type in any public signature — confirmed by direct read (only `crate::error::Error` and `ironrdp_input::{...}` imports used internally in `pub(crate)` functions). |
| `crates/rdpilot/src/error.rs` | `Error::CoordinateOutOfBounds` mirroring `CropOutOfBounds` | ✓ VERIFIED | Variant, constructor (`coordinate_out_of_bounds`), and `category()` arm all present and match the `CropOutOfBounds` shape field-for-field. |
| `crates/rdpilot/src/session.rs` | `Mutex<Database>`, `desktop_size` field/accessor, `check_bounds`, `send_mouse`, `send_key` | ✓ VERIFIED | All present at the lines cited above. Lock is scoped to the synchronous `apply()` call only, dropped before `.await` (confirmed by reading the code, not just the SUMMARY claim) — matches Pitfall 3 mitigation. |
| `crates/rdpilot/src/session_loop.rs` | `RdpInputEvent::FastPath` variant + forwarding `select!` arm | ✓ VERIFIED | `RdpInputEvent::FastPath(Vec<FastPathInputEvent>)` (line 50) forwarded unchanged to `active_stage.process_fastpath_input`; no sleep or construction logic added to the loop. |
| `crates/rdpilot/src/lib.rs` | Re-export `MouseAction`, `KeyAction`, `Button`, `Key` at crate root | ✓ VERIFIED | `pub use input::{Button, Key, KeyAction, MouseAction};` present alongside the Phase 2 public surface. |
| `crates/rdpilot/tests/live_session.rs` | 5 gated tests covering all 4 SC | ✓ VERIFIED | `mouse_click_activates_menu`, `mouse_action_types_all_work`, `coordinate_contract_enforced`, `keyboard_typed_text_is_received`, `keyboard_combos_are_received` — all present, all `#[ignore]`'d, all early-return via `require_target!()` when `RDPILOT_LIVE` is unset (D-18 offline-green contract intact). |
| `infra/scripts/Configure-Target.ps1` | Server Manager suppression fix | ✓ VERIFIED | Step 4/5 writes `DoNotOpenServerManagerAtLogon=1` to the DEFAULT user hive, same idempotent reg-load/write/unload pattern as the existing DPI/SuppressWhenMinimized steps; commit `a4f9f31` confirmed in git log. |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| `Session::send_mouse` | `crate::input::mouse_operations` | direct call, batches applied to `Database` | ✓ WIRED | `session.rs:205` |
| `Session::send_key` | `crate::input::key_operations` | direct call | ✓ WIRED | `session.rs:254` |
| `Session::send_mouse`/`send_key` | `session_loop`'s `input_tx` channel | `input_tx.send(RdpInputEvent::FastPath(events)).await` | ✓ WIRED | `session.rs:217-219`, `:264-267` |
| `session_loop::run`'s `select!` | `ActiveStage::process_fastpath_input` | `RdpInputEvent::FastPath` match arm | ✓ WIRED | `session_loop.rs:101-108` |
| `MouseAction::coordinates()` | `Session::check_bounds` | called before translation/PDU construction | ✓ WIRED | `session.rs:164`, confirmed by offline test that nothing is sent to the channel on rejection |

### Behavioral Spot-Checks / Offline Test Run

Ran directly in this verification session (not taken from SUMMARY claims):

```
RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot --target x86_64-unknown-linux-gnu
```

Result: **46 passed; 0 failed; 0 ignored** (unit tests) + **0 passed; 0 failed; 10 ignored** (gated `tests/live_session.rs`, correctly skipped offline). Matches the counts claimed in 03-04-SUMMARY.md exactly. Confirms Phase 2's regression tests (5) plus Phase 3's new gated tests (5) = 10 ignored, consistent with the canonical live run's "10 passed / 0 failed" claim.

### Probe Execution / Live Validation

No live VM was provisioned during this verification (out of scope for a static goal-backward review — provisioning a new Azure VM to re-run the live suite was not attempted). The canonical live run itself cannot be independently re-executed without spinning up infrastructure; instead, the verifier cross-checked:
- All 4 SC-mapped gated tests exist in `tests/live_session.rs` and assert genuine screenshot-diff or bounds-check behavior (read directly, not inferred from SUMMARY).
- All 14 claimed commit hashes across Plans 01-04 (including the two Plan-04 fix commits `da53bc3`, `a4f9f31`) exist in `git log --all`.
- The infra fix (`Configure-Target.ps1` step 4/5) is present in the current file, not merely claimed.

This is the practical limit of static verification for a live-only phase; the live pass/fail claim itself rests on the SUMMARY's recorded 10/10 result, which is internally consistent with everything independently checkable (test code exists and asserts the right things, infra fix landed, commit history is real).

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| INPUT-01 | 03-01, 03-02, 03-04 | Mouse actions (move, click variants, scroll, drag), mapped to Anthropic/OpenAI computer-use vocabulary | ✓ SATISFIED (core mechanism); ℹ️ NOTE: vocabulary-mapping wrapper explicitly deferred | The `send_mouse`/`MouseAction` primitives are fully implemented and live-proven per the phase's 4 SC. `03-CONTEXT.md` explicitly scopes the Anthropic/OpenAI vocabulary *mapping layer* itself out of this phase ("this phase delivers the underlying send_mouse/send_key primitives; the vocabulary wrapper is a thin layer that can be added later"). This is a legitimate, pre-declared scope decision, not a shipped-vs-claimed gap — but it means INPUT-01's full requirement text is not 100% complete until that wrapper exists (tracked as future work, not currently assigned to a phase in ROADMAP.md beyond this one). |
| INPUT-02 | 03-03, 03-04 | Keyboard type text + key combinations/modifiers | ✓ SATISFIED | `send_key`/`KeyAction` fully implemented and live-proven (Type + Combo, both observable success criteria). |

**Orphaned requirements check:** REQUIREMENTS.md traceability table maps only INPUT-01/INPUT-02 to Phase 3 — no orphans.

**Documentation staleness (non-blocking):** `REQUIREMENTS.md`'s traceability table (line 85-86) still reads "In progress (Plan 1/4: offline contract layer done; Session wiring + live verification pending)" for both INPUT-01 and INPUT-02, even though the checkbox lines (25-26) correctly show `[x]` complete and ROADMAP.md correctly shows Phase 3 as `[x]` Complete with 4/4 plans. This is stale prose left over from Plan 1, not a functional gap — flagged as a documentation cleanup item, not a verification blocker.

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `crates/rdpilot/src/input.rs` | 11 | Unused import (`use crate::error::Error;`) | ℹ️ Info | Pre-existing since Plan 01, documented in `deferred-items.md`, `cargo clippy` exits 0 (warning only). Not a debt marker (no TBD/FIXME/XXX), does not affect correctness. |

No `unwrap()`/`expect()`/`panic!()` found in non-test code across `input.rs`, `session.rs`, `session_loop.rs`, `error.rs` (verified by direct grep against the actual `#[cfg(test)]` module boundaries, not by trusting the SUMMARY's claim) — API-01 holds. No `TBD`/`FIXME`/`XXX`/`TODO`/`HACK`/`PLACEHOLDER` markers found in any Phase 3 file or `Configure-Target.ps1`.

### Human Verification Required

None. All 4 success criteria have either offline-automatable proof (SC#4's rejection half) or a recorded, code-consistent live-validation result (SC#1/#2/#3 and SC#4's physical-pixel half) that the verifier could cross-check against real test source and commit history.

### Flags

1. **Scroll relaxed to round-trip-only (SC#2).** The Windows Server 2022 test VM's Start app list has no scrollable overflow with the currently-installed small app set, so `mouse_action_types_all_work`'s Scroll assertion could not screenshot-diff. This was relaxed to Ok+alive, matching the same carve-out already applied to DoubleClick/Drag. **Assessment: does not materially weaken SC#2.** The underlying `send_mouse(Scroll)` mechanism (wheel-magnitude split, MouseMove-before-Wheel ordering) is fully proven offline by 4 dedicated unit tests (`scroll_always_emits_move_before_wheel_rotations`, `scroll_single_notch_produces_exactly_one_wheel_rotation`, `scroll_over_255_magnitude_splits_into_in_range_operations`, `scroll_negative_magnitude_preserves_sign_when_split`), and the live round-trip confirms the PDU reaches the session without error. What is NOT proven is that a scroll wheel event visibly moves Windows UI on this specific VM image — a content/environment limitation, not a code defect, and explicitly documented as such with a path to re-tighten on a future VM image. Same reasoning applies to DoubleClick/Drag, both of which received a *separate*, stronger empirical confirmation via the Recycle Bin icon diagnostic (documented in SUMMARY, not independently re-run by this verifier but consistent with the code's timing constants and the plan's LIVE-VERIFY design).

2. **Server Manager infra fix (SC#1/#2/#3 unblocking).** A genuine Phase 1 provisioning gap (Server Manager auto-launching maximized on every RDP logon, covering the desktop) was discovered during Plan 04's live run and fixed at the infra layer (`Configure-Target.ps1` step 4/5), not worked around in test code. **Assessment: correctly classified and fixed at the right layer** — verified the actual `.ps1` change lands the registry write using the same established idempotent pattern as the pre-existing DPI/SuppressWhenMinimized steps. This does not weaken any Phase 3 success criterion; it fixes an environmental blocker that was masking real Phase 3 behavior, and benefits Phases 4-6 as well.

3. **INPUT-01 vocabulary-mapping wrapper deferred.** See Requirements Coverage above — legitimate, pre-declared scope decision (documented in `03-CONTEXT.md` before implementation began), not a shipped-vs-claimed discrepancy. Not treated as a gap against this phase's 4 ROADMAP success criteria, none of which mention the Anthropic/OpenAI vocabulary schema by name.

## Gaps Summary

No gaps found. All 4 ROADMAP success criteria are genuinely achieved in the codebase: the owned `MouseAction`/`KeyAction` API exists and is wired end-to-end through `Session::send_mouse`/`send_key` into the session loop; the coordinate contract is enforced (not merely documented) via a bounds-check that runs before any PDU is constructed; and the claimed live-validation result (10/10 pass on a real Azure VM) is consistent with everything independently verifiable from this workstation — the gated test assertions genuinely test the claimed behaviors (screenshot-diff, not stubs), all commit hashes exist in git history, the infra fix is actually present in `Configure-Target.ps1`, and the offline `cargo test -p rdpilot` suite (46 passed, 10 correctly gated-ignored) was re-run fresh in this verification session and matches the SUMMARY's claimed counts exactly. The two live-validation carve-outs (Scroll/DoubleClick/Drag round-trip-only acceptance, Server Manager infra fix) are honestly documented, correctly scoped, and do not weaken any success criterion's actual claim.

---

_Verified: 2026-07-08_
_Verifier: Claude (gsd-verifier)_
