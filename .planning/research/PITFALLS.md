# Pitfalls Research

**Domain:** Session daemon + CLI + MCP server over a stateful Rust RDP-perception SDK (rdpilot v1.1: Consumer Surfaces & File Transfer)
**Researched:** 2026-07-10
**Confidence:** MEDIUM-HIGH (grounded in this codebase's own v1.0 live-gate findings + verified external sources for MCP/CLIPRDR/RDPDR/Unix-socket security; a few items flagged LOW pending live validation)

This research assumes the v1.0 SDK is DONE (IronRDP session core, C# NativeAOT sensor over DVC, RDPDR drive backend with Create/Close/Read/QueryDirectory implemented, WorldState API). Pitfalls below are specific to the NEW v1.1 surfaces: a persistent daemon holding live `Session`s, a stateless CLI client, a dual-mode MCP server, and bidirectional file transfer — not generic SDK advice. (v1.0's RDP-perception-domain pitfalls, e.g. minimized-window framebuffer loss, remain valid and are not repeated here; see git history for the prior PITFALLS.md if needed.)

## Critical Pitfalls

### Pitfall 1: Session-per-OS-thread leak in the daemon

**What goes wrong:**
Each `Session` already runs its session loop on a dedicated OS thread with a current-thread Tokio runtime (a Phase 2 architectural decision, forced by an `!Send` borrow across `.await` in the reactivation path — see PROJECT.md Key Decisions / STATE.md Phase 2 Plan 02). A daemon that holds N concurrent sessions is therefore managing N raw OS threads outside any async task pool. If `disconnect` (explicit or reaped) doesn't positively signal the thread to exit its `select!` loop and `join()` it, the thread — and the `Session`'s framebuffer/input-database memory it owns — leaks silently. Under long daemon uptime with many connect/disconnect cycles this is a slow, hard-to-see resource leak, not a crash.

**Why it happens:**
The OS-thread-per-session design was chosen for a single always-connected session in v1.0's proof harness, where the process exits and the OS reclaims everything. A long-lived daemon is the first place this design has to actually tear sessions down cleanly and repeatedly, and that teardown path did not exist before v1.1.

**How to avoid:**
Give every `Session` an explicit shutdown channel (e.g. a `oneshot`/watch signal into the thread's `select!`) and make the daemon's disconnect path `join()` the thread with a bounded timeout before removing the registry entry. Add a debug-only "session count vs. live-thread count" assertion or metric so a leak surfaces as a divergence, not just growing memory.

**Warning signs:**
Thread count reported by `/proc` (or equivalent) growing across repeated connect/disconnect cycles in a soak test; daemon RSS climbing with no matching increase in the session registry's reported entries.

**Phase to address:**
Session Daemon phase (the phase that introduces the registry and connect/disconnect lifecycle) — must include a soak test (connect/disconnect N times, assert thread/memory return to baseline) as an explicit success criterion, not just "disconnect works once."

---

### Pitfall 2: Implicit default session creeps back in for CLI/MCP convenience

**What goes wrong:**
The requirement is explicit: "no implicit default target — every command explicitly names its session." The realistic way this gets violated is not a deliberate decision but convenience creep during CLI ergonomics work — e.g. falling back to "the only session if there's exactly one," or "the most recently connected session," when the name argument is omitted. This is the single most damaging mistake for the MCP surface specifically: an AI agent operating multiple remote sessions (e.g. one "prod-like" target and one throwaway scratch VM) that omits or fuzzes a session identifier will silently get routed to *some* session and act on the wrong desktop — clicking, typing, or launching processes on a target the agent never intended to touch.

**Why it happens:**
"Just default to the only one" feels harmless when there's exactly one session during development and testing, so it slips in as a quality-of-life shortcut and is never revisited once a second session exists.

**How to avoid:**
Make the session identifier a required, non-optional field in the wire protocol (IPC schema) and MCP tool schema — not a convention enforced only by CLI argument parsing. A missing session id must be a hard schema-validation error at the IPC boundary, before any handler code runs, so no code path can accidentally supply a default. Write a test that calls every verb with the session field omitted and asserts a rejection, not a fallback.

**Warning signs:**
Any handler code with `.unwrap_or_else(|| registry.only_session())` or `.unwrap_or(registry.most_recent())`; MCP tool JSON schemas that mark `session_id` as optional "for convenience."

**Phase to address:**
Session Identity phase — the wire protocol / MCP tool schema design step, verified before CLI or MCP surfaces are built on top of it (a schema-level fix here is far cheaper than retrofitting after both consumers exist).

---

### Pitfall 3: Registry check-then-insert race on session names

**What goes wrong:**
Two concurrent `connect --name foo` calls (plausible from an AI agent that fires parallel tool calls, or a human CLI script) both check "does `foo` exist?", both see "no," and both proceed to open a real RDP connection and insert into the registry. One insert wins and the other's live `Session` (and its OS thread, per Pitfall 1) becomes orphaned — leaked and unreachable by name, silently consuming a connection slot on the remote target with nothing ever able to address or reap it.

**Why it happens:**
A registry implemented as `Mutex<HashMap<String, Session>>` with separate "check" and "insert" operations under two different lock acquisitions (rather than one atomic `entry()`-style operation) is the natural first implementation and looks correct in every single-request test.

**How to avoid:**
Route all registry mutation through a single-writer actor (one task/thread owning the `HashMap`, all callers send requests over a channel) or use `HashMap::entry(name).or_insert_with(...)` inside one lock acquisition so check-and-insert is atomic. Add a concurrency test that fires N simultaneous `connect --name foo` calls and asserts exactly one live session results and N-1 clean rejections (not N-1 silent no-ops).

**Warning signs:**
Registry entry count and live-RDP-connection count diverging under concurrent load; a name that "exists" per `list` but whose commands intermittently fail as if talking to a different underlying connection.

**Phase to address:**
Session Daemon phase, registry design — the same phase as Pitfall 1's shutdown-signal work, since both are the registry's core correctness properties.

---

### Pitfall 4: Debug redaction does not protect the wire protocol

**What goes wrong:**
v1.0 already established a real discipline here: `ConnectionConfig`'s `Debug` impl redacts the password (D-14), and this was verified sufficient for library-internal logging. The daemon's IPC wire protocol, however, needs `Serialize`/`Deserialize` (or an equivalent JSON DTO) to move connection configs and session metadata between CLI/MCP and the daemon — and `derive(Serialize)` does **not** inherit a hand-written `Debug` redaction. A "list sessions" or "status" verb that serializes a `ConnectionConfig`-shaped struct straight to JSON for the IPC response will put the plaintext password on the wire and potentially into any client-side logging of that response (CLI `--verbose`, MCP tool result content shown to the model/user), even though `{:?}` on the same struct still looks perfectly redacted everywhere else in the codebase.

**Why it happens:**
The existing redaction pattern is `Debug`-specific and was correct for v1.0's scope (library-internal error/debug output only). It is easy to assume "we already solved credential redaction" and reuse the same struct for the new IPC/MCP boundary without re-auditing which trait actually governs serialization there.

**How to avoid:**
Define a distinct, deliberately-thin DTO for anything that crosses the IPC/MCP boundary (session id, host, connected-since, status — never the secret), so there is no `Serialize` impl on any type that also holds a credential. Add a boundary test that serializes every response type used by the daemon protocol and greps the output for known test-secret sentinel values.

**Warning signs:**
Any `#[derive(Serialize)]` on (or near) `ConnectionConfig` or a struct containing it; an IPC response schema that includes a `config` or `connection` field typed as the full internal config rather than a status-only view.

**Phase to address:**
Layered Connection Config phase (or wherever the IPC wire protocol/DTOs are first defined) — should be caught at schema-design time, verified with the grep-for-secret test before the MCP/CLI phases consume the protocol.

---

### Pitfall 5: Unauthenticated local IPC socket/pipe — another local user drives the session

**What goes wrong:**
A Unix domain socket created with default umask, or a Windows named pipe created with the default security descriptor, is frequently readable/writable by every local user on the machine, not just the user who started the daemon. Since the daemon holds live, authenticated RDP sessions (potentially to a real corporate or otherwise sensitive target), any other local account on a shared workstation could connect to the socket/pipe and issue mouse/keyboard/file-transfer commands against that session — a full session hijack with no RDP credentials of their own, just local access to the machine running rdpilot.

**Why it happens:**
Socket/pipe creation APIs default to permissive access unless the developer explicitly restricts them, and this is invisible in single-user development/testing (where "another local user" never exists to reveal the gap).

**How to avoid:**
On Unix: create the socket inside a directory created with `0700` permissions (not just `chmod` the socket file itself, which is subject to a TOCTOU window between `bind()` and `chmod()`), owned by the current user; verify with `SO_PEERCRED`/`LOCAL_PEERCRED` on every accepted connection that the calling uid matches the daemon's uid, not just at listen time. On Windows: construct the named pipe with an explicit DACL restricting access to the creating user's SID (do not rely on `CreateNamedPipe`'s default security descriptor). Add a defense-in-depth per-daemon-instance random token (written to a `0600` file alongside the socket/pipe) required on every request, so a filesystem-permission mistake alone is not sufficient for takeover.

**Warning signs:**
`ls -la` on the socket path showing group/other bits set; a fresh daemon instance accepting connections from a test process running under a different uid in CI.

**Phase to address:**
Session Daemon phase (IPC transport implementation) — this is a launch-blocking security property, not a later hardening pass; verify with an explicit "different-uid client is rejected" test before any CLI/MCP surface is built on the same transport.

---

### Pitfall 6: MCP computer-use screenshot resolution/coordinate mismatch

**What goes wrong:**
Anthropic's computer-use tool convention expects screenshots downscaled to XGA (1024x768) or WXGA (1280x800) — sending higher resolutions "relying on the API's own resizing" measurably degrades click accuracy and latency (Anthropic's own guidance). rdpilot's native screenshots are full remote-desktop resolution (e.g. 1920x1080) under an already-enforced 96-DPI physical-pixel coordinate contract. If the MCP server passes the native screenshot straight through to the computer-use tool without a deliberate downscale-and-remap step, the model reasons about click coordinates in the *native* pixel space it was shown, and if the server does not consistently rescale those coordinates back before calling `Session::send_mouse`, clicks land offset from what the model intended — a "looks fine" bug that only shows up as slightly-wrong clicks, which is exactly the kind of failure that erodes trust in a live-LLM demo without an obvious root cause.

**Why it happens:**
The SDK's own 96-DPI physical-pixel contract (already correct and load-bearing for the native tools) creates a false sense that "coordinates are already solved" — but the computer-use tool surface introduces a *second*, independent coordinate space (whatever resolution the server advertises to the model) that must be explicitly bridged, not assumed to be the same space.

**How to avoid:**
Pick one fixed advertised resolution (e.g. 1280x800) for the computer-use tool surface, resize every screenshot to it before sending, and implement a single, tested `scale_to_native(x, y)` function used on every click/move/type-target coordinate before it reaches `Session::send_mouse` — with a round-trip test (native rect corners -> advertised space -> back to native) asserting sub-pixel-class accuracy. Keep this scaling entirely separate from the native-tool surface, which should continue to expose true native coordinates unscaled.

**Warning signs:**
Clicks in the live-LLM demo consistently landing near but not on the intended UI element, worse at the edges/corners of the screen than the center (a classic linear-scaling-error signature).

**Phase to address:**
MCP Server Surface phase — needs its own explicit success criterion ("click a specific button reliably via the computer-use tool schema, verified live"), not folded silently into a generic "screenshot tool works" check.

---

### Pitfall 7: MCP tool call blocks the transport event loop on a slow RDP round trip

**What goes wrong:**
v1.0's measured RDP/sensor round trips are all comfortably sub-second (sensor ping ~165ms, UIA subtree walk ~130ms, WorldState capture 23-73ms) — but v1.1 introduces genuinely long operations: file transfer of an arbitrarily large file, `launch_process` waiting for a slow application to actually appear, or a daemon call against a session whose underlying connection has silently died and is retrying. If the MCP server implementation handles each tool call synchronously on the same task/thread that services the transport (stdio or socket), one slow or hung call blocks the server from responding to *any* other tool call, including unrelated fast ones (e.g. a `list_sessions` call queued behind a stuck file transfer), and can make the whole MCP server appear hung to the host application.

**Why it happens:**
The naive, obviously-correct-looking implementation is "await the daemon IPC call inline in the tool-call handler" — this works perfectly in every manual test where operations are fast, and only breaks under either a genuinely slow operation or a degraded connection, neither of which shows up in a quick smoke test.

**How to avoid:**
Run each tool-call handler on its own task, independent of the transport's read/dispatch loop, with a bounded, explicit timeout per call (distinct from and larger than the SDK's internal per-primitive timeouts — e.g. tens of seconds for a UI action, an explicit longer bound with progress reporting for file transfer) that surfaces as a normal MCP tool error rather than a hang. Use MCP progress notifications for file transfer so the host application (and a human watching) can distinguish "still working" from "stuck."

**Warning signs:**
A live-LLM demo where one slow tool call (e.g. a large file upload) makes an unrelated subsequent tool call in the same conversation appear to hang rather than fail or queue visibly.

**Phase to address:**
MCP Server Surface phase for the isolation/timeout architecture; Bidirectional File Transfer phase for the specific progress-reporting need once transfer exists.

---

### Pitfall 8: Extending the RDPDR drive backend to writes reopens a known path-traversal CVE class

**What goes wrong:**
The existing `RdpilotDriveBackend` (Phase 5) implements 4 of 11 `ServerDriveIoRequest` IRP variants (Create/Close/Read/QueryDirectory); v1.1's upload path requires adding `Write` (and likely richer `Create` semantics for new-file creation). This is precisely the code shape where real, disclosed vulnerabilities have occurred in other RDP drive-redirection implementations: FreeRDP's `contains_dotdot()` path-traversal filter had an off-by-one that missed a trailing `..` with no separator, allowing a malicious peer to read/write one directory above the shared folder (FreeRDP GHSA-3xpj-m4hx-8vmx); a related class of bug (CVE-2025-48817) hit Windows' own RDP client file-transfer path validation. Since rdpilot's own drive backend is the code that will decide what paths are legal on the remote share, an incomplete or off-by-one path-canonicalization check on the new `Write`/`Create` handling is an arbitrary-file-write primitive on the remote target, not just a file-transfer bug.

**Why it happens:**
Path-traversal filtering that only checks for the substring `../` or `..\` mid-path (rather than canonicalizing the full resulting path and verifying it stays within the shared root) is the natural first implementation, and is exactly the shape of bug that shipped in FreeRDP for years before being found.

**How to avoid:**
Canonicalize the full resolved path (join + normalize, resolving `.`/`..` structurally rather than substring-matching) and verify it is still a descendant of the configured share root before honoring any `Create`/`Write` IRP; reject on any ambiguity rather than trying to be permissive. Write unit tests that specifically mirror the disclosed bug class: a path ending in `..` with no trailing separator, a path with mixed `/`/`\` separators, and an absolute path masquerading as relative.

**Warning signs:**
A path-traversal test suite that only tests `../../etc/passwd`-shaped inputs and passes, without testing the trailing-no-separator or mixed-separator edge cases that caused the real disclosed bugs.

**Phase to address:**
Bidirectional File Transfer phase — must include an explicit path-traversal test suite as a success criterion before the upload verb is considered done, referencing the specific disclosed bug shapes above rather than generic "sanitize the path" language.

---

### Pitfall 9: Daemon death does not clean up the corresponding Windows-side RDP session

**What goes wrong:**
If the daemon process dies (crash, OOM-kill, host reboot) while holding live sessions, the local TCP connection to each RDP target dies with it — but per this codebase's own Phase 6 live-gate finding, Windows treats an RDP connection drop as a *disconnect*, not a *logoff*: the interactive session on the target persists in a disconnected state, and Windows reconnects to that same existing session (rather than creating a fresh one) the next time the same user authenticates. The daemon's in-memory registry, however, is gone. On restart, the daemon has zero knowledge that a durable Windows-side session (and possibly a still-running sensor process on it) already exists, and nothing informs the user that reconnecting under the same name/credentials will resume old state rather than start clean — or, worse, that repeated daemon crashes under *different* target credentials could accumulate multiple orphaned disconnected Windows sessions with no client-side way to see or reap them.

**Why it happens:**
The registry-in-memory design is the natural default for a daemon and works perfectly across clean `disconnect` calls; only an unclean daemon death exposes the client/server session-state divergence, which is easy to never test because it requires deliberately killing the daemon mid-session.

**How to avoid:**
Persist minimal registry state (session name, target host, timestamp — never the credential) to disk on every registry mutation, and on daemon startup, reconcile: report any persisted session as "possibly still live on the target, reconnect to confirm" rather than silently discarding it. Explicitly test the crash-and-restart path (kill -9 the daemon with a live session, restart, attempt to address the same session name) as a first-class scenario, not an afterthought — this is a codebase-specific risk given the same reconnect-to-disconnected-session behavior already surprised the Phase 6 sensor deployment logic once (stale sensor process required an explicit taskkill-before-relaunch fix).

**Warning signs:**
A restarted daemon with an empty registry that, on a fresh `connect --name foo`, silently lands on an old disconnected Windows session with stale sensor/process state rather than a clean one — indistinguishable from a true fresh connect until something behaves unexpectedly (a "ghost" running process from the prior daemon lifetime).

**Phase to address:**
Session Daemon phase for the persistence/reconciliation mechanism; should be explicitly retested whenever the MCP/CLI phases add their own crash-recovery expectations.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|--------------------|-----------------|------------------|
| Reuse `Session::ping()`'s sensor-health check as the daemon's connection-liveness signal | No new keepalive code | Conflates "sensor process alive" with "RDP transport alive" — a hung/crashed sensor looks identical to a dead connection, so the daemon can't tell whether to retry the sensor deploy or fully reconnect | Never for the daemon's core health model; fine as one diagnostic signal among several |
| Session registry as a plain in-memory `Mutex<HashMap>` with no persistence | Fast to build, correct for the common clean-shutdown path | Total state loss (and orphaned remote sessions, Pitfall 9) on any unclean daemon death | Acceptable only for a first internal spike; must gain at least minimal disk persistence before this is used for real personal-tooling sessions left running unattended |
| CLI passes `--password` as a plain argument | Simplest to implement and demo | Leaks via `ps`/shell history on any shared machine | Never by default; acceptable only behind an explicit `--i-know-this-is-insecure` style flag for one-off scripting, with env var / prompt as the documented default path |
| MCP server awaits the daemon IPC call inline in the tool-call handler (no per-call task isolation) | Simplest possible MCP server loop | One slow/hung call blocks the whole server (Pitfall 7) | Acceptable only until the first tool with unbounded duration (file transfer, `launch_process` wait) is added — must be fixed before Bidirectional File Transfer phase ships |
| Single-shot (non-resumable) file transfer implementation | Much simpler than chunked resume logic | Any interruption on a large transfer means starting over from zero, with no partial-progress recovery | Acceptable for v1.1 personal-tooling scope, provided partial writes are staged-and-renamed (Pitfall 8/file-transfer gotchas) so failure is at least clean, not silently corrupt |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|------------------|-------------------|
| Anthropic computer-use tool schema | Sending full native-resolution screenshots and trusting the API's own image resizing | Downscale to a fixed advertised resolution (XGA/WXGA) server-side and maintain an explicit, tested coordinate-scaling function (Pitfall 6) |
| MCP transport (stdio/socket) | Defaulting to or accidentally exposing a network-bindable transport instead of local-only stdio/socket/pipe | Default strictly to local transport; require an explicit, documented opt-in (with its own auth story) for anything network-reachable |
| RDPDR drive redirection (extending the existing backend) | Treating the existing Create/Close/Read/QueryDirectory implementation as "the hard part is done" and bolting Write on without re-auditing path validation | Re-audit path canonicalization specifically for the new write path against the disclosed FreeRDP/Windows CVE shapes (Pitfall 8) |
| Windows session reconnect semantics | Assuming a fresh RDP connect always yields a fresh Windows-side session | Explicitly test and document the reconnect-to-disconnected-session behavior (already empirically observed in Phase 6) as it applies to daemon restarts and file-transfer resume |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|-----------------|
| Reading an entire file into memory before starting an RDPDR write loop | Works fine in small-file tests, OOMs or stalls on anything large | Stream in bounded chunks matching MS-RDPEFS's per-IRP size limits, tracking offset explicitly | Any file beyond a few MB, or several concurrent transfers on a memory-constrained workstation |
| One OS thread per `Session` with no upper bound | Fine with 1-2 sessions in testing | Cap concurrent sessions (config limit) and monitor thread count; document the limit rather than silently degrading | Beyond roughly a handful of concurrent sessions per daemon instance, thread scheduling and memory overhead become noticeable — explicitly out of scope per PROJECT.md ("multi-session orchestration at scale" not exercised), but the daemon should still fail loudly rather than silently degrade past some threshold |
| Full-resolution PNG screenshot on every MCP computer-use tool call | Slow round trips, high token cost, sluggish live-LLM demo | Downscale per Pitfall 6, and consider JPEG for computer-use frames where lossy compression is acceptable (native/UIA-perception paths should stay lossless) | Any conversation with more than a handful of screenshot tool calls — costs compound quickly |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| World-readable/writable IPC socket or named pipe | Any local user on a shared machine can drive a live RDP session with someone else's remote credentials, with zero RDP auth of their own | Restrict via directory/DACL permissions plus a peer-uid check on every accepted connection (Pitfall 5), never rely on the socket path being "secret" |
| `Serialize`-derived DTOs reusing the internal `ConnectionConfig` shape | Password leaks over the IPC wire / into MCP tool results even though `Debug` output is correctly redacted | Distinct, credential-free status DTOs for every IPC/MCP response type (Pitfall 4) |
| Trusting substring-based path traversal filters (`contains("..")`) on the RDPDR write path | Arbitrary file read/write on the remote target one level outside the intended share root — the exact disclosed FreeRDP/Windows CVE shape | Canonicalize and verify ancestry, not substring match (Pitfall 8) |
| Logging full daemon requests (including credentials) for debugging | Credentials land in a daemon log file that may have looser permissions than the socket/config file itself | Redact at the logging boundary using the same discipline as the existing `ConnectionConfig` Debug redaction, and audit every new log call site added for the daemon/MCP/CLI surfaces |
| Treating the MCP server's "it's just talking to Claude" framing as inherently safe | An MCP client is still an untrusted-input boundary for anything the model decides to type/click/upload/download on the daemon's behalf — a prompt-injected or misled model can drive destructive file operations exactly as a human CLI user could | Apply the same session-identity-required and path-traversal protections to the MCP surface as the CLI surface; do not special-case "trust" for the AI-driven path |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-------------------|
| Auto-generated session ids that are long opaque UUIDs with no human-legible name | Painful to reference in CLI usage and in MCP tool arguments the model has to carry around verbatim | Short, memorable auto-ids (e.g. adjective-noun or short hash) when the user doesn't supply a name, while still guaranteeing uniqueness (ties into Pitfall 3's atomic-insert requirement) |
| Silent truncate-or-overwrite behavior on file-transfer destination conflicts | A download/upload appears to succeed but clobbers or silently coexists with existing data in a way the user didn't intend | Fail closed by default on destination-exists conflicts, with an explicit `--force`/overwrite flag |
| No visibility into session age/idle time in `list` output | User (or agent) forgets a session is still open, consuming a remote login slot / cloud cost indefinitely | `list` should show connected-since / last-activity so cleanup is a deliberate, informed decision — matches this project's registry-not-mystery philosophy |
| MCP tool error messages that just say "internal error" for every daemon-side failure | Agent can't tell "session name doesn't exist" from "connection lost, needs reconnect" from "click was out of bounds," so it can't decide how to recover | Map distinct daemon error variants to distinct, legible MCP tool-error text (see Pitfall 7's error-surfacing note) |

## "Looks Done But Isn't" Checklist

- [ ] **Session daemon disconnect:** Looks done when a single connect/disconnect cycle works — verify with a soak test of repeated cycles that thread count and memory return to baseline (Pitfall 1).
- [ ] **No-implicit-default session targeting:** Looks done when every documented CLI example includes a session name — verify by testing every verb with the session argument *omitted* and confirming a hard rejection, not a fallback (Pitfall 2).
- [ ] **IPC/MCP credential redaction:** Looks done when `println!("{:?}", config)` is redacted — verify by serializing every daemon response type to JSON and grepping for a planted test secret (Pitfall 4).
- [ ] **IPC transport permissions:** Looks done when it works for the developer's own user — verify with a second local test user/process actually attempting to connect and confirming rejection (Pitfall 5).
- [ ] **Computer-use tool coordinate mapping:** Looks done when a screenshot displays and a click "roughly" lands nearby — verify with a precision test clicking small UI elements near screen edges/corners, not just the center (Pitfall 6).
- [ ] **File-transfer path safety:** Looks done when normal relative paths transfer correctly — verify against the specific disclosed traversal shapes (trailing `..` with no separator, mixed separators, absolute-path-as-relative) (Pitfall 8).
- [ ] **File-transfer chunking:** Looks done when a small test file transfers correctly — verify with a file larger than one MS-RDPEFS IRP's max chunk size to confirm the read/write loop actually loops.
- [ ] **Daemon crash recovery:** Looks done when clean shutdown/restart works — verify by `kill -9`-ing the daemon mid-session and confirming the restart behaves sanely (reports the possibly-still-live remote session rather than silently forgetting it) (Pitfall 9).

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|----------------|------------------|
| Session-per-thread leak (Pitfall 1) | MEDIUM | Add the missing shutdown-signal/join path; restart the daemon to clear existing leaked threads; add the soak test going forward |
| Registry race / orphaned session (Pitfall 3) | MEDIUM | Convert registry mutation to a single-writer actor or atomic `entry()` pattern; any already-orphaned live connections must be found via a "sessions the registry doesn't know about" audit and manually torn down |
| IPC credential leak via Serialize (Pitfall 4) | LOW-MEDIUM | Introduce the credential-free DTO, audit and scrub any already-written logs/history that may have captured the leaked value, rotate any credential that was exposed |
| Unauthenticated local socket/pipe (Pitfall 5) | LOW | Tighten permissions/DACL and add the peer-check; if this shipped to any real multi-user machine, treat any credential used during that window as potentially exposed and rotate it |
| Path-traversal on RDPDR write (Pitfall 8) | HIGH | This is a real remote-file-integrity risk if shipped — requires canonicalization fix, a full audit of what paths were actually reachable during the vulnerable window, and verification the remote target wasn't already touched outside the intended share root |
| Daemon-crash orphaned Windows session (Pitfall 9) | LOW | Add persistence/reconciliation; in the meantime, manually inspect the target for stray disconnected sessions after any daemon crash and log off manually if found |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|--------------------|----------------|
| 1. Session-per-thread leak | Session Daemon phase | Soak test: N connect/disconnect cycles, thread/memory count returns to baseline |
| 2. Implicit default session creep | Session Identity phase (wire protocol / schema design) | Every verb tested with session field omitted → hard rejection |
| 3. Registry check-then-insert race | Session Daemon phase (registry design) | Concurrency test: N simultaneous same-name connects → exactly 1 success |
| 4. Debug redaction ≠ Serialize redaction | Layered Connection Config / IPC protocol design phase | Serialize every response DTO, grep for planted test secret |
| 5. Unauthenticated local IPC socket/pipe | Session Daemon phase (IPC transport) | Different-uid client connection attempt is rejected |
| 6. Computer-use coordinate mismatch | MCP Server Surface phase | Precision click test near screen edges/corners via computer-use tool schema |
| 7. MCP tool call blocks event loop | MCP Server Surface phase (+ File Transfer phase for progress reporting) | Slow tool call in flight does not block a concurrent unrelated fast tool call |
| 8. RDPDR write path traversal | Bidirectional File Transfer phase | Test suite covering disclosed CVE-shaped inputs (trailing `..`, mixed separators, absolute-as-relative) |
| 9. Daemon death orphans Windows session | Session Daemon phase | `kill -9` mid-session, restart, confirm reconciliation behavior (not silent amnesia) |

## Sources

- Anthropic computer-use best practices and XGA/WXGA screenshot-resolution guidance — [claude.com/blog/best-practices-for-computer-and-browser-use-with-claude](https://claude.com/blog/best-practices-for-computer-and-browser-use-with-claude), [github.com/anthropics/claude-quickstarts computer-use-demo README](https://github.com/anthropics/claude-quickstarts/blob/main/computer-use-demo/README.md) — MEDIUM-HIGH confidence, cross-referenced across official Anthropic sources
- MCP long-running tool call / event-loop blocking patterns — [ClickHouse mcp-clickhouse issue #128](https://github.com/ClickHouse/mcp-clickhouse/issues/128), [rapidevelopers.com MCP timeout guide](https://www.rapidevelopers.com/mcp-tutorial/how-to-fix-mcp-server-timeout-errors), [dev.to async handleId pattern](https://dev.to/aws/fix-mcp-timeouts-async-handleid-pattern-8ek) — MEDIUM confidence (community sources, consistent with MCP protocol's documented progress-notification support)
- Unix domain socket / local daemon IPC security (permissions, peer-credential checks) — [Shenanigans Labs: LXD LPE via hijacked Unix socket credentials](https://shenaniganslabs.io/2019/05/21/LXD-LPE.html), [Broadcom: restricting local IPC over Unix domain sockets](https://techdocs.broadcom.com/us/en/symantec-security-software/identity-security/privileged-access-manager/4-2/pam-server-control/Administrate-PAM-SC/endpoint-administration-for-unix/restricting-local-interprocess-communication-over-unix-local-named-domain-sockets.html) — MEDIUM-HIGH confidence
- RDPDR/CLIPRDR path-traversal and file-redirection vulnerabilities — [FreeRDP GHSA-3xpj-m4hx-8vmx (contains_dotdot off-by-one)](https://github.com/FreeRDP/FreeRDP/security/advisories/GHSA-3xpj-m4hx-8vmx), [ZeroPath: CVE-2025-48817 Windows RDP client path traversal](https://zeropath.com/blog/cve-2025-48817-windows-rdp-path-traversal), [Check Point: Reverse RDP Attack](https://research.checkpoint.com/2019/reverse-rdp-attack-code-execution-on-rdp-clients/) — HIGH confidence, disclosed CVEs/advisories from primary sources
- rdpilot v1.0 own accumulated context (Session OS-thread-per-connection architecture, `Session::ping()` transient-error propagation bug fixed at Phase 4, `SetForegroundWindow`/`AttachThreadInput` finding and stale-process-on-reconnect finding at Phase 6, `ConnectionConfig` Debug redaction D-14, RDPDR backend's 4-of-11 implemented IRP variants at Phase 5) — HIGH confidence, drawn directly from `.planning/STATE.md` and `.planning/PROJECT.md`

---
*Pitfalls research for: rdpilot v1.1 (session daemon + CLI + MCP server + bidirectional file transfer over a persistent RDP-perception SDK)*
*Researched: 2026-07-10*
