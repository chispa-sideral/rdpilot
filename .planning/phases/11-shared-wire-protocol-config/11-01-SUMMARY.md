---
phase: 11-shared-wire-protocol-config
plan: 01
subsystem: rdpilot-ipc (wire protocol crate)
tags: [wire-protocol, serde, session-identity, error-taxonomy, credential-redaction]
dependency-graph:
  requires: []
  provides: [rdpilot-ipc-crate, SessionId, Request, SessionScoped, WireResponse, SessionStatus, SessionLifecycle, WireError, WireErrorCode, TransferOutcome-wire-mirror]
  affects: [11-02 (workspace Cargo.toml member list), Phase 12 daemon, Phase 13 CLI, Phase 14 MCP]
tech-stack:
  added: [rdpilot-ipc workspace crate (serde, serde_json, thiserror only)]
  patterns: [per-variant required session field (no serde flatten), exhaustive-match compile-time forcing function, structural credential-free DTOs]
key-files:
  created:
    - crates/rdpilot-ipc/Cargo.toml
    - crates/rdpilot-ipc/src/lib.rs
    - crates/rdpilot-ipc/src/session_id.rs
    - crates/rdpilot-ipc/src/error.rs
    - crates/rdpilot-ipc/src/transfer.rs
    - crates/rdpilot-ipc/src/request.rs
    - crates/rdpilot-ipc/src/response.rs
  modified:
    - Cargo.toml (workspace members += crates/rdpilot-ipc)
decisions:
  - "WireErrorCode::Internal catch-all added per Decision 3 (not a full rdpilot::Error mapping — that's Phase 12's daemon-crate job per Decision 1)"
  - "sample_all_response_variants() tightened per checker's hardening note: samples are re-matched through a non-wildcard exhaustive match so a future WireResponse variant not acknowledged here is a compile error"
metrics:
  duration: "~35 minutes"
  completed: 2026-07-11
---

# Phase 11 Plan 01: rdpilot-ipc wire protocol crate Summary

New `rdpilot-ipc` workspace crate providing session-scoped `Request`/`WireResponse` wire DTOs with a compile-time-enforced required `session` field and structurally credential-free responses — zero dependency on `rdpilot`/IronRDP.

## What Was Built

- **`crates/rdpilot-ipc/`** — a new workspace member (`Cargo.toml` members list now `["crates/rdpilot", "crates/rdpilot-ipc"]`), depending only on `serde` (derive), `serde_json`, `thiserror`. `cargo tree -p rdpilot-ipc` confirms neither `rdpilot` nor any `ironrdp*` crate appears in its dependency graph — the thin-client invariant (D-17) is enforced.
- **`SessionId`** (`session_id.rs`): a `#[serde(transparent)]` newtype around `String`. Serializes as a bare JSON string. Deserializes via a hand-written `Deserialize` (through `FromStr`) that rejects the empty string with `serde::de::Error::custom`, closing the `"session": ""` bypass around SESSION-02's required-field guarantee.
- **`Request` + `SessionScoped`** (`request.rs`): an internally-tagged (`#[serde(tag = "op")]`) enum with six verbs — `Ping`, `Screenshot`, `LaunchProcess`, `SetForeground`, `Put`, `Get` (mirroring `Session::ping`/`screenshot`/`launch_process`/`set_foreground_window`/`upload_file`/`download_file`). Every variant embeds `session: SessionId` as a plain, non-`Option`, no-`serde(default)` field — no field-merging serde container attribute is used anywhere (avoids the serde-rs/serde#1189 deserialize limitation for internally-tagged enums). `SessionScoped::session()` is an exhaustive match over every variant — the compile-time forcing function that makes a future verb added without a `session` field a build failure, not just a missed test.
- **`WireResponse` + `SessionStatus`/`SessionLifecycle`** (`response.rs`): response variants `Ack`, `Pid { pid }`, `Screenshot { png_base64 }`, `Transfer(TransferOutcome)`, `SessionList { sessions: Vec<SessionStatus> }`, `Error(WireError)`. `SessionStatus` carries only `id`, `name`, `host`, `status`, `connected_since`, `last_activity` — no password/secret/credential field exists to leak (D-31). `SessionLifecycle` emits the exact D-30 vocabulary (`Connecting`/`Live`/`Reconnecting`/`Disconnected`, `PascalCase`).
- **`WireError` + `WireErrorCode`** (`error.rs`): `WireErrorCode` is `#[non_exhaustive]` with `#[serde(rename_all = "kebab-case")]`, emitting exactly `session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`, plus an `internal` catch-all (Decision 3) for the ~9 SDK `Error` variants outside D-28's fixed five. The `rdpilot::Error -> WireErrorCode` mapping function is deliberately NOT defined here (Decision 1) — only the types are, per the plan's binding decision; the mapping lives in the Phase 12 daemon crate.
- **`TransferOutcome`** (`transfer.rs`): an owned mirror of `rdpilot::TransferOutcome { bytes_transferred: u64, checksum: String }`, duplicated rather than imported (Decision 1).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed the literal `flatten` substring from `request.rs`'s doc comment**
- **Found during:** Task 2 acceptance-criteria check (`request.rs` must contain no `flatten` token at all, per the plan's own acceptance criteria — stricter than "don't use the attribute").
- **Issue:** The doc comment explaining why `#[serde(flatten)]` is avoided literally spelled out the token, which would fail a strict `grep -c flatten` acceptance check.
- **Fix:** Reworded the doc comment to describe the pitfall ("serde's field-merging container attribute") without using the literal word.
- **Files modified:** `crates/rdpilot-ipc/src/request.rs`
- **Commit:** `99e2bcf`

### Checker's Hardening Note Applied

**Tightened `sample_all_response_variants()` (Task 3, CONFIG-03) per the checker's non-blocking note:** the sample vector is now followed by a `for` loop that re-matches every sample through a **non-wildcard, exhaustive** `match` over `WireResponse`. This mirrors `SessionScoped`'s exhaustive-match forcing-function pattern: if a future phase adds a new `WireResponse` variant without updating this function, the match fails to compile — a missed sample can no longer silently slip through code review the way an unenforced manual list could. The `sample_all_response_variants()` function itself remains a manually-maintained builder (Rust has no reflection to auto-enumerate enum variants), but the trailing match makes *ignoring* a new variant here impossible, not just unlikely.

## Verification

- `cargo +stable-x86_64-unknown-linux-gnu test -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — 13 tests, all green (SESSION-02 rejection + accept, `SessionScoped` accessor, CONFIG-03 sentinel, `WireResponse` round-trip, `SessionLifecycle` vocabulary, `WireErrorCode` kebab strings + round-trip, `SessionId` empty-reject/accept/transparent-serialize, `TransferOutcome` round-trip).
- `cargo +stable-x86_64-unknown-linux-gnu tree -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — dependency graph contains only `serde`, `serde_json`, `thiserror` and their proc-macro/transitive deps; no `rdpilot`/`ironrdp*` node (Decision 1 confirmed).
- `cargo +stable-x86_64-unknown-linux-gnu build --workspace --target x86_64-unknown-linux-gnu` — full workspace builds; the only warning is a pre-existing unused import in `crates/rdpilot/src/input.rs`, unrelated to this plan and out of scope (Phase 11 does not touch `crates/rdpilot/src/`).
- `cargo +stable-x86_64-unknown-linux-gnu clippy -p rdpilot-ipc --target x86_64-unknown-linux-gnu -- -D warnings` — clean, zero warnings.
- Manual grep confirms zero `.unwrap(`/`.expect(` in any file in `crates/rdpilot-ipc/src/` (test or non-test).

### Toolchain substitution (established prior-phase pattern, carried forward unchanged)

This execution host is native Linux (`rustup toolchain list` shows only `stable-x86_64-unknown-linux-gnu`; the workspace's pinned `rust-toolchain.toml` targets `stable-x86_64-pc-windows-gnu`, which is not installed here). Per the same substitution prior phases (06-01, 06-02, 07-x) used, all verification above ran via `cargo +stable-x86_64-unknown-linux-gnu ... --target x86_64-unknown-linux-gnu` rather than the pinned Windows-GNU toolchain. This is a legitimate substitute for `rdpilot-ipc`: the crate is pure `serde`/`serde_json`/`thiserror` with zero `cfg(windows)` code, so the substitute-target result is representative. The genuine `x86_64-pc-windows-gnu` build remains unconfirmed on this host, consistent with every prior phase's carried-forward open item.

## Requirements Delivered

- **SESSION-02 [BLOCKING]:** satisfied — `every_request_verb_rejects_a_missing_session_field` proves all six verbs hard-reject a missing `session` field; `SessionScoped`'s exhaustive match is the compile-time structural backstop for all future verbs.
- **CONFIG-03 [BLOCKING]:** satisfied — `no_wire_response_variant_ever_carries_the_planted_secret` proves no serialized `WireResponse` variant ever emits the sentinel; the guarantee is structural (no credential-shaped field exists anywhere in the response DTOs, and this crate cannot depend on `rdpilot-config`).

## Self-Check: PASSED

- `crates/rdpilot-ipc/Cargo.toml` — FOUND
- `crates/rdpilot-ipc/src/lib.rs` — FOUND
- `crates/rdpilot-ipc/src/session_id.rs` — FOUND
- `crates/rdpilot-ipc/src/error.rs` — FOUND
- `crates/rdpilot-ipc/src/transfer.rs` — FOUND
- `crates/rdpilot-ipc/src/request.rs` — FOUND
- `crates/rdpilot-ipc/src/response.rs` — FOUND
- Commit `8f54cd1` (Task 1) — FOUND in `git log --oneline --all`
- Commit `99e2bcf` (Task 2) — FOUND in `git log --oneline --all`
- Commit `c68eafa` (Task 3) — FOUND in `git log --oneline --all`
