#!/usr/bin/env python3
r"""Proof run for session recording (record, annotate, keep, replay).

Fake mode (offline, no Windows; fake connector with synthetic frames and a
fake Cua; needs ffmpeg, ffprobe and Playwright Chromium and Firefox):
  cargo build --workspace
  uv run --with playwright python3 scripts/e2e/run-recording-proof.py --fake \
    --bin-dir target/debug --output /private/recording-fake-proof

Live mode (a small CrabBox Azure Windows lease with three independent
Windows users; release builds, as users install them):
  cargo build --release --workspace
  export RDPILOT_REC_{A,B,C}_HOST=... RDPILOT_REC_{A,B,C}_USERNAME=... RDPILOT_REC_{A,B,C}_PASSWORD=...
  # optional: RDPILOT_REC_{A,B,C}_PORT, RDPILOT_REC_{A,B,C}_DOMAIN
  uv run --with playwright --with pillow --with numpy python3 scripts/e2e/run-recording-proof.py \
    --bin-dir target/release --output /private/recording-live-proof

In live mode the daemon starts with an empty cache under the run's temporary
directory and downloads the bridge and the Cua driver; --bundle DIR sets its
bundle_path instead (for example a bridge built from this source).

Sessions: a records from connect (fake: `--record`; live: a
`[[recording.hosts]]` entry for its alias), b connects with `--no-record`
and its recording is started from the viewer page, c is not recorded (live)
or shows one change and then a still display (fake). Viewer actions go
through the page UI in a headless browser: Start recording, the annotation
input and the Keep toggle.

Live setup (before the first logon of the three users): the taskbar clock
must be hidden for them, or its minute tick is a display change in the
still interval, and the text caret must not blink, or Notepad's caret is
one. For example, as an administrator on the guest, load
C:\Users\Default\NTUSER.DAT and set
Software\Microsoft\Windows\CurrentVersion\Policies\Explorer HideClock=1
(REG_DWORD) and "Control Panel\Desktop" CursorBlinkRate=-1 (REG_SZ) in it,
so every new profile starts without the clock and with a steady caret.

Credentials come only from the environment and reach rdpilot only through
subprocess environment. Recordings are written to the temp directory and
deleted at the end; the evidence directory (new, owner-only) holds listings,
event-log excerpts, figures, crops and browser captures. The viewer token is
replaced by <token>, and a final scan fails the run if the token, a
credential or the argument marker appears in any evidence file. This
harness provisions nothing: lease the Windows machine with the
crabbox-azure-windows skill first.
"""
import argparse
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

HERE = Path(__file__).resolve().parent
LIVE_TARGETS = ("a", "b", "c")
FAKE_TARGETS = ("a", "b", "c")
# The live proof waits this long with a still display on a.
IDLE_S = 160
# The still interval starts this long after the page annotation: the Cua
# agent cursor overlay fades out about 20 s after the last Cua action.
SETTLE_MS = 35000
# A replayed frame must show the typed marker within this time after the
# typing call finished.
LEGIBLE_WITHIN_MS = 2000
# One pacing interval at the default 4 fps.
FRAME_INTERVAL_MS = 250
STRIP_KINDS = {"call_started", "call_finished", "cua_attached", "cua_detached", "session_ended"}

STRIP_JS = """
(sel) => Array.from(document.querySelectorAll(sel + " .strip li")).map((li) => ({
  kind: li.dataset.kind, seq: Number(li.dataset.seq), outcome: li.dataset.outcome || null,
  text: Array.from(li.querySelectorAll("span")).map((s) => s.textContent).join(" "),
}))
"""
REPLAY_JS = """
() => {
  const st = window.rdpilotViewer.replay;
  const v = document.querySelector(".panel.replay video");
  return st ? Object.assign(JSON.parse(JSON.stringify(st)), {
    video_time: v ? v.currentTime : null, video_paused: v ? v.paused : null,
    video_ready: v ? v.readyState : null, video_error: v && v.error ? v.error.code : null,
    logs_current: Array.from(document.querySelectorAll(".panel.replay .logs li.current")).map((li) => Number(li.dataset.seq)),
    logs_total: document.querySelectorAll(".panel.replay .logs li").length,
  }) : null;
}
"""


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


viewer_proof = load("run_viewer_proof", "run-viewer-proof.py")
ProofError, require, http_request = viewer_proof.ProofError, viewer_proof.require, viewer_proof.http_request
ARGUMENT_MARKER = viewer_proof.ARGUMENT_MARKER
LIVE_FAILING_TOOL = viewer_proof.LIVE_FAILING_TOOL


def credentials_from_env(target):
    prefix = f"RDPILOT_REC_{target.upper()}_"
    creds = {key: os.environ.get(prefix + key.upper()) for key in ("host", "port", "username", "password", "domain")}
    missing = [prefix + k.upper() for k in ("host", "username", "password") if not creds[k]]
    require(not missing, "missing environment: " + ", ".join(missing))
    return {k: v for k, v in creds.items() if v}


def mode_string(path):
    return oct(path.stat().st_mode & 0o777)


class Proof(viewer_proof.Proof):
    """Reuses the viewer proof's process, evidence and browser helpers."""

    def __init__(self, args):
        args.bind = args.bind or "loopback"
        args.marker_region = None
        args.allow_no_tailnet = True
        if args.fake:
            super().__init__(args)
        else:
            # Credentials of three users, read before anything is created.
            creds = {t: credentials_from_env(t) for t in LIVE_TARGETS}
            args.fake = True  # skip the viewer proof's two-target credential read
            super().__init__(args)
            args.fake = False
            self.fake = False
            self.credentials = creds
            self.summary["mode"] = "live"
            self.env.pop("RDPILOT_DAEMON_TEST_CONNECTOR", None)
            self.env.pop("RDPILOT_DAEMON_TEST_FRAMES", None)
            self.env.pop("RDPILOT_DAEMON_TEST_CUA", None)
            (self.temp / "cache").mkdir(mode=0o700, exist_ok=True)
            self.env["XDG_CACHE_HOME"] = str(self.temp / "cache")
            if args.bundle:
                self.env["RDPILOT_BUNDLE_PATH"] = str(Path(args.bundle).resolve())
            self.e2e = load_cua()
        self.targets = FAKE_TARGETS if self.fake else LIVE_TARGETS
        self.recordings_dir = self.temp / "recordings"
        self.env["RDPILOT_RECORDING__DIR"] = str(self.recordings_dir)
        config = self.temp / "config" / "rdpilot"
        config.mkdir(mode=0o700, parents=True, exist_ok=True)
        # Live: a records through the per-host entry keyed on its alias.
        (config / "config.toml").write_text("" if self.fake else '[[recording.hosts]]\nhost = "a"\nenabled = true\n')
        self.ids = {}
        self.page_errors = []

    # --- rdpilot helpers ------------------------------------------------------

    def hosts_for_targets(self):
        if self.fake:
            hosts = {"a": "fake-a", "b": "fake-b", "c": "fake-still"}
            return self.hosts_file({t: {"host": hosts[t], "username": "fake"} for t in FAKE_TARGETS})
        return self.hosts_file({
            t: {"host": "127.0.0.1", "port": self.relays[t][1], "username": self.credentials[t]["username"],
                "domain": self.credentials[t].get("domain"), "accept_invalid_certs": True}
            for t in LIVE_TARGETS})

    async def connect_with(self, target, *flags):
        hosts = self.hosts_for_targets()
        result = await self.cli("connect", target, "-F", str(hosts), "--name", target, *flags, target=target,
                                timeout=self.args.connect_timeout)
        require(result.get("session") == target, f"connect {target}: {result}")
        if not self.fake and "CuaEnabled=no" not in flags:
            require(result.get("bridge_live"), f"{target}: Cua bridge not live")
        state = result.get("recording", {})
        if state.get("state") == "on":
            self.ids[target] = state["id"]
        self.check(f"{target}.connected", recording=state.get("state"),
                   how=" ".join(flags) or ("per-host entry" if target == "a" and not self.fake else "config"))
        return state

    async def recordings(self):
        return await self.cli("recording", "list")

    async def wait_finished(self, ids, timeout=60):
        deadline = time.monotonic() + timeout
        while True:
            listing = await self.recordings()
            active = [r["id"] for r in listing["recordings"] if r["id"] in ids and r["active"]]
            if not active:
                return listing
            require(time.monotonic() < deadline, f"recordings still active: {active}")
            await asyncio.sleep(0.5)

    async def wait_segments(self, target, count, timeout=15):
        deadline = time.monotonic() + timeout
        while True:
            started = [e for e in self.events(target) if e["kind"] == "segment_started"]
            if len(started) >= count:
                return
            require(time.monotonic() < deadline, f"{target}: {len(started)} segment(s) after {timeout} s, want {count}")
            await asyncio.sleep(0.2)

    def rec_dir(self, target):
        return self.recordings_dir / self.ids[target]

    def manifest(self, target):
        return json.loads((self.rec_dir(target) / "manifest.json").read_text())

    def events(self, target):
        out = []
        for line in (self.rec_dir(target) / "events.jsonl").read_text().splitlines():
            if line.strip():
                out.append(json.loads(line))
        return out

    # --- page helpers ---------------------------------------------------------

    async def open_viewer_page(self, browser):
        page = await browser.new_page(viewport={"width": 1600, "height": 1100})
        page.on("console", lambda m: self.page_errors.append(m.text) if m.type == "error" else None)
        await page.goto(self.urls[0])
        await self.wait_state(page, lambda s: {x["id"] for x in s["list"]} >= {"a", "b"}, "session list", 10)
        return page

    async def open_panel(self, page, target):
        await page.click(f'button[data-session="{target}"]')
        await self.wait_state(page, lambda s: target in s["sessions"], f"panel {target}", 10)

    async def panel_message(self, page, target):
        return await page.text_content(f'.panel[data-session="{target}"] .rec .msg')

    async def start_from_page(self, page, target):
        panel = f'.panel[data-session="{target}"]'
        await page.click(f"{panel} .record-toggle")
        state = await self.wait_state(page, lambda s: s["sessions"][target].get("recording"),
                                      f"{target} recording indicator", 10)
        rid = state["sessions"][target]["recording"]
        self.ids[target] = rid
        indicator = await page.text_content(f"{panel} .rec span")
        require(rid in indicator, f"{target}: indicator {indicator}")
        return rid

    async def annotate_from_page(self, page, target, text):
        panel = f'.panel[data-session="{target}"]'
        await page.fill(f"{panel} .annotation-input", text)
        await page.click(f"{panel} .annotation-add")
        deadline = time.monotonic() + 10
        while "annotation added" not in (await self.panel_message(page, target) or ""):
            require(time.monotonic() < deadline, f"{target}: annotation not confirmed: {await self.panel_message(page, target)}")
            await asyncio.sleep(0.1)

    async def keep_from_page(self, page, rid):
        if not await page.is_visible("#recordings-view"):
            await page.click("#recordings-button")
        row = f'#recordings tr[data-recording="{rid}"]'
        await page.wait_for_selector(row, timeout=10000)
        await page.click(f"{row} .keep-toggle")
        deadline = time.monotonic() + 10
        while (await page.text_content(f"{row} td:nth-child(8)")) != "kept":
            require(time.monotonic() < deadline, f"{rid}: keep not shown")
            await asyncio.sleep(0.2)
            if not await page.is_visible(row):
                await page.wait_for_selector(row, timeout=5000)

    # --- disk checks ----------------------------------------------------------

    def listing(self, target):
        root = self.rec_dir(target)
        rows = []
        for path in sorted([root, *root.rglob("*")]):
            rows.append({"path": str(path.relative_to(self.recordings_dir)), "mode": mode_string(path),
                         "bytes": path.stat().st_size if path.is_file() else None})
            if path.is_dir():
                require(path.stat().st_mode & 0o777 == 0o700, f"{path.name}: directory not 0700")
            else:
                require(path.stat().st_mode & 0o777 == 0o600, f"{path.name}: file not 0600")
        return rows

    def probe_segments(self, target):
        """Every closed segment decodes; codec, container and frame count."""
        manifest = self.manifest(target)
        results = []
        for seg in manifest["segments"]:
            path = self.rec_dir(target) / "segments" / seg["file"]
            decode = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-f", "null", "-"],
                                    capture_output=True, text=True, timeout=120)
            require(decode.returncode == 0 and not decode.stderr.strip(),
                    f"{target} {seg['file']}: decode errors: {decode.stderr[:500]}")
            probe = json.loads(subprocess.run(
                ["ffprobe", "-v", "error", "-count_frames", "-show_entries",
                 "format=format_name:stream=codec_name,nb_read_frames,width,height,color_range,color_space",
                 "-of", "json", str(path)], capture_output=True, text=True, timeout=120, check=True).stdout)
            stream = probe["streams"][0]
            require(stream["codec_name"] == "av1", f"{seg['file']}: codec {stream['codec_name']}")
            require("webm" in probe["format"]["format_name"], f"{seg['file']}: container {probe['format']}")
            require(int(stream["nb_read_frames"]) == seg["frames"],
                    f"{seg['file']}: {stream['nb_read_frames']} frames decoded, manifest {seg['frames']}")
            results.append({"file": seg["file"], "codec": stream["codec_name"], "container": probe["format"]["format_name"],
                            "frames": seg["frames"], "bytes": seg["bytes"], "size": [stream["width"], stream["height"]],
                            "colour": [stream.get("color_range"), stream.get("color_space")],
                            "start_offset_ms": seg["start_offset_ms"], "end_offset_ms": seg["end_offset_ms"]})
        return results

    def packet_offsets(self, target):
        """Recording-timeline offsets (ms) of every video frame on disk."""
        offsets = []
        for seg in self.manifest(target)["segments"]:
            path = self.rec_dir(target) / "segments" / seg["file"]
            out = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries",
                                  "packet=pts_time", "-of", "csv=p=0", str(path)],
                                 capture_output=True, text=True, timeout=120, check=True).stdout
            offsets += [seg["start_offset_ms"] + round(float(x) * 1000) for x in out.split() if x.strip()]
        return sorted(offsets)

    def excerpt(self, events):
        keep = ("seq", "at", "offset_ms", "source", "kind", "name", "outcome", "duration_ms", "reason",
                "trigger", "text", "segment", "frames", "bytes", "count")
        return [{k: e[k] for k in keep if k in e} for e in events]

    # --- replay in a browser --------------------------------------------------

    async def replay_state(self, page):
        return await page.evaluate(REPLAY_JS)

    async def wait_replay(self, page, predicate, what, timeout=15):
        deadline = time.monotonic() + timeout
        while True:
            state = await self.replay_state(page)
            if state and predicate(state):
                return state
            require(time.monotonic() < deadline, f"replay: timed out waiting for {what}: {state}")
            await asyncio.sleep(0.05)

    async def open_replay(self, page, rid):
        if not await page.is_visible("#recordings-view"):
            await page.click("#recordings-button")
        row = f'#recordings tr[data-recording="{rid}"]'
        await page.wait_for_selector(row, timeout=10000)
        await page.click(f"{row} .open-recording")
        return await self.wait_replay(page, lambda s: s["id"] == rid and s["loaded"], f"{rid} loaded")

    async def seek_timeline(self, page, ms):
        await page.evaluate("""(ms) => { const t = document.querySelector('.panel.replay .timeline');
            t.value = String(ms); t.dispatchEvent(new Event('input')); }""", int(ms))

    async def select_log(self, page, seq):
        await page.click(f'.panel.replay .logs li[data-seq="{seq}"]')

    async def replay_checks(self, browser_name, page, target, events):
        """Play, seek, strip/logs up to the position, event selection."""
        rid = self.ids[target]
        state = await self.open_replay(page, rid)
        manifest = self.manifest(target)
        require(state["logs_total"] == len(events), f"{browser_name}: logs pane shows {state['logs_total']} of {len(events)} events")
        segs = manifest["segments"]
        require(segs, f"{target}: no segments")
        # Play from a segment start and see the position and video move.
        await self.seek_timeline(page, str(segs[0]["start_offset_ms"]))
        await page.click(".panel.replay .play")
        t0 = await self.wait_replay(page, lambda s: s["playing"] and s["video_time"] is not None, "playing")
        await asyncio.sleep(1.5)
        t1 = await self.replay_state(page)
        require(t1["position_ms"] > t0["position_ms"] + 500, f"{browser_name}: position did not advance: {t0['position_ms']} -> {t1['position_ms']}")
        require(t1["video_error"] is None, f"{browser_name}: video error {t1['video_error']}")
        await page.click(".panel.replay .play")  # pause
        await self.wait_replay(page, lambda s: not s["playing"] and s["video_paused"], "paused")
        crossings = await self.paused_segment_seeks(browser_name, page, target, segs)
        # Selecting each call_finished and annotation seeks to its offset.
        picks = [e for e in events if e["kind"] in ("call_finished", "annotation")]
        results = []
        for e in picks:
            same = {x["seq"] for x in events if x["offset_ms"] == e["offset_ms"]}
            await self.select_log(page, e["seq"])
            st = await self.wait_replay(page, lambda s: abs(s["position_ms"] - e["offset_ms"]) <= 1 and
                                        s["highlighted_seq"] in same, f"seek to {e['seq']}")
            seg = next((s for s in segs if s["start_offset_ms"] <= e["offset_ms"] < s["end_offset_ms"]), None)
            video_ms = None
            if seg:
                st = await self.wait_replay(
                    page, lambda s: s["video_time"] is not None and s["segment"] == int(seg["file"].split(".")[0]) and
                    abs(seg["start_offset_ms"] + s["video_time"] * 1000 - e["offset_ms"]) <= FRAME_INTERVAL_MS,
                    f"video at {e['offset_ms']}")
                video_ms = round(seg["start_offset_ms"] + st["video_time"] * 1000)
            rows = await page.evaluate(STRIP_JS, ".panel.replay")
            want = [x for x in events if x["kind"] in STRIP_KINDS and x["offset_ms"] <= e["offset_ms"]]
            names = " | ".join(r["text"] for r in rows)
            require("Older events dropped" not in names, f"{browser_name}: dropped marker in replay")
            later = [x for x in events if x["kind"] == "call_started" and x["offset_ms"] > e["offset_ms"]]
            for x in later:
                require(not any(r["kind"] == "call" and r["seq"] == x["seq"] for r in rows),
                        f"{browser_name}: strip shows a call after the position")
            for x in want:
                if x["kind"] == "call_started":
                    require(any(r["seq"] == x["seq"] for r in rows), f"{browser_name}: strip misses call {x['seq']}")
            require(len(st["logs_current"]) == 1 and st["logs_current"][0] in same,
                    f"{browser_name}: logs highlight {st['logs_current']}")
            results.append({"seq": e["seq"], "kind": e["kind"], "offset_ms": e["offset_ms"], "video_ms": video_ms,
                            "strip_rows": len(rows)})
        await self.screenshot(page, f"{browser_name}-{target}-replay.png")
        return {"events": results, "paused_segment_seeks": crossings}

    async def paused_segment_seeks(self, browser_name, page, target, segs):
        """While paused, a seek into another segment shows that segment and
        reports it in the page's replay status: forward into the second
        segment, then back into the first."""
        if len(segs) < 2:
            require(not self.fake, f"{target}: {len(segs)} segment(s), the fake run needs 2")
            return []
        out = []
        for seg in (segs[1], segs[0]):
            number = int(seg["file"].split(".")[0])
            ms = seg["start_offset_ms"] + min(300, (seg["end_offset_ms"] - seg["start_offset_ms"]) // 2)
            await self.seek_timeline(page, ms)
            st = await self.wait_replay(
                page, lambda s: s["position_ms"] == ms and s["segment"] == number and s["video_time"] is not None and
                abs(seg["start_offset_ms"] + s["video_time"] * 1000 - ms) <= FRAME_INTERVAL_MS,
                f"paused seek into segment {number} at {ms}")
            require(not st["playing"] and st["video_paused"], f"{browser_name}: a paused seek started playback: {st}")
            out.append({"segment": number, "position_ms": ms, "video_ms": round(seg["start_offset_ms"] + st["video_time"] * 1000)})
        return out

    async def stale_download_check(self, browser_name, page):
        """While paused, a seek into a segment not yet downloaded followed at
        once by a seek back into the shown segment keeps the shown segment
        when the download finishes later."""
        segs = self.manifest("a")["segments"]
        require(len(segs) >= 2, f"a: {len(segs)} segment(s), the fake run needs 2")
        first, second = (int(s["file"].split(".")[0]) for s in segs[:2])
        ms = [s["start_offset_ms"] + min(300, (s["end_offset_ms"] - s["start_offset_ms"]) // 2) for s in segs[:2]]
        await self.open_replay(page, self.ids["a"])
        await self.seek_timeline(page, ms[0])
        await self.wait_replay(page, lambda s: s["segment"] == first and s["video_loaded"] and s["video_time"] is not None,
                               "first segment shown")
        count_js = "(n) => performance.getEntriesByType('resource').filter((e) => e.name.endsWith('/segments/' + n)).length"
        before = await page.evaluate(count_js, second)
        await page.evaluate("""([later, back]) => { const t = document.querySelector('.panel.replay .timeline');
            t.value = String(later); t.dispatchEvent(new Event('input'));
            t.value = String(back); t.dispatchEvent(new Event('input')); }""", [ms[1], ms[0]])
        deadline = time.monotonic() + 10
        while await page.evaluate(count_js, second) <= before:
            require(time.monotonic() < deadline, f"{browser_name}: segment {second} was not downloaded")
            await asyncio.sleep(0.05)
        await asyncio.sleep(1.0)
        st = await self.replay_state(page)
        require(st["position_ms"] == ms[0] and st["segment"] == first and st["video_time"] is not None and
                abs(segs[0]["start_offset_ms"] + st["video_time"] * 1000 - ms[0]) <= FRAME_INTERVAL_MS,
                f"{browser_name}: a late download of segment {second} replaced the shown segment: {st}")
        return {"downloaded_late": second, "shown": first, "position_ms": ms[0],
                "video_ms": round(segs[0]["start_offset_ms"] + st["video_time"] * 1000)}

    async def strip_cases(self, browser_name, page):
        """The recording-only kinds never enter the strip, a call finished
        before the recording started shows no row, a call running at stop
        shows as running."""
        out = {}
        for target in ("a", "b"):
            await self.open_replay(page, self.ids[target])
            total = (await self.replay_state(page))["total_ms"]
            await self.seek_timeline(page, str(total))
            await self.wait_replay(page, lambda s: s["position_ms"] >= total - 10, "end of recording")
            rows = await page.evaluate(STRIP_JS, ".panel.replay")
            text = " | ".join(r["text"] for r in rows)
            for word in ("recording", "annotation", "segment", "Other event", "Older events dropped"):
                require(word not in text, f"{browser_name}: '{word}' in the replay strip of {target}: {text}")
            out[target] = [r["text"] for r in rows]
        a_rows = out["a"]
        require(any(" hold " in f" {r} " and "running" in r for r in a_rows),
                f"{browser_name}: a's call running at stop not shown as running: {a_rows}")
        b_events = self.events("b")
        orphan = [e for e in b_events if e["kind"] == "call_finished" and
                  not any(s["kind"] == "call_started" and s.get("call") == e.get("call") for s in b_events)]
        require(orphan, "b: no call finished without its start in the recording")
        require(not any(" hold " in f" {r} " for r in out["b"]), f"{browser_name}: b shows the call started before recording")
        return out

    # --- flows ----------------------------------------------------------------

    async def fake_flow(self, browser):
        page = await self.open_viewer_page(browser)
        # b: a call starts before its recording and finishes after it.
        b_mcp = await viewer_proof.FakeMcp(self, "b").start()
        self.endpoints["b"] = b_mcp
        await b_mcp.send({"jsonrpc": "2.0", "id": "b-held", "method": "tools/call",
                          "params": {"name": "hold", "arguments": {"text": ARGUMENT_MARKER}}})
        await asyncio.sleep(0.3)
        await self.open_panel(page, "b")
        rid_b = await self.start_from_page(page, "b")
        await self.cli("screenshot", "--session", "b", "--output", str(self.temp / "b.png"))
        await b_mcp.stop()
        await asyncio.sleep(0.5)
        stopped = await self.cli("record", "stop", "--session", "b")
        require(stopped.get("changed") is True and stopped.get("id") == rid_b, f"b: record stop {stopped}")

        # a: Cua calls ok and error, a native verb, CLI and page annotations,
        # and a call still running when a is disconnected.
        a_mcp = await viewer_proof.FakeMcp(self, "a").start()
        self.endpoints["a"] = a_mcp
        await a_mcp.request("tools/call", {"name": "echo", "arguments": {"text": ARGUMENT_MARKER, ARGUMENT_MARKER: 1}})
        failed = await a_mcp.request("tools/call", {"name": "fail", "arguments": {"text": ARGUMENT_MARKER}})
        require(failed.get("isError") is True, "fake fail did not fail")
        await self.cli("screenshot", "--session", "a", "--output", str(self.temp / "a.png"))
        await self.cli("annotate", "--session", "a", "note from the CLI")
        await self.open_panel(page, "a")
        await self.annotate_from_page(page, "a", "note from the page")
        await a_mcp.send({"jsonrpc": "2.0", "id": "a-held", "method": "tools/call",
                          "params": {"name": "hold", "arguments": {"text": ARGUMENT_MARKER}}})
        await asyncio.sleep(0.5)
        await self.cli("recording", "keep", self.ids["a"])
        await self.screenshot(page, "01-live-panels-with-recording-controls.png")
        # The synthetic display resizes every 3 s: a has a second segment for
        # the paused cross-segment seek in the replay.
        await self.wait_segments("a", 2)
        await self.cli("disconnect", "--session", "a")
        await a_mcp.stop()
        # c: one change, then a still display.
        await asyncio.sleep(2.5)
        await self.cli("disconnect", "--session", "c")
        listing = await self.wait_finished(set(self.ids.values()))
        await self.keep_from_page(page, rid_b)
        await self.screenshot(page, "02-recordings-list.png")
        listing = await self.recordings()
        kept = {r["id"]: r["kept"] for r in listing["recordings"]}
        require(kept.get(self.ids["a"]) and kept.get(rid_b), f"a and b kept: {kept}")
        self.check("recordings_listed_and_kept", recordings=listing["recordings"],
                   kept_bytes=listing["kept_bytes"], unkept_bytes=listing["unkept_bytes"])
        await page.close()
        self.disk_checks()

    def disk_checks(self):
        results = {}
        for target in sorted(self.ids):
            events = self.events(target)
            raw = (self.rec_dir(target) / "events.jsonl").read_bytes()
            require(ARGUMENT_MARKER.encode() not in raw, f"{target}: argument marker in the event log")
            require(b"arguments" not in raw, f"{target}: argument names in the event log")
            require(events[0]["kind"] == "recording_started" and events[-1]["kind"] == "recording_stopped",
                    f"{target}: first/last events")
            seqs = [e["seq"] for e in events]
            require(seqs == list(range(1, len(events) + 1)), f"{target}: sequence numbers")
            offsets = [e["offset_ms"] for e in events]
            require(offsets == sorted(offsets), f"{target}: offsets decrease")
            results[target] = {"listing": self.listing(target), "segments": self.probe_segments(target),
                               "manifest_stats": self.manifest(target).get("stats"),
                               "end_reason": self.manifest(target).get("end_reason"),
                               "trigger": self.manifest(target).get("trigger")}
            self.write(f"events-{target}.json", json.dumps(self.excerpt(events), indent=2))
        if self.fake:
            a = self.events("a")
            sources = {(e["kind"], e.get("text")): e["source"] for e in a if e["kind"] == "annotation"}
            require(sources.get(("annotation", "note from the CLI")) == "cli" and
                    sources.get(("annotation", "note from the page")) == "viewer", f"a: annotation sources {sources}")
            outcomes = [e["outcome"] for e in a if e["kind"] == "call_finished"]
            require(outcomes[:3] == ["ok", "error", "ok"], f"a: outcomes {outcomes}")
            b = self.events("b")
            require(b[0]["trigger"] == "viewer" and b[0]["source"] == "viewer", f"b: start {b[0]}")
            require(b[-1]["source"] == "cli" and b[-1]["reason"] == "requested", f"b: stop {b[-1]}")
            c = self.manifest("c")
            require(len(c["segments"]) == 1 and c["segments"][0]["frames"] == 2,
                    f"c: the still display's frames all reach the segment: {c['segments']}")
        self.check("recordings_on_disk", recordings=results)

    async def replay_flow(self, playwright):
        for name in ("chromium", "firefox"):
            browser = await getattr(playwright, name).launch(headless=True)
            try:
                page = await browser.new_page(viewport={"width": 1600, "height": 1200})
                page.on("console", lambda m: self.page_errors.append(f"{name}: {m.text}") if m.type == "error" else None)
                await page.goto(self.urls[0])
                await page.click("#recordings-button")
                await page.wait_for_selector(f'#recordings tr[data-recording="{self.ids["a"]}"]', timeout=10000)
                kept = await page.text_content(f'#recordings tr[data-recording="{self.ids["a"]}"] td:nth-child(8)')
                require(kept == "kept", f"{name}: a not marked kept")
                selections = await self.replay_checks(name, page, "a", self.events("a"))
                strips = await self.strip_cases(name, page)
                stale = await self.stale_download_check(name, page)
                self.check(f"{name}.replay", version=browser.version, selections=selections, strips=strips,
                           stale_download=stale)
            finally:
                await browser.close()

    async def live_flow(self, browser):
        e2e = self.e2e
        page = await self.open_viewer_page(browser)
        for target in ("a", "b"):
            self.endpoints[target] = await e2e.Mcp(viewer_proof.McpRun(self), target).start()
        # a: Notepad, the typed marker, a failing call.
        await self.endpoints["a"].tool("launch_app", {"path": r"C:\Windows\System32\notepad.exe"})
        window = await self.find_window("a", "Notepad")
        await asyncio.sleep(2)
        # Background typing reaches Notepad's text area only when it has the
        # focus: a native click in it gives the focus.
        editor = await self.notepad_editor("a", window)
        frame = editor["frame"]
        await self.cli("input", "click", "--session", "a",
                       "--x", str(frame["x"] + frame["w"] // 2), "--y", str(frame["y"] + frame["h"] // 2))
        await asyncio.sleep(1)
        editor = await self.notepad_editor("a", window)
        before = self.output / "a-native-before-typing.png"
        await self.cli("screenshot", "--session", "a", "--output", str(before))
        marker = self.typed_marker
        await self.endpoints["a"].tool("type_text", {**window, "element_token": editor["element_token"], "text": marker})
        typed_at = time.time()
        await asyncio.sleep(1.5)
        native = self.output / "a-native-after-typing.png"
        await self.cli("screenshot", "--session", "a", "--output", str(native))
        failure = await self.failing_tool_call("a", LIVE_FAILING_TOOL, {"text": marker})
        await self.cli("annotate", "--session", "a", "note from the CLI")
        await self.open_panel(page, "a")
        await self.annotate_from_page(page, "a", "note from the page")
        # Close a's live panel for the long still interval: a headless tab
        # decoding full-HD frames for minutes can run out of memory.
        await page.click('button[data-session="a"]')
        await self.wait_state(page, lambda s: "a" not in s["sessions"], "panel a closed", 10)
        idle_from = time.time()
        await asyncio.sleep(IDLE_S)
        idle_to = time.time()
        await self.endpoints["a"].stop()
        await self.cli("recording", "keep", self.ids["a"])
        await self.cli("disconnect", "--session", "a")
        # b: started from the page, one action, stopped with the CLI.
        await self.open_panel(page, "b")
        rid_b = await self.start_from_page(page, "b")
        await self.endpoints["b"].tool("list_windows")
        await self.endpoints["b"].stop()
        stopped = await self.cli("record", "stop", "--session", "b")
        require(stopped.get("changed") is True and stopped.get("id") == rid_b, f"b: record stop {stopped}")
        await self.wait_finished({self.ids["a"], rid_b}, timeout=120)
        await self.keep_from_page(page, rid_b)
        await self.screenshot(page, "02-recordings-list.png")
        listing = await self.recordings()
        kept = {r["id"]: r["kept"] for r in listing["recordings"]}
        require(kept.get(self.ids["a"]) and kept.get(rid_b), f"a and b kept: {kept}")
        require("c" not in self.ids and not any(r["session"] == "c" for r in listing["recordings"]), "c has a recording")
        self.check("recordings_listed_and_kept", recordings=listing["recordings"],
                   kept_bytes=listing["kept_bytes"], unkept_bytes=listing["unkept_bytes"], no_recording_for="c")
        await page.close()
        self.disk_checks()
        a = self.events("a")
        self.live_event_checks(a, self.events("b"), failure)
        self.idle_check(a, idle_from, idle_to)
        self.live_marker = {"typed_at": typed_at, "native": native, "before": before, "editor": frame}

    async def notepad_editor(self, target, window):
        """Notepad's text area from the accessibility tree: element token and screen frame."""
        e2e = self.e2e
        state = e2e.structured(await self.endpoints[target].tool(
            "get_window_state", {**window, "include_screenshot": False, "include_accessibility_tree": True}))
        editors = [obj for obj in e2e.objects(state)
                   if "element_token" in obj and obj.get("role") == "Edit" and obj.get("label") == "Text Editor"]
        require(editors and "frame" in editors[0], f"{target}: Notepad text area not in the accessibility tree")
        return editors[0]

    async def find_window(self, target, title_part):
        e2e, endpoint = self.e2e, self.endpoints[target]
        for _ in range(40):
            listing = e2e.structured(await endpoint.tool("list_windows"))
            for obj in e2e.objects(listing):
                if title_part in str(obj.get("title", obj.get("window_title", ""))):
                    wid, pid = obj.get("window_id", obj.get("hwnd", obj.get("id"))), obj.get("pid")
                    if wid is not None and pid is not None:
                        return {"pid": int(pid), "window_id": int(wid)}
            await asyncio.sleep(0.5)
        raise ProofError(f"{target}: {title_part} window not discoverable")

    def live_event_checks(self, a, b, failure):
        calls = [(e["name"], e["outcome"]) for e in a if e["kind"] == "call_finished"]
        require(("type_text", "ok") in calls and (LIVE_FAILING_TOOL, "error") in calls and ("screenshot", "ok") in calls,
                f"a: calls {calls}")
        notes = {e["text"]: e["source"] for e in a if e["kind"] == "annotation"}
        require(notes == {"note from the CLI": "cli", "note from the page": "viewer"}, f"a: annotations {notes}")
        require(b[0]["kind"] == "recording_started" and b[0]["source"] == "viewer", f"b: start {b[0]}")
        require(b[-1]["kind"] == "recording_stopped" and b[-1]["source"] == "cli", f"b: stop {b[-1]}")
        self.check("a_and_b_event_logs", failing_call=failure,
                   a_calls=[{k: e.get(k) for k in ("name", "at", "duration_ms", "outcome")} for e in a if e["kind"] == "call_finished"],
                   b_start=self.excerpt([b[0]]), b_stop=self.excerpt([b[-1]]))

    def idle_check(self, events, idle_from, idle_to):
        """No video frame in the still interval: from SETTLE_MS after the page
        annotation (the desktop settles) to the end of the idle wait, at
        least 2 minutes."""
        note = next(e for e in events if e["kind"] == "annotation" and e["source"] == "viewer")
        start = note["offset_ms"] + SETTLE_MS
        end = note["offset_ms"] + round((idle_to - idle_from) * 1000) - 3000
        require(end - start >= 120000, f"a: still interval only {end - start} ms")
        frames = [o for o in self.packet_offsets("a") if start <= o <= end]
        require(not frames, f"a: {len(frames)} frames during the still interval {start}-{end}: {frames[:10]}")
        total = sum(s["bytes"] for s in self.manifest("a")["segments"])
        self.check("idle_interval_adds_no_video", interval_ms=[start, end], frames=0, video_bytes_total=total)

    async def live_replay(self, playwright):
        """Replay a in both browsers at the typing call's finish: the marker is
        legible within 2 s of the call finishing (crop PSNR vs the native
        screenshot)."""
        from PIL import Image
        import numpy as np
        events = self.events("a")
        typing = next(e for e in events if e["kind"] == "call_finished" and e.get("name") == "type_text")
        before = np.asarray(Image.open(self.live_marker["before"]).convert("RGB")).astype(int)
        after = np.asarray(Image.open(self.live_marker["native"]).convert("RGB")).astype(int)
        # Only the first text line of the editor: the title, the status bar
        # and the Cua cursor overlay also change when the marker is typed.
        editor = self.live_marker["editor"]
        left, top = editor["x"], editor["y"]
        diff = np.abs(after - before)[top:top + 48, left:left + editor["w"]].max(axis=2) > 48
        ys, xs = np.nonzero(diff)
        require(len(xs) > 50, "no typed marker found in the native screenshots")
        box = [max(0, left + xs.min() - 4), max(0, top + ys.min() - 4), left + xs.max() + 5, top + ys.max() + 5]
        crop_native = after[box[1]:box[3], box[0]:box[2]]
        Image.fromarray(crop_native.astype("uint8")).save(self.output / "marker-crop-native.png")

        def luma(a):
            return 0.2126 * a[..., 0] + 0.7152 * a[..., 1] + 0.0722 * a[..., 2]

        results = {}
        for name in ("chromium", "firefox"):
            browser = await getattr(playwright, name).launch(headless=True)
            try:
                page = await browser.new_page(viewport={"width": 2200, "height": 1600})
                await page.goto(self.urls[0])
                selections = await self.replay_checks(name, page, "a", events)
                target_ms = typing["offset_ms"] + LEGIBLE_WITHIN_MS
                await self.select_log(page, typing["seq"])
                await self.wait_replay(page, lambda s: s["position_ms"] == typing["offset_ms"], "typing call")
                await self.seek_timeline(page, str(target_ms))
                await self.wait_replay(page, lambda s: s["position_ms"] == target_ms and s["video_ready"] >= 2, "marker frame")
                await asyncio.sleep(0.5)
                data = await page.evaluate("""() => { const v = document.querySelector('.panel.replay video');
                    const c = document.createElement('canvas'); c.width = v.videoWidth; c.height = v.videoHeight;
                    c.getContext('2d').drawImage(v, 0, 0); return c.toDataURL('image/png'); }""")
                import base64, io
                frame = np.asarray(Image.open(io.BytesIO(base64.b64decode(data.split(",", 1)[1]))).convert("RGB")).astype(int)
                crop = frame[box[1]:box[3], box[0]:box[2]]
                Image.fromarray(crop.astype("uint8")).save(self.output / f"marker-crop-{name}.png")
                mse = ((luma(crop) - luma(crop_native)) ** 2).mean()
                psnr = 99.0 if mse == 0 else 10 * np.log10(255 ** 2 / mse)
                require(psnr >= 30, f"{name}: marker crop PSNR {psnr:.1f} dB < 30 dB")
                await self.screenshot(page, f"{name}-replay-marker.png")
                results[name] = {"version": browser.version, "psnr_db": round(float(psnr), 2),
                                 "frame_at_ms": target_ms, "selections": selections}
            finally:
                await browser.close()
        self.check("replay_marker_legible_within_2s", typing_call_finished_ms=typing["offset_ms"], crop_box=[int(v) for v in box],
                   browsers=results)

    def cpu_seconds(self):
        if not self.daemon:
            return None
        stat = Path(f"/proc/{self.daemon.pid}/stat").read_text()
        fields = stat[stat.rindex(")") + 2:].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")

    # --- orchestration --------------------------------------------------------

    async def execute(self):
        from playwright.async_api import async_playwright

        daemon_log = open(self.output / "daemon.log", "w")
        try:
            if not self.fake:
                for target in LIVE_TARGETS:
                    relay = self.e2e.Relay(self.credentials[target])
                    self.relays[target] = (relay, await relay.start())
            self.daemon = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot-daemon"), env=self.env,
                                                               stdout=daemon_log, stderr=daemon_log)
            await asyncio.sleep(0.3)
            cpu0, t0 = self.cpu_seconds(), time.monotonic()
            if self.fake:
                await self.connect_with("a", "--record")
                await self.connect_with("b", "--no-record")
                await self.connect_with("c", "--record")
            else:
                await self.connect_with("a")
                await self.connect_with("b", "--no-record")
                # c only has to stay unrecorded: it needs no Cua bridge.
                await self.connect_with("c", "-o", "CuaEnabled=no")
                require(set(self.ids) == {"a"}, f"only a records from connect: {self.ids}")
            await self.start_viewer()
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(headless=True)
                if self.fake:
                    await self.fake_flow(browser)
                else:
                    await self.live_flow(browser)
                await browser.close()
                cpu = self.cpu_seconds()
                self.summary["daemon_cpu"] = {"seconds": round(cpu - cpu0, 2) if cpu is not None else None,
                                              "wall_seconds": round(time.monotonic() - t0, 1)}
                if self.fake:
                    await self.replay_flow(playwright)
                else:
                    await self.live_replay(playwright)
            # Closed sessions answer 410 by design; Firefox asks for a favicon.
            errors = [e for e in self.page_errors if "favicon" not in e and "410 (Gone)" not in e]
            require(not errors, f"page errors: {errors}")
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
            for target in self.targets:
                try:
                    await self.cli("disconnect", "--session", target, timeout=15, allow_failure=True)
                except (OSError, asyncio.TimeoutError, ValueError, ProofError):
                    pass
            if self.daemon and self.daemon.returncode is None:
                self.daemon.terminate()
                try:
                    await asyncio.wait_for(self.daemon.wait(), 15)
                except asyncio.TimeoutError:
                    self.daemon.kill()
                    await self.daemon.wait()
            for relay, _ in self.relays.values():
                await relay.close()
            daemon_log.close()
            for path in (self.output / "daemon.log", self.output / "view.stderr"):
                if path.exists():
                    path.write_text(self.clean(path.read_text(errors="replace")))
            # MCP error output is evidence; its transcripts (typed text) are not.
            for stderr in (self.temp / "mcp").glob("*.stderr"):
                shutil.copy(stderr, self.output / stderr.name)
            self.save()
            shutil.rmtree(self.temp, ignore_errors=True)
        self.scan_evidence()


def load_cua():
    return load("run_cua_e2e", "run-cua-e2e.py")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fake", action="store_true", help="offline run against the fake connector")
    parser.add_argument("--bin-dir", required=True, help="directory with rdpilot, rdpilot-daemon, rdpilot-mcp")
    parser.add_argument("--bundle", help="live mode: daemon bundle_path directory (default: download into a fresh cache)")
    parser.add_argument("--output", required=True, help="new evidence directory; must not already exist")
    parser.add_argument("--bind", choices=["loopback", "loopback+tailnet"], help="viewer bind set (default: loopback)")
    parser.add_argument("--allow-debug", action="store_true", help="live mode: allow debug binaries")
    parser.add_argument("--connect-timeout", type=int, default=600)
    args = parser.parse_args()
    if not args.fake:
        if "debug" in Path(args.bin_dir).resolve().parts and not args.allow_debug:
            parser.error("live mode needs release binaries (cargo build --release --workspace)")
    os.umask(0o077)
    try:
        proof = Proof(args)
    except (ProofError, OSError) as error:
        print(f"Recording proof not started: {error}", flush=True)
        return 2
    try:
        asyncio.run(proof.execute())
    except (Exception, KeyboardInterrupt) as error:
        print(proof.clean(f"Recording proof failed: {type(error).__name__}: {error}"), flush=True)
        return 1
    print(f"Evidence: {proof.output / 'summary.json'}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
