---
phase: 11-shared-wire-protocol-config
verified: 2026-07-11T00:00:00Z
status: human_needed
score: 4/4 must-haves verified
overrides_applied: 0
human_verification:
  - test: "Genuine build/test run under the pinned x86_64-pc-windows-gnu toolchain"
    expected: "Workspace builds and all rdpilot-ipc/rdpilot-config tests pass identically under the pinned Windows-GNU target"
    why_human: "This verification host only has stable-x86_64-unknown-linux-gnu installed (rust-toolchain.toml pins stable-x86_64-pc-windows-gnu, absent here). Substituted --target x86_64-unknown-linux-gnu for verification, consistent with every prior phase (06-01, 06-02, 07-x, 10-x). Both crates are pure serde/config/directories/thiserror with zero cfg(windows) code, so the substitute is representative, but the pinned-target build itself remains unconfirmed on this host. Non-blocking: does not gate phase completion per the established prior-phase pattern — carried forward unchanged since Phase 06 and not specific to Phase 11's own work."
---

# Phase 11: Shared Wire Protocol & Config Verification Report

**Phase Goal:** One shared crate defines the daemon<->client wire protocol (`rdpilot-ipc`) and another resolves layered configuration (`rdpilot-config`) — with session-identity-as-required-field and credential-redaction enforced at the schema level BEFORE any consumer binary is built on top.

**Verified:** 2026-07-11
**Status:** human_needed (all 4 goal-backward truths independently VERIFIED against code; one non-blocking pinned-Windows-toolchain build confirmation is the sole outstanding human-verification item — a pre-existing environment gap carried forward unchanged since Phase 06, not a Phase 11 defect)
**Overall Phase Verdict:** VERIFIED-WITH-CONCERNS — goal delivered; the only concern is the pinned-toolchain build confirmation, which does not block the phase (see Human Verification Required below)
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (ROADMAP Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | **[BLOCKING] SESSION-02** — every session-scoped `Request` verb carries a required, non-optional `session: SessionId`; omission is a hard deserialize rejection; no `#[serde(flatten)]` anywhere | VERIFIED | `crates/rdpilot-ipc/src/request.rs:34-86` — all 6 `Request` variants (`Ping`, `Screenshot`, `LaunchProcess`, `SetForeground`, `Put`, `Get`) declare `session: SessionId` as a plain field, no `Option`, no `#[serde(default)]`. Test `every_request_verb_rejects_a_missing_session_field` (line 114) constructs one JSON payload per verb with `session` omitted and asserts `serde_json::from_str` errs for every one — ran and passed. Counter-test `every_request_verb_accepts_a_present_session_field` proves the positive case isn't vacuously true. `SessionScoped::session()` (line 96) is an exhaustive match with no wildcard arm — a future verb added without a `session` field is a compile error. `grep -rn "serde(flatten)"` across `crates/rdpilot-ipc/src` and `crates/rdpilot-config/src` returned zero hits. `SessionId::deserialize` (session_id.rs:42) additionally rejects the empty-string bypass (`""` satisfies "field present" but is rejected) — test `rejects_empty_string` passed. |
| 2 | **CONFIG-01** — layered file -> env -> flag/override precedence, highest layer deterministically wins | VERIFIED | `crates/rdpilot-config/src/resolve.rs`. Test `layered_precedence` (line 121) uses injected sources (no real home dir/ambient env): proves env beats file (`env-host` wins over `file-host`), a field set only by file survives (`port: Some(1)`), override beats env+file (`flag-host` wins), and file-only-with-no-env-source preserves the file value. `apply_overrides_none_does_not_clobber` proves a `None` override never destroys a lower layer's value. Both tests ran and passed (`resolve::tests::layered_precedence ... ok`, `resolve::tests::apply_overrides_none_does_not_clobber ... ok`). |
| 3 | **CONFIG-02** — config file at the platform config dir (`BaseDirs`, not `ProjectDirs` — no doubled `\config` on Windows), self-explanatory commented template, discoverable | VERIFIED | `crates/rdpilot-config/src/paths.rs:28` — `config_file_path()` uses `directories::BaseDirs::new().map(\|b\| b.config_dir().join("rdpilot").join("config.toml"))`, explicitly NOT `ProjectDirs` (doc comment explains why: `ProjectDirs` would insert an unwanted extra `config` subfolder on Windows). Test `config_file_path_matches_platform_convention` asserts the path ends in exactly one `rdpilot`/`config.toml` pair and that zero `config` path segments exist anywhere (guards the doubled-segment pitfall) — ran and passed. Template: `crates/rdpilot-config/assets/config.toml.template` documents all 6 D-27 keys (`host`, `port`, `username`, `password`, `domain`, `accept_invalid_certs`) with a comment per key, precedence explanation at the top, and location documentation; test `template_documents_every_d27_key` asserts every key name appears in the template text; `template_parses_into_resolved_config` proves the unedited (all-commented) template parses to an all-`None`/`false` `ResolvedConfig` — both ran and passed. `.gitignore` lines 37-38 add `config.toml` / `.rdpilot.toml` defensive entries (the real file lives outside the repo tree at the platform config dir). |
| 4 | **[BLOCKING] CONFIG-03** — serializing every wire response type and grepping for a planted secret sentinel finds nothing; credential-free DTOs by construction | VERIFIED | `crates/rdpilot-ipc/src/response.rs:145` — test `no_wire_response_variant_ever_carries_the_planted_secret` builds one sample per `WireResponse` variant (`Ack`, `Pid`, `Screenshot`, `Transfer`, `SessionList`, `Error`) via `sample_all_response_variants()`, serializes each, and asserts none contains `RDPILOT-PLANTED-SECRET-SENTINEL` — ran and passed. Exhaustiveness is compiler-enforced: `sample_all_response_variants()` re-matches every sample through a non-wildcard exhaustive `match` over `WireResponse` (line 123-131) — a future variant added without updating this function is a compile error, not a silently-skipped sample. Credential-freedom is structural, not scrubbed: no `WireResponse` variant or `SessionStatus` field is password/credential-shaped (`SessionStatus` carries only `id`, `name`, `host`, `status`, `connected_since`, `last_activity` — `host` is target-addressing, not a secret, per the doc comment). This crate has zero dependency on `rdpilot-config` (confirmed via `cargo tree`, see below), so a `Credentials`/password-shaped type cannot structurally enter the wire graph at all — the guarantee holds even without the test. |

**Score:** 4/4 truths verified

### Binding Invariants (explicitly requested, beyond the four numbered SCs)

| Invariant | Status | Evidence |
|---|---|---|
| `rdpilot-ipc` dependency-free of `rdpilot`/IronRDP/rustls/tokio | VERIFIED | `cargo tree -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — full tree is `serde`, `serde_json`, `thiserror` and their proc-macro/transitive deps only (`serde_core`, `serde_derive`, `proc-macro2`, `quote`, `syn`, `unicode-ident`, `itoa`, `memchr`, `zmij`, `thiserror-impl`). No `rdpilot`, `ironrdp*`, `rustls`, or `tokio*` node anywhere in the graph. `Cargo.toml` dependencies list confirms the same at the manifest level (`serde`, `serde_json`, `thiserror` only). |
| `rdpilot-config` dependency-free of `rdpilot`/IronRDP/rustls/tokio | VERIFIED | `cargo tree -p rdpilot-config --target x86_64-unknown-linux-gnu` — full tree is `config`, `directories`, `serde`, `thiserror` and their transitive deps only (`pathdiff`, `toml`, `serde_spanned`, `toml_datetime`, `toml_parser`, `winnow`, `dirs-sys`, `libc`, `option-ext`, proc-macro chain). No `rdpilot`, `ironrdp*`, `rustls`, or `tokio*` node anywhere in the graph. |
| `From<rdpilot::Error>` NOT present in `rdpilot-ipc`; `WireErrorCode` has the 5 fixed codes + `Internal` catch-all | VERIFIED | `grep -rn "impl From"` for any `rdpilot::Error` conversion in `crates/rdpilot-ipc` returned zero implementation hits — the only match is a doc comment in `error.rs:5` explicitly explaining the mapping is deliberately deferred to Phase 12. `WireErrorCode` (`error.rs:39-61`) enumerates exactly `SessionNotFound`, `DaemonUnreachable`, `TransferFailed`, `PathTraversal`, `ChecksumMismatch`, `Internal` — 5 fixed + 1 catch-all, `#[non_exhaustive]`. Test `wire_error_code_emits_exact_kebab_case_strings` confirms the exact wire strings (`session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`, `internal`) — ran and passed. |

### Required Artifacts

| Artifact | Expected | Status | Details |
|---|---|---|---|
| `crates/rdpilot-ipc/Cargo.toml` | serde/serde_json/thiserror only, no rdpilot dep | VERIFIED | Confirmed manifest + `cargo tree` |
| `crates/rdpilot-ipc/src/lib.rs` | crate root, deny-lints, module wiring | VERIFIED | `#![deny(unsafe_code)]`, `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]` present; exports all public types |
| `crates/rdpilot-ipc/src/session_id.rs` | `SessionId` newtype, empty-reject `Deserialize` | VERIFIED | Hand-written `Deserialize` via `FromStr`, rejects empty string |
| `crates/rdpilot-ipc/src/request.rs` | `Request` enum + `SessionScoped` exhaustive-match trait | VERIFIED | 6 verbs, exhaustive match, 3 inline tests all passing |
| `crates/rdpilot-ipc/src/response.rs` | `WireResponse` + `SessionStatus`/`SessionLifecycle`, credential-free | VERIFIED | 6 variants, sentinel test, exhaustive-match sample builder |
| `crates/rdpilot-ipc/src/error.rs` | `WireError`/`WireErrorCode` taxonomy, types only | VERIFIED | 6 fixed codes, no `rdpilot::Error` conversion present |
| `crates/rdpilot-ipc/src/transfer.rs` | `TransferOutcome` owned mirror | VERIFIED | `{ bytes_transferred, checksum }`, round-trip test passing |
| `crates/rdpilot-config/Cargo.toml` | config 0.15.25 (toml-only)/directories 6.0.0/serde/thiserror, no rdpilot dep | VERIFIED | Confirmed manifest + `cargo tree` |
| `crates/rdpilot-config/src/resolved.rs` | `ResolvedConfig` (Deserialize-only, NOT Serialize) + owned `ConfigError` | VERIFIED | `#[derive(Debug, Clone, Deserialize)]` — no `Serialize` derive or import anywhere in the file |
| `crates/rdpilot-config/src/paths.rs` | `config_file_path()` via `BaseDirs` | VERIFIED | Uses `BaseDirs`, not `ProjectDirs`; doubled-segment guard test passing |
| `crates/rdpilot-config/src/resolve.rs` | file+env resolution + `apply_overrides` typed-override pass | VERIFIED | 3-layer precedence, deterministic, tests passing |
| `crates/rdpilot-config/assets/config.toml.template` | commented, self-explanatory template | VERIFIED | All 6 D-27 keys documented, all commented out, parses to all-defaults |

### Key Link Verification

| From | To | Via | Status | Details |
|---|---|---|---|---|
| `request.rs` | `SessionScoped` | exhaustive match over every `Request` variant | WIRED | `impl SessionScoped for Request` with no wildcard arm |
| `response.rs` | `SessionStatus` | `WireResponse::SessionList` carries `Vec<SessionStatus>` | WIRED | Confirmed at `response.rs:72-75` |
| `paths.rs` | `directories::BaseDirs` | `config_dir().join("rdpilot").join("config.toml")` | WIRED | Confirmed at `paths.rs:28` |
| `resolve.rs` | `config::Environment` | `with_prefix("RDPILOT")` separator `"__"` | WIRED | Confirmed at `resolve.rs:44-48`; env source added last so it wins over file (CONFIG-01) |
| `lib.rs` (rdpilot-config) | `resolved.rs`/`paths.rs`/`resolve.rs` | `pub use` re-exports | WIRED | `ResolvedConfig`, `ConfigError`, `config_file_path`, `resolve`, `apply_overrides`, `CONFIG_TEMPLATE` all publicly exported |

### Behavioral Spot-Checks / Test Suite Run

| Behavior | Command | Result | Status |
|---|---|---|---|
| Full workspace test suite (native-Linux substitute target, per established prior-phase pattern) | `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --workspace --target x86_64-unknown-linux-gnu` | `rdpilot`: 129 passed, 0 failed, 28 live-gated ignored; `rdpilot-config`: 6 passed, 0 failed; `rdpilot-ipc`: 13 passed, 0 failed | PASS |
| `cargo tree -p rdpilot-ipc` — dependency-free invariant | `cargo tree -p rdpilot-ipc --target x86_64-unknown-linux-gnu` | No `rdpilot`/`ironrdp*`/`rustls`/`tokio*` node | PASS |
| `cargo tree -p rdpilot-config` — dependency-free invariant | `cargo tree -p rdpilot-config --target x86_64-unknown-linux-gnu` | No `rdpilot`/`ironrdp*`/`rustls`/`tokio*` node | PASS |
| Clippy clean (both new crates, all targets incl. tests) | `cargo clippy -p rdpilot-ipc -p rdpilot-config --target x86_64-unknown-linux-gnu --all-targets` | Zero warnings | PASS |
| No `#[serde(flatten)]` anywhere in either crate | `grep -rn "serde(flatten)" crates/rdpilot-ipc crates/rdpilot-config` | Zero hits | PASS |
| No `From<rdpilot::Error>` impl in rdpilot-ipc | `grep -rn "impl From.*rdpilot::Error"` | Zero implementation hits (only a doc comment explaining the deferral) | PASS |
| Commits referenced in SUMMARY.md actually exist | `git log --oneline --all \| grep -E "8f54cd1\|99e2bcf\|c68eafa\|568410e\|69a596f\|b8b24ba"` | All 6 commits found | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|---|---|---|---|---|
| SESSION-02 | 11-01 | Every session-scoped command explicitly targets a session, no implicit default | SATISFIED | `Request` enum required-field design + exhaustive rejection test |
| CONFIG-01 | 11-02 | Layered config: file/env/CLI-flag/MCP-init, deterministic precedence | SATISFIED | `layered_precedence` test with injected sources |
| CONFIG-02 | 11-02 | Config file follows CLI-tool convention, discoverable, self-explanatory | SATISFIED | `BaseDirs` path + fully-commented template + gitignore entries |
| CONFIG-03 | 11-01 | Credentials never leak through IPC/MCP wire responses or logs | SATISFIED | Structural credential-freedom (no password-shaped field in any `WireResponse` DTO) + planted-sentinel regression test |

REQUIREMENTS.md's phase-coverage table (lines 89-92) independently lists all four as "Phase 11 / Complete" — consistent with the above. No orphaned requirements found mapped to Phase 11 beyond these four.

### Anti-Patterns Found

None. `grep -n -E "TBD|FIXME|XXX|TODO|HACK|PLACEHOLDER|not yet implemented|not available|coming soon"` across all `rdpilot-ipc`/`rdpilot-config` source and the template returned zero hits. `grep -n "unwrap()\|expect("` across both crates' `src/` returned zero hits outside doc comments (matches the `#![deny(clippy::unwrap_used)]`/`#![deny(clippy::expect_used)]` crate-level lints, confirmed clean by the clippy run above).

### Human Verification Required

### 1. Pinned-toolchain (x86_64-pc-windows-gnu) build confirmation

**Test:** Run `cargo build --workspace` and `cargo test -p rdpilot-ipc -p rdpilot-config` under the project's actual pinned toolchain (`rust-toolchain.toml`: `stable-x86_64-pc-windows-gnu`) on a host that has it installed.
**Expected:** Identical build success and test pass counts as the Linux-substitute-target run documented above.
**Why human:** This verification host (and the executor's host, per both SUMMARY.md files) only has `stable-x86_64-unknown-linux-gnu` installed; the pinned Windows-GNU toolchain requires MinGW-w64 gcc and is not present here. Both new crates are pure `serde`/`config`/`directories`/`thiserror` with zero `cfg(windows)` code, so the substitute-target result is representative, but the genuine pinned-target build has never been confirmed for this phase, consistent with every prior phase (06-01, 06-02, 07-x, 10-x) carrying the same open item forward. **Non-blocking** — this is a pre-existing environment gap unrelated to Phase 11's own work, not a defect introduced by it, and does not affect the goal-achievement verdict.

### Gaps Summary

No gaps found. All four ROADMAP success criteria (SESSION-02, CONFIG-01, CONFIG-02, CONFIG-03) are independently verified against the actual delivered code, not just SUMMARY.md claims: the specific tests that would fail if the guarantees regressed were located, read, and confirmed to run and pass (13/13 in `rdpilot-ipc`, 6/6 in `rdpilot-config`). The dependency-free thin-client invariant was independently confirmed via `cargo tree` for both crates (no `rdpilot`/IronRDP/rustls/tokio transitively). `#[serde(flatten)]` is absent (serde#1189 avoided). The `rdpilot::Error -> WireErrorCode` mapping is confirmed absent from `rdpilot-ipc` (correctly deferred to Phase 12) while the fixed 5+1 `WireErrorCode` taxonomy is confirmed present. The only open item is a non-blocking, pre-existing pinned-Windows-toolchain build confirmation gap that has been carried forward unchanged since Phase 06 and is not specific to this phase's work.

---

_Verified: 2026-07-11_
_Verifier: Claude (gsd-verifier)_
