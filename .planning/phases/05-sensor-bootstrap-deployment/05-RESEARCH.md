# Phase 5: Sensor Bootstrap + Deployment - Research

**Researched:** 2026-07-09
**Domain:** C# .NET 8 NativeAOT server-side DVC sensor + RDPDR client-side drive redirection (IronRDP) + WinRM fallback deployment
**Confidence:** HIGH for the verified, source-read findings (IronRDP RDPDR crate internals, .NET NativeAOT interop rules); MEDIUM for empirical unknowns explicitly deferred to the live gate (binary size, AV/EDR, GPO)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**In-session launch mechanism**
- **D-5.1:** RDPDR (drive-redirection) PRIMARY path launches the copied rdpilot-sensor.exe via injected Win+R keystroke + typed run-command — pure in-band, WinRM-independent (reuses Phase-3 input injection). Matches D-4.5/agent-rdp intent and SC2's literal "launches within the RDP session."
- **D-5.2:** Launch reliability = poll-and-retry. After injecting the launch sequence, poll the DVC with pings; if no pong within a bounded window, re-inject the launch up to N times before failing. SC4's "ping/pong within 1 second of launch" is measured from a SUCCESSFUL launch / first pong, NOT from the first keystroke attempt.

**NativeAOT build config**
- **D-5.3:** Build = pure NativeAOT (PublishAot=true), self-contained, no external runtime dependency (SENSOR-01 literal wording). Binary size (STATE flags 5-30+MB unknown) to be benchmarked during the phase; smallest footprint / fastest cold start also serves SC4's 1s budget.
- **D-5.4:** Phase 5 sensor stays Version/Ping-only — NO COM usage. The COM-under-NativeAOT compatibility question (needed for Phase 7 UIA/IUIAutomation) is recorded as an explicit Phase-7 entry-condition/risk, NOT a Phase-5 spike. Keeps Phase 5 lean and on-scope.

**AV/EDR + drive-redirection-GPO fallback**
- **D-5.5:** AV/EDR handled empirically (matches the Phase 3-4 "tune against the real VM" pattern) — no proactive code-signing or AV exclusion. IF the live gate surfaces an actual AV/EDR block, the fallback mitigation is to add an AV exclusion for the sensor exe/path in the deploy step / Configure-Target.ps1. Code-signing / Azure Trusted Signing is deferred out of v1 (see Deferred Ideas). Ref: PITFALLS.md Pitfall C5.
- **D-5.6:** RDPDR-path success is MANDATORY for phase completion (SC2) — the controlled lab VM is configured to allow drive-redirection; there is NO "conditional pass" that waives RDPDR just because the WinRM fallback works. The WinRM fallback path (SC3) remains independently required by the success criteria.

**C# DVC-open + ping readiness**
- **D-5.7:** The C# server-side DVC-open (WTS P/Invoke) is re-derived idiomatically in C# (DllImport family), NOT a transliteration of the PowerShell fixture. However it MUST honor three empirically-proven Phase-4 findings as HARD constraints: (1) retry-poll the 0x31 / ERROR_GEN_FAILURE channel-open timing race; (2) target the correct interactive session; (3) handle the WTS-read binary-prefix framing (scan for the first `{` byte rather than assuming JSON starts at offset 0). Keep tests/fixtures/sensor-responder.ps1 as a reference until the C# sensor supersedes it, then delete it (and deploy-responder.ps1) as throwaway Phase-4 assets.

### Carried Forward (inherited, already settled — not re-decided this session)
- Sensor language = C# .NET 8 NativeAOT (locked: STATE/REQUIREMENTS/ROADMAP; supersedes stale PROJECT.md "deferred").
- Deployment priority order = RDPDR drive-redirection PRIMARY → WinRM FALLBACK → blob+SAS TERTIARY (Phase 4 D-4.5; overrides the older ARCHITECTURE.md WinRM-first ordering, now stale on this point).
- CLIPRDR clipboard file-copy delivery = explicitly REJECTED (D-4.5), not deferred — do not re-propose.
- Wire protocol FIXED: RDPILOT_SENSOR DVC channel const (connect.rs:47); `{version, req_id, type, payload}` JSON envelope (D-4.3, sensor.rs); PROTOCOL_VERSION=1; MsgType Version/Ping/Pong. The C# sensor implements the SERVER side of this exact protocol — not a new one.
- `.secrets/connection.json` schema fixed (host/user/password/rdpPort/winrmPort); never printed/logged.
- No unwrap/expect/panic in library code; owned-SDK-types-only public API (D-09) — applies to any new Rust-side deploy/bootstrap code.
- Gated live-test pattern (RDPILOT_LIVE env gate, `#[ignore]`, `require_target!`, `tests/common/mod.rs`) is the established live-verification mechanism — Phase 5's live gate reuses it.

### Claude's Discretion
None flagged this session — all four gray areas presented were resolved to explicit user decisions above.

### Deferred Ideas (OUT OF SCOPE)
- Code-signing / Azure Trusted Signing for the sensor binary — out of v1 scope (v1 targets a controlled lab VM). Revisit if/when non-lab targets are in scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SENSOR-01 | "A thin C# .NET 8 NativeAOT sensor helper exposes the structured-perception queries on the target" | Standard Stack / Code Examples give the exact `.csproj` NativeAOT config, the `LibraryImport`-based WTS P/Invoke (NOT `DllImport` — AOT-incompatible, see Pitfall 1), and the `JsonSerializerContext` source-generator requirement for the fixed envelope. |
| SENSOR-02 | "The SDK can bootstrap/deploy and launch the sensor on the target (drive-redirection copy primary, WinRM fallback)" | Architecture Patterns / Don't Hand-Roll give the `ironrdp-rdpdr` wiring (a **static** channel, not a DVC — genuinely net-new registration alongside `DrdynvcClient`), the custom `RdpdrBackend` the SDK must implement (no ready-made cross-platform backend ships upstream — verified from source), the Win+R injection sequence (needs a new `Key::Win` — currently absent from `input.rs`), and the WinRM fallback adapted from Phase 4's proven `deploy-responder.ps1`. |
</phase_requirements>

## Summary

Phase 5 has two genuinely separate engineering surfaces that must each be proven live: a **new C# executable** (`rdpilot-sensor.exe`) implementing the server side of Phase 4's already-fixed DVC envelope, and **new Rust-side deployment plumbing** on the client (RDPDR primary, WinRM fallback) to get that executable onto the target and running inside the interactive session. Both surfaces have one load-bearing finding each that overturns a plausible-but-wrong default assumption, verified directly against upstream source rather than training-data recall:

1. **C# side — `[DllImport]` is not Native-AOT-safe.** Microsoft's own docs are explicit: "Using `DllImport` isn't an option for platforms that require full Native AOT scenarios" because its string/array marshalling relies on a runtime-generated IL stub that AOT cannot produce. `WTSVirtualChannelOpenEx` takes a `string` channel name — exactly the case that trips this. The C# sensor's WTS P/Invoke block must use `[LibraryImport]` (source-generated marshalling, C# 9+/.NET 7+) with explicit `StringMarshalling`, not a straight port of the PowerShell `Add-Type` P/Invoke signatures from Phase 4's `sensor-responder.ps1` (D-5.7 already anticipates re-deriving idiomatically, not transliterating — this is the concrete reason why).

2. **Rust side — `ironrdp-rdpdr` ships zero filesystem backend for Windows (or any OS other than macOS/Linux).** `ironrdp-rdpdr` (0.6.0) is a **static virtual channel** (`SvcProcessor`, registered via `connector.with_static_channel(...)` — the *same* registration site as `DrdynvcClient`, but a sibling channel, not routed through it) that only implements MS-RDPEFS protocol framing. All actual file I/O is delegated to a caller-supplied `Box<dyn RdpdrBackend>`. The only backend IronRDP ships (`ironrdp-rdpdr-native`) is `#[cfg(any(target_os = "macos", target_os = "linux"))]`-gated and compiles to an **empty crate** on Windows. 04-CONTEXT.md's carried-forward note ("the `-native` backend being \*nix-only is a non-issue because our client runs on Linux") is only true for this session's Linux build/test sandbox — it does NOT hold for this repo's own canonical `x86_64-pc-windows-gnu` toolchain target (`rust-toolchain.toml`, Phase 2 architectural decision). Regardless of which target ends up building this phase's live gate, the SDK needs its **own minimal, portable `RdpdrBackend`** written against `std::fs` (portable, works on both targets) — not a dependency on `ironrdp-rdpdr-native`. This is bounded work: `ServerDriveIoRequest` (the only inbound IRP enum the backend must handle) has exactly four variants — `Create`, `Close`, `Read`, `QueryDirectory` — because Phase 5 only needs to serve one read-only file for a copy operation, not a general filesystem.

A third, smaller but load-bearing gap: **the current `Key` vocabulary (`crates/rdpilot/src/input.rs`) has no Windows/GUI key.** D-5.1's Win+R launch sequence cannot be expressed with `Ctrl`/`Alt`/`Shift` alone — `Key::Win` (Set 1 scancode `E0 5B`, extended) must be added, following the exact pattern of every other entry in the existing `scancode()` match.

**Primary recommendation:** Treat this phase as three parallel-but-integrated tracks: (A) the C# sensor project (new top-level `sensor/` directory, `LibraryImport`-based WTS P/Invoke, source-generated JSON, `PublishAot=true` self-contained `win-x64` publish), (B) the Rust RDPDR client path (`ironrdp-rdpdr` + a new hand-written minimal `RdpilotDriveBackend`, registered as a static channel in `connect.rs` alongside the existing `DrdynvcClient`, plus a new `Key::Win` and a `Session`-level launch-and-poll helper reusing `send_key`), and (C) the WinRM fallback (near-verbatim adaptation of Phase 4's proven `deploy-responder.ps1`, swapping the throwaway `.ps1` responder for a `Copy-Item` of the real `.exe` plus the same AtLogOn-scheduled-task launch mechanism). Binary size, AV/EDR behavior, and the drive-redirection GPO are explicitly empirical unknowns the live gate resolves, per D-5.3/D-5.5 — do not attempt to pre-solve them at planning time.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Sensor executable build (NativeAOT publish) | Remote target (C# project, built for `win-x64`) | Local build host (must itself run on Windows — NativeAOT does not cross-OS-compile, see Pitfall 4) | The published artifact runs on the remote Windows target; but the *build* of that artifact is an OS-constrained local/CI concern, not a runtime one. |
| Server-side DVC open + Version/Ping/Pong | Remote target (C# sensor, WTS P/Invoke) | — | Must run inside the interactive RDP session (Session > 0) — an OS constraint carried forward unchanged from Phase 4 (D-5.7). |
| RDPDR client-side channel + drive backend | SDK / RDP Session Manager (local, new `connect.rs` static-channel registration + new `RdpilotDriveBackend`) | — | `ironrdp-rdpdr`'s `RdpdrBackend` trait requires the *client* to implement real file I/O; this is genuinely local-tier code, symmetrical to how `RdpilotSensorProcessor` is local-tier DVC code (Phase 4). |
| Win+R injected launch sequence | SDK / Input Injector (local, `Session::send_key` + new `Key::Win`) | — | Reuses the existing Phase 3 input-injection seam exactly — no new transport, only a new key mapping and a call-site sequence. |
| Launch-reliability poll/retry loop | SDK / Session or a new thin `Bootstrap` helper (local) | — | Mirrors D-5.2's explicit design: this is orchestration logic over the existing `Session::ping()` (Phase 4) and `Session::send_key()` (Phase 3) — no new transport primitive needed. |
| WinRM fallback deploy + launch | Local tooling (`.ps1`, adapted from `deploy-responder.ps1`) → Remote target (Scheduled Task, AtLogOn, Interactive principal) | — | Identical shape to Phase 4's already-live-proven mechanism; only the payload (real `.exe` vs. throwaway `.ps1`) and the launch action change. |
| AV/EDR / drive-redirection GPO resolution | Remote target (empirical, live gate) | Local `Configure-Target.ps1` (only if a live-gate failure demands an AV exclusion, D-5.5) | Explicitly NOT solved at planning/research time (D-5.5) — this row exists only to record where the empirical finding will land if needed. |

## Project Constraints (from CLAUDE.md)

- **Sensor language is locked:** C# .NET 8 NativeAOT — do not substitute Rust for the remote helper even though it would avoid a second language/toolchain (this was evaluated and explicitly rejected in the original stack research: "the `windows-rs` crate's UIA bindings are lower-level and less ergonomic... adds complexity for uncertain benefit in a dumb sensor").
- **RDP library is locked:** IronRDP 0.15 umbrella (member crates at independent versions, already pinned in `Cargo.lock`) — `ironrdp-rdpdr` joins the existing `ironrdp-dvc`/`ironrdp-input`/`ironrdp-tls` set.
- **TLS backend is locked:** rustls (already selected, unaffected by this phase).
- **No unwrap/expect/panic in library code; owned-SDK-types-only public API (D-09)** — applies to all new Rust-side `RdpilotDriveBackend`/bootstrap code exactly as it applied to `RdpilotSensorProcessor` in Phase 4. No `ironrdp-rdpdr` type may leak into `rdpilot`'s public surface.
- **GSD workflow enforcement:** file-changing work in this repo must go through a GSD entry point (`/gsd-execute-phase` etc.) — not itself a technical constraint on the sensor/RDPDR design, but noted for the planner's task-authoring context.
- **Credentials/secrets never logged** — the WinRM fallback path (adapted `deploy-responder.ps1`) must preserve the exact never-echo-password discipline already proven in Phase 4's fixture.
- **`.secrets/connection.json` is the only credential location** — no new secret storage location for this phase.

## Standard Stack

### Core — Remote sensor (C#)

| Component | Version | Purpose | Why |
|-----------|---------|---------|-----|
| .NET SDK | 8.0 (LTS) [CITED: learn.microsoft.com/dotnet/core/deploying/native-aot] | NativeAOT publish toolchain | SENSOR-01 literal wording; .NET 8 is the first version with full NativeAOT console-app support carried from .NET 7 plus expanded trimming/JSON source-gen ergonomics |
| `PublishAot=true` + `SelfContained=true` (project property, not a package) | .NET 8 SDK built-in | Produces a single native `.exe`, no shared CLR install needed on the target | D-5.3, SENSOR-01 |
| `System.Text.Json` source generator (`JsonSerializerContext`) | Built into .NET 8 SDK (no NuGet package needed) | AOT-safe (de)serialization of the fixed `{version, req_id, type, payload}` envelope | Reflection-based JSON serialization is NativeAOT-incompatible by default in .NET 8 (`JsonSerializerIsReflectionEnabledByDefault` exists specifically to catch accidental reflection use); the source generator is the supported path [CITED: learn.microsoft.com — System.Text.Json reflection-vs-source-generation] |
| `[LibraryImport]` (source-generated P/Invoke, C# 9+/.NET 7+) | Built into .NET SDK | WTS P/Invoke (`WTSVirtualChannelOpenEx`/`Read`/`Write`/`Close`) | `[DllImport]` relies on a runtime-JIT'd IL marshalling stub that Native AOT cannot generate — explicitly documented as AOT-incompatible for exactly the string-marshalling case `WTSVirtualChannelOpenEx` needs [CITED: learn.microsoft.com/dotnet/standard/native-interop/pinvoke-source-generation] |

**No new NuGet packages are required** — everything above ships in the .NET 8 SDK itself. Package Legitimacy Audit is therefore N/A for the C# side (see below).

### Core — Local SDK (Rust)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ironrdp-rdpdr` | 0.6.0 [VERIFIED: crates.io / GitHub source read directly, 2026-07-09] | MS-RDPEFS static-channel protocol framing (`Rdpdr` struct, `with_drives`/`add_drive`, `RdpdrBackend` trait) | The only IronRDP crate implementing client-side drive redirection; not yet in `Cargo.toml` — net-new dependency this phase, confirmed absent by reading `crates/rdpilot/Cargo.toml` |
| `ironrdp-svc` | 0.7.x (transitive, pulled in by `ironrdp-rdpdr`) | `SvcProcessor`/`SvcMessage` traits `Rdpdr` implements — same static-channel registration mechanism the umbrella `svc` feature already enables in `Cargo.toml` | Already enabled (`ironrdp = { features = [..., "svc"] }`) — no new umbrella feature needed, only the new direct `ironrdp-rdpdr` dependency |

**Do NOT add `ironrdp-rdpdr-native`.** It is `#[cfg(any(target_os = "macos", target_os = "linux"))]`-gated in its own `lib.rs` (verified by reading the crate source directly) and its `Cargo.toml` only pulls in the `nix` crate under that same cfg — on a Windows build target it resolves to an empty module with nothing exported. Even the official `ironrdp-client` reference implementation only wires `NoopRdpdrBackend` by default (verified: `crates/ironrdp-client/src/rdp.rs`) — there is no upstream "just works" filesystem backend to depend on for this project's canonical target. Write a small custom `RdpilotDriveBackend` (`std::fs`-based, portable) instead — see Code Examples.

### Version verification

```text
ironrdp-rdpdr = "0.6"   # matches the already-pinned ironrdp-dvc/ironrdp-input/ironrdp-pdu "0.6"/"0.8" line in Cargo.toml — same release cadence, verified via GitHub Cargo.toml read directly (2026-07-09), not cargo/crates.io API (no network egress to crates.io confirmed available in this research session; version cross-checked against ironrdp-svc's "0.7" dependency line inside ironrdp-rdpdr's own Cargo.toml, which matches this repo's already-resolved ironrdp-svc via the umbrella `svc` feature)
```

**Installation (Rust side):**
```toml
# Cargo.toml [dependencies] — add alongside the existing ironrdp-dvc/ironrdp-input lines
ironrdp-rdpdr = "0.6"
```

**Installation (C# side):** no `dotnet add package` — the `.csproj` needs only property changes (see Code Examples), not a package reference.

## Package Legitimacy Audit

**Rust side:** `ironrdp-rdpdr` is published by the same `Devolutions` org (crates.io/GitHub) as every other `ironrdp-*` crate already pinned in this repo's `Cargo.lock` — same publisher, same release cadence, same repository. Not independently slopcheck-verified in this session (no `pip`/network execution performed for a single first-party crate from an already-trusted publisher whose sibling crates are already in the dependency tree), but flagged `[ASSUMED]` per the graceful-degradation rule below since `npm view`/`cargo search`-equivalent registry confirmation was not run in this sandbox (no `cargo` binary present — verified via `command -v cargo` returning nothing).

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|-------------|
| `ironrdp-rdpdr` | crates.io | Same repo/release train as `ironrdp-dvc`/`ironrdp-input` (already trusted, pinned in `Cargo.lock`) | Not queried (no `cargo`/network registry tool in this sandbox) | `github.com/Devolutions/IronRDP` (verified by reading source directly) | Not run — `[ASSUMED]`, planner must gate the `Cargo.toml` edit behind a `checkpoint:human-verify` per graceful degradation | Approved pending human `cargo add`/`cargo build` verification at execute time |

**Packages removed due to slopcheck `[SLOP]` verdict:** none — no run performed (see above); no candidate looked suspicious given it's a sibling crate in an already-vetted, already-pinned monorepo.
**Packages flagged as suspicious `[SUS]`:** none.
**C# side:** no new NuGet packages at all — N/A.

## Architecture Patterns

### System Architecture Diagram

```
LOCAL (rdpilot SDK, Rust)                                    REMOTE (Windows target)
──────────────────────────                                    ────────────────────────
connect.rs (connect-time, once):
  DrdynvcClient::with_dynamic_channel(RdpilotSensorProcessor)  ← unchanged from Phase 4
  Rdpdr::new(RdpilotDriveBackend, "rdpilot")                   ← NEW static channel,
    .with_drives(Some([(0, "RDPILOT")]))                          registered alongside
  connector.with_static_channel(drdynvc)                           DrdynvcClient at the
  connector.with_static_channel(rdpdr_channel)                     SAME "before connect_
                                                                     begin" seam (SC#1-
                                                                     style hard constraint)
        │
        ▼  RDP session establishes; \\tsclient\RDPILOT\ becomes browsable server-side

Bootstrap sequence (new, this phase):
  1. RdpilotDriveBackend serves rdpilot-sensor.exe bytes on demand
     (Create/QueryDirectory/Read IRPs from the remote Explorer/cmd.exe)
                                                                 │
  2. Session::send_key(Win) + typed "cmd /c copy               │
     \\tsclient\RDPILOT\rdpilot-sensor.exe                      │
     %TEMP%\rdpilot-sensor.exe && start ..." + Enter             ▼
     (D-5.1, reuses Phase 3 send_key)                    Win+R dialog opens,
        │                                                  copies + launches
        │                                                  rdpilot-sensor.exe
        ▼                                                       │
  3. Poll Session::ping() every ~500ms, up to N retries          │
     (D-5.2) — re-inject the Win+R sequence if the                │
     window expires with no pong                                  ▼
        │                                                rdpilot-sensor.exe:
        │                                                  Program.Main() →
        │                                                  WTSVirtualChannelOpenEx
        │                                                  (retry-poll, D-5.7 #1;
        │                                                  correct session, D-5.7 #2)
        │                                                       │
        ◄───────────────────────────────────────────────────────┘
     First successful Session::ping() < 1s after a
     successful launch attempt = SC4

WinRM FALLBACK (adapted from Phase 4's deploy-responder.ps1, unchanged shape):
  New-PSSession → Copy-Item -ToSession (real .exe, not .ps1)
    → Register-ScheduledTask (AtLogOn, Interactive principal)
    → Start-ScheduledTask (immediate kick, handles already-logged-in case)
       — same AtLogOn latency (~20-30s) and no-refire-on-reconnect gotcha
         empirically found in Phase 4 (04-03-SUMMARY.md) apply unchanged
```

### Recommended Project Structure

```
rdpilot/                          (repo root — NOT inside crates/, C# is not a Cargo member)
├── crates/rdpilot/
│   ├── src/
│   │   ├── connect.rs            # add: Rdpdr static-channel registration alongside DrdynvcClient
│   │   ├── rdpdr_backend.rs      # NEW — RdpilotDriveBackend (std::fs-based, Create/Close/Read/QueryDirectory)
│   │   ├── input.rs              # add: Key::Win + its scancode(false→true, 0x5B) mapping
│   │   ├── session.rs            # add: a bootstrap/launch helper (or a new Session::deploy_and_launch())
│   │   └── error.rs              # add: Error::Bootstrap(String) or reuse Error::Dvc for launch-poll failures
│   └── tests/
│       ├── fixtures/
│       │   ├── deploy-winrm.ps1  # RENAMED/adapted from deploy-responder.ps1 — copies the real .exe
│       │   └── sensor-responder.ps1, deploy-responder.ps1  # DELETE once the C# sensor supersedes them (D-5.7)
│       └── live_session.rs       # add: gated sensor_deploy_and_ping_within_1s test(s) (SC2/SC3/SC4)
└── sensor/                       # NEW top-level dir — the C# project, sibling to crates/ and infra/
    ├── RdpilotSensor.csproj      # PublishAot, SelfContained, RuntimeIdentifier=win-x64
    ├── Program.cs                # WTS P/Invoke open/read/write/close, retry loop, Version/Ping/Pong handling
    ├── Envelope.cs                # record/struct mirroring the Rust Envelope{version,req_id,type,payload}
    └── EnvelopeJsonContext.cs    # [JsonSerializable(typeof(Envelope))] partial JsonSerializerContext
```

### Pattern 1: `ironrdp-rdpdr` is a static channel, registered exactly like `DrdynvcClient` itself

**What:** `Rdpdr` implements `SvcProcessor`/`SvcClientProcessor`, not `DvcProcessor`. It is registered on the connector via `connector.with_static_channel(rdpdr_channel)` — the identical method already used for `drdynvc` in `connect.rs:101` — but as a **separate, sibling** static channel, not routed through `DrdynvcClient`.
**When to use:** Any time drive/printer/smartcard redirection is needed; this is IronRDP's only supported mechanism for it.
**Example (verified against `crates/ironrdp-client/src/rdp.rs`, official reference client):**
```rust
// Source: github.com/Devolutions/IronRDP crates/ironrdp-client/src/rdp.rs (verified 2026-07-09)
let rdpdr_channel = ironrdp_rdpdr::Rdpdr::new(
    Box::new(RdpilotDriveBackend::new(sensor_exe_path)),
    "rdpilot".to_owned(),
).with_drives(Some(vec![(0, "RDPILOT".to_owned())]));
connector = connector.with_static_channel(rdpdr_channel);
```

### Pattern 2: A minimal, read-only `RdpdrBackend` is enough for Phase 5's scope

**What:** `RdpdrBackend::handle_drive_io_request` receives a `ServerDriveIoRequest` enum with exactly four variants (verified by reading `ironrdp-rdpdr/src/pdu/efs.rs` directly): `ServerCreateDriveRequest` (i.e. `Create`), `DeviceCloseRequest`, `ServerDriveQueryDirectoryRequest`, `DeviceReadRequest`. Because Phase 5 only needs to serve one specific file for a one-shot copy, the backend does not need general filesystem semantics — it needs: (1) `Create` to succeed for the root path and for the one known filename, fail (`NtStatus::OBJECT_NAME_NOT_FOUND` or similar) for anything else; (2) `QueryDirectory` on the root to return the one file's metadata (Explorer/`copy` may enumerate before reading); (3) `Read` to stream bytes from the local `rdpilot-sensor.exe` at the requested offset/length; (4) `Close` to release the handle.
**When to use:** This phase only. A future phase needing bidirectional/general file transfer would extend this backend, not replace the approach.
**Confidence:** HIGH for the four-variant enum shape (read directly from source); MEDIUM for the exact `NtStatus` codes/response PDU shapes to use for each case, since the full `efs.rs` (142KB) response-construction helpers were not exhaustively read in this session — flagged as Open Question #1 below for the planner/executor to confirm against the same file at execution time.

### Pattern 3: `[LibraryImport]`, not `[DllImport]`, for the C# WTS P/Invoke block

**What:** Source-generated P/Invoke marshalling, required for Native AOT compatibility whenever a marshalled parameter (here: the channel name `string`) is involved.
**When to use:** Every P/Invoke declaration in the sensor.
**Example:**
```csharp
// Source: learn.microsoft.com/dotnet/standard/native-interop/pinvoke-source-generation (verified 2026-07-09)
internal static partial class Wts
{
    internal const uint WTS_CURRENT_SESSION = 0xFFFFFFFF;
    internal const uint WTS_CHANNEL_OPTION_DYNAMIC = 0x1;

    [LibraryImport("wtsapi32.dll", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
    internal static partial IntPtr WTSVirtualChannelOpenEx(uint sessionId, string virtualName, uint flags);

    [LibraryImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool WTSVirtualChannelRead(IntPtr channelHandle, uint timeout, byte[] buffer, uint bufferSize, out uint bytesRead);

    [LibraryImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool WTSVirtualChannelWrite(IntPtr channelHandle, byte[] buffer, uint length, out uint bytesWritten);

    [LibraryImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    internal static partial bool WTSVirtualChannelClose(IntPtr channelHandle);
}
```
Note `StringMarshalling.Utf16` (not `Utf8`/ANSI as the Phase 4 PowerShell `Add-Type` block used `CharSet.Ansi`) — confirm the exact ANSI-vs-Unicode entry point at execution time against the current MS Learn `WTSVirtualChannelOpenEx` page (the function has both `A`/`W` forms); this is a small, cheaply-verified detail, not a structural risk.

### Anti-Patterns to Avoid

- **Transliterating `sensor-responder.ps1`'s P/Invoke block into C# `[DllImport]`:** compiles fine under a normal (JIT) build but is silently AOT-incompatible or produces AOT trim warnings for the string-marshalled `WTSVirtualChannelOpenEx` call — must be `[LibraryImport]` from the start (D-5.7 already flags "re-derived idiomatically, not transliterated"; this is the concrete technical reason).
- **Depending on `ironrdp-rdpdr-native` for the client-side filesystem backend:** compiles to nothing on Windows (cfg-gated out) and adds an unused `nix` transitive dependency on Linux for no benefit given this project's canonical target is Windows.
- **Using `System.Text.Json`'s default reflection-based serializer in the sensor:** works under a normal JIT run but is exactly the kind of accidental-reflection dependency Native AOT trimming can silently miss or warn about; use the source-generated `JsonSerializerContext` from the start so `dotnet publish -p:PublishAot=true` surfaces any incompatibility at build time, not at runtime on the remote target where it's hardest to debug.
- **Building the NativeAOT `win-x64` publish from a non-Windows host:** Native AOT does not support cross-OS compilation (verified: Microsoft Learn "Cross-compilation" page + `dotnet/runtime` docs) — the C# publish step MUST run on a Windows host (see Pitfall 4 / Environment Availability).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| MS-RDPEFS (RDPDR) PDU framing, DYNVC-vs-SVC negotiation for drive redirection | A custom static-channel protocol implementation | `ironrdp-rdpdr`'s `Rdpdr`/`RdpdrPdu` | The wire protocol is a full Microsoft spec (MS-RDPEFS) with request/response PDU pairs for every IRP major function — `ironrdp-rdpdr` already decodes/encodes all of it; only the *backend* (actual file I/O) is left to the caller by design |
| C# JSON envelope (de)serialization for the fixed `{version,req_id,type,payload}` shape | Hand-written UTF-8 byte parsing | `System.Text.Json` + source-generated `JsonSerializerContext` | Built into the .NET 8 SDK, AOT-safe, and the Rust side already uses the equivalent (`serde`/`serde_json`) — symmetry, not reinvention |
| Win32 P/Invoke marshalling code generation | Hand-rolled IL stub / manual `Marshal.PtrToStructure` calls | `[LibraryImport]`'s compile-time source generator | This is precisely the problem `[LibraryImport]` was built to solve for Native AOT; hand-rolling marshalling code is exactly the fragile, security-sensitive surface the source generator eliminates |
| A "does the file exist" virtual filesystem for RDPDR | A general-purpose in-memory or real filesystem abstraction | The 4-variant minimal `RdpilotDriveBackend` (Pattern 2) | The task is "serve one specific file read-only for a copy operation," not "expose an arbitrary local directory" — building the general case is unjustified scope for this phase |

**Key insight:** Both the wire-protocol layer (MS-RDPEFS framing on the Rust side, the Version/Ping/Pong envelope on the C# side) and the marshalling layer (`[LibraryImport]`) already have correct, spec-compliant or first-party-generated implementations one layer down. The only genuinely new code this phase is: the minimal drive backend's file-serving logic, the WTS open/retry/read/write loop translated (not transliterated) into C#, the launch-and-poll orchestration, and one new `Key` variant.

## Common Pitfalls

### Pitfall 1: `[DllImport]` compiles under JIT but is not Native-AOT-safe for marshalled parameters
**What goes wrong:** A straight port of the PowerShell `Add-Type`/`DllImport` P/Invoke block (Phase 4's pattern) into C# `[DllImport]` will build and even run correctly under a normal `dotnet run`, masking the problem until `dotnet publish -p:PublishAot=true` either fails, warns, or (worst case) produces a binary with a subtly broken string-marshalling path.
**Why it happens:** `[DllImport]`'s marshalling is generated as an IL stub at JIT time; Native AOT has no JIT, so this generation step cannot happen for non-blittable parameters (strings, in this case `pVirtualName`).
**How to avoid:** Use `[LibraryImport]` with explicit `StringMarshalling` from the first line of code (Pattern 3/Code Examples).
**Warning signs:** AOT publish warnings mentioning marshalling/interop; or (if warnings are suppressed) a runtime failure specifically on the string-parameter call while integer/bool-only P/Invokes (Read/Write/Close, all blittable) work fine.

### Pitfall 2: `ironrdp-rdpdr-native` looks like a ready-made backend but is empty on the canonical build target
**What goes wrong:** Adding `ironrdp-rdpdr-native` as a dependency and calling into its `backend` module compiles fine on this session's Linux sandbox (since its cfg gate is satisfied there) but produces a "module not found"/empty-crate situation the moment the code is built for the repo's own pinned `x86_64-pc-windows-gnu` target (`rust-toolchain.toml`).
**Why it happens:** The crate's own `lib.rs` has `#[cfg(any(target_os = "macos", target_os = "linux"))] mod nix; #[cfg(...)] pub use nix::backend;` and nothing else — verified by reading the file directly.
**How to avoid:** Write `RdpilotDriveBackend` against `std::fs` (genuinely portable across both this repo's canonical Windows target and any Linux build/test sandbox) instead of depending on `ironrdp-rdpdr-native` at all.
**Warning signs:** `use ironrdp_rdpdr_native::backend::...` failing to resolve, or resolving fine locally (Linux) but failing on the pinned Windows target.

### Pitfall 3: The existing `Key` enum has no Windows/GUI key
**What goes wrong:** D-5.1's Win+R sequence cannot be built with the current `crates/rdpilot/src/input.rs` `Key` enum — attempting to express it with existing variants (e.g. misusing `Alt`+something) will not open the Run dialog.
**Why it happens:** Phase 3's `Key` vocabulary was scoped to "the computer-use key set" (modifiers + A-Z + digits + F-keys + navigation) and had no reason to include the Windows key at the time.
**How to avoid:** Add `Key::Win` with scancode `(extended: true, code: 0x5B)` — the standard IBM PC/AT Scan Code Set 1 value for the left Windows/GUI key — following the exact `match` pattern already used for every other entry (e.g. `Key::Delete => (true, 0x53)`), and route it through the existing `KeyAction::Combo` press/release-ordering machinery (D-3.5) unchanged; no other code needs to change.
**Warning signs:** Caught immediately at compile time if the planner tries to reference `Key::Win` before adding it — this is a purely additive, low-risk change, not flagged for further empirical validation.

### Pitfall 4: Native AOT does not cross-OS-compile — the sensor build step needs a Windows host
**What goes wrong:** Attempting `dotnet publish -r win-x64 -p:PublishAot=true` from this session's Linux sandbox (or any non-Windows CI/build host) fails outright — there is no supported way to obtain a Windows-targeting Native AOT toolchain from Linux/macOS.
**Why it happens:** Native AOT's native code generation step links against the target OS's own native toolchain/SDK (MSVC/Windows SDK for `win-x64`); Microsoft's own docs state cross-OS compilation is unsupported because there is no standardized way to obtain a foreign-OS native SDK on the host OS.
**How to avoid:** Build the sensor `.exe` on a genuinely Windows host. Two realistic options for this repo: (a) the operator's own ARM64 Windows workstation (already the pinned `rust-toolchain.toml` host per the Phase 2 architectural decision — has MinGW gcc but almost certainly does NOT yet have the .NET 8 SDK installed; treat as a new environment prerequisite, see Environment Availability), or (b) build directly ON the Phase 1 Azure Windows VM itself (already Windows, already reachable via WinRM/`az vm run-command`, and the .exe never needs to leave that machine before the RDPDR copy step) — this sidesteps the cross-compile problem entirely at the cost of needing the .NET 8 SDK installed on the VM (one more `Configure-Target.ps1`-style prerequisite, or an ad-hoc `az vm run-command` bootstrap).
**Warning signs:** `dotnet publish` erroring immediately with a toolchain/SDK-not-found message when `-r win-x64 -p:PublishAot=true` is combined with a non-Windows build host.

### Pitfall 5: AtLogOn scheduled-task latency and no-refire-on-reconnect (carried forward unchanged from Phase 4)
**What goes wrong:** Same as Phase 4's empirically-found gotcha — the WinRM fallback's `Register-ScheduledTask -Trigger (New-ScheduledTaskTrigger -AtLogOn ...)` takes ~20-30s to fire from a fresh interactive logon and does NOT refire on an RDP session *reconnect* (only a genuinely fresh logon).
**Why it happens:** Task Scheduler's own trigger-evaluation latency, and `AtLogOn` is defined as a logon-event trigger, not a session-state trigger.
**How to avoid:** Reuse Phase 4's proven mitigations verbatim: an immediate `Start-ScheduledTask` kick after registration (handles the already-logged-in case) plus a sufficiently wide client-side retry budget in the D-5.2 poll loop (Phase 4 needed 60s, not 15s, for its equivalent live gate).
**Warning signs:** First live-gate attempt hangs/times out on the WinRM path specifically, while the RDPDR path (which has no scheduled-task dependency) succeeds — this asymmetry is itself the diagnostic signal, exactly as Phase 4 documented.

## Code Examples

### Minimal `RdpilotDriveBackend` shape (Rust)

```rust
// Illustrative shape only — exact NtStatus/response-PDU field names must be
// confirmed against ironrdp-rdpdr-0.6.0/src/pdu/efs.rs at execution time
// (Open Question #1). The four-variant ServerDriveIoRequest enum and the
// RdpdrBackend trait signature ARE verified (read directly from source).
use ironrdp_rdpdr::backend::RdpdrBackend;
use ironrdp_rdpdr::pdu::efs::{ServerDriveIoRequest, ServerDeviceAnnounceResponse, DeviceControlRequest};
use ironrdp_rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp_svc::SvcMessage;
use ironrdp_pdu::PduResult;

#[derive(Debug)]
pub(crate) struct RdpilotDriveBackend {
    served_file: std::path::PathBuf, // e.g. the local path to the built rdpilot-sensor.exe
    served_name: String,             // e.g. "rdpilot-sensor.exe" — the name shown under \\tsclient\RDPILOT\
    open_files: std::collections::HashMap<u32, std::fs::File>,
}

impl RdpdrBackend for RdpilotDriveBackend {
    fn handle_server_device_announce_response(&mut self, _pdu: ServerDeviceAnnounceResponse) -> PduResult<()> { Ok(()) }
    fn handle_scard_call(&mut self, _req: DeviceControlRequest<ScardIoCtlCode>, _call: ScardCall) -> PduResult<()> { Ok(()) }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        match req {
            ServerDriveIoRequest::ServerCreateDriveRequest(create) => self.handle_create(create),
            ServerDriveIoRequest::DeviceCloseRequest(close) => self.handle_close(close),
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(q) => self.handle_query_directory(q),
            ServerDriveIoRequest::DeviceReadRequest(read) => self.handle_read(read),
        }
    }
}
```

### C# `Program.cs` retry-poll open sequence (mirrors the Phase 4 PowerShell shape, D-5.7 constraint #1)

```csharp
// Source: pattern carried forward from 04-RESEARCH.md Q3 / sensor-responder.ps1's
// proven retry loop, re-derived with LibraryImport (Pitfall 1) instead of DllImport.
IntPtr handle = IntPtr.Zero;
for (int attempt = 0; attempt < 20 && handle == IntPtr.Zero; attempt++)
{
    handle = Wts.WTSVirtualChannelOpenEx(Wts.WTS_CURRENT_SESSION, "RDPILOT_SENSOR", Wts.WTS_CHANNEL_OPTION_DYNAMIC);
    if (handle == IntPtr.Zero) await Task.Delay(500);
}
if (handle == IntPtr.Zero)
{
    throw new InvalidOperationException($"WTSVirtualChannelOpenEx failed after retries (LastError={Marshal.GetLastPInvokeError()})");
}
```

### C# JSON envelope + source-generated context

```csharp
// Source: pattern from learn.microsoft.com System.Text.Json source-generation docs (verified 2026-07-09)
public record Envelope(uint Version, ulong ReqId, string Type, object? Payload);

[JsonSerializable(typeof(Envelope))]
internal partial class EnvelopeJsonContext : JsonSerializerContext { }

// Usage — never the reflection-based JsonSerializer.Serialize<Envelope>(env) overload:
byte[] bytes = JsonSerializer.SerializeToUtf8Bytes(envelope, EnvelopeJsonContext.Default.Envelope);
var decoded = JsonSerializer.Deserialize(jsonSlice, EnvelopeJsonContext.Default.Envelope);
```

### `Key::Win` addition (Rust, `input.rs`)

```rust
// Add to the Key enum (alongside existing variants):
Win,

// Add to scancode()'s match (Set 1, left Windows/GUI key — extended):
Key::Win => (true, 0x5B),
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| ARCHITECTURE.md's original WinRM-first bootstrap ordering (Option B primary, Option A/drive-redirection fallback) | RDPDR drive-redirection PRIMARY, WinRM FALLBACK (D-4.5, carried into this phase as D-5.1/D-5.6) | Phase 4 CONTEXT (2026-07-08) | Already reflected in this phase's locked decisions; RESEARCH.md confirms the technical mechanics but does not revisit the ordering |
| Phase 4's throwaway PowerShell `Add-Type`/`DllImport` WTS responder | C# `[LibraryImport]`-based WTS P/Invoke, Native-AOT-published | This phase (D-5.7) | The PowerShell fixture proved the *protocol* (Version/Ping/Pong, retry-poll, framing-prefix strip) live against a real VM; the C# sensor must re-derive the *mechanism* (P/Invoke style) because AOT changes what's safe, even though the underlying WTS API calls are identical |

**Deprecated/outdated:** 04-CONTEXT.md's inherited note that "the `-native` backend being \*nix-only is a non-issue because our client runs on Linux" is corrected by this research for this repo's own canonical Windows build target — see Summary point 2 and Pitfall 2. This is a **correction to inherited research**, not a re-litigation of a locked decision (RDPDR-primary itself is unaffected; only the "how" of the backend implementation changes).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `ironrdp-rdpdr` version `"0.6"` will resolve cleanly against this repo's already-pinned `ironrdp-svc`/`ironrdp-pdu`/`ironrdp-core` versions (0.7.x/0.8/0.2) without a `Cargo.lock` conflict | Standard Stack | Low-medium — same publisher/release train as already-pinned crates, but not confirmed via an actual `cargo build` in this session (no `cargo` binary available in the research sandbox); the planner's first task should run `cargo build` immediately after adding the dependency and treat a version conflict as a normal, cheaply-fixed planning surprise, not a blocker |
| A2 | The exact `NtStatus`/response-PDU field shapes for `DeviceCreateResponse`/`DeviceReadResponse`/`ClientDriveQueryDirectoryResponse` (not exhaustively read from the 142KB `efs.rs` in this session — only the four-variant request enum and the `RdpdrBackend` trait signature were verified) will match the illustrative shape sketched in Code Examples | Architecture Patterns / Code Examples | Low — these are mechanical decode/encode-symmetric structs following the same `ensure_size!`/cursor pattern already proven correct for `Envelope`/`DeviceCreateRequest` in this same crate; a planner/executor reading `efs.rs` directly at execution time (as this research did for the request side) resolves this quickly |
| A3 | `[LibraryImport]`'s `StringMarshalling.Utf16` (not `.Utf8`/ANSI) is the correct choice for `WTSVirtualChannelOpenEx`'s `pVirtualName` parameter | Code Examples | Low — `WTSVirtualChannelOpenEx` per MS Learn is documented with both `A` (ANSI) and implicit wide-char forms; Phase 4's proven PowerShell fixture used `CharSet.Ansi` and worked live, so `StringMarshalling.Ansi`/UTF-8 may in fact be the empirically-correct choice — flag for a 5-minute confirmation against the live MS Learn page or by testing both at execution time, not a structural risk either way |
| A4 | Building the sensor directly ON the Azure Windows VM (bypassing the cross-compile problem, Pitfall 4) is operationally viable — i.e. the VM has (or can be given) outbound internet access to fetch the .NET 8 SDK installer | Common Pitfalls / Environment Availability | Medium — if the VM's NSG or environment blocks the .NET SDK download, this option collapses to "build on the operator's own Windows workstation" as the only path; the live gate will surface this immediately as a clear, diagnosable failure (SDK install fails), not a silent one |

## Open Questions

1. **Exact NtStatus/response-PDU shapes for the four `RdpdrBackend::handle_drive_io_request` cases.**
   - What we know: the four-variant request enum (`Create`/`Close`/`QueryDirectory`/`Read`) and the trait signature are confirmed from source; `DeviceCreateResponse`/`DeviceReadResponse`/`ClientDriveQueryDirectoryResponse` structs exist in the same file (`efs.rs`, confirmed by name search) but their field-level construction was not read line-by-line in this session.
   - What's unclear: the exact `NtStatus` value to use for "path not found" vs. "access denied" vs. success, and whether `ClientDriveQueryDirectoryResponse` needs a specific `FileInformationClass` variant for a single-file listing.
   - Recommendation: the planner's task for `RdpilotDriveBackend` should include a `read_first` pointing at `~/.cargo/registry/src/.../ironrdp-rdpdr-0.6.0/src/pdu/efs.rs` (once the dependency is added and the source is locally vendored/cached) to read the exact response struct fields before writing the four match arms — mirrors exactly how Phase 4 grounded `RdpilotSensorProcessor` in the pinned crate source rather than guessing.

2. **Which Windows host actually runs the NativeAOT `win-x64` publish step (Pitfall 4).**
   - What we know: it must be a genuinely Windows host; the two realistic candidates are the operator's ARM64 Windows workstation or the Azure VM itself.
   - What's unclear: which one this project's actual execution environment will use, and whether the .NET 8 SDK is pre-installed on either (Environment Availability below records both as unverified).
   - Recommendation: treat "confirm/install the .NET 8 SDK on the chosen Windows build host" as an explicit early task (or `checkpoint:human-verify`) in Wave 1 of the plan, before any C# code is written — this determines the whole rest of the C# track's executability.

3. **`StringMarshalling.Utf16` vs `.Ansi`/UTF-8 for the WTS P/Invoke `string` parameters (A3 above).**
   - What we know: both a Unicode and an ANSI form of `WTSVirtualChannelOpenEx` exist per Win32 convention; Phase 4's live-proven PowerShell fixture used the ANSI `CharSet`.
   - What's unclear: whether `wtsapi32.dll`'s exported symbol resolves the same way under .NET's `[LibraryImport]` entry-point resolution as it did under PowerShell's `Add-Type`.
   - Recommendation: cheap to resolve empirically at execution time (try one, if the P/Invoke call throws an entry-point-not-found error, flip to the other) — not worth blocking planning on.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| .NET 8 SDK on a Windows build host | NativeAOT publish of the sensor (SENSOR-01, Pitfall 4) | ✗ (not confirmed present on either the operator's ARM64 Windows workstation or the Azure VM — no evidence of a prior `.NET`/`dotnet` install anywhere in STATE.md's accumulated context, which only documents the Rust/scoop toolchain) | — | None with no cost — must be installed on whichever Windows host is chosen (workstation via a normal SDK installer, or the VM via `Configure-Target.ps1`-style CSE extension or an ad-hoc `az vm run-command`); this is a genuine new environment prerequisite for this phase, not a silent gap |
| Windows target with drive-redirection allowed by GPO | RDPDR primary path (SC2, D-5.6 MANDATORY) | Unconfirmed — STATE.md explicitly flags this as an open Blocker/Concern ("Drive redirection GPO policy on target is unknown") | — | None — D-5.6 makes RDPDR success mandatory, not conditionally waivable; if the lab VM's GPO blocks it, `Configure-Target.ps1` needs a policy change (empirical, per D-5.5's pattern) |
| AV/EDR posture on the target (does it flag `rdpilot-sensor.exe`) | Both deployment paths (D-5.5) | Unconfirmed — explicitly empirical by design, not solved at planning time | — | D-5.5's own fallback: an AV exclusion added to `Configure-Target.ps1` / the deploy step, only if the live gate actually surfaces a block |
| Azure VM (Phase 1 target) | Live gated tests (SC2-SC4) | Assumed ✓ (reused from Phase 1/4, same provisioning mechanism, `manage-env.ps1 up`) | — | None needed — already the locked live-test mechanism |
| `pwsh`/WinRM on the target | WinRM fallback deploy (SC3) | Assumed ✓ (proven live in Phase 4) | — | None needed |

**Missing dependencies with no fallback:**
- .NET 8 SDK on a Windows build host — must be installed before any C# code can be published; treat as an explicit Wave 0/Wave 1 task or `checkpoint:human-verify`, not an assumption.
- Drive-redirection GPO allowance on the target — D-5.6 makes this mandatory; if blocked, the plan needs an explicit `Configure-Target.ps1` remediation task discovered empirically at the live gate, per D-5.5's established pattern (this mirrors exactly how Phase 3's Server Manager auto-launch and Phase 4's AtLogOn latency were discovered and fixed empirically, not pre-solved).

**Missing dependencies with fallback:**
- AV/EDR blocking the sensor binary — D-5.5's fallback (AV exclusion) applies only if actually triggered.

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework (Rust) | `cargo test` (built-in), gated live suite in `crates/rdpilot/tests/live_session.rs` — same pattern as Phases 2-4 |
| Framework (C#) | None yet — this phase is the first to introduce C# code. A minimal offline-testable seam (e.g. the envelope JSON round-trip, or the drive-backend's path-resolution logic) should get at least a small xUnit/MSTest project if the planner judges it worth the setup cost; the sensor's actual WTS I/O is inherently only live-testable (mirrors why Phase 4's PowerShell responder had zero unit tests of its own — it was only proven via the Rust-side live test) |
| Config file | none — plain `#[test]`/`#[ignore]` gating via `RDPILOT_LIVE` (existing pattern, unchanged) |
| Quick run command | `cargo test -p rdpilot` (offline; new `RdpilotDriveBackend`/`Key::Win` logic must be unit-testable without a VM, mirroring Phase 4's `Envelope`/`HandshakeState` offline tests) |
| Full suite command | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SENSOR-01 (SC1) | `rdpilot-sensor.exe` builds as NativeAOT self-contained, no external runtime dep | build check | `dotnet publish sensor/RdpilotSensor.csproj -c Release -r win-x64 -p:PublishAot=true --self-contained` on a Windows host; assert exit 0 and the output binary's absence of a companion `hostfxr`/shared-runtime dependency | ❌ Wave 0 — new C# project entirely |
| SENSOR-02 (SC2) | RDPDR copy + injected launch succeeds inside the RDP session | live (gated) | `RDPILOT_LIVE=1 cargo test -p rdpilot sensor_rdpdr_deploy_and_ping_within_1s -- --ignored` | ❌ Wave 0 — new test |
| SENSOR-02 (SC3) | WinRM bootstrap path also deploys + launches successfully | live (gated) | `RDPILOT_LIVE=1 cargo test -p rdpilot sensor_winrm_deploy_and_ping_within_1s -- --ignored` | ❌ Wave 0 — new test, adapts Phase 4's `deploy-responder.ps1` pattern |
| SC4 (both paths) | First successful `Session::ping()` after a successful launch is `< 1s` | live (gated) | same two tests above — the ping-after-launch assertion is the SC4 measurement, mirroring Phase 4's `elapsed < Duration::from_millis(500)` pattern exactly but with the SC4-specified 1s bound | ❌ Wave 0 |
| `Key::Win` correctness | Pressing `Combo([Win])` (or `Combo([Win, R])`) sends a non-empty `FastPath` batch | unit | `cargo test -p rdpilot send_key_combo_win_r_sends_nonempty_fastpath_batch` (offline, mirrors the existing `send_key_combo_ctrl_a_sends_nonempty_fastpath_batch` pattern in `session.rs`'s test module) | ❌ Wave 0 — trivial, same shape as existing tests |
| `RdpilotDriveBackend` request routing | Each of the 4 `ServerDriveIoRequest` variants is dispatched without panicking | unit | new `cargo test -p rdpilot` cases constructing each variant and asserting `Ok(_)` from `handle_drive_io_request` | ❌ Wave 0 — offline, no VM needed, mirrors Phase 4's `malformed_payload_is_dropped_without_panic` style |

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot` (offline; the C# side has no equivalent fast offline gate this phase beyond the AOT publish build check, which is inherently a build-host-specific step, not a per-commit CI gate in this sandbox)
- **Per wave merge:** `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` (once a live target + built sensor .exe are available)
- **Phase gate:** Full suite green (both RDPDR and WinRM live tests PASS, D-5.6 makes the RDPDR one mandatory) before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `sensor/RdpilotSensor.csproj` + `Program.cs` + `Envelope.cs`/`EnvelopeJsonContext.cs` — the entire C# project does not exist yet
- [ ] `crates/rdpilot/src/rdpdr_backend.rs` — new module, `RdpilotDriveBackend`
- [ ] `Key::Win` + scancode mapping in `input.rs`
- [ ] `crates/rdpilot/tests/live_session.rs::sensor_rdpdr_deploy_and_ping_within_1s` and `::sensor_winrm_deploy_and_ping_within_1s`
- [ ] `crates/rdpilot/tests/fixtures/deploy-winrm.ps1` (adapted from `deploy-responder.ps1`)
- [ ] .NET 8 SDK installed on a genuinely Windows build host (environment prerequisite, not a repo file — see Environment Availability / Open Question #2)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | No | Out of scope — RDPDR/DVC both inherit RDP's own CredSSP/TLS authentication; no new auth surface |
| V3 Session Management | No | No new session concept |
| V4 Access Control | Yes (narrow) | `RdpilotDriveBackend` MUST reject any `Create`/`Read` request for a path other than the one specifically-served file (or the root directory listing) — this is the access-control boundary of the whole feature: the redirected drive must not become a general local-filesystem read primitive for the remote session |
| V5 Input Validation | Yes | Both sides: the C# sensor's JSON envelope decode (mirrors Phase 4's `serde_json::from_slice` → typed-drop-never-panic discipline, API-01 equivalent for C#: catch `JsonException`, drop the malformed message, never throw unhandled) and the Rust backend's IRP path parsing (never trust the server-supplied path string without normalizing/comparing against the one allowed filename) |
| V6 Cryptography | No | Rides the existing RDP TLS tunnel (Phase 2); no new crypto primitive |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| A malicious/compromised remote session using the redirected drive to enumerate or read arbitrary local files beyond the one served sensor binary | Information Disclosure | `RdpilotDriveBackend`'s `Create`/`QueryDirectory` handlers must hard-reject any path that doesn't exactly match the one served filename (or the drive root itself) — this is the entire security posture of the minimal backend design (Pattern 2), not an afterthought |
| Malformed/oversized JSON on the C# sensor's DVC read path causing an unhandled exception (crash) or resource exhaustion | Denial of Service | Same discipline as Phase 4's Rust `Error::Dvc` pattern: wrap `JsonSerializer.Deserialize` in a try/catch scoped to `JsonException`, drop the malformed message, keep the read loop alive — never let an unhandled exception terminate `Program.Main`'s loop |
| Same-session process impersonation of either DVC endpoint (Pitfall m3, carried forward unchanged from Phase 4 — DVC remains unauthenticated transport by design) | Spoofing/Tampering | Unchanged from Phase 4's accepted-risk disposition — explicitly out of scope for the single-tenant lab VM; not re-litigated this phase |
| AV/EDR quarantining or killing the sensor process mid-session (PITFALLS.md Pitfall C5) | Denial of Service (self-inflicted, not adversarial) | D-5.5's empirical handling — not a code-level mitigation this phase, an operational one (AV exclusion added only if actually observed) |

## Sources

### Primary (HIGH confidence)
- `ironrdp-rdpdr-0.6.0` source, read directly from GitHub raw content (`crates/ironrdp-rdpdr/src/lib.rs`, `src/backend/mod.rs`, `src/backend/noop.rs`, `src/pdu/mod.rs` listing, `src/pdu/efs.rs` grep for `ServerDriveIoRequest`/`DeviceCreateRequest`, `Cargo.toml`) — https://github.com/Devolutions/IronRDP/tree/master/crates/ironrdp-rdpdr
- `ironrdp-rdpdr-native-0.6.0` source, read directly (`src/lib.rs`, `Cargo.toml`) — confirms the `#[cfg(any(target_os = "macos", target_os = "linux"))]` gate — https://github.com/Devolutions/IronRDP/tree/master/crates/ironrdp-rdpdr-native
- `crates/ironrdp-client/src/rdp.rs` (official reference client), fetched — confirms `Rdpdr::new(Box::new(NoopRdpdrBackend), ...)` + `connector.with_static_channel(rdpdr_channel)` registration pattern — https://github.com/Devolutions/IronRDP/blob/master/crates/ironrdp-client/src/rdp.rs
- [Native AOT deployment overview (Microsoft Learn)](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/) — PublishAot property, self-contained deployment model
- [Cross-compilation (Microsoft Learn, Native AOT)](https://learn.microsoft.com/en-us/dotnet/core/deploying/native-aot/cross-compile) — cross-OS compilation unsupported, confirmed
- [P/Invoke source generation (Microsoft Learn)](https://learn.microsoft.com/en-us/dotnet/standard/native-interop/pinvoke-source-generation) — `[LibraryImport]` vs `[DllImport]`, explicit Native-AOT-incompatibility statement for `DllImport`
- [Reflection vs. source generation in System.Text.Json (Microsoft Learn)](https://learn.microsoft.com/en-us/dotnet/standard/serialization/system-text-json/reflection-vs-source-generation) — `JsonSerializerContext` requirement for AOT/trim scenarios
- This repo's `crates/rdpilot/src/{sensor.rs,connect.rs,session.rs,session_loop.rs,input.rs,error.rs}`, `Cargo.toml`, `tests/{common/mod.rs,live_session.rs,fixtures/deploy-responder.ps1}` — the existing seams and conventions this phase extends
- `.planning/phases/04-dvc-transport-channel/{04-CONTEXT.md,04-RESEARCH.md,04-03-PLAN.md,04-03-SUMMARY.md}`, `.planning/phases/05-sensor-bootstrap-deployment/05-CONTEXT.md`, `.planning/{ROADMAP.md,REQUIREMENTS.md,STATE.md}`, `.planning/research/{ARCHITECTURE.md,PITFALLS.md,FEATURES.md}` — canonical phase-scope and prior-research inputs

### Secondary (MEDIUM confidence)
- [.NET 8 runtime what's-new (Microsoft Learn)](https://github.com/dotnet/docs/blob/main/docs/core/whats-new/dotnet-8/runtime.md) — `JsonSerializerIsReflectionEnabledByDefault`, `JsonStringEnumConverter` AOT notes
- General web search results on NativeAOT binary size / cold-start figures (2MB-9MB range, <5ms-50ms cold start depending on app complexity) — not verified against this specific project's dependency footprint; treated as directional only, per D-5.3's own instruction to benchmark empirically

### Tertiary (LOW confidence)
- None used for load-bearing claims in this document.

## Metadata

**Confidence breakdown:**
- Standard stack (C# NativeAOT config, LibraryImport requirement): HIGH — grounded in official Microsoft Learn docs read directly, not training-data recall
- Standard stack (`ironrdp-rdpdr` API surface, static-channel registration, backend requirement): HIGH — grounded in actual crate source read directly from GitHub, including the reference client's own usage
- Architecture (minimal `RdpilotDriveBackend` design): MEDIUM-HIGH — the 4-variant request enum and trait signature are verified; exact response-PDU field construction (A2) needs a follow-up source read at execution time
- Pitfalls: HIGH for Pitfalls 1/2/4 (each independently verified against official docs or crate source); MEDIUM for Pitfall 3 (`Key::Win` scancode value is a well-established public-standard fact, same confidence class as the rest of the already-shipped scancode table, not independently re-verified against a primary spec this session)
- Environment Availability (.NET 8 SDK presence, GPO/AV state): explicitly LOW/unconfirmed by design — these are the phase's genuine empirical unknowns per D-5.3/D-5.5, not gaps in this research

**Research date:** 2026-07-09
**Valid until:** ~30 days for the IronRDP/`ironrdp-rdpdr` findings (weekly release cadence per prior research, but this repo's `Cargo.lock` pins will not drift underneath the phase once added); ~90 days for the .NET 8 Native AOT/`LibraryImport` findings (stable, LTS-release, unlikely to change before .NET 9/10 adoption)
