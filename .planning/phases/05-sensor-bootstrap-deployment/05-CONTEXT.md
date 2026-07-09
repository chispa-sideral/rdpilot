# Phase 5: Sensor Bootstrap + Deployment - Context

**Gathered:** 2026-07-09
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 5 builds the real `rdpilot-sensor.exe` — a C# .NET 8 NativeAOT self-contained executable that speaks the already-built `RDPILOT_SENSOR` DVC envelope protocol (Version/Ping/Pong only, per Phase 4) — and gets it onto a real remote Windows target and running inside the interactive RDP session, replacing Phase 4's throwaway PowerShell WTS responder. Covers SENSOR-01 (the binary, no external runtime) and SENSOR-02 (deploy + launch: RDPDR drive-redirection primary, WinRM fallback), proven live by a ping/pong round-trip within 1 second of launch. It explicitly does NOT implement real perception payloads (window list, process tree, UIA — Phases 6-7) beyond the existing handshake/ping contract.

</domain>

<decisions>
## Implementation Decisions

### In-session launch mechanism
- **D-5.1:** RDPDR (drive-redirection) PRIMARY path launches the copied rdpilot-sensor.exe via injected Win+R keystroke + typed run-command — pure in-band, WinRM-independent (reuses Phase-3 input injection). Matches D-4.5/agent-rdp intent and SC2's literal "launches within the RDP session."
- **D-5.2:** Launch reliability = poll-and-retry. After injecting the launch sequence, poll the DVC with pings; if no pong within a bounded window, re-inject the launch up to N times before failing. SC4's "ping/pong within 1 second of launch" is measured from a SUCCESSFUL launch / first pong, NOT from the first keystroke attempt.

### NativeAOT build config
- **D-5.3:** Build = pure NativeAOT (PublishAot=true), self-contained, no external runtime dependency (SENSOR-01 literal wording). Binary size (STATE flags 5-30+MB unknown) to be benchmarked during the phase; smallest footprint / fastest cold start also serves SC4's 1s budget.
- **D-5.4:** Phase 5 sensor stays Version/Ping-only — NO COM usage. The COM-under-NativeAOT compatibility question (needed for Phase 7 UIA/IUIAutomation) is recorded as an explicit Phase-7 entry-condition/risk, NOT a Phase-5 spike. Keeps Phase 5 lean and on-scope.

### AV/EDR + drive-redirection-GPO fallback
- **D-5.5:** AV/EDR handled empirically (matches the Phase 3-4 "tune against the real VM" pattern) — no proactive code-signing or AV exclusion. IF the live gate surfaces an actual AV/EDR block, the fallback mitigation is to add an AV exclusion for the sensor exe/path in the deploy step / Configure-Target.ps1. Code-signing / Azure Trusted Signing is deferred out of v1 (see Deferred Ideas). Ref: PITFALLS.md Pitfall C5.
- **D-5.6:** RDPDR-path success is MANDATORY for phase completion (SC2) — the controlled lab VM is configured to allow drive-redirection; there is NO "conditional pass" that waives RDPDR just because the WinRM fallback works. The WinRM fallback path (SC3) remains independently required by the success criteria.

### C# DVC-open + ping readiness
- **D-5.7:** The C# server-side DVC-open (WTS P/Invoke) is re-derived idiomatically in C# (DllImport), NOT a transliteration of the PowerShell fixture. However it MUST honor three empirically-proven Phase-4 findings as HARD constraints: (1) retry-poll the 0x31 / ERROR_GEN_FAILURE channel-open timing race; (2) target the correct interactive session; (3) handle the WTS-read binary-prefix framing (scan for the first `{` byte rather than assuming JSON starts at offset 0). Keep tests/fixtures/sensor-responder.ps1 as a reference until the C# sensor supersedes it, then delete it (and deploy-responder.ps1) as throwaway Phase-4 assets.

### Carried Forward (inherited, already settled — not re-decided this session)
- Sensor language = C# .NET 8 NativeAOT (locked: STATE/REQUIREMENTS/ROADMAP; supersedes stale PROJECT.md "deferred").
- Deployment priority order = RDPDR drive-redirection PRIMARY → WinRM FALLBACK → blob+SAS TERTIARY (Phase 4 D-4.5; overrides the older ARCHITECTURE.md WinRM-first ordering, now stale on this point).
- CLIPRDR clipboard file-copy delivery = explicitly REJECTED (D-4.5), not deferred — do not re-propose.
- Wire protocol FIXED: RDPILOT_SENSOR DVC channel const (connect.rs:44); {version, req_id, type, payload} JSON envelope (D-4.3, sensor.rs); PROTOCOL_VERSION=1; MsgType Version/Ping/Pong. The C# sensor implements the SERVER side of this exact protocol — not a new one.
- .secrets/connection.json schema fixed (host/user/password/rdpPort/winrmPort); never printed/logged.
- No unwrap/expect/panic in library code; owned-SDK-types-only public API (D-09) — applies to any new Rust-side deploy/bootstrap code.
- Gated live-test pattern (RDPILOT_LIVE env gate, #[ignore], require_target!, tests/common/mod.rs) is the established live-verification mechanism — Phase 5's live gate reuses it.

### Claude's Discretion
None flagged this session — all four gray areas presented were resolved to explicit user decisions above.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope and requirements
- `.planning/ROADMAP.md` (Phase 5 section: goal, 4 success criteria, SENSOR-01/02 mapping)
- `.planning/REQUIREMENTS.md` (SENSOR-01, SENSOR-02 full text; stack line)
- `.planning/PROJECT.md` (Key Decisions table — NOTE: stale on sensor-language "deferred" status, superseded by STATE.md)
- `.planning/STATE.md` (Accumulated Context decisions; Blockers/Concerns: AV/EDR unknown, drive-redirection GPO unknown, NativeAOT binary size 5-30+MB unknown)

### Architecture and research
- `.planning/research/ARCHITECTURE.md` (Component 6 rdpilot-sensor; Remote Helper Bootstrap Options A/B/C/D — READ WITH the D-4.5 override in mind; WinRM-first ordering here is stale)
- `.planning/research/FEATURES.md` (TS-10 Sensor Helper Bootstrap + Transport)
- `.planning/research/PITFALLS.md` (Pitfall C5 AV/EDR flagging the sensor helper)

### Phase 4 prior art
- `.planning/phases/04-dvc-transport-channel/04-CONTEXT.md` (D-4.1..D-4.5, esp. D-4.5 delivery-mechanism writeup + D-4.2 sensor-language deferral)
- `.planning/phases/04-dvc-transport-channel/04-RESEARCH.md` (DVC/WTS mechanics, envelope wire format)
- `.planning/phases/04-dvc-transport-channel/04-03-PLAN.md` and `04-03-SUMMARY.md` (proven Scheduled-Task launch; AtLogOn latency + reconnect gotcha; live-run bug fixes)

### Rust-side integration points
- `crates/rdpilot/src/sensor.rs`, `crates/rdpilot/src/connect.rs`, `crates/rdpilot/src/session.rs`, `crates/rdpilot/src/session_loop.rs` (Rust-side DVC/session plumbing the C# sensor interoperates with)
- `crates/rdpilot/tests/fixtures/sensor-responder.ps1`, `crates/rdpilot/tests/fixtures/deploy-responder.ps1` (throwaway Phase-4 assets to be replaced/deleted in Phase 5; sensor-responder.ps1 is the WTS reference)
- `crates/rdpilot/Cargo.toml` (confirms ironrdp-rdpdr is NOT yet a dependency — must be added if the RDPDR path needs it)

### Live-test environment
- `infra/manage-env.ps1` and `infra/` Bicep (live-test environment the Phase 5 live gate provisions against)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- DVC wire protocol already built and live-proven (sensor.rs, connect.rs): PROTOCOL_VERSION=1, Envelope{version,req_id,type,payload}, MsgType Version/Ping/Pong. Phase 5's C# sensor is a from-scratch SERVER-side implementation of this protocol; no .csproj exists anywhere in the repo yet (fully greenfield C# project).
- Empirically-proven WinRM launch mechanism (deploy-responder.ps1): New-PSSession (SkipCACheck/SkipCNCheck, UseSSL, Negotiate) → Copy-Item -ToSession → Register-ScheduledTask (New-ScheduledTaskTrigger -AtLogOn -User, New-ScheduledTaskPrincipal -LogonType Interactive) → immediate Start-ScheduledTask. Live-validated: AtLogOn fires ~20-30s after fresh logon, does NOT refire on RDP reconnect — affects SC4 launch-detection timing (measure from actual process start, not task-registration).
- Existing Error enum pattern (error.rs) and gated live-test harness (tests/common/mod.rs, tests/live_session.rs) are the extension points for new Phase 5 Rust-side deploy/bootstrap code and its live gate.

### Established Patterns
- Gated live-test pattern (RDPILOT_LIVE env gate, #[ignore], require_target!, tests/common/mod.rs) is the established live-verification mechanism — Phase 5's live gate reuses it.
- No unwrap/expect/panic in library code; owned-SDK-types-only public API (D-09) — applies to any new Rust-side deploy/bootstrap code.

### Integration Points
- RDPDR unimplemented: ironrdp-rdpdr absent from Cargo.toml; the D-4.5 primary path (drive-redirection copy + injected Win+R launch) is net-new Rust work in Phase 5, unlike the WinRM path which has a directly adaptable Phase-4 precedent.
- New C# sensor project connects to existing Rust SDK only via the wire protocol over the DVC — no direct code-level coupling.

</code_context>

<specifics>
## Specific Ideas

No specific requirements beyond the four decision areas above — open to standard approaches for implementation details not otherwise locked.

</specifics>

<deferred>
## Deferred Ideas

- Code-signing / Azure Trusted Signing for the sensor binary, to make it robust against AV/EDR on less-controlled future targets — out of v1 scope (v1 targets a controlled lab VM; per PROJECT.md "personal tooling first"). Revisit if/when non-lab targets are in scope.

</deferred>

---

*Phase: 5-Sensor Bootstrap + Deployment*
*Context gathered: 2026-07-09*
