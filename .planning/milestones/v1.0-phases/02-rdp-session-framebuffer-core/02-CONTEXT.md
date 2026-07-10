# Phase 2: RDP Session + Framebuffer Core - Context

**Gathered:** 2026-06-05
**Status:** Ready for planning

<domain>
## Phase Boundary

A working IronRDP session connects + authenticates (NLA/CredSSP) to the live
Phase-1 Windows VM, stays rendered while the local client is windowless/hidden,
and produces correct-color (RGB, not YUV-grey) PNG screenshots of the full
remote desktop — plus a crop-to-rect primitive. This phase introduces the
**first Rust code in the repo** and establishes the SDK foundation every later
phase builds on (session handle, framebuffer pipeline, config surface).

**Requirements:** SESS-01 (connect/authenticate), SESS-02 (session lifecycle +
keep rendered), CAP-01 (full-desktop screenshot).

**Explicitly NOT in this phase** (belongs elsewhere — redirect any scope creep):
- Window enumeration / real window geometry → Phase 6 (Phase 2 only crops to a
  caller-supplied `Rect`).
- Input injection (mouse/keyboard) → Phase 3 (Phase 2 may emit synthetic null
  input solely for keepalive).
- DVC sensor / structured perception / the C# sensor → Phase 4+ (Phase 2 only
  leaves the `RDPILOT_SENSOR` registration seam intact).

</domain>

<decisions>
## Implementation Decisions

### Crate & Workspace Layout
- **D-01:** Cargo **workspace** at repo root with a **single member
  `crates/rdpilot`** today. Future phases add sibling crates (e.g.
  `rdpilot-transport`, `rdpilot-input`) without restructuring — chosen to
  minimize churn as the SDK grows toward sensor/DVC/input/PyO3 consumers.
- **D-02:** `Cargo.lock` **is committed** (resolves the lockfile policy deferred
  from Phase 1; this is a workspace that will produce binaries/examples).
- **D-03:** Rust **1.78+** (per CLAUDE.md / research stack). Crate name:
  `rdpilot`.

### Session API & Lifecycle
- **D-04:** **SDK owns the session loop.** `Session::connect(&cfg).await`
  returns a `Session` handle; internally the SDK spawns a Tokio background task
  that pumps the IronRDP connection and maintains the latest `DecodedImage`.
  Consumers never drive the RDP state machine. Public shape:
  ```rust
  let session = Session::connect(&cfg).await?;
  let png = session.screenshot().await?;   // latest framebuffer, decoded
  session.close().await?;
  ```
- **D-05:** **Headless library** — rdpilot creates no local RDP GUI window, so
  "stays rendered while local window minimized/hidden" is inherently satisfied
  on the client side. The remote-side rendering concern is handled by
  `RemoteDesktop_SuppressWhenMinimized=2` (already applied by Phase 1).
- **D-06:** **Keepalive is automatic** — a background keepalive (synthetic null
  input ~every 60s, per PITFALLS M1) runs while a `Session` is alive, no opt-in.
  Satisfies the 10-minute idle criterion with zero caller effort.
- **D-07:** **Teardown** = explicit async `close().await` for a clean disconnect
  **plus a `Drop` guard** that aborts the background task / tears down the
  socket if the handle is dropped without `close()`.
- **D-08:** **Verify the minimized-render property behaviorally**, not by
  reading the remote registry. Hold a windowless session through the idle
  period, then assert the framebuffer is still live, full-resolution, and
  non-blank (real desktop pixels). The registry value is Phase 1's contract;
  Phase 2 proves the *effect*. **No WinRM dependency in Phase 2's session path.**

### Screenshot Output Contract
- **D-09:** `screenshot()` returns an **owned SDK type**, not a third-party type:
  ```rust
  pub struct Screenshot { width: u32, height: u32, rgba: Vec<u8> }
  impl Screenshot {
      pub fn to_png(&self) -> Result<Vec<u8>>;
      pub fn crop(&self, r: Rect) -> Screenshot;
  }
  ```
  The `image` crate is an **internal impl detail**, never in the public API
  (clean semver; trivial to marshal over PyO3/MCP later).
- **D-10:** **Crop = caller supplies the `Rect`** (CAP-01 success-criterion #3).
  Real window geometry / enumeration is out of scope → Phase 6.
- **D-11:** **PNG encoding lives on the SDK** (`.to_png()`); the caller writes
  bytes to disk. Raw RGBA buffer is retained so cropping never re-decodes a PNG.

### Config, Secrets & Connection
- **D-12:** SDK public API takes a **typed `ConnectionConfig`** (host, port,
  credentials, requested width/height, ...). The **library is env-agnostic** —
  it does not hard-code any repo-specific secrets path.
- **D-13:** Parameters reach the SDK via **common practices — environment
  variables or a config file**. Convenience loaders for both may live in the SDK,
  but the *source/path* is the caller's choice. **For our testing, the config
  file (`.secrets/connection.json`, written by Phase 1's `manage-env.ps1`) is
  the path.**
- **D-14 (boundary / non-goal):** rdpilot **does not get in the middle of OS
  credential machinery** — Windows Credential Manager or Windows Hello.
  Credentials are passed as parameters and handed to NLA/CredSSP by our own
  IronRDP client; we never intercept, store, or trigger OS credential UI. (This
  is naturally true since we are a pure-Rust IronRDP client, not mstsc — but it
  is an explicit constraint.)
- **D-15:** **Server-cert trust:** default to normal cert validation, but expose
  an explicit, risk-named `ConnectionConfig` flag (`accept_invalid_certs(true)`
  or equivalent) for the self-signed workgroup lab VM. **Thumbprint pinning is
  deferred** — the VM is recreated by auto-destroy each run, so a pinned
  thumbprint would churn.

### Validation
- **D-16:** Validation is a **full automated integration test suite** covering
  all 5 success criteria: (1) connect/authenticate, (2) RGB-correct full-desktop
  framebuffer, (3) crop-to-rect, (4) stays-rendered-while-idle/windowless,
  (5) 10-minute keepalive prevents idle disconnect.
- **D-17:** **All local at this stage — no CI yet.** The developer provisions the
  VM (`manage-env.ps1 up`), populates `.secrets/connection.json`, and runs the
  suite locally against the live target. The suite must be **CI-portable**: when
  CI is set up later, the same tests run there against a CI-provisioned live VM.
- **D-18:** A default `cargo test` with **no live target available must not
  hard-fail the build** — live-VM tests are gated/skipped when no target is
  present. The canonical validation run executes the full suite (full 10-min
  idle included).
- **D-19 (env prerequisite):** The **Rust toolchain is not yet installed** on
  this machine and must be installed via **`scoop`** before any build/test work.

### Claude's Discretion
The user explicitly delegated these to planning/research — no user decision
required:
- **Color decode (criterion #2 / known pitfall):** ensure the IronRDP codec
  path decodes to correct RGB, not YUV-grey. Implementation detail for the
  researcher/planner.
- **Error-type design:** `thiserror`-style enum, `Result` surface; **no
  `unwrap`/`expect` in library code** (API-01). Style chosen during planning.
- **Logging/observability:** `tracing` for diagnostics (default off/quiet).
- **Default requested resolution / color depth** for the session.
- **Idle-test duration parameterization** (short in dev iteration, full 10-min
  for the canonical validation run) — an impl detail, as long as the canonical
  run does the full duration.
- **DVC seam:** leave the `RDPILOT_SENSOR` `DvcProcessor` registration hook
  point so Phase 4 can register *before* `connector.connect()` completes (hard
  IronRDP constraint) — but do **not** build the sensor in Phase 2.
- Exact idiomatic async API ergonomics (builder vs plain ctor for
  `ConnectionConfig`, convenience loader signatures).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope & requirements
- `.planning/ROADMAP.md` — Phase 2 goal, 5 success criteria, requirement mapping (SESS-01, SESS-02, CAP-01).
- `.planning/REQUIREMENTS.md` — full text of SESS-01, SESS-02, CAP-01 (note: CAP-02 per-window crop is mapped to Phase 6; Phase 2 only provides crop-to-rect).
- `.planning/PROJECT.md` — locked stack decisions and project boundaries.
- `.planning/STATE.md` — accumulated decisions and current position.

### Research (locked tech & pitfalls)
- `.planning/research/SUMMARY.md` — IronRDP/Rust/DVC/UIA conclusions, architecture overview.
- `.planning/research/STACK.md` — IronRDP 0.14.x crate table, Tokio, rustls, `image` deps.
- `.planning/research/ARCHITECTURE.md` — six-component diagram, coordinate contract.
- `.planning/research/PITFALLS.md` — C1–C6 critical + M1–M4 moderate pitfalls (esp. M1 keepalive, YUV color, C2 session-account, C3 NLA cert trust, UIPI/C4).
- `.planning/research/FEATURES.md` — must-have feature list (TS-1..TS-13).

### Phase 1 (test target contract this phase consumes)
- `.planning/phases/01-test-environment/01-CONTEXT.md` — VM, auto-destroy, network, credentials decisions.
- `.planning/phases/01-test-environment/01-PATTERNS.md` — conventions established by Phase 1.
- `infra/` — IaC root; `manage-env.ps1 up` provisions the VM and writes connection details.
- `.secrets/connection.json` *(gitignored)* — live target credentials/host; the testing config source. **Do not read its contents into context** (secrets rule).

### IronRDP upstream (external)
- IronRDP `screenshot.rs` example — `DecodedImage` → RGBA framebuffer pattern (https://github.com/Devolutions/IronRDP).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- **None in Rust** — greenfield. No `Cargo.toml`, no `crates/`, no `src/`, no
  IronRDP usage exists yet. Phase 2 creates the first Rust artifacts.
- **Phase 1 infra (`infra/`)** — `manage-env.ps1` (`up`/`down`) provisions and
  tears down the live VM and emits `.secrets/connection.json`; the test target
  and credential source for this phase.

### Established Patterns
- **Phase 1 used live-infra tests (Pester) against the real Azure VM** — precedent
  for the "validate against the live target" approach (now expressed as Rust
  integration tests, local-only at this stage).
- **`.gitignore`** currently excludes `/target/` and `*.rs.bk` (Rust-ready);
  `.secrets/` is gitignored.

### Integration Points
- Session connect path must leave the **DVC `RDPILOT_SENSOR` registration seam**
  for Phase 4 (register before `connector.connect()` completes).
- Coordinate space: virtual-desktop pixels, (0,0) top-left, 96 DPI enforced on
  the VM by Phase 1 — relevant to crop `Rect` semantics and future input (Phase 3).

</code_context>

<specifics>
## Specific Ideas

- Public API shape the user signed off on (verbatim intent):
  ```rust
  let session = Session::connect(&cfg).await?;
  let png = session.screenshot().await?;
  session.close().await?;
  ```
  and:
  ```rust
  pub struct Screenshot { width: u32, height: u32, rgba: Vec<u8> }
  // .to_png() -> Vec<u8>, .crop(rect) -> Screenshot
  ```
- 7-Zip File Manager (installed by Phase 1) is the canonical "remote-only
  program" for eyeballing screenshots if a visual sanity check is wanted.

</specifics>

<deferred>
## Deferred Ideas

- **Thumbprint cert pinning** — deferred; revisit once the lab target is stable
  or for non-disposable targets (currently churns with auto-destroy).
- **CI integration for live-VM tests** — explicitly later; tests are written
  CI-portable now, wired into CI when CI is set up.
- **Per-window screenshots driven by real window geometry / enumeration** →
  Phase 6 (CAP-02). Phase 2 only crops to a caller-supplied rect.
- **Input injection** → Phase 3. **DVC sensor / structured perception** → Phase 4+.

</deferred>

---

*Phase: 2-RDP Session + Framebuffer Core*
*Context gathered: 2026-06-05*
