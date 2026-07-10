# Phase 5: Sensor Bootstrap + Deployment - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-07-09
**Phase:** 05-sensor-bootstrap-deployment
**Areas discussed:** In-session launch mechanism, NativeAOT build config, AV/EDR + drive-redirection-GPO fallback, C# DVC-open + ping readiness

---

## In-session launch mechanism

| Option | Description | Selected |
|--------|-------------|----------|
| Injected Win+R keystroke + typed run-command | Pure in-band, WinRM-independent; reuses Phase-3 input injection | ✓ |
| One-time WinRM-registered task | Adapt the Phase-4 Scheduled-Task approach for the RDPDR path | |
| RDPDR-copy + WinRM-launch hybrid | Copy via RDPDR, launch via WinRM | |

**User's choice:** Injected Win+R keystroke + typed run-command.
**Notes:** Matches D-4.5/agent-rdp intent and SC2's literal "launches within the RDP session." Recorded as D-5.1.

**Follow-up: launch robustness**

| Option | Description | Selected |
|--------|-------------|----------|
| Poll-and-retry | Inject launch, poll DVC with pings, re-inject up to N times on no pong | ✓ |
| Run-from-redirected-path | Execute directly from the redirected drive without local copy | |
| Copy-to-local-then-launch | Copy to local disk first, then launch from local path | |

**User's choice:** Poll-and-retry.
**Notes:** SC4's "ping/pong within 1 second of launch" is measured from a successful launch / first pong, not from the first keystroke attempt. Recorded as D-5.2.

---

## NativeAOT build config

| Option | Description | Selected |
|--------|-------------|----------|
| Pure NativeAOT now | PublishAot=true, self-contained, no external runtime | ✓ |
| Trimmed self-contained non-AOT | Self-contained but not fully AOT-compiled | |
| AOT-with-fallback | AOT with a non-AOT fallback build path | |

**User's choice:** Pure NativeAOT now.
**Notes:** SENSOR-01 literal wording requires no external runtime dependency. Binary size (5-30+MB, per STATE.md) to be benchmarked during the phase. Recorded as D-5.3.

**Follow-up: COM-under-AOT risk**

| Option | Description | Selected |
|--------|-------------|----------|
| Small proactive Phase-5 spike | Investigate COM/IUIAutomation compatibility under NativeAOT now | |
| Document as Phase-7 watch-item | Defer the compatibility question to Phase 7 as an entry-condition/risk | ✓ |

**User's choice:** Document as Phase-7 watch-item.
**Notes:** Phase 5 sensor stays Version/Ping-only, no COM usage — keeps Phase 5 lean and on-scope. Recorded as D-5.4.

---

## AV/EDR + drive-redirection-GPO fallback

| Option | Description | Selected |
|--------|-------------|----------|
| Empirical mitigate-if-fails | No proactive AV exclusion or signing; add exclusion only if live gate surfaces a block | ✓ |
| Pre-add AV exclusion | Add the exclusion proactively before the live gate | |
| Pursue code-signing now | Sign the sensor binary before deployment | |

**User's choice:** Empirical mitigate-if-fails.
**Notes:** Matches the Phase 3-4 "tune against the real VM" pattern. Fallback mitigation if AV/EDR blocks: add exclusion for sensor exe/path in deploy step / Configure-Target.ps1. Ref PITFALLS.md Pitfall C5. Recorded as D-5.5.

**Follow-up: GPO / success-bar question**

| Option | Description | Selected |
|--------|-------------|----------|
| WinRM-fallback conditional pass | Allow phase completion if WinRM fallback works even if RDPDR fails | |
| RDPDR success mandatory | RDPDR-path success is required for phase completion; no conditional pass | ✓ |

**User's choice:** RDPDR success mandatory.
**Notes:** The controlled lab VM is configured to allow drive-redirection. WinRM fallback (SC3) remains independently required by success criteria. Recorded as D-5.6.

---

## C# DVC-open + ping readiness

| Option | Description | Selected |
|--------|-------------|----------|
| Port the PowerShell fixture | Transliterate tests/fixtures/sensor-responder.ps1 into C# | |
| Re-derive idiomatically in C# | Write a fresh DllImport-based WTS implementation in idiomatic C# | ✓ |

**User's choice:** Re-derive idiomatically.
**Notes:** Recorded as D-5.7 base.

**Follow-up: Phase-4 gotchas**

| Option | Description | Selected |
|--------|-------------|----------|
| Mandatory constraints | The three empirically-proven Phase-4 findings (channel-open retry-poll, session targeting, WTS-read framing) are hard constraints on the re-derivation | ✓ |
| Clean-slate rediscover | Re-derive without carrying forward the Phase-4 findings, rediscover any issues live | |

**User's choice:** Mandatory constraints.
**Notes:** Three hard constraints: (1) retry-poll the 0x31/ERROR_GEN_FAILURE channel-open timing race; (2) target the correct interactive session; (3) handle WTS-read binary-prefix framing (scan for first `{` byte). tests/fixtures/sensor-responder.ps1 kept as reference until superseded, then deleted along with deploy-responder.ps1. Recorded as D-5.7.

---

## Claude's Discretion

None — all four gray areas presented were resolved to explicit user decisions.

## Deferred Ideas

- Code-signing / Azure Trusted Signing for the sensor binary — deferred out of v1 scope (v1 targets a controlled lab VM; revisit if/when non-lab targets are in scope).
