# Technology Stack

**Project:** rdpilot — AI-driven computer use over RDP
**Researched:** 2026-06-04
**Overall confidence:** HIGH (RDP library choice, perception APIs); MEDIUM (language binding strategy)

---

## 1. Prior Art Discovery

**Search performed across:** GitHub, crates.io, npm, PyPI, HackerNews, security blogs, vendor docs.
**Verdict:** No general-purpose "AI computer use SDK over RDP" existed before mid-2025. One purpose-built tool now exists that is the closest known prior art.

### agent-rdp (thisnick/agent-rdp) — CLOSEST PRIOR ART

| Attribute | Value |
|-----------|-------|
| URL | https://github.com/thisnick/agent-rdp |
| Language | Rust 72%, PowerShell 13%, TypeScript 11% |
| License | MIT / Apache-2.0 |
| RDP library | IronRDP |
| Latest release | v0.6.5 (2026-02-26), 55 total releases |
| Stars | 13 |

**What it does:** A CLI tool for AI agents to control Windows Remote Desktop sessions. Capabilities: screenshot capture (PNG/JPEG/base64), mouse clicks/drag/scroll, keyboard input, clipboard sync, drive mapping, OCR-based fallback text location, Windows UI Automation over a Dynamic Virtual Channel (DVC). PowerShell is injected into the remote session; it captures the UIA accessibility tree and performs actions. Communication uses a DVC for bidirectional IPC. The TypeScript component wraps the Rust CLI for agent consumption.

**How close it gets:**
- Screenshots: yes
- Mouse/keyboard input: yes
- Process tree: NOT documented
- Window list/geometry: NOT documented
- UIA tree over DVC: yes (PowerShell agent, DVC transport)
- Clean SDK API surface: no — it is a CLI tool, not a library

**Gaps relative to rdpilot:**
1. CLI tool, not an embeddable library with a programmatic API surface
2. No documented process tree enumeration
3. No documented window-list/geometry data
4. Remote helper is PowerShell (interpreted, session-spawned), not a compiled service
5. No TypeScript/Python consumption story as a library (only wraps the CLI)
6. UIA constraints: WebView content unreachable, UAC dialogs on secure desktop unreachable

**Conclusion:** agent-rdp proves the IronRDP + DVC + UIA pattern is viable and is ~6 months ahead. rdpilot must go further: clean library API, structured process/window data, compiled remote helper, and a consumption story beyond CLI wrapping.

### Other Investigated Options

| Tool | What it is | Gap |
|------|------------|-----|
| QuickDesk (barry-ran/QuickDesk) | AI-native remote desktop (MCP server), C++/Qt, Chrome Remoting (WebRTC), NOT RDP | Wrong protocol, not an SDK |
| PyRDP (GoSecure/pyrdp) | RDP MITM/security tool, Python, GPL-3.0 | MITM-focused, not a control SDK, last release Jan 2024 |
| rdp-rs (citronneur/rdp-rs) | Pure-Rust RDP client, v0.1.0 | Abandoned since 2020 |
| mcp-vnc (hrrrsn/mcp-vnc) | VNC MCP server | Wrong protocol |
| FreeRDP/freerdp-rs (elmarco) | Rust FFI wrapper for FreeRDP | Last updated 3+ years ago, stale |
| pywinauto | Windows GUI automation from Python | Runs on the machine being automated, not over RDP |

**Explicit negative finding:** No npm package, PyPI package, or Go module providing "RDP session as programmable SDK" was found. The space is genuinely thin.

---

## 2. RDP Client Library Comparison

### IronRDP (Devolutions)

| Attribute | Value |
|-----------|-------|
| Language | Pure Rust |
| License | Apache-2.0 / MIT (dual) |
| Latest version | 0.14.0 (2026-01-13) |
| Maturity | Production — used by Devolutions Gateway, Teleport (migrating), Cloudflare browser RDP |
| Codebase | ~1,531 commits, actively maintained, weekly releases |

**Framebuffer:** `DecodedImage` struct holds the RGBA32 pixel buffer. The active session loop calls `ActiveStage::process()` on each received PDU; graphics updates (RLE, RDP6.0, RemoteFX/RFX) are decoded and applied to `DecodedImage`. The buffer is directly accessible as a `&[u8]` slice — zero-copy conversion to `image::ImageBuffer` is demonstrated in the official screenshot example.

**Input injection:** `ironrdp::input::Database::apply()` converts native events to `FastPathInputEvent` values. These are passed as `RdpInputEvent::FastPath` to `ActiveStage::process_fastpath_input()` which encodes and transmits them. Both keyboard (PS/2 scancodes) and mouse (position, buttons, wheel) are supported.

**Virtual channels:**
- Static Virtual Channels (SVC): established during connection. `ironrdp-svc` provides traits.
- Dynamic Virtual Channels (DVC): `ironrdp-dvc` provides `DvcProcessor` trait. Implement `channel_name()`, `start()`, `process()`, `close()`. Framework handles chunking/reassembly automatically. `ironrdp-dvc-pipe-proxy` enables named-pipe proxying of DVC traffic for testing and integration.

**Embeddability:** Designed as a workspace of reusable crates. Consume only what you need. The `ironrdp-blocking` crate provides a synchronous connection path (used in the screenshot example). `ironrdp-tokio` provides async. No mandatory runtime imposed.

**Key crates for rdpilot:**

| Crate | Version | Purpose |
|-------|---------|---------|
| `ironrdp` | 0.14.0 | Top-level re-export facade |
| `ironrdp-connector` | 0.14.x | Connection state machine |
| `ironrdp-session` | 0.14.x | Active session, fast-path processing, SVC/DVC dispatch |
| `ironrdp-graphics` | 0.14.x | Codec decode, `DecodedImage`, pixel buffer |
| `ironrdp-input` | 0.14.x | Input PDU construction from raw events |
| `ironrdp-dvc` | 0.14.x | DVC traits, `DvcProcessor`, channel management |
| `ironrdp-tokio` | 0.14.x | Tokio async wrappers |
| `ironrdp-tls` | 0.14.x | TLS (rustls) integration |

### FreeRDP

| Attribute | Value |
|-----------|-------|
| Language | C |
| License | Apache-2.0 |
| Latest version | 3.17.0 (2025-08-22) |
| Maturity | Battle-tested reference implementation, 10+ years |

**Framebuffer:** GDI software rendering surface (`rdpGdi`). Callbacks registered on `rdpUpdate`. Embeddable by extending `rdpContext`. The pattern is established but the API is C-idiomatic (function pointers, struct inheritance). The GDI layer produces a raw pixel buffer accessible via the `rdpGdi` structure.

**Input injection:** Supported via `freerdp_input_*` functions in `libfreerdp-client`.

**Virtual channels:** Static and dynamic VCs supported. DVC via `WTSVirtualChannelOpenEx` patterns (server side); client-side DVC plugin model requires a COM-style DLL plugin on Windows targets.

**Embeddability:** Yes, but significant friction:
- C API — requires FFI from any non-C language
- The only maintained Rust FFI wrapper (`freerdp2-sys`, `elmarco/freerdp-rs`) was last updated 3+ years ago and targets an older FreeRDP API
- Building FreeRDP on Windows requires specific CMake/vcpkg setup
- Memory safety is the caller's responsibility

### Microsoft's Own RDP Stack

**Option A: MSTSC / ActiveX control** — Not programmable for framebuffer access without API hooking (Devolutions documented this). The ActiveX control exists but does not expose the decoded framebuffer or DVC creation to arbitrary code. Rejected.

**Option B: Remote Desktop Protocol Provider API (Win32)** — Server-side protocol provider for implementing custom protocols. Not a client SDK.

**Option C: WinRT `Windows.System.RemoteDesktop.*`** — Input handling helpers for text input, not a full RDP client stack.

**Conclusion:** Microsoft does not provide a public, embeddable RDP _client_ library. All viable paths are third-party.

### Recommendation

**Use IronRDP.** Rationale:

1. Pure Rust — no unsafe FFI boundary between the SDK and the RDP protocol layer
2. Framebuffer access is first-class and well-documented (`DecodedImage`, RGBA32, direct byte slice)
3. DVC is implemented as a trait, not a COM plugin — clean integration
4. Actively maintained by Devolutions (a company whose products depend on it), with a stable release cadence
5. agent-rdp proves the exact use case works on IronRDP
6. Teleport is migrating to it for production use; Cloudflare uses it for browser RDP
7. RemoteFX codec support — better image quality than RDP6.0 bitmaps
8. Dual license (MIT/Apache-2.0) is compatible with LGPL-3.0 goal

**Do not use FreeRDP** for rdpilot. The Rust FFI wrapper is abandoned. Writing fresh FFI against the C API introduces unsafe code, build complexity (CMake on Windows), and ongoing maintenance burden — exactly what a Rust-native library eliminates.

---

## 3. Implementation Language

### Decision: Rust as Primary Language

**Rationale:**

The RDP library (IronRDP) is Rust-native. Writing the SDK in a different language would require an FFI bridge through the lowest-level, most performance-sensitive component of the system. That bridge would need to handle:
- Borrowing and lifetime of the `DecodedImage` pixel buffer (large, frequently updated)
- Async session loop integration
- DVC message dispatch

The FFI cost is not zero. For a hot loop (framebuffer + input), it is material.

**Against each alternative:**

| Language | Problem |
|----------|---------|
| TypeScript | V8 has no stable native async RDP path. WASM (ironrdp-web) exists but is browser-targeted; NAPI-RS bindings to IronRDP do not exist and would need to be written. Adds a Node.js process to the critical path. Author fluent in TS but TS is not the right substrate for a systems SDK. |
| Python | PyO3/maturin makes Rust↔Python workable but the boundary overhead is real for high-frequency calls (framebuffer polling, input injection). Python is not the right first layer for a transport-layer SDK. Useful as a consumer later. |
| Go | cgo is painful. No viable RDP library in Go. Writing protocol-level code in Go against a Rust library would require cgo FFI — worse than PyO3 in ergonomics. |

**Consumption story:**
- v1: scripted harness in Rust (same language, zero FFI cost)
- Later MCP server: either a Rust binary exposing a JSON-RPC/MCP socket, or PyO3 Python bindings for the structured data layer (not the hot RDP loop)
- The remote helper (runs on Windows target): Rust binary via `windows` crate, compiled and bootstrapped by the SDK

**Verdict: Write the SDK core in Rust. The remote helper is also Rust (or PowerShell for a v1 quick start, graduated to Rust). Expose higher-level language bindings (PyO3/NAPI-RS) as a subsequent milestone.**

---

## 4. Structured Perception: Remote Helper APIs and Transport

### What the Remote Helper Must Provide

| Data | Windows API | Notes |
|------|-------------|-------|
| Process tree | `Win32_Process` (WMI/CIM) via PowerShell `Get-CimInstance` or `windows` crate COM | Owner, PID, parent PID, image path |
| Window list + geometry | `EnumWindows` + `GetWindowRect` + `GetWindowText` (Win32) | Z-order via `GetWindow(GW_HWNDPREV)` |
| UI Automation tree | `IUIAutomation` COM interface (`UIAutomationCore.dll`) | Element tree, control patterns, bounding rects |
| Launch process | `CreateProcess` / PowerShell `Start-Process` | For bootstrapping and command execution |

**UIA on RDP sessions:** UIA runs in-process on the remote machine. It works in an active RDP session. The key constraint is that UIAutomation requires an interactive session (Session > 0), which a connected RDP session satisfies. WebView2/Electron content is outside the UIA tree (known gap, same as agent-rdp).

**Rust crates for the remote helper:**

| Crate | Version | Purpose |
|-------|---------|---------|
| `windows` (Microsoft) | 0.58.x | Win32, COM, WinRT bindings generated from metadata. Use for `EnumWindows`, `GetWindowRect`, `IUIAutomation`. |
| `uiautomation` (leexgone/uiautomation-rs) | 0.22.0 (2025-07-24) | Higher-level ergonomic wrapper over the `windows` UIA COM interfaces. `UITreeWalker` for traversal. |
| `serde` + `serde_json` | 1.x | Serialize perception data to JSON for transport |

### Transport Options

#### Option A: RDP Dynamic Virtual Channel (DVC) — RECOMMENDED

The remote helper opens a named DVC using `WTSVirtualChannelOpenEx` (Windows Terminal Services API). The local SDK side implements `DvcProcessor` in IronRDP and receives the channel. Data flows over the existing RDP TCP connection — no additional firewall ports, no NAT traversal needed.

**Evidence this works:** agent-rdp uses exactly this pattern for UIA + bidirectional IPC. Microsoft ships official DVC samples in 7 languages including Rust (`microsoft/rdp-dvc-plugin-samples`).

**Pros:**
- No extra port/socket needed — rides the RDP connection
- Low latency (RDP is already a persistent TCP connection)
- Works through NAT/firewalls since RDP itself does
- IronRDP has `ironrdp-dvc` built-in; server side uses standard WTS API

**Cons:**
- Remote helper must be running in the RDP session (Session > 0, same session as the RDP client connection)
- More complex bootstrapping (helper must be deployed and started in the right session)

#### Option B: WinRM / PowerShell Remoting — GOOD FALLBACK FOR STRUCTURED DATA

WinRM (port 5985/5986) provides a separate HTTP/HTTPS-based management channel over which PowerShell commands can be executed. This is how agent-rdp handles UIA in its PowerShell-based approach.

**Pros:**
- Dead simple to use: `Invoke-Command -ComputerName ... -ScriptBlock { ... }`
- Process tree (`Get-CimInstance Win32_Process`) and basic window data trivial via PowerShell
- No extra binary to deploy for initial validation

**Cons:**
- Requires WinRM to be enabled on the target (not always the case; requires configuration)
- Separate TCP port — may be blocked in restrictive environments
- WinRM serializes PSObjects as XML (deserialized objects lose methods); need explicit JSON conversion on the remote side
- Higher per-invocation overhead than DVC for frequent queries
- UIA tree is harder to transport this way (needs explicit JSON serialization PowerShell)

#### Option C: TCP Socket / Named Pipe out-of-band

The remote helper listens on a TCP port or named pipe; the local SDK connects directly.

**Pros:** Simple protocol, full control

**Cons:** Extra firewall port, manual NAT traversal, or requires the helper to punch back through the RDP connection. Unnecessary when DVC is available.

### Recommended Approach

**Phase 1 (v1 validation):** Use WinRM + PowerShell for process tree and window data. Fast to implement, no binary deployment. UIA via a remote PowerShell scriptblock that serializes a subset of the tree to JSON. This unblocks the proof harness immediately.

**Phase 2 (production path):** Compiled Rust remote helper binary (`rdpilot-sensor`). Bootstrapped by the SDK via drive mapping (IronRDP supports RDPDR drive redirection) or WinRM `Copy-Item`. Helper opens a DVC named `rdpilot.sensor.v1`. SDK side registers a `DvcProcessor` on that channel name. Helper serializes perception data (process tree, window list, UIA subtrees) as `serde_json` and sends over DVC. UIA queries stay in Rust via the `uiautomation` crate or `windows` crate direct COM calls.

**Serialization format:** JSON (serde_json) for v1. Protobuf (prost) if wire size becomes a concern later — not needed for v1 inspection tasks.

---

## 5. Screenshot and Input Specifics

### Screenshot capture

IronRDP's active session loop produces `ActiveStageOutput` values. When a graphics update PDU arrives, `ActiveStage::process()` applies it to the `DecodedImage` framebuffer and returns a list of dirty rectangles. The full framebuffer is always available as `image.data()` — a `&[u8]` in RGBA32 format, dimensions `image.width() x image.height()`.

**Full-desktop screenshot:** call `image.data()`, wrap with `image::ImageBuffer::from_raw(width, height, data)`, encode to PNG via the `image` crate. This is exactly what the official `screenshot.rs` example does.

**Per-window screenshot:** There is no native "window screenshot" in RDP. The approach is: (1) query window geometry via the remote helper (`GetWindowRect`), (2) crop the corresponding rectangle from `DecodedImage`. Pixel coordinates are in the remote desktop coordinate system — the same coordinate space as `DecodedImage`. No coordinate mapping needed.

**Codecs decoded by IronRDP (graphics quality):**
- Raw uncompressed bitmaps
- RLE compression
- RDP 6.0 Bitmap Compression
- Microsoft RemoteFX (RFX) — produces best image quality

### Input coordinate mapping

RDP input events (`PointerPositionPduData`) use remote desktop coordinates. `DecodedImage` is sized to the negotiated desktop resolution (set at connection time via `connector::Config`). A click at pixel `(x, y)` in the `DecodedImage` is injected as a mouse move to `(x, y)` in the remote coordinate space — 1:1 mapping. No scaling needed as long as the connection desktop size matches the image dimensions (which it does by construction in IronRDP).

For per-window coordinates: add the window's `GetWindowRect` top-left offset to get the desktop-absolute coordinates. The SDK should expose both a "desktop-absolute click" and a "window-relative click" helper.

---

## Recommended Stack Summary

### Core

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| Rust | 1.78+ (stable) | SDK primary language | IronRDP is Rust-native; zero FFI on the critical path |
| IronRDP | 0.14.0 | RDP protocol client | Best maintained pure-Rust RDP lib, first-class DVC and framebuffer |
| Tokio | 1.x | Async runtime | IronRDP-tokio integration; de-facto standard |
| rustls | 0.23.x | TLS for RDP | Used by IronRDP-tls; memory-safe, no OpenSSL dependency |

### Framebuffer and Image Processing

| Library | Version | Purpose |
|---------|---------|---------|
| `ironrdp-graphics` | 0.14.x | Decode RLE/RDP6/RFX into `DecodedImage` |
| `image` | 0.25.x | PNG/JPEG encode from raw pixel buffer |

### Input

| Library | Version | Purpose |
|---------|---------|---------|
| `ironrdp-input` | 0.14.x | Build `FastPathInputEvent` (mouse, keyboard) |

### Virtual Channel / Sensor Transport

| Library | Version | Purpose |
|---------|---------|---------|
| `ironrdp-dvc` | 0.14.x | `DvcProcessor` trait, channel management |
| `ironrdp-dvc-pipe-proxy` | 0.2.x | Named-pipe DVC proxy (dev/test) |

### Remote Helper (runs on Windows target)

| Library | Version | Purpose |
|---------|---------|---------|
| `windows` | 0.58.x | Win32/COM/WinRT — `EnumWindows`, `GetWindowRect`, `IUIAutomation` |
| `uiautomation` | 0.22.0 | Ergonomic UIA tree walker (`UITreeWalker`) |
| `serde` + `serde_json` | 1.x | Serialize perception data for transport |
| WTS API (in-session) | OS | `WTSVirtualChannelOpenEx` — open the DVC server side |

### Out-of-Band (v1 bootstrap / fallback)

| Tool | Purpose |
|------|---------|
| WinRM / `winrm` crate or PowerShell via `std::process::Command` | Bootstrap remote helper, ad-hoc process/window queries in v1 |

### Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| RDP library | IronRDP 0.14.0 | FreeRDP 3.17.0 (C) | FFI overhead, no maintained Rust bindings, unsafe C |
| RDP library | IronRDP 0.14.0 | rdp-rs 0.1.0 (Rust) | Abandoned 2020 |
| Primary language | Rust | TypeScript | No native IronRDP bindings; WASM is browser-targeted |
| Primary language | Rust | Python | PyO3 overhead on hot framebuffer path; Python not substrate for transport-layer SDK |
| Remote transport | DVC (IronRDP + WTS) | TCP out-of-band socket | Extra port, NAT complexity, unnecessary |
| Remote transport | DVC | WinRM only | WinRM requires separate port, not always enabled, higher latency per-call |
| UIA serialization | `uiautomation` + `serde_json` | Power Automate / UiPath Remote Runtime | Commercial/heavyweight; defeats FOSS goal |
| TLS | rustls | OpenSSL | Memory-safe, no system dependency |

### Installation (SDK core)

```toml
[dependencies]
ironrdp = "0.14"
ironrdp-tokio = "0.14"
ironrdp-tls = "0.14"
ironrdp-dvc = "0.14"
ironrdp-input = "0.14"
tokio = { version = "1", features = ["full"] }
image = "0.25"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_System_Com",
    "Win32_UI_Accessibility",
] }
uiautomation = "0.22"
```

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| IronRDP as RDP library | HIGH | Verified current version (0.14.0, Jan 2026), active, production use by Devolutions/Teleport |
| Framebuffer API | HIGH | `DecodedImage` + RGBA32 + screenshot example verified in official repo |
| Input API | HIGH | `ironrdp-input`, `FastPathInputEvent` verified via docs.rs + DeepWiki |
| DVC viability | HIGH | `ironrdp-dvc` verified; agent-rdp proves the pattern; MS official samples |
| Remote helper UIA | HIGH | `uiautomation` crate v0.22 (Jul 2025) verified; `IUIAutomation` COM works in RDP sessions |
| WinRM as v1 fallback | HIGH | Standard PowerShell remoting, well-documented |
| Rust as primary language | HIGH | Driven by IronRDP being Rust-native; no maintained FFI wrappers for alternatives |
| PyO3 binding cost | MEDIUM | Research shows overhead is real but manageable; not relevant for v1 |
| Remote helper DVC server | MEDIUM | WTS API is documented and standard; agent-rdp proves the concept; v1 starts with WinRM |

---

## Sources

- [IronRDP GitHub (Devolutions)](https://github.com/Devolutions/IronRDP)
- [IronRDP ARCHITECTURE.md](https://github.com/Devolutions/IronRDP/blob/master/ARCHITECTURE.md)
- [IronRDP screenshot.rs example](https://github.com/Devolutions/IronRDP/blob/master/crates/ironrdp/examples/screenshot.rs)
- [DeepWiki: IronRDP DVC](https://deepwiki.com/Devolutions/IronRDP/5.4-dynamic-virtual-channels)
- [DeepWiki: IronRDP Graphics](https://deepwiki.com/Devolutions/IronRDP/3.3-graphics-and-rendering)
- [DeepWiki: IronRDP Session](https://deepwiki.com/Devolutions/IronRDP/3.1-session-management)
- [agent-rdp (thisnick)](https://github.com/thisnick/agent-rdp) — prior art
- [microsoft/rdp-dvc-plugin-samples](https://github.com/microsoft/rdp-dvc-plugin-samples) — DVC samples (7 languages)
- [MS-RDPEDYC specification](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpedyc/3bd53020-9b64-4c9a-97fc-90a79e7e1e06)
- [uiautomation-rs (leexgone)](https://github.com/leexgone/uiautomation-rs)
- [FreeRDP 3.17.0 release](https://www.freerdp.com/) — current version confirmed
- [QuickDesk (barry-ran)](https://github.com/barry-ran/QuickDesk) — prior art (WebRTC, not RDP)
- [PyRDP (GoSecure)](https://github.com/GoSecure/pyrdp) — eliminated
- [rdp-rs (citronneur)](https://github.com/citronneur/rdp-rs) — eliminated (abandoned)
- [Microsoft windows crate docs](https://microsoft.github.io/windows-docs-rs/)
- [ironrdp-server docs.rs](https://docs.rs/ironrdp-server) — input/display types
- [Teleport secure RDP client blog](https://goteleport.com/blog/secure-rdp-client/) — IronRDP migration signal
