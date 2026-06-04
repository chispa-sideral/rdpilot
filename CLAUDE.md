<!-- GSD:project-start source:PROJECT.md -->
## Project

**rdpilot**

`rdpilot` is a reusable **SDK/library** that opens and manages an RDP session to a remote **Windows** machine and exposes that session as a controllable, *perceivable* surface. It is the foundation; AI computer use is its first and most demanding consumer.

The library provides two layers of perception over a single managed remote desktop:

- **Pixels (RDP-native):** screenshots of the full desktop or an individual window, plus mouse/keyboard input injection mapped to remote coordinates.
- **Structured (helper-sourced):** the remote process tree, window list/geometry, and the UI Automation (accessibility) tree — data the RDP protocol does *not* carry natively, retrieved via a **thin remote "sensor" helper** (a dumb perception/control component, explicitly NOT the AI agent) running on the target over an RDP virtual channel and/or out-of-band Windows remote management (WinRM/WMI).

The deliberately rejected alternative is the "obvious" path of running the AI agent inside the RDP session. The whole point is that intelligence stays on the **local** workstation and operates the remote desktop through `rdpilot`.

**Core Value:** A local AI agent can connect to a remote Windows desktop over RDP and **read/inspect a program that is only reachable via RDP** — navigating it and reporting what it sees — using both screenshots and structured accessibility data, without installing or running the agent itself on the remote machine.
<!-- GSD:project-end -->

<!-- GSD:stack-start source:research/STACK.md -->
## Technology Stack

## 1. Prior Art Discovery
### agent-rdp (thisnick/agent-rdp) — CLOSEST PRIOR ART
| Attribute | Value |
|-----------|-------|
| URL | https://github.com/thisnick/agent-rdp |
| Language | Rust 72%, PowerShell 13%, TypeScript 11% |
| License | MIT / Apache-2.0 |
| RDP library | IronRDP |
| Latest release | v0.6.5 (2026-02-26), 55 total releases |
| Stars | 13 |
- Screenshots: yes
- Mouse/keyboard input: yes
- Process tree: NOT documented
- Window list/geometry: NOT documented
- UIA tree over DVC: yes (PowerShell agent, DVC transport)
- Clean SDK API surface: no — it is a CLI tool, not a library
### Other Investigated Options
| Tool | What it is | Gap |
|------|------------|-----|
| QuickDesk (barry-ran/QuickDesk) | AI-native remote desktop (MCP server), C++/Qt, Chrome Remoting (WebRTC), NOT RDP | Wrong protocol, not an SDK |
| PyRDP (GoSecure/pyrdp) | RDP MITM/security tool, Python, GPL-3.0 | MITM-focused, not a control SDK, last release Jan 2024 |
| rdp-rs (citronneur/rdp-rs) | Pure-Rust RDP client, v0.1.0 | Abandoned since 2020 |
| mcp-vnc (hrrrsn/mcp-vnc) | VNC MCP server | Wrong protocol |
| FreeRDP/freerdp-rs (elmarco) | Rust FFI wrapper for FreeRDP | Last updated 3+ years ago, stale |
| pywinauto | Windows GUI automation from Python | Runs on the machine being automated, not over RDP |
## 2. RDP Client Library Comparison
### IronRDP (Devolutions)
| Attribute | Value |
|-----------|-------|
| Language | Pure Rust |
| License | Apache-2.0 / MIT (dual) |
| Latest version | 0.14.0 (2026-01-13) |
| Maturity | Production — used by Devolutions Gateway, Teleport (migrating), Cloudflare browser RDP |
| Codebase | ~1,531 commits, actively maintained, weekly releases |
- Static Virtual Channels (SVC): established during connection. `ironrdp-svc` provides traits.
- Dynamic Virtual Channels (DVC): `ironrdp-dvc` provides `DvcProcessor` trait. Implement `channel_name()`, `start()`, `process()`, `close()`. Framework handles chunking/reassembly automatically. `ironrdp-dvc-pipe-proxy` enables named-pipe proxying of DVC traffic for testing and integration.
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
- C API — requires FFI from any non-C language
- The only maintained Rust FFI wrapper (`freerdp2-sys`, `elmarco/freerdp-rs`) was last updated 3+ years ago and targets an older FreeRDP API
- Building FreeRDP on Windows requires specific CMake/vcpkg setup
- Memory safety is the caller's responsibility
### Microsoft's Own RDP Stack
### Recommendation
## 3. Implementation Language
### Decision: Rust as Primary Language
- Borrowing and lifetime of the `DecodedImage` pixel buffer (large, frequently updated)
- Async session loop integration
- DVC message dispatch
| Language | Problem |
|----------|---------|
| TypeScript | V8 has no stable native async RDP path. WASM (ironrdp-web) exists but is browser-targeted; NAPI-RS bindings to IronRDP do not exist and would need to be written. Adds a Node.js process to the critical path. Author fluent in TS but TS is not the right substrate for a systems SDK. |
| Python | PyO3/maturin makes Rust↔Python workable but the boundary overhead is real for high-frequency calls (framebuffer polling, input injection). Python is not the right first layer for a transport-layer SDK. Useful as a consumer later. |
| Go | cgo is painful. No viable RDP library in Go. Writing protocol-level code in Go against a Rust library would require cgo FFI — worse than PyO3 in ergonomics. |
- v1: scripted harness in Rust (same language, zero FFI cost)
- Later MCP server: either a Rust binary exposing a JSON-RPC/MCP socket, or PyO3 Python bindings for the structured data layer (not the hot RDP loop)
- The remote helper (runs on Windows target): Rust binary via `windows` crate, compiled and bootstrapped by the SDK
## 4. Structured Perception: Remote Helper APIs and Transport
### What the Remote Helper Must Provide
| Data | Windows API | Notes |
|------|-------------|-------|
| Process tree | `Win32_Process` (WMI/CIM) via PowerShell `Get-CimInstance` or `windows` crate COM | Owner, PID, parent PID, image path |
| Window list + geometry | `EnumWindows` + `GetWindowRect` + `GetWindowText` (Win32) | Z-order via `GetWindow(GW_HWNDPREV)` |
| UI Automation tree | `IUIAutomation` COM interface (`UIAutomationCore.dll`) | Element tree, control patterns, bounding rects |
| Launch process | `CreateProcess` / PowerShell `Start-Process` | For bootstrapping and command execution |
| Crate | Version | Purpose |
|-------|---------|---------|
| `windows` (Microsoft) | 0.58.x | Win32, COM, WinRT bindings generated from metadata. Use for `EnumWindows`, `GetWindowRect`, `IUIAutomation`. |
| `uiautomation` (leexgone/uiautomation-rs) | 0.22.0 (2025-07-24) | Higher-level ergonomic wrapper over the `windows` UIA COM interfaces. `UITreeWalker` for traversal. |
| `serde` + `serde_json` | 1.x | Serialize perception data to JSON for transport |
### Transport Options
#### Option A: RDP Dynamic Virtual Channel (DVC) — RECOMMENDED
- No extra port/socket needed — rides the RDP connection
- Low latency (RDP is already a persistent TCP connection)
- Works through NAT/firewalls since RDP itself does
- IronRDP has `ironrdp-dvc` built-in; server side uses standard WTS API
- Remote helper must be running in the RDP session (Session > 0, same session as the RDP client connection)
- More complex bootstrapping (helper must be deployed and started in the right session)
#### Option B: WinRM / PowerShell Remoting — GOOD FALLBACK FOR STRUCTURED DATA
- Dead simple to use: `Invoke-Command -ComputerName ... -ScriptBlock { ... }`
- Process tree (`Get-CimInstance Win32_Process`) and basic window data trivial via PowerShell
- No extra binary to deploy for initial validation
- Requires WinRM to be enabled on the target (not always the case; requires configuration)
- Separate TCP port — may be blocked in restrictive environments
- WinRM serializes PSObjects as XML (deserialized objects lose methods); need explicit JSON conversion on the remote side
- Higher per-invocation overhead than DVC for frequent queries
- UIA tree is harder to transport this way (needs explicit JSON serialization PowerShell)
#### Option C: TCP Socket / Named Pipe out-of-band
### Recommended Approach
## 5. Screenshot and Input Specifics
### Screenshot capture
- Raw uncompressed bitmaps
- RLE compression
- RDP 6.0 Bitmap Compression
- Microsoft RemoteFX (RFX) — produces best image quality
### Input coordinate mapping
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
<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->
## Conventions

Conventions not yet established. Will populate as patterns emerge during development.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->
## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:skills-start source:skills/ -->
## Project Skills

No project skills found. Add skills to any of: `.claude/skills/`, `.agents/skills/`, `.cursor/skills/`, `.github/skills/`, or `.codex/skills/` with a `SKILL.md` index file.
<!-- GSD:skills-end -->

<!-- GSD:workflow-start source:GSD defaults -->
## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd-quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd-debug` for investigation and bug fixing
- `/gsd-execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->



<!-- GSD:profile-start -->
## Developer Profile

> Profile not yet configured. Run `/gsd-profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
