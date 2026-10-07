# rdpilot-mcp

`rdpilot-mcp --session NAME` exposes the Cua Driver's native MCP over stdio for
one existing RDP connection. Connect first with `rdpilot connect TARGET --name
NAME` (with `CuaEnabled yes`, the default); the daemon downloads and deploys the
Cua bundle during that connect, so no `bundle_path` is needed.

The endpoint forwards initialization, tool schemas, requests, responses,
notifications and image content unchanged, with one exception that the
daemon applies (see Takeover in the main README): it adds an optional boolean
`takeover` argument to every acting tool in `tools/list` results and removes
it from every call before Cua sees it. While a human viewer holds the
session's control lease, the daemon answers an acting call with an `isError`
result that names the holder and the `takeover` argument, and does not
forward it; a call with `"takeover": true` ends the lease first and is then
forwarded. Read-only tools are never refused. This binary stays an opaque
forwarder. It has no desktop tool adapters, coordinate conversion or
additional MCP management tools. Use separate MCP
processes for separate RDP targets. Cua's session labels are guest data and
never select another RDP target.

One active attachment is allowed per target. Caller EOF, transport failure or
runtime termination ends that attachment; requests are never replayed and the
endpoint never reconnects silently. Start a new endpoint explicitly after
recovery. Native `rdpilot screenshot`, `rdpilot input ...` and session management
remain available independently of Cua.

Use `rdpilot put` and `rdpilot get` for verified transfers. Paths returned by
Cua refer to the Windows guest; they are not files on the caller's machine.

The local connection uses the daemon's existing authenticated Unix socket or
Windows named pipe. A compatibility handshake precedes the stream upgrade.
Both framed IPC and stdio lines are bounded to 16 MiB, including the IPC
wrapper; blocked writes terminate after five seconds. Transport errors go to
stderr, never as invented MCP results on stdout.

This thin binary links IPC, JSON, clap and Tokio only. It does not embed Cua,
IronRDP, a tool schema registry or an MCP SDK. Cua owns MCP protocol behavior.
