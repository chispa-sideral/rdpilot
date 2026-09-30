#!/usr/bin/env python3
r"""Proof run for human takeover in the live viewer (control lease).

Fake mode (offline, no Windows; fake connector with synthetic frames, a fake
Cua and a counting input sink; needs Playwright Chromium):
  cargo build --workspace
  uv run --with playwright python3 scripts/e2e/run-takeover-proof.py --fake \
    --bin-dir target/debug --output /private/takeover-fake-proof

Live mode (a Windows host with one throwaway Windows user; release builds,
as users install them; run on the tailnet host):
  cargo build --release --workspace
  export RDPILOT_TAKE_HOST=... RDPILOT_TAKE_USERNAME=... RDPILOT_TAKE_PASSWORD=...
  # optional: RDPILOT_TAKE_PORT, RDPILOT_TAKE_DOMAIN
  uv run --with playwright python3 scripts/e2e/run-takeover-proof.py \
    --bin-dir target/release --output /private/takeover-live-proof

One session, "notepad". The agent acts through rdpilot-mcp (Cua) and the
native CLI; two headless browser tabs (separate browser contexts) act as two
human viewers on the ordinary viewer URL (the tailnet URL in live mode). The
run checks, in order: Takeover and typing from tab 1 (live: into Notepad,
with a Shift combination, read back through Cua `get_window_state`); the
human-control refusal of an acting Cua call (MCP text with the `takeover`
argument) and of native input (CLI text with the takeover command, exit
code 10) while a read-only Cua call and a screenshot succeed; `tools/list`
advertising `takeover` on acting tools only; tab 2 taking over and tab 1's
loss notice; tab 2 holding Shift while the agent repeats its call with
`"takeover": true` (tab 2 shows "Taken over by the agent"; no key stays
down: the fake sink's held count drops to 0, live text arrives lowercase);
`rdpilot takeover` run exactly as printed in the native error; a tab closed
while holding the lease (control returns to the agent within the heartbeat
bound); `rdpilot list` and the session's event log with every change, its
source and reason; and `rdpilot view --read-only` with no Takeover control
and every write route answering 404 on every bound address.

Credentials come only from the environment and reach rdpilot only through
subprocess environment. The viewer token is replaced by <token> in evidence,
MCP transcripts stay in the temporary directory (they carry typed text), and
a final scan fails the run if the token, a credential, the lease ids or the
typed marker appear in any evidence file. The output directory is new and
owner-only. This harness provisions nothing: prepare the Windows host first.
"""
import argparse
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import shlex
import time
import uuid

HERE = Path(__file__).resolve().parent
SESSION = "notepad"
PANEL = f'.panel[data-session="{SESSION}"]'
# Acceptance: a tab shows why it lost the lease within about a second (the
# heartbeat runs every second).
NOTICE_BUDGET_S = 3
# Heartbeat timeout (5 s) plus one sweep interval and slack.
HEARTBEAT_BOUND_S = 8
SHIFT_H = [("down", "Shift"), ("press", "KeyH"), ("up", "Shift")]


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


viewer_proof = load("run_viewer_proof", "run-viewer-proof.py")
ProofError, require, http_request = viewer_proof.ProofError, viewer_proof.require, viewer_proof.http_request


def credentials_from_env():
    prefix = "RDPILOT_TAKE_"
    creds = {key: os.environ.get(prefix + key.upper()) for key in ("host", "port", "username", "password", "domain")}
    missing = [prefix + k.upper() for k in ("host", "username", "password") if not creds[k]]
    require(not missing, "missing environment: " + ", ".join(missing))
    return {k: v for k, v in creds.items() if v}


class Proof(viewer_proof.Proof):
    """Reuses the viewer proof's process, evidence and browser helpers."""

    def __init__(self, args):
        args.marker_region = None
        live_creds = None if args.fake else credentials_from_env()
        fake = args.fake
        args.fake = True  # the viewer proof would read its own two-target credentials
        super().__init__(args)
        args.fake = fake
        self.fake = fake
        self.targets = (SESSION,)
        self.summary["mode"] = "fake" if fake else "live"
        self.input_log = self.temp / "input.jsonl"
        if fake:
            self.env["RDPILOT_DAEMON_TEST_INPUT_LOG"] = str(self.input_log)
        else:
            self.credentials = {SESSION: live_creds}
            for key in ("RDPILOT_DAEMON_TEST_CONNECTOR", "RDPILOT_DAEMON_TEST_FRAMES", "RDPILOT_DAEMON_TEST_CUA"):
                self.env.pop(key, None)
            (self.temp / "cache").mkdir(mode=0o700, exist_ok=True)
            self.env["XDG_CACHE_HOME"] = str(self.temp / "cache")
            if args.bundle:
                self.env["RDPILOT_BUNDLE_PATH"] = str(Path(args.bundle).resolve())
            self.e2e = viewer_proof.load_cua_e2e()
        self.agent = None
        self.leases = []
        self.window = None
        self.typed = f"take{uuid.uuid4().hex[:8]}"

    # --- evidence -------------------------------------------------------------

    def scan_evidence(self):
        needles = [s.encode() for s in self.secrets()] + ([self.token.encode()] if self.token else [])
        require(len(self.leases) >= 4, f"only {len(self.leases)} lease ids captured for the scan")
        needles += [lease.encode() for lease in self.leases]
        needles.append(self.typed.encode())
        leaks = []
        for path in self.output.rglob("*"):
            if path.is_file():
                data = path.read_bytes()
                leaks += [path.name for needle in needles if needle and needle in data]
        require(not leaks, f"token, credential, lease id or typed marker found in evidence: {sorted(set(leaks))}")
        self.check("evidence_contains_no_token_credential_lease_or_typed_text", lease_ids_scanned=len(self.leases),
                   files=sum(1 for p in self.output.rglob('*') if p.is_file()))

    # --- rdpilot helpers --------------------------------------------------------

    def hosts_for_targets(self):
        if self.fake:
            return self.hosts_file({SESSION: {"host": "fake-notepad", "username": "fake"}})
        return self.hosts_file({SESSION: {
            "host": "127.0.0.1", "port": self.relays[SESSION][1], "username": self.credentials[SESSION]["username"],
            "domain": self.credentials[SESSION].get("domain"), "accept_invalid_certs": True}})

    async def raw_cli(self, *arguments, timeout=60):
        """Run the CLI without --json; returns (exit code, stdout, stderr)."""
        proc = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot"), *arguments, env=self.env,
                                                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        out, err = await asyncio.wait_for(proc.communicate(), timeout)
        return proc.returncode, out.decode(errors="replace"), err.decode(errors="replace")

    async def start_view(self, *extra):
        """Like the viewer proof's start, with extra `rdpilot view` arguments."""
        args = ["view", "--json", *extra]
        if self.args.bind:
            args += ["--bind", self.args.bind]
        stderr = open(self.output / ("view-read-only.stderr" if extra else "view.stderr"), "w")
        self.viewer = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot"), *args, env=self.env,
                                                           stdout=asyncio.subprocess.PIPE, stderr=stderr)
        stderr.close()
        text, deadline = "", time.monotonic() + 15
        while True:
            line = await asyncio.wait_for(self.viewer.stdout.readline(), max(0.1, deadline - time.monotonic()))
            require(line, "rdpilot view exited before printing URLs")
            text += line.decode()
            try:
                started = json.loads(text)
                break
            except ValueError:
                continue
        self.urls = started["urls"]
        self.token = self.urls[0].split("token=", 1)[1]
        if not self.fake and not self.tailnet_url():
            require(self.args.allow_no_tailnet, "no tailnet URL; run on the tailnet host or pass --allow-no-tailnet")
            self.summary["unverified"].append("tailnet access (no tailnet address bound)")
        self.check("viewer_started" + ("_read_only" if extra else ""), urls=[self.clean(u) for u in self.urls],
                   read_only=started.get("read_only"))

    def page_url(self):
        return self.tailnet_url() or self.urls[0]

    def page_address(self):
        """The client address the viewer sees for the tabs."""
        return self.addr(self.page_url()).rsplit(":", 1)[0]

    def api(self, path, method="GET", body=None, addr=None):
        addr = addr or self.addr(self.page_url())
        headers = {"Authorization": f"Bearer {self.token}"}
        if body is not None:
            headers.update({"Content-Type": "application/json", "Origin": f"http://{addr}",
                            "Content-Length": str(len(body))})
        ip, port = addr.rsplit(":", 1)
        import http.client
        conn = http.client.HTTPConnection(ip, int(port), timeout=20)
        try:
            conn.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
            conn.putheader("Host", addr)
            for key, value in headers.items():
                conn.putheader(key, value)
            conn.endheaders(body.encode() if body is not None else None)
            response = conn.getresponse()
            return response.status, response.read()
        finally:
            conn.close()

    def control_events(self):
        status, body = self.api(f"/api/sessions/{SESSION}/events?after=0")
        require(status == 200, f"events HTTP {status}")
        return [e for e in json.loads(body)["events"] if e["kind"].startswith("control_")]

    async def controller(self):
        listing = await self.cli("list")
        entry = next(s for s in listing if s["id"] == SESSION)
        return entry.get("controller") or {}

    def input_counts(self):
        if not self.input_log.exists():
            return {"events": 0, "held": 0}
        lines = [line for line in self.input_log.read_text().splitlines() if line.strip()]
        return json.loads(lines[-1]) if lines else {"events": 0, "held": 0}

    async def wait_input(self, predicate, what, timeout=5):
        deadline = time.monotonic() + timeout
        while not predicate(self.input_counts()):
            require(time.monotonic() < deadline, f"timed out waiting for {what}: {self.input_counts()}")
            await asyncio.sleep(0.05)
        return self.input_counts()

    # --- agent (Cua through rdpilot-mcp) ------------------------------------------

    async def start_agent(self):
        if self.fake:
            self.agent = await viewer_proof.FakeMcp(self, SESSION).start()
        else:
            self.agent = await self.e2e.Mcp(viewer_proof.McpRun(self), SESSION).start()

    def tool_args(self, name, arguments):
        args = dict(arguments)
        if not self.fake and "session" in self.agent.tools.get(name, {}).get("inputSchema", {}).get("properties", {}):
            args["session"] = self.agent.label
        return args

    async def call(self, name, arguments):
        """One raw tools/call; returns the result (isError included)."""
        return await self.agent.request("tools/call", {"name": name, "arguments": self.tool_args(name, arguments)})

    def acting_call(self):
        """The acting call the agent repeats: (tool, arguments)."""
        if self.fake:
            return "echo", {"text": viewer_proof.ARGUMENT_MARKER}
        return "type_text", {**self.window, "text": "agent"}

    def read_only_call(self):
        return ("list_windows", {}) if self.fake else ("get_window_state", {**self.window, "include_screenshot": False,
                                                                           "include_accessibility_tree": True})

    @staticmethod
    def result_text(result):
        return " ".join(c.get("text", "") for c in result.get("content", []) if c.get("type") == "text")

    # --- live Notepad -----------------------------------------------------------

    async def launch_notepad(self):
        e2e = self.e2e
        await self.call("launch_app", {"path": r"C:\Windows\System32\notepad.exe"})
        for _ in range(60):
            listing = e2e.structured(await self.call("list_windows", {}))
            for obj in e2e.objects(listing):
                title = str(obj.get("title", obj.get("window_title", "")))
                if "Notepad" in title:
                    wid, pid = obj.get("window_id", obj.get("hwnd", obj.get("id"))), obj.get("pid")
                    if wid is not None and pid is not None:
                        self.window = {"pid": int(pid), "window_id": int(wid)}
                        self.check("notepad_launched_by_the_agent", tool="launch_app")
                        return
            await asyncio.sleep(0.5)
        raise ProofError("Notepad window not discoverable")

    async def notepad_text(self):
        """The Notepad accessibility state as JSON text (never evidence)."""
        name, args = self.read_only_call()
        result = await self.call(name, args)
        require(not result.get("isError"), f"{name} refused: {self.result_text(result)[:300]}")
        return json.dumps(self.e2e.structured(result))

    async def wait_text(self, needle, what, timeout=15):
        deadline = time.monotonic() + timeout
        while True:
            state = await self.notepad_text()
            if needle in state:
                return
            if time.monotonic() >= deadline:
                # Structure only (keys and roles), never values: for diagnosis.
                editor = self.editor(state)
                self.summary["editor_keys"] = sorted(editor.keys())
                self.save()
                raise ProofError(f"Notepad does not show {what}")
            await asyncio.sleep(0.5)

    def editor(self, state_text):
        """Notepad's text area: element token and screen frame."""
        state = json.loads(state_text)
        editors = [o for o in self.e2e.objects(state)
                   if "element_token" in o and o.get("role") == "Edit" and o.get("label") == "Text Editor"]
        require(editors, "Notepad text area not in the accessibility tree")
        return editors[0]

    # --- browser ----------------------------------------------------------------

    async def open_tab(self, browser, name):
        context = await browser.new_context(viewport={"width": 1600, "height": 1000})
        page = await context.new_page()
        page.on("response", lambda response: asyncio.ensure_future(self.grab_lease(response)))
        await page.goto(self.page_url())
        await self.wait_state(page, lambda s: SESSION in {x["id"] for x in s["list"]}, f"{name}: session list", 10)
        await page.click(f'button[data-session="{SESSION}"]')
        await self.wait_state(page, lambda s: s["sessions"].get(SESSION, {}).get("seq", 0) > 0, f"{name}: frames", 20)
        state = await self.status(page)
        require(state["sessions"][SESSION]["control"]["held"] is False, f"{name}: viewing took the lease")
        return page

    async def control_state(self, page):
        return (await self.status(page))["sessions"][SESSION]["control"]

    async def take(self, page, name):
        await page.click(f"{PANEL} .takeover")
        state = await self.wait_state(page, lambda s: s["sessions"][SESSION]["control"]["held"], f"{name}: take", 15)
        button = await page.text_content(f"{PANEL} .takeover")
        framed = await page.get_attribute(PANEL, "class")
        line = await page.text_content(f"{PANEL} .control .controller")
        require(button == "Release", f"{name}: button reads {button}")
        require("holding" in framed, f"{name}: panel not framed")
        require(line == "You control this session", f"{name}: controller line {line}")
        return state

    async def grab_lease(self, response):
        """Keep each granted lease id, only to prove it reaches no evidence."""
        if response.url.endswith("/control") and response.status == 200:
            try:
                lease = (await response.json()).get("lease")
            except Exception:  # noqa: BLE001 - a body that is not JSON has no lease
                return
            if lease:
                self.leases.append(lease)

    async def wait_notice(self, page, name, expected):
        started = time.monotonic()
        state = await self.wait_state(
            page, lambda s: not s["sessions"][SESSION]["control"]["held"]
            and s["sessions"][SESSION]["control"]["notice"] == expected, f"{name}: notice {expected!r}", NOTICE_BUDGET_S)
        button = await page.text_content(f"{PANEL} .takeover")
        require(button == "Takeover", f"{name}: button reads {button} after the loss")
        return round((time.monotonic() - started) * 1000), state

    async def keys(self, page, steps):
        for op, key in steps:
            await getattr(page.keyboard, op)(key)

    async def click_into(self, page):
        """Click into the session: the canvas centre (live: Notepad's text area)."""
        box = await page.locator(f"{PANEL} canvas").bounding_box()
        x, y = box["width"] / 2, box["height"] / 2
        if not self.fake:
            frame = self.editor(await self.notepad_text())["frame"]
            width = await page.evaluate("(sel) => document.querySelector(sel).width", f"{PANEL} canvas")
            scale = box["width"] / width
            x, y = (frame["x"] + frame["w"] / 2) * scale, (frame["y"] + min(frame["h"] / 2, 40)) * scale
        await page.mouse.click(box["x"] + x, box["y"] + y)
        # Like a person: the guest moves the keyboard focus on the click
        # before the next keys arrive.
        await asyncio.sleep(0.2 if self.fake else 1.0)

    # --- the flow ---------------------------------------------------------------

    async def flow(self, browser):
        address = self.page_address()
        human = f"human viewer {address}"
        await self.start_agent()
        if not self.fake:
            await self.launch_notepad()

        # tools/list: takeover on acting tools only.
        tools = {t["name"]: t for t in (await self.agent.request("tools/list", {}))["tools"]}
        with_takeover = sorted(n for n, t in tools.items() if "takeover" in t.get("inputSchema", {}).get("properties", {}))
        without = sorted(n for n in tools if n not in with_takeover)
        acting_name, acting_args = self.acting_call()
        read_name, read_args = self.read_only_call()
        require(acting_name in with_takeover and read_name in without, f"schema patch: {with_takeover} / {without}")
        prop = tools[acting_name]["inputSchema"]["properties"]["takeover"]
        require(prop.get("type") == "boolean" and prop.get("default") is False, f"takeover schema {prop}")
        self.check("tools_list_advertises_takeover_on_acting_tools_only",
                   acting=with_takeover if self.fake else len(with_takeover), read_only=without if self.fake else len(without))

        # 1. Tab 1 takes over from the agent and types.
        tab1 = await self.open_tab(browser, "tab 1")
        await self.take(tab1, "tab 1")
        await self.screenshot(tab1, "01-tab1-holds-the-lease.png")
        await self.click_into(tab1)
        await self.keys(tab1, SHIFT_H)
        await tab1.keyboard.type(self.typed)
        if self.fake:
            counts = await self.wait_input(lambda c: c["events"] >= 5 and c["held"] == 0, "typed input")
            typed = {"events_taken": counts["events"], "held_after": counts["held"]}
        else:
            await asyncio.sleep(1.5)
            await self.screenshot(tab1, "01b-tab1-typed.png")
            await self.wait_text("H" + self.typed, "the typed text with its Shift letter")
            typed = {"shift_combination": "Shift+H", "read_back_through": read_name}
        self.check("tab1_takeover_and_typing", controller_line="You control this session", **typed)
        controller = await self.controller()
        require(controller.get("kind") == "human" and controller.get("address") == address, f"list controller {controller}")

        # 2. During the lease: agent refusals, reads succeed.
        result = await self.call(acting_name, acting_args)
        text = self.result_text(result)
        require(result.get("isError") is True, f"acting call not refused: {text[:200]}")
        require(text.startswith(f'session "{SESSION}" is controlled by {human} since ') and
                text.endswith('; wait and retry, or repeat this call with "takeover": true to take control'),
                f"MCP refusal text: {text}")
        read = await self.call(read_name, read_args)
        require(not read.get("isError"), f"read-only call refused: {self.result_text(read)[:200]}")
        code, _, err = await self.raw_cli("input", "key", "--session", SESSION, "--combo", "enter")
        require(code == 10, f"native key exit {code}: {err}")
        marker = "or take over with: "
        require(f"is controlled by {human} since " in err and marker in err, f"native refusal: {err}")
        cli_message = err.strip()
        command = cli_message.rsplit(marker, 1)[1]
        require(command == f"rdpilot takeover --session {SESSION}", f"printed command {command}")
        code, _, err = await self.raw_cli("screenshot", "--session", SESSION, "--output", str(self.temp / "shot.png"))
        require(code == 0, f"screenshot during the lease: {err}")
        self.check("agent_refused_during_the_lease", mcp_message=text, cli_exit_code=10,
                   cli_message=cli_message,
                   read_only_call=read_name, screenshot="ok")

        # 3. Tab 2 takes over; tab 1 is told who.
        tab2 = await self.open_tab(browser, "tab 2")
        await self.take(tab2, "tab 2")
        latency, _ = await self.wait_notice(tab1, "tab 1", f"Taken over by {human}")
        await self.screenshot(tab1, "02-tab1-taken-over-by-tab2.png")
        await self.click_into(tab2)
        await tab2.keyboard.type("x")
        self.check("tab2_takeover_and_tab1_notice", notice=f"Taken over by {human}", notice_latency_ms=latency)

        # 4. Tab 2 holds Shift; the agent takes over with the argument.
        before = self.input_counts()["events"] if self.fake else None
        await tab2.keyboard.down("Shift")
        if self.fake:
            await self.wait_input(lambda c: c["held"] == 1 and c["events"] > before, "Shift held")
        else:
            await asyncio.sleep(1)
        if self.fake:
            args = {**acting_args, "takeover": True}
        else:
            element = self.editor(await self.notepad_text())["element_token"]
            args = {**self.window, "element_token": element, "text": "lower" + self.typed, "takeover": True}
        result = await self.call(acting_name, args)
        require(not result.get("isError"), f"takeover call failed: {self.result_text(result)[:300]}")
        latency, _ = await self.wait_notice(tab2, "tab 2", "Taken over by the agent")
        await self.screenshot(tab2, "03-tab2-taken-over-by-the-agent.png")
        await tab2.keyboard.up("Shift")
        if self.fake:
            counts = await self.wait_input(lambda c: c["held"] == 0, "held keys released")
            # The fake Cua answers isError to any call that still carries it.
            stuck = {"held_after_takeover": counts["held"], "takeover_stripped_before_cua": True}
        else:
            await self.wait_text("lower" + self.typed, "the agent's lowercase text")
            # Scancode keys (not Unicode text) show a stuck Shift: q, z, x
            # arrive as "qzx" only when no Shift is down in the guest.
            for key in ("q", "z", "x"):
                code, _, err = await self.raw_cli("input", "key", "--session", SESSION, "--combo", key)
                require(code == 0, f"native key after takeover: {err}")
            await self.wait_text(self.typed + "qzx", "the native scancode keys in lowercase")
            state = await self.notepad_text()
            require("QZX" not in state and "LOWER" not in state, "a stuck Shift changed the text")
            stuck = {"agent_text_lowercase": True, "native_scancode_keys_lowercase": "qzx"}
        self.check("agent_takeover_through_mcp", notice="Taken over by the agent", notice_latency_ms=latency, **stuck)

        # 5. The printed CLI command takes over.
        await self.take(tab1, "tab 1")
        code, out, err = await self.raw_cli(*shlex.split(command)[1:])
        require(code == 0 and out.startswith(f"control of {SESSION} returned to the agent (was {human} since "),
                f"rdpilot takeover: {code} {out} {err}")
        latency, _ = await self.wait_notice(tab1, "tab 1", "Taken over by the agent")
        code, out, _ = await self.raw_cli(*shlex.split(command)[1:])
        require(code == 0 and "nothing changed" in out, f"second takeover: {out}")
        self.check("cli_takeover_as_printed", command=command, notice_latency_ms=latency, second_run="no change")

        # 6. A tab closed while holding the lease: control returns.
        await self.take(tab1, "tab 1")
        started = time.monotonic()
        await tab1.context.close()
        while (await self.controller()).get("kind") != "agent":
            require(time.monotonic() - started < HEARTBEAT_BOUND_S, "lease survived the closed tab")
            await asyncio.sleep(0.2)
        ended_after = round((time.monotonic() - started) * 1000)
        if self.fake:
            result = await self.call(acting_name, acting_args)
        else:
            element = self.editor(await self.notepad_text())["element_token"]
            result = await self.call(acting_name, {**self.window, "element_token": element, "text": "after"})
        require(not result.get("isError"), "agent call after the closed tab")
        self.check("closed_tab_returns_control", within_ms=ended_after, bound_s=HEARTBEAT_BOUND_S)

        # 7. Status, log and strip show every change with source and reason.
        events = self.control_events()
        summary = [{"kind": e["kind"], "source": e["source"],
                    **{k: e[k] for k in ("from", "by", "reason") if k in e}} for e in events]
        kinds = [(e["kind"], e["source"]) for e in events]
        expected = [("control_taken", "viewer"), ("control_taken_over", "viewer"), ("control_taken_over", "cua"),
                    ("control_taken", "viewer"), ("control_taken_over", "cli"), ("control_taken", "viewer")]
        require(kinds[:6] == expected, f"control events {kinds}")
        last = kinds[6] if len(kinds) > 6 else None
        require(last in (("control_released", "viewer"), ("control_ended", "daemon")), f"closed-tab event {last}")
        require(not any(k in json.dumps(events) for k in ('"x"', '"code"', '"lease"')), "input content in events")
        strip = await tab2.evaluate("""() => Array.from(document.querySelectorAll('%s .strip li[data-kind="marker"]'))
            .map((li) => li.textContent)""" % PANEL)
        require(any("taken over from" in s and "by the agent" in s and "cua" in s for s in strip), f"strip {strip}")
        require(any("taken over from" in s and "by the agent" in s and "cli" in s for s in strip), f"strip {strip}")
        controller = await self.controller()
        require(controller.get("kind") == "agent", f"list controller {controller}")
        await self.screenshot(tab2, "04-activity-strip-with-control-changes.png")
        self.write("control-events.json", json.dumps(summary, indent=2))
        self.check("status_log_and_strip_show_every_change", events=summary, list_controller="agent")
        await tab2.context.close()

    async def read_only_flow(self, browser):
        await self.stop_viewer()
        await self.start_view("--read-only")
        page = await self.open_tab(browser, "read-only tab")
        takeover_visible = await page.is_visible(f"{PANEL} .takeover")
        recording_visible = await page.is_visible(f"{PANEL} .record-toggle")
        require(not takeover_visible and not recording_visible,
                f"read-only page offers write controls: takeover {takeover_visible}, recording {recording_visible}")
        await self.screenshot(page, "05-read-only-viewer.png")
        statuses = {}
        for url in self.urls:
            addr = self.addr(url)
            for path, body in ((f"/api/sessions/{SESSION}/control", '{"action":"take"}'),
                               (f"/api/sessions/{SESSION}/input", '{"lease":"x","generation":0,"width":1,"height":1,"events":[]}'),
                               (f"/api/sessions/{SESSION}/recording", '{"action":"start"}')):
                status, _ = self.api(path, "POST", body, addr=addr)
                statuses[f"{addr.rsplit(':', 1)[0]} {path.rsplit('/', 1)[1]}"] = status
        require(all(s == 404 for s in statuses.values()), f"read-only write routes: {statuses}")
        require((await self.controller()).get("kind") == "agent", "read-only viewer changed control")
        self.check("read_only_viewer_has_no_takeover_and_no_write_route", statuses=statuses)
        await page.context.close()

    async def execute(self):
        from playwright.async_api import async_playwright

        daemon_log = open(self.output / "daemon.log", "w")
        try:
            if not self.fake:
                relay = self.e2e.Relay(self.credentials[SESSION])
                self.relays[SESSION] = (relay, await relay.start())
            self.daemon = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot-daemon"), env=self.env,
                                                               stdout=daemon_log, stderr=daemon_log)
            await asyncio.sleep(0.3)
            await self.connect(SESSION)
            await self.start_view()
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(headless=True)
                await self.flow(browser)
                await self.read_only_flow(browser)
                await browser.close()
            await self.stop_viewer()
            self.summary["status"] = "passed"
        except BaseException as error:
            self.summary["status"] = "failed"
            self.summary["failure"] = self.clean(f"{type(error).__name__}: {error}")
            raise
        finally:
            if self.agent:
                await self.agent.stop()
            if self.viewer and self.viewer.returncode is None:
                self.viewer.kill()
                await self.viewer.wait()
            try:
                await self.cli("disconnect", "--session", SESSION, timeout=15, allow_failure=True)
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
            for path in self.output.glob("*.stderr"):
                path.write_text(self.clean(path.read_text(errors="replace")))
            log = self.output / "daemon.log"
            if log.exists():
                log.write_text(self.clean(log.read_text(errors="replace")))
            self.save()
            import shutil
            shutil.rmtree(self.temp, ignore_errors=True)
        self.scan_evidence()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fake", action="store_true", help="offline run against the fake connector")
    parser.add_argument("--bin-dir", required=True, help="directory with rdpilot, rdpilot-daemon, rdpilot-mcp")
    parser.add_argument("--bundle", help="live mode: daemon bundle_path directory (default: download into a fresh cache)")
    parser.add_argument("--output", required=True, help="new evidence directory; must not already exist")
    parser.add_argument("--bind", choices=["loopback", "loopback+tailnet"],
                        help="viewer bind set (default: loopback in fake mode, the viewer's default in live mode)")
    parser.add_argument("--allow-no-tailnet", action="store_true", help="live mode: run without a tailnet URL")
    parser.add_argument("--allow-debug", action="store_true", help="live mode: allow debug binaries")
    parser.add_argument("--connect-timeout", type=int, default=600)
    args = parser.parse_args()
    if args.fake and not args.bind:
        args.bind = "loopback"
    if not args.fake and "debug" in Path(args.bin_dir).resolve().parts and not args.allow_debug:
        parser.error("live mode needs release binaries (cargo build --release --workspace)")
    os.umask(0o077)
    try:
        proof = Proof(args)
    except (ProofError, OSError) as error:
        print(f"Takeover proof not started: {error}", flush=True)
        return 2
    try:
        asyncio.run(proof.execute())
    except (Exception, KeyboardInterrupt) as error:
        print(proof.clean(f"Takeover proof failed: {type(error).__name__}: {error}"), flush=True)
        return 1
    print(f"Evidence: {proof.output / 'summary.json'}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
