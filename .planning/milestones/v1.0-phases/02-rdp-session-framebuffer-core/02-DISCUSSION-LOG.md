# Phase 2: RDP Session + Framebuffer Core - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-06-05
**Phase:** 2-RDP Session + Framebuffer Core
**Areas discussed:** Crate & workspace layout, Session API & lifecycle, Screenshot output contract, Config/secrets/validation

---

## Crate & Workspace Layout

| Option | Description | Selected |
|--------|-------------|----------|
| Workspace, one crate now | Workspace at root, single member `crates/rdpilot`; siblings added later without restructuring. Low churn. | ✓ |
| Single flat crate | `Cargo.toml` + `src/` at root, no workspace. Simplest, but splitting later means moving files. | |
| Workspace, split now | Multiple crates from day 1 (core + facade). Risks premature boundaries. | |

**User's choice:** Workspace, one crate now
**Notes:** Cargo.lock committed (resolves Phase-1 deferred lockfile policy). Rust 1.78+.

---

## Session API & Lifecycle

### Q1 — Loop ownership / API shape

| Option | Description | Selected |
|--------|-------------|----------|
| SDK owns background task | `Session::connect(cfg).await` → handle; SDK spawns Tokio task pumping the connection; `session.screenshot().await`. Hides IronRDP internals, keeps framebuffer live. | ✓ |
| Caller pumps the loop | SDK returns object with `poll`/`tick` the caller drives. More control, leaks state machine, risks stale framebuffer. | |
| You decide | Defer to planning. | |

**User's choice:** SDK owns background task

### Q2 — Keepalive & teardown

| Option | Description | Selected |
|--------|-------------|----------|
| Auto keepalive + close()+Drop | Keepalive automatic; `close().await` clean path + `Drop` guard aborts task/socket. Zero-config correctness for idle criterion. | ✓ |
| Opt-in keepalive | Off by default, caller enables via config. 10-min criterion depends on harness remembering. | |
| You decide | Defer to planning. | |

**User's choice:** Auto keepalive + close()+Drop

### Q3 — Verifying RemoteDesktop_SuppressWhenMinimized=2 (criterion #4)

| Option | Description | Selected |
|--------|-------------|----------|
| Behavioral verification | Windowless session stays live/full-res/non-blank through idle. No extra transport. | ✓ |
| WinRM registry read | Query the remote registry value over WinRM and assert ==2. Pulls WinRM into Phase 2. | |
| Both | WinRM assert + behavioral check. Most thorough, more to build. | |

**User's choice:** Behavioral verification
**Notes:** The registry value is Phase 1's contract; Phase 2 proves the effect. No WinRM in Phase 2 path.

---

## Screenshot Output Contract

### Q1 — Return type

| Option | Description | Selected |
|--------|-------------|----------|
| Owned Screenshot struct | `Screenshot { width, height, rgba }` + `.to_png()` + `.crop(rect)`; `image` crate internal only. Clean semver, FFI-friendly. | ✓ |
| Return image::RgbaImage | Most ergonomic in pure Rust but leaks third-party type into public API; awkward for PyO3/MCP. | |
| Return PNG bytes only | Simplest, but discards raw buffer; cropping requires re-decoding. | |

**User's choice:** Owned Screenshot struct
**Notes:** Settles criterion #3 — crop = caller-supplied `Rect` (window geometry → Phase 6). PNG encoding on the SDK via `.to_png()`; caller writes to disk.

---

## Config, Secrets & Validation

### Q1 — Config source

| Option | Description | Selected |
|--------|-------------|----------|
| Typed config; caller loads | SDK takes typed `ConnectionConfig`; library env-agnostic; harness reads the JSON. | ✓ (via "You decide" + user clarification) |
| SDK reads the file | SDK defaults to reading `.secrets/connection.json` itself. Bakes repo path into library. | |
| You decide | Defer to planning. | (selected, then clarified) |

**User's choice:** "You decide" → user clarified: parameters reach us via common practices (env vars **or** config file); for testing, the config file is the right approach (as proposed). **Plus an explicit boundary:** credentials might be stored in Windows / Windows Hello might appear — rdpilot must **not** get in the middle of that. Result: typed `ConnectionConfig`, env-var/config-file loaders, config file for testing, no OS Credential Manager / Hello involvement.

### Q2 — Server-cert trust

| Option | Description | Selected |
|--------|-------------|----------|
| Configurable, insecure opt-in | Default validate; explicit `accept_invalid_certs(true)` flag for self-signed lab VM. | ✓ |
| Pin thumbprint | Assert server cert thumbprint. Churns with auto-destroy VM recreation. | |
| You decide | Defer to planning. | |

**User's choice:** Configurable, insecure opt-in
**Notes:** Pinning deferred — auto-destroy recreates the VM each run.

### Q3 — Validation approach

| Option | Description | Selected |
|--------|-------------|----------|
| Example bin + ignored integ test | Example binary + `#[ignore]`-by-default integration tests, parameterized idle. | |
| Example binary + manual checklist | Binary + written human checklist. No repeatable assertions. | |
| Full integration suite | All 5 criteria automated incl. full 10-min idle. Most rigorous. | ✓ |

**User's choice:** Full integration suite
**Notes:** User clarified — **no CI for now, all local at this stage**; when CI is set up later, live-VM tests will run there too. Suite written CI-portable; default `cargo test` must not hard-fail without a live target.

---

## Claude's Discretion

- YUV→RGB color decode (criterion #2 pitfall).
- Error-type design (`thiserror`, no `unwrap`/`expect` per API-01).
- `tracing` logging (quiet by default).
- Default requested resolution / color depth.
- Idle-test duration parameterization (full duration on the canonical run).
- DVC `RDPILOT_SENSOR` registration seam left for Phase 4 (not built here).
- Idiomatic `ConnectionConfig` builder/loader ergonomics.

## Deferred Ideas

- Thumbprint cert pinning (non-disposable targets / stable lab).
- CI integration for live-VM tests (tests written CI-portable now).
- Per-window screenshots via real window geometry/enumeration → Phase 6 (CAP-02).
- Input injection → Phase 3; DVC sensor / structured perception → Phase 4+.
- **Env prerequisite:** install Rust toolchain via `scoop` before build/test.
