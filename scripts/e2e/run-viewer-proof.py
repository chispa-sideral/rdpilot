#!/usr/bin/env python3
"""Proof run for the read-only live viewer (`rdpilot view`, Ticket 560).

Fake mode (offline, no Windows; fake connector with synthetic frames):
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
# Acceptance: a change appears in the viewer within 2 s at default settings.
CHANGE_BUDGET_MS = 2000
# The viewer caps each session at 4 frames per second.
MAX_FPS = 4
# A canvas pixel counts as changed when a colour channel moves by more than this.
PIXEL_DELTA = 48
# The typed marker changes about 1400 text-box pixels at 1920x1080; a blinking
# caret changes fewer than 100.
MARKER_MIN_CHANGED = 200

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
            self.env.update({"RDPILOT_DAEMON_TEST_CONNECTOR": "1", "RDPILOT_DAEMON_TEST_FRAMES": "1"})
        else:
            self.env["RDPILOT_BUNDLE_PATH"] = str(Path(args.bundle).resolve())
        self.e2e = None if self.fake else load_cua_e2e()
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
        leaks = []
        for path in self.output.rglob("*"):
            if path.is_file():
                data = path.read_bytes()
                leaks += [path.name for needle in needles if needle and needle in data]
        require(not leaks, f"token or credential found in evidence: {sorted(set(leaks))}")
        self.check("evidence_contains_no_token_or_credentials", files=sum(1 for p in self.output.rglob('*') if p.is_file()))

    # --- rdpilot processes --------------------------------------------------

    def target_env(self, target):
        env = dict(self.env)
        if self.fake:
            # The fake connector ends the frames of host "fake-server-end" after 4 s.
            host = "fake-server-end" if target == "b" else f"fake-{target}"
            env.update({"RDPILOT_HOST": host, "RDPILOT_USERNAME": "fake", "RDPILOT_PASSWORD": "fake"})
        else:
            env.update({"RDPILOT_" + key.upper(): str(value) for key, value in self.credentials[target].items()})
            env["RDPILOT_HOST"], env["RDPILOT_PORT"] = "127.0.0.1", str(self.relays[target][1])
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
        extra = [] if self.fake else ["--accept-invalid-certs"]
        result = await self.cli("connect", "--name", target, *extra, target=target, timeout=self.args.connect_timeout)
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
        state = await self.wait_state(page, lambda s: {x["id"] for x in s["list"]} >= set(TARGETS), "session list", 10)
        for target in TARGETS:
            await page.click(f"button[data-session=\"{target}\"]")
        state = await self.wait_state(page, lambda s: all(s["sessions"].get(t, {}).get("seq", 0) > 0 for t in TARGETS),
                                      "first frames of both sessions", 15)
        await self.screenshot(page, "01-session-list-and-both-sessions.png")
        listed = [{k: x.get(k) for k in ("id", "name", "host", "status", "connected_since", "last_activity")} for x in state["list"]]
        self.check("session_list_and_two_sessions_viewed", via="tailnet" if self.tailnet_url() else "loopback",
                   sessions=listed, frames={t: state["sessions"][t] for t in TARGETS})
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
            for name, path, host, headers, expected in cases:
                method = "POST" if name == "POST" else "GET"
                status, _, _ = http_request(addr, path, host=host, headers=headers, method=method)
                results.append({"address": addr.rsplit(":", 1)[0], "case": name, "status": status})
                require(status == expected, f"{name} on {addr}: HTTP {status}, expected {expected}")
        self.check("unauthorised_requests_rejected_on_every_bound_address", results=results)
        if not self.fake and not self.tailnet_url():
            self.summary["unverified"].append("rejection on the tailnet address (no tailnet address bound)")

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
        # Session b's frames end 4 s after connect (a server-ended session).
        await self.wait_state(page, lambda s: s["sessions"]["b"]["state"] == "ended", "ended banner", 10)
        await self.screenshot(page, "04-server-ended-banner.png")
        self.check("b.disconnect_shown", banner="Disconnected (server ended the session)")
        await self.cli("disconnect", "--session", "a")
        await self.wait_state(page, lambda s: s["sessions"]["a"]["state"] == "closed", "closed banner", 5)
        await self.wait_state(page, lambda s: "a" not in {x["id"] for x in s["list"]}, "list without a", 5)
        await self.screenshot(page, "05-session-closed-banner.png")
        self.check("a.close_shown_and_listed")

    # --- live mode ---------------------------------------------------------------

    async def launch_fixture(self, target):
        """A small WinForms window with a text box, driven through Cua MCP."""
        e2e, endpoint = self.e2e, self.endpoints[target]
        title = f"rdpilot-view-{self.run_id}-{target}"
        script = r"""
Add-Type -AssemblyName System.Windows.Forms
$f=New-Object Windows.Forms.Form;$f.Text=TITLE;$f.Width=900;$f.Height=300;$f.StartPosition='CenterScreen'
$t=New-Object Windows.Forms.TextBox;$t.AccessibleName='Proof input';$t.Left=20;$t.Top=30;$t.Width=820
$t.Font=New-Object Drawing.Font('Consolas',20)
$f.Controls.Add($t);[Windows.Forms.Application]::Run($f)
""".replace("TITLE", e2e.psquote(title))
        await endpoint.tool("launch_app", e2e.powershell(script))
        for _ in range(40):
            listing = e2e.structured(await endpoint.tool("list_windows"))
            for obj in e2e.objects(listing):
                if title in str(obj.get("title", obj.get("window_title", ""))):
                    wid, pid = obj.get("window_id", obj.get("hwnd", obj.get("id"))), obj.get("pid")
                    if wid is not None and pid is not None:
                        return {"pid": int(pid), "window_id": int(wid)}
            await asyncio.sleep(0.5)
        raise ProofError(f"{target}: fixture window not discoverable")

    async def type_marker(self, target, window, text):
        e2e, endpoint = self.e2e, self.endpoints[target]
        result = await endpoint.tool("get_window_state", {**window, "include_screenshot": False, "include_accessibility_tree": True})
        token = e2e.Run.element(e2e.structured(result), "Proof input")
        await endpoint.tool("type_text", {**window, "element_token": token, "text": text})

    async def live_flow(self, page):
        for target in TARGETS:
            self.endpoints[target] = await self.e2e.Mcp(self, target).start()
        window = await self.launch_fixture("a")
        await asyncio.sleep(3)
        state = await self.status(page)
        region = self.marker_region(state["sessions"]["a"]["width"], state["sessions"]["a"]["height"])
        await self.canvas_baseline(page, "a", region, "05-a-canvas-before-typing.png")
        marker = f"viewer-proof-{self.run_id}"
        await self.type_marker("a", window, marker)
        # t0 is when the Cua call returns; the change counts only once a viewer
        # frame shows the marker in the text box.
        t0 = time.time() * 1000
        latency, samples = await self.wait_visible_change(page, "a", region, MARKER_MIN_CHANGED, t0)
        await self.screenshot(page, "06-page-marker-visible.png")
        self.check("a.cua_change_visible_within_budget", latency_ms=round(latency), budget_ms=CHANGE_BUDGET_MS,
                   marker=marker, region=region, samples=samples)
        native = self.output / "06-a-native-screenshot.png"
        await self.cli("screenshot", "--session", "a", "--output", str(native))
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
        await self.screenshot(page, "07-b-server-ended-banner.png")
        self.check("b.disconnect_shown", method="RDP connection dropped", banner="Disconnected (server ended the session)")
        await self.endpoints["a"].stop()
        await self.cli("disconnect", "--session", "a")
        await self.wait_state(page, lambda s: s["sessions"]["a"]["state"] == "closed", "closed banner", 10)
        await self.wait_state(page, lambda s: "a" not in {x["id"] for x in s["list"]}, "list without a", 5)
        await self.screenshot(page, "08-a-session-closed-banner.png")
        self.check("a.close_shown_and_listed")

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
            for target in TARGETS:
                await self.connect(target)
            await self.start_viewer()
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(headless=True)
                page = await self.open_page(browser)
                if self.fake:
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
            for target in TARGETS:
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
