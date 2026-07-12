# Roadmap: rdpilot

## Milestones

- ✅ **v1.0 MVP** — Phases 1-9 (shipped 2026-07-10)
- ✅ **v1.1 Consumer Surfaces & File Transfer** — Phases 10-15 (shipped 2026-07-11)

## Phases

Phase details for shipped milestones are archived. Full phase directories now live under `.planning/milestones/v1.0-phases/`.

<details>
<summary>✅ v1.0 MVP (Phases 1-9) — SHIPPED 2026-07-10</summary>

- [x] Phase 1: Test Environment (4/4 plans) — completed 2026-06-05
- [x] Phase 2: RDP Session + Framebuffer Core (3/3 plans) — completed 2026-06-05
- [x] Phase 3: Input Injection (4/4 plans) — completed 2026-07-08
- [x] Phase 4: DVC Transport Channel (3/3 plans) — completed 2026-07-09
- [x] Phase 5: Sensor Bootstrap + Deployment (4/4 plans) — completed 2026-07-09
- [x] Phase 6: Window + Process Perception (5/5 plans) — completed 2026-07-09
- [x] Phase 7: UIA Tree Module (5/5 plans) — completed 2026-07-09
- [x] Phase 8: Public SDK API + WorldState (3/3 plans) — completed 2026-07-09
- [x] Phase 9: Scripted Proof Harness (4/4 plans) — completed 2026-07-10

Full phase goals, success criteria, and plan-by-plan detail: `.planning/milestones/v1.0-ROADMAP.md`.

</details>

### v1.1 — Consumer Surfaces & File Transfer (Phases 10-15)

- [x] **Phase 10: SDK File-Transfer Extension** — Generalize the RDPDR drive backend to bidirectional, allow-listed, canonicalization-guarded file transfer with sensor upload/download commands. (completed 2026-07-10)
- [x] **Phase 11: Shared Wire Protocol & Config** — `rdpilot-ipc` (required-session-id schema, credential-free DTOs) + `rdpilot-config` (layered file/env/flag resolution). (completed 2026-07-10)
- [x] **Phase 12: Session Daemon** — Long-lived daemon with a leak-free named-session registry, local-only DACL/peer-scoped IPC, auto-start/idle-shutdown, and crash-restart orphan reconciliation. (completed 2026-07-11: 6/7 plans executed offline (12-01..12-06, all offline success criteria verified — see `12-VERIFICATION.md`); Plan 12-07's live-gate scope (Windows explicit-DACL pipe, live orphan-liveness confirmation, e2e session verify) was superseded by design and re-authored/live-verified via Phase 15's 15-01 (offline author) / 15-05 (Windows DACL live-run) / 15-06 (orphan-liveness + e2e live-run) — not an incomplete phase, functionally complete)
- [x] **Phase 13: CLI Surface** — Thin `rdpilot` CLI over the daemon: lifecycle + perception/input/launch/file verbs, each explicitly targeting a named session. (completed 2026-07-11)
- [x] **Phase 14: MCP Server Surface** — `rmcp` server exposing a computer-use `computer` mega-tool + rdpilot-native tools, with a tested coordinate-scaling bridge and non-blocking per-call isolation. (completed 2026-07-11)
- [x] **Phase 15: Proof Harnesses & Live-LLM Capstone** — Scripted per-surface proof (CLI, MCP, no live LLM) plus the capstone live-LLM read/inspect + file-transfer demo through MCP. (completed 2026-07-11: Waves 3-6 LIVE-RUN — single VM provisioned and held UP through 15-05/06/07/08; DAEMON-02 Windows half, PROOF-02, PROOF-03, and PROOF-04 (capstone) all live-verified PASS. Developer-authorized teardown (`manage-env.ps1 down`) confirmed the test RG destroyed (`az group exists -n rdpilot-test` => `false`), management RG `rdpilot-mgmt` persists. v1.1 milestone's proof/live-gate track is now fully closed.)

## Phase Details

### Phase 10: SDK File-Transfer Extension

**Goal**: The SDK can move files in both directions between local disk and the remote target through the already-proven RDPDR channel plus new sensor-mediated commands — safely, and without touching `Session`'s threading model.
**Depends on**: Nothing new (extends the v1.0 SDK: `RdpilotDriveBackend`, the DVC sensor protocol)
**Requirements**: FILE-01, FILE-02, FILE-03, FILE-04
**Success Criteria** (what must be TRUE):

  1. A local file uploads to a named remote destination (`Session::upload_file`) and is verified present on the target (FILE-01).
  2. A remote file downloads to a named local destination (`Session::download_file`) with matching size/checksum (FILE-02).
  3. **[BLOCKING]** An adversarial path-traversal test suite — trailing `..` with no separator, mixed `/`/`\` separators, and absolute-path-as-relative inputs — is rejected by full canonicalized ancestry validation under the share root (never substring matching), with no `Write`/`Create` IRP escaping the share root (FILE-03; Pitfall 8 / FreeRDP GHSA-3xpj-m4hx-8vmx / CVE-2025-48817).
  4. A file larger than one MS-RDPEFS per-IRP chunk transfers correctly (the chunked read/write loop actually loops), and an interrupted transfer surfaces a clean, detectable failure (staged-and-renamed) rather than silent corruption (FILE-04).

**Plans**: 5/5 plans complete

- [x] 10-01-PLAN.md — Rust foundation: error taxonomy + share-root config + canonicalizing path validator + backend generalization + FILE-03 Rust adversarial suite (Wave 1)
- [x] 10-02-PLAN.md — Staged-write IRPs (DeviceWrite + SetInformation) + atomic-rename-on-Close + FILE-04 multi-IRP/interrupted offline proxy (Wave 2)
- [x] 10-03-PLAN.md — Sensor FileTransfer wire variant + C# copy-with-inline-SHA256 handler + C# GetRelativePath validator + FILE-03 C# selftest (Wave 1)
- [x] 10-04-PLAN.md — Public Session::upload_file/download_file + TransferOutcome + SHA-256 verify / ChecksumMismatch (Wave 3)
- [x] 10-05-PLAN.md — Live Azure VM gate: FILE-01/02/04 end-to-end + FILE-03 live re-confirm + real chunk-size measurement (Wave 4)

### Phase 11: Shared Wire Protocol & Config

**Goal**: One shared crate defines the daemon↔client wire protocol and another resolves layered configuration — with session-identity-as-required-field and credential-redaction enforced at the schema level *before* any consumer binary is built on top.
**Depends on**: Phase 10 (wire protocol must cover the file-transfer request/response variants)
**Requirements**: SESSION-02, CONFIG-01, CONFIG-02, CONFIG-03
**Success Criteria** (what must be TRUE):

  1. **[BLOCKING]** Every session-scoped request carries a required, non-optional `session: SessionId` field; omitting it is a hard schema-validation rejection at the wire boundary (not a fallback), verified by a test that omits the field on every verb (SESSION-02; Pitfall 2).
  2. Target host + credentials resolve through a layered file → env → flag/MCP-init precedence, with the highest-precedence layer deterministically winning (CONFIG-01).
  3. The config file follows common CLI-tool convention (platform config dir / clearly-named `.rdpilot.*`), is gitignored, and is discoverable and self-explanatory (CONFIG-02).
  4. **[BLOCKING]** Serializing every wire response type and grepping the output for a planted secret sentinel finds nothing — credential-free status DTOs, no plaintext password on the wire or in logs (CONFIG-03; Pitfall 4, closing the v1.0 D-14 Debug-only redaction gap).

**Plans**: 2/2 plans complete

- [x] 11-01-PLAN.md — `rdpilot-ipc` crate: required-session Request schema (SESSION-02) + credential-free WireResponse DTOs (CONFIG-03) + WireError taxonomy types + TransferOutcome mirror; zero rdpilot dependency (Wave 1)
- [x] 11-02-PLAN.md — `rdpilot-config` crate: platform-config-dir path via BaseDirs (CONFIG-02) + layered file→env→override resolution (CONFIG-01) + commented template; Serialize-free ResolvedConfig, zero rdpilot dependency (Wave 2)

### Phase 12: Session Daemon

**Goal**: A long-lived local daemon holds N named RDP sessions behind a correct, leak-free, local-only registry and survives its own crashes without orphaning remote Windows sessions.
**Depends on**: Phase 11 (registry and dispatch are written against the shared protocol types)
**Requirements**: DAEMON-01, DAEMON-02, DAEMON-03, DAEMON-04, SESSION-01, SESSION-03, SESSION-04
**Success Criteria** (what must be TRUE):

  1. A session opens under a caller-supplied name (or a short, human-legible auto-id when unnamed) and stays addressable on later commands; duplicate names are rejected, and a concurrency test firing N simultaneous same-name connects yields exactly one live session and N-1 clean rejections (SESSION-01, SESSION-04; Pitfall 3 atomic insert).
  2. `list` reports active sessions with name/id, target, status, and connected-since / last-activity (SESSION-03).
  3. **[BLOCKING]** A soak test of N connect/disconnect cycles returns thread count and RSS to baseline — no session-per-OS-thread leak (DAEMON-01; Pitfall 1).
  4. **[BLOCKING]** The IPC transport is restricted to the local user (Unix `0700` dir + peer-uid check / Windows explicit DACL via `create_with_security_attributes_raw`), verified by a different-uid/other-account client being rejected (DAEMON-02; Pitfall 5).
  5. **[BLOCKING]** The daemon auto-starts on first client connect and self-shuts-down when the registry empties; after a `kill -9` mid-session and restart it reports the possibly-still-live remote session and tears down / reconciles orphans rather than silently forgetting them (DAEMON-03, DAEMON-04; Pitfall 9, in-memory registry + minimal disk-persisted reconciliation state).

**Plans**: 6/7 plans executed — functionally complete (12-07 superseded by design, see below; its scope is live-verified via 15-01/15-05/15-06)

- [x] 12-01-PLAN.md — `rdpilot-ipc` lifecycle-verb extension: Connect/List/Disconnect + WireResponse::Connected + WireErrorCode::DuplicateSession + SessionLifecycle::Orphaned; SessionScoped -> Option (Wave 1)
- [x] 12-02-PLAN.md — `rdpilot-daemon` crate scaffold + workspace member + session/reconciliation seams + rdpilot::Error->WireError mapping (Wave 2)
- [x] 12-03-PLAN.md — Registry: atomic claim-then-connect + auto-id (D-29) + close-not-drop teardown; SC#1 concurrency test + SC#3 [BLOCKING] thread/RSS soak (Wave 3)
- [x] 12-04-PLAN.md — Unix IPC (0700 dir + peer-uid) + framing + dispatch (incl `list`, SESSION-03); SC#4 [BLOCKING] different-uid-rejected (Wave 4)
- [x] 12-05-PLAN.md — Crash-survivable reconciliation: JSON disk record + startup orphan scan/seed; SC#5 [BLOCKING] offline crash-restart-surface (DAEMON-04) (Wave 4)
- [x] 12-06-PLAN.md — Server assembly + idle reaper + empty-grace self-shutdown + connect-or-spawn auto-start; SC#5 [BLOCKING] auto-start/self-shutdown (DAEMON-03) (Wave 5)
- [ ] 12-07-PLAN.md — Live gate: Windows explicit-DACL pipe + anti-squatting (DAEMON-02) + live remote-liveness reconciliation (DAEMON-04) + e2e session verify; `windows-permissions` legitimacy checkpoint (Wave 6). **Superseded by Phase 15's file split (15-RESEARCH.md):** the DACL code + both split live-gate test files are now AUTHORED via `windows-sys` (no legitimacy checkpoint needed) in 15-01-PLAN.md; the actual live compile+run closes this plan's remaining scope in 15-05 (Windows DACL) / 15-06 (orphan-liveness + e2e).

### Phase 13: CLI Surface

**Goal**: A thin `rdpilot` CLI drives the full session/perception/input/file verb set over the daemon, with every command explicitly targeting a named session.
**Depends on**: Phase 12 (CLI is a thin `rdpilot-ipc` client of the daemon)
**Requirements**: CLI-01, CLI-02, CLI-03
**Success Criteria** (what must be TRUE):

  1. `rdpilot connect [--name] / list / disconnect` manages session lifecycle over the daemon, transparently auto-starting it on first use (CLI-01).
  2. The full perception + input + launch verb set (screenshot, world_state, uia, window/process list, click/type/key/scroll/drag, launch, foreground) runs against an explicit `--session` (CLI-02).
  3. `put`/`get` transfer files (no-clobber by default with an explicit `--force`), and failures — session-not-found, daemon-unreachable, transfer failure — surface as distinct, legible errors (CLI-03).

**Plans**: 7/7 plans complete — Complete offline (CLI-01/02/03 all proven against the real compiled `rdpilot-daemon` binary with its canned fake connector; no live Windows target required for this phase's own verification)

- [x] 13-01-PLAN.md — Relocate the auto-start transport (socket_path/framing/connect_or_spawn) from rdpilot-daemon into rdpilot-ipc so the CLI reaches the daemon without the RDP stack (Wave 1)
- [x] 13-02-PLAN.md — Extend rdpilot-ipc with perception/input wire DTOs + six Request / four WireResponse verbs, preserving SESSION-02 (Wave 2)
- [x] 13-03-PLAN.md — Extend ManagedSession with operational &self methods + Arc<TokioMutex<Option<Box>>> registry storage + Registry::call (Wave 2)
- [x] 13-04-PLAN.md — Wire every operational dispatch arm to the live Session + fix the share_root Connect gap from config (Wave 3)
- [x] 13-05-PLAN.md — rdpilot-cli scaffold + connect/list/disconnect lifecycle verbs, thin-client + auto-start (CLI-01, Wave 4)
- [x] 13-06-PLAN.md — CLI perception/input/launch verb set against --session, screenshot --output, base64 legitimacy gate (CLI-02, Wave 5)
- [x] 13-07-PLAN.md — CLI put/get no-clobber (asymmetric) + path absolutization + distinct error taxonomy/exit codes (CLI-03, Wave 6)

**Deferred to the Phase 15 batched live gate** (never re-litigated per-plan, re-exercised once through the CLI surface at the gate): real screenshot pixel content, real click/UIA-tree shape against a live Windows target (CLI-02); a real multi-MB `put`/`get` transfer's bytes-transferred/checksum correctness through the CLI (CLI-03 — FILE-01/02/04 were already live-verified in Phase 10; this re-exercises the same semantics through a new client surface, it does not re-prove them). `put`'s remote no-clobber asymmetry is NOT part of the Phase 15 gate — it is tracked separately as backlog Phase 999.5.

### Phase 14: MCP Server Surface

**Goal**: An `rmcp`-based MCP server exposes rdpilot to any MCP client through a computer-use-compatible mega-tool plus rdpilot-native tools — without coordinate drift or event-loop stalls.
**Depends on**: Phase 13 (built on the daemon/IPC foundation the CLI already validated end-to-end)
**Requirements**: MCP-01, MCP-02, MCP-03, MCP-04, MCP-05, MCP-06
**Success Criteria** (what must be TRUE):

  1. An MCP client (e.g. Claude) can call a single Anthropic computer-use-compatible `computer` tool (screenshot + action-discriminated mouse/keyboard/scroll) that maps onto the SDK input/capture verbs (MCP-01, MCP-02).
  2. rdpilot-native tools (world_state, uia, window/process list, launch, foreground, session connect/list/disconnect, file put/get returning `{path, bytes_transferred, checksum}` metadata only — never inline file bytes) are exposed as MCP Tools (MCP-03, MCP-05).
  3. **[BLOCKING]** Coordinates round-trip correctly: one fixed advertised resolution + a single tested `scale_to_native(x, y)` bridges rdpilot's 96-DPI physical-pixel space to the computer-use scaled screenshot/coordinate space, verified by a precision click test near screen edges/corners (not just center) (MCP-04; Pitfall 6).
  4. **[BLOCKING]** A slow tool call (large file transfer, `launch_process` wait) does not block a concurrent unrelated fast tool call — per-call task isolation with bounded, explicit timeouts (MCP-06; Pitfall 7).

**Plans**: 5/5 plans complete

- [x] 14-01-PLAN.md — Upstream wire/daemon extensions: DesktopSize verb (native-dim sourcing for the bridge) + shared parse_wire_key + daemon dispatch (MCP-02/04, Wave 1)
- [x] 14-02-PLAN.md — rdpilot-mcp thin-client scaffold: rmcp stdio server + bounded-timeout transport + error taxonomy + thin-client cargo-tree gate (MCP-01/06, Wave 1)
- [x] 14-03-PLAN.md — `computer` mega-tool: scale_to_native [BLOCKING MCP-04] + computer_20250124 action schema + dispatch mapping + 3 explicit gap rejections (MCP-02/04, Wave 2)
- [x] 14-04-PLAN.md — 11 rdpilot_* native tools + put/get metadata-only [MCP-05] + connect config resolution + trust-model README + tools/list schema test (MCP-01/03/05, Wave 3)
- [x] 14-05-PLAN.md — MCP-06 [BLOCKING] non-blocking isolation proof: slow tool ∥ fast tool against the real compiled daemon + fake-connector slowness hook (MCP-06, Wave 4)

**Offline this phase; batched to Phase 15 live gate**: real screenshot pixel content, real click landing near edges/corners (the live half of MCP-04 — the pure scale_to_native is offline-proven here), and the live-LLM capstone (PROOF-04). Phase 14 is maximally offline-verifiable on the native-Linux substitute: tool schema/registration, dispatch against a fake daemon/session, scale_to_native (pure), the action mapping + 3 rejections, metadata-only file responses, and the non-blocking-isolation test (MCP-06, fully self-contained — architecture-level, not target-host-dependent, so it does not need re-exercising at the live gate).

### Phase 15: Proof Harnesses & Live-LLM Capstone

**Goal**: Both consumer surfaces are proven end-to-end by scripted harnesses, and a live LLM drives a real read/inspect + file-transfer task through MCP — the milestone's dual finish line.
**Depends on**: Phase 14 (validates the combination of every prior phase functioning end-to-end)
**Requirements**: PROOF-02, PROOF-03, PROOF-04
**Success Criteria** (what must be TRUE):

  1. A scripted harness proves the CLI surface end-to-end against a real remote-only Windows program, with no live LLM (PROOF-02).
  2. A scripted harness proves the MCP surface end-to-end — tool calls exercised programmatically — with no live LLM (PROOF-03).
  3. A capstone live-LLM demo drives a read/inspect + file-transfer task through the MCP surface against a real remote-only Windows program (PROOF-04).

**Plans**: 8/8 plans complete

- [x] 15-01-PLAN.md — OFFLINE-AUTHOR: 12-07 windows-sys owner-only DACL + split live test files (Windows DACL / Linux e2e) (Wave 1) (completed 2026-07-11)
- [x] 15-02-PLAN.md — OFFLINE-AUTHOR: PROOF-02 CLI harness + CLI-02/03 live re-exercise (gated tests) (Wave 1) (completed 2026-07-11)
- [x] 15-03-PLAN.md — OFFLINE-AUTHOR: rmcp client dev-deps + PROOF-03 MCP harness + MCP-04 live half (Wave 1) (completed 2026-07-11)
- [x] 15-04-PLAN.md — OFFLINE-AUTHOR: PROOF-04 capstone via `claude -p` (transcript + side-effect verification) (Wave 2) (completed 2026-07-11)
- [x] 15-05-PLAN.md — LIVE-RUN: provision VM + on-VM sensor build + Windows-DACL gate on the VM (DAEMON-02) (Wave 3) (completed 2026-07-11: VM `rdpilot-vm` UP and held for 15-06/07/08; sensor SHA-256 byte-verified; DAEMON-02 Windows half live-verified — owner-only DACL, anti-squatting, genuine cross-account rejection all PASS)
- [x] 15-06-PLAN.md — LIVE-RUN: orphan-liveness/e2e + CLI-02/03 live + MCP-04 live half, from the Linux host (Wave 4)
- [x] 15-07-PLAN.md — LIVE-RUN: PROOF-02 + PROOF-03 scripted proofs (no live LLM) (Wave 5)
- [x] 15-08-PLAN.md — LIVE-RUN: PROOF-04 capstone (`claude -p`) + teardown authorization checkpoint (Wave 6) (completed 2026-07-11). **Capstone PASS**: `claude -p` drove read/inspect + file-transfer through the real MCP surface against the live Azure VM, verified by transcript + independent side-effect (one live-diagnosed fix: `RDPILOT_SENSOR_BINARY_PATH` missing from the rendered `--mcp-config`, mirroring 15-06/15-07's finding). **Teardown developer-authorized and confirmed**: `manage-env.ps1 down` ran clean (exit 0), `az group exists -n rdpilot-test` => `false`, `rdpilot-mgmt` persists (`Succeeded`), no stray local daemon/sensor/claude processes or IPC socket left behind.

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|-----------------|--------|-----------|
| 1. Test Environment | v1.0 | 4/4 | Complete | 2026-06-05 |
| 2. RDP Session + Framebuffer Core | v1.0 | 3/3 | Complete | 2026-06-05 |
| 3. Input Injection | v1.0 | 4/4 | Complete | 2026-07-08 |
| 4. DVC Transport Channel | v1.0 | 3/3 | Complete | 2026-07-09 |
| 5. Sensor Bootstrap + Deployment | v1.0 | 4/4 | Complete | 2026-07-09 |
| 6. Window + Process Perception | v1.0 | 5/5 | Complete | 2026-07-09 |
| 7. UIA Tree Module | v1.0 | 5/5 | Complete | 2026-07-09 |
| 8. Public SDK API + WorldState | v1.0 | 3/3 | Complete | 2026-07-09 |
| 9. Scripted Proof Harness | v1.0 | 4/4 | Complete | 2026-07-10 |
| 10. SDK File-Transfer Extension | v1.1 | 5/5 | Complete   | 2026-07-10 |
| 11. Shared Wire Protocol & Config | v1.1 | 2/2 | Complete   | 2026-07-10 |
| 12. Session Daemon | v1.1 | 6/7 (12-07 superseded, live-verified via 15-01/05/06) | Complete | 2026-07-11 |
| 13. CLI Surface | v1.1 | 7/7 | Complete   | 2026-07-11 |
| 14. MCP Server Surface | v1.1 | 5/5 | Complete   | 2026-07-11 |
| 15. Proof Harnesses & Live-LLM Capstone | v1.1 | 8/8 | Complete   | 2026-07-11 |

## Backlog

### Phase 999.1: Harden live RDP test suite against frame-timing races (BACKLOG)

**Goal:** Replace fixed-time settles / blank-frame tolerances in the live integration suite with deterministic frame-readiness signals (poll-until-shell-ready, wait-for-first-real-paint), reducing flakiness like the screenshot_is_rgb_correct failure observed during Phase 2 live validation (fixed in commit 03302a0 as a band-aid).
**Requirements:** TBD
**Plans:** 3/3 plans complete

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.2: subagent tool grants — ensure file-producing agents have Write (BACKLOG)

**Goal:** Audit GSD subagent definitions so agents expected to produce files (researchers, planners, doc-writers, etc.) are granted Write/Edit tools. Observed failure: a researcher subagent burned ~6 turns trying to write a file via fs/bash/pwsh/python workarounds because it lacked Write().
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.3: Auto-surface the Azure live-test environment infra (local skill) (BACKLOG)

**Goal:** Make the existing live-test infrastructure DISCOVERABLE and self-surfacing so any agent proactively provisions it when a phase needs live RDP validation — instead of treating live testing as unavailable or forgetting the infra exists. Phase 1 already built the full stack: `infra/` Bicep (network/NSG/WS2022 VM + CustomScriptExtension), in-guest `Configure-Target.ps1` (WinRM HTTPS, 96 DPI, SuppressWhenMinimized, SHA-verified 7-Zip), `manage-env.ps1 up`/`down`, and a scheduled auto-destroy runbook. The gap is DISCOVERABILITY, not capability.

**Motivation:** During Phase 3 (Input Injection) execution, live validation (Wave 4 / 03-04) needs a running Windows target, but nothing in the executor's default context points at `manage-env.ps1 up`. Live-testing capability should announce itself at the moment of need.

**Proposed approach:** Author a local project skill (e.g. `.claude/skills/live-test-env/SKILL.md`) that documents: (a) the infra exists and where (`infra/`, `manage-env.ps1`), (b) how to spin it up/down (`manage-env.ps1 up` → connection details in `.secrets/connection.json`; `down` to tear down; auto-destroy as backstop), (c) the known gotchas (VM size `Standard_B2s_v2` in westeurope since `Standard_B2ms` is SkuNotAvailable; scoop rustup + MinGW gcc env for the Windows build host; RDPILOT_LIVE env gating for the gated suite), and (d) a trigger note so agents consult it whenever a phase's success criteria require a live remote Windows session. Cross-reference from CLAUDE.md if useful.

**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.4: Remote Assistance / Shadowing (BACKLOG)

**Goal:** We should support session shadowing (with and without control) so that an agent can offer assistance to a user in need.
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.5: Symmetric remote no-clobber for `put` (BACKLOG)

**Goal:** Make `rdpilot put`'s no-clobber symmetric with `get`. Phase 13 (CLI-03) enforces no-clobber fully for `get` (local destination `exists()` check + `--force`), but `put`'s destination is REMOTE and its overwrite-refusal is a documented known gap — the CLI/daemon cannot cheaply check remote existence without a new sensor round trip, and the Phase 10 C# `FileTransfer` Upload handler has no overwrite-refusal. Extend the C# sensor's Upload handler with a `no_clobber` flag + `File.Exists` check-then-refuse (honoring `--force`), thread the flag through the wire `Request::Put` and dispatch, and add a live gate proving a remote overwrite is refused without `--force` and allowed with it.

**Motivation:** During Phase 13 planning the developer accepted the asymmetry deliberately (sensor is out of Phase 13 scope); this backlog item tracks the follow-up so the asymmetry is closed, not silently permanent.

**Requirements:** TBD (extends CLI-03 / FILE-01)
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.6: Web viewer for daemon sessions (BACKLOG)

**Goal:** Now that Phase 12 introduces a session daemon with a session registry, provide a web viewer so humans can see what agents/sessions are doing in real time, and enable human intervention (takeover/control) when needed.
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.8: Dogfood rdpilot locally — SKILL.md on the CLI + install for other agents in this box (BACKLOG)

**Goal:** Make rdpilot available in this box for other agents so we can dogfood it — author a `SKILL.md` documenting the `rdpilot` CLI (commands, connection setup, common workflows) and install the CLI so other agents on this machine can discover and invoke it.
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.7: Session recording with timestamps/annotations (BACKLOG)

**Goal:** Support recording sessions (relates to the daemon/session lifecycle from Phase 12), capturing timestamps and annotations, with a config option to enable/disable (default-configurable).
**Requirements:** TBD
**Plans:** 0 plans

Plans:

- [ ] TBD (promote with /gsd-review-backlog when ready)

### Phase 999.9: Expose WinRM / PSRemoting (or similar) tunneled over the RDP channel — headless system access for agents (BACKLOG)

**Goal:** [Captured for future planning]
**Requirements:** TBD
**Plans:** 0 plans

Plans:
- [ ] TBD (promote with /gsd-review-backlog when ready)
