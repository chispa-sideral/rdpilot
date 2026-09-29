#!/usr/bin/env python3
"""Proof run for the read-only live viewer (`rdpilot view`).

Fake mode (offline, no Windows; fake connector with synthetic frames and a
fake Cua): four sessions in one tab, their activity strips driven by Cua tool
calls through rdpilot-mcp and a native verb, then a panel closed and reopened:
  cargo build --workspace
  uv run --with playwright python3 scripts/e2e/run-viewer-proof.py --fake \
    --bin-dir target/debug --output /private/viewer-fake-proof

Live mode (a small CrabBox Azure Windows lease with two independent Windows
users; release builds, as users install them):
  cargo build --release --workspace
  export RDPILOT_VIEW_A_HOST=... RDPILOT_VIEW_A_USERNAME=... RDPILOT_VIEW_A_PASSWORD=...
  export RDPILOT_VIEW_B_HOST=... RDPILOT_VIEW_B_USERNAME=... RDPILOT_VIEW_B_PASSWORD=...
  # optional: RDPILOT_VIEW_{A,B}_PORT, RDPILOT_VIEW_{A,B}_DOMAIN
  uv run --with playwright python3 scripts/e2e/run-viewer-proof.py \
    --bin-dir target/release --bundle /path/bundle --output /private/viewer-live-proof

Credentials come only from the environment and reach rdpilot only through
subprocess environment, never arguments or evidence. The viewer token is
replaced by <token> in every evidence file, and a final scan fails the run if
the token or a credential value appears in any evidence file. The output
directory is new and owner-only; it holds guest screen content. This harness
provisions nothing: lease the Windows machine with the crabbox-azure-windows
skill first. Tailnet evidence is the host's own tailnet URL plus the rejection
checks on loopback and tailnet; the cross-machine check is the human opening
the printed tailnet URL from another tailnet device.

The 2 s check times the first viewer frame that SHOWS the change: the harness
keeps the canvas pixels of a region before the change and waits for a frame
in which enough of those pixels differ. In live mode the region is the fixture
text box and the change is the typed marker; in fake mode it is the whole
canvas and the change is the next synthetic colour. The check fails if no
frame shows the change within 2 s. Live mode ends session b from the server
side by dropping its RDP connection after its Cua calls finish.

Activity strips: each call row (a successful and a failing Cua call, and a
native verb) must appear within 2 s of the call returning, only in its own
session's strip, and without argument values. Live mode keeps the MCP
transcripts in the temp directory, since they carry the typed text, and
fails if the typed text reaches any evidence file.
"""
import argparse
import asyncio
import base64
import http.client
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import tempfile
import time
import uuid

HERE = Path(__file__).resolve().parent
TARGETS = ("a", "b")
# Fake mode views the page's maximum of four sessions at once; "e" is the
# server-ended session, connected later into a freed panel.
FAKE_TARGETS = ("a", "b", "c", "d")
FAKE_SERVER_END = "e"
# Typed text and argument values carry this; it must never reach the daemon
# output, the page, an HTTP response or any evidence file.
ARGUMENT_MARKER = "rdpilot-argument-marker-5c1e"
# A tool Cua does not have: live mode's failing call.
LIVE_FAILING_TOOL = "rdpilot_proof_missing_tool"
# Acceptance: a change appears in the viewer within 2 s at default settings.
CHANGE_BUDGET_MS = 2000
# The viewer caps each session at 4 frames per second.
MAX_FPS = 4
# A canvas pixel counts as changed when a colour channel moves by more than this.
PIXEL_DELTA = 48
# The typed marker changes about 1400 text-box pixels at 1920x1080; a blinking
# caret changes fewer than 100.
MARKER_MIN_CHANGED = 200

# One panel's activity strip as rendered, newest first.
STRIP_JS = """
(id) => Array.from(document.querySelectorAll(`.panel[data-session="${id}"] .strip li`)).map((li) => ({
  kind: li.dataset.kind, seq: Number(li.dataset.seq), call: li.dataset.call || null,
  outcome: li.dataset.outcome || null,
  text: Array.from(li.querySelectorAll("span")).map((span) => span.textContent).join(" "),
}))
"""

# Canvas helpers run in the page. `baseline` keeps the region pixels of one
# session; `changed` counts region pixels that differ from them and returns the
# frame status read in the same task as the pixels, so both describe one frame.
CANVAS_JS = """
([op, id, region, delta]) => {
  const canvas = document.querySelector(`.panel[data-session="${id}"] canvas`);
  const status = JSON.parse(JSON.stringify(window.rdpilotViewer.sessions[id] || {}));
  const store = (window.rdpilotProofBaselines = window.rdpilotProofBaselines || {});
  const [x0, y0, x1, y1] = region;
  if (!canvas || canvas.width < x1 || canvas.height < y1) {
    return { status, changed: null, reason: `canvas ${canvas ? canvas.width + "x" + canvas.height : "missing"}` };
  }
  const pixels = canvas.getContext("2d").getImageData(x0, y0, x1 - x0, y1 - y0).data;
  if (op === "baseline") {
    store[id] = { width: canvas.width, height: canvas.height, pixels: Array.from(pixels) };
    return { status, changed: 0 };
  }
  const base = store[id];
  if (base.width !== canvas.width || base.height !== canvas.height) {
    return { status, changed: (x1 - x0) * (y1 - y0), reason: "canvas resized" };
  }
  let changed = 0;
  for (let i = 0; i < pixels.length; i += 4) {
    if (Math.abs(pixels[i] - base.pixels[i]) > delta ||
        Math.abs(pixels[i + 1] - base.pixels[i + 1]) > delta ||
        Math.abs(pixels[i + 2] - base.pixels[i + 2]) > delta) {
      changed += 1;
    }
  }
  return { status, changed };
}
"""


class ProofError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProofError(message)


def load_cua_e2e():
    """Reuse the live Cua harness's relay, MCP client and helpers."""
    spec = importlib.util.spec_from_file_location("run_cua_e2e", HERE / "run-cua-e2e.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def credentials_from_env(target):
    prefix = f"RDPILOT_VIEW_{target.upper()}_"
    creds = {key: os.environ.get(prefix + key.upper()) for key in ("host", "port", "username", "password", "domain")}
    missing = [prefix + k.upper() for k in ("host", "username", "password") if not creds[k]]
    require(not missing, "missing environment: " + ", ".join(missing))
    return {k: v for k, v in creds.items() if v}


def http_request(addr, path, host=None, headers=None, method="GET", timeout=20):
    """One request; returns (status, headers dict, body bytes)."""
    ip, port = addr.rsplit(":", 1)
    conn = http.client.HTTPConnection(ip, int(port), timeout=timeout)
    try:
        conn.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
        conn.putheader("Host", host or addr)
        for key, value in (headers or {}).items():
            conn.putheader(key, value)
        conn.endheaders()
        response = conn.getresponse()
        return response.status, {k.lower(): v for k, v in response.getheaders()}, response.read()
    finally:
        conn.close()


class FakeMcp:
    """A minimal MCP client over `rdpilot-mcp` for the fake Cua (fake mode only).

    It keeps no transcript: requests carry the argument marker, and only the
    daemon's and the viewer's outputs are evidence.
    """

    def __init__(self, run, target):
        self.run, self.target, self.proc, self.number = run, target, None, 0

    async def start(self):
        stderr = open(self.run.output / f"mcp-{self.target}.stderr", "w")
        self.proc = await asyncio.create_subprocess_exec(
            str(self.run.bin / "rdpilot-mcp"), "--session", self.target, env=self.run.env,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=stderr)
        stderr.close()
        result = await self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                                   "clientInfo": {"name": "rdpilot-viewer-proof", "version": "1"}})
        require("serverInfo" in result, f"{self.target}: fake Cua initialize")
        await self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        require(len((await self.request("tools/list", {}))["tools"]) == 3, f"{self.target}: fake Cua tools")
        return self

    async def send(self, message):
        self.proc.stdin.write(json.dumps(message).encode() + b"\n")
        await asyncio.wait_for(self.proc.stdin.drain(), 5)

    async def request(self, method, params, timeout=10):
        self.number += 1
        request_id = f"{self.target}-{self.number}"
        await self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while True:
            line = await asyncio.wait_for(self.proc.stdout.readline(), max(0.1, deadline - time.monotonic()))
            require(line, f"{self.target}: MCP EOF during {method}")
            message = json.loads(line)
            if message.get("id") == request_id:
                require("error" not in message, f"{self.target}: {method} error")
                return message["result"]

    async def stop(self):
        if self.proc and self.proc.returncode is None:
            self.proc.stdin.close()
            try:
                await asyncio.wait_for(self.proc.wait(), 10)
            except asyncio.TimeoutError:
                self.proc.kill()
                await self.proc.wait()


class McpRun:
    """What the shared Cua MCP client needs from a run, with its transcripts in
    the temp directory: they carry typed text, so they are not evidence."""

    def __init__(self, proof):
        self.proof = proof
        self.output = proof.temp / "mcp"
        self.output.mkdir(mode=0o700, exist_ok=True)

    def __getattr__(self, name):
        return getattr(self.proof, name)


class Proof:
    def __init__(self, args):
        self.args = args
        self.fake = args.fake
        # Read credentials before creating anything, so a missing variable
        # leaves no partial evidence directory.
        self.credentials = {} if self.fake else {t: credentials_from_env(t) for t in TARGETS}
        self.bin = Path(args.bin_dir).resolve()
        self.output = Path(args.output).resolve()
        self.output.mkdir(parents=True, exist_ok=False, mode=0o700)
        self.temp = Path(tempfile.mkdtemp(prefix="rdpilot-viewer-proof-"))
        self.run_id = uuid.uuid4().hex[:10]
        # Live mode types this into the guest; it must reach no evidence file.
        self.typed_marker = f"viewer-proof-{self.run_id}-{uuid.uuid4().hex[:8]}"
        self.token = None
        self.checks = []
        self.summary = {"mode": "fake" if self.fake else "live", "status": "running", "checks": self.checks, "unverified": []}
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("RDPILOT_") and k != "RUST_LOG"}
        for directory in ("runtime", "config", "share"):
            (self.temp / directory).mkdir(mode=0o700)
        self.env.update({
            "XDG_RUNTIME_DIR": str(self.temp / "runtime"),
            "XDG_CONFIG_HOME": str(self.temp / "config"),
            "RDPILOT_SHARE_ROOT": str(self.temp / "share"),
            "RDPILOT_DAEMON_SINK_PATH": str(self.temp / "sessions.json"),
            "RDPILOT_DAEMON_IDLE_TIMEOUT_MS": "3600000",
            "RDPILOT_DAEMON_EMPTY_GRACE_MS": "3600000",
        })
        if self.fake:
            self.env.update({"RDPILOT_DAEMON_TEST_CONNECTOR": "1", "RDPILOT_DAEMON_TEST_FRAMES": "1",
                             "RDPILOT_DAEMON_TEST_CUA": "1"})
        else:
            self.env["RDPILOT_BUNDLE_PATH"] = str(Path(args.bundle).resolve())
        self.e2e = None if self.fake else load_cua_e2e()
        self.targets = FAKE_TARGETS if self.fake else TARGETS
        self.relays, self.endpoints = {}, {}
        self.daemon = self.viewer = None
        self.urls, self.notices = [], []

    # --- evidence ---------------------------------------------------------

    def secrets(self):
        values = [c["password"] for c in self.credentials.values() if c.get("password")]
        return values

    def clean(self, text):
        for secret in self.secrets():
            text = text.replace(secret, "[REDACTED]")
        if self.token:
            text = text.replace(self.token, "<token>")
        return text

    def check(self, name, **data):
        self.checks.append({"check": name, "passed": True, **data})
        self.save()
        print(name + ": passed", flush=True)

    def save(self):
        (self.output / "summary.json").write_text(self.clean(json.dumps(self.summary, indent=2)))

    def write(self, name, data):
        path = self.output / name
        if isinstance(data, str):
            path.write_text(self.clean(data))
        else:
            path.write_bytes(data)
        return path

    def scan_evidence(self):
        needles = [s.encode() for s in self.secrets()] + ([self.token.encode()] if self.token else [])
        needles.append((ARGUMENT_MARKER if self.fake else self.typed_marker).encode())
        leaks = []
        for path in self.output.rglob("*"):
            if path.is_file():
                data = path.read_bytes()
                leaks += [path.name for needle in needles if needle and needle in data]
        require(not leaks, f"token, credential or argument marker found in evidence: {sorted(set(leaks))}")
        self.check("evidence_contains_no_token_or_credentials", argument_marker_scanned=True,
                   files=sum(1 for p in self.output.rglob('*') if p.is_file()))

    # --- rdpilot processes --------------------------------------------------

    def hosts_file(self, entries):
        """Write a 0600 hosts file for this run; the password is passed only
        through the CLI child's environment (PasswordCommand), never the file."""
        def quote(value):
            return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'
        lines = []
        for target, entry in entries.items():
            lines += [f"Host {target}", f"  HostName {quote(entry['host'])}", f"  User {quote(entry['username'])}",
                      f"  PasswordCommand {quote('printenv E2E_PASSWORD_' + target.upper())}"]
            if entry.get("port"):
                lines.append(f"  Port {entry['port']}")
            if entry.get("domain"):
                lines.append(f"  Domain {quote(entry['domain'])}")
            if entry.get("accept_invalid_certs"):
                lines.append("  AcceptInvalidCerts yes")
        path = self.temp / "hosts"
        path.write_text("\n".join(lines) + "\n")
        path.chmod(0o600)
        return path

    def hosts_for_targets(self):
        if self.fake:
            # The fake connector ends the frames of host "fake-server-end" after 4 s.
            return self.hosts_file({t: {"host": "fake-server-end" if t == FAKE_SERVER_END else f"fake-{t}",
                                        "username": "fake"}
                                    for t in FAKE_TARGETS + (FAKE_SERVER_END,)})
        return self.hosts_file({
            t: {"host": "127.0.0.1", "port": self.relays[t][1], "username": self.credentials[t]["username"],
                "domain": self.credentials[t].get("domain"), "accept_invalid_certs": True}
            for t in TARGETS})

    def target_env(self, target):
        env = dict(self.env)
        env["E2E_PASSWORD_" + target.upper()] = "fake" if self.fake else str(self.credentials[target]["password"])
        return env

    async def cli(self, *arguments, target=None, timeout=90, allow_failure=False):
        env = self.target_env(target) if target else self.env
        proc = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot"), *arguments, "--json", env=env,
                                                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        try:
            out, err = await asyncio.wait_for(proc.communicate(), timeout)
        except BaseException:
            proc.kill()
            await proc.wait()
            raise
        if proc.returncode and not allow_failure:
            raise ProofError(self.clean(f"CLI {arguments[0]} failed: {out.decode(errors='replace')} {err.decode(errors='replace')}"))
        return json.loads(out) if out.strip() else {}

    async def connect(self, target):
        hosts = self.hosts_for_targets()
        result = await self.cli("connect", target, "-F", str(hosts), "--name", target, target=target, timeout=self.args.connect_timeout)
        require(result.get("session") == target, f"connect {target}: {result}")
        if not self.fake:
            require(result.get("bridge_live"), f"{target}: Cua bridge not live")
        self.check(f"{target}.connected")

    async def start_viewer(self):
        args = ["view", "--json"]
        if self.args.bind:
            args += ["--bind", self.args.bind]
        stderr = open(self.output / "view.stderr", "w")
        self.viewer = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot"), *args, env=self.env,
                                                           stdout=asyncio.subprocess.PIPE, stderr=stderr)
        stderr.close()
        text = ""
        deadline = time.monotonic() + 15
        while True:
            line = await asyncio.wait_for(self.viewer.stdout.readline(), max(0.1, deadline - time.monotonic()))
            require(line, "rdpilot view exited before printing URLs")
            text += line.decode()
            try:
                started = json.loads(text)
                break
            except ValueError:
                continue
        self.urls, self.notices = started["urls"], started["notices"]
        self.token = self.urls[0].split("token=", 1)[1]
        require(all(u.endswith("?token=" + self.token) for u in self.urls), "one token per start")
        require(len(self.token) == 64, "token has 64 hex characters")
        addrs = [self.addr(u) for u in self.urls]
        require(addrs[0].startswith("127.0.0.1:"), "loopback URL first")
        for addr in addrs[1:]:
            first, second = (int(x) for x in addr.split(".")[:2])
            require(first == 100 and 64 <= second <= 127, f"non-tailnet address bound: {addr}")
        self.check("viewer_started", urls=[self.clean(u) for u in self.urls], notices=self.notices)

    @staticmethod
    def addr(url):
        return url.split("//", 1)[1].split("/", 1)[0]

    def tailnet_url(self):
        return next((u for u in self.urls[1:]), None)

    async def stop_viewer(self):
        require(self.viewer.returncode is None, "viewer still running before Ctrl-C")
        self.viewer.send_signal(signal.SIGINT)
        code = await asyncio.wait_for(self.viewer.wait(), 10)
        require(code == 0, f"rdpilot view exit code {code}")
        for url in self.urls:
            host, port = self.addr(url).rsplit(":", 1)
            try:
                reader, writer = await asyncio.wait_for(asyncio.open_connection(host, int(port)), 2)
                writer.close()
                raise ProofError(f"viewer port still open on {host}")
            except (OSError, asyncio.TimeoutError):
                pass
        self.check("viewer_stopped_on_ctrl_c_and_ports_closed")

    # --- browser --------------------------------------------------------------

    async def status(self, page):
        return await page.evaluate("() => JSON.parse(JSON.stringify(window.rdpilotViewer))")

    async def wait_state(self, page, predicate, what, timeout):
        deadline = time.monotonic() + timeout
        while True:
            state = await self.status(page)
            if predicate(state):
                return state
            require(time.monotonic() < deadline, f"timed out waiting for {what}: {self.clean(json.dumps(state))[:2000]}")
            await asyncio.sleep(0.05)

    async def screenshot(self, page, name):
        self.write(name, await page.screenshot(full_page=True))

    async def canvas_png(self, page, target, name):
        data = await page.evaluate(
            "(id) => document.querySelector(`.panel[data-session=\"${id}\"] canvas`).toDataURL('image/png')", target)
        return self.write(name, base64.b64decode(data.split(",", 1)[1]))

    async def canvas_baseline(self, page, target, region, name):
        """Keep the region pixels of the current canvas as the pre-change baseline."""
        result = await page.evaluate(CANVAS_JS, ["baseline", target, region, PIXEL_DELTA])
        require(result["changed"] == 0, f"{target}: no baseline for region {region}: {result.get('reason')}")
        await self.canvas_png(page, target, name)
        self.summary.setdefault("baselines", {})[target] = {"region": region, "seq": result["status"].get("seq")}
        return result["status"]

    async def wait_visible_change(self, page, target, region, min_changed, t0, observe_ms=10000):
        """Time from t0 (ms) until the first frame that shows the change.

        A frame shows the change when at least `min_changed` region pixels
        differ from the baseline. Fails when no frame shows it within the
        2 s budget; it keeps polling up to `observe_ms` only to report how
        late the change was.
        """
        seen, visible, samples = None, None, []
        while time.time() * 1000 - t0 < observe_ms:
            result = await page.evaluate(CANVAS_JS, ["changed", target, region, PIXEL_DELTA])
            status = result["status"]
            if status.get("seq") != seen:
                seen = status.get("seq")
                samples.append({"seq": seen, "received_after_t0_ms": round(status.get("receivedAt", 0) - t0),
                                "changed_pixels": result["changed"]})
                if result["changed"] is not None and result["changed"] >= min_changed:
                    visible = status["receivedAt"] - t0
                    await self.canvas_png(page, target, f"{target}-canvas-change-visible.png")
                    break
            await asyncio.sleep(0.02)
        self.summary.setdefault("visibility_samples", {})[target] = samples
        self.save()
        require(visible is not None, f"{target}: no frame showed the change within {observe_ms} ms: {samples}")
        require(visible <= CHANGE_BUDGET_MS,
                f"{target}: the change became visible after {round(visible)} ms (budget {CHANGE_BUDGET_MS} ms)")
        return visible, samples

    def marker_region(self, width, height):
        """The fixture text box interior on the guest desktop.

        The fixture form (900x300) is centred on the working area above a 40 px
        taskbar; the text box is 50 px right and 66 px down from the form's
        top-left corner. At 1920x1080 this is (560, 436, 880, 466).
        """
        if self.args.marker_region:
            return [int(v) for v in self.args.marker_region.split(",")]
        x0, y0 = (width - 900) // 2 + 50, (height - 40 - 300) // 2 + 66
        return [x0, y0, x0 + 320, y0 + 30]

    async def open_page(self, browser):
        url = self.tailnet_url() or self.urls[0]
        if not self.fake and not self.tailnet_url():
            require(self.args.allow_no_tailnet, "no tailnet URL; run on the tailnet host or pass --allow-no-tailnet")
            self.summary["unverified"].append("tailnet access (no tailnet address bound)")
        page = await browser.new_page(viewport={"width": 1600, "height": 1000})
        await page.goto(url)
        require("token=" in page.url, "the token stays in the address bar")
        targets = self.targets
        state = await self.wait_state(page, lambda s: {x["id"] for x in s["list"]} >= set(targets), "session list", 10)
        for target in targets:
            await page.click(f"button[data-session=\"{target}\"]")
        state = await self.wait_state(page, lambda s: all(s["sessions"].get(t, {}).get("seq", 0) > 0 for t in targets),
                                      "first frames of every session", 15)
        await self.screenshot(page, "01-session-list-and-sessions.png")
        listed = [{k: x.get(k) for k in ("id", "name", "host", "status", "connected_since", "last_activity")} for x in state["list"]]
        self.check("session_list_and_sessions_viewed", via="tailnet" if self.tailnet_url() else "loopback",
                   sessions=listed, frames={t: {k: state["sessions"][t][k] for k in ("seq", "width", "height", "state")}
                                            for t in targets})
        return page

    async def sample_rate(self, page, target, seconds):
        first = (await self.status(page))["sessions"][target]["seq"]
        seqs, started = set(), time.monotonic()
        while time.monotonic() - started < seconds:
            seqs.add((await self.status(page))["sessions"][target]["seq"])
            await asyncio.sleep(0.02)
        frames = len(seqs - {first})
        require(frames <= MAX_FPS * seconds + 1, f"{target}: {frames} frames in {seconds} s exceeds the cap")
        self.check(f"{target}.frame_rate_capped", seconds=seconds, frames_received=frames, cap_fps=MAX_FPS)

    def frame_samples(self, target, count=3):
        """Fetch frames directly to record PNG size and capture+encode time."""
        addr, seq, samples = self.addr(self.urls[0]), 0, []
        for _ in range(count):
            status, headers, body = http_request(addr, f"/api/sessions/{target}/frame?after={seq}",
                                                 headers={"Authorization": f"Bearer {self.token}"})
            if status != 200:
                break
            seq = int(headers["x-frame-seq"])
            samples.append({"seq": seq, "png_bytes": len(body), "encode_ms": int(headers["x-frame-encode-ms"]),
                            "width": int(headers["x-frame-width"]), "height": int(headers["x-frame-height"])})
        self.check(f"{target}.png_size_and_encode_time", samples=samples)

    # --- access checks --------------------------------------------------------

    def rejection_matrix(self):
        results = []
        for url in self.urls:
            addr = self.addr(url)
            bearer = {"Authorization": f"Bearer {self.token}"}
            cases = [
                ("no token", "/api/sessions", None, {}, 403),
                ("wrong token", "/api/sessions", None, {"Authorization": "Bearer " + "0" * 64}, 403),
                ("document without token", "/", None, {}, 403),
                ("foreign Origin", "/api/sessions", None, {**bearer, "Origin": "http://evil.example"}, 403),
                ("wrong Host", "/api/sessions", "attacker.example", bearer, 403),
                ("cross-site fetch", "/api/sessions", None, {**bearer, "Sec-Fetch-Site": "cross-site"}, 403),
                ("POST", "/api/sessions", None, {**bearer, "Content-Length": "0"}, 405),
                ("authorised", "/api/sessions", None, bearer, 200),
            ]
            if self.fake:
                events = "/api/sessions/b/events?after=0"
                cases += [
                    ("events no token", events, None, {}, 403),
                    ("events wrong token", events, None, {"Authorization": "Bearer " + "0" * 64}, 403),
                    ("events foreign Origin", events, None, {**bearer, "Origin": "http://evil.example"}, 403),
                    ("events wrong Host", events, "attacker.example", bearer, 403),
                    ("events authorised", events, None, bearer, 200),
                ]
            for name, path, host, headers, expected in cases:
                method = "POST" if name == "POST" else "GET"
                status, _, _ = http_request(addr, path, host=host, headers=headers, method=method)
                results.append({"address": addr.rsplit(":", 1)[0], "case": name, "status": status})
                require(status == expected, f"{name} on {addr}: HTTP {status}, expected {expected}")
        self.check("unauthorised_requests_rejected_on_every_bound_address", results=results)
        if not self.fake and not self.tailnet_url():
            self.summary["unverified"].append("rejection on the tailnet address (no tailnet address bound)")

    # --- activity strip (fake mode) -------------------------------------------------

    async def strip(self, page, target):
        return await page.evaluate(STRIP_JS, target)

    async def wait_strip(self, page, target, predicate, what, budget_s):
        """Wait until `target`'s strip satisfies `predicate`; fail after `budget_s`."""
        start = time.monotonic()
        while True:
            rows = await self.strip(page, target)
            if predicate(rows):
                return rows
            require(time.monotonic() - start < budget_s,
                    f"{target}: strip did not show {what} within {budget_s} s: {rows}")
            await asyncio.sleep(0.02)

    def events_request(self, target, after=0):
        return http_request(self.addr(self.urls[0]), f"/api/sessions/{target}/events?after={after}",
                            headers={"Authorization": f"Bearer {self.token}"})

    @staticmethod
    def calls(rows):
        """(name text, outcome) of call rows, oldest first."""
        return [(r["text"], r["outcome"]) for r in reversed(rows) if r["kind"] == "call"]

    async def frames_advance(self, page, what):
        """Every open panel shows a newer frame within the 2 s budget."""
        before = {t: s["seq"] for t, s in (await self.status(page))["sessions"].items()}
        start = time.monotonic()
        state = await self.wait_state(
            page, lambda s: all(s["sessions"].get(t, {}).get("seq", 0) > seq for t, seq in before.items()),
            f"new frames on every panel ({what})", CHANGE_BUDGET_MS / 1000)
        latency = round((time.monotonic() - start) * 1000)
        require(latency <= CHANGE_BUDGET_MS, f"frames {what}: {latency} ms")
        return {"panels": sorted(before), "latency_ms": latency,
                "seq": {t: state["sessions"][t]["seq"] for t in before}}

    async def timed_row(self, page, target, predicate, what):
        start = time.monotonic()
        rows = await self.wait_strip(page, target, predicate, what, CHANGE_BUDGET_MS / 1000)
        return rows, round((time.monotonic() - start) * 1000)

    async def strip_flow(self, page):
        """Four panels in one tab: Cua calls on c and d, a native verb on a."""
        timings = {}
        secret_args = {"text": ARGUMENT_MARKER, "path": f"/tmp/{ARGUMENT_MARKER}"}
        c, d = await FakeMcp(self, "c").start(), await FakeMcp(self, "d").start()
        self.endpoints.update({"c": c, "d": d})
        for target in ("c", "d"):
            _, timings[f"{target}.cua_attached"] = await self.timed_row(
                page, target, lambda rows: any("Cua attached" in r["text"] for r in rows), "Cua attached")

        result = await c.request("tools/call", {"name": "echo", "arguments": secret_args})
        require(not result.get("isError"), "fake echo failed")
        _, timings["c.ok"] = await self.timed_row(page, "c", lambda rows: self.calls(rows)[-1:] and
                                                  self.calls(rows)[-1][1] == "ok", "ok call")
        result = await c.request("tools/call", {"name": "fail", "arguments": secret_args})
        require(result.get("isError") is True, "fake fail did not fail")
        _, timings["c.error"] = await self.timed_row(page, "c", lambda rows: len(self.calls(rows)) == 2 and
                                                     self.calls(rows)[-1][1] == "error", "error call")

        await d.request("tools/call", {"name": "echo", "arguments": secret_args})
        await d.send({"jsonrpc": "2.0", "id": "d-held", "method": "tools/call",
                      "params": {"name": "hold", "arguments": secret_args}})
        _, timings["d.running"] = await self.timed_row(page, "d", lambda rows: [o for _, o in self.calls(rows)] ==
                                                       ["ok", "running"], "held call running")
        await d.stop()
        _, timings["d.no_reply_and_detached"] = await self.timed_row(
            page, "d", lambda rows: [o for _, o in self.calls(rows)] == ["ok", "no_reply"] and
            any("Cua detached (" in r["text"] for r in rows), "held call no_reply and detach")

        shot = self.temp / "native.png"
        await self.cli("screenshot", "--session", "a", "--output", str(shot))
        _, timings["a.native"] = await self.timed_row(page, "a", lambda rows: self.calls(rows)[-1:] and
                                                      self.calls(rows)[-1][1] == "ok", "native verb")
        await self.screenshot(page, "02-four-panels-with-strips.png")
        self.check_strips(await self.strips(page), "before reopen")
        frames = await self.frames_advance(page, "four panels")
        self.check("strip_rows_in_order_within_budget", budget_ms=CHANGE_BUDGET_MS, row_latency_ms=timings,
                   frames=frames)

        # Close and reopen a panel: its in-flight requests are aborted, and the
        # reopened panel rebuilds its strip and keeps up with the others.
        await page.click('button[data-session="c"]')
        await self.wait_state(page, lambda s: "c" not in s["sessions"], "panel c closed", 5)
        await page.click('button[data-session="c"]')
        await self.wait_state(page, lambda s: s["sessions"].get("c", {}).get("seq", 0) > 0, "panel c reopened", 5)
        panels = await page.evaluate('() => document.querySelectorAll(\'.panel[data-session="c"]\').length')
        require(panels == 1, f"c: {panels} panels after reopen")
        await c.request("tools/call", {"name": "echo", "arguments": secret_args})
        _, reopen_ms = await self.timed_row(page, "c", lambda rows: [o for _, o in self.calls(rows)] ==
                                            ["ok", "error", "ok"], "call after reopen")
        strips = await self.strips(page)
        self.check_strips(strips, "after reopen", c_calls=3)
        frames = await self.frames_advance(page, "after reopen")
        await c.stop()
        await self.screenshot(page, "03-after-reopen.png")
        self.check("panel_close_and_reopen_keeps_strip_and_frames", row_latency_ms=reopen_ms, frames=frames,
                   strips={t: [r["text"] for r in rows] for t, rows in strips.items()})
        self.check_events_route()

    async def strips(self, page):
        return {t: await self.strip(page, t) for t in self.targets}

    def check_strips(self, strips, when, c_calls=2):
        names = {t: [r["text"] for r in rows] for t, rows in strips.items()}
        blob = json.dumps(names)
        require(ARGUMENT_MARKER not in blob and "/tmp/" not in blob, f"argument value in the strip ({when})")
        require(self.calls(strips["b"]) == [], f"b: unexpected rows ({when}): {names['b']}")
        a_calls = self.calls(strips["a"])
        require([o for _, o in a_calls] == ["ok"] and " screenshot cli ok " in f" {a_calls[0][0]} ",
                f"a: native row ({when}): {names['a']}")
        c_outcomes = ["ok", "error", "ok"][:c_calls]
        require([o for _, o in self.calls(strips["c"])] == c_outcomes and
                all(" cua " in f" {n} " and n.count("echo") + n.count("fail") == 1 for n, _ in self.calls(strips["c"])),
                f"c: rows ({when}): {names['c']}")
        require([o for _, o in self.calls(strips["d"])] == ["ok", "no_reply"], f"d: rows ({when}): {names['d']}")
        for target in ("a", "b"):
            require(not any("Cua" in n for n in names[target]), f"{target}: another session's Cua rows ({when})")
        for target, rows in strips.items():
            seqs = [r["seq"] for r in rows]
            require(seqs == sorted(seqs, reverse=True), f"{target}: strip not newest first ({when}): {seqs}")

    def check_events_route(self):
        """The events route answers at once, per session, without argument values."""
        results = {}
        for target in self.targets:
            start = time.monotonic()
            status, headers, body = self.events_request(target)
            elapsed = round((time.monotonic() - start) * 1000)
            require(status == 200, f"{target}: events HTTP {status}")
            require(ARGUMENT_MARKER.encode() not in body and b"/tmp/" not in body, f"{target}: argument value in events")
            page = json.loads(body)
            require(page["header"]["session"] == target, f"{target}: events of another session")
            status, _, empty = self.events_request(target, page["latest"])
            require(status == 200 and json.loads(empty)["events"] == [], f"{target}: events after latest")
            results[target] = {"events": len(page["events"]), "latest": page["latest"], "elapsed_ms": elapsed}
            self.write(f"events-{target}.json", json.dumps(page, indent=2))
        bearer = {"Authorization": f"Bearer {self.token}"}
        addr = self.addr(self.urls[0])
        for after in ("x", "-1"):
            status, _, _ = http_request(addr, f"/api/sessions/a/events?after={after}", headers=bearer)
            require(status == 400, f"after={after}: HTTP {status}")
        status, _, _ = http_request(addr, "/api/sessions/a/events", headers={**bearer, "Content-Length": "0"},
                                    method="POST")
        require(status == 405, f"POST events: HTTP {status}")
        self.check("events_route_immediate_and_per_session", sessions=results)

    # --- fake mode ---------------------------------------------------------------

    async def fake_flow(self, page):
        # The fake source publishes a new solid colour every 100 ms; the change
        # is visible when most canvas pixels differ from the baseline.
        state = await self.status(page)
        width, height = state["sessions"]["a"]["width"], state["sessions"]["a"]["height"]
        region = [0, 0, width, height]
        await self.canvas_baseline(page, "a", region, "02-a-canvas-before-change.png")
        t0 = time.time() * 1000
        latency, samples = await self.wait_visible_change(page, "a", region, width * height // 2, t0)
        self.check("a.change_visible_within_budget", latency_ms=round(latency), budget_ms=CHANGE_BUDGET_MS,
                   region=region, samples=samples)
        await self.sample_rate(page, "a", 5)
        self.frame_samples("a")
        # The fake frame size switches between 64x48 and 96x64 every 3 s.
        first = (await self.status(page))["sessions"]["a"]["width"]
        await self.wait_state(page, lambda s: s["sessions"]["a"]["width"] not in (0, first), "resize", 8)
        await self.screenshot(page, "03-resized.png")
        self.check("a.resize_shown")
        # A server-ended session: host "fake-server-end" ends its frames 4 s
        # after connect. Free a panel for it first.
        end = FAKE_SERVER_END
        await page.click('button[data-session="b"]')
        await self.wait_state(page, lambda s: "b" not in s["sessions"], "panel b closed", 5)
        await self.connect(end)
        await self.wait_state(page, lambda s: end in {x["id"] for x in s["list"]}, f"{end} listed", 5)
        await page.click(f'button[data-session="{end}"]')
        await self.wait_state(page, lambda s: s["sessions"][end]["state"] == "ended", "ended banner", 10)
        rows = await self.wait_strip(page, end, lambda rows: any("Session ended by the server" in r["text"] for r in rows),
                                     "session-ended marker", 2)
        await self.screenshot(page, "04-server-ended-banner.png")
        self.check(f"{end}.disconnect_shown", banner="Disconnected (server ended the session)",
                   strip=[r["text"] for r in rows if r["kind"] == "marker"])
        await self.cli("disconnect", "--session", "a")
        await self.wait_state(page, lambda s: s["sessions"]["a"]["state"] == "closed", "closed banner", 5)
        await self.wait_state(page, lambda s: "a" not in {x["id"] for x in s["list"]}, "list without a", 5)
        await self.screenshot(page, "05-session-closed-banner.png")
        # The page stops asking for a closed session's events.
        status, _, body = self.events_request("a")
        require(status == 410 and json.loads(body) == {"state": "closed"}, f"a: events after close HTTP {status}")
        self.check("a.close_shown_and_listed", events_status=status)

    # --- live mode ---------------------------------------------------------------

    def fixture_title(self, target):
        return f"rdpilot-view-{self.run_id}-{target}"

    async def launch_app(self, target):
        """Launch a small WinForms window with a text box through Cua MCP."""
        e2e, endpoint = self.e2e, self.endpoints[target]
        title = self.fixture_title(target)
        script = r"""
Add-Type -AssemblyName System.Windows.Forms
$f=New-Object Windows.Forms.Form;$f.Text=TITLE;$f.Width=900;$f.Height=300;$f.StartPosition='CenterScreen'
$t=New-Object Windows.Forms.TextBox;$t.AccessibleName='Proof input';$t.Left=20;$t.Top=30;$t.Width=820
$t.Font=New-Object Drawing.Font('Consolas',20)
$f.Controls.Add($t);[Windows.Forms.Application]::Run($f)
""".replace("TITLE", e2e.psquote(title))
        return await endpoint.tool("launch_app", e2e.powershell(script))

    async def find_fixture(self, target):
        e2e, endpoint = self.e2e, self.endpoints[target]
        title = self.fixture_title(target)
        for _ in range(40):
            listing = e2e.structured(await endpoint.tool("list_windows"))
            for obj in e2e.objects(listing):
                if title in str(obj.get("title", obj.get("window_title", ""))):
                    wid, pid = obj.get("window_id", obj.get("hwnd", obj.get("id"))), obj.get("pid")
                    if wid is not None and pid is not None:
                        return {"pid": int(pid), "window_id": int(wid)}
            await asyncio.sleep(0.5)
        raise ProofError(f"{target}: fixture window not discoverable")

    def call_row(self, name, source, outcome, after):
        """A predicate: a call row newer than call-row count `after` shows `name`, `source`, `outcome`."""
        def predicate(rows):
            calls = self.calls(rows)[after:]
            return any(f" {name} {source} {outcome} " in f" {text} " and got == outcome for text, got in calls)
        return predicate

    async def live_call(self, page, target, name, source, outcome, call):
        """Run `call` (a coroutine factory) and time its strip row from when it returns."""
        before = len(self.calls(await self.strip(page, target)))
        result = await call()
        rows, latency = await self.timed_row(page, target, self.call_row(name, source, outcome, before),
                                             f"{name} {source} {outcome}")
        self.summary.setdefault("strip_row_latency_ms", {})[f"{target}.{name}"] = latency
        return result, rows, latency

    async def failing_tool_call(self, target, name, arguments):
        """A Cua call expected to fail; returns how it failed. Nothing is logged."""
        endpoint = self.endpoints[target]
        endpoint.number += 1
        request_id = f"{target}-{endpoint.number}"
        await endpoint.send({"jsonrpc": "2.0", "id": request_id, "method": "tools/call",
                             "params": {"name": name, "arguments": arguments}})
        deadline = time.monotonic() + 30
        while True:
            line = await asyncio.wait_for(endpoint.proc.stdout.readline(), max(0.1, deadline - time.monotonic()))
            require(line, f"{target}: MCP EOF during the failing call")
            message = json.loads(line)
            if "method" in message and "id" in message:
                await endpoint.send({"jsonrpc": "2.0", "id": message["id"],
                                     "error": {"code": -32601, "message": "client capability not offered"}})
                continue
            if message.get("id") == request_id and "method" not in message:
                if "error" in message:
                    return "jsonrpc_error"
                require(message.get("result", {}).get("isError") is True, f"{target}: {name} did not fail")
                return "is_error"

    async def live_flow(self, page):
        for target in TARGETS:
            # MCP transcripts carry typed text and screenshots: keep them out
            # of the evidence directory (they are deleted with the temp dir).
            self.endpoints[target] = await self.e2e.Mcp(McpRun(self), target).start()
        for target in TARGETS:
            await self.wait_strip(page, target, lambda rows: any("Cua attached" in r["text"] for r in rows),
                                  "Cua attached", 5)
        _, _, launch_ms = await self.live_call(page, "a", "launch_app", "cua", "ok",
                                               lambda: self.launch_app("a"))
        window = await self.find_fixture("a")
        await asyncio.sleep(3)
        state = await self.status(page)
        region = self.marker_region(state["sessions"]["a"]["width"], state["sessions"]["a"]["height"])
        await self.canvas_baseline(page, "a", region, "05-a-canvas-before-typing.png")
        marker = self.typed_marker
        window_state = await self.endpoints["a"].tool(
            "get_window_state", {**window, "include_screenshot": False, "include_accessibility_tree": True})
        element = self.e2e.Run.element(self.e2e.structured(window_state), "Proof input")
        await self.endpoints["a"].tool("type_text", {**window, "element_token": element, "text": marker})
        # t0 is when the Cua call returns; the change counts only once a viewer
        # frame shows the marker in the text box. The strip row is timed from
        # the same moment, concurrently.
        t0 = time.time() * 1000
        type_rows = self.wait_strip(page, "a", self.call_row("type_text", "cua", "ok", 0), "type_text cua ok",
                                    CHANGE_BUDGET_MS / 1000)
        (latency, samples), _ = await asyncio.gather(
            self.wait_visible_change(page, "a", region, MARKER_MIN_CHANGED, t0), type_rows)
        type_ms = round(time.time() * 1000 - t0)
        self.summary.setdefault("strip_row_latency_ms", {})["a.type_text"] = f"<= {type_ms}"
        await self.screenshot(page, "06-page-marker-visible.png")
        self.check("a.cua_change_visible_within_budget", latency_ms=round(latency), budget_ms=CHANGE_BUDGET_MS,
                   marker="typed marker (value withheld)", region=region, samples=samples)
        failure, _, fail_ms = await self.live_call(
            page, "a", LIVE_FAILING_TOOL, "cua", "error",
            lambda: self.failing_tool_call("a", LIVE_FAILING_TOOL, {"text": marker}))
        native = self.output / "06-a-native-screenshot.png"
        _, rows, native_ms = await self.live_call(
            page, "a", "screenshot", "cli", "ok",
            lambda: self.cli("screenshot", "--session", "a", "--output", str(native)))
        await self.screenshot(page, "06b-a-activity-strip.png")
        await self.check_live_strips(page, marker)
        self.check("a.strip_shows_each_call_within_budget", budget_ms=CHANGE_BUDGET_MS,
                   row_latency_ms={"launch_app": launch_ms, "type_text": f"<= {type_ms}",
                                   LIVE_FAILING_TOOL: fail_ms, "screenshot": native_ms},
                   failing_call=failure, strip=[r["text"] for r in rows])
        self.compare_optional(self.output / "a-canvas-change-visible.png", native)
        await self.sample_rate(page, "a", 10)
        self.frame_samples("a")
        self.frame_samples("b")
        # End session b from the server side: stop its Cua MCP client first, so
        # no Cua call is in flight, then drop its RDP connection. (A guest
        # logoff through Cua ends the bridge before the call returns.)
        await self.endpoints["b"].stop()
        await self.relays["b"][0].drop()
        await self.wait_state(page, lambda s: s["sessions"]["b"]["state"] == "ended", "ended banner", 120)
        ended = await self.session_ended_marker(page, "b")
        await self.screenshot(page, "07-b-server-ended-banner.png")
        self.check("b.disconnect_shown", method="RDP connection dropped", banner="Disconnected (server ended the session)",
                   session_ended_marker=ended)
        await self.endpoints["a"].stop()
        await self.cli("disconnect", "--session", "a")
        await self.wait_state(page, lambda s: s["sessions"]["a"]["state"] == "closed", "closed banner", 10)
        await self.wait_state(page, lambda s: "a" not in {x["id"] for x in s["list"]}, "list without a", 5)
        await self.screenshot(page, "08-a-session-closed-banner.png")
        self.check("a.close_shown_and_listed")

    async def check_live_strips(self, page, marker):
        """Each strip holds only its own session's calls, names only; the
        events route answers per session with no argument values."""
        strips = await self.strips(page)
        names = {t: [r["text"] for r in rows] for t, rows in strips.items()}
        blob = json.dumps(names)
        require(marker not in blob, "typed text in a strip")
        require(self.token not in await page.content(), "viewer token in the page DOM")
        for secret in self.secrets():
            require(secret not in await page.content(), "credential in the page DOM")
        a_names = [n for n, _ in self.calls(strips["a"])]
        for name in ("launch_app", "type_text", LIVE_FAILING_TOOL):
            require(not any(f" {name} " in f" {n} " for n, _ in self.calls(strips["b"])),
                    f"b shows a's {name} call: {names['b']}")
            require(any(f" {name} cua " in f" {n} " for n in a_names), f"a: no {name} row: {names['a']}")
        for target, rows in strips.items():
            require(any("Cua attached" in r["text"] for r in rows), f"{target}: no Cua attached row")
            seqs = [r["seq"] for r in rows]
            require(seqs == sorted(seqs, reverse=True), f"{target}: strip not newest first: {seqs}")
        results = {}
        for target in TARGETS:
            status, _, body = self.events_request(target)
            require(status == 200, f"{target}: events HTTP {status}")
            require(marker.encode() not in body, f"{target}: typed text in the events route")
            events = json.loads(body)
            require(events["header"]["session"] == target, f"{target}: events of another session")
            results[target] = {"events": len(events["events"]), "latest": events["latest"]}
            self.write(f"events-{target}.json", json.dumps(events, indent=2))
        self.check("strips_per_session_names_only", strips=names, events=results)

    async def session_ended_marker(self, page, target):
        """The server-ended session shows its end marker in the strip or, if the
        panel stopped polling first, in the events route."""
        try:
            await self.wait_strip(page, target, lambda rows: any("Session ended by the server" in r["text"]
                                                                 for r in rows), "session ended marker", 10)
            return "strip"
        except ProofError:
            status, _, body = self.events_request(target)
            require(status == 200 and any(e["kind"] == "session_ended" for e in json.loads(body)["events"]),
                    f"{target}: no session_ended event (events HTTP {status})")
            return "events route"

    def compare_optional(self, canvas, native):
        """Extra evidence only: a threshold miss never fails the proof."""
        try:
            from PIL import Image, ImageChops, ImageStat
        except ImportError:
            self.summary["unverified"].append("canvas vs native screenshot comparison (Pillow not available)")
            return
        a, b = Image.open(canvas).convert("RGB"), Image.open(native).convert("RGB")
        if a.size != b.size:
            self.summary["canvas_vs_native"] = {"sizes": [a.size, b.size]}
            return
        diff = sum(ImageStat.Stat(ImageChops.difference(a, b)).mean) / 3
        self.summary["canvas_vs_native"] = {"mean_abs_diff": round(diff, 2), "size": a.size}
        self.save()

    # --- orchestration ---------------------------------------------------------------

    async def execute(self):
        from playwright.async_api import async_playwright

        daemon_log = open(self.output / "daemon.log", "w")
        browser = None
        try:
            if not self.fake:
                for target in TARGETS:
                    relay = self.e2e.Relay(self.credentials[target])
                    self.relays[target] = (relay, await relay.start())
            self.daemon = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot-daemon"), env=self.env,
                                                               stdout=daemon_log, stderr=daemon_log)
            await asyncio.sleep(0.3)
            for target in self.targets:
                await self.connect(target)
            await self.start_viewer()
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(headless=True)
                page = await self.open_page(browser)
                if self.fake:
                    await self.strip_flow(page)
                    await self.fake_flow(page)
                else:
                    await self.live_flow(page)
                await browser.close()
            self.rejection_matrix()
            await self.stop_viewer()
            self.summary["status"] = "passed"
        except BaseException as error:
            self.summary["status"] = "failed"
            self.summary["failure"] = self.clean(f"{type(error).__name__}: {error}")
            raise
        finally:
            for endpoint in self.endpoints.values():
                await endpoint.stop()
            if self.viewer and self.viewer.returncode is None:
                self.viewer.kill()
                await self.viewer.wait()
            for target in self.targets + ((FAKE_SERVER_END,) if self.fake else ()):
                try:
                    await self.cli("disconnect", "--session", target, timeout=15, allow_failure=True)
                except (OSError, asyncio.TimeoutError, ValueError, ProofError):
                    pass
            if self.daemon and self.daemon.returncode is None:
                self.daemon.terminate()
                try:
                    await asyncio.wait_for(self.daemon.wait(), 5)
                except asyncio.TimeoutError:
                    self.daemon.kill()
                    await self.daemon.wait()
            for relay, _ in self.relays.values():
                await relay.close()
            daemon_log.close()
            for path in (self.output / "daemon.log", self.output / "view.stderr"):
                if path.exists():
                    path.write_text(self.clean(path.read_text(errors="replace")))
            self.save()
            shutil.rmtree(self.temp, ignore_errors=True)
        self.scan_evidence()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fake", action="store_true", help="offline run against the fake connector")
    parser.add_argument("--bin-dir", required=True, help="directory with rdpilot, rdpilot-daemon, rdpilot-mcp")
    parser.add_argument("--bundle", help="Cua bundle directory (live mode)")
    parser.add_argument("--output", required=True, help="new evidence directory; must not already exist")
    parser.add_argument("--bind", choices=["loopback", "loopback+tailnet"],
                        help="viewer bind set (default: the viewer's default, loopback+tailnet)")
    parser.add_argument("--marker-region", metavar="X0,Y0,X1,Y1",
                        help="live mode: guest pixel box of the fixture text box (default: derived from the desktop size)")
    parser.add_argument("--allow-no-tailnet", action="store_true", help="live mode: run without a tailnet URL (leaves tailnet checks unverified)")
    parser.add_argument("--allow-debug", action="store_true", help="live mode: allow debug binaries (the 2 s budget assumes release builds)")
    parser.add_argument("--connect-timeout", type=int, default=600)
    args = parser.parse_args()
    if not args.fake:
        if not args.bundle:
            parser.error("live mode needs --bundle")
        if "debug" in Path(args.bin_dir).resolve().parts and not args.allow_debug:
            parser.error("live mode needs release binaries (cargo build --release --workspace)")
    if args.marker_region:
        parts = args.marker_region.split(",")
        if len(parts) != 4 or not all(v.strip().isdigit() for v in parts):
            parser.error("--marker-region needs four non-negative integers X0,Y0,X1,Y1")
        x0, y0, x1, y1 = (int(v) for v in parts)
        if x1 <= x0 or y1 <= y0:
            parser.error("--marker-region needs X1 > X0 and Y1 > Y0")
    os.umask(0o077)
    try:
        proof = Proof(args)
    except (ProofError, OSError) as error:
        print(f"Viewer proof not started: {error}", flush=True)
        return 2
    try:
        asyncio.run(proof.execute())
    except (Exception, KeyboardInterrupt) as error:
        print(proof.clean(f"Viewer proof failed: {type(error).__name__}: {error}"), flush=True)
        return 1
    print(f"Evidence: {proof.output / 'summary.json'}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
