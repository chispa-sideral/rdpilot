# rdpilot-mcp

`rdpilot-mcp --session NAME` exposes the pinned Cua Driver's native MCP over
stdio for one existing RDP connection. Connect first with `rdpilot connect
TARGET --name NAME`. The daemon must have a configured `bundle_path`.

The endpoint forwards initialization, tool schemas, requests, responses,
notifications and image content unchanged. It has no desktop tool adapters,
coordinate conversion or additional MCP management tools. Use separate MCP
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
