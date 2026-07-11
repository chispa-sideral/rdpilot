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

- [ ] **CLI-01**: A `rdpilot` CLI manages session lifecycle (connect [--name] / list / disconnect) over the daemon — prerequisite transport relocation complete (13-01): `connect_or_spawn`/`socket_path`/framing now live in rdpilot-ipc, reachable without depending on rdpilot/IronRDP; the CLI binary itself (13-05) remains outstanding
- [ ] **CLI-02**: CLI exposes the full perception + input + launch verb set (screenshot, world_state, UIA, window/process list, click/type/key/scroll/drag, launch, foreground), each targeting a named session
- [ ] **CLI-03**: CLI exposes file put/get and reports errors clearly (session-not-found, daemon-unreachable, transfer failure)

### MCP Server Surface

- [ ] **MCP-01**: An `rmcp`-based MCP server exposes rdpilot over MCP to any MCP client (e.g. Claude)
- [ ] **MCP-02**: MCP exposes a single Anthropic computer-use-compatible `computer` tool (screenshot + action-discriminated mouse/keyboard/scroll) mapping onto the SDK input/capture verbs
- [ ] **MCP-03**: MCP exposes rdpilot-native tools (world_state, UIA, window/process list, launch, foreground, session connect/list/disconnect, file put/get) as MCP Tools
- [ ] **MCP-04**: The computer-use surface bridges rdpilot's 96-DPI physical-pixel coordinates to the tool's expected scaled screenshot/coordinate space
- [ ] **MCP-05**: MCP file put/get operate on local disk paths and return path/size/checksum metadata (never inline file bytes)
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
| CLI-01 | Phase 13 | In Progress (13-01: prerequisite transport relocation complete — `socket_path`/`connect_or_spawn`/length-prefixed framing now live in `rdpilot_ipc::transport`, verified `cargo tree` free of `ironrdp`/`rustls`, so the CLI binary (13-05) can auto-start/reach the daemon without depending on `rdpilot`. The `rdpilot` CLI binary itself, and its connect/list/disconnect verbs, remain outstanding) |
| CLI-02 | Phase 13 | In Progress (13-03: daemon-side seam complete — `ManagedSession` extended with the full `&self` operational method set (screenshot/world_state/window+process list/uia/mouse/key/foreground/launch/ping), `impl` for `rdpilot::Session`, and a deadlock-free `Registry::call` dispatch path. Dispatch is not yet wired to these methods — that is 13-04) |
| CLI-03 | Phase 13 | In Progress (13-03: daemon-side seam complete — `ManagedSession::upload_file`/`download_file` added and reachable via `Registry::call`; dispatch wiring for `Put`/`Get` remains 13-04) |
| MCP-01 | Phase 14 | Pending |
| MCP-02 | Phase 14 | Pending |
| MCP-03 | Phase 14 | Pending |
| MCP-04 | Phase 14 | Pending |
| MCP-05 | Phase 14 | Pending |
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
