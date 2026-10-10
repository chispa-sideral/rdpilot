# Principal and permission model

This page defines who may do what in rdpilot for M1. The operation table is
in [`principal-model.toml`](principal-model.toml). The table describes target
behaviour. Each row also states the behaviour of the daemon today, and the
change needed to reach the target. No public surface changes because of this
page.

## Single-operator model

M1 has one operator. Every authenticated principal is trusted with every
operation. The only restriction is the control-lease rule (see
[Control lease](#control-lease)). ACLs, user and group identities,
per-session or per-user scoping, and restricted share-link classes are
deferred. A principal that is authenticated reaches the whole daemon: every
session and every recording.

## Principals

| Principal | Who | How the daemon identifies it |
| --- | --- | --- |
| `socket_owner` | The CLI over the owner-only socket. | Socket peer. |
| `agent` | An IPC socket peer, and an MCP session over the stdio adapter on that socket. | Socket peer. |
| `human` | An authenticated WebUI user. | Pairing cookie or Cloudflare Access assertion. |

The socket owner and the agent run as the same OS user and use the same
socket. The daemon cannot tell them apart. A rule that treats them
differently is policy, not an OS security boundary. In M1 no such rule exists.

A human is an observer or a controller of a session. This is session state
held by the control lease. It is not a principal and not a permission level.

## Identity per adapter

- IPC: the owner-only socket. On Unix the daemon compares the peer uid with
  its own effective uid (`crates/rdpilot-daemon/src/ipc/unix.rs:105`). On
  Windows an owner-only, protected DACL on the named pipe is the whole
  mechanism (`crates/rdpilot-daemon/src/ipc/windows.rs:21`).
- MCP: an MCP session over the stdio adapter. The adapter forwards requests
  on the IPC socket, so it carries the socket peer identity.
- HTTP: today one bearer token per viewer start, checked together with the
  Host and Origin headers (`crates/rdpilot-daemon/src/viewer/auth.rs:76`).
  Only the control and input routes require an Origin
  (`crates/rdpilot-daemon/src/viewer/auth.rs:129`). The target is a pairing
  cookie or a verified Cloudflare Access assertion.

A loopback or tailnet listener is not owner-only. Every local OS user, and on a
tailnet every node, can open a TCP connection to it. Only the IPC socket has an
OS identity check. Therefore every HTTP request needs a credential. The daemon
never trusts a bare identity header.

## Human authentication

An unauthenticated HTTP request is refused. M1 has two configurable modes.

### Pairing

1. The CLI issues a pairing token over the socket.
2. The human enters the token in the WebUI once. This exchange is the only
   HTTP operation that a caller without a cookie may reach. The token is
   single-use, short-lived and rate-limited.
3. The daemon sets a cookie with `HttpOnly`, `SameSite=Strict` and `Path=/`.
   The cookie keeps the browser logged in.

On a plain-http tailnet listener the `Secure` attribute and the `__Host-`
prefix are not available. A cookie is not isolated by port: other servers on
the same address receive it, and `SameSite` treats other ports as same-site.
Therefore the daemon checks the exact-authority Origin on every POST, not only
on control and input. Pairing state lives in the daemon, not in a viewer start.
A cookie that survives restarts needs a fixed listener port.

Pairing-token issuance is a socket operation. It is not offered over HTTP.
The CLI and the agent can issue a token, because they run as the same OS user.

### Cloudflare Access

The daemon verifies the `Cf-Access-Jwt-Assertion` header on every request. A
bare header is never trusted. The daemon:

- accepts RS256 only, and rejects every other `alg`, including `none`;
- selects the key by `kid` from the team JWKS
  (`https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`), caches the
  keys, and refetches on an unknown `kid` with a rate limit;
- requires `aud` to contain a configured application AUD tag;
- requires `iss` to equal the configured team domain;
- checks `exp` and `nbf` with a small leeway;
- takes the identity from the `email` claim;
- ignores the `CF_Authorization` cookie;
- fails closed when no key verifies.

The configuration holds the team domain, the AUD tag or tags, and the public
origin or origins. In this mode the Host and Origin checks accept the
configured public origins. The Host value that `cloudflared` forwards by
default is not verified here and must be tested by the implementer.

A header set by a tailnet identity proxy is the same class of mode. It needs a
verifiable source first, and is not part of M1.

The controller identity of a human lease becomes the authenticated principal
(the Access email or the pairing identity). Today it is the TCP peer address,
which is `127.0.0.1` for every human behind a local proxy. This changes
public output elsewhere and belongs to the implementing work.

## Action classes

In M1 a class classifies an operation for the table and for later ACL work.
A class restricts no principal.

| Class | Meaning |
| --- | --- |
| `read` | Observes session, recording or daemon state. Changes nothing. |
| `act` | Changes a live session or its daemon-side state: input, control lease, disconnect, recording start, stop and annotate. |
| `transfer` | Moves file or clipboard content into or out of a remote session. |
| `admin` | Changes daemon-wide state outside one live session: viewer start, pairing-token issuance and exchange, recording retention. |
| `credential-command` | Receives or resolves a host credential: connect with a password, or connect by saved host, which runs `PasswordCommand`. |

## Control lease

Input and clipboard require the control lease. This covers mouse and key
events, the Cua `act` class, Cua `clipboard_read`, and clipboard operations.

The socket owner and the agent are the controller by default. A human becomes
the controller by taking the lease. While a human holds the lease, calls from
the socket owner and the agent are refused until they take over. All other
operations are allowed to every authenticated principal, including recording
start, stop, annotate and keep, connect and disconnect.

## Downloads

A file that leaves the remote session (a print PDF, or the output of `Get`)
lands in a per-session staging area in the daemon. Every authenticated
principal, observers included, may fetch staged downloads. A recording keeps
its staged downloads when it is configured to do so.

## PasswordCommand

When the daemon owns connects, `PasswordCommand` runs in the daemon. Today it
runs in the CLI (`crates/rdpilot-cli/src/verbs/session.rs:47`).

- It runs only for a value that comes from the owner-authored hosts file. The
  daemon refuses `-o` and URL credential overrides from principals that are
  not socket principals.
- Any authenticated principal may start it by connecting a saved host.
- The resulting password never crosses an adapter back to any principal.
- `${VAR}` expands against the daemon environment, not the client
  environment. The daemon has no user terminal, so a helper that prompts
  interactively may fail. Both points are target-behaviour changes that no
  test covers yet.
- The `%h`, `%n`, `%r` and `%p` values are validated before shell use, so a
  principal-chosen alias cannot inject shell text.

## Invariants

The table satisfies these rules, row by row:

1. Every HTTP request is authenticated: by the pairing cookie or Cloudflare
   Access, or, for the pairing exchange only, by its single-use, short-lived,
   rate-limited pairing token. A request without authentication gets a refusal
   or, in pairing mode, a static pairing form. The form carries no daemon data
   and performs no operation, so it is not an operation row.
2. Any authenticated human may connect a saved host, which runs
   `PasswordCommand` in the daemon. No restricted share-link class exists in
   M1.
3. A `PasswordCommand` password never leaves the daemon.
4. An unknown Cua tool is `act`.
5. Input and clipboard require the control lease.
6. Every operation has at least one allowed principal.

## The table file

`principal-model.toml` holds one `[[operation]]` entry per row.

| Key | Meaning |
| --- | --- |
| `schema` | Format version. |
| `principals`, `classes`, `decisions` | The allowed values. |
| `adapter` | `ipc`, `http`, `cua` or `planned`. |
| `name` | IPC: the exact `Request` variant. HTTP: the exact `Route` variant. Cua: the class (`read`, `clipboard_read`, `act`). Planned: an identifier. |
| `methods`, `path` | HTTP rows only. `methods` lists exactly the methods that the route accepts. |
| `class` | One of the action classes. |
| `authentication` | Array of the ways the row's adapter identifies its caller: `socket-peer`, `http-session`, `pairing-token`. |
| `summary` | One line. |
| `decision` | One value per principal. |
| `current` | Behaviour of the daemon today. |
| `delta` | The change needed to reach the target, or `none`. |
| `evidence` | `path:line` entries for `current`, as of commit `7a353e6`. Empty only for planned rows. |

A decision is the policy for that principal and that operation, whether or not
an adapter reaches it today. `current` and `delta` state reachability.

- `allow`: the principal may perform the operation.
- `allow-with-lease`: the principal may perform the operation only as the
  current controller of the session.
- `deny`: the principal must not perform the operation through any adapter.

The table has 42 rows: 19 IPC requests, 13 HTTP routes, 3 Cua classes and 7
planned operations. The `Route::BadId` pseudo-route answers 404 and is not an
operation. Planned rows name operations that have no adapter yet; the
implementing work gives each one its adapter and keeps the row.

The only `deny` cells are `ViewerStart` for `human`, `pairing-token-issuance`
for `human`, and `pairing-exchange` for `socket_owner` and `agent` (socket
principals have no HTTP identity; the exchange creates a human principal).
