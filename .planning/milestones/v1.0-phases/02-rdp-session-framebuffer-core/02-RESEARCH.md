# Phase 2: RDP Session + Framebuffer Core - Research

**Researched:** 2026-06-05
**Domain:** Pure-Rust RDP client transport + framebuffer (IronRDP), async session lifecycle, PNG screenshot pipeline
**Confidence:** HIGH (core API surface verified against current upstream source + crates.io index)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Cargo **workspace** at repo root, single member `crates/rdpilot` today. Future phases add sibling crates without restructuring.
- **D-02:** `Cargo.lock` **is committed** (resolves the Phase 1 deferral; this workspace produces binaries/examples).
- **D-03:** Rust **1.78+**. Crate name: `rdpilot`.
- **D-04:** **SDK owns the session loop.** `Session::connect(&cfg).await` returns a `Session` handle; SDK spawns a Tokio background task that pumps the IronRDP connection and maintains the latest `DecodedImage`. Consumers never drive the RDP state machine. Public shape:
  ```rust
  let session = Session::connect(&cfg).await?;
  let png = session.screenshot().await?;   // latest framebuffer, decoded
  session.close().await?;
  ```
- **D-05:** **Headless library** — rdpilot creates no local RDP GUI window, so "stays rendered while local window minimized/hidden" is inherently satisfied client-side. Remote-side rendering handled by `RemoteDesktop_SuppressWhenMinimized=2` (already applied by Phase 1).
- **D-06:** **Keepalive is automatic** — background keepalive (synthetic null input ~every 60s, per PITFALLS M1) runs while a `Session` is alive, no opt-in. Satisfies the 10-minute idle criterion with zero caller effort.
- **D-07:** **Teardown** = explicit async `close().await` for a clean disconnect **plus a `Drop` guard** that aborts the background task / tears down the socket if dropped without `close()`.
- **D-08:** **Verify the minimized-render property behaviorally**, not by reading the remote registry. Hold a windowless session through the idle period, assert the framebuffer is still live, full-resolution, non-blank. **No WinRM dependency in Phase 2's session path.**
- **D-09:** `screenshot()` returns an **owned SDK type**, not a third-party type:
  ```rust
  pub struct Screenshot { width: u32, height: u32, rgba: Vec<u8> }
  impl Screenshot { pub fn to_png(&self) -> Result<Vec<u8>>; pub fn crop(&self, r: Rect) -> Screenshot; }
  ```
  The `image` crate is an **internal impl detail**, never in the public API.
- **D-10:** **Crop = caller supplies the `Rect`** (CAP-01 #3). Real window geometry → Phase 6.
- **D-11:** **PNG encoding lives on the SDK** (`.to_png()`); caller writes bytes. Raw RGBA retained so cropping never re-decodes a PNG.
- **D-12:** SDK public API takes a **typed `ConnectionConfig`** (host, port, credentials, requested width/height). **Library is env-agnostic** — no repo-specific secrets path hard-coded.
- **D-13:** Parameters reach the SDK via **env vars or config file**. Convenience loaders may live in the SDK; source/path is the caller's choice. **For our testing, `.secrets/connection.json` (written by Phase 1's `manage-env.ps1`) is the path.**
- **D-14:** rdpilot **does not get in the middle of OS credential machinery** (Credential Manager / Hello). Credentials are passed as parameters and handed to NLA/CredSSP by our own IronRDP client.
- **D-15:** **Server-cert trust:** default to normal cert validation, but expose an explicit, risk-named `ConnectionConfig` flag (`accept_invalid_certs(true)` or equivalent) for the self-signed workgroup lab VM. **Thumbprint pinning deferred.**
- **D-16:** Validation is a **full automated integration test suite** covering all 5 success criteria.
- **D-17:** **All local at this stage — no CI yet.** Suite must be **CI-portable**.
- **D-18:** Default `cargo test` with **no live target must not hard-fail** — live-VM tests gated/skipped when no target present. Canonical run executes the full suite (full 10-min idle).
- **D-19:** **Rust toolchain not yet installed** — install via **`scoop`** before any build/test work.

### Claude's Discretion (research/planner decides)
- **Color decode (criterion #2 / YUV-grey pitfall):** ensure the IronRDP codec path decodes to correct RGB. → **Resolved below: IronRDP decodes to `PixelFormat::RgbA32` natively; no manual YUV conversion needed on the bitmap/RemoteFX path.**
- **Error-type design:** `thiserror` enum, `Result` surface; **no `unwrap`/`expect` in library code** (API-01).
- **Logging/observability:** `tracing` (default off/quiet).
- **Default requested resolution / color depth.**
- **Idle-test duration parameterization** (short in dev, full 10-min canonical).
- **DVC seam:** leave the `RDPILOT_SENSOR` `DvcProcessor` registration hook point so Phase 4 can register **before** `connect_finalize` completes — do **not** build the sensor in Phase 2.
- **Async API ergonomics** (builder vs ctor for `ConnectionConfig`, loader signatures).

### Deferred Ideas (OUT OF SCOPE)
- **Thumbprint cert pinning** — deferred.
- **CI integration for live-VM tests** — later; write tests CI-portable now.
- **Per-window screenshots from real window geometry / enumeration** → Phase 6 (CAP-02). Phase 2 only crops to a caller-supplied rect.
- **Input injection** → Phase 3. **DVC sensor / structured perception** → Phase 4+.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SESS-01 | Connect to and authenticate (NLA / credentials) an RDP session to a Windows target | `ironrdp::connector::Config` with `enable_credssp: true` + `Credentials::UsernamePassword`; connect flow `connect_begin` → TLS upgrade → `mark_as_upgraded` → `connect_finalize`. Cert trust via custom rustls `ServerCertVerifier`. (Standard Stack, Code Examples, Architecture Pattern 1) |
| SESS-02 | Manage session lifecycle (open, keepalive, teardown) and keep the session rendered | Async background task with `tokio::select!` over `reader.read_pdu()` + an input-event mpsc channel; keepalive = periodic synthetic `FastPathInputEvent`; teardown = `graceful_shutdown()` + `Drop` guard. Stays-rendered satisfied by construction (no Suppress Output PDU emitted). (Patterns 1–3, Pitfall headless) |
| CAP-01 | Capture a full-desktop screenshot from the RDP framebuffer (+ crop-to-rect #3) | `DecodedImage` (RGBA32) `image.data()` → `image::ImageBuffer::<Rgba<u8>>::from_raw` → PNG; crop = stride math on the RGBA buffer. (Patterns 4–5, Code Examples, Don't Hand-Roll) |
</phase_requirements>

## Summary

Phase 2 introduces the first Rust code in the repo and builds the IronRDP transport + framebuffer foundation. The good news: **the entire flow this phase needs is demonstrated by two canonical, currently-maintained upstream references** — `crates/ironrdp/examples/screenshot.rs` (blocking; the exact connect + DecodedImage → PNG pattern) and `crates/ironrdp-client/src/rdp.rs` (async; the exact `tokio::select!` session loop with reader/writer split, input-event channel, and Deactivation-Reactivation handling). The SDK design in CONTEXT.md (D-04: SDK-owned async loop, `Session` handle, latest-framebuffer snapshot) maps almost 1:1 onto `ironrdp-client`'s `active_session` function.

**Two corrections to the locked stack (CLAUDE.md / research/STACK.md predate the current release):**
1. **Crate versions have moved and are *not* uniform.** The umbrella `ironrdp` facade is now **0.15.0** (was 0.14.0). Member crates carry **independent** versions: `ironrdp-connector`/`-session`/`-tokio`/`-blocking` = **0.9.0**, `ironrdp-graphics` = **0.8.0**, `ironrdp-pdu` = **0.8.0**, `ironrdp-input` = **0.6.0**, `ironrdp-dvc` = **0.6.0**, `ironrdp-tls` = **0.2.1**. Pinning `ironrdp-tokio = "0.14"` (as STACK.md's install block does) **will not resolve**. Depend on the umbrella `ironrdp = "0.15"` facade for the re-exported types and pin the few standalone crates (`ironrdp-tokio`, `ironrdp-blocking`, `ironrdp-tls`, `ironrdp-dvc`, `ironrdp-input`, `ironrdp-pdu`) at their real versions, or use `ironrdp` features. [VERIFIED: crates.io sparse index, 2026-06-05]
2. **`RemoteDesktop_SuppressWhenMinimized=2` is irrelevant to a custom IronRDP client.** That registry key only controls **mstsc.exe**'s behavior of emitting a *Client Suppress Output PDU* when its window is minimized. A headless IronRDP client never has a window and never sends that PDU, so the server keeps sending full-resolution updates unconditionally. The success criterion's mechanism is an mstsc artifact; for rdpilot the property is satisfied **by construction**. D-05/D-08 already encode this correctly — verify the *effect* behaviorally, do not depend on the registry key in Phase 2's path. [CITED: MS-RDPBCGR Client Suppress Output PDU]

**Primary recommendation:** Mirror `ironrdp-client/src/rdp.rs`'s `active_session` loop inside an SDK-owned Tokio task. Keep a shared latest-`DecodedImage` (the loop already owns the only mutating copy — snapshot its RGBA bytes on each `GraphicsUpdate` into an `Arc<Mutex<…>>` / watch channel that `screenshot()` reads). Decode is native RGBA32 — no YUV conversion. Keepalive is a periodic synthetic `FastPathInputEvent` sent through the same input-event channel the loop already selects on.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| RDP connect + NLA/CredSSP auth (SESS-01) | rdpilot SDK (local, Rust) | — | Pure-Rust IronRDP client owns the protocol; no remote/agent component |
| TLS handshake + server-cert trust decision | rdpilot SDK (local) | — | rustls `ClientConfig` built by the SDK; cert policy is a `ConnectionConfig` flag |
| Session loop / PDU pump (SESS-02) | rdpilot SDK background Tokio task | — | D-04: SDK owns the loop; consumer never drives the state machine |
| Keepalive (SESS-02) | rdpilot SDK background task | — | D-06: synthetic input emitted locally; no remote helper in Phase 2 |
| Framebuffer maintenance (CAP-01) | rdpilot SDK (local, `ironrdp-graphics`) | — | `DecodedImage` lives in the loop; server pushes graphics updates |
| Screenshot encode + crop (CAP-01) | rdpilot SDK (local, `image` crate) | — | D-09/D-11: SDK owns PNG + crop; `image` is internal |
| Remote desktop "stays rendered" | **Remote Windows target** (DWM rendering) | rdpilot SDK (never sends Suppress Output PDU) | Server renders unconditionally; SDK's only obligation is to *not* suppress |
| Credentials source (config/env) | Consumer / test harness | rdpilot SDK convenience loader (optional) | D-12/D-13: library is env-agnostic |

## Standard Stack

> All crate versions below verified against the crates.io sparse index (`index.crates.io`) on 2026-06-05, the authoritative Rust registry. Core IronRDP crates are cross-confirmed by the live upstream example source (`screenshot.rs`, `ironrdp-client/src/rdp.rs`) fetched the same day.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ironrdp` (umbrella facade) | **0.15.0** | Re-exports `connector`, `session`, `pdu`, `graphics`, `input` under one crate | Single dependency for the common path; the screenshot/client examples import via `ironrdp::…` |
| `ironrdp-tokio` | **0.9.0** | Async `TokioFramed`, `connect_begin`/`connect_finalize`, `split_tokio_framed`, `single_sequence_step_read` | Required for the D-04 SDK-owned async loop (the blocking `screenshot.rs` uses `ironrdp-blocking` instead — we want async) |
| `ironrdp-tls` | **0.2.1** | rustls TLS integration helper for RDP | Optional convenience; the examples build the rustls `ClientConfig` directly (see Code Examples) — either path works |
| `tokio` | **1.52.3** (pin `1`) | Async runtime | IronRDP-tokio is built on it; de-facto standard |
| `rustls` | **0.23.40** (pin `0.23`) | TLS for RDP | Used by IronRDP; memory-safe, no OpenSSL. **Must disable resumption (CredSSP requirement).** |
| `tokio-rustls` | **0.26.4** | Async rustls stream wrapper | Used by the examples for the TLS-upgraded stream |
| `sspi` | **0.21.0** | CredSSP / NLA credential negotiation; provides `ReqwestNetworkClient` passed to `connect_finalize` | IronRDP delegates CredSSP to `sspi`; required by `connect_finalize` |
| `image` | **0.25.10** (pin `0.25`) | PNG encode from raw RGBA buffer (`ImageBuffer<Rgba<u8>>`) | D-11; internal impl detail only |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `ironrdp-pdu` | **0.8.0** | `FastPathInputEvent`, `client_info` types (`CompressionType`, `PerformanceFlags`, `TimezoneInfo`), input PDUs | Building keepalive synthetic input; config struct fields |
| `ironrdp-input` | **0.6.0** | `Database`, `MouseButton`, `Scancode`, `Operation` → `FastPathInputEvent` | Keepalive null-input construction (and Phase 3 real input) |
| `ironrdp-dvc` | **0.6.0** | `DvcProcessor` trait, `DrdynvcClient` for channel registration | **Phase 2: only leave the registration seam** (D: discretion). Do not build the sensor. |
| `thiserror` | **2.0.18** (pin `2`) | Library error enum (`#[derive(Error)]`) | Error-type design (API-01: no `unwrap`/`expect` in lib code) |
| `tracing` | **0.1.44** | Structured diagnostics, default-quiet | Discretion item; the examples use it |
| `x509-cert` | (latest 0.2.x) | DER decode of peer cert to extract server public key for CredSSP | The examples decode `subject_public_key_info` to feed `connect_finalize`; needed if you replicate the example's TLS path |
| `anyhow` | (latest 1.x) | Examples use it for app-level errors | **Examples only** — the SDK should use `thiserror`, not leak `anyhow` into the public API |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `ironrdp-tokio` (async) | `ironrdp-blocking` | Blocking is what `screenshot.rs` uses and is simpler, but D-04 mandates an SDK-owned async background task pumping the session while `screenshot()` is called concurrently → async is required. Blocking would force a dedicated OS thread + channel bridge for no benefit. |
| Building `rustls::ClientConfig` by hand | `ironrdp-tls` helper | The examples hand-build the config (to install a custom cert verifier and disable resumption). `ironrdp-tls` wraps the common case; for the D-15 "accept invalid certs" flag you need the custom-verifier path anyway. Recommend hand-building per the example. |
| `ironrdp` umbrella facade | Pinning every member crate individually | Umbrella is cleaner and matches example imports (`ironrdp::connector`, `ironrdp::session`). Only pin standalone crates (`ironrdp-tokio`, `-tls`, `-dvc`, `-input`, `-pdu`) that the facade doesn't re-export the needed surface of. |

**Installation (corrected — supersedes the version block in research/STACK.md):**
```toml
[dependencies]
ironrdp        = "0.15"
ironrdp-tokio  = "0.9"
ironrdp-tls    = "0.2"      # optional; or hand-build rustls ClientConfig
ironrdp-dvc    = "0.6"      # Phase 2: registration seam only
ironrdp-input  = "0.6"
ironrdp-pdu    = "0.8"
tokio          = { version = "1", features = ["full"] }
tokio-rustls   = "0.26"
rustls         = "0.23"
sspi           = "0.21"
image          = "0.25"
thiserror      = "2"
tracing        = "0.1"
x509-cert      = "0.2"      # if replicating the example's cert-key extraction
```

**Version verification performed:**
```
index.crates.io → ironrdp=0.15.0  ironrdp-connector=0.9.0  ironrdp-session=0.9.0
                  ironrdp-graphics=0.8.0  ironrdp-pdu=0.8.0  ironrdp-tokio=0.9.0
                  ironrdp-blocking=0.9.0  ironrdp-input=0.6.0  ironrdp-dvc=0.6.0
                  ironrdp-tls=0.2.1  image=0.25.10  tokio=1.52.3  rustls=0.23.40
                  tokio-rustls=0.26.4  sspi=0.21.0  thiserror=2.0.18  tracing=0.1.44
```

## Package Legitimacy Audit

> slopcheck installed successfully but does **not** support the crates.io (Rust) ecosystem — it targets npm/PyPI. All crates were instead verified against the authoritative `index.crates.io` sparse index, and the core IronRDP crates are independently confirmed by the live upstream source files I fetched (the `Devolutions/IronRDP` GitHub repo). This is correct ecosystem verification per the protocol.

| Package | Registry | Source Repo | Verification | Disposition |
|---------|----------|-------------|--------------|-------------|
| `ironrdp` (+ members) | crates.io | github.com/Devolutions/IronRDP | Index resolves 0.15.0; example source fetched live | Approved — `[VERIFIED: crates.io index + upstream source]` |
| `ironrdp-tokio` | crates.io | github.com/Devolutions/IronRDP | Index 0.9.0; used in `ironrdp-client/src/rdp.rs` | Approved `[VERIFIED]` |
| `ironrdp-tls` | crates.io | github.com/Devolutions/IronRDP | Index 0.2.1 | Approved `[VERIFIED]` |
| `ironrdp-dvc` | crates.io | github.com/Devolutions/IronRDP | Index 0.6.0; DVC docs verified | Approved `[VERIFIED]` |
| `ironrdp-input` | crates.io | github.com/Devolutions/IronRDP | Index 0.6.0; lib.rs source fetched | Approved `[VERIFIED]` |
| `ironrdp-pdu` | crates.io | github.com/Devolutions/IronRDP | Index 0.8.0; imported in example | Approved `[VERIFIED]` |
| `tokio` | crates.io | github.com/tokio-rs/tokio | Index 1.52.3 | Approved `[VERIFIED]` |
| `rustls` | crates.io | github.com/rustls/rustls | Index 0.23.40 | Approved `[VERIFIED]` |
| `tokio-rustls` | crates.io | github.com/rustls/tokio-rustls | Index 0.26.4 | Approved `[VERIFIED]` |
| `sspi` | crates.io | github.com/Devolutions/sspi-rs | Index 0.21.0; required by `connect_finalize` | Approved `[VERIFIED]` |
| `image` | crates.io | github.com/image-rs/image | Index 0.25.10 | Approved `[VERIFIED]` |
| `thiserror` | crates.io | github.com/dtolnay/thiserror | Index 2.0.18 | Approved `[VERIFIED]` |
| `tracing` | crates.io | github.com/tokio-rs/tracing | Index 0.1.44 | Approved `[VERIFIED]` |
| `x509-cert` | crates.io | github.com/RustCrypto/formats | RustCrypto org | Approved `[VERIFIED]` |

**Packages removed due to slopcheck [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none
**Note:** Because `Cargo.lock` is committed (D-02), the resolved transitive graph is auditable in the lockfile diff at install time — recommend the planner add a checkpoint to review new transitive crates the first time `cargo build` runs.

## Architecture Patterns

### System Architecture Diagram

```
 CONSUMER (test harness / future AI agent)
        │  Session::connect(&cfg).await → Session handle
        │  session.screenshot().await  ──────────────────────────┐
        │  session.close().await                                 │ reads latest
        ▼                                                        │ framebuffer snapshot
 ┌──────────────────────────────────────────────────────────────┼───────────────┐
 │ rdpilot SDK (crates/rdpilot)                                  │               │
 │                                                               ▼               │
 │  ConnectionConfig ─► connect():                       ┌──────────────────┐    │
 │     1. TCP connect                                    │ shared latest    │    │
 │     2. ironrdp_tokio::connect_begin                   │ framebuffer      │    │
 │     3. TLS upgrade (rustls ClientConfig,              │ Arc<Mutex<RGBA>> │    │
 │        custom cert verifier per D-15,                 │  or watch chan   │    │
 │        resumption disabled)                           └────────▲─────────┘    │
 │     4. mark_as_upgraded                                        │ snapshot on  │
 │     5. connect_finalize (sspi CredSSP/NLA)                     │ GraphicsUpdate│
 │            │ returns ConnectionResult + UpgradedFramed         │              │
 │            ▼                                                   │              │
 │   ┌────────────────────────────────────────────────────┐      │              │
 │   │ spawned Tokio task: active_session loop            │      │              │
 │   │   split_tokio_framed → (reader, writer)            │      │              │
 │   │   tokio::select! {                                 │      │              │
 │   │     reader.read_pdu()  → active_stage.process()────┼──────┘              │
 │   │        → GraphicsUpdate → snapshot RGBA            │                     │
 │   │        → ResponseFrame → writer.write_all          │                     │
 │   │        → DeactivateAll → reactivation sequence     │                     │
 │   │        → Terminate → exit loop                     │                     │
 │   │     input_rx.recv()    → FastPath (keepalive)      │◄── keepalive timer  │
 │   │                        → Close → graceful_shutdown │◄── close()/Drop     │
 │   │     keepalive_interval.tick() → emit null FastPath │                     │
 │   │   }                                                │                     │
 │   └────────────────────────────────────────────────────┘                     │
 │            │ DVC registration seam (Phase 4): DrdynvcClient + DvcProcessor    │
 │            │ MUST be registered on the connector BEFORE connect_finalize      │
 └────────────┼──────────────────────────────────────────────────────────────────┘
              ▼  RDP over TCP/TLS :3389 (NLA/CredSSP)
        REMOTE WINDOWS TARGET (Phase 1 VM) — DWM renders unconditionally;
        no Suppress Output PDU is ever sent, so updates never pause.
```

### Recommended Project Structure
```
.                              # repo root = cargo workspace (D-01)
├── Cargo.toml                 # [workspace] members = ["crates/rdpilot"]
├── Cargo.lock                 # committed (D-02)
└── crates/
    └── rdpilot/
        ├── Cargo.toml
        ├── src/
        │   ├── lib.rs            # pub use of Session, ConnectionConfig, Screenshot, Rect, Error
        │   ├── config.rs         # ConnectionConfig (+ optional env/file loaders, D-12/D-13)
        │   ├── error.rs          # thiserror enum (API-01)
        │   ├── connect.rs        # connect_begin → TLS → connect_finalize; cert policy (D-15)
        │   ├── session.rs        # Session handle, spawn task, close(), Drop guard (D-04/D-07)
        │   ├── session_loop.rs   # active_session: tokio::select! pump (mirrors ironrdp-client; named session_loop because `loop` is a Rust keyword)
        │   ├── framebuffer.rs    # shared latest DecodedImage snapshot
        │   ├── screenshot.rs     # Screenshot type: to_png(), crop() (D-09/D-11)
        │   └── keepalive.rs      # synthetic FastPathInputEvent emitter (D-06)
        ├── examples/
        │   └── screenshot.rs  # tiny binary: load .secrets/connection.json → connect → save png
        └── tests/
            └── live_session.rs # gated integration suite (D-16/D-17/D-18)
```

### Pattern 1: Connect + NLA/CredSSP (SESS-01)
**What:** Drive the IronRDP connector state machine to an active session over TLS with CredSSP.
**When to use:** Once, in `Session::connect`.
**Async sequence** (from `ironrdp-client/src/rdp.rs`, the canonical async path):
```rust
// Source: github.com/Devolutions/IronRDP crates/ironrdp-client/src/rdp.rs (live 2026-06-05)
let mut framed = ironrdp_tokio::TokioFramed::new(tcp_stream);
let mut connector = ironrdp::connector::ClientConnector::new(config, client_addr);
// >>> Phase 4 seam: register the RDPILOT_SENSOR DvcProcessor on `connector` HERE,
//     before connect_begin/finalize — registration is impossible after the session is active.
let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector).await?;
// TLS upgrade: build rustls ClientConfig (custom cert verifier per D-15, resumption disabled),
// wrap the stream, extract the server public key from the peer cert (x509-cert DER decode).
let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
let mut upgraded_framed = ironrdp_tokio::TokioFramed::new_with_leftover(erased_stream, leftover);
let mut network_client = sspi::network_client::reqwest_network_client::ReqwestNetworkClient;
let connection_result = ironrdp_tokio::connect_finalize(
    upgraded, connector, &mut upgraded_framed,
    &mut network_client, server_name.into(), server_public_key, None,
).await?;   // ConnectionResult carries desktop_size, compression_type, channel IDs
```
**Config (from `screenshot.rs::build_config`):** `connector::Config { credentials: Credentials::UsernamePassword { username, password }, domain, enable_tls: false, enable_credssp: true, desktop_size: DesktopSize { width, height }, keyboard_type: KeyboardType::IbmEnhanced, … performance_flags, compression_type, … }`. Note `enable_tls: false` in the example means "no frontend TLS re-export" — the wire **is** TLS via the rustls upgrade above; `enable_credssp: true` is what does NLA.

### Pattern 2: SDK-owned async session loop (SESS-02, D-04)
**What:** A spawned Tokio task that pumps PDUs, maintains the framebuffer, and accepts input/control via an mpsc channel.
**When to use:** The heart of `Session`. Mirror `ironrdp-client::active_session`.
```rust
// Source: github.com/Devolutions/IronRDP crates/ironrdp-client/src/rdp.rs (live 2026-06-05)
let (mut reader, mut writer) = ironrdp_tokio::split_tokio_framed(framed);
let mut image = DecodedImage::new(PixelFormat::RgbA32,
    connection_result.desktop_size.width, connection_result.desktop_size.height);
let mut active_stage = ActiveStage::new(connection_result);
loop {
    let outputs = tokio::select! {
        frame = reader.read_pdu() => {
            let (action, payload) = frame?;
            active_stage.process(&mut image, action, &payload)?
        }
        input_event = input_rx.recv() => match input_event {
            RdpInputEvent::FastPath(events) => active_stage.process_fastpath_input(&mut image, &events)?,
            RdpInputEvent::Close          => active_stage.graceful_shutdown()?,
        },
        _ = keepalive_interval.tick() => { /* push a synthetic null FastPath via input_rx, see Pattern 3 */ Vec::new() }
    };
    for out in outputs {
        match out {
            ActiveStageOutput::ResponseFrame(f) => writer.write_all(&f).await?,
            ActiveStageOutput::GraphicsUpdate(_region) => { /* snapshot image.data() into shared state */ }
            ActiveStageOutput::DeactivateAll(seq)       => { /* run reactivation, see Pattern 5 */ }
            ActiveStageOutput::Terminate(reason)        => break,
            _ => {} // Pointer* variants: ignore in Phase 2 (no GUI; enable_server_pointer: false)
        }
    }
}
```
**Critical:** `split_tokio_framed` gives **independent** reader and writer so the keepalive/close path can write while the reader awaits PDUs. The loop owns the only mutating `DecodedImage`; `screenshot()` reads a snapshot, never the live image directly (no shared mutable borrow across the await).

### Pattern 3: Automatic keepalive (SESS-02, D-06, Pitfall M1)
**What:** Every ~60 s, emit a benign synthetic input so the server's idle timer never fires.
**When to use:** Always, while the `Session` is alive — no opt-in.
**How:** A `tokio::time::interval(Duration::from_secs(60))` arm in the `select!` (or a separate task feeding `input_rx`) that pushes a no-op `FastPathInputEvent` — e.g. a mouse-move to the current/cursor position, or a sync event — through `process_fastpath_input`. Use `ironrdp-input`'s `Database`/`Operation` to build the event so it is a well-formed PDU. Avoid keystrokes (could leak into a focused field); a zero-delta pointer move is the safest null input. **Interval must be shorter than the target's idle timeout** (default lab VM has none, but 60 s is the safe figure from PITFALLS M1).

### Pattern 4: Full-desktop screenshot → PNG (CAP-01, D-09/D-11)
**What:** Convert the latest RGBA32 framebuffer to PNG bytes.
**When to use:** `screenshot()` / `Screenshot::to_png()`.
```rust
// Source: github.com/Devolutions/IronRDP crates/ironrdp/examples/screenshot.rs (live 2026-06-05)
let img: image::ImageBuffer<image::Rgba<u8>, _> =
    image::ImageBuffer::from_raw(width, height, rgba_bytes /* image.data() */)?;
// to PNG bytes (D-11): write to an in-memory Cursor<Vec<u8>> with PngEncoder, or img.write_to(...)
```
The framebuffer is **`PixelFormat::RgbA32`** — already correct-color RGBA, byte order R,G,B,A, 4 bytes/pixel, no stride padding (tight `width*4`). **No YUV→RGB conversion is required** on the bitmap / RLE / RDP6 / RemoteFX paths IronRDP decodes (resolves the criterion-#2 YUV-grey concern). See Pitfall "YUV-grey" below for the one residual risk (EGFX/H.264).

### Pattern 5: Deactivation-Reactivation handling (correctness for criterion #4)
**What:** The server can send a Deactivate-All PDU (e.g. on a resolution change), after which the session must run a reactivation sequence or the framebuffer goes stale/wrong-size.
**When to use:** Handle `ActiveStageOutput::DeactivateAll(connection_activation)`.
```rust
// Source: ironrdp-client/src/rdp.rs — Deactivation-Reactivation Sequence
loop {
    let written = single_sequence_step_read(&mut reader, &mut *connection_activation, &mut buf).await?;
    if written.size().is_some() { writer.write_all(buf.filled()).await?; }
    if let ConnectionActivationState::Finalized { desktop_size, io_channel_id, user_channel_id,
        share_id, enable_server_pointer, pointer_software_rendering } =
        connection_activation.connection_activation_state()
    {
        image = DecodedImage::new(PixelFormat::RgbA32, desktop_size.width, desktop_size.height);
        active_stage.set_fastpath_processor(fast_path::ProcessorBuilder { /* new channel IDs */ }.build());
        active_stage.set_share_id(share_id);
        break;
    }
}
```
This is why the loop must **own** the `image` (it gets replaced on resize). Without this, a server-side resolution change during the 10-min idle window would leave `screenshot()` returning a stale-size or blank buffer — directly threatening criterion #4.

### Pattern 6: Teardown — graceful close + Drop guard (D-07)
**What:** `close().await` sends `RdpInputEvent::Close` → `active_stage.graceful_shutdown()?` (emits a graceful disconnect), awaits the task to finish, and drops the socket. The `Drop` impl on the `Session` handle aborts the spawned task's `JoinHandle` and closes the channel if `close()` was not called.
**Note:** `graceful_shutdown()` returns frames to write before the loop exits on `Terminate`. The `Drop` guard is best-effort (cannot `await`), so it should `abort()` the task and let the OS reap the TCP socket — `close()` is the clean path.

### Anti-Patterns to Avoid
- **Sharing the live `DecodedImage` across `.await` to satisfy `screenshot()`:** holds a lock across the network await and risks deadlock/stale borrow. Instead snapshot RGBA bytes into an `Arc<Mutex<…>>`/`watch` on each `GraphicsUpdate`.
- **Depending on `RemoteDesktop_SuppressWhenMinimized` at runtime in Phase 2:** it's an mstsc-only key; our client never emits a Suppress Output PDU. Verify the effect behaviorally (D-08), do not read the remote registry, do not introduce WinRM.
- **Pinning `ironrdp-* = "0.14"` uniformly:** member crates are at 0.9/0.8/0.6/0.2 — uniform 0.14 pins will not resolve. Use the umbrella `ironrdp = "0.15"` + real member versions.
- **Leaking `anyhow`/`image`/`ironrdp` types in the public API:** D-09 requires owned SDK types (`Screenshot`, `Rect`, `Error`). Keep third-party types internal for clean semver and future PyO3/MCP marshalling.
- **Building input keepalive from keystrokes:** could inject characters into a focused remote field. Use a zero-delta pointer move.
- **Registering the DVC channel after `connect_finalize`:** impossible — must be on the connector before connection completes (hard IronRDP constraint). Phase 2 leaves the seam; do not exercise it.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| RDP protocol / NLA / CredSSP | A handshake or CredSSP impl | `ironrdp-connector` + `sspi` | Protocol-level correctness, security-reviewed, production-used (Devolutions, Teleport) |
| Async framing of RDP PDUs | Manual length-prefix reader | `ironrdp-tokio::TokioFramed` + `read_pdu` | Handles fast-path/slow-path framing and partial reads |
| Graphics codec decode (RLE/RDP6/RemoteFX) | A bitmap decoder | `ActiveStage::process` → `DecodedImage` | Auto-selects the codec per PDU; outputs RGBA32 |
| YUV→RGB color conversion | A SIMD color converter | `DecodedImage` (already RGBA32) | Native correct-color output on the standard paths; conversion only needed if you opt into EGFX/H.264 (avoid for Phase 2) |
| PNG encoding | A PNG writer | `image` crate | Battle-tested; D-11 keeps it internal |
| TLS for RDP | Anything OpenSSL | `rustls` (resumption disabled) | Memory-safe, no system dep; CredSSP needs resumption off |
| Deactivation-reactivation state machine | Custom resize handling | `single_sequence_step_read` + `ConnectionActivationState` | IronRDP drives the exact MS-RDPBCGR sequence |

**Key insight:** Almost everything Phase 2 needs is already a single upstream function call. The SDK's real work is *composition and lifecycle* (owning the loop, exposing a clean handle, snapshotting the framebuffer, gating tests) — not protocol code.

## Common Pitfalls

### Pitfall 1: YUV-grey / scrambled-color screenshot (criterion #2)
**What goes wrong:** Screenshots come back grey (luma only) or color-banded.
**Why it happens:** Only if the server negotiates the EGFX/H.264 (AVC420/AVC444) graphics pipeline, whose decoded surface is YUV, and the code reads it as RGB (PITFALLS m2). The standard bitmap / RLE / RDP6 / RemoteFX paths IronRDP decodes into `DecodedImage` are **already RGBA32** — no conversion needed.
**How to avoid:** Do **not** opt into the EGFX/H.264 pipeline for Phase 2 (the example config does not). Let the server use bitmap/RemoteFX. Then `PixelFormat::RgbA32` is correct by construction.
**Warning signs:** grey/banded image despite a live session. **Verification step (build into the test suite):** assert a known pixel's RGB on a controlled remote desktop (e.g. a solid-color region) — this is the criterion-#2 acceptance check.

### Pitfall 2: Stale or zero-size framebuffer after server resize/reactivation
**What goes wrong:** After ~minutes idle, `screenshot()` returns a blank or wrong-resolution image.
**Why it happens:** A server `DeactivateAll` (resolution/share change) was not handled, so the `DecodedImage` is the old size or never repainted.
**How to avoid:** Implement Pattern 5. Replace `image` on `Finalized { desktop_size }` and rebuild the fast-path processor.
**Warning signs:** `image.width()/height()` no longer match the server; `GraphicsUpdate` events stop arriving.

### Pitfall 3: `screenshot()` deadlock / borrow conflict with the loop
**What goes wrong:** `screenshot()` hangs or won't compile because the loop owns `&mut DecodedImage`.
**Why it happens:** Trying to share the live `DecodedImage` between the task and the API.
**How to avoid:** Snapshot RGBA bytes + dims into shared state (`Arc<Mutex<FrameSnapshot>>` or a `tokio::sync::watch`) on each `GraphicsUpdate`. `screenshot()` clones the latest snapshot — never touches the live image.
**Warning signs:** lock held across `.await`; `screenshot()` latency tracks frame cadence.

### Pitfall 4: Keepalive too infrequent or wrong event type
**What goes wrong:** Idle disconnect at the timeout boundary, or stray characters appear remotely.
**Why it happens:** Interval ≥ idle timeout, or keepalive built from keystrokes.
**How to avoid:** 60 s interval (< any realistic idle timeout), zero-delta pointer move as the null event (Pattern 3).
**Warning signs:** disconnect exactly at N minutes; unexpected input in a focused field.

### Pitfall 5: CredSSP TLS resumption / cert-trust failure on connect (C3)
**What goes wrong:** Connection hangs at "Securing remote connection" or fails after TLS.
**Why it happens:** TLS resumption left enabled (CredSSP forbids it), or self-signed lab cert rejected by default verifier.
**How to avoid:** `config.resumption = Resumption::disabled()` (the example does this with a cited MS-CSSP reference). For the lab VM (D-15) wire a custom `ServerCertVerifier` behind the `accept_invalid_certs` flag — default path keeps normal validation.
**Warning signs:** handshake stall; "peer certificate is missing"; CredSSP errors.

### Pitfall 6: Wrong crate versions / non-resolving Cargo.toml
**What goes wrong:** `cargo build` fails to resolve `ironrdp-* = "0.14"`.
**Why it happens:** STACK.md's install block predates the split versioning; members are 0.9/0.8/0.6/0.2, umbrella is 0.15.
**How to avoid:** Use the corrected Installation block above. Commit `Cargo.lock` (D-02) so the resolved graph is reproducible.

## Runtime State Inventory

> Phase 2 is greenfield Rust code — **not** a rename/refactor/migration. This section is included only to record the one external-state touchpoint the SDK consumes (it creates none of its own persistent state).

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | None — the SDK persists nothing | None |
| Live service config | The **remote VM** carries `RemoteDesktop_SuppressWhenMinimized=2` (set by Phase 1) — but **Phase 2 does not depend on it** (mstsc-only key). | None for Phase 2 — verify the rendered-while-idle *effect* behaviorally (D-08) |
| OS-registered state | None created by Phase 2 | None |
| Secrets/env vars | Consumes `.secrets/connection.json` (Phase 1 output) at test time; library itself is env-agnostic (D-12). **Do not read its contents into context** (secrets rule). | None — pass through to `ConnectionConfig` |
| Build artifacts | First Rust build: `/target/` (gitignored), `Cargo.lock` (committed, D-02) | Commit `Cargo.lock` alongside `Cargo.toml` |

## Code Examples

### Build the rustls config with the D-15 cert policy
```rust
// Source: github.com/Devolutions/IronRDP crates/ironrdp/examples/screenshot.rs (tls_upgrade)
let mut config = rustls::client::ClientConfig::builder()
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(/* NoCertificateVerification when accept_invalid_certs */))
    .with_no_client_auth();
config.resumption = rustls::client::Resumption::disabled(); // CredSSP requirement (MS-CSSP)
```
For the **default** (validate) path, build the config with the platform root store instead of the custom verifier; gate the custom verifier behind `ConnectionConfig::accept_invalid_certs`.

### Extract server public key for CredSSP (feeds `connect_finalize`)
```rust
// Source: screenshot.rs extract_tls_server_public_key
let cert = x509_cert::Certificate::from_der(peer_cert_der)?;
let server_public_key = cert.tbs_certificate.subject_public_key_info
    .subject_public_key.as_bytes().context("BIT STRING not aligned")?.to_owned();
```

### DecodedImage construction (note the exact pixel format)
```rust
// Source: screenshot.rs + ironrdp-client/src/rdp.rs
let mut image = DecodedImage::new(
    ironrdp::graphics::image_processing::PixelFormat::RgbA32, // R,G,B,A 8-bit each
    connection_result.desktop_size.width,
    connection_result.desktop_size.height,
);
// image.data() -> &[u8] RGBA32, tightly packed width*height*4
```

### Crop math (D-10) for `Screenshot::crop(rect)`
```rust
// rgba is width*height*4, row-major, 4 bytes/pixel, no padding.
fn crop(rgba: &[u8], w: u32, r: Rect) -> Vec<u8> {
    let mut out = Vec::with_capacity((r.w * r.h * 4) as usize);
    for y in r.y..r.y + r.h {
        let row = ((y * w + r.x) * 4) as usize;
        out.extend_from_slice(&rgba[row..row + (r.w * 4) as usize]);
    }
    out
}
// Caller must clamp r to image bounds (return an Error on out-of-bounds, not panic — API-01).
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `ironrdp* = "0.14"` uniform pins (CLAUDE.md / STACK.md) | Umbrella `ironrdp = "0.15"`, members at independent versions (0.9/0.8/0.6/0.2) | Between 2026-01 research and 2026-06 | Install block in STACK.md will not resolve — use corrected versions |
| Blocking `ironrdp-blocking` (screenshot.rs) | Async `ironrdp-tokio` (ironrdp-client) for an SDK-owned loop | n/a (both maintained) | D-04 needs async; use the `ironrdp-client` loop as the template |
| Assume `RemoteDesktop_SuppressWhenMinimized` is the rendered-while-minimized mechanism | Recognize it as mstsc-only; a headless client satisfies the property by never sending a Suppress Output PDU | clarified this research | Criterion #4 = behavioral verification, no registry/WinRM dependency in Phase 2 |

**Deprecated/outdated:**
- The version table in `.planning/research/STACK.md` and `CLAUDE.md` ("IronRDP 0.14.0"). Treat the corrected table in this document as authoritative for Phase 2 planning.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | A zero-delta pointer-move `FastPathInputEvent` is sufficient to reset the server idle timer | Pattern 3 / Pitfall 4 | If the server only counts "real" movement, keepalive could fail criterion #5; mitigated by validating during the 10-min idle test, and falling back to a tiny alternating ±1px move if needed |
| A2 | The lab VM negotiates bitmap/RLE/RDP6/RemoteFX (not EGFX/H.264), so `RgbA32` is correct-color without conversion | Pattern 4 / Pitfall 1 | If EGFX is forced server-side, screenshots could be YUV-grey; mitigated by the known-pixel color assertion in the test suite and by not enabling EGFX client-side |
| A3 | `connect_finalize`'s 7-arg async signature and `ReqwestNetworkClient` match between the blocking example and `ironrdp-tokio` 0.9 | Pattern 1 | If the async signature differs, connect code needs adjustment; low risk — both come from the same workspace release line; verify against `ironrdp-tokio` 0.9 docs at implementation time |
| A4 | `x509-cert` 0.2.x DER decode path is still how the public key is extracted in the current example | Code Examples | If IronRDP added a helper that does this, our code is merely more verbose, not wrong |
| A5 | `ironrdp-tls` can be skipped in favor of hand-building the rustls config | Standard Stack / Alternatives | None functional — both work; hand-building is needed for the D-15 custom verifier regardless |

## Open Questions (RESOLVED)

1. **Does the lab VM expose an idle timeout at all?**
   - What we know: Phase 1 provisioned the VM; STATE.md does not mention an idle-timeout policy.
   - What's unclear: whether criterion #5 (10-min keepalive) is exercising a real timeout or a no-op.
   - Recommendation: Keepalive runs unconditionally (D-06) regardless; the test still asserts the session is alive after 10 min. If the VM has no idle timeout, the test proves keepalive does no harm and the session survives — still a valid pass. Optionally have Phase 1 set a short idle timeout to make the test meaningful (out of Phase 2 scope; note for the planner).
   - RESOLVED: keepalive runs unconditionally; the 10-min live test (Plan 03) asserts liveness regardless of whether the VM enforces a timeout. Absorbed into Plan 02 (keepalive) + Plan 03 (live suite).

2. **`enable_server_pointer: false` vs. handling Pointer* outputs.**
   - What we know: the screenshot example sets `enable_server_pointer: false` (no GUI, no cursor needed).
   - What's unclear: whether a visible remote cursor must appear in screenshots for downstream phases.
   - Recommendation: keep `enable_server_pointer: false` for Phase 2 (simpler, matches the example). Revisit when an AI consumer needs to see the cursor.
   - RESOLVED: keep `enable_server_pointer: false` for Phase 2. Absorbed into Plan 02 Task 1 (connect config) and the config.rs pattern.

3. **Exact async `connect_finalize` signature in `ironrdp-tokio` 0.9.**
   - What we know: `ironrdp-client/src/rdp.rs` calls `ironrdp_tokio::connect_finalize(...)` with the upgraded framed + network client + server name + public key.
   - What's unclear: argument order vs. the blocking variant.
   - Recommendation: confirm against `docs.rs/ironrdp-tokio/0.9` when wiring connect.rs (the planner should add a "verify signature" sub-step, not a research blocker).
   - RESOLVED: verify the signature against `docs.rs/ironrdp-tokio/0.9` at implementation time — captured as a Plan 02 Task 1 sub-step (not a research blocker).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust toolchain (rustc/cargo) | All build/test | ✗ | — | Install via `scoop install rustup` then `rustup default stable` (D-19) — **blocking until installed** |
| `scoop` | Installing Rust (D-19) | ✓ | (present in PATH) | — |
| Live Windows RDP target | Integration suite (D-16/D-17) | provisioned on demand | — | `manage-env.ps1 up` (Phase 1); tests gated/skipped when absent (D-18) |
| `.secrets/connection.json` | Test credentials/host (D-13) | produced by Phase 1 `up` | — | Tests skip when missing (D-18) |
| Network egress to crates.io | First `cargo build` | ✓ | — | — |

**Missing dependencies with no fallback:**
- **Rust toolchain** — must be installed via `scoop` before any Phase 2 build/test (D-19). The planner's first task/wave must install it (e.g. `scoop install rustup`, then `rustup toolchain install stable`, verify `rustc --version` ≥ 1.78).

**Missing dependencies with fallback:**
- Live VM + `.secrets/connection.json` — absent by default; integration tests must be gated so `cargo test` passes without them (D-18). The canonical validation run provisions the VM first.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness (`#[test]` / `#[tokio::test]`); integration tests in `crates/rdpilot/tests/`. Add `tokio` (test feature) and optionally `serial_test` if live tests must not run concurrently against one VM. |
| Config file | `crates/rdpilot/Cargo.toml` `[dev-dependencies]` + `[[test]]` if needed — see Wave 0 |
| Quick run command | `cargo test -p rdpilot` (unit + gated-skipped integration when no target) |
| Full suite command | `RDPILOT_LIVE=1 cargo test -p rdpilot -- --include-ignored --test-threads=1` (full 10-min idle included) |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SESS-01 | Connect + authenticate (NLA/CredSSP) reaches active session | integration (live) | `cargo test -p rdpilot --test live_session connect_authenticates -- --ignored` | ❌ Wave 0 |
| CAP-01 #2 | Full-desktop PNG with correct RGB (known-pixel assertion) | integration (live) | `... screenshot_is_rgb_correct -- --ignored` | ❌ Wave 0 |
| CAP-01 #3 | Crop-to-rect produces expected sub-image dims/pixels | unit + integration | `cargo test -p rdpilot crop_` (unit) / `... screenshot_crop -- --ignored` | ❌ Wave 0 |
| SESS-02 #4 | Windowless session stays full-res, non-blank after idle | integration (live, long) | `... stays_rendered_while_idle -- --ignored` | ❌ Wave 0 |
| SESS-02 #5 | 10-min keepalive prevents idle disconnect | integration (live, long) | `... keepalive_survives_10min -- --ignored` | ❌ Wave 0 |
| (cross) | `cargo test` green with **no** live target (D-18) | unit + gating logic | `cargo test -p rdpilot` (no env) | ❌ Wave 0 |

**Gating mechanism (D-18):** mark live tests `#[ignore]` and/or early-return when `RDPILOT_LIVE`/`.secrets/connection.json` is absent, so the default `cargo test` never hard-fails. The canonical run sets the env var and `--include-ignored`. Crop unit tests need **no** live target (pure buffer math) — keep them in the default run.

### Sampling Rate
- **Per task commit:** `cargo test -p rdpilot` (unit + crop math; no VM cost)
- **Per wave merge:** `cargo test -p rdpilot` + a short-idle live smoke (parameterized idle, not full 10 min)
- **Phase gate:** Full suite with full 10-min idle green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/rdpilot/Cargo.toml` `[dev-dependencies]` — `tokio` (macros, rt), optionally `serial_test`
- [ ] `crates/rdpilot/tests/live_session.rs` — covers SESS-01, CAP-01, SESS-02
- [ ] `crates/rdpilot/tests/crop.rs` (or unit `#[cfg(test)]` in `screenshot.rs`) — pure crop-math, runs without a VM
- [ ] A test helper that loads `.secrets/connection.json` into `ConnectionConfig` and returns `None`/skips when absent (D-18 gating)
- [ ] Rust toolchain install step (`scoop install rustup`) — Wave 0 prerequisite (D-19); without it nothing builds

## Security Domain

> `security_enforcement: true`, `security_asvs_level: 1`, `security_block_on: high`. Phase 2 is a network client establishing an authenticated, encrypted RDP session with credentials — security is in scope.

### Applicable ASVS Categories
| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | NLA/CredSSP via `enable_credssp: true` + `sspi`; credentials passed as params, never stored by the lib (D-14) |
| V3 Session Management | yes | Single `Session` per process; explicit `close()` + `Drop` teardown (D-07); graceful disconnect |
| V4 Access Control | no | No multi-user/authorization surface in the SDK transport layer |
| V5 Input Validation | yes | Crop `Rect` must be bounds-validated (return `Error`, never panic — API-01); config values validated |
| V6 Cryptography | yes | TLS via `rustls` (never hand-rolled); resumption disabled per CredSSP; **D-15 cert policy** — default validate, explicit risk-named opt-out for lab |
| V7 Error/Logging | yes | `tracing` default-quiet; **never log credentials or cert material** (secrets rule, PITFALLS m3) |
| V9 Communications | yes | All RDP traffic over TLS on 3389; verify the channel is TLS-upgraded before `connect_finalize` |

### Known Threat Patterns for {pure-Rust IronRDP client over TLS/CredSSP}
| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Accept-invalid-certs left on in production (MITM) | Spoofing/Tampering | `accept_invalid_certs` is **opt-in, risk-named, default off** (D-15); document lab-only use; pinning deferred but noted |
| Credentials leaking into logs / crash dumps | Information Disclosure | Never `tracing`-log credential fields; do not `Read` `.secrets/connection.json` into context; zero/secure handling where practical (PITFALLS m3) |
| Credentials in committed config | Information Disclosure | Library is env-agnostic (D-12); `.secrets/` gitignored; test config is the caller's path |
| TLS downgrade / resumption oracle (CredSSP) | Tampering | `Resumption::disabled()`; negotiate TLS 1.2+ (rustls default) |
| Crop/coords panic on malformed `Rect` (DoS) | Denial of Service | Bounds-check and return `Error`; no `unwrap`/`expect` in lib code (API-01) |
| Stale framebuffer served as "current" | (integrity of perception) | Snapshot-on-`GraphicsUpdate`; handle `DeactivateAll` (Pattern 5) so size/content stay valid |

## Sources

### Primary (HIGH confidence)
- `Devolutions/IronRDP` — `crates/ironrdp/examples/screenshot.rs` (live source fetched 2026-06-05): connect flow, `connector::Config`, `DecodedImage::new(PixelFormat::RgbA32, …)`, TLS upgrade, cert key extraction, PNG via `image`
- `Devolutions/IronRDP` — `crates/ironrdp-client/src/rdp.rs` (live source 2026-06-05): async `active_session` loop, `TokioFramed`, `split_tokio_framed`, `single_sequence_step_read`, `tokio::select!` over reader + input channel, `process_fastpath_input`, `graceful_shutdown`, DeactivateAll reactivation
- `Devolutions/IronRDP` — `crates/ironrdp-input/src/lib.rs` (live 2026-06-05): `MouseButton`, `Scancode`, `FastPathInputEvent` construction
- crates.io sparse index `index.crates.io` (2026-06-05): all crate versions verified
- docs.rs `ironrdp-session` `ActiveStageOutput` enum: full variant list (ResponseFrame, GraphicsUpdate, Pointer*, Terminate, DeactivateAll, MultitransportRequest, AutoDetect) — crate 0.9.0
- [MS-RDPBCGR Client Suppress Output PDU](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/79202daa-76b8-4a09-ac67-fc3cae7d895f) and [TS_SUPPRESS_OUTPUT_PDU](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/0be71491-0b01-402c-947d-080706ccf91b): confirms Suppress Output is client→server, mstsc-on-minimize behavior
- [MS-CSSP — no TLS resumption](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/385a7489-d46b-464c-b224-f7340e308a5c): why resumption must be disabled

### Secondary (MEDIUM confidence)
- docs.rs `ironrdp-tokio` (0.9.0): public API names (`TokioFramed`, `connect_begin/finalize`, `single_sequence_step_read`, `split_tokio_framed`) — signatures partially documented; confirm exact arg order at implementation time
- Project research: `.planning/research/STACK.md`, `ARCHITECTURE.md`, `PITFALLS.md` (2026-06-04) — domain context, pitfalls C1–C6/M1/m2/m3 (version table superseded here)

### Tertiary (LOW confidence)
- General WebSearch for IronRDP async patterns (returned mostly non-IronRDP results) — superseded by direct source fetches above

## Metadata

**Confidence breakdown:**
- Standard stack (versions): HIGH — verified against crates.io sparse index 2026-06-05 and live example imports
- Connect/auth flow (SESS-01): HIGH — full source of both blocking and async connect paths fetched
- Session loop / keepalive / teardown (SESS-02): HIGH — async `active_session` loop is the exact template; keepalive event choice is MEDIUM (A1)
- Framebuffer → PNG (CAP-01): HIGH — `PixelFormat::RgbA32` + `image::ImageBuffer` confirmed in source
- Stays-rendered mechanism (criterion #4): HIGH — Suppress Output PDU semantics confirmed via MS-RDPBCGR; satisfied by construction
- Color correctness (criterion #2): HIGH for the bitmap/RemoteFX path; MEDIUM residual on EGFX (A2, mitigated by test assertion)

**Research date:** 2026-06-05
**Valid until:** 2026-07-05 for stack versions (IronRDP releases weekly — re-verify crate versions at implementation time and pin via committed `Cargo.lock`). Architecture/patterns valid longer (stable API shape).
