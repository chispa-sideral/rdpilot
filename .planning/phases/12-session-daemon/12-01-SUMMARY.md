---
phase: 12-session-daemon
plan: 01
subsystem: rdpilot-ipc
tags: [wire-protocol, session-lifecycle, serde]
dependency-graph:
  requires: []
  provides:
    - rdpilot_ipc::Request::Connect
    - rdpilot_ipc::Request::List
    - rdpilot_ipc::Request::Disconnect
    - rdpilot_ipc::SessionScoped::session() -> Option<&SessionId>
    - rdpilot_ipc::WireResponse::Connected
    - rdpilot_ipc::SessionLifecycle::Orphaned
    - rdpilot_ipc::WireErrorCode::DuplicateSession
  affects:
    - crates/rdpilot-daemon (Plan 12-02+, consumes these verbs/types)
tech-stack:
  added: []
  patterns:
    - "Exhaustive match, no wildcard arm, as compile-time forcing function (SessionScoped, sample_all_response_variants)"
key-files:
  created: []
  modified:
    - crates/rdpilot-ipc/src/request.rs
    - crates/rdpilot-ipc/src/response.rs
    - crates/rdpilot-ipc/src/error.rs
decisions: []
metrics:
  duration: "~25m"
  completed: "2026-07-11"
---

# Phase 12 Plan 01: Session-Lifecycle Wire Verbs Summary

Extended `rdpilot-ipc`'s `Request`/`WireResponse`/`WireErrorCode` with the three session-lifecycle wire verbs (`Connect`, `List`, `Disconnect`) and their supporting types (`WireResponse::Connected`, `SessionLifecycle::Orphaned`, `WireErrorCode::DuplicateSession`) that Phase 11 deliberately left out of scope — the Wave-1 prerequisite for SESSION-01/03/04.

## What was built

**Task 1 — `request.rs`:** Added `Request::Connect { name: Option<String>, host, port, username, password, domain, accept_invalid_certs }` (session-less; `name: None` requests an auto-generated id per D-29), `Request::List {}` (session-less), and `Request::Disconnect { session: SessionId }` (session-scoped, required field, SESSION-02 preserved). Changed `SessionScoped::session()`'s signature from `fn session(&self) -> &SessionId` to `fn session(&self) -> Option<&SessionId>`. The `impl SessionScoped for Request` match remains exhaustive with no wildcard arm: `Connect`/`List` return `None`; all seven session-targeting verbs (the original six operational verbs + `Disconnect`) return `Some`. Added a doc comment on `Connect.password` marking it the one legitimate plaintext-credential wire carrier (D-31), with redaction obligation pushed to daemon-side log sites.

**Task 2 — `response.rs`:** Added `WireResponse::Connected { session: SessionId }` (the reply to a successful `Connect`, carrying the possibly-auto-generated id) and `SessionLifecycle::Orphaned` (DAEMON-04's crash-restart orphan-surfacing status, placed after `Disconnected`, emitting `"Orphaned"` under the existing PascalCase rename). `sample_all_response_variants()`'s exhaustive match gained the `Connected` arm (no wildcard) — the CONFIG-03 planted-secret regression automatically covers it since it has no credential field.

**Task 3 — `error.rs`:** Added `WireErrorCode::DuplicateSession`, emitting `"duplicate-session"` under the existing kebab-case rename — the wire-level signal for SESSION-04's name/id collision rejection, a daemon-only concept with no `rdpilot::Error` equivalent (same category as `SessionNotFound`/`DaemonUnreachable`).

## Verification

- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — 17/17 tests green (all modules).
- `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo build -p rdpilot-ipc --target x86_64-unknown-linux-gnu` — succeeds.
- `cargo tree -p rdpilot-ipc` confirms zero `rdpilot` dependency — thin-client invariant (D-17) preserved.
- `cargo clippy -p rdpilot-ipc --target x86_64-unknown-linux-gnu --tests` — zero warnings.

## Deviations from Plan

**Offline/target substitution (not a deviation from plan content, a repeated environment note):** This host has no `x86_64-pc-windows-gnu` Rust target installed; all verification ran on `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu --target x86_64-unknown-linux-gnu` exactly as Phases 10/11 did and as this plan's own `<verify>` blocks specify. No functional deviation.

None otherwise — plan executed exactly as written. All three tasks' acceptance criteria are met verbatim (grep-checkable claims below).

- `request.rs` contains `Connect {`, `List {`, `Disconnect {` — confirmed.
- `fn session(&self) -> Option<&SessionId>` — confirmed, no wildcard arm in the match.
- A test asserts `{"op":"Disconnect"}` (no session) is `Err` — `disconnect_without_a_session_field_is_a_hard_rejection`.
- A test asserts `{"op":"Connect",...}` with no `session` is `Ok` and `session()` is `None` — `connect_without_a_session_field_is_accepted_and_session_is_none`.
- Existing six-verb missing-session rejection test remains and still asserts `is_err()` — `every_operational_request_verb_rejects_a_missing_session_field`.
- `response.rs` contains `Connected {` with a `session: SessionId` field — confirmed.
- `SessionLifecycle` contains `Orphaned` — confirmed, emits `"Orphaned"` (`session_lifecycle_emits_the_d30_vocabulary`).
- `error.rs` `WireErrorCode` contains `DuplicateSession`, emits `"duplicate-session"`, round-trips — confirmed.

## Known Stubs

None — this plan produces pure type/schema additions with full inline test coverage; no runtime behavior is stubbed.

## Threat Flags

None — this plan's `<threat_model>` (T-12-01/02/03) is fully covered by the implementation as described in the plan; no new surface outside that register was introduced.

## Self-Check: PASSED

- FOUND: crates/rdpilot-ipc/src/request.rs (contains `Connect {`, `List {`, `Disconnect {`)
- FOUND: crates/rdpilot-ipc/src/response.rs (contains `Connected {`, `Orphaned`)
- FOUND: crates/rdpilot-ipc/src/error.rs (contains `DuplicateSession`)
- FOUND commit 8d3d1e5 (Task 1: request.rs)
- FOUND commit c6546ea (Task 2: response.rs)
- FOUND commit ef6c4ee (Task 3: error.rs)
