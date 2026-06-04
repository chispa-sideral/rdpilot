# Domain Pitfalls: AI-Driven Computer Use over RDP

**Domain:** RDP + Windows perception SDK (pixels + UIA, thin sensor helper, local agent)
**Researched:** 2026-06-04
**Overall confidence:** HIGH — most pitfalls confirmed by Microsoft documentation, RPA vendor documentation, and open-source project issue trackers

---

## Critical Pitfalls

Mistakes that cause silent failures, full rewrites, or security incidents.

---

### Pitfall C1: Minimized or Disconnected RDP Window Kills UI Automation and Screenshots

**What goes wrong:**
When the local RDP client window is minimized, the Windows RDP client by default disconnects the remote display buffer to conserve resources. On the remote machine, the result is that the display pipeline stops rendering — the desktop resolution may drop to 1024×768 or 640×480, applications stop painting, and any automation relying on screen coordinates, screenshot capture, or UI Automation element geometry fails silently or with timeout exceptions. When the session is fully disconnected (not just minimized), the remote session locks, showing the logon screen on the virtual desktop, which means no GUI exists for automation to interact with at all.

This is the single most-documented failure mode across all RPA platforms (UiPath, TestComplete, Ranorex, Power Automate Desktop, pywinauto) operating over RDP. It is not an edge case — it is the default behavior.

**Why it happens:**
- The RDP client (`mstsc.exe`) uses an optimization called "suppress when minimized" — it signals the server to stop sending graphics updates when the local window is not visible.
- The remote DWM (Desktop Window Manager) stops compositing when there is no display consumer.
- UI Automation's `IUIAutomation` queries depend on the element actually being rendered — `BoundingRectangle` returns empty, `FindAll` returns zero results, and cached elements go stale.
- On disconnect, `winlogon.exe` switches the interactive desktop to a locked (non-input) desktop. `SendInput` and `SetCursorPos` require `DESKTOP_JOURNALPLAYBACK` access on the *input* desktop; on a locked desktop, this returns `ERROR_ACCESS_DENIED (5)`.

**Warning signs:**
- Automation works while RDP window is visible, fails the moment it is minimized
- Screenshots come back blank or at a wrong resolution (640×480 or 1024×768)
- `FindAll` returns zero results despite elements being visible before minimization
- `SetCursorPos` / `SendInput` raises access denied after session disconnects

**Prevention strategy:**
1. **Client-side registry key** (apply on the machine running the RDP client, not the target):
   ```
   HKLM\SOFTWARE\Microsoft\Terminal Server Client
   HKLM\SOFTWARE\Wow6432Node\Microsoft\Terminal Server Client
   RemoteDesktop_SuppressWhenMinimized = DWORD:2
   ```
   This prevents the client from signaling the server to pause the display pipeline when minimized. Confirmed fix by UiPath, TestComplete, and SmartBear documentation.

2. **Never close the RDP session with the normal disconnect button** when automation must persist. Use `tscon.exe` to hand the session back to the console without locking it:
   ```bat
   for /f "skip=1 tokens=3" %%s in ('query user %USERNAME%') do (
     %windir%\System32\tscon.exe %%s /dest:console
   )
   ```
   This leaves the remote session active and the desktop unlocked. Run with elevated privileges.

3. **Disable the screen saver and power-off timer** on the remote target — both trigger desktop switching to a non-input desktop, producing identical failures.

4. **Disable session idle timeout** (Group Policy: `Set time limit for active but idle RDS sessions`) or implement a keep-alive that sends synthetic input at an interval shorter than the timeout.

5. **For rdpilot specifically:** the SDK must document and enforce the `RemoteDesktop_SuppressWhenMinimized` key as a prerequisite. The scripted proof harness should verify session state before asserting UIA results.

**Phase:** Phase 1 (session lifecycle) and Phase 2 (UIA integration). If not addressed in Phase 1, every UIA test will produce flaky results in Phase 2.

---

### Pitfall C2: RDP Session Semantics — Console vs. Virtual Session Collision

**What goes wrong:**
On Windows workstations (Home, Pro, Enterprise), there is **exactly one interactive console session (Session 1)**. When you connect via RDP, one of two things happens depending on whether anyone is physically logged in:

- If no one is logged in locally: RDP creates a new virtual session (Session 2+) OR takes over the console session.
- If someone is physically logged in as the same user: RDP **disconnects the physical console** and reconnects the same session remotely. The physical monitor goes dark.
- If someone is logged in as a different user: RDP either fails ("session access denied") or forces a logoff, depending on policy.

For Windows Server, each RDP user gets their own virtual session (Session 1, 2, 3...). Only two simultaneous remote sessions are licensed without RDS CALs.

The automation trap: if your SDK reconnects as the same user account from a different client, the *previous* session (which may have been running the automation) gets silently disconnected. All in-flight UIA calls hang or return stale data.

**Warning signs:**
- UIA calls that previously succeeded suddenly return `COMException` with `RPC_E_DISCONNECTED` or `E_FAIL`
- The previous RDP connection silently closes when a new one opens
- Automation works for the first run but breaks on reconnect

**Prevention strategy:**
- Never share the automation user account with interactive logins on the same target.
- Use a **dedicated service account** for rdpilot that is never used for interactive sessions.
- On Windows Server, verify the target is licensed for the session count required.
- The SDK must detect session ID at connect time and assert it matches on each command to detect silent session replacement.
- For workstations: do not connect to the console session (`/admin` or `/console` flag in mstsc) unless you own that machine exclusively — this forcibly disconnects any physical user.

**Phase:** Phase 1 (session management design). The session account model must be decided before any other component is built on top.

---

### Pitfall C3: NLA/CredSSP Authentication — Certificate Trust Failures

**What goes wrong:**
NLA (Network Level Authentication) authenticates the user before the RDP session is established, using CredSSP over TLS. Most modern Windows targets enforce NLA by default. The failure modes are:

1. **Self-signed or untrusted certificate:** If the target uses a self-signed RDP certificate (the default for non-domain machines), the standard RDP client prompts the user to accept it. In an automated/headless context, this prompt blocks the connection entirely or silently fails.
2. **TLS version mismatch:** If TLS 1.0/1.1 is disabled on the target (increasingly common as a hardening measure) but the client library negotiates only older TLS, the handshake fails before NLA completes.
3. **CredSSP encryption oracle patch:** After MS17-010 and related patches, CredSSP has an `AllowEncryptionOracle` policy. A patched client connecting to an unpatched server (or vice versa) will be blocked with "CredSSP Encryption Oracle Remediation" errors.
4. **Domain vs. workgroup:** NLA on domain-joined machines goes through Kerberos. On workgroup machines, it uses NTLM. An automation client running off-domain connecting to a domain-joined server will fall back to NTLM, which some hardened environments explicitly block. If the domain controller is unreachable, NLA will fail even if credentials are correct.
5. **IronRDP note:** TLS session resumption is explicitly not supported in IronRDP's CredSSP implementation — every connection performs a full handshake.

**Warning signs:**
- "CredSSP Encryption Oracle Remediation" error on connect
- Connection immediately fails after TLS negotiation with no error message
- Works on one network, fails on another (domain connectivity issue)
- Connection hangs at "Securing remote connection" phase indefinitely

**Prevention strategy:**
- Store the expected certificate thumbprint at connect time and pin it for subsequent connections; reject on mismatch rather than prompt.
- Always negotiate TLS 1.2+ explicitly in the RDP client library.
- Support `DisableNLA` mode as a fallback for lab/test targets, but log a prominent warning.
- For NTLM fallback: verify that the automation service account exists locally on the target if it is not domain-joined, with a matching password.
- Keep `AllowEncryptionOracle` policies aligned between client and server — document the required GPO setting in SDK prerequisites.
- Do not store credentials in the SDK's configuration files. Use OS credential stores (Windows Credential Manager, DPAPI-encrypted secrets) or environment variables read at connect time.

**Phase:** Phase 1 (authentication and connection). Must be resolved before any integration test can run.

---

### Pitfall C4: UIPI (User Interface Privilege Isolation) Blocks Input Injection and UIA

**What goes wrong:**
Windows Vista+ enforces UIPI: a process cannot send window messages or simulate input to a process running at a *higher* integrity level. This means:

- If the automation helper or the SDK client runs as a standard user and the target application runs elevated (e.g., an admin-required setup program), `SendInput`, `PostMessage`, and most UIA interaction patterns will silently fail with `ERROR_ACCESS_DENIED`.
- Even when running at the same integrity level, if the helper is not signed with a UIAccess certificate and installed to `%ProgramFiles%` or `%SystemRoot%`, Windows blocks it from bypassing UIPI to interact with secure desktops (UAC prompt dialogs, Ctrl+Alt+Del screen).
- UAC dialogs appear on the **secure desktop**, which is a completely separate desktop object. No automation can interact with a UAC dialog without `UIAccess=true` and the binary being signed and in a trusted location.

**Warning signs:**
- Input events appear to be sent (no API exception) but the target application does not respond
- UIA `Invoke()` on a button in an elevated process raises `COMException`
- Automation works for standard applications but fails for anything that launches elevated
- UAC dialogs freeze the automation loop

**Prevention strategy:**
- For v1 (read/inspect): avoid invoking elevated processes. Scope the v1 harness to applications that run at standard user integrity.
- If elevation is needed: the remote sensor helper must itself run elevated (as the session user with admin rights) and use `SendInput` from that elevated context.
- For UAC dialogs: the only safe approach is to disable UAC on the automation target (acceptable for dedicated lab/automation VMs, not for shared machines) or use `UIAccess=true` signed binary.
- Document as a hard requirement: the automation target user must be able to run the target application at the same or lower integrity as the helper.

**Phase:** Phase 2 (input injection). v1 read/inspect scope limits exposure, but the constraint must be documented.

---

### Pitfall C5: AV/EDR Flagging the Sensor Helper as Malware

**What goes wrong:**
A custom Windows executable that: (a) is not code-signed by a known certificate authority, (b) enumerates processes, (c) walks the accessibility tree, (d) injects keyboard/mouse input via `SendInput`, and (e) communicates over a custom channel — will match behavioral signatures used to detect RATs (Remote Access Trojans) and keyloggers. This is not hypothetical: commercial RPA agents (UiPath Robot, Power Automate Desktop, Automation Anywhere) regularly appear in AV/EDR incident queues and require explicit allow-listing.

Specific behaviors that trigger detection:
- Process tree enumeration (matches reconnaissance behavior)
- `SetWindowsHookEx` (keyboard hooking) — used by some UIA approaches
- `SendInput` with inhuman timing (key-down/key-up intervals measured in microseconds, unlike human input)
- Writing an executable to a temp directory and launching it (bootstrap path)
- Unsigned PE with network I/O on a custom port

**Warning signs:**
- Helper executable is quarantined on first deploy
- Helper deploys successfully but input injection silently fails (EDR intercepts the call)
- Process is killed mid-session without error
- AV logs show "suspicious process injection" or "potential keylogger" alerts

**Prevention strategy:**
- Code-sign the helper binary with an Extended Validation (EV) certificate if the tool will be deployed to machines with EDR. For personal/lab use, add to the AV exclusion list explicitly.
- Keep the helper as minimal as possible — single responsibility (sensor + relay), no self-update, no network listener on unexpected ports.
- Use `SendInput` with realistic human-like timing jitter when operating in environments with behavioral EDR. Synthetic input with microsecond precision is a known detection heuristic.
- Bootstrap via a trusted channel (WinRM/PowerShell remoting, not a dropped executable in `%TEMP%`).
- Document the AV exclusion requirement as part of the deployment guide from day one.
- Avoid `SetWindowsHookEx` entirely; prefer UIAutomation's `InvokePattern` and `ValuePattern` for structured interaction.

**Phase:** Phase 3 (sensor helper bootstrap). Must be designed around this constraint; retrofitting is painful.

---

### Pitfall C6: DPI Scaling and Coordinate System Mismatch

**What goes wrong:**
RDP in high-DPI environments has two independent scaling factors:

1. **Local display scaling:** The machine running the RDP client may have 150% or 200% DPI scaling. The RDP client window renders at the local physical resolution.
2. **Remote display scaling:** The remote session inherits the client's DPI setting by default (controlled by `IgnoreClientDesktopScaleFactor` registry key on the server). If the remote session also scales at 150%, all UIA `BoundingRectangle` coordinates are in scaled logical pixels, not physical pixels.
3. **Screenshot vs. UIA coordinate mismatch:** If the screenshot is captured at the RDP wire resolution (unscaled) but UIA reports coordinates in the scaled logical space, click coordinates calculated from UIA geometry will miss by a factor equal to the DPI scale ratio.

On dynamic resolution changes (user resizes the RDP window, or the connection is established with different geometry than expected), the remote desktop resizes and all coordinate mappings become invalid until remeasured.

**Warning signs:**
- Clicks land at consistently wrong positions (offset by a predictable factor like 1.25x or 1.5x)
- UIA `BoundingRectangle` dimensions are physically smaller than what the screenshot shows
- Works on a 1080p machine, breaks on a 4K laptop
- Correct coordinates after fresh connect, wrong coordinates after RDP window resize

**Prevention strategy:**
- At connect time, force a specific desktop resolution that is known and fixed. Negotiate the resolution explicitly in the RDP connection parameters; do not inherit the client's display geometry.
- Set `IgnoreClientDesktopScaleFactor=1` on the remote target to decouple server DPI from client DPI.
- Force 100% DPI scaling on the remote session for the automation user account (registry: `HKCU\Control Panel\Desktop\LogPixels = 96`).
- When capturing screenshots, record the wire-level resolution and pixel dimensions. When querying UIA, record the DPI-aware logical resolution. Emit both in the SDK's perception data structure, and let the consumer perform the mapping.
- After any resolution change event (RDP resize notification), invalidate the coordinate cache and re-query display info before the next action.

**Phase:** Phase 2 (screenshot + UIA integration). The coordinate normalization contract must be defined before any consumer uses coordinates.

---

## Moderate Pitfalls

---

### Pitfall M1: RDP Session Idle Timeout and Reconnect Disruption

**What goes wrong:**
Windows RDP sessions have two independent timeout policies: idle timeout (no user input for N minutes → disconnect) and disconnected session timeout (session has been disconnected for N hours → logoff). Most hardened Windows Server environments set idle timeout to 15–30 minutes. An agent loop that is "thinking" (no RDP input) for longer than the idle timeout will find its session disconnected when it next attempts an action.

On reconnect, if the session has not yet been logged off, the SDK can re-attach. However:
- The desktop may now be locked (requiring a credential re-entry)
- The resolution may have changed on reconnect
- Any in-flight UIA subscriptions or event hooks are dead
- The remote process the agent was operating may have shown a dialog while the session was disconnected (e.g., "save unsaved changes?") — the automation is now blocked waiting for user input with no way to detect it except by screenshot

**Warning signs:**
- Automation fails exactly at the idle timeout interval
- Session reconnect succeeds but subsequent UIA calls return stale handles
- Screenshot shows a lock screen after reconnect

**Prevention strategy:**
- Send a synthetic keep-alive every ~60 seconds: a null mouse move within the remote desktop coordinate space, or a `VK_NONAME` key to prevent idle timeout.
- Configure idle timeout policy on the target to "Never" for the automation service account, or extend it significantly.
- After every reconnect, treat the session as fresh: re-query display info, re-enumerate windows, re-build the UIA snapshot.
- Subscribe to session state change events if the RDP library exposes them; treat a disconnect notification as a hard reset point.

**Phase:** Phase 1 (session lifecycle). Keep-alive must be part of the session management loop.

---

### Pitfall M2: UI Automation Full-Tree Walk Performance

**What goes wrong:**
`IUIAutomation::FindAll` with `TreeScope_Descendants` on a complex application (browser, Office, modern Electron app) can take 5–30 seconds and block the COM thread. The underlying reason is documented by Microsoft: UIA caches the entire descendant scope before applying the search predicate — every node in the tree is marshalled across the in-process boundary.

In a remote session where the UIA call crosses from the helper process to the application process (and potentially across session boundaries), this overhead is amplified. A full tree walk on a Chrome browser with 20 tabs can return tens of thousands of elements.

Additionally, `TreeWalker` traversal (element-by-element) is even slower than `FindAll` for large trees because each `GetFirstChildElement` / `GetNextSiblingElement` call is an individual COM round-trip.

**Warning signs:**
- UIA queries take >5 seconds for complex applications
- The helper process becomes unresponsive during a tree walk
- Memory usage spikes during tree capture (hundreds of MB for large applications)

**Prevention strategy:**
- Never use `TreeScope_Descendants` as the default scope. Default to `TreeScope_Children` and expand only when necessary.
- Walk the tree lazily from a known root (e.g., the window handle returned by `EnumWindows`), not from `GetRootElement`.
- Cache the UIA snapshot with a TTL; do not re-walk on every agent action. Invalidate only on window change events.
- Use `CacheRequest` to fetch multiple properties in a single round-trip rather than querying each property separately.
- Implement a timeout on tree walks and fall back to screenshot-only mode when UIA is unavailable or too slow.

**Phase:** Phase 2 (UIA integration). Must be designed into the helper's perception loop from the start.

---

### Pitfall M3: Elements Without UIA Support (Vision Fallback Required)

**What goes wrong:**
Not all Windows applications expose a useful UIA tree. Known classes of applications that produce sparse or useless trees:

- **Legacy Win32 with owner-draw controls:** custom-drawn list boxes, toolbars, and grids report as a single opaque element with no children.
- **DirectX/OpenGL/game-engine UIs:** the entire application surface is a single `HWND` with no accessibility peers. No UIA structure exists at all.
- **Older MFC applications:** may use `IAccessible` (MSAA) but not full UIA; the MSAA-to-UIA bridge produces a shallow, often incorrect tree.
- **Custom web apps in a WebView2/Edge frame:** inner web elements are accessible only via the browser's own UIA provider, which may not be active in embedded contexts.
- **Electron apps with custom chrome:** depend on the app enabling accessibility explicitly; many do not unless a screen reader is detected.

**Warning signs:**
- UIA tree for a window shows only one or two elements regardless of visible UI complexity
- `GetCurrentPropertyValue(UIA_IsEnabledPropertyId)` returns false on most elements
- Element names are empty or generic ("Window", "Pane")

**Prevention strategy:**
- Design the SDK's perception API to treat UIA as **opportunistic**, not required. The screenshot is always available; UIA enriches it when available.
- For applications known to have poor UIA: fall back to OCR on the screenshot, or to MSAA (`IAccessible`) which has broader legacy coverage.
- Document per-application UIA quality in the test harness to guide the agent on what to expect.

**Phase:** Phase 2 (perception integration). The API design must anticipate mixed-fidelity perception from day one.

---

### Pitfall M4: RDP Virtual Channel Bootstrap — Trust and Version Skew

**What goes wrong:**
A DVC (Dynamic Virtual Channel) plugin requires a client-side component registered on the machine running the RDP client AND a server-side component running in the remote session. The handshake is:

1. Register the client plugin (COM object on the client machine)
2. Establish RDP connection
3. Launch the server-side component inside the remote session

If the client plugin and server component are version-skew'd (e.g., after an SDK update), the channel negotiation may silently fail — the RDP session connects normally but the virtual channel never opens. There is no visible error.

Additionally, the DVC plugin must be registered on every client machine that will run rdpilot. For a library intended to run headlessly (not mstsc.exe), this is moot — IronRDP and FreeRDP implement DVC natively in the client library without requiring COM registration. But if `mstsc.exe` is the transport, the plugin COM registration is a deployment step that must succeed before the channel is available.

**Warning signs:**
- RDP session connects successfully but `OpenDynamicVirtualChannel` on the server side returns `ERROR_NOT_FOUND`
- Channel works after fresh deploy, fails after SDK update without re-registering the plugin

**Prevention strategy:**
- Prefer out-of-band transport (WinRM/named pipe) over a DVC for the sensor helper in v1. DVC is powerful but adds significant complexity to deployment and version management.
- If DVC is used: implement a version handshake as the first message on the channel. If versions mismatch, close and report an error with the version delta — do not silently continue with broken data.
- For IronRDP/FreeRDP as the client library: DVC is part of the library and does not require COM registration — this removes the client-side deployment concern.

**Phase:** Phase 3 (sensor helper transport).

---

### Pitfall M5: FreeRDP 3.x Screenshot API Breakage

**What goes wrong:**
FreeRDP 2.x included a proxy module (`DecordGFX`) that enabled BMP capture of the remote session framebuffer. In FreeRDP 3.x this module was removed with no documented replacement and no migration guide. Developers implementing framebuffer capture against FreeRDP 3.x must manually hook the GDI update pipeline (`SurfaceBits` and `GDI` callbacks) and call `winpr_image_write_bmp` to serialize the buffer — an undocumented, brittle approach.

Additionally, FreeRDP has a known bug where taking a screenshot (via external screenshot software) can crash the RDP connection (FreeRDP issue #8735). The `SDL` renderer in FreeRDP 3.x has exhibited blank output on connect in some configurations (issue #9354).

IronRDP provides a first-class screenshot example (`crates/ironrdp/examples/screenshot.rs`) and is the better-documented path for headless bitmap capture.

**Warning signs:**
- Linker errors or missing symbols when trying to use the `DecordGFX` module with FreeRDP 3.x
- Framebuffer callbacks are called but produce zero-size or all-black bitmaps
- RDP connection drops immediately after a screenshot attempt

**Prevention strategy:**
- Use IronRDP rather than FreeRDP for headless screenshot automation. IronRDP's screenshot example is functional and maintained.
- If FreeRDP must be used: use the GDI bitmap pointer directly from the `freerdp_get_image_format_pixel_format` / `gdi->primary_buffer` path, not the proxy screenshot module.
- Test screenshot reliability under both the GFX pipeline (H.264/RFX) and the classic RemoteFX path — they produce different pixel formats.

**Phase:** Phase 1 (RDP stack selection). This is a stack decision, not a bug to work around later.

---

## Minor Pitfalls

---

### Pitfall m1: Windows Workstation Single-Session Licensing Limit

**What goes wrong:**
Windows 10/11 (all editions) and Windows Server 2019/2022 with default licensing permit only two concurrent RDP sessions (both administrative). Windows workstations permit only one RDP session per user. Attempting a second connection with the same user account either: reconnects to the existing session (disconnecting any in-flight automation), or fails with "session access denied" depending on the `fSingleSessionPerUser` policy.

This is a licensing constraint, not a bug. Bypassing it via `rdpwrap` or registry edits is a license violation.

**Prevention strategy:**
- Use a Windows Server target for any multi-session scenario.
- For single-target development: use one dedicated service account, never share it.
- Document this limit prominently; do not design the v1 API as if multi-session on a workstation is possible.

**Phase:** Phase 1 (target prerequisites documentation).

---

### Pitfall m2: GFX/H.264 Codec Pixel Format Surprises

**What goes wrong:**
Modern RDP uses the Graphics Pipeline Extension (MS-RDPEGFX) with H.264/AVC420 or AVC444 encoding. When the client library decodes the H.264 frame, the output is typically in YUV (NV12 or YUV420) format, not RGB. If the screenshot code assumes RGB/BGRA and reads a YUV buffer, the captured image will have scrambled colors or appear as a grey luma-only image.

AVC444 mode encodes chroma at full resolution, which requires a different decode path than AVC420. Clients that only implement AVC420 decode will produce incorrectly colored screenshots when AVC444 is negotiated.

**Warning signs:**
- Screenshots are grey (luma only) or have severe color banding
- Screenshot colors are correct with some servers, wrong with others
- Colors correct on initial connect, wrong after the server switches codec (which can happen dynamically based on content)

**Prevention strategy:**
- After decoding an RDP frame, explicitly convert YUV → RGB using a SIMD-accelerated converter (libyuv or the platform's media foundation) before passing to the screenshot API.
- Negotiate a specific codec version at connect time rather than accepting whatever the server offers. For automation, prioritize image fidelity over compression efficiency — consider negotiating RemoteFX (lossless-mode) if available.
- Verify screenshot pixel format in the test harness by checking a known-color pixel value.

**Phase:** Phase 1 (RDP client implementation).

---

### Pitfall m3: Credentials in Process Memory and Channel Leakage

**What goes wrong:**
The RDP connection requires plaintext credentials (username + password) at the point of CredSSP negotiation. Whatever credentials the SDK receives must be passed to the RDP library, which holds them in memory for the duration of the handshake. In managed languages (C#, Python), the GC does not zero strings; the credential can persist in heap memory longer than expected.

Additionally, if the RDP virtual channel carries the sensor helper's command stream unencrypted, any other process in the same session that can attach to the DVC can read the channel. On Windows, DVC channels are accessible to code running in the same session.

**Warning signs:**
- Memory dump of the SDK process contains credentials in plaintext
- Other processes in the session can open the virtual channel by name

**Prevention strategy:**
- Use `SecureString` or pinned byte arrays that are explicitly zeroed after the CredSSP handshake completes.
- Do not log credentials anywhere, including debug output and crash dumps.
- Name the virtual channel with a randomized suffix to reduce predictability.
- Treat the DVC as unauthenticated transport — add an application-layer authentication step on first connect (the helper authenticates to the client using a session-derived token, not a stored secret).
- If the channel carries commands that mutate system state, encrypt and MAC the channel even though it runs over RDP's TLS layer — defense in depth.

**Phase:** Phase 3 (sensor helper) and Phase 1 (connection management).

---

### Pitfall m4: UIA Cross-Process and Cross-Session Access Restrictions

**What goes wrong:**
`IUIAutomation` in the local session is the well-tested path. When the sensor helper runs in the RDP session and calls UIA on applications in the same session, it generally works. However:

- If the helper runs at a lower integrity level than the target application, UIA property access is blocked (UIPI applies to UIA just as it does to `SendMessage`).
- UIA calls against applications in a **different** session (cross-session) are blocked at the Windows session boundary — UIA is session-local. This is relevant only if someone attempts to drive the remote session's UIA from the local machine directly (which is not possible); the sensor helper must live in the same session as the target.
- If the application being automated launches elevated child processes (common in installers), UIA access to those children is denied unless the helper is also elevated.

**Prevention strategy:**
- The sensor helper must run **in the same session as the target application** — this is already implied by the architecture (helper runs in the RDP session), but it must be verified at bootstrap.
- The helper process must run at the same or higher integrity level as the most-elevated process it needs to query. For v1 (read/inspect), this means running as the logged-in user (standard integrity) is sufficient for most applications.
- Add a self-test step in the helper bootstrap that verifies it can successfully call `FindAll(TreeScope_Children)` on the desktop root before reporting "ready."

**Phase:** Phase 3 (helper bootstrap), cross-referenced with Phase 2 (UIA integration).

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| RDP stack selection (P1) | FreeRDP 3.x screenshot breakage (C) | Choose IronRDP; verify screenshot example works against target before committing |
| Session lifecycle (P1) | Console vs. virtual session collision (C2) | Dedicated service account; detect session ID at connect |
| Authentication (P1) | NLA/CredSSP cert trust failure (C3) | Pin cert thumbprint; test against both domain-joined and workgroup targets |
| Screenshot capture (P1) | YUV pixel format mismatch (m2) | Force explicit color space conversion; test with a known-color pixel |
| Session management (P1) | Idle timeout disconnection (M1) | Implement keep-alive from day one |
| UIA integration (P2) | Minimized window kills UIA (C1) | `RemoteDesktop_SuppressWhenMinimized=2` must be set before any UIA test |
| UIA integration (P2) | Full tree walk performance (M2) | Default to `Children` scope; cache with TTL |
| UIA integration (P2) | Applications without UIA (M3) | API design: UIA is optional enrichment, screenshot is always the fallback |
| Input injection (P2) | UIPI blocks SendInput (C4) | v1 read-only scope avoids this; document as a Phase 3+ concern |
| Coordinate mapping (P2) | DPI scaling mismatch (C5) | Fix remote DPI to 96 (100%); emit both logical and physical coords |
| Helper bootstrap (P3) | AV/EDR flags helper as RAT (C5) | Minimize helper surface; plan for code signing; document exclusion requirement |
| Helper transport (P3) | DVC version skew (M4) | Version handshake on first message; prefer WinRM in v1 |
| Helper security (P3) | Credential leakage via channel (m3) | Zero credentials post-handshake; randomize channel name |

---

## Sources

- UiPath documentation on executing tasks in minimized RDP windows: https://docs.uipath.com/robot/standalone/2024.10/admin-guide/executing-automations-in-minimized-rdp-windows
- SmartBear TestComplete: Disconnecting from Remote Desktop while running automated tests: https://support.smartbear.com/testcomplete/docs/testing-with/running/via-rdp/keeping-computer-unlocked.html
- Microsoft Power Automate UIPI issues documentation (updated 2026-04-03): https://learn.microsoft.com/en-us/troubleshoot/power-platform/power-automate/desktop-flows/ui-automation/uipi-issues
- UiPath Robot Windows Sessions documentation: https://docs.uipath.com/robot/docs/windows-sessions
- pywinauto issue #1096 "no active desktop required for moving mouse cursor": https://github.com/pywinauto/pywinauto/issues/1096
- FreeRDP issue #11765 "How to implement session capture (BMP screenshot) in FreeRDP3 proxy": https://github.com/FreeRDP/FreeRDP/issues/11765
- FreeRDP issue #8735 "Screenshot crashes RDP connection": https://github.com/FreeRDP/FreeRDP/issues/8735
- IronRDP screenshot example: https://github.com/Devolutions/IronRDP/blob/master/crates/ironrdp/examples/screenshot.rs
- IronRDP Hacker News discussion (HN #43436894): https://news.ycombinator.com/item?id=43436894
- Microsoft documentation: RemoteDesktop_SuppressWhenMinimized registry key (via GitHub gist): https://gist.github.com/GrumpyChunks/bb7b58c63883af8f137a1264078d30ca
- EdgeVerve AssistEdge: Running UI automation with RDP session disconnected: https://www.edgeverve.com/assistedge/knowledge-base/RPA19.0/Troubleshooting/Running_UI_RDPdisconnected.htm
- Microsoft documentation: DPI scaling in RDP (Windows OS Hub): https://woshub.com/dpi-scaling-font-size-rdp/
- GoSecure blog: Capturing RDP NetNTLMv2 Hashes: https://gosecure.ai/blog/2022/01/17/capturing-rdp-netntlmv2-hashes-attack-details-and-a-technical-how-to-guide/
- Microsoft documentation: `SetCursorPos` — input desktop requirement: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setcursorpos
- Microsoft documentation: Session 0 isolation: https://techcommunity.microsoft.com/blog/askperf/application-compatibility---session-0-isolation/372361
- Microsoft documentation: RDP DVC plugin samples: https://learn.microsoft.com/en-us/samples/microsoft/rdp-dvc-plugin-samples/rdp-dvc-plugin-samples/
- TSplus: NLA Errors in RDP — causes, fixes, best practices: https://tsplus.net/advanced-security/blog/nla-errors-in-rdp-causes-fixes-best-practices/
- Microsoft documentation: UI Automation threading issues: https://learn.microsoft.com/en-us/dotnet/framework/ui-automation/ui-automation-threading-issues
