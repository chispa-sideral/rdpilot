# Requirements: rdpilot — v1.1 (Consumer Surfaces & File Transfer)

**Defined:** 2026-07-10
**Core Value:** A local AI agent can connect to a remote Windows desktop over RDP and read/inspect a program that is only reachable via RDP — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.

## v1.1 Requirements

Requirements for the v1.1 milestone. Each maps to roadmap phases.

### Session Daemon

- [ ] **DAEMON-01**: A long-lived daemon holds N live RDP sessions with keepalive, decoupled from any CLI process lifetime
- [ ] **DAEMON-02**: A local IPC transport (unix socket / Windows named pipe) carries requests between clients (CLI, MCP) and the daemon, restricted to the local user (permission/DACL-scoped) — Unix path complete (12-04); Windows DACL path deferred to the 12-07 live gate
- [x] **DAEMON-03**: The daemon auto-starts on first client connect and reaps idle sessions / self-shuts-down when the registry empties
- [ ] **DAEMON-04**: On restart, the daemon detects and tears down orphaned Windows-side sessions (in-memory registry; no reattach)

### Session Lifecycle & Identity

- [ ] **SESSION-01**: User can open a session under a caller-supplied name, or receive an auto-generated id when unnamed
- [x] **SESSION-02**: Every perception/input/file command explicitly targets a session by name/id — no implicit default
- [x] **SESSION-03**: User can list active sessions (name/id, target, status)
- [ ] **SESSION-04**: User can disconnect a named session; names/ids are unique (collisions rejected)

### CLI Surface

- [x] **CLI-01**: A `rdpilot` CLI manages session lifecycle (connect [--name] / list / disconnect) over the daemon — COMPLETE (13-05): the `rdpilot-cli` crate/binary implements `connect [--name] / list / disconnect`, transparently auto-starting the daemon via the relocated `rdpilot_ipc::transport::connect_or_spawn` (13-01); offline-proven end-to-end against the real `rdpilot-daemon` binary (fake connector) in `tests/cli_lifecycle.rs`; `cargo tree -p rdpilot-cli` confirmed free of `ironrdp`/`rustls`/`rdpilot`/`rdpilot-daemon` (thin-client invariant, D-17)
- [x] **CLI-02**: CLI exposes the full perception + input + launch verb set (screenshot, world_state, UIA, window/process list, click/type/key/scroll/drag, launch, foreground), each targeting a named session
- [x] **CLI-03**: CLI exposes file put/get and reports errors clearly (session-not-found, daemon-unreachable, transfer failure) — COMPLETE (13-07): `rdpilot put`/`get` (flat) and `rdpilot file put|get` (grouped) absolutize `--local` via `current_dir()` join before sending (no relative path ever hits the wire — the daemon's cwd differs from the invoking shell's); `get`'s no-clobber is fully CLI-side enforced (`exists()` check before the request is sent, `CliError::NoClobber`, exit 8, `--force` override) while `put`'s remote no-clobber is a documented known gap this phase (`--force` accepted for forward-compat only; `put --help` names the gap; tracked as backlog Phase 999.5 — symmetric remote no-clobber via a C# sensor `File.Exists` check, not built this phase); the finalized `exit_codes.rs` taxonomy maps every `WireErrorCode` plus the CLI-local `DaemonUnreachable`/`NoClobber` classes to 8 distinct non-zero exit codes, and `--json` errors emit `{"error":{"code":"<kebab>","message":"..."}}`; offline-proven in `tests/cli_errors.rs` against the real `rdpilot-daemon` binary (session-not-found exit 2, daemon-unreachable exit 3 — CLI binary copied without a sibling daemon binary so auto-spawn fails immediately, no-clobber exit 8 then `--force` succeeds); real multi-MB transfer correctness (already Phase-10 live-verified, FILE-01/02/04) is re-exercised through the CLI at the Phase 15 batched live gate, not re-proven here

### MCP Server Surface

- [x] **MCP-01**: An `rmcp`-based MCP server exposes rdpilot over MCP to any MCP client (e.g. Claude)
- [x] **MCP-02**: MCP exposes a single Anthropic computer-use-compatible `computer` tool (screenshot + action-discriminated mouse/keyboard/scroll) mapping onto the SDK input/capture verbs
- [x] **MCP-03**: MCP exposes rdpilot-native tools (world_state, UIA, window/process list, launch, foreground, session connect/list/disconnect, file put/get) as MCP Tools
- [x] **MCP-04**: The computer-use surface bridges rdpilot's 96-DPI physical-pixel coordinates to the tool's expected scaled screenshot/coordinate space
- [x] **MCP-05**: MCP file put/get operate on local disk paths and return path/size/checksum metadata (never inline file bytes)
- [ ] **MCP-06**: Slow RDP round-trips (file transfer, launch waits) do not block the MCP transport event loop

### Bidirectional File Transfer

- [x] **FILE-01**: User can upload a file local→remote to a named session — LIVE-VERIFIED end-to-end (10-05): `Session::upload_file` round-trips against a real Azure VM
- [x] **FILE-02**: User can download a file remote→local from a named session — LIVE-VERIFIED end-to-end (10-05): `Session::download_file`'s bytes_transferred/checksum independently confirmed against a real Azure VM
- [x] **FILE-03**: Transfer validates remote paths via canonicalization to prevent path traversal / arbitrary write (blocking security requirement) — LIVE re-confirmed (10-05): the mixed-separator and Windows-drive-absolute adversarial cases both reject as `Error::PathTraversal` on the real Windows target, both directions
- [x] **FILE-04**: Large files transfer reliably (chunked); partial-transfer failure is detected and surfaced — LIVE-VERIFIED (10-05): a 3 MiB file transfers correctly (102-137 read IRPs / 48 write IRPs observed, real per-IRP write chunk = 65,536 bytes), and an abruptly-interrupted download leaves no final-named file

### Connection Config

- [x] **CONFIG-01**: Consumers supply target host + credentials via layered config — a clearly-named gitignored file, overridable by env vars and CLI flags / MCP init params
- [x] **CONFIG-02**: The config file follows common CLI-tool convention (platform config dir / clearly-named `.rdpilot.*`), discoverable and self-explanatory
- [x] **CONFIG-03**: Credentials never leak through IPC/MCP wire responses or logs — Serialize paths redact secrets (closing the v1.0 D-14 Debug-only redaction gap)

### Proof

- [ ] **PROOF-02**: A scripted harness proves the CLI surface end-to-end against a real remote-only Windows program (no live LLM)
- [ ] **PROOF-03**: A scripted harness proves the MCP surface end-to-end (tool calls exercised programmatically, no live LLM)
- [ ] **PROOF-04**: A capstone live-LLM demo drives a read/inspect + file-transfer task through the MCP surface against a real remote-only Windows program

## Future Requirements

Deferred to future release. Tracked but not in current roadmap.

- PyO3 / NAPI-RS bindings exposing the SDK to Python and TypeScript consumers
- Clipboard read/write over CLIPRDR (text sync from remote apps)
- MCP progress notifications (notifications/progress) for long transfers
- Durable session reattach across daemon restart (v1.1 uses in-memory registry + orphan cleanup)
- Remote-assist / session shadowing (Backlog Phase 999.4 / SEED-001)

## Out of Scope

Explicitly excluded. Documented to prevent scope creep.

| Feature | Reason |
|---------|--------|
| Running the AI agent on the remote session | Explicitly rejected; intelligence stays local |
| Non-Windows RDP targets (Linux/xrdp, macOS) | Windows-only to lean on UIA/WinRM/WMI |
| Multi-session orchestration / concurrency AT SCALE | The registry holds multiple named sessions, but load/scale orchestration is out of scope |
| Published-package polish (public API stability guarantees, comprehensive docs, multi-registry distribution) | Remains personal tooling first |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| FILE-01 | Phase 10 | Complete (live-verified 10-05) |
| FILE-02 | Phase 10 | Complete (live-verified 10-05) |
| FILE-03 | Phase 10 | Complete (live-verified 10-05, BLOCKING re-confirm passed) |
| FILE-04 | Phase 10 | Complete (live-verified 10-05, BLOCKING re-confirm passed) |
| SESSION-02 | Phase 11 | Complete |
| CONFIG-01 | Phase 11 | Complete |
| CONFIG-02 | Phase 11 | Complete |
| CONFIG-03 | Phase 11 | Complete |
| DAEMON-01 | Phase 12 | In Progress (12-01/02/03: registry-level leak-free session holding proven — SC#3 BLOCKING thread+RSS soak passes across N=50 real connect/disconnect cycles; the long-lived daemon *process* with keepalive, decoupled from any CLI process lifetime, is Wave 4/6, 12-04/12-06) |
| DAEMON-02 | Phase 12 | In Progress (12-04: Unix path complete — `0700` runtime dir (atomic mode-at-creation) + per-connection `peer_cred()` uid check; SC#4 [BLOCKING] offline proof passes `cargo test -p rdpilot-daemon --test ipc_security` — `authorize_uid` rejects a mismatched uid, accepts a matching one, socket dir confirmed mode 0700; a real cross-account variant is gated behind `RDPILOT_SECOND_UID`+`#[ignore]`. Windows explicit-DACL path deferred to the 12-07 live gate) |
| DAEMON-03 | Phase 12 | Complete (12-06: auto-start on first client connect (`autostart::connect_or_spawn`, bind-as-mutex) + idle reap (routed through the awaited `Registry::close`) + empty-registry grace-period self-shutdown (D-31 anti-thrash) all proven OFFLINE against the REAL compiled `rdpilot-daemon` binary — `tests/autostart_lifecycle.rs`'s `#[ignore]`-gated SC#5 [BLOCKING] test, run via `cargo test -p rdpilot-daemon --test autostart_lifecycle -- --include-ignored`. This requirement's full text (auto-start + idle-reap + self-shutdown-on-empty) has no live/Windows component of its own — DAEMON-03 is fully satisfied by this plan. The broader ROADMAP success criterion 5 [BLOCKING] also combines DAEMON-04's remote-liveness confirmation, which remains pending in 12-07) |
| DAEMON-04 | Phase 12 | In Progress (12-05: offline crash-restart reconciliation mechanics proven — kill -9 surrogate + restart surfaces the prior session as `Orphaned` in `list`, never silently forgotten, reconciled only via explicit `close`; SC#5 [BLOCKING] offline portion passes `cargo test -p rdpilot-daemon --test crash_restart_reconcile`. Live confirmation that the remote Windows session is genuinely still live vs. logged off is deferred to Plan 12-07) |
| SESSION-01 | Phase 12 | In Progress (12-01/02/03: wire verb `Request::Connect` + registry `open()` with caller-name/auto-id (D-29) both implemented and tested — SC#1 concurrency proves auto-id collision-safety; 12-04: wire-level dispatch now routes `Request::Connect` to `Registry::open` and returns `WireResponse::Connected`/`Error(DuplicateSession)`, unit-tested in `dispatch.rs`. Remaining: the actual daemon process/IPC transport a real client connects through is Wave 5, 12-06) |
| SESSION-03 | Phase 12 | Complete (12-04: `dispatch`'s `List {}` arm returns the registry's credential-free `SessionList`, with `connected_since`/`last_activity` now wall-clock-populated for live entries — registry.rs/seams.rs extended this wave to finish the wiring their own doc comments had flagged as outstanding; a dispatch-level unit test asserts field completeness) |
| SESSION-04 | Phase 12 | In Progress (12-01/02/03: wire verb `Request::Disconnect` + registry `close()`/uniqueness enforcement both implemented and tested — SC#1 proves N=16 same-name contention yields exactly one winner; 12-04: wire-level dispatch now routes `Request::Disconnect` to `Registry::close` and returns `Ack`/`Error(SessionNotFound)`, unit-tested in `dispatch.rs`. Remaining: the actual daemon process/IPC transport a real client connects through is Wave 5, 12-06) |
| CLI-01 | Phase 13 | Complete (13-05: the `rdpilot-cli` crate/binary implements `connect [--name] / list / disconnect`, transparently auto-starting the daemon via the relocated `rdpilot_ipc::transport::connect_or_spawn` (13-01) — proven offline end-to-end against the real compiled `rdpilot-daemon` binary with the fake connector in `tests/cli_lifecycle.rs` (no daemon pre-started; auto-start observed; `Live` status + name/host round-trip through `list --json`; `SessionNotFound` maps to exit code 2). `cargo tree -p rdpilot-cli` confirmed free of `ironrdp`/`rustls`/`rdpilot`/`rdpilot-daemon` (thin-client invariant, D-17). The shared clap tree/config-flag layer/exit-code taxonomy/table renderer this plan also laid down are reused by 13-06/13-07) |
| CLI-02 | Phase 13 | In Progress (13-03: daemon-side seam complete. 13-04: `dispatch.rs`'s exhaustive match now wires every operational verb (screenshot/world_state/window+process list/uia/mouse/key/foreground/launch) through `Registry::call` to the live `rdpilot::Session`, with wire<->SDK conversion and offline dispatch tests proving each verb against a canned fake session — the `not_implemented_for` stub is gone. Remaining: the actual `rdpilot` CLI binary that issues these requests over IPC is 13-06) |
| CLI-03 | Phase 13 | Complete (13-03: daemon-side seam complete. 13-04: `dispatch.rs`'s `Put`/`Get` arms call `upload_file`/`download_file` via `Registry::call`, and the pre-existing Phase-12 `share_root` gap is fixed. 13-07: `rdpilot put`/`get` (flat) + `rdpilot file put\|get` (grouped) absolutize `--local` client-side (`current_dir()` join, not the 1.79-only stdlib helper — workspace pins 1.78); `get`'s no-clobber is fully CLI-side enforced (exit 8, `--force` override), `put`'s remote no-clobber is a documented known gap (backlog Phase 999.5); `exit_codes.rs` finalized to 8 distinct non-zero exit codes plus `--json` `{"error":{"code":"<kebab>","message":"..."}}` rendering; offline-proven in `tests/cli_errors.rs` (session-not-found exit 2, daemon-unreachable exit 3, no-clobber exit 8/`--force` exit 0) against the real `rdpilot-daemon` binary. Real transfer-bytes correctness deferred to the Phase 15 batched live gate) |
| MCP-01 | Phase 14 | Complete |
| MCP-02 | Phase 14 | Complete |
| MCP-03 | Phase 14 | Complete |
| MCP-04 | Phase 14 | Complete |
| MCP-05 | Phase 14 | Complete |
| MCP-06 | Phase 14 | Pending |
| PROOF-02 | Phase 15 | Pending |
| PROOF-03 | Phase 15 | Pending |
| PROOF-04 | Phase 15 | Pending |

**Coverage:**

- v1.1 requirements: 27 total
- Mapped to phases: 27 ✓
- Unmapped: 0

---
*Requirements defined: 2026-07-10*
*Last updated: 2026-07-10 after initial v1.1 definition*
