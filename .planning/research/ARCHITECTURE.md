# Architecture Research: v1.1 Consumer Surfaces

**Domain:** Session daemon + CLI + MCP server + bidirectional file transfer, integrating with an existing Rust RDP-perception SDK
**Researched:** 2026-07-10
**Confidence:** HIGH for integration points grounded directly in v1.0 source (`Session`, `session_loop`, `connect.rs`, `rdpdr_backend.rs`); MEDIUM for external library choices (rmcp, `interprocess`) verified via Context7/official docs; flagged LOW/MEDIUM inline where a claim rests on a single web source.

This is a **subsequent-milestone** research pass. v1.0 (SDK core + sensor) is DONE and is treated as fixed ground truth, not re-researched. Everything below is about how the NEW v1.1 surfaces plug into that existing, already-shipped architecture.

## Existing Architecture (Ground Truth, v1.0 — Not Re-Researched)

This section is a factual recap of what already exists, pulled directly from `crates/rdpilot/src/*.rs`, because every v1.1 integration point is a delta against it.

- **`Session`** (`session.rs`) is the sole public handle. `Session::connect()` runs the IronRDP connect sequence, then spawns a **dedicated OS thread** running a **current-thread Tokio runtime** that drives `session_loop::run(...)` — deliberately NOT `tokio::spawn`, because the PDU-reactivation step holds a non-`Send` borrow across an `.await` (an HRTB limitation, not a design choice up for revisiting). The public API is 100% ordinary `async fn(&self, ...)` methods (`screenshot`, `send_mouse`, `send_key`, `ping`, `get_window_list`, `get_process_tree`, `get_uia_tree`, `world_state`, `set_foreground_window`, `launch_process`, `deploy_and_launch`, `close`). Every one of them just does `mpsc::Sender::send` into the dedicated thread's channel and awaits a `oneshot` reply — none of them need to *run on* the dedicated thread, only to talk to it. This means **`Session` is already safely callable from any ordinary multi-thread Tokio runtime task** with zero modification.
- `Session` holds `Mutex<Database>` (std, sync, never held across `.await`), `Arc<SensorShared>`, `mpsc::Sender`, `AtomicU64`, a `JoinHandle<Result<()>>`. All are `Send + Sync`; `Session` itself is `Send + Sync` by auto-trait inference. **This is the load-bearing fact for the daemon's registry design** (see Pattern 1 below).
- `Session::close(self)` **consumes** `self` (takes ownership, sends a graceful-shutdown message, then `spawn_blocking`-joins the dedicated thread). `Drop` is a best-effort fallback (drops the channel sender, does not join) for the case where a caller doesn't call `close()`. This ownership shape is a direct constraint on how the daemon's session registry must be built (Pattern 2).
- The DVC channel (`RDPILOT_SENSOR`, dynamic virtual channel to the sensor) and the RDPDR static channel (drive redirection) **must both be registered on the connector before `connect_begin()`** — registering a DVC after the session is active is impossible (a hard IronRDP constraint, already discovered and worked around in v1.0). This constrains file-transfer design: any RDPDR-based transfer channel must be wired at `connect()` time, not added mid-session.
- `RdpilotDriveBackend` (`rdpdr_backend.rs`) is a hand-rolled, crate-internal `RdpdrBackend` implementation (IronRDP ships zero filesystem backend on the `x86_64-pc-windows-gnu` target). Today it is deliberately narrow: it serves **exactly one read-only file** (the sensor exe) under a hardcoded `served_name`, allow-listing only the drive root or that one name in `handle_create`, and **explicitly rejects `DeviceWriteRequest`** (`Self::reject_unsupported`). This is the single most important existing artifact for file-transfer design — see Pattern 4.
- The sensor already has a generic `req_id`-keyed request/reply pattern (`Session::sensor_request`, one non-`Version` match arm serves every reply type) with a `{success, data|error}` JSON envelope over the DVC channel, and the sensor was already used to trigger a remote-side file operation once before: `deploy_and_launch` opens Win+R and types a `cmd /c copy \\tsclient\RDPILOT\... && start ...` command. That precedent — "inject a remote-side command that acts on the RDPDR-redirected share" — is the direct ancestor of the recommended file-transfer design.
- `WindowInfo`, `ProcessInfo`, `UiaElement`, `WorldState` all derive `Serialize` only (D-09: owned SDK types, no third-party type leaks). `lib.rs`'s own doc comment says the public API is "ready for future PyO3/MCP marshalling" — the v1.0 authors already anticipated this milestone's shape. `Error` derives `thiserror::Error` but **not** `Serialize` — an IPC/wire adapter is needed (Pattern 3), not a change to the SDK's public `Error` type.
- Build/host reality: the operator's own workstation is ARM64 Windows (no MSVC), cross-compiling the *target* Windows binaries via `x86_64-pc-windows-gnu`. The daemon/CLI/MCP-server, however, are **local-workstation** processes — they run on whatever OS the operator's own machine uses (currently Windows, but the SDK itself is pure-Rust/cross-platform and nothing in v1.0 ties it to Windows-as-host). Design the IPC transport to be cross-platform-capable even though the immediate target is Windows named pipes.

## Standard Architecture

### System Overview

```
┌────────────────────────────────────────────────────────────────────────┐
│ LOCAL WORKSTATION (operator's machine)                                 │
│                                                                          │
│  ┌───────────────┐   ┌───────────────────┐                             │
│  │ rdpilot-cli   │   │ rdpilot-mcp       │   invoke-and-exit /          │
│  │ (thin client) │   │ (rmcp stdio proc, │   long-lived-per-chat        │
│  │               │   │  dual tool router)│                             │
│  └──────┬────────┘   └─────────┬─────────┘                             │
│         │ IPC (unix socket /   │ IPC (same protocol)                   │
│         │ named pipe, framed)  │                                       │
│         └──────────┬───────────┘                                       │
│                     ▼                                                  │
│         ┌─────────────────────────────────────────┐                    │
│         │ rdpilot-daemon (long-lived, multi-thread │                    │
│         │ Tokio runtime)                           │                    │
│         │  ┌───────────────────────────────────┐   │                    │
│         │  │ Session Registry                  │   │                    │
│         │  │  name-or-auto-id -> Arc<Session>   │   │                    │
│         │  └───────────────────────────────────┘   │                    │
│         │  IPC listener · idle-session reaper ·     │                    │
│         │  idle-daemon self-shutdown · auto-start   │                    │
│         │  bootstrap entrypoint                     │                    │
│         └───────────────┬───────────────────────────┘                    │
│                          │ each Arc<Session> internally owns:            │
│                          │  dedicated OS thread + current-thread Tokio   │
│                          │  runtime (UNCHANGED from v1.0, per-session)   │
└──────────────────────────┼────────────────────────────────────────────┘
                           │ RDP (TLS/CredSSP) + DVC (RDPILOT_SENSOR)
                           │ + RDPDR static channel (drive redirection)
                           ▼
┌────────────────────────────────────────────────────────────────────────┐
│ REMOTE WINDOWS TARGET                                                  │
│  RDP session · rdpilot-sensor.exe (C# NativeAOT, DONE in v1.0)         │
│  — extended in v1.1 with UploadFile/DownloadFile sensor commands       │
└────────────────────────────────────────────────────────────────────────┘
```

### Component Responsibilities

| Component | Responsibility | Status |
|-----------|----------------|--------|
| `crates/rdpilot` (SDK) | `Session`, connect, session loop, DVC/RDPDR, sensor protocol | DONE (v1.0) — extended, not rebuilt, for file transfer |
| `RdpilotDriveBackend` | RDPDR static-channel filesystem backend | MODIFIED — generalize from one hardcoded file to an allow-listed, per-transfer set of files, add `Write` IRP support |
| Sensor (C#/.NET 8 NativeAOT) | Remote-side perception + actions over DVC | MODIFIED — add `UploadFile`/`DownloadFile` `MsgType`s that trigger a remote-side copy to/from the RDPDR-redirected share |
| `rdpilot-ipc` (new crate) | Shared wire protocol: request/response enums, session-id newtype, framing, transport bootstrap (connect-or-autostart) | NEW |
| `rdpilot-daemon` (new bin) | Session registry, IPC server, lifecycle (idle reaper, auto-start, self-shutdown) | NEW |
| `rdpilot-cli` (new bin) | Thin invoke-and-exit client over `rdpilot-ipc` | NEW |
| `rdpilot-mcp` (new bin) | Long-lived `rmcp` stdio server, dual tool router, thin client over `rdpilot-ipc` | NEW |
| Layered config (file/env/flags) | Resolve host + credentials into a connect payload | NEW — small, shared by CLI + MCP only (daemon never parses config, it only receives already-resolved connect parameters) |

## Recommended Project Structure

```
crates/
├── rdpilot/                 # EXISTING SDK — extended, not restructured
│   └── src/rdpdr_backend.rs #   MODIFIED: multi-file allow-list + Write IRP
│   └── src/session.rs       #   MODIFIED: Session::upload_file/download_file
│   └── src/sensor.rs        #   MODIFIED: MsgType::UploadFile/DownloadFile
├── rdpilot-ipc/              # NEW — shared wire protocol + transport bootstrap
│   └── src/
│       ├── protocol.rs      # Request/Response enums, SessionId, WireError
│       ├── framing.rs       # length-delimited codec wrapper
│       └── transport.rs     # cfg(unix)/cfg(windows) listen+connect+autostart
├── rdpilot-daemon/           # NEW — long-lived background service (bin)
│   └── src/
│       ├── registry.rs      # name-or-auto-id -> Arc<Session>, idle tracking
│       ├── server.rs        # accept loop, per-connection request dispatch
│       └── lifecycle.rs     # idle-session reaper, idle-daemon shutdown
├── rdpilot-cli/              # NEW — thin invoke-and-exit client (bin)
├── rdpilot-mcp/              # NEW — rmcp stdio server (bin)
└── rdpilot-config/           # NEW (small) — layered file+env+flag resolution
```

### Structure Rationale

- **`rdpilot-ipc` is its own crate**, not folded into the daemon, because both the daemon (server side) AND the CLI/MCP (client side) need the same request/response types and framing code — putting it in `rdpilot-daemon` would force the CLI/MCP binaries to depend on the entire daemon binary's code (registry, lifecycle) just to get wire types.
- **The SDK crate (`crates/rdpilot`) is extended, not wrapped.** The daemon calls `rdpilot::Session` methods directly — there is no adapter/facade layer needed beyond the wire (de)serialization in `rdpilot-ipc`, because `Session` is already `Send + Sync` and its methods are already plain `async fn(&self, ...)`.
- **`rdpilot-config` is separate from `rdpilot-ipc`** because config layering (file/env/flags → host+credentials) is a CLI/MCP-side concern only — the daemon receives an already-resolved connect payload in a `Connect` request and never reads a config file itself. This keeps the daemon config-format-agnostic and keeps `ConnectionConfig` construction out of the wire protocol crate.

## Architectural Patterns

### Pattern 1: Daemon-runtime / session-thread coexistence (no core SDK change needed)

**What:** The daemon process runs one ordinary `#[tokio::main]` (or `Runtime::new()`) **multi-thread** Tokio runtime. Every `Arc<Session>` in the registry was constructed by calling `Session::connect(&cfg).await` from an ordinary daemon task — this internally spawns `Session`'s own dedicated OS thread + current-thread runtime, exactly as it does today, completely opaque to the daemon. The daemon's own runtime threads never touch a `Session`'s internals; they only call its public `async fn` methods, which are cheap channel round-trips.

**When to use:** Always, for this daemon — there is no reason to change `Session`'s internal threading model. N sessions = N dedicated OS threads + the daemon's own thread pool (typically `num_cpus` threads) for everything else (IPC accept loop, per-request handlers, the idle reaper). This scales fine to the "a handful of concurrent sessions on one operator's machine" scale this project targets (explicitly not "multi-session orchestration at scale", per PROJECT.md Out of Scope).

**Trade-offs:** A dedicated OS thread per session (not just per SDK, per *session*) means memory/thread overhead scales linearly with concurrent sessions — acceptable for a personal-tooling daemon holding a small number of named sessions, not for hundreds.

### Pattern 2: Registry ownership shape respects `Session::close(self)`'s consuming signature

**What:** Store the registry as (conceptually) `tokio::sync::RwLock<HashMap<SessionId, Arc<Session>>>`. Because `Session::close(self)` consumes by value, a registry entry stored behind `Arc` cannot call `close()` directly while other clones might still be alive (e.g. a request in flight against that session). The recommended shutdown sequence for `Disconnect`/idle-reaping:

1. Write-lock the registry, `remove()` the entry, drop the lock.
2. `Arc::try_unwrap(arc)` — if it succeeds (no other clone outstanding, i.e. no in-flight request holds a reference), you have an owned `Session` and can `.close().await` it cleanly (graceful shutdown + joined thread).
3. If `try_unwrap` fails (a request is still in flight), just drop the `Arc` — `Session`'s own `Drop` impl (already implemented, v1.0) does the best-effort teardown: it drops the channel sender, the session-loop thread observes the channel close and exits on its own. This is a documented, intentional degrade path already built into the SDK — the daemon does not need to invent new drop semantics, just rely on the one that exists.

**When to use:** Every `Disconnect` request and every idle-reaper sweep.

**Trade-offs:** The "drop, don't await-close" fallback path (step 3) means a session under heavy concurrent use takes slightly longer to fully release its remote RDP connection than an idle one — acceptable, matches `Session`'s own documented Drop contract.

### Pattern 3: IPC as a thin IDL over `Session`'s existing async methods, with a wire-only error adapter

**What:** `rdpilot-ipc::protocol` defines one `Request` enum and one `Response` enum. Every session-scoped variant carries a `session: SessionId` field explicitly — **there is no "current session" concept, per the requirement**; only a small non-session-scoped subset (`Connect`, `ListSessions`, `DaemonStatus`, `Shutdown`) omits it. `Connect` returns the resolved `SessionId` (echoing the caller's requested name, or the daemon-generated auto-id if none was given) so the caller can use it in every subsequent call.

Framing: length-delimited frames (`tokio_util::codec::LengthDelimitedCodec`, HIGH confidence — standard, well-documented Tokio pattern) over whichever transport stream the platform gives you (`tokio::net::UnixStream` on Unix, `tokio::net::windows::named_pipe::{NamedPipeServer, NamedPipeClient}` on Windows). Payload is `serde_json`-encoded (matches the sensor's existing wire choice — `Value` envelopes with `serde_json`, already proven at 22-165ms round trips over a *slower* DVC channel; a local socket/pipe is materially faster, so JSON's overhead is a non-issue) or `bincode` if binary compactness matters later — start with JSON for consistency with the rest of the codebase and easy debuggability (`nc`/manual testing).

For large payloads (screenshots — up to several MB raw, less compressed), avoid base64-in-JSON: define the response envelope as `[1-byte kind][JSON metadata]` optionally followed by a second raw-bytes frame when the response carries binary data (screenshot PNG bytes, downloaded file bytes). This mirrors how the sensor's own envelope already separates control (`Envelope`) from payload.

`rdpilot::Error` derives `thiserror::Error` but not `Serialize` (deliberately, per D-09 — the SDK's public error type is not coupled to any wire format). `rdpilot-ipc` (or the daemon) owns a small `WireError { kind: String, message: String }` built via a `match`/`From` conversion at the daemon's request-dispatch boundary — this keeps the SDK crate's public surface untouched.

**When to use:** For every session-scoped and registry-scoped request.

**Trade-offs:** A hand-rolled protocol (vs. adopting gRPC/Cap'n Proto) is more code up front but keeps the dependency graph small and matches this project's existing "hand-roll the minimal thing" bias (the sensor protocol itself is hand-rolled JSON-over-DVC, not gRPC).

**Security note (local-only auth) — flagged, MEDIUM/verify further at implementation time:** Windows named pipes created without an explicit security descriptor default to a DACL that (per Windows named-pipe security documentation, and corroborated independently by a security-research write-up) grants **read access to the Everyone group and the anonymous account on the same machine**, not just the pipe's creator — i.e. any other local account could otherwise read/write your daemon's pipe. `tokio::net::windows::named_pipe::ServerOptions` exposes `create_with_security_attributes_raw` (HIGH confidence, docs.rs-verified) specifically to let a caller supply a custom `SECURITY_ATTRIBUTES`/DACL. **Recommendation: do not rely on the `interprocess` crate's default pipe creation for this daemon** — its cross-platform `local_socket` abstraction does not appear (based on available documentation) to expose ACL/security-descriptor configuration on the Windows leg. Instead, branch `cfg(windows)`/`cfg(unix)` directly: on Windows, build the pipe via `tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw`, constructing a DACL scoped to the current user's SID (+ SYSTEM) using the `windows` crate (already a proven dependency, used by the sensor side); on Unix, a plain `tokio::net::UnixListener` at a private per-user runtime path (`$XDG_RUNTIME_DIR/rdpilot/daemon.sock` or equivalent) with `0600` permissions is sufficient (standard Unix socket file-permission model). This is more code than depending on `interprocess`, but it is the only way to guarantee the "local-only" requirement actually holds on Windows.

### Pattern 4: RDPDR-generalized bidirectional file transfer, sensor-mediated

**What:** File transfer is **not** CLIPRDR (clipboard) — clipboard read/write is a separately-deferred capability (PROJECT.md still lists it as a distinct out-of-scope/v2 item) and is the wrong transport for arbitrary files anyway (CLIPRDR is oriented around clipboard formats, size/latency characteristics differ, and no v1.0 code touches it at all). File transfer instead **extends the existing RDPDR drive-redirection path** already proven in v1.0 for sensor deployment, plus **new sensor-mediated commands** that trigger the actual remote-side copy — combining two of the three options the question raises, because they solve two different halves of the problem:

- **RDPDR drive redirection = the passive transport.** Generalize `RdpilotDriveBackend` from "exactly one hardcoded read-only file" to an allow-listed set of named entries registered per active transfer (e.g. `register_upload(transfer_id, local_path)` for local→remote, `register_download(transfer_id) -> local_staging_path` for remote→local), still bounded by an explicit allow-list (never general filesystem access — preserves the existing T-05-04 security posture, just widened from "one static name" to "a dynamically registered, still-explicit set"). **This requires implementing `DeviceWriteRequest`**, which the current backend explicitly rejects (`Self::reject_unsupported`) — the remote→local (download) direction needs the remote side to *write* bytes into the RDPDR-redirected share, which the client-side backend must now accept and persist to a local staging file.
- **Sensor-mediated commands = the active trigger.** RDPDR redirection is entirely passive from the SDK's side — nothing happens until something running *on the remote* actually reads or writes through `\\tsclython\RDPILOT\...`. v1.0's only precedent for triggering remote-side action is `deploy_and_launch`'s Win+R-and-type hack, which is fragile (empirically needed multiple live-tuned timing fixes) and was only ever used once, for bootstrapping. For file transfer, **do not reuse the keystroke-injection hack** — instead add two new sensor `MsgType`s (`UploadFile`, `DownloadFile`) that the sensor executes directly as a `File.Copy`/`CopyFileW`-equivalent call between the RDPDR share and a real remote destination path, exactly mirroring the existing `LaunchProcess`/`SetForegroundWindow` request/reply shape (`{success, data|error}` over the DVC channel) that Phase 6 already established. This is far more reliable than input injection (no dialog timing, no focus dependency) and is a small, additive change to a sensor architecture already built for exactly this kind of typed request/response extension.

**Data flow — upload (local→remote):**
```
CLI/MCP → daemon: UploadFile{session, local_path, remote_dest}
daemon: Session::upload_file(local_path, remote_dest)
  1. RdpilotDriveBackend.register_upload(transfer_id, local_path)   [client-side, in-process]
  2. sensor_request(MsgType::UploadFile, {transfer_id, remote_dest})
     → sensor copies \\tsclient\RDPILOT\<transfer_id> → remote_dest
  3. sensor replies {success:true}; SDK deregisters the transfer entry
```

**Data flow — download (remote→local):**
```
CLI/MCP → daemon: DownloadFile{session, remote_src, local_dest}
daemon: Session::download_file(remote_src, local_dest)
  1. RdpilotDriveBackend.register_download(transfer_id) -> staging path
  2. sensor_request(MsgType::DownloadFile, {transfer_id, remote_src})
     → sensor copies remote_src → \\tsclient\RDPILOT\<transfer_id>
       (this is the DeviceWriteRequest path the backend must now accept)
  3. sensor replies {success:true}; SDK moves the staged bytes to local_dest
```

**When to use:** Both directions, for every file-transfer verb exposed by CLI/MCP.

**Trade-offs:** Reusing RDPDR (already-proven channel, already registered at connect time, no new virtual channel needed) is cheaper than adding a second transport, but couples file-transfer to the same connect-time-registration constraint as everything else DVC/RDPDR-related — a session that connected without file-transfer support enabled can't have it added mid-session (consistent with the existing sensor-deploy precedent, so this is not a new constraint, just an inherited one). Bounding transfers by an explicit allow-list registered per-request (not exposing the whole remote/local filesystem) keeps the existing security posture (T-05-04) intact.

### Pattern 5: Auto-start-with-explicit-override daemon lifecycle (sccache-style)

**What:** Both `rdpilot-cli` and `rdpilot-mcp` share one `rdpilot-ipc::ensure_daemon_running()` bootstrap helper: attempt to connect to the well-known socket/pipe; on a "not found"/connection-refused error, spawn the daemon binary detached (Unix: double-fork/`setsid`-equivalent via `std::process::Command` with `.process_group(0)`; Windows: `CREATE_NO_WINDOW`/`DETACHED_PROCESS` creation flags) and retry-connect with a short backoff. This exact pattern (silent auto-start on first client invocation, with an explicit override to force foreground/no-daemon mode, and a configurable idle self-shutdown timeout) is the well-established design used by `sccache` (MEDIUM confidence, verified via `sccache`'s own docs/GitHub: `SCCACHE_NO_DAEMON`, `SCCACHE_IDLE_TIMEOUT`, "if you do not start sccache explicitly, it will spin up its own daemon automatically") and is analogous to `adb`/`gpg-agent`/`ssh-agent`'s auto-start-on-first-use model.

Idle teardown operates at **two levels**, not one:
1. **Per-session idle reaping**: the daemon tracks a `last_activity: Instant` per registry entry, updated on every request; a background `tokio::time::interval` task periodically sweeps the registry and closes (Pattern 2) any session idle past a configurable threshold (e.g. default 30 min) — this matters because each live session holds an actual remote RDP connection + keepalive, which is a real remote-side resource, not just local memory.
2. **Whole-daemon idle self-shutdown**: if the registry has been *empty* for a configurable period (mirroring `sccache`'s `SCCACHE_IDLE_TIMEOUT`, default e.g. 10 min, `0` = never), the daemon process exits on its own — so a forgotten daemon doesn't linger forever, and the next CLI/MCP invocation transparently re-auto-starts it.

Provide an explicit `rdpilot-daemon` binary/subcommand (foreground mode, `--no-daemon`-equivalent, `status`, `stop`) for debugging and for operators who want to wire it into `systemd --user`/Windows Task Scheduler themselves instead of relying on auto-start.

**When to use:** Always — this is the default lifecycle model; explicit start is the escape hatch, not the primary path, matching the requirement's "auto-start vs explicit-start" framing as a spectrum, not a binary choice.

**Trade-offs:** Auto-start hides daemon-crash-loop failures behind a client-side retry loop unless the bootstrap helper surfaces the daemon's stderr/exit code on a failed retry — the CLI/MCP bootstrap helper should read back the spawned daemon's early failure (e.g. via a short-lived readiness pipe or by checking the daemon's own log file) rather than retry blindly forever.

## Data Flow

### CLI invoke-and-exit flow

```
rdpilot-cli screenshot --session my-vm
  → rdpilot-config: resolve nothing new (session already named, no connect needed)
  → rdpilot-ipc::ensure_daemon_running()  [connect, or spawn+retry]
  → send Request::Screenshot { session: "my-vm" }
  → daemon: registry.get("my-vm") -> Arc<Session> -> session.screenshot().await
  → daemon: PNG bytes framed as [JSON metadata][raw bytes] response
  → CLI: write PNG to stdout/file, process exits
```

### MCP tool-call flow (long-lived process, many tool calls per chat)

```
LLM host spawns rdpilot-mcp over stdio (rmcp transport-io)
  rdpilot-mcp connects ONCE to the daemon via rdpilot-ipc (persists for the process lifetime)
  For each tool call (e.g. Anthropic "computer" tool action=screenshot, or a native "rdpilot_get_uia_tree" tool):
    → map tool args (must include an explicit session id/name — no implicit default target)
    → send the corresponding Request over the already-open IPC connection
    → map Response back into the tool's MCP content block (image content for screenshots,
      structured JSON for get_window_list/get_uia_tree/world_state)
```

### File-transfer flow

See Pattern 4's two data-flow diagrams above (upload/download) — both surfaces (CLI `upload`/`download` verbs, MCP native `upload_file`/`download_file` tools) call the identical `Request::UploadFile`/`Request::DownloadFile` IPC messages; there is no surface-specific file-transfer logic, all of it lives in `Session::upload_file`/`download_file` plus the generalized `RdpilotDriveBackend`.

## Scaling Considerations

This project is explicitly personal tooling, not a multi-tenant service (PROJECT.md: "Multi-session orchestration / concurrency at scale... not exercised"). Scale axis here is "how many concurrent named sessions on one operator's machine", not "how many users".

| Scale | Architecture fit |
|-------|-------------------|
| 1 session | Trivial — registry is a `HashMap` with one entry; idle reaper mostly idle |
| 2-10 concurrent named sessions | Still fine — N dedicated OS threads (one per `Session`, unchanged from v1.0) plus the daemon's own small tokio thread pool; each session's RDP connection is independent, no shared bottleneck |
| Dozens+ concurrent sessions | Not a target for this milestone (Out of Scope) — if ever needed, the dedicated-thread-per-session model (not just per-SDK-instance) would need revisiting, since OS threads are not free at that scale; out of scope to design for now |

### Scaling Priorities

1. **First real limit**: remote-side resource pressure (each session is a full interactive RDP logon + keepalive on the target machine), not local daemon overhead — the idle-session reaper (Pattern 5) is the actual mitigation, not thread-pool tuning.
2. **Not a concern at this milestone's scale**: IPC throughput — local socket/named-pipe transport on one machine is not the bottleneck for screenshot-sized payloads at the request rates a single human/agent operator generates.

## Anti-Patterns

### Anti-Pattern 1: Embedding a second, MCP-server-owned `Session` registry

**What people might do:** Give `rdpilot-mcp` its own in-process `Session`/registry, bypassing the daemon entirely, reasoning "the MCP server is already long-lived, why route through another process?"

**Why it's wrong:** This directly contradicts the milestone's own stated requirement ("MCP server surface: ... a daemon client") and reintroduces exactly the problem the daemon exists to solve: a CLI invocation and an MCP tool call both need to be able to see and act on the *same* named session, from *different processes*, without either one owning the RDP connection exclusively. It also duplicates the idle-reaping/lifecycle logic in two places.

**Do this instead:** Both `rdpilot-cli` and `rdpilot-mcp` are thin `rdpilot-ipc` clients of the one shared `rdpilot-daemon`; neither links against `rdpilot::Session` construction directly (only against the shared value types for deserializing responses, if even that — prefer daemon-side-only mapping).

### Anti-Pattern 2: Reusing the Win+R keystroke-injection hack for file transfer

**What people might do:** Extend `deploy_and_launch`'s existing Win+R-and-type pattern to trigger file copies, since "it already works for the sensor exe".

**Why it's wrong:** That pattern needed multiple live-tuned timing workarounds in v1.0 (settle delays, chunked typing to defeat ComboBox autocomplete corruption) precisely because it drives the remote UI through input injection — fragile by construction, and unnecessary now that a running sensor process already exists on the target with a proven typed-request/reply channel.

**Do this instead:** Add `UploadFile`/`DownloadFile` as ordinary sensor `MsgType`s (Pattern 4) — a direct API call on the remote side, no UI, no timing dependency.

### Anti-Pattern 3: Reaching for CLIPRDR for file transfer

**What people might do:** Treat "clipboard" and "file transfer" as the same deferred capability and try to satisfy both with one CLIPRDR implementation.

**Why it's wrong:** CLIPRDR is clipboard-format-oriented (text/bitmap/file-list clipboard formats), was never touched in v1.0, and is tracked as its own separate deferred item — conflating it with file transfer adds an entire new virtual-channel implementation for no benefit when RDPDR is already proven, already connected, and already has a working request/reply precedent.

**Do this instead:** Generalize RDPDR (Pattern 4); leave CLIPRDR out of scope exactly as PROJECT.md already has it.

## Integration Points

### External Services

| Service | Integration Pattern | Notes |
|---------|---------------------|-------|
| Remote Windows target (RDP server) | Existing IronRDP `Session::connect` — unchanged | v1.0 DONE |
| rdpilot sensor (on target) | Existing DVC request/reply — extended with 2 new `MsgType`s | Modify, don't replace |
| MCP host (Claude Desktop / other LLM app) | `rmcp` stdio transport (`transport-io` feature) — the LLM host spawns `rdpilot-mcp` as a child process and talks MCP over its stdin/stdout | New; standard rmcp pattern, verified via Context7 |

### Internal Boundaries

| Boundary | Communication | Notes |
|----------|---------------|-------|
| `rdpilot-cli` ↔ `rdpilot-daemon` | `rdpilot-ipc` over unix socket / named pipe, framed JSON | Invoke-and-exit; connects fresh per invocation |
| `rdpilot-mcp` ↔ `rdpilot-daemon` | Same `rdpilot-ipc` protocol, one persistent connection for the process's lifetime | Long-lived; many requests over one connection |
| `rdpilot-daemon` ↔ `crates/rdpilot::Session` | Direct in-process async method calls (no serialization) | `Session` is `Send + Sync`; daemon just holds `Arc<Session>` per registry entry |
| `Session` ↔ sensor (remote) | Existing DVC request/reply, extended with `UploadFile`/`DownloadFile` | Modify sensor + `Session`, not the transport |
| `Session` ↔ `RdpilotDriveBackend` | Existing RDPDR static channel, generalized from one static file to a registered allow-list + `Write` IRP support | Core file-transfer modification point |

## Sources

- `crates/rdpilot/src/session.rs`, `session_loop.rs` (read directly), `connect.rs`, `rdpdr_backend.rs` — v1.0 source, HIGH confidence (primary source, not inferred)
- `.planning/PROJECT.md`, `.planning/STATE.md` — milestone requirements and accumulated v1.0 decisions
- [rmcp (official Rust MCP SDK) — docs.rs](https://docs.rs/rmcp/latest/rmcp/) — server/stdio transport patterns, tool_router macros (Context7-verified, HIGH confidence)
- [interprocess crate — docs.rs](https://docs.rs/interprocess/latest/interprocess/local_socket/index.html) — cross-platform local_socket abstraction (WebSearch-verified, MEDIUM confidence; Windows ACL configurability NOT confirmed available — flagged, do not rely on it for the security-sensitive Windows leg)
- [tokio::net::windows::named_pipe::ServerOptions — docs.rs](https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/struct.ServerOptions.html) — `create_with_security_attributes_raw` for custom DACL (HIGH confidence, official docs)
- [Offensive Windows IPC Internals 1: Named Pipes — csandker.io](https://csandker.io/2021/01/10/Offensive-Windows-IPC-1-NamedPipes.html) — default named-pipe DACL grants Everyone/anonymous read access absent a custom security descriptor (MEDIUM confidence, single security-research source; corroborated by general Windows named-pipe documentation, worth re-verifying against current Windows docs at implementation time)
- [sccache — GitHub / crates.io / docs](https://github.com/mozilla/sccache) — auto-start-on-first-use + configurable idle-timeout self-shutdown daemon model (MEDIUM confidence, official project docs)
- [Anthropic "computer use" tool — platform.claude.com](https://platform.claude.com/docs/en/agents-and-tools/tool-use/computer-use-tool) — current action set (`screenshot`, `left_click`, `type`, `key`, `scroll`, etc.) for the Anthropic-compatible half of the dual tool surface (MEDIUM confidence, WebSearch-aggregated from official docs + `claude-quickstarts` reference implementation)

---
*Architecture research for: rdpilot v1.1 consumer surfaces (daemon, CLI, MCP server, bidirectional file transfer)*
*Researched: 2026-07-10*
