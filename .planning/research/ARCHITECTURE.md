# Architecture Patterns

**Domain:** AI-driven computer use over RDP — local SDK controlling a remote Windows desktop
**Researched:** 2026-06-04
**Confidence:** HIGH (protocol specs verified; library APIs verified via docs.rs and official samples)

---

## Recommended Architecture

Three distinct runtime components communicating over two transports.

```
┌─────────────────────────────────────────────────────────────────┐
│  LOCAL WORKSTATION                                              │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  rdpilot SDK (Rust / IronRDP-based)                      │   │
│  │                                                          │   │
│  │  ┌─────────────────┐   ┌───────────────────────────┐    │   │
│  │  │  RDP Session    │   │  Sensor Client            │    │   │
│  │  │  Manager        │   │                           │    │   │
│  │  │  - connect/auth │   │  - DVC channel handler    │    │   │
│  │  │  - keepalive    │   │  - serialises/deserialises│    │   │
│  │  │  - teardown     │   │    sensor payloads        │    │   │
│  │  └────────┬────────┘   └───────────┬───────────────┘    │   │
│  │           │                        │                     │   │
│  │  ┌────────▼────────────────────────▼───────────────┐    │   │
│  │  │  IronRDP core (session, connector, dvc)          │    │   │
│  │  │  + ironrdp-graphics (DecodedImage framebuffer)   │    │   │
│  │  │  + ironrdp-input  (keyboard / mouse PDUs)        │    │   │
│  │  └────────────────────────┬────────────────────────┘    │   │
│  │                           │  RDP over TCP/TLS (3389)     │   │
│  │  ┌────────────────────────▼────────────────────────┐    │   │
│  │  │  Public SDK API                                  │    │   │
│  │  │  connect() / screenshot() / send_input()         │    │   │
│  │  │  get_windows() / get_uia_tree() / launch_proc()  │    │   │
│  │  └──────────────────────────────────────────────────┘    │   │
│  └──────────────────────────────────────────────────────────┘   │
│                           │                                     │
│       Scripted harness / AI agent consumer                      │
└───────────────────────────┼─────────────────────────────────────┘
                            │  RDP over TCP/TLS
                            │  (DVC multiplexed on same connection)
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│  REMOTE WINDOWS TARGET                                          │
│                                                                 │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  rdpilot-sensor helper (thin, dumb)                      │   │
│  │  - C# or Rust .exe running in the RDP user session       │   │
│  │                                                          │   │
│  │  ┌──────────────────┐  ┌────────────────────────────┐   │   │
│  │  │  Sensor Modules  │  │  DVC Server Endpoint        │   │   │
│  │  │                  │  │  (WTSVirtualChannelOpen)    │   │   │
│  │  │  - process tree  │  │  - receives cmd payloads   │   │   │
│  │  │    (WMI/WTS)     │  │  - sends response payloads │   │   │
│  │  │  - window list   │  └────────────────────────────┘   │   │
│  │  │    (EnumWindows  │                                    │   │
│  │  │     GetWindowRect│                                    │   │
│  │  │     GetWindowText│                                    │   │
│  │  │  - UIA tree dump │                                    │   │
│  │  │    (IUIAutomation│                                    │   │
│  │  │     TreeWalker)  │                                    │   │
│  │  │  - launch proc   │                                    │   │
│  │  └──────────────────┘                                    │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Component Boundaries

### Component 1: RDP Session Manager (local)

**Responsibility:** Own the lifecycle of the RDP TCP/TLS connection. Authenticate, negotiate capabilities, keep the session alive, and expose a raw PDU send/receive interface to the layers above it.

**What it talks to:**
- IronRDP `ironrdp-connector` (connection state machine) and `ironrdp-session` (session loop)
- Sensor Client (passes the DVC channel instance after session is established)
- Public SDK API (receives connect/disconnect commands, emits session-state events)

**What it does NOT do:** Decode pixels, dispatch inputs, or know anything about UIA.

**Library constraint:** IronRDP's `ironrdp-blocking` or `ironrdp-async` crate handles the I/O abstraction. The session manager instantiates `DrdynvcClient` (from `ironrdp-dvc`) and registers the sensor DVC handler at connection time so it is negotiated during the RDP capability exchange. There is no way to add a DVC channel after the session is established — registration must happen before `connector.connect()` completes. This is a hard IronRDP constraint (HIGH confidence).

---

### Component 2: Framebuffer / Pixel Layer (local)

**Responsibility:** Maintain a `DecodedImage` framebuffer that reflects the current remote desktop state. Accept graphics update PDUs from the session loop and apply them. Expose `screenshot()` and `screenshot_window(hwnd, rect)` calls.

**What it talks to:**
- IronRDP `ironrdp-graphics` (`DecodedImage`, codec decoders)
- IronRDP `ActiveStage` which emits `GraphicsUpdate` events on each decoded frame
- Public SDK API (returns `&[u8]` RGBA pixel buffers or encoded PNG/BMP)

**Key implementation note:** `ActiveStage` in IronRDP automatically calls codec decoders (uncompressed bitmap, RLE, RDP 6.0, RemoteFX, ZGFX) and applies the result to `DecodedImage`. Callers subscribe to `ActiveStageOutput::GraphicsUpdate` and read `image.data()`. No manual codec selection needed for the common path (HIGH confidence, from official IronRDP docs).

**Window screenshot:** The pixel layer alone cannot clip to a single window — it holds the full virtual desktop framebuffer. The window geometry (rect) must come from the Sensor Client. The pixel layer takes that rect and crops the framebuffer. Both must be queried in the same "snapshot" to avoid tearing artefacts (see Perception Pipeline section).

---

### Component 3: Input Injector (local)

**Responsibility:** Accept abstract input commands (mouse move/click/scroll, key press/release) and translate them to RDP input PDUs sent via the session loop.

**What it talks to:**
- IronRDP `ironrdp-input` (builds keyboard/mouse input PDUs)
- RDP Session Manager (sends the PDU on the wire)
- Public SDK API

**Coordinate system:** RDP mouse input is absolute in virtual desktop space (screen coordinates, top-left origin). The input injector must accept coordinates in the same space. If callers work in window-relative coordinates, they are responsible for adding the window origin (from the window list) before passing to the injector. Making this transform explicit in the API prevents accidental off-by-one errors at scale (MEDIUM confidence, based on MS-RDPBCGR spec note that RDP only supports absolute mouse mode, not relative).

---

### Component 4: Sensor Client (local)

**Responsibility:** Own the DVC channel object and implement a request/response protocol over it. Expose typed async calls: `get_process_tree()`, `get_window_list()`, `get_uia_tree(hwnd?)`, `launch_process(cmd)`. Serialize requests and deserialize responses.

**What it talks to:**
- IronRDP `ironrdp-dvc` (`DvcProcessor` / `DvcClientProcessor` trait implementations)
- RDP Session Manager (DVC data flows inside the RDP session multiplexed with screen data)
- Public SDK API

**Key constraint:** Because the DVC channel is multiplexed on the same RDP TCP connection, there is no additional network port required. The sensor channel travels alongside screen data through whatever NAT/firewall already permits port 3389.

---

### Component 5: Public SDK API (local)

**Responsibility:** The library's external surface. Exposes a clean, typed, async-friendly API hiding all IronRDP internals and the sensor protocol. This is what a scripted harness or an AI agent calls.

```
SessionHandle::connect(target, creds, options) -> Result<Session>
Session::screenshot() -> Result<RgbaImage>
Session::screenshot_window(hwnd) -> Result<RgbaImage>
Session::send_mouse(action: MouseAction) -> Result<()>
Session::send_key(action: KeyAction) -> Result<()>
Session::windows() -> Result<Vec<WindowInfo>>       // via sensor
Session::process_tree() -> Result<Vec<ProcessInfo>> // via sensor
Session::uia_tree(hwnd: Option<Hwnd>) -> Result<UiaNode> // via sensor
Session::launch(cmd: &str) -> Result<ProcessHandle>  // via sensor
Session::disconnect() -> Result<()>
```

---

### Component 6: rdpilot-sensor (remote, thin helper)

**Responsibility:** Run inside the RDP user session on the Windows target. Open the named DVC endpoint, wait for requests, execute local Windows API calls, serialise results, send response. That is all — no agent logic, no reasoning, no state beyond the open channel.

**What it talks to:** Nothing except the DVC channel and local Win32/UIA/WMI APIs. It does NOT make outbound network connections.

**Implementation language:** C# (.NET 8 NativeAOT or trimmed self-contained) is the pragmatic choice. Reasons:
- UIA is a native .NET/COM API; the managed wrapper (`UIAutomationClient.dll`) is well-tested and documented.
- EnumWindows, GetWindowText, GetWindowRect, WTSEnumerateProcesses are all trivially accessible via P/Invoke or `System.Diagnostics`.
- Produces a single self-contained `.exe` that needs no runtime installed if published as NativeAOT or with `--self-contained`.
- Microsoft's official DVC server sample has a C# (.NET) variant (HIGH confidence — 33.6% of the `rdp-dvc-plugin-samples` repo is C#).

Alternative (Rust): possible and type-safe, but the windows-rs crate's UIA bindings are lower-level and less ergonomic for tree traversal than .NET's managed UIA. Adds complexity for uncertain benefit in a dumb sensor.

---

## Transport: Recommended — RDP Dynamic Virtual Channel (DVC)

### How DVC Works

A DVC is a named channel negotiated during the RDP capability exchange. The client registers a plugin (in IronRDP: implement `DvcProcessor` / `DvcClientProcessor`); the server-side application opens the same channel by name with `WTSVirtualChannelOpenEx(sessionId, "RDPILOT_SENSOR", WTS_CHANNEL_OPTION_DYNAMIC)`. Once both ends are open, bidirectional message exchange works. The RDP connection itself multiplexes the channel data alongside graphics and input (HIGH confidence — MS-RDPEDYC spec + IronRDP docs.rs).

### Trade-off comparison

| Criterion | RDP DVC (recommended) | Separate TCP/named-pipe | WinRM |
|-----------|----------------------|------------------------|-------|
| Extra ports required | None — multiplexed on 3389 | Yes — second port/pipe address needed | Yes — 5985/5986 |
| NAT/firewall | No additional rules | Requires open port or loopback trick | Requires open port |
| Setup complexity | Medium (DVC plugin reg on client; sensor opens channel on server) | Low (plain TCP) but network-harder | High (Enable-PSRemoting, auth config) |
| Coupling to RDP session | Tightly coupled — channel dies when session dies (GOOD — natural cleanup) | Decoupled — can outlive session (BAD for v1) | Decoupled |
| Latency for small messages | Low (same socket) | Low (local connection) | High (SOAP/HTTP overhead) |
| Security | TLS-encrypted in the RDP session (inherited from RDP auth) | Must add its own auth/TLS | HTTPS optional; HTTP encrypted at message layer |
| Library support | `ironrdp-dvc` (Rust client); WTS API (server C#) | Standard sockets | pywinrm / winrm PowerShell module |
| Binary framing | Must implement (length-prefixed protobuf or msgpack recommended) | Must implement | Provided (SOAP envelope) |

**Recommendation: DVC.** The multiplexed-on-3389 property is decisive. Most RDP targets are behind firewalls and NAT that permit exactly port 3389 and nothing else. DVC works there by default. The natural lifecycle coupling is a feature — if the RDP session drops, the sensor channel drops, preventing a zombie helper process accepting rogue connections. WinRM adds a heavyweight prerequisite (`Enable-PSRemoting`) that the user may not be able to run on a managed machine; it is appropriate as a bootstrap mechanism (see below) but not as the primary structured-data transport.

### DVC constraint on client library choice

IronRDP exposes `DvcProcessor` / `DvcClientProcessor` traits and `DrdynvcClient` for registering custom channels. This is a first-class supported use case (crates.io `ironrdp-dvc`, `ironrdp-dvc-pipe-proxy`). FreeRDP also implements DRDYNVC but its C API is harder to wrap for safe Rust. **The decision to use IronRDP (deferred to STACK.md) enables the DVC-first architecture.** If the stack decision lands on FreeRDP-C instead, the DVC client path becomes significantly more complex.

---

## Remote Helper Bootstrap

### Options evaluated

**Option A — Drive/clipboard redirection + launch (v1 recommended)**

1. During session establishment, enable drive redirection so the client's local filesystem appears as a network drive inside the RDP session (e.g. `\\tsclient\C\`).
2. Copy `rdpilot-sensor.exe` from the local machine to the remote temp folder via the redirected drive (standard file-copy).
3. Send a keyboard shortcut + typing sequence (Win+R → `cmd.exe /c "\\tsclient\...\rdpilot-sensor.exe"`) via RDP input injection, or use the RAIL `RemoteAppExec` command if RAIL is enabled.

Strengths: works over a pure RDP connection with no out-of-band access; no pre-configuration on the target.
Weaknesses: drive redirection must be enabled on the target (it is the default on Windows Server RDS, and on Windows 10/11 Pro with default settings); relies on Win+R working as expected (fragile); the launch sequence is pixel-level brittle.

**Option B — WinRM bootstrap (v1 acceptable where available)**

If the target has WinRM enabled (default on Windows Server, requires `Enable-PSRemoting` on desktop Windows):
1. `Invoke-Command -ComputerName <target> -ScriptBlock { ... }` copies and launches the sensor via PowerShell remoting.

Strengths: clean, scriptable, not brittle pixel-level input.
Weaknesses: WinRM disabled by default on Windows 10/11 desktop editions; requires network access on port 5985/5986 which may not be available if only 3389 is open.

**Option C — Pre-installed (long-term / managed environments)**

Deploy `rdpilot-sensor` as a Windows service or startup item via GPO/Intune. The sensor starts automatically when a user session begins and opens the DVC endpoint without any bootstrap step.

Strengths: cleanest runtime; no bootstrap complexity; correct for managed fleets.
Weaknesses: requires deployment authority on the target machine; not viable for ad-hoc targets.

**Option D — RAIL / RemoteApp (not recommended for bootstrap)**

RAIL enables launching a specific remote application as a pseudo-local window. It could launch the sensor, but RAIL requires server-side configuration (`RemoteApp` publishing) that is harder to automate than drive-redirection bootstrap. The complexity is not justified for a dumb sensor launcher.

### Recommendation

**v1: Option A (drive redirection) as primary, Option B (WinRM) as fallback.**

Implement a `Bootstrapper` component in the SDK that tries:
1. WinRM if the target responds on 5985 (fast check, no RDP input needed).
2. Drive-redirection copy + keyboard-macro launch otherwise.

Expose the bootstrap as a separate step so callers can substitute their own mechanism (pre-installed targets skip it entirely).

**Long-term: Option C.** Publish a `rdpilot-sensor` installer/package. When the target is a managed machine (the primary production use case), pre-install and let the SDK just open the DVC channel without any bootstrap dance.

---

## Perception Pipeline: World State Model

### The coherence problem

A "world state" snapshot consumed by an agent needs: (a) a screenshot, (b) a window list with positions, (c) optionally a UIA tree for one or all windows. These come from three different sources on potentially different clocks:

- Screenshot: framebuffer decoded from the most recently received graphics PDU.
- Window list: a response from the sensor over the DVC channel.
- UIA tree: another response from the sensor (expensive — can take 100-500 ms for a complex tree).

If the agent gets a screenshot from frame T, a window list from a request at T+200 ms (after a window moved), and a UIA tree from T+800 ms (after content changed), the coordinates will be inconsistent.

### Recommended approach: snapshot-on-demand with timestamps

```
WorldState {
    framebuffer_ts:   Instant,       // when the last graphics update was applied
    screenshot:       RgbaImage,     // copy of the framebuffer at framebuffer_ts
    window_list:      Vec<WindowInfo>, // bounding rects in virtual desktop space
    window_list_ts:   Instant,
    uia_tree:         Option<UiaNode>, // may be None if not requested
    uia_tree_ts:      Option<Instant>,
}
```

The sensor is instructed to do a single atomic "snapshot" request that returns window list + (optionally) UIA tree in one message. The SDK sends that request, then immediately captures the current framebuffer, and packages both into a `WorldState`. The timestamp spread should be < 50 ms for window list (fast Win32 calls) and < 500 ms for UIA (depends on complexity).

For the v1 scripted harness this is sufficient. An AI agent loop can tolerate 100-500 ms staleness per step — it re-queries before each action.

### Coordinate spaces

There is exactly one coordinate space: **virtual desktop space**, pixels from (0,0) top-left of the remote desktop. All elements use the same space:

- Screenshot pixels are in virtual desktop space (IronRDP `DecodedImage` is the full desktop).
- Window `GetWindowRect` returns screen coordinates = virtual desktop space.
- UIA element `BoundingRectangle` is in screen coordinates = virtual desktop space.
- RDP mouse input is absolute in virtual desktop space.

No translation is needed between these sources as long as the sensor runs in the same session as the target application. The only exception is DPI scaling: if the remote session uses non-100% DPI and the application is DPI-unaware, its reported `GetWindowRect` may be in logical (scaled) coordinates while the framebuffer is in physical pixels. The sensor should be DPI-aware (call `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)` at startup) and always return physical pixel coordinates. The SDK should negotiate the remote desktop resolution at a known fixed size (e.g. 1920x1080, no local DPI scaling) to eliminate ambiguity.

### Per-window vs full-desktop screenshot

Default: capture the full virtual desktop framebuffer (always available, zero extra cost).
Per-window screenshot: crop the framebuffer using the window rect from the window list. This requires a recent window list from the sensor — add a `get_window_screenshot(hwnd)` convenience that fetches the window rect if not cached, then crops the framebuffer. Do not use a separate RDP mechanism for this.

### UIA tree depth strategy

Fetching the entire desktop UIA tree is slow and produces huge payloads. The sensor should default to a focused-window query or an explicit hwnd parameter. For v1, always require the caller to specify a target hwnd. A full-desktop dump is an opt-in.

---

## Session Model

### RDP session types

| Type | Description | UI Automation access | Notes |
|------|-------------|---------------------|-------|
| Console session | Session 0-equivalent physical console (or `/console` RDP connect) | Full — same desktop | Only one active at a time; connecting via RDP to console disconnects any local user |
| RDP virtual session | Normal RDP connection creates a new session (Session 1, 2, ...) | Full — each session has its own desktop | Default for all standard RDP connections |
| Session 0 | Service host session | None — no interactive UI | Not relevant for rdpilot |

**v1 target: RDP virtual session.** This is the default and the common case. The sensor runs inside that session and has full UIA access to windows in that session. There is no cross-session UIA access issue because the sensor is in the same session as the target application.

### Session isolation

Windows Vista+ Session 0 isolation means services have no UI. rdpilot does not deal with Session 0. The sensor is a user-mode process in the interactive RDP session — it has full Win32/UIA access to windows in that session (HIGH confidence from Microsoft session isolation docs).

### Headless / unattended considerations

When an RDP session is disconnected (not logged off — disconnected), the session persists but the display updates stop. If the SDK reconnects to the same session, the sensor helper is already running (if it was started during the previous connection). The DVC channel re-establishes on reconnect because IronRDP re-negotiates capabilities. For v1 this is out of scope — assume a connected session. Document that the sensor must be re-launched on fresh connections unless Option C (pre-installed service) is used.

For unattended headless targets (no physical monitor), the remote desktop session still renders to a virtual framebuffer — no HDMI dummy plug is required for RDP sessions (unlike VNC/HDMI-dependent setups).

### Single connection v1

v1 is a single `Session` object per process. No connection pooling, no concurrent sessions. This is explicit — concurrency and multi-session orchestration are out of scope.

---

## Suggested Build Order (Dependency-Ordered)

This order minimises the risk of building on an unstable foundation and maps cleanly to phases.

```
Phase 1 — RDP Session + Pixel Core
  ┌─ IronRDP wiring: connect, authenticate, session loop
  ├─ Framebuffer (DecodedImage) maintained by ActiveStage
  ├─ screenshot() returning full-desktop RGBA/BMP
  └─ PROVES: RDP connection works, graphics decode works

Phase 2 — Input Injection
  ┌─ Mouse: absolute move, left/right/middle click, scroll
  ├─ Keyboard: key down/up, character typing
  └─ PROVES: round-trip perception+action; enables the "navigate" part of v1 finish line
  DEPENDS ON: Phase 1 (need a live session to inject input)

Phase 3 — Sensor Transport (DVC channel)
  ┌─ DVC client plugin (DvcProcessor impl) registered with DrdynvcClient
  ├─ Sensor server endpoint in sensor helper (WTSVirtualChannelOpenEx)
  ├─ Message framing: length-prefixed protobuf (use prost on Rust side, Google.Protobuf on C# side)
  ├─ Ping/heartbeat to confirm channel is live
  └─ PROVES: bidirectional structured data flows over the RDP session
  DEPENDS ON: Phase 1 (DVC channel is part of the RDP session)

Phase 4 — Sensor Bootstrap + Deployment
  ┌─ rdpilot-sensor.exe build (C# NativeAOT or trimmed self-contained)
  ├─ Bootstrap via drive redirection (Option A)
  ├─ Bootstrap fallback via WinRM (Option B)
  └─ PROVES: end-to-end helper deployment on a real target
  DEPENDS ON: Phase 3 (must have a working channel to verify bootstrap succeeded)

Phase 5 — Sensor Perception Modules (window list + process tree)
  ┌─ EnumWindows / GetWindowText / GetWindowRect → WindowInfo list
  ├─ WTSEnumerateProcesses / CreateToolhelp32Snapshot → ProcessInfo tree
  ├─ Sensor request/response protocol for these two queries
  └─ PROVES: structured perception for window geometry
  DEPENDS ON: Phase 4 (sensor is running and channel is established)

Phase 6 — UIA Tree Module
  ┌─ IUIAutomation / TreeWalker traversal in sensor helper
  ├─ Serialize UiaNode tree to protobuf/JSON
  ├─ Sensor request/response: get_uia_tree(hwnd?)
  └─ PROVES: deep accessibility tree is retrievable
  DEPENDS ON: Phase 5 (confirms sensor infrastructure is stable before adding the expensive UIA module)
  NOTE: UIA is the most complex module — separate phase reduces debugging surface

Phase 7 — World State + Public SDK API
  ┌─ WorldState snapshot: combine framebuffer + window list (+ optional UIA)
  ├─ Typed public API surface (Session struct, all methods)
  ├─ Coordinate-space contract enforced in API types
  └─ PROVES: the SDK is coherent and usable as a library
  DEPENDS ON: Phases 1-6

Phase 8 — Scripted Proof Harness
  ┌─ connect → screenshot → get_windows → get_uia_tree → send_input → screenshot again
  ├─ Target: a real remote-only program (not a toy)
  ├─ Harness asserts expected UIA properties are found
  └─ PROVES: v1 finish line; SDK works end-to-end against a real target
  DEPENDS ON: Phase 7
```

---

## Anti-Patterns to Avoid

### Anti-Pattern 1: Intelligence Leakage into the Sensor

**What:** Adding any decision-making, retry logic, or "smart" behaviour into `rdpilot-sensor`.
**Why bad:** The sensor is dumb by design. Any logic there executes on the remote machine and is harder to debug, update, and reason about. It also violates the core architectural constraint.
**Instead:** All logic, retries, and reasoning stay in the local SDK.

### Anti-Pattern 2: Per-Window DVC Channels

**What:** Opening a separate DVC channel per target window or per query type.
**Why bad:** DVC channels are negotiated at connection time and must be pre-registered. Multiple channels multiply the surface and complicate teardown.
**Instead:** One `RDPILOT_SENSOR` channel with a request/response multiplexing protocol (tag each request with a request ID).

### Anti-Pattern 3: Using RAIL/RemoteApp for Perception

**What:** Relying on RAIL window metadata (which the RDP server sends during RemoteApp sessions) instead of querying EnumWindows from the sensor.
**Why bad:** RAIL requires server-side RemoteApp publishing configuration; not all targets support it; it only exposes windows of published applications, not all windows.
**Instead:** Sensor-side EnumWindows gives complete window list for all processes in the session.

### Anti-Pattern 4: Full-Desktop UIA Dump by Default

**What:** Crawling the entire UIA tree for the desktop on every world-state snapshot.
**Why bad:** UIA tree traversal is inherently slow (100-500+ ms for complex applications). Doing it on every snapshot will dominate the perception loop latency.
**Instead:** Default to focused-window UIA query; full-desktop dump is opt-in with explicit timeout.

### Anti-Pattern 5: Bypassing DVC with a Separate TCP Connection

**What:** Opening a second TCP connection from the sensor to the local machine for structured data.
**Why bad:** Requires an open inbound port on the local machine OR an open outbound port on the remote target beyond 3389. Breaks in NAT/firewall environments. Creates a second auth surface.
**Instead:** DVC on the existing RDP connection.

---

## Scalability Considerations

| Concern | At v1 (1 session) | At multi-session (future) |
|---------|------------------|--------------------------|
| Session management | Single Session object | Session pool with per-session IDs |
| UIA tree latency | Acceptable, ~100-500 ms | Consider caching with invalidation |
| Bootstrap | Per-connection script | Pre-installed service (Option C) |
| Sensor restarts | Manual re-bootstrap | Health-check + auto-relaunch via DVC heartbeat |
| Framebuffer memory | One DecodedImage per session | Memory-bounded ring buffer or reference-counted |

---

## Sources

- IronRDP ARCHITECTURE.md: https://github.com/Devolutions/IronRDP/blob/master/ARCHITECTURE.md (HIGH confidence)
- IronRDP DVC crate docs: https://docs.rs/ironrdp-dvc/latest/ironrdp_dvc/ (HIGH confidence)
- IronRDP Graphics Rendering guide: https://mintlify.wiki/Devolutions/IronRDP/guides/graphics-rendering (HIGH confidence)
- MS-RDPEDYC (DVC protocol spec): https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpedyc/1edc9fd6-c7f9-4de9-82d6-5d13ee41d03a (HIGH confidence)
- MS-RDPBCGR keyboard/mouse input: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/f0ea088b-1398-43f0-af42-f4e027ab2009 (HIGH confidence)
- MS-RDPERP (RAIL protocol): https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdperp/485e6f6d-2401-4a9c-9330-46454f0c5aba (HIGH confidence)
- Microsoft DVC plugin samples (7 languages): https://github.com/microsoft/rdp-dvc-plugin-samples (HIGH confidence)
- UI Automation Overview (Win32): https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-uiautomationoverview (HIGH confidence)
- Windows Session 0 Isolation: https://techcommunity.microsoft.com/blog/askperf/application-compatibility---session-0-isolation/372361 (HIGH confidence)
- WinRM installation and configuration: https://learn.microsoft.com/en-us/windows/win32/winrm/installation-and-configuration-for-windows-remote-management (HIGH confidence)
- Remote and console terminal differences: https://learn.microsoft.com/en-us/windows/win32/termserv/consoles-vs-terminals (HIGH confidence)
- EnumWindows (Win32): https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-enumwindows (HIGH confidence)
