# Phase 9: Scripted Proof Harness - Research

**Researched:** 2026-07-10
**Domain:** Composition of an existing Rust SDK's public API into a scripted, no-LLM proof harness (example binary + gated integration test) against a real remote-only Windows program (7-Zip File Manager) over RDP.
**Confidence:** HIGH (this phase has zero new external dependencies, zero new library research surface — it composes Phase 2-8's already-live-verified `Session` API. All findings below are `[VERIFIED: codebase]` against files in this repo, not third-party docs.)

## Summary

Phase 9 is a pure composition phase: no new crates, no new wire protocol, no new public types (barring the D-9.1 spike's flagged exception). The two things that make this phase non-trivial are (1) an *unknown* — whether 7-Zip File Manager's UIA tree is fidelity-adequate under the existing `TreeScope_Children`-only scope (D-7.4), which can only be resolved by a live throwaway spike modeled exactly on the proven Phase 7 D-7.5 risk-gate pattern — and (2) a *mechanical* Rust question — how to share one harness function between an `examples/*.rs` binary crate root and a `tests/*.rs` integration-test crate root, which Cargo does not support natively via `mod` and requires an explicit `#[path = "..."]` module declaration.

Every reusable building block already exists and is proven live: `Session::connect`/`deploy_and_launch`/`launch_process`/`get_window_list`/`get_uia_tree`/`send_mouse`/`send_key`/`screenshot`/`close`, the `launch_notepad_and_find_window` poll-loop idiom (direct template for `launch_7zip_and_find_window`), the `region_changed`/`clamped_rect`/`changed_fraction` screenshot-diff helpers (direct template for D-9.2's navigate-then-verify step), and the `require_target!()`/`common::load_config()` live-gate machinery. The `examples/screenshot.rs` binary (137 lines) is the literal shape template for `examples/proof_harness.rs`.

**Primary recommendation:** Run the D-9.1 fidelity spike FIRST as a throwaway, disposable-VM, dump-and-inspect step (no assertion code, no harness code) — exactly mirroring 07-03-PLAN.md's structure (spike task + blocking checkpoint:human-verify task, VM up → spike → capture → VM down). Only after the spike's PASS/fallback decision is recorded should the planner design the shared harness function, its physical module location (recommend: a new `crates/rdpilot/tests/support/proof_harness.rs` module reachable from both call sites via `#[path]`), and the concrete SC#2/SC#3 assertions.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| RDP connect/auth | SDK core (`Session::connect`) | — | Already built (Phase 2); harness only calls it |
| Sensor deploy/launch | SDK core (`Session::deploy_and_launch`) | — | Already built (Phase 5); harness only calls it |
| Screenshot capture | SDK core (client-side framebuffer) | — | No sensor round trip; already built (Phase 2/6) |
| Window enumeration / UIA tree / process launch | Remote sensor (C# NativeAOT, over DVC) | SDK core (typed wrapper) | Already built (Phase 6/7); harness only calls the typed `Session` methods, never touches the wire |
| Navigation input injection | SDK core (`Session::send_mouse`/`send_key`) | — | Already built (Phase 3); harness composes existing calls |
| Fidelity spike (D-9.1) | Throwaway example/test-adjacent script | Disposable Azure VM | New, but structurally identical to the Phase 7 D-7.5 spike — not real harness code |
| Shared harness function (D-9.3) | New module, physically outside `src/` (see Pattern 1) | `examples/proof_harness.rs` + `tests/live_session.rs` callers | New composition surface — the phase's only genuinely new code |
| Pass/Fail report (D-9.4) | `examples/proof_harness.rs` (stdout + `ExitCode`) | Shared harness fn (returns a report value) | No new public SDK type required — report is example/test-local |

## Standard Stack

### Core
No new dependencies. Phase 9 reuses the crate's existing dependency graph unchanged (`ironrdp*` internal, `tokio`, `serde`/`serde_json` for report/trace shaping if desired, `image` transitively via `Screenshot::to_png`). `[VERIFIED: codebase]` — `crates/rdpilot/Cargo.toml` (see contents below) has no gap requiring a new crate for anything Phase 9 needs.

### Supporting
None. `dev-dependencies` already include `tokio` (macros/rt-multi-thread) and `serial_test` — sufficient for a new `#[ignore]` test.

### Alternatives Considered
None applicable — this phase has no library-selection decision. The only "alternative" evaluated is *how* to share code between an example and a test crate root (see Pattern 1), which is a language-mechanics question, not a stack question.

**Installation:** None required.

**Version verification:** N/A — no new packages.

## Package Legitimacy Audit

Not applicable — Phase 9 installs zero new packages. Skip the Package Legitimacy Gate protocol entirely; there is nothing to audit.

## Architecture Patterns

### System Architecture Diagram

```
 [examples/proof_harness.rs]              [tests/live_session.rs]
   main() -> ExitCode                       #[ignore] fn proof_harness_end_to_end()
   load_config() (local, like screenshot.rs)  require_target!() -> Option<cfg>
        |                                        |
        v                                        v
   ConnectionConfig ---------------------> ConnectionConfig
        |                                        |
        +-------------------+-------------------+
                             v
              rdpilot::Session::connect(&cfg)
                             |
                             v
              session.deploy_and_launch()   (sensor bootstrap over RDPDR/WinRM)
                             |
                             v
        +--------------------------------------------+
        |     SHARED HARNESS FN (new module, D-9.3)   |
        |  async fn run_proof_harness(&Session)       |
        |    -> ProofReport                           |
        |                                              |
        |  1. session.screenshot()            (SC#1)  |
        |  2. session.launch_process("7zFM.exe",...)  |
        |     + poll session.get_window_list()        |
        |       until 7zFM window appears  (SC#4 step) |
        |  3. session.get_uia_tree(hwnd)      (SC#2)  |
        |     -> assert named elements + valid bbox   |
        |  4. session.send_mouse/send_key(...)(SC#3)  |
        |     navigate: open "File" menu (D-9.2)      |
        |  5. poll-with-timeout verify:                |
        |     session.screenshot() diff, OR            |
        |     session.get_uia_tree(hwnd) re-query      |
        |     (D-9.5 bounded poll, NOT fixed sleep)    |
        |  6. build stdout trace + PASS/FAIL (D-9.4)  |
        +--------------------------------------------+
                             |
                 +-----------+-----------+
                 v                       v
      examples: print trace,   tests: assert!() on the
      map to ExitCode          returned ProofReport,
                                session.close()
```

### Recommended Project Structure
```
crates/rdpilot/
├── examples/
│   └── proof_harness.rs        # main() -> ExitCode; connect, deploy, call shared fn, print trace, close
├── tests/
│   ├── common/mod.rs           # UNCHANGED — load_config()/sensor_exe_path()/idle_secs() (D-9.7 reuse)
│   ├── support/                # NEW (recommended) — shared harness module, reachable via #[path] from both crate roots
│   │   └── proof_harness.rs    # pub async fn run_proof_harness(session: &rdpilot::Session) -> ProofReport
│   └── live_session.rs         # existing 1647-line suite + one new #[ignore] test that calls the shared fn
```

### Pattern 1: Sharing one async fn between an `examples/` binary and a `tests/` integration test (the D-9.3 core mechanic)

**What:** Cargo compiles every top-level file directly under `examples/` as an independent binary crate root, and every top-level file directly under `tests/` as an independent integration-test crate root (`tests/common/mod.rs` is only reachable from `tests/*.rs` files via `mod common;` because Cargo does NOT auto-compile `tests/common/mod.rs` as its own test binary — the `common/` convention specifically avoids that; `examples/` has no equivalent auto-exclusion mechanism for subdirectories used as pure modules unless the binary itself lives at `examples/<name>/main.rs`). Neither compilation unit can `mod` a file living under the other's directory using a bare relative path — Rust's `mod` resolution is relative to the *current crate root*, and an `examples/*.rs` file and a `tests/*.rs` file are always different crate roots. The `#[path = "..."]` attribute is the standard, well-established workaround: it lets any crate root declare a module whose source lives at an arbitrary relative path, breaking the default `mod name;` -> `./name.rs` convention.

**When to use:** Whenever the *exact same* async logic must run identically from both a human-facing example binary and a machine-gated integration test — precisely D-9.3's constraint ("single shared harness function... called by BOTH").

**Example:**
```rust
// crates/rdpilot/tests/support/proof_harness.rs
// NOTE: this file is compiled TWICE — once inside the `examples/proof_harness`
// binary crate, once inside the `tests/live_session` test crate. It has NO
// access to `crate::` meaning "the rdpilot library" — from both call sites,
// the library is an EXTERNAL crate reached via `rdpilot::...` (see Pitfall
// below). Treat this module as if it lives outside the workspace entirely.
use rdpilot::{Session, UiaElement, WindowInfo};

pub struct ProofReport {
    pub steps: Vec<(&'static str, bool, String)>, // (step name, pass?, detail)
    pub passed: bool,
}

pub async fn run_proof_harness(session: &Session) -> ProofReport {
    // 1. screenshot (SC#1) -> 2. launch_process + poll (SC#4 prep) ->
    // 3. get_uia_tree assertions (SC#2) -> 4. navigate (SC#3) ->
    // 5. poll-with-timeout verify (D-9.5) -> 6. assemble ProofReport (D-9.4)
    todo!("planner fills in exact assertions post D-9.1 spike")
}
```

```rust
// crates/rdpilot/examples/proof_harness.rs
#[path = "../tests/support/proof_harness.rs"]
mod proof_harness;

use std::process::ExitCode;
use rdpilot::{ConnectionConfig, Session};

fn main() -> ExitCode {
    // same current-thread runtime pattern as examples/screenshot.rs
    // ... connect, deploy_and_launch, then:
    // let report = runtime.block_on(proof_harness::run_proof_harness(&session));
    // print trace, map report.passed -> ExitCode::SUCCESS/FAILURE
    ExitCode::SUCCESS
}
```

```rust
// crates/rdpilot/tests/live_session.rs (new test, appended)
#[path = "support/proof_harness.rs"]
mod proof_harness;

#[test]
#[ignore = "live: requires a provisioned RDP target + published rdpilot-sensor.exe (RDPILOT_LIVE=1)"]
fn proof_harness_end_to_end() {
    let Some(cfg) = require_target!("proof_harness_end_to_end") else { return };
    // ... existing cfg.sensor_binary_path(...) + connect + deploy_and_launch pattern ...
    block_on(async {
        let report = proof_harness::run_proof_harness(&session).await;
        assert!(report.passed, "proof harness failed: {:?}", report.steps);
        session.close().await.expect("close");
    });
}
```

The `#[path]` value is relative to the *declaring file's directory*: from `examples/proof_harness.rs` (in `examples/`), `../tests/support/proof_harness.rs` reaches `tests/support/proof_harness.rs`; from `tests/live_session.rs` (in `tests/`), `support/proof_harness.rs` reaches the same file directly. `[VERIFIED: codebase understanding of Cargo's target auto-discovery + Rust's `#[path]` attribute semantics — this is standard, widely-documented Rust module-system behavior, not project-specific]`.

**Location alternatives (all valid, D-9.3 leaves this to planner discretion):**
| Location | Pros | Cons |
|----------|------|------|
| `tests/support/proof_harness.rs` (recommended) | Physically near `tests/common/`; obviously test-support, not example-only; both `#[path]` references are short | Slight asymmetry — "lives under tests/ but is used by examples/" reads oddly to a first-time reader without a doc comment |
| `examples/proof_harness/harness.rs` (with `examples/proof_harness/main.rs` as the actual binary) | Self-contained under `examples/`; Cargo auto-discovers `examples/proof_harness/main.rs` as target `proof_harness` with zero extra Cargo.toml wiring | `tests/live_session.rs` must reach *up and across* (`#[path = "../examples/proof_harness/harness.rs"]`), which reads backwards (tests depending on examples/ is unusual) |
| A brand new top-level dir, e.g. `crates/rdpilot/harness/proof_harness.rs` | Symmetric — neither caller "owns" it; matches the "neither test-only nor example-only" nature of the code | New directory convention with no established meaning to Cargo or to a reader; needs an explicit doc comment explaining why it's neither `src/`, `tests/`, nor `examples/` |

**Pitfall to flag explicitly:** inside the shared module, do NOT write `crate::Session` — `crate` refers to whichever binary/test crate root is currently compiling the module (the example binary or the test binary), neither of which re-exports `Session`. Always import via the external crate name: `use rdpilot::Session;` (exactly as `tests/live_session.rs` already does at its top, `crates/rdpilot/tests/live_session.rs:21-24`).

### Pattern 2: The D-7.5 risk-gate spike template (for D-9.1)

**What:** Before any assertion code is written, run a throwaway, disposable-VM, dump-and-inspect step that resolves the single highest-uncertainty question, gated by a blocking `checkpoint:human-verify` task, with explicit up-front fallback criteria.

**Concrete Phase 7 template (07-03-PLAN.md, `.planning/phases/07-uia-tree-module/07-03-PLAN.md`):**
- Task 1 (`type="auto"`): provision the disposable VM (`infra/manage-env.ps1 up -VmSize Standard_B2s_v2`), AOT-publish/run the throwaway probe ON the VM (`az vm run-command invoke` — the proven pattern since WinRM auth fails from this Linux dev host), iterate fixes until a documented PASS line prints, with an explicit isolation order for diagnosing failures (task 1's `<action>` lists exactly which sub-check to inspect first).
- Task 2 (`type="checkpoint:human-verify" gate="blocking"`): pause for the operator to review the captured PASS output, record what was learned, then tear the VM down and confirm its resource group is absent before proceeding.
- `<verify>` and `<success_criteria>` sections state explicitly what "the risk is retired" means.

**Applying this to D-9.1:** the spike is NOT "launch 7-Zip and write the harness" — it's narrower: launch 7zFM.exe on a disposable VM (via the already-proven `deploy_and_launch()` + `launch_process("7zFM.exe", None, None)` + `launch_notepad_and_find_window`-style poll, adapted to match on 7-Zip's window instead of Notepad's), call `session.get_uia_tree(hwnd)` once, and **dump the flat `UiaElement[]` to stdout/eprintln for manual human inspection** — no assertions, no pass/fail logic yet. The blocking checkpoint is where the human inspects the dumped tree and either (a) approves 7-Zip as the SC#2 target and names the specific elements (menu bar "File" item, a toolbar button, the listview) to assert on downstream, or (b) invokes the fallback to Notepad per D-9.1's explicit criteria ("opaque/unlabeled toolbar controls with no distinguishing name/role").

**Fallback criteria to make explicit in the spike task** (already stated in CONTEXT D-9.1, restate verbatim in the plan): fall back to Notepad ONLY if 7-Zip's tree proves genuinely inadequate — e.g., toolbar buttons all report `role: "Button"` with an empty or generic `name` (making them visually indistinguishable in the flat array), or the menu bar's "File" item is not present/not distinguishable at `TreeScope_Children` depth.

**Also flagged in this spike (Deferred item, do not silently absorb):** if the spike shows the meaningful elements (e.g. listview rows, deep toolbar structure) sit BELOW `TreeScope_Children` depth (D-7.4's locked scope), that is an explicit scoped-and-flagged capability addition — a caller-configurable deeper walk — not something the planner silently designs around. Record the finding either way (adequate-as-is vs. needs-deeper-walk) in the spike's SUMMARY.

### Anti-Patterns to Avoid
- **Writing assertion/harness code before the D-9.1 spike completes:** the whole point of the spike gate is that SC#2's exact element names/roles and D-9.2's exact navigation target are *outputs* of the spike, not inputs to it. Writing `assert_eq!(role, "MenuItem")`-style code before seeing a real dump risks guessing wrong against a real Win32 non-instrumented app (the same class of app Notepad already proved surprising in Phase 7 D-7.2's `AutomationId`-empty finding).
- **Fixed `tokio::time::sleep` instead of poll-with-timeout for the navigate→verify step (D-9.5):** the project has an explicit anti-pattern backlog item (999.1) filed against exactly this; every existing "wait for remote state to settle" idiom in this codebase (`launch_notepad_and_find_window`, `FOREGROUND_CONFIRM_ATTEMPTS` in `set_foreground_window_confirmed_by_followup_query`) is a *bounded poll loop with an `eprintln` progress line*, never a single blind sleep for the *decisive* wait. (A short, small, *fixed* `settle()` sleep is still fine as an inter-step throttle — see `SETTLE = 800ms` in `live_session.rs:269` — that pattern is not what 999.1 targets; 999.1 targets using a fixed sleep as the ONLY mechanism to confirm a state transition completed.)
- **Reusing `require_target!()` outside `tests/live_session.rs`:** the macro is a local `macro_rules!` defined inline in that file (`live_session.rs:28-42`), not exported from `tests/common`. `examples/proof_harness.rs` cannot call it — it needs its own config-loading path, mirroring `examples/screenshot.rs`'s local `load_config()` (`examples/screenshot.rs:96-137`), which already duplicates (deliberately, per that file's own doc comment) the `.secrets/connection.json` parsing rather than trying to reuse `tests/common`.
- **Putting the shared harness function inside `src/` as a new public API item:** D-9.3's phrasing ("Any harness code that lives in the library crate obeys D-09...") is a conditional guard, not an instruction to put it there. Given the harness is composition-only (no new capability), keeping it in a `tests/`-or-adjacent module (Pattern 1) avoids expanding the locked public API surface (D-09, Phase 2) for zero benefit — this is the RESEARCH recommendation; the planner may still choose `src/` if a concrete reason emerges (none is currently visible).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Waiting for the 7-Zip window to appear after `launch_process` | A new bespoke poll loop from scratch | Adapt `launch_notepad_and_find_window` (`tests/live_session.rs:1222-1242`) — same `session.get_window_list()` poll, swap the match predicate from `class_name.eq_ignore_ascii_case("Notepad")` to 7-Zip's actual class name/title (confirm via the D-9.1 spike's window-list dump — 7-Zip's window class is historically `#32770`-adjacent or an MFC-style class; do not assume, read it live) | The poll idiom (bounded attempts, `settle()` between, `eprintln!` progress, `panic!` with full context on exhaustion) is already correct and proven live across Phase 6/7/8 |
| Proving a navigation action visibly changed the remote desktop | A screenshot pixel-hash / perceptual-diff library | `region_changed`/`changed_fraction`/`clamped_rect` (`tests/live_session.rs:279-325`) | Already implemented, already live-tuned (`REGION_CHANGE_THRESHOLD = 0.01`), already used for an almost-identical case (`mouse_click_activates_menu`'s right-click-opens-context-menu assertion, `tests/live_session.rs:333-383`) — D-9.2's menu-open default is structurally the same shape (click/key -> settle -> screenshot -> diff a bounded region) |
| Loading `.secrets/connection.json` in the example binary | A new shared secrets-loading crate/module | Copy the pattern already in `examples/screenshot.rs:96-137` verbatim (adjust only if a genuinely new field is needed) | The example crate root cannot `mod common;` into `tests/common` (different crate root) — duplicating this ~40-line function is the established, already-precedented approach in this exact codebase, not a violation of DRY worth fixing here |
| A JSON/structured report schema | Any report-serialization design | Plain `println!`/`eprintln!` step trace + a final `PASS`/`FAIL` line + non-zero `std::process::ExitCode` on failure | D-9.4 explicitly rules this out — "No JSON schema, no report file — no consumer exists yet" |

**Key insight:** every "hard" sub-problem in this phase already has a proven, live-verified reference implementation somewhere in `tests/live_session.rs` or `examples/screenshot.rs` from a prior phase. The actual net-new engineering surface of Phase 9 is small: (1) the D-9.1 spike's throwaway dump script, (2) the exact assertions the spike's findings dictate, (3) the `#[path]` module-sharing wiring, and (4) one new `launch_7zip_and_find_window` + navigation step. Nothing here justifies a new dependency or a new architectural pattern.

## Common Pitfalls

### Pitfall 1: Guessing 7-Zip's window class/title match predicate before the spike runs
**What goes wrong:** `launch_notepad_and_find_window` matches on `w.class_name.eq_ignore_ascii_case("Notepad") || w.title.to_ascii_lowercase().contains("notepad")`. 7-Zip File Manager's window class and title text are NOT yet confirmed live in this codebase (no prior phase launched 7zFM.exe programmatically — only Notepad).
**Why it happens:** It's tempting to hard-code a plausible guess (e.g. `"7zFM"` as a class name) based on general Windows knowledge.
**How to avoid:** The D-9.1 spike's window-list poll step should `eprintln!` the full matched `WindowInfo` (class_name + title) the first time it successfully finds a window after `launch_process("7zFM.exe", ...)`, so the real values are captured and hard-coded from an actual live observation, not assumed. Since 7zFM.exe is a single top-level window with no other windows racing to register (unlike Notepad, which is also simple), a title-substring match (e.g. `title.to_ascii_lowercase().contains("7-zip") || title.to_ascii_lowercase().contains(".zip")` — 7-Zip's title often reflects the current path/archive, e.g. `"7-Zip - C:\Program Files"` when seeded per D-9.6) is likely safer than a class-name guess.

### Pitfall 2: The D-9.1 spike accidentally becoming "real harness code"
**What goes wrong:** Because the spike necessarily launches 7zFM and calls `get_uia_tree`, it is tempting to leave the assertion logic in place afterward rather than treating it as genuinely throwaway (mirroring 07-02/07-03's separation: 07-02 built the interop scaffolding + smoke-test entry point, 07-03 was the actual gate run — but both were explicitly pre-handler, and 07-04 was the real handler built only after the gate passed).
**Why it happens:** Spike code that "basically works" is hard to discard.
**How to avoid:** Structure the spike plan exactly like 07-03-PLAN.md: Task 1 is `type="auto"` (dump-only, no assertions), Task 2 is `type="checkpoint:human-verify" gate="blocking"` (human reviews the dump and records the decision + which elements to target). The REAL harness function (Pattern 1) is then a SEPARATE, later plan/wave that consumes the spike's recorded findings — do not let the spike's throwaway script silently become `run_proof_harness`.

### Pitfall 3: 7-Zip's default UI state is non-deterministic across VM provisioning runs (D-9.6)
**What goes wrong:** 7zFM.exe's default file-listing view depends on the last-used folder / registry state, which may differ between a freshly provisioned VM and a VM that has been used before (unlikely for a disposable VM, but the *default* view on first launch may still show an empty or unpredictable location e.g. "Computer").
**Why it happens:** 7-Zip persists its last-visited path in the registry per-user; a brand-new disposable VM with a brand-new user profile has no such history, so the very first launch's default view is whatever 7-Zip's own hardcoded default is (historically "This PC"/drive list) — this IS deterministic across fresh VMs, but should not be assumed without the spike confirming it.
**How to avoid:** Per D-9.6, seed determinism at harness runtime: launch `7zFM.exe` with a path argument (`session.launch_process("7zFM.exe", Some("C:\\Program Files"), None)` — confirm 7-Zip's actual CLI argument-passing behavior for a starting directory during the spike; 7-Zip File Manager's classic invocation is `7zFM.exe <path>` to open at that folder) rather than relying on whatever the bare launch shows. If the spike's chosen navigation target (D-9.2) is menu-bar/toolbar chrome (always present regardless of file-listing content), seeding may be entirely unnecessary — decide during the spike checkpoint, not before.

### Pitfall 4: Alt-key menu activation may need `KeyAction::Combo` semantics, not a plain click
**What goes wrong:** If D-9.2's spike-chosen navigation lands on "open the File menu via keyboard" (e.g. `Alt+F`) rather than a mouse click on the menu bar's bounding box, note that `Key` (`crates/rdpilot/src/input.rs`) has `Alt` and `F` as separate `Key` variants — `KeyAction::Combo(vec![Key::Alt, Key::F])` is how this project already expresses held-modifier combos (`Key::Ctrl, Key::A` and `Key::Alt, Key::F4` are the existing SC#3-proven Phase 3 examples). A raw `Alt` press-then-release followed by a separate `F` keypress is a DIFFERENT semantic (Windows menu-mnemonic activation via a *released* Alt, then the letter) — if the spike's chosen approach needs classic Win32 menu-mnemonic behavior rather than a modifier combo, this may need live tuning; do not assume `Combo(vec![Alt, F])`'s held-then-released ordering (press Alt, press F, release F, release Alt — per D-3.5) reproduces the same UI effect as tapping Alt separately then F. **Recommendation:** prefer the mouse-click-on-UIA-bbox path for D-9.2's default (click the "File" menu bar item's `UiaElement::bbox` center via `MouseAction::Click`) — it sidesteps this ambiguity entirely and is the more literal reading of ROADMAP SC#3's own example ("menu open, button click"). `[ASSUMED]` — this specific Alt+letter-mnemonic semantic distinction is based on general Win32 knowledge, not verified live in this codebase; flag for the D-9.2 spike/live-tuning to confirm empirically if keyboard-based menu activation is chosen instead.

### Pitfall 5: The shared harness function's error type must not leak SDK-internal detail inconsistently between the two callers
**What goes wrong:** `examples/proof_harness.rs` wants a `String`-mapped error path (matching `screenshot.rs`'s `Result<(), String>` pattern) for clean `eprintln!`/`ExitCode` handling, while `tests/live_session.rs` wants `.expect()`-style panics (matching the rest of that 1647-line file's established idiom, which the crate's `lib.rs` inner `#![deny(clippy::expect_used)]` deliberately does NOT apply to, per `lib.rs:20-31`). A shared function returning `rdpilot::Result<T>` (the SDK's own `Result` alias) works cleanly for BOTH — the example maps it with `.map_err(|e| format!(...))` exactly like its other calls already do, and the test can `.expect(...)` on it exactly like its other calls already do.
**Why it happens:** Trying to design one bespoke report/error type shared across both callers, when the SDK's existing `Result<T, Error>` already composes fine with each caller's own existing idiom.
**How to avoid:** Have `run_proof_harness` return something like `rdpilot::Result<ProofReport>` where `ProofReport` carries the step trace and a `passed: bool` (or make individual step failures propagate as `Err` immediately and let the caller decide FAIL reporting — either shape is planner discretion per D-9.4, but keep the *error type* itself the SDK's own `Result`, not a bespoke new error enum).

### Pitfall 6: Forgetting the sensor publish precondition
**What goes wrong:** Every existing sensor-backed live test (`launch_process`, `get_window_list`, `get_uia_tree`, etc.) asserts `sensor_exe.exists()` BEFORE connecting, with a clear failure message pointing at the exact `dotnet publish` command, and calls `cfg.sensor_binary_path(sensor_exe)` before `Session::connect`. Phase 9's harness needs `launch_process`/`get_window_list`/`get_uia_tree`, so it has the SAME precondition as every Phase 6/7/8 live test — easy to forget when composing a "new" harness from scratch instead of copy-adapting an existing test.
**How to avoid:** Copy the exact preamble from any Phase 7/8 live test (e.g. `tests/live_session.rs:1252-1262`, the `uia_tree_returns_populated_elements` preamble) verbatim into the new gated test; the example binary needs the equivalent (`common::sensor_exe_path()`'s logic reimplemented locally, or read `RDPILOT_SENSOR_EXE`/the same default path directly, since `examples/` cannot `mod common;` either — see Pattern 1's Pitfall).

### Pitfall 7: NativeAOT build/run only happens on Windows — no Linux-runnable automated proxy exists for this phase's live gate
**What goes wrong:** Exactly as documented for every prior phase's live gate (07-03-PLAN.md's `<verify><automated>MISSING`), this dev host is Linux (`x86_64-pc-windows-gnu` cross-compile target, confirmed in STATE.md's Build toolchain decision) — the sensor binary must be AOT-published and the harness executed against a REAL Windows target. There is no offline substitute for the actual pass/fail proof; only the Rust-side code (harness composition logic minus the live network calls) can be compiled/type-checked offline.
**How to avoid:** Plan the phase with an explicit offline-compile-check wave (mirroring every prior phase's Wave 1) followed by a live-gate wave that follows D-9.7's infra path exactly (see Environment Availability below).

## Code Examples

### Poll-with-timeout idiom (D-9.5) — direct reusable template
```rust
// Source: crates/rdpilot/tests/live_session.rs:1222-1242 (launch_notepad_and_find_window)
// Adapt the match predicate for 7-Zip; same bounded-attempts + settle() + eprintln! shape
// reused for the navigate-then-verify step (poll get_uia_tree or screenshot instead of
// get_window_list until the expected post-navigation state is observed, or exhaust
// attempts and fail with full diagnostic context).
async fn launch_7zip_and_find_window(session: &rdpilot::Session) -> WindowInfo {
    session
        .launch_process("7zFM.exe", Some("C:\\Program Files"), None) // D-9.6 seeding — confirm arg form at spike time
        .await
        .expect("launch_process(7zFM.exe) should return a PID");

    for attempt in 0..20 {
        settle().await; // 800ms, tests/live_session.rs:269-273
        let windows = session.get_window_list().await.expect("get_window_list should round-trip while polling");
        if let Some(w) = windows.iter().find(|w| /* confirmed live during D-9.1 spike */ w.title.to_ascii_lowercase().contains("7-zip")) {
            return w.clone();
        }
        eprintln!("[poll] 7-Zip window not yet visible (attempt {attempt})");
    }
    panic!("7-Zip window did not appear in get_window_list after polling");
}
```

### Screenshot-diff navigate-verify idiom (D-9.2/D-9.5 default) — direct reusable template
```rust
// Source: crates/rdpilot/tests/live_session.rs:333-383 (mouse_click_activates_menu),
// region_changed/clamped_rect at lines 307-325.
let before = session.screenshot().await.expect("screenshot before navigate");
session.send_mouse(MouseAction::Click { x, y, button: Button::Left }).await.expect("click round-trips");
settle().await;
let after = session.screenshot().await.expect("screenshot after navigate");
let target_rect = clamped_rect(x as u32, y as u32, 300, 400, desktop_w, desktop_h);
assert!(region_changed(&before, &after, target_rect), "navigation action produced no visible change (SC#3)");
```

## State of the Art

Not applicable — this phase reuses only this project's own already-live-verified code from prior phases. No external ecosystem "state of the art" question exists here; the SDK's dependency versions (IronRDP 0.15 umbrella, etc.) are unchanged and were already verified current as of Phase 2/4/5 research.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | 7-Zip File Manager's CLI accepts a starting-folder argument as `7zFM.exe <path>` (used for D-9.6 seeding in the code example) | Pitfall 3, Code Examples | Low — the D-9.1 spike will empirically confirm or refute this before any real harness code depends on it; if wrong, seeding falls back to a UIA/keyboard navigation step inside 7-Zip instead of a launch argument |
| A2 | `KeyAction::Combo(vec![Key::Alt, Key::F])`'s held-then-released semantics may not reproduce classic Win32 Alt-then-letter menu-mnemonic activation | Pitfall 4 | Low-Medium — only relevant if the spike/planner chooses keyboard-based (not mouse-click) menu activation for D-9.2; the RESEARCH recommendation is to prefer mouse-click-on-UIA-bbox specifically to sidestep this uncertainty |
| A3 | 7-Zip File Manager's window title on a freshly seeded launch will contain a recognizable substring (e.g. "7-zip" or the seeded path) usable for the window-list poll match predicate | Pitfall 1, Code Examples | Low — the spike is explicitly tasked with capturing and recording the real observed title/class before any real match predicate is written; the code example is illustrative only |

**If this table is empty:** N/A — see entries above. All three are LOW-risk because the D-9.1 spike (already mandated by CONTEXT D-9.1) is structurally positioned to empirically resolve every one of them before real harness code is written.

## Open Questions

1. **Exact SC#2 element assertions and SC#3 navigation target**
   - What we know: the safe-default recommendation (D-9.2) is menu-open of 7zFM's "File" menu, verified via submenu appearance; SC#2 wants "specific named elements... with valid bounding boxes."
   - What's unclear: which exact elements (by name/role) 7-Zip's UIA tree actually exposes at `TreeScope_Children` depth — cannot be known without the live spike.
   - Recommendation: the planner should NOT hard-code specific element names/roles in the plan's task descriptions beyond "whatever the D-9.1 spike's checkpoint records" — make the spike's SUMMARY.md the explicit input to the assertion-writing wave/plan.

2. **Whether 7-Zip needs D-9.6 UI seeding at all**
   - What we know: if D-9.2 lands on menu-bar/toolbar chrome (always present regardless of file-listing content), seeding is likely unnecessary.
   - What's unclear: whether the default (unseeded) 7zFM view has ANY quirk (e.g., a first-run dialog, an update-check prompt) that could interfere even with a menu-chrome-only navigation target — 7-Zip is not known to show first-run dialogs, but this has not been confirmed live in THIS codebase/VM image.
   - Recommendation: have the D-9.1 spike's dump also `eprintln!` the very first screenshot after launch (not just the UIA dump) so any unexpected first-run UI is caught visually during the same spike, at zero extra cost.

3. **Physical location of the shared harness module (Pattern 1's three options)**
   - What we know: D-9.3 explicitly leaves this to planner discretion; three viable options are laid out above with tradeoffs.
   - What's unclear: no strong technical reason favors one over another — this is a naming/discoverability judgment call.
   - Recommendation: `tests/support/proof_harness.rs` (this RESEARCH's stated recommendation) — mirrors the existing `tests/common/` precedent for "shared, not itself a test" code, and keeps `examples/proof_harness.rs` a single, short, example-reader-friendly file (matching `examples/screenshot.rs`'s existing simplicity).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Disposable Azure Windows VM (`infra/manage-env.ps1 up`) | D-9.1 spike + the phase's live gate | ✗ (not currently provisioned on this dev host) | — | None — this phase's success criteria are inherently live; `manage-env.ps1 up -VmSize Standard_B2s_v2` must be run (default `Standard_B2ms` is `SkuNotAvailable` in westeurope per STATE.md's recorded pending todo) |
| `RDPILOT_LIVE` env var + `.secrets/connection.json` | Gating the new `#[ignore]` test and enabling `examples/proof_harness.rs` to find a target | ✗ (gitignored, VM-dependent) | — | None — produced by `manage-env.ps1 up`; see `tests/common/mod.rs:65-112`'s `load_config()` contract for the exact schema (`host`, `user`, `password`, `rdpPort`, optionally `winrmPort`) |
| Published `rdpilot-sensor.exe` (win-x64 NativeAOT) | Every sensor-backed call (`launch_process`, `get_window_list`, `get_uia_tree`) | ✗ (build artifact, not committed) | — | None for AOT cross-compile from this Linux host (confirmed repeatedly in Phase 5-8: `az vm run-command invoke` builds it ON the VM itself, then a short-lived Storage blob SAS relays the artifact back) — this is the established, working pattern, not a gap to solve |
| .NET 8 SDK (for the win-x64 NativeAOT publish) | Sensor build | Runs on the VM via `az vm run-command invoke`, not on this dev host | — | Established pattern (see above); no local .NET SDK needed on this Linux host |
| `x86_64-pc-windows-gnu` Rust target + MinGW gcc | Building/running the SDK-side (`examples/proof_harness.rs`, the new `#[ignore]` test) for real against the VM | Per STATE.md's Pending Todos: "Future agents on this machine must export the scoop rustup env... and have MinGW gcc on PATH" — NOT confirmed present in THIS session's environment | — | The offline compile-check (Rust type-checking/build) can also validate on the native Linux target when no `cfg(windows)` code is involved, per the repeated precedent in STATE.md ("perception.rs/session.rs/sensor.rs have no cfg(windows) code so this is a safe substitute" — Phase 6/7 finding) — the harness composition code is expected to be equally `cfg`-agnostic |

**Missing dependencies with no fallback:**
- A live disposable VM and its `.secrets/connection.json` — required for the D-9.1 spike and for the phase's actual live gate; there is no way to satisfy PROOF-01's success criteria without a real live run, per the phase's own goal statement ("against a REAL Windows target").

**Missing dependencies with fallback:**
- Rust-side offline compilation can proceed on the native Linux host target (as every prior phase has done) for everything except the actual live execution — this is a well-established, low-risk substitution already used 06/07/08 times over.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | `cargo test` (Rust built-in) + `#[ignore]`-gated live integration tests, exactly as established Phases 2-8 |
| Config file | none — no `pytest.ini`/`jest.config`-equivalent; gating is via the `RDPILOT_LIVE` env var + `.secrets/connection.json` presence (`tests/common/mod.rs`'s `load_config()`) |
| Quick run command | `cargo test -p rdpilot --lib` (offline unit tests only — no live gate) |
| Full suite command | `RDPILOT_LIVE=1 RDPILOT_IDLE_SECS=600 cargo test -p rdpilot -- --include-ignored --test-threads=1` (the established canonical live-gate invocation, `tests/live_session.rs:1-10`) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| PROOF-01 SC#1 | Connect + authenticate + produce a screenshot of a real remote-only program | live | `RDPILOT_LIVE=1 cargo test -p rdpilot proof_harness_end_to_end -- --ignored --test-threads=1` | ❌ new test, this phase |
| PROOF-01 SC#2 | UIA tree for 7-Zip's main window with named elements + valid bboxes | live | (same test — one assertion group within it, per D-9.4's single end-to-end trace) | ❌ new test, this phase |
| PROOF-01 SC#3 | Navigation action + follow-up verify | live | (same test — spike-dependent exact assertion, D-9.2) | ❌ new test, this phase |
| PROOF-01 SC#4 | Full loop completes with pass/fail report + exit code | manual + live | `examples/proof_harness.rs` run directly: `RDPILOT_LIVE=1 cargo run -p rdpilot --example proof_harness` (human reads stdout trace + checks exit code) | ❌ new example, this phase |

### Sampling Rate
- **Per task commit:** offline compile-check (`cargo build -p rdpilot --tests --examples`) — no VM needed for this.
- **Per wave merge:** N/A for offline waves; the live-gate wave IS the phase-completion signal (no earlier wave can partially validate this phase's success criteria, since every SC is inherently live).
- **Phase gate:** the D-9.1 spike checkpoint (Wave N, mirroring 07-03) PLUS the final live-gate run of `proof_harness_end_to_end` and a manual run of `examples/proof_harness.rs`, both against the same or a fresh disposable VM, before `/gsd-verify-work`.

### Wave 0 Gaps
- `tests/support/proof_harness.rs` (or the planner's chosen alternative location) — the shared harness module itself; does not exist yet.
- A new `#[ignore]` test appended to `tests/live_session.rs` — does not exist yet.
- `examples/proof_harness.rs` — does not exist yet.
- No new test-framework install needed — `cargo test` + the existing `tests/common` + `serial_test`/`tokio` dev-dependencies already cover everything this phase needs mechanically.

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-------------------|
| V2 Authentication | Indirect (reuses existing `ConnectionConfig`/NLA-CredSSP path, no new auth surface) | Already covered by Phase 2's `Session::connect` — Phase 9 adds no new credential handling |
| V3 Session Management | No | Phase 9 reuses `Session::close()` teardown; no new session state |
| V4 Access Control | No — this is a local SDK harness, not a multi-tenant service | N/A |
| V5 Input Validation | Marginal — the harness's own navigation coordinates/hwnd values come from the SDK's own typed returns (`WindowInfo`, `UiaElement`), not untrusted external input | No new validation surface; existing `Error::CoordinateOutOfBounds` (Phase 3) already guards `send_mouse` |
| V6 Cryptography | No | Unchanged — TLS/rustls path is Phase 2's, untouched here |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|----------------------|
| Credential leakage via `examples/proof_harness.rs`'s stdout trace | Information Disclosure | Follow `examples/screenshot.rs`'s own explicit doc-comment discipline verbatim: "It NEVER prints the password (the config's `Debug` redacts it). Do not log credentials from here." — the new example must carry the identical constraint; `ConnectionConfig`'s `Debug` impl already redacts the password (`crates/rdpilot/src/config.rs` — confirmed via `config.rs:199`'s `Debug` field listing pattern already excluding raw password printing) |
| A left-running disposable VM after the D-9.1 spike or the phase's live gate | Information Disclosure / cost | Every prior phase's live-gate discipline applies unchanged: `infra/manage-env.ps1 down` + `az group exists -n rdpilot-test` confirmed `false` before the phase is considered done (the blocking-checkpoint pattern from 07-03-PLAN.md Task 2, Step 4) |
| Leaving the D-9.1 spike's throwaway assertion/dump code in the final harness (scope creep beyond composition) | Tampering (of scope, not memory) | Explicit CONTEXT boundary: "Phase 9 is pure composition + assertion authoring... UNLESS the D-9.1 fidelity spike proves 7-Zip's UIA tree needs a capability the SDK doesn't have... in which case that's a scoped, flagged addition — not a redesign" — the planner must flag any such addition explicitly, not silently fold it in |

## Project Constraints (from CLAUDE.md)

- **GSD workflow enforcement:** all file-changing work must go through a GSD entry point (`/gsd-execute-phase` for planned phase work) — not a Phase 9-specific constraint, but a standing project rule the planner/executor must continue to honor.
- **Owned-types-only public API (D-09):** no `ironrdp`/`image`/`rustls` type may appear in any new `pub` signature — applies to the shared harness function's signature if it is ever promoted to a library type (it should not be, per this RESEARCH's Pattern 1 recommendation to keep it in `tests/support/`).
- **No `unwrap`/`expect`/`unsafe` in library code (API-01):** enforced at compile time via `lib.rs`'s inner `#![deny(...)]` attributes, scoped ONLY to the library crate's own compilation unit (`lib.rs:20-31`) — this does NOT extend to `tests/*.rs` or `examples/*.rs` crate roots, so the shared harness module and both callers may use `.expect()` freely, matching the existing 116 `.expect()` calls already in `tests/live_session.rs`.
- **Never log credentials:** explicit, repeated project convention (`ConnectionConfig`'s `Debug` redaction, `tests/common/mod.rs`'s doc comment, `examples/screenshot.rs`'s doc comment) — the new example/test must not deviate.

## Sources

### Primary (HIGH confidence — direct codebase inspection, this repository)
- `crates/rdpilot/src/session.rs` (1920 lines) — full public `Session` API surface, all method signatures and timing constants.
- `crates/rdpilot/src/worldstate.rs` (137 lines) — `WorldStateOptions`/`UiaMode`/`WorldState` shapes.
- `crates/rdpilot/src/lib.rs` (63 lines) — public re-export list, lint-gate scoping.
- `crates/rdpilot/src/input.rs` — `Key`/`Button`/`MouseAction`/`KeyAction` enums.
- `crates/rdpilot/src/screenshot.rs` — `Screenshot`/`Rect`/`crop` shapes.
- `crates/rdpilot/src/config.rs` — `ConnectionConfig`/`sensor_binary_path` builder.
- `crates/rdpilot/tests/live_session.rs` (1647 lines) — every reusable poll/diff/gating idiom cited above, with exact line numbers.
- `crates/rdpilot/tests/common/mod.rs` (151 lines) — `load_config`/`LIVE_ENV`/`sensor_exe_path`/`idle_secs`.
- `crates/rdpilot/examples/screenshot.rs` (137 lines) — the direct shape template for `examples/proof_harness.rs`.
- `crates/rdpilot/Cargo.toml` — dependency graph confirmation (no new crate needed).
- `.planning/phases/07-uia-tree-module/07-03-PLAN.md` — the D-7.5 risk-gate template this phase's D-9.1 spike must mirror.
- `.planning/phases/07-uia-tree-module/07-CONTEXT.md`, `.planning/phases/09-scripted-proof-harness/09-CONTEXT.md` — locked decisions D-9.1..D-9.7, D-7.4/D-7.5.
- `.planning/ROADMAP.md` §Phase 9, §Phase 7 — success criteria, requirement mapping, prior-phase live-gate measured results.
- `.planning/REQUIREMENTS.md` — PROOF-01 full text.
- `.planning/STATE.md` — accumulated decisions, build-toolchain facts, pending todos (VM size override, MinGW/rustup env), Phase 7/9 blocker note ("Target application UIA fidelity is unknown — identify and test before Phase 9 harness assertion design").
- `.planning/config.json` — `nyquist_validation: true`, `security_enforcement: true`, `security_asvs_level: 1` confirmed.
- `infra/manage-env.ps1`, `infra/scripts/Configure-Target.ps1` (7-Zip 26.01 pin, `C:\Program Files\7-Zip\7zFM.exe`), `infra/tests/Validate-Target.ps1` (ENV-01 assertion list including the 7-Zip presence check) — infra/live-target documentation (D-9.7).

### Secondary (MEDIUM confidence)
None used — this phase required no external web research; every claim is either a direct codebase citation or explicitly flagged `[ASSUMED]` in the Assumptions Log above (three items, all low-risk and spike-resolvable).

### Tertiary (LOW confidence)
None.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — zero new dependencies, fully verified against `Cargo.toml`.
- Architecture: HIGH — every pattern cited is an existing, live-verified implementation in this repository, not a third-party recommendation.
- Pitfalls: HIGH for mechanics (Rust module-sharing, existing timing constants, existing idioms); MEDIUM for the three `[ASSUMED]` 7-Zip-specific behavioral claims (title format, CLI path argument, Alt-mnemonic semantics) — all explicitly logged and spike-resolvable.

**Research date:** 2026-07-10
**Valid until:** No external time-decay risk (no third-party library versions to go stale) — valid until the D-9.1 spike produces new empirical findings that supersede the three logged assumptions, or until 7-Zip/Windows itself changes in a way this research did not anticipate (low likelihood within the project's timeframe).
