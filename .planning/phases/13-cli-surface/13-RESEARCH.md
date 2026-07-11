# Phase 13: CLI Surface - Research

**Researched:** 2026-07-11
**Domain:** Rust CLI (clap 4.x) as a thin `rdpilot-ipc` client; daemon-side wire-protocol extension + dispatch wiring for the perception/input/launch/file verb set
**Confidence:** HIGH (stack/versions, exact source contracts) / MEDIUM (new architecture recommendations not yet compiled/tested)

## Summary

Phase 13 has two genuinely separable halves, and the ROADMAP/CONTEXT already say so: (1) **wire + daemon-dispatch wiring** — `rdpilot-ipc` is missing DTOs for the rich perception/input verb set, and `rdpilot-daemon`'s dispatch currently returns `Internal("not implemented")` for every operational verb it *does* already have a wire shape for; (2) **the CLI binary itself**, a thin `clap` 4.6.1 client that is almost mechanical once (1) exists.

The most consequential finding in this research is **not** about clap — it's two pre-existing architecture gaps in the Phase 12 codebase that Phase 13 must close before any operational verb can be wired at all:

1. **`ManagedSession` (the daemon's registry-facing session trait) only exposes `close()`/`describe()`.** There is no seam to call `screenshot`/`send_mouse`/`get_uia_tree`/etc. on the live `Box<dyn ManagedSession>` the registry holds. The trait must be extended with `&self`-based async operational methods, and — because a `std::sync::MutexGuard` around the registry's `HashMap` cannot be held across an `.await` in this single-`LocalSet`-OS-thread daemon (`server.rs`'s own doc comment) without deadlocking sibling tasks — the registry's storage must change from `Box<dyn ManagedSession>` to `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` so an operation can be dispatched without holding the outer registry lock. See "Daemon Dispatch Wiring" below for the exact pattern and why the more obvious `Arc<dyn ManagedSession>` + `Arc::into_inner` idea does **not** compile (you cannot move an unsized `dyn Trait` value out of an `Arc`).

2. **`rdpilot-daemon::autostart::connect_or_spawn` and `ipc::unix::socket_path`/`bind` are doc-commented as "consumed later by the Phase 13/14 CLI/MCP thin clients"** — but `rdpilot-daemon`'s `Cargo.toml` depends directly on `rdpilot` (full IronRDP/rustls/tokio-full stack). If `rdpilot-cli` added `rdpilot-daemon` as a dependency to reuse that code, it would transitively pull in the entire RDP protocol stack, **directly violating** CONTEXT.md's locked "NOT `rdpilot` directly — thin client, no IronRDP" invariant. The socket-path resolution, length-prefixed JSON framing (`read_frame`/`write_frame`, currently private to the daemon crate), and the `connect_or_spawn` helper must be **relocated to `rdpilot-ipc`** (the crate both the daemon and the CLI already depend on, zero-`rdpilot`-dependency by design) before the CLI can use them.

Everything else — the clap command tree, the `--session`/`--json`/`--output`/`--force` flag surface, the wire DTO shapes for the new verbs, the CLI-03 exit-code mapping, and the offline-vs-batched-live testability split — is straightforward mechanical work once these two seams are fixed.

**Primary recommendation:** Sequence Phase 13 as three waves: **Wave 1** relocates the shared transport primitives from `rdpilot-daemon` into `rdpilot-ipc` and extends `rdpilot-ipc::Request`/`WireResponse` with the new perception/input DTOs (offline, no daemon dependency); **Wave 2** extends `ManagedSession` + the registry storage and wires `dispatch.rs`'s exhaustive match to call the real `rdpilot::Session` methods (offline, fake-connector-tested exactly like Phase 12's own tests); **Wave 3** builds the new `rdpilot-cli` binary crate against the now-complete wire surface. This ordering means Wave 3 never blocks on anything Wave 1/2 didn't already prove offline.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Session lifecycle (connect/list/disconnect) | API/Backend (`rdpilot-daemon`) | Client (`rdpilot-cli`) | Daemon owns the registry; CLI is invoke-and-exit and cannot hold state (D-17) |
| Perception (screenshot/world_state/uia/window+process list) | API/Backend (`rdpilot-daemon` → `rdpilot::Session`) | Client (`rdpilot-cli`, renders/writes) | Daemon owns the live RDP session and sensor DVC channel; CLI only renders the reply |
| Input (click/type/key/scroll/drag) | API/Backend (`rdpilot-daemon` → `rdpilot::Session`) | — | Coordinate bounds-check and PDU construction happen inside `Session`; CLI passes through raw args |
| Launch/foreground | API/Backend (`rdpilot-daemon` → `rdpilot::Session`) | — | Sensor round-trip owned by the daemon's `Session` handle |
| File transfer (put/get) | API/Backend (`rdpilot-daemon`, local `fs` + RDPDR) | Client (`rdpilot-cli`, path resolution + no-clobber check) | Daemon and CLI run on the SAME machine — bytes never cross the IPC wire; only paths + `TransferOutcome` metadata do |
| Config resolution (host/creds) | Client (`rdpilot-cli` via `rdpilot-config`) | API/Backend (daemon receives resolved values in `Request::Connect`) | D-27: file→env→flag precedence resolves client-side; the daemon only ever sees the final values |
| IPC transport (framing, socket path, auto-start) | Shared library (`rdpilot-ipc`, relocated from `rdpilot-daemon`) | Both `rdpilot-cli` and `rdpilot-daemon` | Must be byte-identical on both sides of the socket; currently daemon-only, must move (see Summary finding 2) |
| Error taxonomy → exit codes | Client (`rdpilot-cli`) | — | `WireErrorCode`→exit-code mapping is a CLI-only concern (MCP will do its own code+message rendering in Phase 14) |

## User Constraints

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-13.1 — CLI shape: GROUPED noun-verb subcommands with flat lifecycle/file verbs; human tables by default, `--json` opt-in; clap 4.x.**
  The CLI is organized as grouped noun-verb subcommand families — `session`, `perceive`, `input`, `file` — for the perception/input/file verb sets, while the session-lifecycle verbs (`connect` / `list` / `disconnect`) and the file transfer verbs (`put` / `get`) are ALSO exposed FLAT at the top level for ergonomics (e.g. `rdpilot connect`, `rdpilot put`, `rdpilot get`). Default output is HUMAN-READABLE TABLES; a `--json` flag opts into machine-readable output for scripting (PROOF-02 in Phase 15 consumes the CLI exclusively via `--json`). The `screenshot` verb writes its image to a REQUIRED `--output <path>` argument (binary image bytes are never dumped to stdout). Built on `clap` 4.x (per D-26). Perception/input verb naming is to be FINALIZED in planning — the grouped-vs-flat structure and the table/`--json`/`--output` contract are locked here.

- **Lifecycle + file verb set is fixed:** `connect` / `list` / `disconnect` (CLI-01) and `put` / `get` (CLI-03) are the committed lifecycle and file verbs.
- **File transfer default is no-clobber:** `put`/`get` refuse to overwrite an existing destination by default; an explicit `--force` flag is the only override.

### Claude's Discretion

- Exact final spelling of each perception/input verb and whether they live only under their noun group, only flat, or both (D-13.1 fixes the grouping model, not every leaf name).
- Table-rendering approach (hand-rolled vs a formatting crate) — keep dependency footprint minimal and consistent with D-26's stack.
- How `--session` is surfaced in clap (global arg vs per-subcommand) so long as D-29's "required on every command" contract holds.

### Deferred Ideas (OUT OF SCOPE)

No scope-creep ideas surfaced during this phase's context gathering. The MCP `put`/`get` tool, the scripted CLI proof harness (PROOF-02), and the daemon/IPC/config crates all belong to other phases (14, 15, 12, 11 respectively) and are already tracked there.

### Inherited / Cross-Cutting Decisions (from DECISIONS-INDEX.md)

- **D-27 (CC1 — Config):** CLI resolves target host + credentials through `config.toml` (platform config dir) → `RDPILOT_`-prefixed env vars → CLI flags. CLI flag spellings MUST reuse the config-key vocabulary verbatim (`host`, `port`, `username`, `password`, `domain`, `cert-bypass`... **NOTE**: the actual `ResolvedConfig`/`ConnectionConfig` field name is `accept_invalid_certs`, not `cert-bypass` — see "Pitfalls" below).
- **D-28 (CC2 — Error taxonomy → exit codes):** CLI maps `WireError{code,message}`'s fixed code set to a DISTINCT non-zero process exit code per class.
- **D-29 (CC3 — `--session` on every command):** No implicit/default session target; every session-scoped verb requires `--session`.
- **D-30 (CC4 — lifecycle status vocabulary):** `rdpilot list` renders `Connecting`/`Live`/`Reconnecting`/`Disconnected`/`Orphaned`.
- **D-16:** v1.1 ships exactly CLI + MCP; no bindings, no clipboard.
- **D-17:** CLI is a THIN client of the daemon — no embedded/duplicated session logic.
- **D-18:** Every command explicitly targets a session by name/id.
- **D-26:** Stack: `clap` 4.6.1 for the CLI; `interprocess` 2.4.2 / native named-pipe for IPC to the daemon (Phase 12 chose the native `tokio::net` path — see "State of the Art" below for why the CLI should follow suit, not introduce `interprocess`).
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| CLI-01 | `rdpilot` CLI manages session lifecycle (connect [--name] / list / disconnect) over the daemon, transparently auto-starting it | `connect_or_spawn` already exists in `rdpilot-daemon::autostart` and is proven by `tests/autostart_lifecycle.rs` — relocate it to `rdpilot-ipc` (see Summary #2) and call it unmodified from `rdpilot-cli` |
| CLI-02 | CLI exposes the full perception + input + launch verb set, each targeting a named session | Exact `rdpilot::Session` method signatures confirmed by source read (see "Session Method Contract" table); wire DTO shapes proposed in "Wire Protocol Extension" |
| CLI-03 | CLI exposes file put/get and reports errors clearly (session-not-found, daemon-unreachable, transfer failure) | `WireErrorCode` already has all 3 needed codes (D-28); `Request::Put`/`Get` already exist; no-clobber semantics + exit-code mapping proposed below |
</phase_requirements>

## Project Constraints (from CLAUDE.md)

- Rust primary language; `rdpilot` (SDK) crate must never appear as a dependency of `rdpilot-cli` — only `rdpilot-ipc` + `rdpilot-config`.
- No `unsafe` in library code except the one documented, localized exception in `rdpilot-daemon::ipc::unix::effective_uid` — `rdpilot-cli` should carry `#![deny(unsafe_code)]` like every other crate in this workspace.
- Every crate in this workspace uses `#![deny(clippy::unwrap_used)]` / `#![deny(clippy::expect_used)]` — `rdpilot-cli` must match this convention (no panics on malformed CLI input or wire errors).
- GSD workflow enforcement: file-changing work must go through `/gsd-execute-phase`, not direct edits.

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `clap` | 4.6.1 `[VERIFIED: crates.io registry, 2026-07-11 — max_stable_version 4.6.1]` | CLI argument/subcommand parsing (derive API) | Already locked by D-26; confirmed current on crates.io at research time |
| `rdpilot-ipc` | 0.1.0 (workspace path dep) | Wire DTOs + (after relocation) transport/framing/socket-path/auto-start helpers | Already the zero-`rdpilot`-dependency crate both daemon and CLI must share |
| `rdpilot-config` | 0.1.0 (workspace path dep) | Layered host/credential resolution (file→env→flag) | Already built in Phase 11; CLI supplies the flag layer only |
| `tokio` | 1.x, feature set TBD (at minimum `rt`, `net`, `io-util`, `macros`, `time`) `[VERIFIED: crates.io registry — max_stable_version 1.52.3]` | Async runtime for the CLI's single request/response round trip | Already a workspace-wide dependency; the CLI needs it to drive one `UnixStream`/named-pipe round trip and `connect_or_spawn`'s backoff sleeps |
| `serde_json` | 1.x | JSON encode/decode for `--json` output and wire framing | Already used end-to-end by `rdpilot-ipc`/`rdpilot-daemon` |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `base64` | 0.22.x `[ASSUMED — not yet a workspace dependency; verify exact current version before adding]` | Decode `WireResponse::Screenshot{png_base64}`/`WorldState.screenshot` back to raw PNG bytes for `--output` | Only needed by the CLI's screenshot-writing code path — a single `general_purpose::STANDARD.decode(...)` call |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-rolled column-aligned table printer | `comfy-table` or `tabled` | CONTEXT.md's Claude's-Discretion note explicitly asks for minimal dependency footprint "consistent with D-26's stack" — D-26 does not list a table crate, and Phase 12 already declined the one extra transport crate D-26 suggested (`interprocess`) in favor of the native `tokio::net` API already in the dependency graph. **Recommendation: hand-roll** a small column-width-computing printer (≈40-60 lines) rather than add a new dependency; `list`'s columns (id/name/host/status/connected_since/last_activity) and `window`/`process` list output are simple enough that padding-to-max-column-width is sufficient. Revisit only if the planner finds the hand-rolled version genuinely awkward. |
| `rdpilot-daemon`-hosted transport helpers | Duplicate the ~150 lines of `socket_path`/`bind`-adjacent/`connect_or_spawn`/framing code directly into `rdpilot-cli` | Duplication risks the CLI and daemon computing two DIFFERENT socket paths on a platform/env edge case (e.g. a future `XDG_RUNTIME_DIR` change), silently breaking auto-start. **Recommendation: relocate to `rdpilot-ipc`**, not duplicate. |
| `Arc<dyn ManagedSession>` for registry storage | `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` | The simpler `Arc<dyn ManagedSession>` cannot support `close()`'s owned-`Box<Self>` contract — Rust cannot move an unsized `dyn Trait` value out of an `Arc` (`Arc::into_inner`/`Arc::try_unwrap` require `T: Sized`). The `Mutex<Option<Box<...>>>` wrapper sidesteps this because the `Box` (a sized pointer) is what moves, not the trait object itself. See "Daemon Dispatch Wiring" for the full pattern. |

**Installation:**
```bash
# In crates/rdpilot-cli/Cargo.toml (new crate)
clap = { version = "4.6.1", features = ["derive"] }
rdpilot-ipc = { path = "../rdpilot-ipc" }
rdpilot-config = { path = "../rdpilot-config" }
tokio = { version = "1", features = ["rt", "net", "io-util", "macros", "time"] }
serde_json = "1"
base64 = "0.22"  # verify current version before pinning — see Package Legitimacy Audit
```

**Version verification:** `clap` (4.6.1), `directories` (6.0.0), `interprocess` (2.4.2, NOT recommended for use — see above), and `tokio` (1.52.3) were confirmed live against the crates.io registry API on 2026-07-11 (`curl -H "User-Agent: ..." https://crates.io/api/v1/crates/<name>`). `base64` was not checked against the registry this session — the planner/executor should run `cargo add base64` (or `cargo search base64`) at implementation time and confirm the actual current version rather than trusting the `0.22.x` guess above.

## Package Legitimacy Audit

`slopcheck` (`/home/marc/.local/bin/slopcheck`, already installed on this host) was run against the two genuinely new-to-this-phase packages:

```
slopcheck install clap directories
  [OK] directories (crates.io)
  [OK] clap (crates.io)
  scanned 2 packages -- 2 OK
```

(The actual `cargo add` step inside `slopcheck install` failed only because this sandboxed research environment has no `cargo` on `PATH` — the scan-and-verdict step, which is what matters for this gate, completed successfully before that.)

| Package | Registry | Age | Downloads | Source Repo | slopcheck | Disposition |
|---------|----------|-----|-----------|-------------|-----------|-------------|
| `clap` | crates.io | Multi-year, de facto standard Rust CLI crate | Very high (top-20 crates.io downloads) | github.com/clap-rs/clap | [OK] | Approved — already a locked D-26 decision, re-verified current (4.6.1) |
| `directories` | crates.io | Multi-year, already a transitive/direct dependency of `rdpilot-daemon`/`rdpilot-config` | High | github.com/dirs-dev/directories-rs | [OK] | Approved — only relevant if `socket_path` relocation lands in `rdpilot-ipc`, which would then need to add this dependency (currently absent from `rdpilot-ipc/Cargo.toml`) |
| `base64` | crates.io | `[ASSUMED — not verified this session]` | `[ASSUMED]` | `[ASSUMED]` | not run | **Flagged — planner must add a `checkpoint:human-verify` (or re-run `slopcheck install base64`) before this dependency is actually added to `rdpilot-cli/Cargo.toml`.** `base64` is an extremely common, long-established crate (docs.rs top-tier), so risk is low, but it was not run through the legitimacy gate this session because a `cargo`-less environment blocked the full `slopcheck install` flow after the scan step for `clap`/`directories`. |

**Packages removed due to slopcheck [SLOP] verdict:** none.
**Packages flagged as suspicious [SUS]:** none scanned this session (`base64` was never submitted to the scanner — treat as unverified, not as scanned-and-clean).

## Session Method Contract (the exact signatures dispatch must call)

Read directly from `crates/rdpilot/src/session.rs` (public API surface confirmed against `crates/rdpilot/src/lib.rs`'s `pub use` list — every type below is publicly re-exported from the `rdpilot` crate root):

| Verb (CLI-02 name) | `rdpilot::Session` method | Signature | Notes |
|---|---|---|---|
| screenshot | `screenshot` | `pub async fn screenshot(&self) -> Result<Screenshot>` | `Screenshot{width,height,rgba}` — `rgba` is `#[serde(skip)]`; only `.to_png() -> Result<Vec<u8>>` yields transportable bytes |
| world_state | `world_state` | `pub async fn world_state(&self, opts: WorldStateOptions) -> Result<WorldState>` | `WorldState{timestamp: SystemTime, capture_span: Duration, screenshot: Option<Screenshot>, window_list: Option<Vec<WindowInfo>>, uia: Option<Vec<(u64, Vec<UiaElement>)>>}`; `SystemTime`/`Duration` have no serde impl — dispatch must convert |
| uia | `get_uia_tree` | `pub async fn get_uia_tree(&self, hwnd: u64, scope: UiaScope) -> Result<Vec<UiaElement>>` | `UiaScope::Children` (default) or `UiaScope::Subtree{max_depth: u32}` |
| window list | `get_window_list` | `pub async fn get_window_list(&self) -> Result<Vec<WindowInfo>>` | `WindowInfo{hwnd:u64, title:String, rect:Rect, z_order:u32, state:WindowState, class_name:String, pid:u32}` |
| process list | `get_process_tree` | `pub async fn get_process_tree(&self) -> Result<Vec<ProcessInfo>>` | `ProcessInfo{pid,parent_pid:u32, name,path:String, command_line:Option<String>, owner:Option<String>}` |
| click/move/scroll/drag | `send_mouse` | `pub async fn send_mouse(&self, action: MouseAction) -> Result<()>` | `MouseAction` enum: `Move{x,y}` / `Click{x,y,button}` / `DoubleClick{x,y,button}` / `Scroll{x,y,dy}` / `Drag{from_x,from_y,to_x,to_y,button}`; all coords `u16`, `dy: i16`, `button: Button{Left,Right,Middle}` — bounds-checked against `desktop_size()` INSIDE `Session`, no CLI-side duplicate check needed |
| type/key | `send_key` | `pub async fn send_key(&self, action: KeyAction) -> Result<()>` | `KeyAction::Type(String)` / `KeyAction::Combo(Vec<Key>)`; `Key` is a large named-virtual-key enum (A-Z, Digit0-9, F1-F12, Ctrl/Alt/Shift/Win, arrows, etc. — full list in `crates/rdpilot/src/input.rs`) |
| launch | `launch_process` | `pub async fn launch_process(&self, exe: &str, args: Option<&str>, cwd: Option<&str>) -> Result<u32>` | Returns the new PID; fire-and-forget, no completion wait |
| foreground | `set_foreground_window` | `pub async fn set_foreground_window(&self, hwnd: u64) -> Result<()>` | Caller (CLI/human) must follow up with `window list` to confirm — the sensor only confirms the Win32 call itself succeeded |
| put | `upload_file` | `pub async fn upload_file(&self, local: &Path, remote_name: &str) -> Result<TransferOutcome>` | `TransferOutcome{bytes_transferred:u64, checksum:String}`; fails `Error::Config` if no `share_root` configured at connect time — the DAEMON must configure `share_root` when building `ConnectionConfig` in `Request::Connect` handling (verify this is already wired; if not, it's a Phase-13-adjacent Phase-12 gap worth flagging to the planner) |
| get | `download_file` | `pub async fn download_file(&self, remote_name: &str, local: &Path) -> Result<TransferOutcome>` | Same `share_root` precondition as `upload_file` |
| (already wired) list | `Registry::list` (not a `Session` method) | `pub fn list(&self) -> Vec<SessionStatus>` | Already fully wired in Phase 12 (`dispatch.rs`'s `Request::List` arm) — no Phase 13 work needed here beyond CLI rendering |
| (already wired) connect/disconnect | `Registry::open`/`Registry::close` | — | Already fully wired in Phase 12 — no Phase 13 daemon-side work, only CLI rendering |

**Not part of CLI-02's locked verb list** (available on `Session` but out of scope this phase): `screenshot_window` (client-side crop, no sensor round trip — could be a nice-to-have `--window <hwnd>` flag on the `screenshot` verb later, not required), `ping` (already wired as `Request::Ping`→`WireResponse::Ack` from Phase 11's wire shape — dispatch should wire it since the DTO already exists, but it is not in CLI-02's named verb list; low-cost to include as a `session ping`/diagnostic verb), `deploy_and_launch` (bootstrap-only, invoked automatically at connect time, never a standalone CLI verb).

## Daemon Dispatch Wiring

### The gap

`crates/rdpilot-daemon/src/dispatch.rs`'s `dispatch()` function has an EXHAUSTIVE match over `rdpilot_ipc::Request` (no wildcard arm — a documented, deliberate forcing function). Every operational verb (`Ping`, `Screenshot`, `LaunchProcess`, `SetForeground`, `Put`, `Get`) currently routes to `not_implemented_for()`, which returns `WireErrorCode::Internal` for a known session or `SessionNotFound` for an unknown one. Phase 13 must replace each of these arms with a real call — but the registry has no way to reach into a live session's operational methods today.

`crates/rdpilot-daemon/src/seams.rs`'s `ManagedSession` trait is:

```rust
pub trait ManagedSession: Send + 'static {
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>>;
    fn describe(&self) -> SessionLifecycle;
}
```

`Registry`'s `SessionEntry::Live` variant stores `session: Box<dyn ManagedSession>`. `close()` deliberately consumes ownership (`self: Box<Self>`) because it must join `rdpilot::Session`'s dedicated OS thread — a contract that comes from `rdpilot::Session::close(mut self)` itself (SDK-level, untouchable this phase). There is no seam for a `&self`-based operational call.

### Why the "obvious" fix doesn't compile

The natural-looking fix — change storage to `Arc<dyn ManagedSession>`, clone the `Arc` under the registry's synchronous lock, drop the lock, `.await` the operation on the clone, and use `Arc::try_unwrap`/`Arc::into_inner` to reclaim ownership for `close()` — **does not compile**. `Arc<T>::into_inner`/`try_unwrap` require `T: Sized`; `dyn ManagedSession` is an unsized trait object, and Rust cannot move an unsized value out of an `Arc` by value under any stable API.

### Recommended pattern

Change `SessionEntry::Live.session` from `Box<dyn ManagedSession>` to `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>`, and extend `ManagedSession` with `&self`-based async operational methods (same manual-`BoxFuture` shape `close`/`connect` already use — these traits are `dyn`-used, so native `async fn` in the trait is not usable without boxing by hand, exactly as `seams.rs`'s own doc comment explains for the existing methods):

```rust
pub trait ManagedSession: Send + 'static {
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), DaemonError>>;
    fn describe(&self) -> SessionLifecycle;

    // New, Phase 13:
    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>>;
    fn world_state(&self, opts: rdpilot::WorldStateOptions) -> BoxFuture<'_, Result<rdpilot::WorldState, DaemonError>>;
    fn get_window_list(&self) -> BoxFuture<'_, Result<Vec<rdpilot::WindowInfo>, DaemonError>>;
    fn get_process_tree(&self) -> BoxFuture<'_, Result<Vec<rdpilot::ProcessInfo>, DaemonError>>;
    fn get_uia_tree(&self, hwnd: u64, scope: rdpilot::UiaScope) -> BoxFuture<'_, Result<Vec<rdpilot::UiaElement>, DaemonError>>;
    fn send_mouse(&self, action: rdpilot::MouseAction) -> BoxFuture<'_, Result<(), DaemonError>>;
    fn send_key(&self, action: rdpilot::KeyAction) -> BoxFuture<'_, Result<(), DaemonError>>;
    fn set_foreground_window(&self, hwnd: u64) -> BoxFuture<'_, Result<(), DaemonError>>;
    fn launch_process(&self, exe: String, args: Option<String>, cwd: Option<String>) -> BoxFuture<'_, Result<u32, DaemonError>>;
    fn upload_file(&self, local: std::path::PathBuf, remote_name: String) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>>;
    fn download_file(&self, remote_name: String, local: std::path::PathBuf) -> BoxFuture<'_, Result<rdpilot::TransferOutcome, DaemonError>>;
    fn ping(&self) -> BoxFuture<'_, Result<std::time::Duration, DaemonError>>;
}

impl ManagedSession for Session {
    // close()/describe() unchanged.
    fn screenshot(&self) -> BoxFuture<'_, Result<rdpilot::Screenshot, DaemonError>> {
        Box::pin(async move { self.screenshot().await.map_err(DaemonError::Sdk) })
    }
    // ... one line per method, all delegating directly and mapping via the
    // already-existing `DaemonError::Sdk(#[from] rdpilot::Error)` variant —
    // identical pattern to `RealConnector::connect`'s existing body.
}
```

`Registry` gains a method that clones the `Arc` under the synchronous outer lock, drops the lock, then `.lock().await`s the INNER `tokio::sync::Mutex` (safe and idiomatic to hold across `.await` — that's what `tokio::sync::Mutex` is for) to reach the boxed session:

```rust
pub async fn call<T>(
    &self,
    id: &SessionId,
    op: impl for<'a> FnOnce(&'a dyn ManagedSession) -> BoxFuture<'a, Result<T, DaemonError>>,
) -> Result<T, DaemonError> {
    let entry = {
        let guard = self.sessions.lock().expect("registry mutex poisoned");
        match guard.get(id) {
            Some(SessionEntry::Live { session, .. }) => Arc::clone(session),
            Some(SessionEntry::Connecting { .. }) => return Err(DaemonError::StillConnecting(id.as_str().to_owned())),
            Some(SessionEntry::Orphaned { .. }) | None => return Err(DaemonError::SessionNotFound(id.as_str().to_owned())),
        }
    }; // outer lock dropped here — never held across the .await below

    let guard = entry.lock().await; // tokio::sync::Mutex — safe across .await
    match guard.as_deref() {
        Some(session) => op(session).await,
        None => Err(DaemonError::SessionNotFound(id.as_str().to_owned())), // closed mid-flight
    }
}
```

`Registry::close` changes to acquire the SAME inner `tokio::sync::Mutex`, `.take()` the `Option<Box<dyn ManagedSession>>` (this moves a *sized* `Box`, not the unsized trait object — legal), and `.close().await` it, before removing the outer map entry — preserving the existing "close, never bare-drop" invariant (DAEMON-01) exactly.

**Why this is correct, not just compiling:** every `rdpilot::Session` operational method already takes `&self` (verified above) and is internally safe for concurrent invocation (its own `Mutex`/`mpsc` channels handle synchronization) — so the extra `tokio::sync::Mutex` around the `Box` is not needed for `Session`'s own thread-safety, it exists purely to make "eventually reclaim ownership for `close()`" possible. Its side effect — calls to the SAME session serialize, calls to DIFFERENT sessions never block each other — is exactly the property Phase 14's MCP-06 [BLOCKING] requirement ("a slow tool call does not block a concurrent unrelated fast tool call") needs, so building this now in Phase 13 sets Phase 14 up cleanly rather than requiring a rewrite (ROADMAP already notes Phase 14 is "built on the daemon/IPC foundation the CLI already validated end-to-end").

**Test-fixture impact:** the existing `FakeSession`/`FakeTestSession` test doubles in `registry.rs`, `dispatch.rs`, and `server.rs`'s own `#[cfg(test)]` modules only implement `close`/`describe` today. Every new `ManagedSession` method needs a matching (trivial, fixed-value) stub on these fakes or the crate will not compile — flag this as required Wave-2 work, not a Wave-3 (CLI) concern.

## Wire Protocol Extension

### New `rdpilot-ipc` mirror types

Following the crate's existing convention (`TransferOutcome` deliberately duplicates `rdpilot::TransferOutcome`'s shape rather than importing it — `rdpilot-ipc` must never depend on `rdpilot`), add owned, `Serialize + Deserialize` mirrors of every perception/input type the new verbs need:

```rust
// rdpilot-ipc: new module, e.g. perception.rs
#[derive(Debug, Clone, Serialize, Deserialize)] pub struct WireRect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }
#[derive(Debug, Clone, Copy, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum WireWindowState { Normal, Minimized, Maximized }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireWindowInfo { pub hwnd: u64, pub title: String, pub rect: WireRect, pub z_order: u32, pub state: WireWindowState, pub class_name: String, pub pid: u32 }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireProcessInfo { pub pid: u32, pub parent_pid: u32, pub name: String, pub path: String, pub command_line: Option<String>, pub owner: Option<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireUiaScope { Children, Subtree { max_depth: u32 } }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireUiaElement { pub id: String, pub role: String, pub name: String, pub bbox: WireRect, pub enabled: bool, pub visible: bool, pub focusable: bool, pub focused: bool, pub depth: u32, pub parent_id: String }

// rdpilot-ipc: new module, e.g. input.rs
#[derive(Debug, Clone, Copy, Serialize, Deserialize)] pub enum WireButton { Left, Right, Middle }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireMouseAction {
    Move { x: u16, y: u16 },
    Click { x: u16, y: u16, button: WireButton },
    DoubleClick { x: u16, y: u16, button: WireButton },
    Scroll { x: u16, y: u16, dy: i16 },
    Drag { from_x: u16, from_y: u16, to_x: u16, to_y: u16, button: WireButton },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)] pub enum WireKey { /* mirrors rdpilot::Key's full variant list 1:1 */ }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireKeyAction { Type(String), Combo(Vec<WireKey>) }

// world_state options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireUiaMode { None, Foreground, Hwnd(Vec<u64>), AllTopLevel }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireWorldStateOptions { pub screenshot: bool, pub window_list: bool, pub uia: WireUiaMode }
```

### New `Request` variants (all session-scoped, extending `SessionScoped`'s exhaustive match)

```rust
WindowList { session: SessionId },
ProcessList { session: SessionId },
Uia { session: SessionId, hwnd: u64, scope: WireUiaScope },
WorldState { session: SessionId, options: WireWorldStateOptions },
Mouse { session: SessionId, action: WireMouseAction },
Key { session: SessionId, action: WireKeyAction },
```

(`Ping`, `Screenshot`, `LaunchProcess`, `SetForeground`, `Put`, `Get` already exist from Phase 11 — only dispatch wiring, not new DTOs, is needed for those.)

### New `WireResponse` variants

```rust
WindowList { windows: Vec<WireWindowInfo> },
ProcessList { processes: Vec<WireProcessInfo> },
Uia { elements: Vec<WireUiaElement> },
WorldState {
    timestamp: String,                    // ISO-8601 — SystemTime has no serde impl; convert on dispatch (reuse registry.rs's existing iso8601_now-style helper for consistency with SessionStatus's own timestamp fields)
    capture_span_ms: u64,                  // Duration has no serde impl either — convert to millis
    screenshot: Option<String>,            // base64 PNG, same convention as WireResponse::Screenshot
    window_list: Option<Vec<WireWindowInfo>>,
    uia: Option<Vec<(u64, Vec<WireUiaElement>)>>,
},
```

`Mouse`/`Key`/`SetForeground` all return `WireResponse::Ack` (already exists — `Session`'s corresponding methods return `Result<()>`). `LaunchProcess` already maps to `WireResponse::Pid`. `Screenshot` already maps to `WireResponse::Screenshot{png_base64}`. `Ping` already maps to `WireResponse::Ack`. No new response variants are needed beyond `WindowList`/`ProcessList`/`Uia`/`WorldState`.

### Binary blob framing (screenshot bytes) — resolved

`WireResponse::Screenshot { png_base64: String }` **already exists** from Phase 11 — confirming the wire-level design decision was already made: PNG bytes are base64-encoded and carried inline in the same length-prefixed JSON frame every other response uses (no separate binary channel, no streaming). `Session::screenshot()` returns raw `Screenshot{rgba}`; dispatch must call `.to_png()` (already public on `Screenshot`, `crates/rdpilot/src/screenshot.rs`) before base64-encoding.

The CLI-side contract, per D-13.1 (locked): the `screenshot` verb writes the decoded PNG bytes to a REQUIRED `--output <path>` argument — the CLI base64-decodes `png_base64` and does a plain `std::fs::write`. `world_state`'s embedded `screenshot` field should get the same treatment when the caller also passes `--output` on the `world-state`/`perceive world-state` verb (discretion — recommend supporting `--output` there too for symmetry, but it is not CLI-02-blocking since `world_state`'s screenshot component is optional and off by default is arguably fine for a first cut... actually `WorldStateOptions::default()` sets `screenshot: true`, so plan to support `--output` on `world-state` too, or explicitly document that the CLI's `world-state` verb defaults to `screenshot: false` / requires an explicit `--screenshot` flag paired with `--output`).

This structurally satisfies the MCP-01/05 "file verbs return metadata only, never inline bytes" rule automatically for `put`/`get` (see next section) while correctly diverging for `screenshot`/`world_state`, which legitimately need the image bytes themselves, not a file-metadata description of them.

### `put`/`get`: no bytes cross the wire at all

`Request::Put{session, local_path: String, remote_name: String}` / `Request::Get{session, remote_name: String, local_path: String}` already exist (Phase 11). Because the daemon and the CLI run on the **same machine** (local-only IPC, DAEMON-02), `Session::upload_file`/`download_file` read/write `local_path` directly via `std::fs` on the daemon's own process — **no file bytes ever need to be serialized onto the JSON wire**. Only the path strings and the resulting `TransferOutcome{bytes_transferred, checksum}` cross the socket. This is already how the DTOs are shaped; Phase 13 only needs dispatch wiring (`session.upload_file(&PathBuf::from(local_path), &remote_name)` / mirror for download) plus one CRITICAL CLI-side fix — see the relative-path Pitfall below.

## CLI-03 Error Taxonomy → Exit Codes

`WireErrorCode` (already `#[non_exhaustive]`, defined in Phase 11, extended in Phase 12) already has every code CLI-03 needs:

| `WireErrorCode` | Origin | Recommended CLI exit code |
|---|---|---|
| (success) | — | `0` |
| `Internal` | Decision-3 catch-all for ~9 `rdpilot::Error` variants (Connect/Tls/Decode/Session/CoordinateOutOfBounds/Dvc/Bootstrap/SensorRejected/Config) | `1` (generic failure) |
| `SessionNotFound` | Unknown/mistyped `--session`, or `Disconnect`/operational verb on a gone session | `2` |
| `DaemonUnreachable` | **Client-side only** — never sent BY the daemon (if the daemon is unreachable there is no wire response at all); the CLI synthesizes this itself when `connect_or_spawn`'s bounded backoff (`[50,100,200,400,800]` ms, already defined in `autostart.rs`) exhausts without a successful connect | `3` |
| `TransferFailed` | Non-path-traversal, non-checksum `Put`/`Get` failure | `4` |
| `PathTraversal` | 1:1 from `rdpilot::Error::PathTraversal` | `5` |
| `ChecksumMismatch` | 1:1 from `rdpilot::Error::ChecksumMismatch` | `6` |
| `DuplicateSession` | `connect --name` collision | `7` |
| (CLI-local, no-clobber refusal) | `put`/`get` destination already exists and `--force` was not passed | `8` — this is a CLI-side check BEFORE the request is even sent (see next section), not a `WireErrorCode` at all |

Exact numeric assignments are the planner's call — the important finding is that **every CLI-03-required distinction (session-not-found / daemon-unreachable / transfer-failure) is already representable** with zero new `WireErrorCode` variants, and that `DaemonUnreachable` is structurally different from the other five (client-synthesized, never wire-transmitted).

## No-Clobber `put`/`get` Semantics

Per CONTEXT.md's locked decision: `put`/`get` refuse to overwrite an existing destination by default; `--force` overrides. Recommended placement: **the CLI checks this, not the daemon** — the destination-exists check is a plain `local_path.exists()`/`Path::new(&local).exists()` call the CLI can do BEFORE sending the request at all (it's a client-local filesystem check either way: `put`'s destination is remote — the sensor's C# `ValidateRemotePath`/copy-with-inline-SHA256 handler from Phase 10 does not currently implement an overwrite-refusal — so `put`'s no-clobber check may need either (a) a daemon/sensor-side check, since the CLI cannot see the remote filesystem, or (b) accepting that `put`'s no-clobber is best-effort/CLI-only-for-`get`. **Flag this asymmetry explicitly for the planner**: `get`'s destination is 100% local (CLI/daemon-machine) — a simple client- or dispatch-side `Path::exists()` check suffices. `put`'s destination is REMOTE (the sensor's transfer root) — the CLI/daemon has no cheap way to check remote existence without adding a new sensor round trip (out of scope for the existing `FileTransfer` wire variant, which only has `Upload`/`Download` ops, confirmed in `crates/rdpilot/src/session.rs`'s `upload_file`/`download_file` payload construction). **Recommendation:** implement `get`'s no-clobber check CLI-side (or dispatch-side, functionally equivalent since both run on the same machine) trivially; for `put`, either (a) accept this phase ships `put --force`-only-effective no-clobber as a documented known gap (remote overwrite is not actually prevented, only requested-and-silently-allowed), or (b) extend the C# sensor's `FileTransfer` `Upload` handler to check-then-refuse before writing when a `no_clobber` flag is set on the wire request and there's no `--force`. Given Phase 10's sensor code is out of this phase's stated scope ("the daemon itself and the session registry (Phase 12)... the sensor" is not explicitly listed as owned by Phase 13), **recommend (a)**: document the asymmetry, ship `get`'s no-clobber fully working, and flag `put`'s remote no-clobber as a known limitation for the planner to explicitly accept or escalate — do not silently under-deliver CLI-03 without calling this out.

## Architecture Patterns

### System Architecture Diagram

```
                     ┌─────────────────────────────────────────┐
                     │              rdpilot-cli                  │
                     │  (clap 4.x, no rdpilot/IronRDP dependency) │
                     └───────────────┬─────────────────────────┘
                                     │
                1. resolve config    │  rdpilot-config::resolve()
                   (file→env→flag)   │  (host/user/pass/port/domain/cert-bypass)
                                     ▼
                     ┌─────────────────────────────────────────┐
                     │   rdpilot-ipc  (relocated transport)      │
                     │   socket_path() / connect_or_spawn()       │
                     │   read_frame() / write_frame()             │
                     └───────────────┬─────────────────────────┘
                                     │  2. connect (or auto-spawn
                                     │     rdpilot-daemon binary, backoff-retry)
                                     ▼
                     ┌─────────────────────────────────────────┐
                     │            rdpilot-daemon                  │
                     │  ipc::serve_connection (LocalSet/spawn_local)│
                     │       │                                    │
                     │       ▼                                    │
                     │  dispatch::dispatch(Request) -> WireResponse│
                     │       │  3. registry.call(id, |session| ...) │
                     │       ▼                                    │
                     │  Registry (Arc<TokioMutex<Option<Box<     │
                     │            dyn ManagedSession>>>> per entry)│
                     │       │  4. .lock().await -> &dyn ManagedSession
                     │       ▼                                    │
                     │  rdpilot::Session (screenshot/send_mouse/  │
                     │    get_uia_tree/upload_file/... methods)   │
                     └───────────────┬─────────────────────────┘
                                     │  5. RDP/DVC/RDPDR over the wire
                                     ▼
                          Remote Windows target
                     (screen, input, sensor DVC channel, RDPDR drive)

  6. WireResponse flows back up the SAME path -> rdpilot-cli renders
     (table by default, --json opt-in) or writes bytes (--output for screenshot).
```

### Recommended Project Structure

```
crates/rdpilot-cli/
├── Cargo.toml
├── src/
│   ├── main.rs           # #[tokio::main], top-level error -> exit code translation
│   ├── cli.rs             # clap derive: Cli { command: Command }, Command enum (flat + grouped)
│   ├── connect.rs         # connect_or_spawn wrapper + rdpilot-ipc socket_path resolution
│   ├── config_flags.rs    # ResolvedConfig-mirroring clap flags (host/port/username/password/domain/accept-invalid-certs)
│   ├── render/
│   │   ├── mod.rs         # trait Render { fn table(&self) -> String; fn json(&self) -> serde_json::Value; }
│   │   ├── table.rs       # hand-rolled column-aligned printer
│   │   └── responses.rs   # WireResponse -> Render impls
│   ├── exit_codes.rs      # WireErrorCode -> process::ExitCode mapping (D-28)
│   └── verbs/
│       ├── session.rs     # connect/list/disconnect (flat + `session` group)
│       ├── perceive.rs    # screenshot/world-state/uia/window-list/process-list
│       ├── input.rs       # click/type/key/scroll/drag
│       └── file.rs        # put/get (flat + `file` group)
```

### Pattern 1: Global `Cli` struct with a top-level `Command` enum mixing flat and grouped variants

**What:** clap derive's `#[derive(Subcommand)]` enum can freely mix leaf variants (`Connect(ConnectArgs)`) with nested-subcommand variants (`Session(SessionCommand)` where `SessionCommand` is itself a `#[derive(Subcommand)]` enum) in the same enum — this directly implements D-13.1's "both flat and grouped" requirement without duplicating argument-parsing code (the flat `Connect` variant and the grouped `Session::Connect` variant can share one `ConnectArgs` struct).
**When to use:** Exactly this phase's locked shape.
**Example:**
```rust
// Source: clap 4.x derive API (docs.rs/clap — mixing flat + nested subcommands is a standard, documented pattern)
#[derive(clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Machine-readable JSON output instead of a human table.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(clap::Subcommand)]
enum Command {
    Connect(ConnectArgs),           // flat: `rdpilot connect ...`
    List,                            // flat: `rdpilot list`
    Disconnect { session: String },  // flat: `rdpilot disconnect --session ...`
    Put(PutArgs),                    // flat: `rdpilot put ...`
    Get(GetArgs),                    // flat: `rdpilot get ...`
    #[command(subcommand)]
    Session(SessionCmd),             // grouped: `rdpilot session connect ...`
    #[command(subcommand)]
    Perceive(PerceiveCmd),           // grouped-only: `rdpilot perceive screenshot ...`
    #[command(subcommand)]
    Input(InputCmd),                 // grouped-only: `rdpilot input click ...`
    #[command(subcommand)]
    File(FileCmd),                   // grouped: `rdpilot file put ...`
}
```

### Pattern 2: `--session` as a required field on each session-scoped leaf struct, not a global clap arg

**What:** clap 4's `global = true` args apply to every subcommand INCLUDING ones that legitimately have none (`connect`, `list`) — a `global` arg cannot be conditionally required per-subcommand in clap's model. Declaring `session: String` as a plain `#[arg(long, required = true)]` field on each session-scoped leaf args struct (`ScreenshotArgs`, `ClickArgs`, `PutArgs`, etc.) — but NOT on `ConnectArgs`/no-args `List` — gives clap's own `--help`/parse-error machinery the correct per-command required-ness for free, matching D-29 exactly.
**When to use:** Every CLI-02/CLI-03 verb.
**Example:**
```rust
// Source: standard clap derive pattern — no external doc needed, direct application of struct-field-per-variant
#[derive(clap::Args)]
struct ScreenshotArgs {
    #[arg(long)]
    session: String,
    #[arg(long)]
    output: std::path::PathBuf, // REQUIRED per D-13.1 — no default, binary bytes never hit stdout
}
```

### Pattern 3: absolutize local paths before sending `put`/`get` requests

**What:** the daemon may run with a different current working directory than the shell the user invoked `rdpilot put`/`get` from (it is a long-lived, possibly auto-started-from-elsewhere background process). The CLI must resolve `local_path` to an absolute path (`std::path::absolute` — stabilized in Rust 1.79, or manually join with `std::env::current_dir()` if the workspace's pinned `rust-version = "1.78"` predates it — **verify this at implementation time**, see Pitfall below) before putting it on the wire.
**When to use:** `put`'s `local_path` (source) and `get`'s `local_path` (destination).

### Anti-Patterns to Avoid

- **Sending a raw relative path in `Request::Put`/`Get`:** resolves against the DAEMON's cwd, not the user's shell cwd — a correctness bug, not a security one, but one that will look like intermittent "file not found" flakiness depending on how/where the daemon happened to be started. Absolutize client-side, always.
- **Adding `interprocess` as a new dependency:** D-26 mentions it, but Phase 12 already established the actual pattern (`tokio::net::UnixListener`/`UnixStream` directly, `#[cfg(windows)]` stub deferred to 12-07). The CLI's `connect_or_spawn`-based client code should match whatever the relocated `rdpilot-ipc` transport module ends up using (native `tokio::net`), not introduce a second, inconsistent transport library.
- **Building a NEW `rdpilot-daemon`-independent socket-path resolver in `rdpilot-cli`:** guaranteed drift risk (see Summary #2) — relocate the shared logic, don't fork it.
- **Holding the registry's outer `std::sync::Mutex` guard across an `.await` when calling an operational verb:** deadlocks the single-`LocalSet`-OS-thread daemon the moment two tasks compete for the registry (see "Daemon Dispatch Wiring").

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| CLI argument/subcommand parsing, `--help` generation | A hand-rolled `std::env::args()` matcher | `clap` 4.6.1 derive API | Already D-26-locked; battle-tested `--help`/error-message ergonomics |
| Base64 encode/decode of PNG bytes | A hand-rolled base64 codec | `base64` crate (`general_purpose::STANDARD`) | Trivial but easy to get padding/alphabet wrong; a one-function crate, near-zero footprint cost |
| Length-prefixed JSON socket framing | A second framing implementation in the CLI | Relocated `rdpilot-ipc::transport::{read_frame, write_frame}` (currently daemon-only, must move) | Byte-identical framing is a hard correctness requirement between the two processes — one implementation, shared |
| Socket path / platform runtime-dir resolution | A second `directories::BaseDirs`-based resolver in the CLI | Relocated `rdpilot-ipc::transport::socket_path()` | Same identical-on-both-sides requirement as framing |

**Key insight:** every "don't hand-roll" item in this phase is really the same finding stated four ways — the daemon and the CLI must agree byte-for-byte on transport mechanics, and the only way to guarantee that is one shared, `rdpilot`-independent implementation, not two synchronized-by-convention copies.

## Common Pitfalls

### Pitfall 1: Relative `local_path` sent to the daemon
**What goes wrong:** `put`/`get` silently reads/writes the wrong file (or fails with a confusing "not found") because the daemon resolves the relative path against its own cwd, not the invoking shell's.
**Why it happens:** the daemon is a long-lived, possibly auto-started background process; its cwd is whatever it happened to inherit at spawn time, not the user's terminal.
**How to avoid:** the CLI absolutizes `local_path` before constructing `Request::Put`/`Get` — never send a relative path over the wire.
**Warning signs:** `put`/`get` works when run from one directory and mysteriously fails (or writes to the wrong place) from another.

### Pitfall 2: Moving a `dyn Trait` object out of shared ownership
**What goes wrong:** a naive `Arc<dyn ManagedSession>` + `Arc::into_inner`/`try_unwrap` design for reaching operational methods does not compile — `T: Sized` is required by both APIs, and `dyn ManagedSession` is unsized.
**Why it happens:** it looks like the obvious generalization of the existing `Box<dyn ManagedSession>` pattern, but `Arc`'s ownership-reclaiming APIs have a `Sized` bound that `Box`'s move-by-value semantics never needed.
**How to avoid:** wrap in `Arc<tokio::sync::Mutex<Option<Box<dyn ManagedSession>>>>` — the `Box` (sized) is what gets `.take()`n, never the trait object directly. See "Daemon Dispatch Wiring".
**Warning signs:** a compiler error citing `the trait bound dyn ManagedSession: Sized is not satisfied` on an `Arc::try_unwrap`/`into_inner` call.

### Pitfall 3: Holding the registry's `std::sync::Mutex` guard across an `.await`
**What goes wrong:** the entire daemon (a single `LocalSet` pinned to one OS thread, per `server.rs`'s own doc comment) deadlocks the moment a second task tries to synchronously `.lock()` the same registry mutex while the first task is suspended mid-`.await` still holding the guard.
**Why it happens:** it's tempting to just borrow `&dyn ManagedSession` out of the locked `HashMap` and `.await` a method call on it directly — this compiles (Rust does not prevent holding a `std::sync::MutexGuard` across an `.await` point syntactically) but is a genuine deadlock hazard in this specific single-OS-thread cooperative-scheduling design.
**How to avoid:** always extract/clone what's needed synchronously, drop the outer lock, THEN `.await` (exactly the pattern `Registry::open`/`close` already establish and explicitly comment on — "guard dropped here — never held across the .await below").
**Warning signs:** the daemon hangs under any concurrent load (two CLI invocations targeting different or the same session at once) despite passing single-request tests.

### Pitfall 4: `rdpilot-cli` accidentally depending on `rdpilot` (or `rdpilot-daemon`)
**What goes wrong:** the "thin client, no IronRDP" invariant (D-17, CONTEXT.md's locked crate-dependency rule) is silently violated — `cargo tree -p rdpilot-cli` would show `ironrdp*`/`rustls`/`tokio-full` in the dependency graph, and the binary's size/build-time balloons.
**Why it happens:** reusing `connect_or_spawn`/`socket_path` by adding `rdpilot-daemon` as a path dependency is the path of least resistance — it already compiles, it's already tested — but `rdpilot-daemon` itself depends on `rdpilot`.
**How to avoid:** relocate the transport helpers to `rdpilot-ipc` BEFORE building `rdpilot-cli`, per this research's recommended Wave 1.
**Warning signs:** `cargo tree -p rdpilot-cli | grep ironrdp` returns any output.

### Pitfall 5: `WorldState`'s `SystemTime`/`Duration` fields have no `serde` impl
**What goes wrong:** a naive `#[derive(Serialize)]` mirror of `WorldState` that includes `timestamp: SystemTime`/`capture_span: Duration` fields directly fails to compile (neither type implements `Serialize` in std, and `rdpilot-ipc` has no `serde_with`/`humantime-serde` dependency).
**Why it happens:** `rdpilot::WorldState` itself DOES derive `Serialize` — but only because `rdpilot` (the SDK crate) is not the crate producing the wire DTO; `rdpilot-ipc`'s OWN mirror type must convert these fields to `String`/`u64` explicitly, on the dispatch side, before constructing the wire response.
**How to avoid:** convert `SystemTime` to an ISO-8601 string (reuse `registry.rs`'s existing `iso8601_now`-style conversion for consistency with `SessionStatus.connected_since`/`last_activity`) and `Duration` to `u64` milliseconds in `dispatch.rs`, never attempt to derive `Serialize` on a struct containing these types directly.
**Warning signs:** `the trait bound SystemTime: Serialize is not satisfied` at compile time.

### Pitfall 6: `share_root` not configured on the daemon-side `ConnectionConfig`
**What goes wrong:** `put`/`get` fail with `Error::Config("upload_file requires ConnectionConfig::share_root to be configured")` even though the wire request/dispatch wiring is otherwise correct.
**Why it happens:** `Request::Connect` (Phase 12's dispatch arm) builds a `ConnectionConfig` from the wire fields (`host`/`username`/`password`/`domain`/`port`/`accept_invalid_certs`) — reading `dispatch.rs`'s current `Connect` arm shows it does **not** call `.share_root(...)` on the builder at all. This is a pre-existing Phase 12 gap, not something Phase 13 introduces, but Phase 13's `put`/`get` cannot work until it's fixed.
**How to avoid:** confirm during planning whether Phase 12's `dispatch.rs` `Connect` arm sets `share_root` (it does not, per the source read in this research); if not, Phase 13 must add it — likely resolving a daemon-local share-root directory via `directories`/a fixed cache-dir subpath, since the wire `Request::Connect` has no `share_root` field of its own (nor should it — this is daemon-local operational config, not something a client should dictate per-connect).
**Warning signs:** every `put`/`get` call returns `WireErrorCode::Internal` with a message containing "share_root" regardless of how correct the CLI-side code is.

## Code Examples

### `connect_or_spawn`-based CLI startup (after relocation to `rdpilot-ipc`)

```rust
// Source: pattern already proven in crates/rdpilot-daemon/src/autostart.rs
// (relocate verbatim to rdpilot-ipc, then call unmodified from rdpilot-cli)
let socket_path = rdpilot_ipc::transport::socket_path()?;
let daemon_exe = std::env::current_exe()?.with_file_name("rdpilot-daemon"); // sibling binary
let stream = rdpilot_ipc::transport::connect_or_spawn(&socket_path, &daemon_exe).await
    .map_err(|_| CliError::DaemonUnreachable)?; // synthesizes WireErrorCode::DaemonUnreachable client-side
```

### Config flag layer feeding `rdpilot-config::resolve`

```rust
// Source: crates/rdpilot-config/src/resolve.rs's already-existing apply_overrides contract
let overrides = ResolvedConfig {
    host: cli_args.host,
    port: cli_args.port,
    username: cli_args.username,
    password: cli_args.password,
    domain: cli_args.domain,
    accept_invalid_certs: cli_args.accept_invalid_certs, // bool: false never clobbers a lower layer (per apply_overrides's documented semantics)
};
let resolved = rdpilot_config::resolve(overrides)?;
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|---------------|--------|
| D-26's suggested `interprocess` crate for IPC transport | Native `tokio::net::UnixListener`/`UnixStream` (Unix) + `tokio::net::windows::named_pipe` (Windows, stubbed) | Phase 12 (12-04, already shipped) | The CLI should follow the ALREADY-SHIPPED pattern, not the original D-26 suggestion — D-26 predates Phase 12's actual implementation choice |

**Deprecated/outdated:** none specific to this phase's domain beyond the above — `clap` 4.x derive API, `tokio` 1.x, and the wire-protocol conventions established in Phases 11/12 are all current and stable as of this research.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `base64` crate's exact current version (`0.22.x`) | Standard Stack | Low — `base64` is an extremely stable, widely-used crate; worst case the planner pins a slightly stale version that `cargo update` corrects. Not run through `slopcheck` this session (no `cargo` on this research host after the `clap`/`directories` scan). |
| A2 | `std::path::absolute` is available under this workspace's pinned `rust-version = "1.78"` (Pitfall/Pattern 3) | Architecture Patterns / Pitfalls | Low-medium — `std::path::absolute` stabilized in Rust 1.79, one minor version above this workspace's pin. If unavailable, the fallback (`std::env::current_dir().join(local_path)` when `local_path` is relative) is trivial and well-understood, but the planner must verify the actual toolchain version (`rust-toolchain.toml`, not yet read this session) before assuming the stdlib function is usable. |
| A3 | Phase 12's `dispatch.rs` `Request::Connect` arm does NOT currently set `ConnectionConfig::share_root` (Pitfall 6) | Common Pitfalls | Medium — this was read directly from `dispatch.rs`'s source (not assumed), so confidence is actually HIGH that the code as it stands today omits it; tagged as an assumption only in the sense that the *fix* (where the daemon should source its share-root path from) was not specified by any existing decision and is proposed here, not verified against a locked design. |
| A4 | Recommending hand-rolled table rendering over `comfy-table`/`tabled` | Standard Stack (Alternatives) | Low — this is within CONTEXT.md's explicit "Claude's Discretion" grant; if the planner disagrees, swapping in a table crate is a contained, low-risk change with no wire-protocol implications. |

## Open Questions

1. **Does the planner want `put`'s remote no-clobber to be a real (sensor-enforced) check, or a documented known-gap?**
   - What we know: `get`'s no-clobber is trivially enforceable client/dispatch-side (local filesystem). `put`'s destination is remote; the existing sensor `FileTransfer` `Upload` handler (Phase 10) has no overwrite-check semantics today, and extending it is arguably outside "the sensor is Phase 10's concern."
   - What's unclear: whether CLI-03's "no-clobber by default" success criterion is meant to cover both directions equally, or whether a documented asymmetry (get: enforced, put: --force is a no-op since nothing prevents overwrite either way) is acceptable for v1.1.
   - Recommendation: raise this explicitly in planning/discuss-phase rather than silently under- or over-building; the cheapest correct-by-construction fix (extend the C# sensor's Upload handler with a `no_clobber: bool` field + `File.Exists` check) is a small, contained change if the planner wants full symmetry.

2. **Where should the daemon source its `share_root` path from now that `Request::Connect` has no such field?**
   - What we know: `ConnectionConfig::share_root` must be set for `upload_file`/`download_file` to work at all (Pitfall 6); the wire `Request::Connect` deliberately has no `share_root` field (it would be daemon-local operational config, not something a client dictates).
   - What's unclear: whether this should be a fixed, hardcoded subdirectory under a `directories`-resolved cache/data dir (e.g. `<cache_dir>/rdpilot/transfer-staging/<session-id>/`), or a daemon-startup-time config knob (env var, following the `RDPILOT_DAEMON_*` convention `RunConfig::from_env` already establishes for `sink_path_override`/lifecycle timings).
   - Recommendation: a fixed, `directories`-resolved per-daemon-process staging root (not per-session — `Session::unique_share_name()` already guarantees collision-free staging filenames within one root) is the simplest correct default; expose an override env var only if the planner judges it's needed for the Phase 15 live gate's test isolation (mirroring `RDPILOT_DAEMON_SINK_PATH`'s existing precedent).

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `cargo`/`rustc` toolchain | Building/testing this phase's crates | ✗ (not on `PATH` in this research sandbox) | — | The execution phase runs on the project's normal dev environment where `cargo`/`rustc` ARE available (confirmed present at `~/.cargo/bin`, just not on this research session's `PATH`) — not a real blocker, just this session's tool-visibility quirk |
| crates.io registry access | Version verification | ✓ | — | — |
| `slopcheck` | Package Legitimacy Audit | ✓ (`/home/marc/.local/bin/slopcheck`) | — | — |
| A remote Windows RDP target (Azure VM) | Any LIVE end-to-end CLI proof (screenshot/click/uia/file-transfer actually round-tripping against a real machine) | ✗ this session | — | Not needed for Phase 13 execution — CONTEXT.md's "batch all live gates near Phase 15" instruction means this phase's own success criteria are provable OFFLINE (see next section); the live proof is explicitly deferred |

**Missing dependencies with no fallback:** none — every offline verification path for this phase's own success criteria is achievable with `cargo test`/fake connectors/fake daemons, following the exact pattern Phase 12 already established and proved.

**Missing dependencies with fallback:** `cargo`/`rustc` not on this research session's `PATH` — not a real execution blocker (confirmed installed via `rustup`, just not exported to this sandboxed research shell).

## Offline vs. Batched-Live Testability (per success criterion)

| Success Criterion | Offline-provable this phase? | How | Batched-live component (defer to Phase 15) |
|---|---|---|---|
| SC#1 (CLI-01): `connect`/`list`/`disconnect` manage lifecycle over the daemon, auto-starting it | **Yes, fully.** | `tests/autostart_lifecycle.rs`'s existing pattern (spawn the REAL compiled `rdpilot-daemon` binary with `RDPILOT_DAEMON_TEST_CONNECTOR` set, so it uses `FakeTestConnector` — no real RDP target — and drive it via the CLI binary instead of the raw `connect_or_spawn` helper) proves this end-to-end offline | None strictly required — a real Windows target changes nothing about lifecycle mechanics; optionally re-run the same test against a live target at the Phase 15 gate for extra confidence, not because it's structurally necessary |
| SC#2 (CLI-02): full perception/input/launch verb set runs against `--session` | **Mostly — dispatch/wire round-trip yes; REAL sensor semantics no.** | Wire encode/decode + dispatch routing + CLI arg parsing + table/`--json` rendering are all provable with `FakeTestConnector`/a `FakeSession` extended with the new operational methods returning canned values (mirrors `FakeSession`'s existing `close`/`describe` stub pattern) | Whether `screenshot`/`click`/`uia`/`launch`/`foreground` actually work against a REAL Windows desktop (coordinate correctness, sensor round-trip timing, UIA tree shape) is exactly what Phase 15's batched live gate is for — flag every such assertion as `#[ignore]`-gated live-only, following Phase 10's `RDPILOT_LIVE`-gating precedent |
| SC#3 (CLI-03): `put`/`get` no-clobber + distinct error legibility | **Yes, fully, for the error-taxonomy/exit-code half; no-clobber logic yes; actual bytes-transferred correctness no.** | Exit-code mapping, `WireErrorCode`→CLI-error rendering, and the no-clobber existence-check are all pure/offline-testable logic; `SessionNotFound`/`DaemonUnreachable` are directly reproducible offline (query a made-up session id; point the CLI at a socket path with no listener and a nonexistent daemon binary) | Whether a REAL multi-MB file genuinely round-trips correctly through a live daemon+sensor+RDPDR chain is Phase 10's already-completed FILE-01/02/04 concern, re-exercised (not re-proven) through the CLI surface at the Phase 15 live gate |

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Cargo's built-in test harness (`#[test]`/`#[tokio::test]`), no external test framework — matches every other crate in this workspace |
| Config file | none — see Wave 0 gaps below |
| Quick run command | `cargo test -p rdpilot-ipc -p rdpilot-cli` (once `rdpilot-cli` exists) |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CLI-01 | connect/list/disconnect + auto-start | integration (real daemon binary, fake connector) | `cargo test -p rdpilot-cli --test cli_lifecycle -- --include-ignored` (mirrors `rdpilot-daemon`'s `autostart_lifecycle.rs` pattern) | ❌ Wave 0/3 — new file |
| CLI-02 | wire encode/decode + dispatch routing for every new verb | unit (rdpilot-ipc) + unit (rdpilot-daemon dispatch, fake session) | `cargo test -p rdpilot-ipc` / `cargo test -p rdpilot-daemon --lib dispatch` | ❌ Wave 0/2 — extends existing `request.rs`/`dispatch.rs` inline test modules |
| CLI-02 | CLI arg parsing + table/`--json` rendering | unit (rdpilot-cli, clap's own parse-from-args testing pattern) | `cargo test -p rdpilot-cli --lib` | ❌ Wave 0/3 — new file |
| CLI-03 | exit-code mapping, no-clobber refusal | unit (rdpilot-cli) | `cargo test -p rdpilot-cli --lib exit_codes` | ❌ Wave 0/3 — new file |
| CLI-03 | session-not-found / daemon-unreachable reproduced end-to-end | integration (real daemon binary or a deliberately-absent one) | `cargo test -p rdpilot-cli --test cli_errors -- --include-ignored` | ❌ Wave 0/3 — new file |

### Sampling Rate

- **Per task commit:** `cargo test -p <crate-being-touched>`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full workspace suite green before `/gsd-verify-work`; any `#[ignore]`-gated live-only assertions explicitly deferred to Phase 15, never silently skipped without a comment explaining why (matching Phase 10's `RDPILOT_LIVE` precedent).

### Wave 0 Gaps

- [ ] `crates/rdpilot-ipc/src/perception.rs` + `input.rs` (new modules) — the wire mirror types, covers CLI-02's wire-shape half
- [ ] `crates/rdpilot-daemon/src/seams.rs` — extended `ManagedSession` trait + updated test-fixture fakes (registry.rs/dispatch.rs/server.rs's inline `#[cfg(test)]` modules all need matching stub methods added)
- [ ] `crates/rdpilot-daemon/src/registry.rs` — `Arc<TokioMutex<Option<Box<dyn ManagedSession>>>>` storage change + new `Registry::call` method
- [ ] `crates/rdpilot-ipc/src/transport.rs` (new module, relocated from `rdpilot-daemon`) — `socket_path`/`bind`-adjacent path resolution, `read_frame`/`write_frame`, `connect_or_spawn`
- [ ] `crates/rdpilot-cli/` (entire new crate) — no existing scaffold
- [ ] Framework install: none — `cargo`'s built-in harness already covers everything; no new test framework dependency needed

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | Indirect | Credentials flow through `rdpilot-config`'s already-audited layered resolution (Phase 11, CONFIG-01/03) — Phase 13 only adds the flag layer, must not log/echo `--password` |
| V3 Session Management | Yes | D-29's required `--session` on every command IS the access-control primitive here — no implicit default target, already locked and structurally enforced at the `rdpilot-ipc` type level (`SessionScoped`'s exhaustive match, non-`Option` `session` field) |
| V4 Access Control | Yes (inherited from Phase 12) | Local-user-only IPC (Unix `0700` dir + peer-uid check, DAEMON-02) — Phase 13 does not touch this, but the CLI must not, e.g., print the socket path or connection details in a way that weakens it |
| V5 Input Validation | Yes | Path-traversal validation for `put`/`get` remote paths is entirely SDK/sensor-side (Phase 10, already live-verified) — Phase 13 must not introduce a SECOND, weaker validation path (e.g. don't try to "helpfully" pre-validate remote paths CLI-side with a naive check that could diverge from the canonicalization-based sensor validator) |
| V6 Cryptography | No new surface | Checksum verification (SHA-256) is entirely SDK-side (Phase 10, D-10.5) — Phase 13 only renders the already-verified `TransferOutcome.checksum` |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Credential leakage via `--password` on the command line (visible in `ps`/shell history) | Information Disclosure | Document `RDPILOT_PASSWORD` env var / config-file password as the recommended path in `--help` text; do not remove `--password` entirely (D-27 requires flag-layer parity with file/env keys) but flag it as the least-safe option in CLI docs — this is a documentation/UX mitigation, not a code-level block, since D-27 already locked flag-layer parity |
| Relative-path confusion between CLI and daemon processes (Pitfall 1) | Tampering (writes/reads land somewhere unintended) | Absolutize `local_path` client-side before sending, per this research's Pattern 3 |
| A malicious/compromised local process racing to connect to the daemon socket before the intended CLI does | Spoofing | Already fully mitigated by Phase 12's DAEMON-02 peer-uid check — Phase 13 introduces no new attack surface here since the CLI is just another local-uid-matching client |
| CLI printing the full `WireError.message` verbatim in `--json`/table output, potentially including internal detail an operator shouldn't see | Information Disclosure (low severity — local-only tool) | Low risk given DAEMON-02's local-user-only scoping already limits the audience to the authenticated local user; no additional redaction needed beyond what `WireError`/`DaemonError`'s existing D-31 credential-free-message guarantee already provides |

## Sources

### Primary (HIGH confidence — direct source read, this repository)
- `crates/rdpilot/src/session.rs` — every `Session` method signature cited in "Session Method Contract"
- `crates/rdpilot/src/perception.rs`, `input.rs`, `worldstate.rs`, `screenshot.rs`, `error.rs`, `config.rs`, `keepalive.rs`, `lib.rs` — owned SDK public type shapes
- `crates/rdpilot-ipc/src/request.rs`, `response.rs`, `error.rs`, `session_id.rs`, `transfer.rs`, `lib.rs` — existing wire DTO shapes and the `SessionScoped` forcing-function pattern
- `crates/rdpilot-daemon/src/dispatch.rs`, `seams.rs`, `registry.rs`, `error_map.rs`, `server.rs`, `autostart.rs`, `lib.rs`, `ipc/mod.rs`, `ipc/unix.rs`, `ipc/framing.rs`, `main.rs` — the exact "not implemented" gap, `ManagedSession`'s current shape, the non-Send/`LocalSet` execution model, and the existing `connect_or_spawn`/framing/socket-path code that needs relocating
- `crates/rdpilot-config/src/paths.rs`, `resolved.rs`, `resolve.rs`, `lib.rs` — layered config resolution the CLI's flag layer must match
- `.planning/phases/13-cli-surface/13-CONTEXT.md`, `.planning/ROADMAP.md`, `.planning/DECISIONS-INDEX.md`, `.planning/REQUIREMENTS.md` — locked decisions, requirement text, cross-cutting decisions D-16 through D-30
- `Cargo.toml` (workspace root + all four existing crates) — exact current dependency versions in use

### Secondary (MEDIUM confidence)
- crates.io registry API (`curl -H "User-Agent: ..." https://crates.io/api/v1/crates/<name>`) — confirmed `clap` 4.6.1, `directories` 6.0.0, `interprocess` 2.4.2, `tokio` 1.52.3 current as of 2026-07-11

### Tertiary (LOW confidence)
- `base64` crate's exact current version — not verified against the registry this session (see Assumptions Log A1)
- `std::path::absolute` availability under `rust-version = "1.78"` — not verified against `rust-toolchain.toml` this session (see Assumptions Log A2)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — clap/directories/tokio versions directly confirmed against the crates.io registry; `rdpilot-ipc`/`rdpilot-config` are already-shipped workspace crates
- Architecture (dispatch wiring, Arc<Mutex<Option<Box<...>>>> pattern): MEDIUM — reasoned from direct source read and confirmed Rust language constraints (unsized-move restriction), but not yet compiled/tested — this is a NEW design, not an already-proven one
- Pitfalls: HIGH for the relative-path and unsized-move issues (derived directly from source + language semantics); MEDIUM for the `share_root`-not-configured gap (confirmed absent from current `dispatch.rs`, but the fix's exact shape is proposed, not locked)

**Research date:** 2026-07-11
**Valid until:** 30 days (stable Rust ecosystem; the two architecture findings — transport relocation and `ManagedSession` extension — should be revalidated against the actual codebase state if Phase 12's remaining 12-07 live-gate plan lands materially different code than what this research read)
