# Phase 12: Session Daemon - Research

**Researched:** 2026-07-11
**Domain:** Long-lived local IPC daemon (Rust/tokio), cross-platform local-user-scoped transport security, OS-thread lifecycle management
**Confidence:** MEDIUM-HIGH (architecture, thread-leak mechanics, and Unix security path are HIGH; the exact `interprocess` 2.4.2 Windows security-descriptor API surface could not be conclusively verified this session — see Open Questions)

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions (Phase-Local)

- **D-31:** Daemon lifecycle policy (owned by Phase 12). Idle sessions ARE reaped after a config-overridable inactivity timeout (satisfying DAEMON-03's "reaps idle sessions"), and when the registry empties the daemon self-exits — Success Criterion 5's immediate self-shutdown-on-empty is honored, softened only by a short config-overridable anti-thrash **grace period** before the empty daemon actually exits (so a rapid disconnect→reconnect does not pay a full daemon cold-start). On crash/restart the daemon SURFACES possibly-live orphaned remote Windows sessions (from the minimal disk-persisted reconciliation state) for EXPLICIT reclaim/teardown rather than auto-killing them — it never silently destroys a session a human may be actively using, and never silently forgets it (DAEMON-04). All durations (idle-session timeout, empty-daemon grace period) are config-overridable via the Phase 11 config layers.

### Inherited / Already-Locked Decisions (do NOT re-open)

**Owned by Phase 12 (this phase defines the concrete shape):**
- **D-29:** Session identity & targeting. Auto-generated session ids are short, human-legible adjective-noun word-pairs (e.g. `brave-otter`). There is NO implicit/silent default target. Phase 12 owns the id-generation logic and uniqueness/collision enforcement.
- **D-30:** `list` status vocabulary. The daemon's `list` reports a lifecycle status enum — `Connecting` / `Live` / `Reconnecting` / `Disconnected` — derived from the SDK keepalive signal. Phase 12 defines this enum (already typed in `rdpilot-ipc::SessionLifecycle`, extensible by this phase — see Open Questions).

**Locked upstream (Phase 12 implements, does not re-decide):**
- **D-17:** CLI and MCP are BOTH thin clients of this single long-lived daemon over local IPC; the daemon owns session state and the shared registry.
- **D-18:** Explicit per-command session targeting; no implicit default target; session identity is a required wire-schema field.
- **D-19:** In-memory registry + orphan cleanup on restart, reattach deferred. This registry architecture is LOCKED — Phase 12 implements it (in-memory registry + minimal disk-persisted reconciliation state), it does not re-decide it.
- **D-26:** Stack. IPC transport is `interprocess` 2.4.2 / native named-pipe; jsonrpsee/daemonize/axum/tonic are anti-recommended for local IPC. DAEMON-02 scoping uses `interprocess`'s `create_with_security_attributes_raw` on Windows (explicit DACL) and a `0700` socket-dir + peer-uid check on Unix. **This local-user IPC-scoping mechanism is LOCKED** (Success Criterion 4) — Phase 12 implements it, it does not re-decide the transport or the scoping primitive.
- **D-24:** Serialize-path credential redaction — enforced at the Phase 11 schema level; the daemon's `list`/status responses ride credential-free DTOs and must not leak the password.
- **D-27 / D-28:** Config form (`config.toml`, `RDPILOT_`-prefixed env) / Wire error taxonomy (`session-not-found`, `daemon-unreachable`, `transfer-failed`, `path-traversal`, `checksum-mismatch`, `internal` catch-all). The daemon reads config via `rdpilot-config`; D-31's config-overridable durations live there. The daemon's dispatch maps failures onto the fixed `WireError` code set.

### Out of Scope (Deferred Ideas)

- Durable session reattach across daemon restart — explicitly deferred per D-19; orphans are reconciled/torn-down, not re-adopted as live sessions.
- Remote-assist / session shadowing — Backlog Phase 999.4 / SEED-001.
- MCP progress notifications for long operations — Phase 14+ concern, not the daemon.

</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DAEMON-01 | A long-lived daemon holds N live RDP sessions with keepalive, decoupled from any CLI process lifetime | §Architecture Patterns (registry + per-session OS thread), §Common Pitfalls Pitfall 1 (thread-join discipline), §Validation Architecture (soak test) |
| DAEMON-02 | A local IPC transport carries requests between clients and the daemon, restricted to the local user (permission/DACL-scoped) | §Standard Stack (`interprocess` 2.4.2, `tokio` named-pipe fallback), §Common Pitfalls Pitfall 5, §Code Examples (Unix peer-uid check, Windows DACL) |
| DAEMON-03 | The daemon auto-starts on first client connect and reaps idle sessions / self-shuts-down when the registry empties | §Architecture Patterns (bind-as-mutex spawn-or-connect), §Common Pitfalls (double-spawn race), D-31 (grace period) |
| DAEMON-04 | On restart, the daemon detects and tears down orphaned Windows-side sessions (in-memory registry; no reattach) | §Common Pitfalls Pitfall 9, §Architecture Patterns (reconciliation-state file), §Open Questions (SessionLifecycle::Orphaned) |
| SESSION-01 | User can open a session under a caller-supplied name, or receive an auto-generated id when unnamed | §Open Questions (Request::Connect verb gap), §Code Examples (atomic claim-then-connect) |
| SESSION-03 | User can list active sessions (name/id, target, status) | §Open Questions (Request::List verb gap — `WireResponse::SessionList` already exists) |
| SESSION-04 | User can disconnect a named session; names/ids are unique (collisions rejected) | §Common Pitfalls Pitfall 3 (atomic insert), §Code Examples |

</phase_requirements>

## Summary

Phase 12 builds a single long-lived Rust daemon binary (`rdpilot-daemon`) that owns an in-memory registry of `rdpilot::Session` handles, exposes them over a local-user-scoped IPC transport (Unix domain socket / Windows named pipe), and survives its own crashes by persisting just enough reconciliation state to disk to surface — never silently discard — a possibly-still-live remote Windows session after a `kill -9` restart.

The single most consequential finding of this research is **not** a library choice: it is that `rdpilot-ipc`'s `Request`/`WireResponse` enums, as shipped by Phase 11, have **no wire verbs for opening, listing, or closing a session** — every existing `Request` variant (`Ping`, `Screenshot`, `LaunchProcess`, `SetForeground`, `Put`, `Get`) requires an *already-open* `SessionId`. Phase 11's own doc comments explicitly scoped this ("Wire verb scope for Phase 11... Richer input/perception verbs... are deferred to Phases 13/14") but never mention session lifecycle verbs at all — they were simply out of that phase's boundary. Phase 12 **must** extend `rdpilot-ipc` with `Request::Connect`, `Request::List`, `Request::Disconnect` (and a `WireResponse::Connected` reply) before SESSION-01/03/04 can be implemented at all. This is safe and expected: D-29/D-30 already delegate ownership of the session-identity and status-vocabulary *types* in this same crate to Phase 12, and `WireErrorCode` is explicitly `#[non_exhaustive]` to allow exactly this kind of phase-scoped extension. See Open Questions for the precise recommended shape.

The second load-bearing finding is the **thread-join discipline**: `Session`'s dedicated-OS-thread-per-session model (`crates/rdpilot/src/session.rs`) has a working, awaited join path (`Session::close()` → `spawn_blocking(move || thread.join())`) and a deliberately non-joining `Drop` fallback that "exits on its own shortly after." DAEMON-01's leak-free soak-test criterion is a direct test of whether the registry's disconnect/reap paths *always* route through `close()` (never through an implicit drop of the map entry) and *never* hold the registry's `std::sync::Mutex` guard across the `.await`.

The third: the DAEMON-02 security model has a straightforward, well-documented HIGH-confidence Unix path (`0700` `XDG_RUNTIME_DIR` subdirectory + `tokio::net::UnixStream::peer_cred()`, which is a **stable** tokio API, unlike the still-nightly-gated `std::os::unix::net::UnixStream::peer_cred()`) and a HIGH-confidence Windows fallback path (`tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw` + `.first_pipe_instance(true)`, both stable, verified against the current tokio 1.52.3 docs — this is very likely the literal API D-26/the phase brief is naming). The `interprocess` 2.4.2 crate's *own* API surface for setting a Windows security descriptor could not be conclusively verified this session (docs.rs fetches returned incomplete content); this is flagged as a Wave-0 spike, with the tokio-native path as a fully-specified fallback.

**Primary recommendation:** Extend `rdpilot-ipc` with the three missing lifecycle verbs first (Wave 0/1, before anything else can compile end-to-end); build the registry as `Arc<Mutex<HashMap<SessionId, SessionEntry>>>` with a claim-then-connect atomic-insert pattern (never a bare `.entry().or_insert()` around a slow `await`); route every session teardown path through `Session::close().await`, never a bare drop; implement the Unix and Windows IPC-security paths natively via `tokio`'s stable APIs, treating `interprocess` as the connection-acceptance/framing convenience layer only.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Session registry (name/id → live `Session` handle) | Backend / Daemon process | — | The daemon is the only process holding `rdpilot::Session`; CLI/MCP are stateless per-invocation clients (D-17) |
| IPC transport + local-user access control | Backend / Daemon process (server side) | OS (Unix socket perms / Windows DACL) | Security enforcement must happen at the OS primitive (filesystem mode / kernel DACL check), not in application logic — the daemon configures the OS primitive correctly, it does not reimplement authorization |
| Session lifecycle (connect/disconnect/idle-reap/self-shutdown) | Backend / Daemon process | — | Only the daemon has the long-lived Tokio runtime + OS-thread registry needed to track keepalive/idle state |
| Wire protocol type definitions (`Request`/`WireResponse`) | Shared library (`rdpilot-ipc`) | Backend (daemon adds new verbs) | Must be linkable by thin CLI/MCP clients without pulling in `rdpilot`/IronRDP (D-17); Phase 12 is the first consumer to need lifecycle verbs, so it extends the shared crate rather than inventing a daemon-private protocol |
| Config resolution (host/creds/durations) | Client process (CLI/MCP resolve `ResolvedConfig`) → Backend (daemon converts to `rdpilot::ConnectionConfig`) | — | `resolve()` is designed to run in the CLI/MCP process (it consumes flags/MCP-init params those processes parse); the daemon receives the *already-resolved* values via the `Connect` request and performs the `ResolvedConfig → ConnectionConfig` conversion at session-open time |
| Crash-restart reconciliation state | Local disk (daemon-owned file under the platform state/cache dir) | Backend (daemon reads/writes it) | Must survive the daemon process dying; nothing else can hold it |
| Remote Windows session teardown | Backend (daemon, via `rdpilot::Session`) | Windows target (WTS logoff semantics) | The daemon is the only component with `rdpilot` linked; actual logoff semantics are the remote OS's, observed indirectly through reconnect behavior |

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `interprocess` | 2.4.2 `[VERIFIED: crates.io]` | Cross-platform local-socket accept/connect abstraction (Unix domain socket / Windows named pipe) | D-26 locked; verified current on crates.io (`max_version: 2.4.2`, `default_version: 2.4.2`); its `local_socket::ListenerOptions` + Unix-only `ListenerOptionsExt::mode()` for socket-file permissions is documented (MEDIUM confidence per §Open Questions on the Windows security-descriptor surface specifically) |
| `tokio` | 1.52.3 `[VERIFIED: crates.io + docs.rs]` | Async runtime; already a `rdpilot` dependency (`tokio = { version = "1", features = ["full"] }`). Also the vehicle for the Windows-native named-pipe DACL fallback and the stable Unix `peer_cred()` | Already the workspace's async runtime; `tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw` and `tokio::net::UnixStream::peer_cred()` are both stable, verified against the current docs |
| `dashmap` OR plain `std::sync::Mutex<HashMap<..>>` | `dashmap` 6.2.1 `[VERIFIED: crates.io]` if used | Concurrent session registry | See §Don't Hand-Roll and §Architecture Patterns — a plain `Mutex<HashMap>` is RECOMMENDED over `dashmap` for this workload (session counts are small, single global lock is simpler and equally race-free); `dashmap` listed as the alternative if measured contention ever warrants it |
| `serde` / `serde_json` | 1.x (already a workspace dep via `rdpilot-ipc`) | Wire message framing (length-prefixed JSON, matching `rdpilot-ipc`'s existing `#[serde(tag = "op")]` `Request` shape) | Already established in `rdpilot-ipc` |
| `directories` | 6.0.0 `[VERIFIED: crates.io; already a rdpilot-config dependency]` | Resolve the Unix socket directory (`BaseDirs::runtime_dir()` → `$XDG_RUNTIME_DIR`, falling back to `cache_dir()`) and the reconciliation-state file path | Already pinned and used identically by `rdpilot-config::paths::config_file_path()` for the config file; reuse the same crate/pattern rather than introducing a second path-resolution dependency |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `sysinfo` | 0.39.6 `[VERIFIED: crates.io]` | Read current-process RSS for the DAEMON-01 soak-test baseline/return-to-baseline assertion | Test-only dependency (dev-dependency), used by the soak-test harness, not daemon runtime code |
| `windows-permissions` | 0.2.4 `[ASSUMED — package-name provenance from WebSearch, not Context7/official docs]` | Safe wrapper for constructing a Windows `SECURITY_DESCRIPTOR` from an SDDL string, to hand to `create_with_security_attributes_raw` | Candidate for the Windows DACL construction step; verify exact `SecurityDescriptor`/SDDL API at implementation time (docs.rs fetch was inconclusive this session — see Open Questions). 829K total downloads, 6+ years old, has a source repo — passed slopcheck `[OK]` |
| `libc` | 0.2.186 `[VERIFIED: crates.io]` | Low-level fallback only if `tokio::net::UnixStream::peer_cred()` proves insufficient (e.g. needs `SO_PEERCRED` directly) | Not expected to be needed — `tokio`'s `peer_cred()` should cover DAEMON-02's Unix path entirely |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Plain `Mutex<HashMap<SessionId, SessionEntry>>` for the registry | `dashmap` 6.2.1 | `dashmap`'s sharded locking only pays off under high concurrent contention; a session daemon handling human-scale connect/disconnect/list traffic (not thousands of req/s) gets no measurable benefit and adds a dependency + a slightly less obvious "what does atomicity mean across shards" mental model. Use plain `Mutex` first; only revisit if profiling shows contention. |
| `interprocess`'s cross-platform Windows named-pipe creation | `tokio::net::windows::named_pipe::ServerOptions` directly (bypass `interprocess` for pipe *creation* only, still usable for the framing/read-write side if desired) | `tokio`'s API is fully verified (stable, `create_with_security_attributes_raw` + `first_pipe_instance` both confirmed against current docs) whereas `interprocess`'s exact Windows security-descriptor method name could not be verified this session. If the Wave-0 spike can't confirm `interprocess` exposes an equivalent, fall back to this path without touching D-26's Unix-side transport choice. |
| Disk-persisted reconciliation state as a hand-rolled binary format | `serde_json`-serialized struct (already a workspace dependency) | JSON is simplest, human-inspectable (useful for a solo-author debugging a crash), and the reconciliation file is tiny (one record per session) — no performance case for a binary format. |
| Custom PID-file single-instance lock for DAEMON-03 | Bind-as-mutex (rely on the OS's exclusive-bind semantics of the socket path / `first_pipe_instance` flag) | A PID file can go stale after `kill -9` (no cleanup) and requires its own race-prone check-then-act logic. The OS's own bind/first-instance exclusivity is atomic by construction and self-heals after a crash (a new process can always bind once the crashed process's kernel resource is released, which the OS itself detects) — see Pitfall discussion. |

**Installation:**
```toml
# workspace Cargo.toml
[workspace]
members = ["crates/rdpilot", "crates/rdpilot-ipc", "crates/rdpilot-config", "crates/rdpilot-daemon"]

# crates/rdpilot-daemon/Cargo.toml
[dependencies]
rdpilot = { path = "../rdpilot" }
rdpilot-ipc = { path = "../rdpilot-ipc" }
rdpilot-config = { path = "../rdpilot-config" }
interprocess = "2.4.2"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
directories = "6.0.0"
thiserror = "2"

[target.'cfg(windows)'.dependencies]
windows-permissions = "0.2.4"   # verify exact API before locking this in — see Open Questions

[dev-dependencies]
sysinfo = "0.39.6"
```

**Version verification:** All versions above marked `[VERIFIED: crates.io]` were confirmed live via `curl -A "Mozilla/5.0" https://crates.io/api/v1/crates/<pkg>` during this research session (2026-07-11); `tokio`'s specific API methods were additionally confirmed against current docs.rs content for tokio 1.52.3.

## Package Legitimacy Audit

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|-------------|
| `interprocess` | crates.io | created 2020-08-05 (~6 yrs) | 11.2M total / 3.0M recent | github.com/kotauskas/interprocess | `[OK]` | Approved (D-26 locked anyway) |
| `tokio` | crates.io | long-established | very high | github.com/tokio-rs/tokio | `[OK]` | Approved (already a workspace dep) |
| `dashmap` | crates.io | created 2019-08-25 (~7 yrs) | 312.9M total / 67.9M recent | github.com/xacrimon/dashmap | `[OK]` | Approved as alternative (not primary recommendation) |
| `directories` | crates.io | long-established | high | — | `[OK]` | Approved (already a workspace dep) |
| `sysinfo` | crates.io | created 2015-07-25 (~11 yrs) | 165.6M total / 37.1M recent | github.com/GuillaumeGomez/sysinfo | `[OK]` | Approved (dev-dependency only) |
| `windows-permissions` | crates.io | created 2019-06-11 (~7 yrs) | 829K total / 43.5K recent | github.com/danieldulaney/windows-permissions-rs | `[OK]` | Approved, tagged `[ASSUMED]` for exact API shape — verify at implementation time |
| `libc` | crates.io | long-established | very high | — | `[OK]` | Approved as fallback only |

**Packages removed due to slopcheck `[SLOP]` verdict:** none
**Packages flagged as suspicious `[SUS]`:** none

slopcheck 0.6.1 was installed and run (`slopcheck install <pkgs> --ecosystem crates.io`) against all 8 candidate packages (interprocess, windows-permissions, directories, uzers, tokio, sysinfo, dashmap, libc); all returned `[OK]`. `uzers` was checked as a candidate for Unix uid resolution but is **not** recommended (tokio's `peer_cred()` already returns the peer uid directly; no separate uid-lookup crate is needed for DAEMON-02's numeric-uid comparison — `uzers` would only be relevant if the daemon needed to resolve a *username* from a uid, which it does not).

## Architecture Patterns

### System Architecture Diagram

```
                         ┌─────────────────────────────────────────────┐
                         │              rdpilot-daemon (process)         │
                         │                                                │
  Client (CLI/MCP,  ───▶ │  IPC Listener (interprocess / tokio native)   │
  Phase 13/14,           │  ├─ Unix: 0700 runtime-dir socket +           │
  built later)           │  │        peer_cred() uid check per conn     │
  "connect-or-spawn"     │  └─ Windows: named pipe, explicit DACL +      │
  handshake               │            first_pipe_instance(true)         │
                         │        │                                       │
                         │        ▼                                       │
                         │  Dispatch (match on rdpilot-ipc::Request)      │
                         │        │                                       │
                         │        ▼                                       │
                         │  Registry: Mutex<HashMap<SessionId,            │
                         │            SessionEntry>>                      │
                         │   ┌─────────────┬─────────────┬─────────────┐  │
                         │   │ SessionEntry│ SessionEntry│ SessionEntry│  │
                         │   │ Connecting  │ Live        │ Live        │  │
                         │   │ (claimed,   │ {rdpilot::  │ {rdpilot::  │  │
                         │   │  no thread  │  Session,   │  Session,   │  │
                         │   │  yet)       │  thread}    │  thread}    │  │
                         │   └─────────────┴──────┬──────┴──────┬──────┘  │
                         │                          │             │        │
                         │                          ▼             ▼        │
                         │              dedicated OS thread  dedicated OS  │
                         │              (current-thread       thread      │
                         │               tokio runtime,      (same)       │
                         │               session_loop.rs)                 │
                         │                          │             │        │
                         │  Idle reaper (tokio task, periodic) ───┘        │
                         │  Empty-registry watcher → grace period → exit  │
                         │                                                │
                         │  Reconciliation-state file (disk, JSON):       │
                         │  written on every connect/disconnect/reap;     │
                         │  read once at daemon startup                   │
                         └─────────────────────────┬───────────────────────┘
                                                     │  RDP (TLS/NLA) per session
                                                     ▼
                                        Remote Windows target(s)
```

A reader tracing SESSION-01 (open under a name): client → IPC listener (uid/DACL check) → dispatch matches `Request::Connect` → registry claims the name atomically (inserts a `Connecting` placeholder under the mutex, releases the lock) → `rdpilot::Session::connect()` runs *outside* the lock on the caller's async task → on success, the registry is re-locked briefly to upgrade the placeholder to `Live { session, thread }` → `WireResponse::Connected { session: id }` returns to the client.

### Recommended Project Structure

```
crates/rdpilot-daemon/
├── Cargo.toml
├── src/
│   ├── main.rs            # binary entry: parse minimal CLI (socket path override?), init, run
│   ├── lib.rs              # re-exports for the integration-test harness (CARGO_BIN_EXE pattern)
│   ├── registry.rs         # Mutex<HashMap<SessionId, SessionEntry>>, atomic claim/insert/remove
│   ├── ipc/
│   │   ├── mod.rs          # cfg-gated re-export of unix/windows listener + shared framing
│   │   ├── unix.rs         # 0700 runtime-dir + peer_cred() uid check
│   │   ├── windows.rs      # named pipe + DACL + first_pipe_instance
│   │   └── framing.rs      # length-prefixed serde_json read/write over the accepted Stream
│   ├── dispatch.rs         # match on rdpilot-ipc::Request -> registry ops -> WireResponse
│   ├── error_map.rs        # rdpilot::Error -> rdpilot_ipc::WireErrorCode (D-28's Phase-12 mapping)
│   ├── lifecycle.rs        # idle reaper task, empty-registry grace-period self-shutdown
│   ├── autostart.rs        # client-side connect-or-spawn helper (consumed later by Phase 13/14,
│   │                       # but implemented+tested here so DAEMON-03 has real coverage now)
│   └── reconcile.rs        # disk-persisted reconciliation-state read/write, startup orphan scan
└── tests/
    ├── registry_concurrency.rs   # SC#1: N simultaneous same-name connects -> 1 winner
    ├── thread_leak_soak.rs       # SC#3 [BLOCKING]: connect/disconnect cycles, thread+RSS baseline
    ├── ipc_security.rs           # SC#4 [BLOCKING]: different-uid client rejected (Unix, native)
    ├── autostart_lifecycle.rs    # SC#5 [BLOCKING, partial]: spawn-on-first-connect, empty-shutdown
    └── crash_restart_reconcile.rs # SC#5 [BLOCKING, partial]: kill -9 + restart surfaces orphan
```

### Pattern 1: Atomic claim-then-connect (Pitfall 3 / SESSION-01 / SESSION-04)

**What:** Reserve a session name under the registry mutex synchronously (no `.await` while the lock is held), release the lock, THEN perform the slow async `Session::connect()`. On success, re-lock briefly to upgrade the placeholder; on failure, re-lock briefly to remove the claim.

**When to use:** Every `Request::Connect` dispatch — this is the ONLY correct way to make "N simultaneous same-name connects yield exactly one live session" true, because checking "does this name exist" and inserting it must be a single atomic operation, not two.

**Example:**
```rust
// Source: derived from codebase precedent — mirrors the existing
// `input_db: Mutex<Database>` discipline in crates/rdpilot/src/session.rs
// ("Locked only for the synchronous call — never held across an .await").

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use rdpilot::{ConnectionConfig, Session};
use rdpilot_ipc::SessionId;

enum SessionEntry {
    Connecting { claimed_at: Instant },
    Live { session: Session, connected_since: Instant },
}

struct Registry {
    sessions: Mutex<HashMap<SessionId, SessionEntry>>,
}

impl Registry {
    /// Returns `Err(DuplicateName)` if `name` is already claimed or live.
    /// Never holds `sessions` locked across the `.await` inside `Session::connect`.
    async fn open(&self, name: SessionId, cfg: ConnectionConfig) -> Result<SessionId, DaemonError> {
        {
            let mut guard = self.sessions.lock().expect("registry mutex poisoned");
            match guard.entry(name.clone()) {
                Entry::Occupied(_) => return Err(DaemonError::DuplicateSession(name)),
                Entry::Vacant(slot) => {
                    slot.insert(SessionEntry::Connecting { claimed_at: Instant::now() });
                }
            }
        } // lock released — the slow RDP connect happens with NO lock held

        match Session::connect(&cfg).await {
            Ok(session) => {
                let mut guard = self.sessions.lock().expect("registry mutex poisoned");
                guard.insert(name.clone(), SessionEntry::Live {
                    session,
                    connected_since: Instant::now(),
                });
                Ok(name)
            }
            Err(e) => {
                let mut guard = self.sessions.lock().expect("registry mutex poisoned");
                guard.remove(&name); // release the claim so a retry with the same name can succeed
                Err(DaemonError::from(e))
            }
        }
    }
}
```

Concurrency test shape for SC#1: spawn N tokio tasks that all call `registry.open(SessionId::from_str("same-name")?, cfg.clone())` simultaneously via `tokio::join!`/`FuturesUnordered`; assert exactly one `Ok`, N-1 `Err(DuplicateSession)`, and the registry has exactly one entry afterward.

### Pattern 2: Explicit `close().await`, never a bare drop (Pitfall 1 / DAEMON-01)

**What:** Every registry code path that removes a `Session` from the map — explicit disconnect, idle reap, daemon graceful shutdown — must extract the owned `Session`, then `.await` its `close()`. It must never let the removed value simply go out of scope.

**Example:**
```rust
// Source: derived from crates/rdpilot/src/session.rs's own documented Drop
// contract ("Drop cannot block on async teardown... `close()` is the clean,
// awaited path").
async fn close_session(registry: &Registry, id: &SessionId) -> Result<(), DaemonError> {
    let entry = {
        let mut guard = registry.sessions.lock().expect("registry mutex poisoned");
        guard.remove(id) // extract, don't let it drop in this scope
    };
    match entry {
        Some(SessionEntry::Live { session, .. }) => {
            session.close().await.map_err(DaemonError::from) // joins the OS thread
        }
        Some(SessionEntry::Connecting { .. }) => {
            // A connect is in flight for this id; removing the placeholder here
            // is a race with Pattern 1's re-lock-on-success/failure step — the
            // registry API should reject concurrent disconnect-during-connect
            // (return SessionNotFound or a dedicated "still connecting" error)
            // rather than attempt to interrupt the in-flight connect.
            Err(DaemonError::SessionNotFound(id.clone()))
        }
        None => Err(DaemonError::SessionNotFound(id.clone())),
    }
}
```

**Soak-test shape for SC#3 [BLOCKING]:** record `sysinfo::System::new_all()` process RSS + `/proc/self/status` `Threads:` count (Linux-native, offline-testable on this host) before any session opens; run N connect→close cycles sequentially (or with bounded concurrency); assert both metrics return to within a small tolerance of baseline afterward. The Windows equivalent (thread count via `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)` or similar) is a `#[cfg(windows)]` concern deferred to the live/pinned-machine gate — flag this explicitly (see Environment Availability).

### Pattern 3: Bind-as-mutex auto-start (DAEMON-03)

**What:** A client (or the daemon's own test harness standing in for one) always attempts to *connect* to the well-known socket/pipe path first. Only on connect failure does it spawn the daemon binary (detached) and retry connecting with a bounded backoff. The daemon itself, on startup, attempts to bind/listen on the well-known path; if the bind fails because another daemon already holds it, the new process exits immediately (not an error — this is the expected outcome of a benign spawn race between multiple simultaneously-racing clients).

**When to use:** DAEMON-03's auto-start, and its double-spawn-race safety.

**Example:**
```rust
// Source: standard "bind is the mutex" daemon idiom, adapted to this workspace.
async fn connect_or_spawn(socket_path: &Path, daemon_exe: &Path) -> io::Result<Stream> {
    if let Ok(stream) = try_connect(socket_path).await {
        return Ok(stream);
    }
    // No listener yet (or a stale socket file on Unix — try_connect must treat a
    // connect-refused/timeout on an existing special file as "not listening" and
    // proceed to spawn, not as a hard error).
    std::process::Command::new(daemon_exe)
        .spawn()?; // detached; do not wait — the daemon daemonizes/backgrounds itself
    for backoff in [50, 100, 200, 400, 800] {
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        if let Ok(stream) = try_connect(socket_path).await {
            return Ok(stream);
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "daemon did not become reachable"))
}
```

On the daemon side, `first_pipe_instance(true)` (Windows) and a plain `bind()` on the Unix socket path (which fails `EADDRINUSE` if a live listener already holds it — the daemon must additionally detect and clean up a *stale* socket special-file left by a `kill -9`'d predecessor, by attempting a connect to it first and unlinking it if that connect fails) are the two platform primitives that make "only one daemon ever actually listens" true without a separate PID-file lock.

### Anti-Patterns to Avoid

- **Holding the registry `Mutex` guard across an `.await`:** compiles in some cases with `tokio::sync::Mutex` but is a correctness/liveness bug (blocks all other registry operations for the duration of a slow RDP connect or thread join) and is an outright hazard with `std::sync::Mutex`. Every example above scopes the guard with an explicit block that ends before the next `.await`.
- **Letting a `SessionEntry` drop implicitly on `HashMap::remove` without calling `.close().await`:** this is precisely DAEMON-01's soak-test failure mode — `Session::drop()` is a best-effort, non-blocking fallback, not the normal path.
- **A daemon-private wire protocol that bypasses `rdpilot-ipc`:** breaks the D-17 thin-client premise the moment Phase 13/14 need to construct a `Connect`/`List`/`Disconnect` request — see Open Questions.
- **PID-file-based single-instance enforcement:** stale after `kill -9`, requires its own race-prone check; prefer bind-as-mutex (Pattern 3).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Unix peer-uid retrieval | Manual `SO_PEERCRED` `getsockopt` FFI via raw `libc` calls | `tokio::net::UnixStream::peer_cred() -> io::Result<UCred>` | Stable tokio API (confirmed against tokio 1.52.3 docs), already exercised by the tokio ecosystem; hand-rolled FFI risks struct-layout mistakes for zero benefit |
| Windows named-pipe access-control descriptor | Raw `ACL`/`ACE`/`SID` Win32 struct construction by hand | `windows-permissions`'s SDDL-string-based `SecurityDescriptor` (verify exact method at implementation time) OR the well-known minimal SDDL string for owner-only access, passed to `create_with_security_attributes_raw` | SDDL strings are the standard, reviewable way to express "this object, one ACE, owner only" — hand-built ACE lists are exactly the kind of code most prone to subtle security bugs (e.g. forgetting `SE_DACL_PROTECTED`, leaving a NULL DACL that grants everyone access) |
| Concurrent name-uniqueness enforcement | A custom compare-and-swap loop over an `AtomicPtr`/lock-free structure | `HashMap::entry()` under a single `std::sync::Mutex` (Pattern 1) | The workload (human-scale connect/disconnect calls) has no throughput requirement that justifies lock-free complexity; `entry()` is already atomic w.r.t. the single lock |
| Daemon single-instance enforcement | A hand-rolled PID file + `flock`/mutex-name check | Bind-as-mutex (Pattern 3) | The OS's own bind-exclusivity is already atomic and self-healing after a crash; a PID file is strictly worse on every axis here |
| Adjective-noun auto-id generation | A hand-picked word list + naive random pick with no collision handling | A small deterministic word-pair generator + the SAME atomic-claim registry insert path used for caller-supplied names (Pattern 1) — collision handling is "just" the existing duplicate-name rejection path, retried with a fresh pair | Reuses the already-correct concurrency-safe insert path instead of adding a second, differently-shaped "generate and hope" code path |

**Key insight:** every "Don't Hand-Roll" item above has a stable, already-verified library or OS primitive backing it. The temptation in a small solo-author daemon is to write ad-hoc versions of all five because "it's just a few lines" — but three of the five (peer-uid, DACL, single-instance enforcement) are exactly the kind of local-security-boundary code where a subtly wrong hand-rolled version fails DAEMON-02's BLOCKING criterion silently (the test would need to specifically probe the wrong case to catch it).

## Runtime State Inventory

Not applicable — Phase 12 is new-code (a new `rdpilot-daemon` crate), not a rename/refactor/migration phase.

## Common Pitfalls

### Pitfall 1: OS-thread leak on disconnect (DAEMON-01, [BLOCKING] SC#3)

**What goes wrong:** The registry removes a `Session` from its map (e.g. via `HashMap::remove`) and the returned owned value is simply discarded at the end of the removing function's scope, instead of being explicitly `.close().await`'d.

**Why it happens:** `Session`'s `Drop` impl is intentionally silent and non-blocking (it cannot `.await` inside `drop()`), so the bug produces no panic, no error, no test failure at the unit level — only a slow-growing thread/RSS leak visible in a soak test. It is exactly the kind of bug that "looks fine" in a quick manual test (disconnect one session, list is empty, looks great) and only shows up under N cycles.

**How to avoid:** Route every registry-remove path (explicit disconnect, idle reap, daemon shutdown drain) through the exact `close_session` shape in Pattern 2. Code-review checklist: any `HashMap::remove` on the registry must be immediately followed by an `if let Some(entry) = ... { entry.session.close().await? }`, never just discarded.

**Warning signs:** A soak test whose thread count climbs linearly with cycle count (not staying flat); `close()` calls appearing only in the explicit-disconnect dispatch handler but NOT in the idle-reaper or shutdown-drain code paths (grep for every place `sessions.remove(` is called and verify each has a matching `.close().await`).

### Pitfall 3: Non-atomic named-session insert (SESSION-01/04, SC#1)

**What goes wrong:** A naive implementation checks `if registry.contains_key(&name) { reject } else { let session = Session::connect(&cfg).await?; registry.insert(name, session); }` — the `contains_key` check and the eventual `insert` are separated by the slow `await`, so N simultaneous callers with the same name ALL see "not present" and ALL proceed to connect, and the LAST one to finish silently overwrites the registry entry (dropping the earlier ones without `.close()`, compounding Pitfall 1).

**Why it happens:** The natural way to write "check then act" reads correctly in sequential code; the race only exists under real concurrency, which a single-threaded manual test never exercises.

**How to avoid:** Pattern 1 (claim-then-connect) — the check-and-insert must be a single, lock-held, non-`await`ing operation (`HashMap::entry()`), with the connect happening strictly after the lock is released.

**Warning signs:** A concurrency test firing N simultaneous same-name connects that produces more than one `Ok`, or produces a registry with the wrong session's thread orphaned (Pitfall 1 compounding).

### Pitfall 5: IPC transport not actually local-user-scoped (DAEMON-02, [BLOCKING] SC#4)

**What goes wrong:** Several subtly-wrong implementations all "look secure" in a casual test but fail against a genuinely different local account: (a) setting `0700` on the socket *file* but not the containing *directory* (the directory's own permissions gate whether another user can even `stat`/traverse to find the socket); (b) on Windows, creating the named pipe with `interprocess`'s or tokio's default security attributes (`None`/null), which on Windows means the DEFAULT DACL applies — often inherited from the process token, which frequently grants broader access than intended, not automatically "current user only"; (c) named-pipe squatting — an unprivileged local attacker process pre-creates a pipe with the same name *before* the daemon starts, and the daemon's `CreateNamedPipe` call for a NON-first instance silently succeeds by attaching to the attacker's existing pipe instead of creating a new, properly-DACL'd one.

**Why it happens:** All three failure modes require a second local OS user account (or, for (c), a timing race) to observe — a solo-author single-account dev machine will never notice any of them without deliberately testing from a different account.

**How to avoid:**
- Unix: create the socket's PARENT DIRECTORY with `DirBuilder::new().mode(0o700).create(...)` (atomic mode-at-creation, avoiding a TOCTOU window where the dir briefly exists world-readable) under `directories::BaseDirs::runtime_dir()`; additionally verify `tokio::net::UnixStream::peer_cred().uid` on every accepted connection matches the daemon's own effective uid (`rustix`/`nix`/`libc::geteuid()`) as defense-in-depth against a misconfigured directory.
- Windows: pass an EXPLICIT `SECURITY_ATTRIBUTES` pointing at a `SECURITY_DESCRIPTOR` scoped to the current user's SID (never `null`/default) to pipe creation, AND set `.first_pipe_instance(true)` so a pre-existing pipe of the same name causes creation to FAIL LOUDLY (`PermissionDenied`) rather than silently attaching to a squatted instance.
- The BLOCKING test (SC#4) must be run from a genuinely different OS account/uid (or simulate it — e.g. on Linux, a second unprivileged user account created in CI/the dev container) attempting to connect and being rejected; a same-user test that merely checks "the socket file has mode 0700" is necessary but not sufficient evidence.

**Warning signs:** A "security" test that only asserts file permissions via `stat` rather than actually attempting a cross-account connection; any code path where `create_with_security_attributes_raw`'s `attrs` argument is `null`/`None`.

### Pitfall 9: Crash-restart forgets or blindly kills orphaned remote sessions (DAEMON-04, [BLOCKING] SC#5)

**What goes wrong:** Because the registry is purely in-memory (D-19), a `kill -9` on the daemon process destroys every `Session`'s local RDP TCP connection AND its OS thread instantly (no graceful `close()` runs) — but per a LIVE-DIAGNOSED behavior already documented in `crates/rdpilot/src/session.rs` ("a Windows interactive RDP session is REUSED (reconnected, not recreated)... unless the prior session was explicitly logged off"), the REMOTE Windows-side session is likely NOT logged off just because the local TCP connection died — it sits in a `Disc` (disconnected) state server-side, possibly with the sensor process still running. A naive restart either (a) has zero record of it ever existing (silently forgotten — a slow zombie-session leak on the Windows target across repeated crash cycles) or (b) auto-reconnects and force-closes it without checking whether a human is actively using it (potentially destroying real work).

**Why it happens:** The in-memory-only registry (a deliberate, locked D-19 tradeoff to avoid full reattach complexity) has literally no record surviving the crash unless something is separately written to disk BEFORE the crash, and Windows' disconnect-vs-logoff distinction is a genuinely non-obvious RDP semantic that only surfaces under live testing (already independently rediscovered once in this codebase, per that comment).

**How to avoid:** Persist a minimal reconciliation record (session id, target host, connect timestamp, last-known-status) to a small on-disk JSON file on every state transition (connect success, explicit clean disconnect — which REMOVES the record since it was a clean logoff; idle reap — likewise removes it if the reap performed a clean `close()`). A record that is STILL PRESENT at daemon startup (i.e. was never cleanly removed) is definitionally a possible orphan: the prior daemon died before it could clean up after itself. On startup, load these leftover records and surface them distinctly (recommend extending `rdpilot-ipc::SessionLifecycle` with an `Orphaned` variant — see Open Questions) via `list`, rather than either forgetting them or auto-reconnecting/force-closing. Explicit human/agent action (a future `reconcile`/`disconnect` call against that id) is what actually resolves it — this phase's job is to make the possible orphan VISIBLE and ACTIONABLE, not to auto-resolve it (per D-31).

**Warning signs:** A restart test that shows an EMPTY `list` immediately after `kill -9` + restart (the orphan was silently forgotten — this is the failure mode DAEMON-04 exists to prevent); a restart implementation that reconnects and immediately force-closes every prior session without surfacing it first (violates D-31's anti-destroy-without-explicit-reclaim policy).

### Pitfall: Named-pipe squatting (Windows, folds into Pitfall 5)

**What goes wrong:** See Pitfall 5(c) above — documented separately here because it is a well-known, named Windows IPC vulnerability class (distinct from the DACL-scoping concern) worth its own verification step.

**How to avoid:** `.first_pipe_instance(true)` on `ServerOptions`, verified to cause pipe creation to fail (not silently succeed against a pre-existing instance) when this flag is set and another instance already exists.

## Code Examples

### Unix IPC listener setup with `0700` runtime dir + peer-uid check

```rust
// Source: derived from tokio 1.52.3 docs.rs (UnixStream::peer_cred, verified
// stable this session) + directories 6.0.0 (already an rdpilot-config dep).
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use directories::BaseDirs;
use tokio::net::{UnixListener, UnixStream};

fn socket_dir() -> io::Result<PathBuf> {
    let base = BaseDirs::new()
        .ok_or_else(|| io::Error::other("could not resolve a home/runtime directory"))?;
    let dir = base
        .runtime_dir()
        .unwrap_or_else(|| base.cache_dir())
        .join("rdpilot");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700) // atomic at creation — no TOCTOU window
        .create(&dir)?;
    Ok(dir)
}

async fn accept_and_authorize(listener: &UnixListener) -> io::Result<UnixStream> {
    let (stream, _addr) = listener.accept().await?;
    let peer = stream.peer_cred()?; // stable tokio API
    let our_uid = std::env::var("UID") // fallback; prefer a `nix`/`rustix` geteuid() call
        .ok()
        .and_then(|s| s.parse::<u32>().ok());
    // Recommend replacing the UID env-var fallback above with
    // `nix::unistd::Uid::effective().as_raw()` or `rustix::process::geteuid()`
    // at implementation time — this sketch prioritizes clarity over picking
    // yet another crate without verifying it against this session's evidence.
    if Some(peer.uid()) != our_uid {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "peer uid mismatch"));
    }
    Ok(stream)
}
```

### Windows IPC listener setup with explicit DACL (tokio-native fallback path)

```rust
// Source: tokio 1.52.3 docs.rs, ServerOptions::create_with_security_attributes_raw
// and ::first_pipe_instance — both verified against current docs this session.
// The exact SECURITY_ATTRIBUTES construction below is illustrative; verify the
// `windows-permissions` SDDL API (or fall back to raw `windows` crate calls)
// before treating this as final.
use tokio::net::windows::named_pipe::ServerOptions;

const PIPE_NAME: &str = r"\\.\pipe\rdpilot-daemon";

fn create_first_secured_pipe() -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    // "D:P(A;;GA;;;OW)" = DACL Protected, one ACE: allow Generic-All to OWNER.
    // Verify this exact SDDL string and its construction into a raw
    // SECURITY_ATTRIBUTES pointer against the chosen crate's actual API
    // before implementation — flagged in Open Questions.
    let attrs: *mut std::ffi::c_void = std::ptr::null_mut(); // placeholder — build real SECURITY_ATTRIBUTES here
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(true) // fails loudly if a pipe of this name already exists (anti-squatting)
            .create_with_security_attributes_raw(PIPE_NAME, attrs)
    }
}
```

### Reconciliation-state record (DAEMON-04)

```rust
// Source: shape derived from D-19/D-31's constraints, no external library.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReconciliationRecord {
    id: String,           // SessionId as_str()
    host: String,
    connected_since: String, // ISO-8601, matches SessionStatus's existing field shape
}

// Written to e.g. `directories::BaseDirs::cache_dir().join("rdpilot").join("sessions.json")`
// as a `Vec<ReconciliationRecord>`, rewritten wholesale on every state transition
// (small N, no need for an append-only log at this scale). Removed entries
// (clean disconnect/reap) mean "no reconciliation needed"; entries still
// present at startup are the candidate orphans.
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| `std::os::unix::net::UnixStream::peer_cred()` | Not yet stabilized (still behind the `peer_credentials_unix_socket` nightly feature) — use `tokio::net::UnixStream::peer_cred()` instead, which IS stable | Ongoing (as of this research date) | A researcher relying on training-data assumptions that std's peer_cred is stable would ship code that only compiles on nightly; verified this session that the tokio equivalent is the stable path |
| PID-file-based single-instance daemon locking | Bind-as-mutex (OS-level exclusive bind / `first_pipe_instance`) | Long-established Unix daemon idiom, still the correct recommendation here | Avoids stale-lock-after-crash entirely, which is directly relevant to DAEMON-04's crash-restart scenario |

**Deprecated/outdated:** None specific to this phase's stack — `interprocess`, `tokio`, `dashmap`, `sysinfo`, `directories` are all actively maintained with 2025/2026 releases.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `interprocess` 2.4.2 exposes a Windows security-descriptor-setting API equivalent to `create_with_security_attributes_raw` on its own `PipeListenerOptions` type | Standard Stack / Open Questions | If it does not, the planner must route Windows pipe creation entirely through `tokio::net::windows::named_pipe::ServerOptions` (fully specified fallback given above) instead of `interprocess` — low risk since the fallback is already HIGH confidence and code-complete in this document |
| A2 | `windows-permissions` 0.2.4's `SecurityDescriptor` type supports SDDL-string construction and interop with a raw `SECURITY_ATTRIBUTES` pointer | Standard Stack / Code Examples | If the exact API differs, the DACL-construction step needs either a different helper crate or raw `windows`-crate ACL/SID calls (more code, same underlying Win32 semantics — not a blocking risk, just more implementation effort) |
| A3 | Session-count / connect-disconnect frequency in this daemon's real usage is low enough that a single `std::sync::Mutex<HashMap<..>>` has no measurable contention downside vs. `dashmap` | Standard Stack / Don't Hand-Roll | If wrong (e.g. very high-frequency automated agent usage), `dashmap`'s `entry()` API is a drop-in-shaped alternative — same atomic-claim pattern applies, low risk either way |
| A4 | Windows-side OS thread count for the DAEMON-01 soak test needs a `#[cfg(windows)]`-specific implementation (e.g. `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)`) not yet identified precisely | Environment Availability / Validation Architecture | If the exact Windows API differs from what's sketched, the soak test's Windows-specific thread-count assertion needs adjustment at implementation time on the pinned dev machine; the Linux-native offline path (`/proc/self/status`) is unaffected and fully verifiable now |

## Open Questions

1. **`rdpilot-ipc::Request`/`WireResponse` are missing session-lifecycle verbs entirely.**
   - What we know: The Phase-11-shipped `Request` enum has exactly six variants (`Ping`, `Screenshot`, `LaunchProcess`, `SetForeground`, `Put`, `Get`), every one of which requires an *already-open* `SessionId` via the `SessionScoped` trait's exhaustive match. `WireResponse::SessionList { sessions: Vec<SessionStatus> }` already exists and is ready to serve as the `List` response with no changes. `WireErrorCode` is explicitly `#[non_exhaustive]` specifically to allow future codes like a dedicated duplicate-session code.
   - What's unclear: The exact variant shape for `Connect`/`Disconnect`/`List` and how they interact with `SessionScoped`'s current "every variant has a required session field" compile-time guarantee (which correctly enforces SESSION-02 for the six existing *operate-on-a-session* verbs, but doesn't naturally fit `Connect`/`List`, which don't target an existing session).
   - Recommendation (confident, not tentative): Add `Request::Connect { name: Option<String>, host: String, port: Option<u16>, username: String, password: String, domain: Option<String>, accept_invalid_certs: bool }`, `Request::List`, and `Request::Disconnect { session: SessionId }` to the SAME `Request` enum (not a separate wrapper type — keeps one wire message shape for all clients). Change `SessionScoped::session()` to return `Option<&SessionId>` (`Some` for the seven session-targeting variants including the new `Disconnect`, `None` for `Connect`/`List`), preserving the exhaustive-match forcing function for future variants while accurately modeling that Connect/List don't target an existing session. Add `WireResponse::Connected { session: SessionId }` to return the (possibly auto-generated) id. Add `WireErrorCode::DuplicateSession` for SESSION-04's collision-rejection case (has no `rdpilot::Error` equivalent, same category as the existing `SessionNotFound`/`DaemonUnreachable` daemon-native codes). This is Phase 12 scope, not a re-litigation of Phase 11: D-29/D-30 already establish that Phase 12 owns extending this crate's session-identity and status-vocabulary types; these three verbs are the same category of extension, just on the `Request`/`WireResponse` side.
   - This should be sequenced as an early (Wave 0 or Wave 1) task — nothing else in this phase can be end-to-end tested without it.

2. **Exact `interprocess` 2.4.2 Windows security-descriptor API.**
   - What we know: `interprocess`'s `local_socket::ListenerOptions` has a confirmed Unix-only `mode()` method (via `ListenerOptionsExt`) for socket-file permissions. Its lower-level `os::windows::named_pipe::PipeListenerOptions` is described (via WebSearch summaries, not a direct docs.rs fetch that rendered cleanly) as allowing "thorough customization," but no specific `security_descriptor`-shaped method/field name was confirmed this session.
   - What's unclear: Whether `interprocess` itself exposes DAEMON-02's Windows DACL requirement directly, or whether the daemon should bypass `interprocess` for Windows pipe *creation* specifically and use `tokio::net::windows::named_pipe::ServerOptions::create_with_security_attributes_raw` instead (fully verified, HIGH confidence, code-complete above).
   - Recommendation: Treat this as a small Wave-0 spike (read the actual `interprocess` 2.4.2 source via `cargo doc --open` or the vendored source once the crate is added to the workspace, which this offline-research session could not do without `cargo` available). If `interprocess` doesn't cleanly expose it, use the tokio-native fallback path for Windows pipe creation only — the Unix side's `interprocess`/tokio choice is unaffected either way.

3. **How does the daemon obtain `sensor_binary_path` / `share_root` for `ConnectionConfig`, given D-27's config keys (host/port/username/password/domain/accept_invalid_certs) don't cover them?**
   - What we know: `ConnectionConfig::new(host, username, password)` plus builder methods for `port`/`domain`/`dimensions`/`accept_invalid_certs`/`sensor_binary_path`/`share_root` exist in `crates/rdpilot/src/config.rs`. `rdpilot-config::ResolvedConfig` only carries the D-27-listed fields — no `sensor_binary_path`/`share_root`.
   - What's unclear: Whether the daemon resolves these two paths itself (e.g. relative to its own binary location, or a fixed convention like "next to the daemon executable" or a new config key), or whether this is deliberately deferred/out of Phase 12's success criteria (none of DAEMON-01..04/SESSION-01/03/04 explicitly require file transfer or sensor deployment to work through the daemon in this phase — `Put`/`Get`/`Ping`/`LaunchProcess`/`SetForeground`/`Screenshot` dispatch is plumbing Phase 11 already typed, but this phase's OWN success criteria don't test any of those verbs).
   - Recommendation: Out of this phase's blocking path — the daemon can construct a minimal `ConnectionConfig` (host/port/username/password/domain/accept_invalid_certs only) sufficient for SESSION-01/03/04 and DAEMON-01..04's connect/list/disconnect/soak/security/reconciliation testing, which does not require a working sensor DVC channel. If the planner wants `Ping`/file-transfer dispatch wired end-to-end in this phase too (not required by the stated success criteria), flag `sensor_binary_path`/`share_root` resolution as an additional small task.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `cargo`/`rustc` (native Linux) | All offline Rust build/test | ✗ (not installed in this research sandbox) | — | Not this agent's concern — prior phases (10/11) establish `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo <cmd> --target x86_64-unknown-linux-gnu` as the working offline substitution on the actual dev machine; this research session verified crate versions via crates.io/docs.rs directly instead |
| Windows target machine / VM | Windows-side DACL verification (Pitfall 5(b)/(c)), real thread-count API (`#[cfg(windows)]`), the pinned `x86_64-pc-windows-gnu` real build | ✗ in this sandbox | — | Substitute native Linux target for everything with no `cfg(windows)` branches (matches Phase 10's established precedent); the Windows-only DACL/pipe-squatting code genuinely needs either the pinned Windows dev machine or a live gate — cannot be soundly verified any other way |
| Second local OS user account | DAEMON-02's BLOCKING different-uid-rejected test (SC#4) | Unknown — not probed this session (no destructive account creation attempted without explicit instruction) | — | If unavailable on the dev machine, create a throwaway low-privilege Linux user in a container/CI step for this one test, OR simulate by manually constructing a `UCred` with a different uid in a unit test that bypasses the real `peer_cred()` call (weaker evidence — the planner should prefer the real cross-account test if at all feasible) |
| Live remote Windows RDP target | DAEMON-04's genuine "possibly-still-live remote session" reconciliation (the actual server-side disconnect-vs-logoff behavior) | Not available in this offline research session | — | The registry/IPC/thread-leak/security machinery (SC#1/2/3/4) is fully local/offline-testable; DAEMON-04's full crash-restart-reconcile loop (SC#5) can be partially tested offline (kill the daemon process, restart, assert the reconciliation record is loaded and surfaced) but the CLAIM that the remote Windows session is genuinely "possibly still live" (vs. having actually logged off) can only be confirmed against a real target — recommend a live gate plan (mirroring Phase 10's 10-05 pattern) for final confirmation, with the offline reconciliation-file-round-trip test carrying the bulk of the automated coverage |

**Missing dependencies with no fallback:** None — every gap above has a documented fallback or is explicitly scoped to a later live-gate plan.

**Missing dependencies with fallback:** cargo/rustc (use the established `RUSTUP_TOOLCHAIN`/`--target x86_64-unknown-linux-gnu` substitution on the actual dev machine, not available in this research sandbox); Windows-only DACL/thread-count code (native-Linux-testable code proceeds normally, `cfg(windows)`-gated code needs the pinned machine or a live gate); a second OS user account (create one, or accept weaker simulated-UCred coverage as an interim).

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | `cargo test` (built-in), matching every existing crate in this workspace — no new test framework needed |
| Config file | none — `rdpilot`/`rdpilot-ipc`/`rdpilot-config` all use inline `#[cfg(test)] mod tests` plus, for `rdpilot` only, a `tests/` integration directory (`crates/rdpilot/tests/live_session.rs`, gated `#[ignore]` + `RDPILOT_LIVE=1`). The new `rdpilot-daemon` crate should follow the SAME split: inline unit tests for registry/dispatch/error-mapping logic, a `tests/` directory for the concurrency/soak/security/lifecycle tests that need real OS processes/sockets. |
| Quick run command | `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu` (matches Phase 10's established offline substitution pattern; the Windows-`cfg`-gated code is skipped by this substitution and must be separately confirmed on the pinned Windows dev machine before any live gate) |
| Full suite command | `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test --workspace --target x86_64-unknown-linux-gnu` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SESSION-01/04 | N simultaneous same-name connects → exactly one `Ok`, N-1 `DuplicateSession` | integration (native, no live RDP target needed if `Session::connect` is exercised against a mock/stub — see note below) | `cargo test -p rdpilot-daemon registry_concurrency` | ❌ Wave 0 |
| SESSION-03 | `list` reports name/id/target/status/timestamps | unit (dispatch + registry, no real RDP connect needed — seed the registry directly with synthetic `SessionEntry`s) | `cargo test -p rdpilot-daemon dispatch::list_reports_all_fields` | ❌ Wave 0 |
| DAEMON-01 [BLOCKING] | Thread count + RSS return to baseline after N connect/disconnect cycles | integration/soak (native Linux, `/proc/self/status` thread count) | `cargo test -p rdpilot-daemon --test thread_leak_soak -- --ignored` (soak tests are typically slow — gate behind `--ignored` like the existing `live_session.rs` convention, run explicitly) | ❌ Wave 0 |
| DAEMON-02 [BLOCKING] | Different-uid client rejected (Unix); DACL enforced (Windows) | integration, native for Unix / live-gate for Windows | `cargo test -p rdpilot-daemon --test ipc_security` (Unix path); Windows path needs the pinned dev machine | ❌ Wave 0 |
| DAEMON-03 [BLOCKING, partial] | Auto-start on first connect; self-shutdown on empty registry + grace period | integration, spawns the real compiled `rdpilot-daemon` binary via `env!("CARGO_BIN_EXE_rdpilot-daemon")` | `cargo test -p rdpilot-daemon --test autostart_lifecycle` | ❌ Wave 0 |
| DAEMON-04 [BLOCKING, partial] | Restart after `kill -9` surfaces a reconciliation record for a possibly-live session | integration (offline: file round-trip + startup scan); live gate (genuine remote-session liveness confirmation) | `cargo test -p rdpilot-daemon --test crash_restart_reconcile` (offline portion); separate live-gate plan for the remote-target portion | ❌ Wave 0 |

**Note on SESSION-01/04's concurrency test and a real RDP target:** the registry-concurrency race (Pattern 1) is a pure Rust-concurrency property independent of what `Session::connect` actually does — it can be tested by substituting a slow-but-deterministic async stub in place of a real `Session::connect` call (e.g. a `tokio::time::sleep` + a toggleable success/failure), fully offline, with no Windows target needed. The registry logic itself should be written generically enough (or behind a small trait) that the test doesn't need `rdpilot`'s real `Session` type at all. Flag this design choice for the planner — it is what makes SC#1's BLOCKING-adjacent concurrency test fully offline-testable.

### Sampling Rate

- **Per task commit:** the relevant fast unit tests for the file(s) touched (`cargo test -p rdpilot-daemon <module>::`)
- **Per wave merge:** `RUSTUP_TOOLCHAIN=stable-x86_64-unknown-linux-gnu cargo test -p rdpilot-daemon --target x86_64-unknown-linux-gnu` (excludes `--ignored` soak/live tests)
- **Phase gate:** full suite including `--ignored` soak test green before `/gsd-verify-work`; Windows-`cfg`-gated DACL code and DAEMON-04's genuine remote-liveness confirmation explicitly deferred to a live-gate plan (mirroring Phase 10's 10-05), not blocking the offline phase gate

### Wave 0 Gaps

- [ ] `rdpilot-ipc` extension: `Request::Connect/List/Disconnect`, `WireResponse::Connected`, `WireErrorCode::DuplicateSession`, `SessionScoped::session() -> Option<&SessionId>` — this is a prerequisite, not a test gap per se, but blocks every test below
- [ ] `crates/rdpilot-daemon/` crate scaffold + workspace `Cargo.toml` member addition
- [ ] `tests/registry_concurrency.rs` — covers SESSION-01/04, SC#1
- [ ] `tests/thread_leak_soak.rs` — covers DAEMON-01, SC#3 [BLOCKING]
- [ ] `tests/ipc_security.rs` — covers DAEMON-02, SC#4 [BLOCKING] (Unix portion; Windows portion needs the pinned machine)
- [ ] `tests/autostart_lifecycle.rs` — covers DAEMON-03, SC#5 [BLOCKING, partial]
- [ ] `tests/crash_restart_reconcile.rs` — covers DAEMON-04, SC#5 [BLOCKING, partial]
- [ ] A registry design decision (trait-based `Session`-connect seam, or similar) enabling the concurrency test to run without a real RDP target — flagged above, needed before `registry_concurrency.rs` can be written

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | Partial — the daemon does not authenticate RDP credentials itself (that's `rdpilot`'s NLA/CredSSP path, already validated in v1.0); the daemon's OWN "authentication" is local-OS-identity-based (peer uid / DACL), not a credential scheme | Peer uid check (Unix) / DACL (Windows) — see DAEMON-02 |
| V3 Session Management | Yes — the daemon's session registry IS the thing being managed; "session" here means the RDP session object, not a web/auth session, but the same rigor applies (unique identity, explicit lifecycle, no orphaned state) | Atomic claim-then-connect (Pattern 1); explicit close-not-drop (Pattern 2); disk-persisted reconciliation state (Pitfall 9) |
| V4 Access Control | Yes — DAEMON-02's entire point is access control at the IPC boundary | `0700` dir + peer_cred() (Unix); explicit SID-scoped DACL + first_pipe_instance (Windows) — never a default/null security descriptor |
| V5 Input Validation | Yes — every `Request` variant's fields (paths, hwnd, host strings) flow into `rdpilot`'s already-validated paths (`Error::PathTraversal`, `Error::CoordinateOutOfBounds`, etc. — v1.0/Phase 10 already ship this validation); the daemon's NEW surface (the `Connect` request's host/port/credentials) has no additional validation need beyond what `ConnectionConfig`/IronRDP already perform at connect time | Rely on existing `rdpilot::Error` variants surfacing through the D-28 mapping; do not re-validate what `rdpilot` already validates |
| V6 Cryptography | No new cryptography introduced by this phase — TLS/NLA is `rdpilot`'s existing `rustls` path; the IPC transport is LOCAL (no network, no TLS needed — local-user-scoping via OS primitives is the correct control here, not cryptographic) | N/A |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Named-pipe squatting (attacker pre-creates the pipe name) | Spoofing / Elevation of Privilege | `first_pipe_instance(true)` — creation fails loudly instead of silently attaching to the attacker's instance |
| Default/null Windows security descriptor on a named pipe | Elevation of Privilege | Always pass an explicit, current-user-SID-scoped `SECURITY_ATTRIBUTES`; never `None`/null |
| TOCTOU on Unix socket directory permissions | Tampering | `DirBuilder::new().mode(0o700).create(...)` — mode applied atomically at creation, not `chmod`'d after the fact |
| Credential leakage via Serialize (any future `Request::Connect` logging) | Information Disclosure | The `Connect` request DOES legitimately carry a plaintext password over the wire once (D-31: "supplied ONCE to the daemon on connect... held daemon-side only") — but the daemon MUST NOT log the raw `Request` value (its `Debug`/logging must redact the `Connect` variant's password field the same way `ConnectionConfig`'s manual `Debug` impl already does) |
| Stale socket file after `kill -9` reused by a different (possibly malicious) process | Spoofing | Bind-as-mutex: attempt connect-then-unlink-then-rebind on a stale socket special-file, never assume an existing file means "safe to skip setup" |

## Sources

### Primary (HIGH confidence)
- `crates/rdpilot/src/session.rs` (this repo) — `Session`'s dedicated-OS-thread model, `close()`'s `spawn_blocking(join)` path, `Drop`'s non-joining fallback — read directly, ground truth for Pitfall 1
- `crates/rdpilot/src/error.rs`, `crates/rdpilot-ipc/src/{error,request,response,session_id}.rs`, `crates/rdpilot-config/src/{lib,resolved,resolve,paths}.rs` — read directly, ground truth for the Open Questions §1 finding and the D-27/D-28 mapping details
- crates.io API (`curl -A "Mozilla/5.0" https://crates.io/api/v1/crates/<pkg>`), queried live this session (2026-07-11) — `interprocess` 2.4.2, `tokio` 1.52.3, `dashmap` 6.2.1 (default) / 7.0.0-rc2 (max), `directories` 6.0.0, `sysinfo` 0.39.6, `windows-permissions` 0.2.4, `libc` 0.2.186 (default), `nix` 0.31.3, `uzers` 0.12.2
- docs.rs fetch of `tokio::net::windows::named_pipe::ServerOptions` (tokio 1.52.3) — `create_with_security_attributes_raw` exact signature, `first_pipe_instance`, `max_instances`, `pipe_mode` confirmed
- docs.rs fetch of `tokio::net::UnixStream` (tokio 1.52.3) — `peer_cred() -> Result<UCred>`, confirmed stable
- slopcheck 0.6.1 (installed and run this session) — `[OK]` on all 8 candidate crates.io packages

### Secondary (MEDIUM confidence)
- `interprocess` crate GitHub repo + docs.rs `local_socket` module summaries (WebFetch) — confirmed `ListenerOptions`/`ListenerOptionsExt::mode()` (Unix), general shape of the cross-platform API; version 2.4.2 confirmed
- Rust std `os::unix::net::UnixStream::peer_cred` unstable-feature status (WebSearch, cross-checked against the tokio-stable finding above)

### Tertiary (LOW confidence)
- `interprocess` crate's Windows `os::windows::named_pipe::PipeListenerOptions` exact security-descriptor API surface — WebSearch/WebFetch summaries only, could not obtain a fully-rendered docs.rs page this session; flagged as Open Question 2
- `windows-permissions` 0.2.4's exact `SecurityDescriptor` SDDL-construction API — WebFetch returned no usable detail; flagged as Assumption A2

## Metadata

**Confidence breakdown:**
- Standard stack: MEDIUM-HIGH — `interprocess`/`tokio`/`dashmap`/`directories`/`sysinfo` versions and Unix-side APIs are HIGH (crates.io + docs.rs verified); the Windows-side DACL construction has a HIGH-confidence tokio-native fallback but the `interprocess`-native path is unverified (MEDIUM/LOW)
- Architecture: HIGH — the registry/thread-lifecycle patterns are derived directly from this codebase's own documented, already-shipped `Session` contract, not speculation
- Pitfalls: HIGH for Pitfalls 1/3/9 (directly evidenced by codebase comments and the D-19/D-31 decision text); MEDIUM for Pitfall 5's exact Windows API names (same caveat as Standard Stack)

**Research date:** 2026-07-11
**Valid until:** 30 days for the architecture/pitfalls content (stable, codebase-derived); recommend re-verifying the `interprocess`/`windows-permissions` exact Windows API surface at implementation time regardless of elapsed time, since it was never conclusively confirmed in this session
