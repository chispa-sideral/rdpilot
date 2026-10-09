#!/usr/bin/env python3
"""Performance workloads and the baseline measurement for the live viewer.

W1 (display) runs one 1920x1080 session that shows a continuously scrolling
window, two viewer pages (two browser contexts on the one viewer server) and
the recorder, for a fixed steady-state window. W2 (resize), W3 (clipboard) and
W4 (print) are defined and reported as "not measurable at baseline" while the
feature does not exist; they never report a number and never pass.

Frame-to-screen latency uses a pixel clock. A test-only painter window in the
RDP session (scripts/e2e/perf/PixelClock.cs) paints the current system time as
a black and white bar code, twice, at the top of the desktop, on a fixed
timer (a parameter, 20 ms by default). Each viewer page decodes the bar code
from its canvas on every animation frame (stale-frame sampling: a frame that
stays on screen counts as old on every tick) and subtracts it from Date.now().
The sample takes the first copy whose checksum is valid. The painter timer
puts 0 to one interval of paint-to-capture age into every sample, and two
copies that differ (a torn frame) leave one interval of uncertainty; both
make the figures conservative. The page callback reads the canvas of the
frame it presents; the compositor delay after the callback is not in the
sample. The same machine runs painter, daemon and browser, so one clock
serves; the offset between Date.now() and the painter clock is measured next
to the latency and not corrected.

Each run has one pass per recorder rate (4 and 8 fps by default), each in its
own daemon lifetime. A pass reports, per viewer and pooled: frame age p50, p95
and max with sample counts, the share of undecodable ticks, distinct frames and
torn frames, frame rate and encode time from the frame response headers; the
input-to-change wall time of a key press as a cross-check (never pooled); the
recorder counters of the recording manifest, with the recording started and
stopped at the window edges; and daemon CPU (cores) and working set. The
result is perf-result.json (schema 1). The summary status "passed" means the
measurement path completed with non-zero sample counts in every pass; no value
is compared with a target.

Fake mode (offline, Linux; fake connector, the real page, sampler, decoder,
statistics, CPU sampler, recorder start and stop and manifest reading, against
synthetic frames of a known age that the harness serves in place of the
daemon's PNG responses):
  cargo build --workspace
  uv run --with playwright python3 scripts/e2e/run-perf-proof.py --fake \
    --bin-dir target/debug --output perf-fake-proof

Self test (no binaries, no browser):
  python3 scripts/e2e/run-perf-proof.py --selftest

Live mode (a Windows host with one standard user; release builds; the hosted
runner starts it with the workflow_dispatch input perf_workload through
scripts/ci/run-hosted-desktop.ps1 -Proof perf). The painter starts from a
logon entry of the test user; --painter-dir holds PixelClock.cs and
painter.ps1:
  cargo build --release --workspace
  export RDPILOT_PERF_A_HOST=... RDPILOT_PERF_A_USERNAME=... RDPILOT_PERF_A_PASSWORD=...
  # optional: RDPILOT_PERF_A_PORT, RDPILOT_PERF_A_DOMAIN
  python3 scripts/e2e/run-perf-proof.py --bin-dir target/release \
    --painter-dir scripts/e2e/perf --output perf-live-proof

The session connects with Cua disabled: no Cua download and no Cua overlay.
Credentials come only from the environment and reach rdpilot only through the
environment of its commands. The viewer token and every credential value are
replaced in the evidence, and a final scan fails the run if one remains.
Exit codes: 0 measured, 1 failed, 2 not started or only skipped workloads.
"""
import argparse
import asyncio
import base64
import ctypes
import http.client
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib

HERE = Path(__file__).resolve().parent
DAEMON = "rdpilot-daemon.exe" if os.name == "nt" else "rdpilot-daemon"
POWERSHELL = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
SCHEMA = 1

# Bar code layout; it must match perf/PixelClock.cs.
CELL = 24
CELLS = 72
ROW_HEIGHT = 32
ROW_YS = (ROW_HEIGHT // 2, ROW_HEIGHT + ROW_HEIGHT // 2)  # centre line of each copy
NEED_W = CELLS * CELL
STRIP_HEIGHT = 2 * ROW_HEIGHT
MODULUS = 1 << 48
SCREEN_W, SCREEN_H = 1920, 1080
# The key-driven colour block (cross-check), in desktop pixels; it must match the painter.
BLOCK = (1620, 100, 280, 200)
BLOCK_PROBE = (BLOCK[0] + BLOCK[2] // 2, BLOCK[1] + BLOCK[3] // 2)
BLOCK_ON, BLOCK_OFF = (255, 220, 0), (0, 0, 160)
GRAY = (44, 52, 70)

# A clock sample counts for the offset bound when its round trip is at most this.
RTT_MS_MAX = 5.0
PANEL_CANVAS = '.panel[data-session="a"] canvas'

# Fake mode: the synthetic frames.
FAKE_AGE_MS = 100
FAKE_HOLD_MS = 200
FAKE_ENCODE_MS = 37

SKIPPED = {
    "w2": {"name": "resize", "metric": "controller resize to first frame at the new size",
           "status": "not_measurable_at_baseline", "owner_area": "resize",
           "reason": "no resize operation exists in the CLI, IPC or MCP and the connection negotiates no display control channel"},
    "w3": {"name": "clipboard", "metric": "1 MB text round trip",
           "status": "not_measurable_at_baseline", "owner_area": "clipboard",
           "reason": "no clipboard channel exists in any crate"},
    "w4": {"name": "print", "metric": "one-page print to PDF",
           "status": "not_measurable_at_baseline", "owner_area": "printer",
           "reason": "no printer device is redirected; the device channel serves drives only"},
}


class PerfError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise PerfError(message)


# --- bar code ----------------------------------------------------------------------------

def crc16(data):
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def encode_bits(micros):
    """The cells of the bar code for a time in microseconds (1 = white)."""
    value = micros & (MODULUS - 1)
    crc = crc16(value.to_bytes(6, "big"))
    bits = [1, 0, 1, 0]
    bits += [(value >> (47 - i)) & 1 for i in range(48)]
    bits += [(crc >> (15 - i)) & 1 for i in range(16)]
    bits += [0, 1, 0, 1]
    return bits


def decode_bits(bits):
    """Value of a decoded cell list, or -1 (sync) or -2 (checksum)."""
    if bits[:4] != [1, 0, 1, 0] or bits[68:72] != [0, 1, 0, 1]:
        return -1
    data = bytes(int("".join(str(b) for b in bits[4 + 8 * i:12 + 8 * i]), 2) for i in range(6))
    crc = int("".join(str(b) for b in bits[52:68]), 2)
    if crc16(data) != crc:
        return -2
    return int.from_bytes(data, "big")


def full_micros(now_ms, value):
    """The painter time nearest to `now_ms` that matches the 48-bit value."""
    now_us = now_ms * 1000
    return now_us - ((now_us - value + MODULUS // 2) % MODULUS - MODULUS // 2)


# --- statistics --------------------------------------------------------------------------

def percentile(values, q):
    """Nearest-rank percentile of a list, or None."""
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, max(0, math.ceil(q * len(ordered)) - 1))]


def dist(values):
    """n, min, p50, p95, p99, max and mean of a list; None members for an empty list."""
    if not values:
        return {"n": 0, "min": None, "p50": None, "p95": None, "p99": None, "max": None, "mean": None}
    return {"n": len(values), "min": min(values), "p50": percentile(values, 0.5), "p95": percentile(values, 0.95),
            "p99": percentile(values, 0.99), "max": max(values), "mean": statistics.fmean(values)}


def analyze_ticks(data):
    """Decode ratio, tick rate, torn frames and the frame ages of one page from its raw samples.

    Returns (summary, ages). A tick with canvas data counts; of its two copies the
    first one with a valid checksum gives the painted time.
    """
    ts, now, seq, a, b = data["ts"], data["now"], data["seq"], data["a"], data["b"]
    n = len(ts)
    out = {"ticks": n, "page_errors": data.get("errors", []), "visibility": data.get("visibility")}
    if n >= 2:
        span_s = (ts[-1] - ts[0]) / 1000.0
        out["raf_hz_mean"] = (n - 1) / span_s if span_s > 0 else None
        out["raf_interval_ms"] = dist([ts[i + 1] - ts[i] for i in range(n - 1)])
        out["tick_cost_ms"] = dist(data.get("cost", []))
    with_frame = [i for i in range(n) if a[i] != -3 or b[i] != -3]
    valid = [i for i in with_frame if a[i] >= 0 or b[i] >= 0]
    valid_set = set(valid)
    both = [i for i in with_frame if a[i] >= 0 and b[i] >= 0]
    torn_ticks = [i for i in both if a[i] != b[i]]
    frames = {}
    for i in with_frame:
        frames.setdefault(seq[i], []).append(i)
    out.update({
        "ticks_with_frame": len(with_frame),
        "ticks_decoded": len(valid),
        "undecodable_ticks": len(with_frame) - len(valid),
        "undecodable_ratio": (len(with_frame) - len(valid)) / len(with_frame) if with_frame else None,
        "distinct_frames": len(frames),
        "distinct_frames_decoded": sum(1 for idx in frames.values() if any(i in valid_set for i in idx)),
        "torn_ticks": len(torn_ticks),
        "torn_frames": sum(1 for idx in frames.values() if any(a[i] >= 0 and b[i] >= 0 and a[i] != b[i] for i in idx)),
    })
    ages = []
    for i in valid:
        value = a[i] if a[i] >= 0 else b[i]
        ages.append(now[i] - full_micros(now[i], value) / 1000.0)
    out["frame_age_negative"] = sum(1 for age in ages if age < 0)
    return out, ages


def analyze_clock(samples):
    """Offset of the painter clock from Date.now() at the midpoint of an in-page round trip."""
    rows = [{"offset_ms": painter_us / 1000.0 - (r0 + r1) / 2.0, "rtt_ms": rtt}
            for r0, r1, painter_us, rtt, _p0, _p1 in samples]
    tight = [r for r in rows if r["rtt_ms"] <= RTT_MS_MAX]
    offsets = [r["offset_ms"] for r in tight]
    return {
        "samples": len(rows),
        "samples_rtt_le_5ms": len(tight),
        "rtt_ms_median": statistics.median([r["rtt_ms"] for r in rows]) if rows else None,
        "median_offset_ms": statistics.median(offsets) if offsets else None,
        "max_abs_offset_ms": max(abs(v) for v in offsets) if offsets else None,
        "min_offset_ms": min(offsets) if offsets else None,
        "max_offset_ms": max(offsets) if offsets else None,
    }


# --- in-page code ----------------------------------------------------------------------

SAMPLER_JS = r"""
(cfg) => {
  const cell = cfg.cell, cells = cfg.cells, need = cells * cell;
  function crc16(bytes) {
    let crc = 0xFFFF;
    for (const byte of bytes) {
      crc ^= byte << 8;
      for (let i = 0; i < 8; i++) { crc = (crc & 0x8000) ? (((crc << 1) ^ 0x1021) & 0xFFFF) : ((crc << 1) & 0xFFFF); }
    }
    return crc;
  }
  function decode(px) {
    const bits = new Array(cells);
    for (let i = 0; i < cells; i++) {
      const o = (i * cell + (cell >> 1)) * 4;
      bits[i] = (px[o] + px[o + 1] + px[o + 2]) >= 384 ? 1 : 0;
    }
    const head = [1, 0, 1, 0], tail = [0, 1, 0, 1];
    for (let i = 0; i < 4; i++) { if (bits[i] !== head[i] || bits[68 + i] !== tail[i]) { return -1; } }
    const bytes = [];
    let value = 0;
    for (let b = 0; b < 6; b++) {
      let x = 0;
      for (let k = 0; k < 8; k++) { x = (x << 1) | bits[4 + b * 8 + k]; }
      bytes.push(x);
      value = value * 256 + x;
    }
    let crc = 0;
    for (let k = 0; k < 16; k++) { crc = (crc << 1) | bits[52 + k]; }
    return crc16(bytes) === crc ? value : -2;
  }
  const S = window.__pc = { ts: [], now: [], seq: [], a: [], b: [], cost: [], running: true, done: false,
                            errors: [], visibility: document.visibilityState };
  function tick(ts) {
    if (!S.running) { S.done = true; return; }
    try {
      const t0 = performance.now();
      const now = Date.now();
      const session = (window.rdpilotViewer && window.rdpilotViewer.sessions && window.rdpilotViewer.sessions[cfg.id]) || {};
      const canvas = document.querySelector(cfg.selector);
      let a = -3, b = -3;
      if (canvas && canvas.width >= need && canvas.height > cfg.rowYs[1]) {
        const ctx = canvas.getContext("2d");
        a = decode(ctx.getImageData(0, cfg.rowYs[0], need, 1).data);
        b = decode(ctx.getImageData(0, cfg.rowYs[1], need, 1).data);
      }
      S.ts.push(ts); S.now.push(now); S.seq.push(session.seq || 0); S.a.push(a); S.b.push(b);
      S.cost.push(performance.now() - t0);
    } catch (error) {
      S.errors.push(String(error));
    }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
  return true;
}
"""

STOP_JS = r"""
() => {
  const S = window.__pc;
  S.running = false;
  return new Promise((resolve) => {
    const started = performance.now();
    (function wait() {
      if (S.done || performance.now() - started > 3000) {
        resolve({ ts: S.ts, now: S.now, seq: S.seq, a: S.a, b: S.b, cost: S.cost, errors: S.errors,
                  visibility: S.visibility });
      } else {
        setTimeout(wait, 20);
      }
    })();
  });
}
"""

CLOCK_BATCH_JS = r"""
async ([url, count, gapMs]) => {
  const out = [];
  for (let i = 0; i < count; i++) {
    const p0 = performance.now(), r0 = Date.now();
    const response = await fetch(url, { cache: "no-store" });
    const text = await response.text();
    const r1 = Date.now(), p1 = performance.now();
    out.push([r0, r1, Number(text), p1 - p0, performance.timeOrigin + p0, performance.timeOrigin + p1]);
    await new Promise((resolve) => setTimeout(resolve, gapMs));
  }
  return out;
}
"""

# Distinct Date.now() values seen in a 300 ms busy loop, as step sizes in ms.
GRANULARITY_JS = r"""
() => {
  const steps = [];
  let last = Date.now();
  const end = performance.now() + 300;
  while (performance.now() < end) {
    const now = Date.now();
    if (now !== last) { steps.push(now - last); last = now; }
  }
  steps.sort((x, y) => x - y);
  return { count: steps.length, min: steps[0] ?? null, median: steps[Math.floor(steps.length / 2)] ?? null,
           max: steps[steps.length - 1] ?? null };
}
"""

# State of the key-driven colour block on the canvas: "on" (yellow), "off" (blue) or "other".
BLOCK_STATE_JS = r"""
([selector, x, y]) => {
  const canvas = document.querySelector(selector);
  if (!canvas || canvas.width <= x || canvas.height <= y) { return null; }
  const d = canvas.getContext("2d").getImageData(x, y, 1, 1).data;
  if (d[0] > 200 && d[2] < 100) { return "on"; }
  if (d[2] > 100 && d[0] < 80) { return "off"; }
  return "other";
}
"""

# Date.now() of the first animation frame whose canvas shows the block in state `want`.
WATCH_BLOCK_JS = r"""
([selector, x, y, want, timeoutMs]) => new Promise((resolve) => {
  const started = performance.now();
  function state() {
    const canvas = document.querySelector(selector);
    if (!canvas || canvas.width <= x || canvas.height <= y) { return null; }
    const d = canvas.getContext("2d").getImageData(x, y, 1, 1).data;
    if (d[0] > 200 && d[2] < 100) { return "on"; }
    if (d[2] > 100 && d[0] < 80) { return "off"; }
    return "other";
  }
  function tick() {
    const now = Date.now();
    const seen = state();
    if (seen === want) { resolve({ t: now, state: seen }); return; }
    if (performance.now() - started > timeoutMs) { resolve({ t: null, state: seen }); return; }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
})
"""

STRIP_PNG_JS = r"""
([selector, width, height]) => {
  const canvas = document.querySelector(selector);
  if (!canvas) { return null; }
  const crop = document.createElement("canvas");
  crop.width = Math.min(width, canvas.width); crop.height = Math.min(height, canvas.height);
  crop.getContext("2d").drawImage(canvas, 0, 0);
  return crop.toDataURL("image/png");
}
"""


# --- process meter -----------------------------------------------------------------------

class ProcessMeter:
    """CPU time and working set of one process: ctypes on Windows, /proc on Linux."""

    def __init__(self, pid):
        self.pid = pid
        self.handle = None
        if os.name == "nt":
            from ctypes import wintypes

            class FileTime(ctypes.Structure):
                _fields_ = [("low", wintypes.DWORD), ("high", wintypes.DWORD)]

            class MemoryCounters(ctypes.Structure):
                _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                            ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t),
                            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t), ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                            ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]

            self.FileTime, self.MemoryCounters = FileTime, MemoryCounters
            self.k32 = ctypes.WinDLL("kernel32", use_last_error=True)
            self.k32.OpenProcess.restype = wintypes.HANDLE
            self.k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
            self.k32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(FileTime)] * 4
            self.k32.K32GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(MemoryCounters), wintypes.DWORD]
            self.k32.CloseHandle.argtypes = [wintypes.HANDLE]
            self.handle = self.k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
            require(self.handle, f"OpenProcess failed with error {ctypes.get_last_error()}")

    def close(self):
        if self.handle:
            self.k32.CloseHandle(self.handle)
            self.handle = None

    def read(self):
        """(cpu seconds, working set bytes, peak working set bytes)."""
        if os.name == "nt":
            created, exited, kernel, user = (self.FileTime() for _ in range(4))
            require(self.k32.GetProcessTimes(self.handle, ctypes.byref(created), ctypes.byref(exited),
                                             ctypes.byref(kernel), ctypes.byref(user)), "GetProcessTimes failed")
            counters = self.MemoryCounters()
            counters.cb = ctypes.sizeof(counters)
            require(self.k32.K32GetProcessMemoryInfo(self.handle, ctypes.byref(counters), counters.cb),
                    "GetProcessMemoryInfo failed")

            def seconds(t):
                return ((t.high << 32) | t.low) * 1e-7
            return seconds(kernel) + seconds(user), counters.WorkingSetSize, counters.PeakWorkingSetSize
        stat = Path(f"/proc/{self.pid}/stat").read_text()
        fields = stat[stat.rindex(")") + 2:].split()
        cpu = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
        status = {}
        for line in Path(f"/proc/{self.pid}/status").read_text().splitlines():
            if line.startswith(("VmRSS:", "VmHWM:")):
                status[line.split(":")[0]] = int(line.split()[1]) * 1024
        return cpu, status.get("VmRSS", 0), status.get("VmHWM", 0)


def memory_mb():
    try:
        if os.name == "nt":
            class MemoryStatus(ctypes.Structure):
                _fields_ = [("length", ctypes.c_ulong), ("load", ctypes.c_ulong), ("total", ctypes.c_ulonglong),
                            ("avail", ctypes.c_ulonglong), ("total_page", ctypes.c_ulonglong),
                            ("avail_page", ctypes.c_ulonglong), ("total_virtual", ctypes.c_ulonglong),
                            ("avail_virtual", ctypes.c_ulonglong), ("avail_ext", ctypes.c_ulonglong)]
            status = MemoryStatus()
            status.length = ctypes.sizeof(status)
            ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status))
            return int(status.total / (1024 * 1024))
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1]) // 1024
    except (OSError, AttributeError, ValueError):
        pass
    return None


def powershell_text(command):
    """Output of a Windows PowerShell 5.1 command, or None."""
    if os.name != "nt":
        return None
    try:
        done = subprocess.run([POWERSHELL, "-NoProfile", "-NonInteractive", "-Command", command],
                              capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return done.stdout.strip() if done.returncode == 0 else None


# --- synthetic frames (fake mode) -------------------------------------------------------

def png_chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


class FakeFrames:
    """1920x1080 PNG frames with a bar code of a known age and a key-driven block.

    The body (everything under the strip) is compressed once per block state; a
    frame compresses only the strip, ends it with a full flush and appends the
    body stream, so a frame costs a few milliseconds and its bar code time is
    taken right before.
    """

    def __init__(self, age_ms):
        self.age_us = age_ms * 1000
        self.block_on = False
        self.seq = 0
        plain = bytes([0]) + bytes(GRAY) * SCREEN_W
        x0, y0, w, h = BLOCK
        self.body_raw, self.body_deflate = {}, {}
        for state, colour in ((False, BLOCK_OFF), (True, BLOCK_ON)):
            block = bytes([0]) + bytes(GRAY) * x0 + bytes(colour) * w + bytes(GRAY) * (SCREEN_W - x0 - w)
            raw = plain * (y0 - STRIP_HEIGHT) + block * h + plain * (SCREEN_H - y0 - h)
            compressor = zlib.compressobj(1, zlib.DEFLATED, -15)
            self.body_raw[state] = raw
            self.body_deflate[state] = compressor.compress(raw) + compressor.flush()

    def png(self):
        bits = encode_bits(time.time_ns() // 1000 - self.age_us)
        row = bytearray([0])
        for bit in bits:
            row += (b"\xff\xff\xff" if bit else b"\x00\x00\x00") * CELL
        row += b"\x00\x00\x00" * (SCREEN_W - NEED_W)
        strip = bytes(row) * STRIP_HEIGHT
        compressor = zlib.compressobj(1, zlib.DEFLATED, -15)
        head = compressor.compress(strip) + compressor.flush(zlib.Z_FULL_FLUSH)
        adler = zlib.adler32(self.body_raw[self.block_on], zlib.adler32(strip)) & 0xFFFFFFFF
        idat = b"\x78\x01" + head + self.body_deflate[self.block_on] + struct.pack(">I", adler)
        return (b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">IIBBBBB", SCREEN_W, SCREEN_H, 8, 2, 0, 0, 0))
                + png_chunk(b"IDAT", idat) + png_chunk(b"IEND", b""))


class FakePainter:
    """A stand-in for the painter's loopback endpoints (fake mode)."""

    def __init__(self, port, interval_ms):
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        painter = self
        self.started = time.time()
        self.interval_ms = interval_ms
        self.toggles = 0

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                micros = time.time_ns() // 1000
                if self.path.startswith("/stats"):
                    draws = int((time.time() - painter.started) * 1000 / painter.interval_ms)
                    body = json.dumps({"draws": draws, "max_draw_us": 1000, "late_wakeups": 0, "toggles": painter.toggles,
                                       "scroll_step": 24, "interval_ms": painter.interval_ms, "width": SCREEN_W,
                                       "height": SCREEN_H, "now_us": micros})
                else:
                    body = str(micros)
                self.send_response(200)
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body.encode())

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def painter_get(port, path="/t", timeout=3):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        conn.request("GET", path)
        return conn.getresponse().read().decode()
    finally:
        conn.close()


def decode_native(image, y):
    """Decode one copy of the bar code from a PIL image row."""
    rgb = image.convert("RGB")
    if rgb.width < NEED_W or rgb.height <= y:
        return -3
    bits = []
    for i in range(CELLS):
        r, g, b = rgb.getpixel((i * CELL + CELL // 2, y))
        bits.append(1 if r + g + b >= 384 else 0)
    return decode_bits(bits)


def password_command(name):
    if os.name != "nt":
        return "printenv " + name
    script = f"[Console]::Write([Environment]::GetEnvironmentVariable('{name}'))"
    return "powershell.exe -NoProfile -NonInteractive -EncodedCommand " + base64.b64encode(script.encode("utf-16le")).decode("ascii")


PASSWORD_ENV = "PERF_PASSWORD_A"


def credentials_from_env():
    prefix = "RDPILOT_PERF_A_"
    creds = {key: os.environ.get(prefix + key.upper()) for key in ("host", "port", "username", "password", "domain")}
    missing = [prefix + k.upper() for k in ("host", "username", "password") if not creds[k]]
    require(not missing, "missing environment: " + ", ".join(missing))
    return {k: v for k, v in creds.items() if v}


def commit_id():
    if os.environ.get("GITHUB_SHA"):
        return os.environ["GITHUB_SHA"]
    try:
        done = subprocess.run(["git", "rev-parse", "HEAD"], cwd=HERE, capture_output=True, text=True, timeout=20)
        return done.stdout.strip() or "unknown"
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"


# --- the run -------------------------------------------------------------------------------

class Perf:
    def __init__(self, args):
        self.args = args
        self.fake = args.fake
        self.window_s = args.window_s or (5 if self.fake else 60)
        self.warmup_s = args.warmup_s if args.warmup_s is not None else (1 if self.fake else 30)
        self.rates = [float(v) for v in (args.recorder_fps or ("4" if self.fake else "4,8")).split(",")]
        # Read credentials before creating anything, so a missing variable leaves no partial evidence.
        self.credentials = {} if self.fake else credentials_from_env()
        self.bin = Path(args.bin_dir).resolve()
        self.output = Path(args.output).resolve()
        self.output.mkdir(parents=True, exist_ok=False, mode=0o700)
        self.temp = Path(tempfile.mkdtemp(prefix="rdpilot-perf-proof-"))
        self.token = None
        self.checks = []
        self.summary = {"mode": "fake" if self.fake else "live", "status": "running", "checks": self.checks,
                        "unverified": [], "notes": []}
        self.result = {"schema": SCHEMA, "mode": self.summary["mode"], "status": "running", "commit": commit_id(),
                       "environment": {}, "workloads": {"w1": {"status": "not_run", "passes": []}}}
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
            (self.temp / "cache").mkdir(mode=0o700, exist_ok=True)
            self.env["XDG_CACHE_HOME"] = str(self.temp / "cache")
            if args.bundle:
                self.env["RDPILOT_BUNDLE_PATH"] = str(Path(args.bundle).resolve())
        self.daemon = self.viewer = self.painter = self.frames = None
        self.urls = []

    # --- evidence ------------------------------------------------------------------

    def secrets(self):
        return [self.credentials["password"]] if self.credentials.get("password") else []

    def clean(self, text):
        for secret in self.secrets():
            text = text.replace(secret, "[REDACTED]")
        if self.token:
            text = text.replace(self.token, "<token>")
        return text

    def note(self, text):
        print(text, flush=True)
        self.summary["notes"].append(text)

    def check(self, name, **data):
        self.checks.append({"check": name, "passed": True, **data})
        self.save()
        print(name + ": passed", flush=True)

    def save(self):
        (self.output / "summary.json").write_text(self.clean(json.dumps(self.summary, indent=2, default=str)))

    def write_json(self, name, data):
        (self.output / name).write_text(self.clean(json.dumps(data, default=str)))

    def write_png(self, name, data_url):
        if data_url:
            (self.output / name).write_bytes(base64.b64decode(data_url.split(",", 1)[1]))

    def scan_evidence(self):
        needles = [s.encode() for s in self.secrets()] + ([self.token.encode()] if self.token else [])
        leaks = []
        for path in self.output.rglob("*"):
            if path.is_file():
                data = path.read_bytes()
                leaks += [path.name for needle in needles if needle and needle in data]
        require(not leaks, f"token or credential found in evidence: {sorted(set(leaks))}")
        self.check("evidence_contains_no_token_or_credentials", files=sum(1 for p in self.output.rglob('*') if p.is_file()))

    # --- rdpilot processes ------------------------------------------------------------

    def hosts_file(self):
        def quote(value):
            return '"' + str(value).replace("\\", "\\\\").replace('"', '\\"') + '"'
        if self.fake:
            lines = ["Host a", '  HostName "fake-a"', '  User "fake"', "  PasswordCommand " + quote(password_command(PASSWORD_ENV)),
                     "  CuaEnabled no"]
        else:
            c = self.credentials
            lines = ["Host a", "  HostName " + quote(c["host"]), "  User " + quote(c["username"]),
                     "  PasswordCommand " + quote(password_command(PASSWORD_ENV)), "  Port " + str(c.get("port", 3389))]
            if c.get("domain"):
                lines.append("  Domain " + quote(c["domain"]))
            lines += ["  AcceptInvalidCerts yes", "  CuaEnabled no"]
        path = self.temp / "hosts"
        path.write_text("\n".join(lines) + "\n")
        path.chmod(0o600)
        return path

    async def cli(self, *arguments, password=False, timeout=90, allow_failure=False):
        env = dict(self.env)
        if password:
            env[PASSWORD_ENV] = "fake" if self.fake else self.credentials["password"]
        proc = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot"), *arguments, "--json", env=env,
                                                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
        try:
            out, err = await asyncio.wait_for(proc.communicate(), timeout)
        except BaseException:
            proc.kill()
            await proc.wait()
            raise
        if proc.returncode and not allow_failure:
            raise PerfError(self.clean(f"rdpilot {arguments[0]} failed: {out.decode(errors='replace')} {err.decode(errors='replace')}"))
        text = out.decode(errors="replace").strip()
        try:
            return json.loads(text) if text else {}
        except ValueError:
            return {"raw": self.clean(text), "exit_code": proc.returncode}

    async def start_daemon(self):
        log = open(self.output / "daemon.log", "a")
        self.daemon = await asyncio.create_subprocess_exec(str(self.bin / DAEMON), env=self.env, stdout=log, stderr=log)
        log.close()
        await asyncio.sleep(1)

    async def start_viewer(self):
        stderr = open(self.output / "view.stderr", "a")
        options = {"creationflags": subprocess.CREATE_NEW_CONSOLE} if os.name == "nt" else {}
        self.viewer = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot"), "view", "--json", "--bind", "loopback",
                                                           env=self.env, stdout=asyncio.subprocess.PIPE, stderr=stderr, **options)
        stderr.close()
        text, deadline = "", time.monotonic() + 20
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
        require(self.urls[0].split("//", 1)[1].startswith("127.0.0.1:"), "loopback URL first")
        self.check("viewer_started", urls=len(self.urls))

    async def stop_pass_processes(self):
        if self.viewer and self.viewer.returncode is None:
            self.viewer.kill()
            await self.viewer.wait()
        self.viewer = None
        if self.daemon is None:
            return
        try:
            await self.cli("disconnect", "--session", "a", timeout=20, allow_failure=True)
        except (OSError, asyncio.TimeoutError, PerfError):
            pass
        if self.daemon.returncode is None:
            self.daemon.terminate()
            try:
                await asyncio.wait_for(self.daemon.wait(), 5)
            except asyncio.TimeoutError:
                self.daemon.kill()
                await self.daemon.wait()
        self.daemon = None

    # --- painter -------------------------------------------------------------------------

    async def painter_stats(self):
        return json.loads(await asyncio.to_thread(painter_get, self.args.painter_port, "/stats"))

    async def painter_up(self, timeout):
        """True once the painter answers and its draw counter grows."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                first = await self.painter_stats()
                await asyncio.sleep(1)
                if (await self.painter_stats())["draws"] > first["draws"]:
                    return True
            except (OSError, ValueError, KeyError):
                await asyncio.sleep(1)
        return False

    async def start_painter_by_hand(self):
        """The logon entry did not start the painter: type its command into the Run dialog."""
        directory = self.args.painter_dir or str(HERE / "perf")
        command = (f'powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "{directory}\\painter.ps1"'
                   f' -User {self.credentials["username"]} -Port {self.args.painter_port}'
                   f' -IntervalMs {self.args.painter_interval_ms}')
        await self.cli("input", "key", "--session", "a", "--combo", "win,r", allow_failure=True)
        await asyncio.sleep(3)
        await self.cli("input", "type", "--session", "a", "--text", command, allow_failure=True)
        await asyncio.sleep(1)
        await self.cli("input", "key", "--session", "a", "--combo", "enter", allow_failure=True)

    async def native_first_decode(self, name):
        """Native screenshots decoded in Python until one carries a valid bar code (live only)."""
        try:
            from PIL import Image
        except ImportError:
            self.summary["unverified"].append("native screenshot decode (Pillow not available)")
            return None
        rows, deadline = [], time.monotonic() + 60
        while time.monotonic() < deadline:
            path = self.output / name
            await self.cli("screenshot", "--session", "a", "--output", str(path), timeout=60)
            with Image.open(path) as image:
                image.load()
                values = [decode_native(image, y) for y in ROW_YS]
                rows.append({"values_ok": [v >= 0 for v in values], "size": list(image.size)})
            if any(v >= 0 for v in values):
                break
            await asyncio.sleep(2)
        return rows[-3:]

    # --- browser -------------------------------------------------------------------------

    async def wait_page(self, page, predicate, what, timeout):
        deadline = time.monotonic() + timeout
        while True:
            state = await page.evaluate("() => JSON.parse(JSON.stringify(window.rdpilotViewer))")
            if predicate(state):
                return state
            require(time.monotonic() < deadline, f"timed out waiting for {what}: {self.clean(json.dumps(state))[:1500]}")
            await asyncio.sleep(0.1)

    @property
    def clock_url(self):
        return f"http://127.0.0.1:{self.args.painter_port}/t"

    async def fake_route(self, route):
        """Answer a frame long-poll after the hold with a synthetic frame (fake mode)."""
        try:
            await asyncio.sleep(FAKE_HOLD_MS / 1000)
            self.frames.seq += 1
            await route.fulfill(status=200, content_type="image/png", body=self.frames.png(), headers={
                "x-frame-seq": str(self.frames.seq), "x-frame-width": str(SCREEN_W), "x-frame-height": str(SCREEN_H),
                "x-frame-encode-ms": str(FAKE_ENCODE_MS), "cache-control": "no-store"})
        except Exception:  # noqa: BLE001 - the page or browser closed while the poll was held
            pass

    def watch_responses(self, page, label, ctx):
        async def handle(response):
            arrived = time.time()
            try:
                path = response.url.split("?", 1)[0]
                if "/api/sessions/" not in path or not path.endswith("/frame") or response.status != 200:
                    return
                headers = await response.all_headers()
                ctx["responses"][label].append({
                    "t": arrived, "seq": int(headers["x-frame-seq"]), "encode_ms": int(headers["x-frame-encode-ms"]),
                    "width": int(headers["x-frame-width"]), "height": int(headers["x-frame-height"])})
            except Exception:  # noqa: BLE001 - a response that went away is not a sample
                pass
        page.on("response", lambda response: ctx["tasks"].append(asyncio.ensure_future(handle(response))))

    async def open_viewer(self, browser, label, ctx):
        context = await browser.new_context(viewport={"width": 1600, "height": 1000})
        if self.fake:
            await context.route("**/api/sessions/*/frame*", self.fake_route)
        page = await context.new_page()
        ctx["page_errors"][label] = []
        page.on("pageerror", lambda error: ctx["page_errors"][label].append(str(error)))
        self.watch_responses(page, label, ctx)
        await page.goto(self.urls[0])
        await self.wait_page(page, lambda s: "a" in {x["id"] for x in s["list"]}, "session list", 30)
        await page.click('button[data-session="a"]')
        state = await self.wait_page(page, lambda s: s["sessions"].get("a", {}).get("seq", 0) > 0, "first frame", 60)
        ctx["first_frame"][label] = {k: state["sessions"]["a"].get(k) for k in ("seq", "width", "height", "state")}
        # The viewer page may only fetch its own origin: a second page of the same
        # context, on the painter's origin, makes the clock round trips.
        clock = await context.new_page()
        await clock.goto(self.clock_url)
        return page, clock

    async def clock_loop(self, page, until, samples):
        """One in-page round trip to the painter clock every second until `until`."""
        while time.time() < until:
            samples.extend(await page.evaluate(CLOCK_BATCH_JS, [self.clock_url, 1, 0]))
            await asyncio.sleep(1)

    async def cpu_loop(self, meter, until, samples):
        while time.time() < until:
            cpu, working_set, _ = meter.read()
            samples.append((time.monotonic(), cpu, working_set))
            await asyncio.sleep(1)

    async def cross_check(self, pages, painter):
        """Wall time from a key press to the first frame that shows the toggled block (never pooled)."""
        x, y = BLOCK_PROBE
        results = {label: [] for label in pages}
        for _ in range(self.args.cross_checks):
            current = {label: await page.evaluate(BLOCK_STATE_JS, [PANEL_CANVAS, x, y]) for label, page in pages.items()}
            wants = {label: "off" if state == "on" else "on" for label, state in current.items()}
            watches = {label: asyncio.ensure_future(page.evaluate(WATCH_BLOCK_JS, [PANEL_CANVAS, x, y, wants[label], 15000]))
                       for label, page in pages.items()}
            await asyncio.sleep(0.3)
            if self.fake:
                self.frames.block_on = not self.frames.block_on
                painter.toggles += 1
            started = time.time() * 1000
            await self.cli("input", "key", "--session", "a", "--combo", "space", allow_failure=True, timeout=30)
            returned = time.time() * 1000
            for label, watch in watches.items():
                seen = (await watch)["t"]
                results[label].append(None if seen is None else {"from_cli_start_ms": seen - started,
                                                                  "from_cli_return_ms": seen - returned})
            await asyncio.sleep(2)
        return results

    async def read_manifest(self, rec_dir, rec_id, timeout=60):
        path = rec_dir / rec_id / "manifest.json"
        deadline = time.monotonic() + timeout
        while True:
            try:
                manifest = json.loads(path.read_text())
            except (OSError, ValueError):
                manifest = None
            if manifest and manifest.get("stats") is not None and manifest.get("end_reason"):
                return manifest
            require(time.monotonic() < deadline, "the recording manifest has no stats after the recording stopped")
            await asyncio.sleep(0.5)

    # --- one W1 pass -----------------------------------------------------------------------

    async def run_pass(self, fps, index):
        from playwright.async_api import async_playwright

        tag = f"p{fps:g}"
        rec_dir = self.temp / f"recordings-{tag}"
        rec_dir.mkdir(mode=0o700)
        self.env["RDPILOT_RECORDING__MAX_FPS"] = f"{fps:.1f}"
        self.env["RDPILOT_RECORDING__DIR"] = str(rec_dir)
        ctx = {"responses": {"viewer1": [], "viewer2": []}, "tasks": [], "page_errors": {}, "first_frame": {}}
        entry = {"recorder_fps": fps, "window_s": self.window_s, "warmup_s": self.warmup_s}
        self.note(f"pass {index + 1}: recorder at {fps:g} fps")
        try:
            await self.start_daemon()
            connected = await self.cli("connect", "a", "-F", str(self.hosts_file()), "--name", "a", password=True,
                                       timeout=self.args.connect_timeout)
            require(connected.get("session") == "a", f"connect a: {self.clean(json.dumps(connected))[:500]}")
            self.check("a.connected", cua="disabled")
            if self.fake:
                up = await self.painter_up(10)
            else:
                up = await self.painter_up(150)
                if not up:
                    self.note("painter not up after 150 s: starting it from the Run dialog")
                    await self.start_painter_by_hand()
                    up = await self.painter_up(90)
            require(up, "painter clock endpoint unreachable")
            painter_start = await self.painter_stats()
            entry["resolution"] = [painter_start.get("width"), painter_start.get("height")]
            if not self.fake:
                entry["native_first_decode"] = await self.native_first_decode(f"native-first-{tag}.png")
            await self.start_viewer()
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch(headless=True)
                try:
                    pages, clocks = {}, {}
                    for label in ("viewer1", "viewer2"):
                        pages[label], clocks[label] = await self.open_viewer(browser, label, ctx)
                    entry["browser"] = {"name": "chromium", "version": browser.version, "headless": True}
                    entry["first_frame"] = ctx["first_frame"]
                    await asyncio.sleep(self.warmup_s)
                    entry["date_now_step_ms"] = {label: await page.evaluate(GRANULARITY_JS) for label, page in clocks.items()}
                    before = {label: await page.evaluate(CLOCK_BATCH_JS, [self.clock_url, 60, 20]) for label, page in clocks.items()}
                    window = await self.measure_window(tag, rec_dir, pages, clocks, ctx)
                    after = {label: await page.evaluate(CLOCK_BATCH_JS, [self.clock_url, 60, 20]) for label, page in clocks.items()}
                    cross = await self.cross_check(pages, self.painter if self.fake else None)
                    for label, page in pages.items():
                        if index == 0:
                            self.write_png(f"strip-{tag}-{label}.png", await page.evaluate(
                                STRIP_PNG_JS, [PANEL_CANVAS, NEED_W + 48, STRIP_HEIGHT]))
                    await asyncio.gather(*ctx["tasks"], return_exceptions=True)
                finally:
                    await browser.close()
            painter_end = await self.painter_stats()
            entry.update(self.assemble_pass(tag, fps, window, before, after, cross, ctx, painter_start, painter_end))
        finally:
            await self.stop_pass_processes()
        await asyncio.sleep(2 if self.fake else 8)
        return entry

    async def measure_window(self, tag, rec_dir, pages, clocks, ctx):
        """Record, sample and meter for the steady-state window; return the raw window data."""
        meter = ProcessMeter(self.daemon.pid)
        try:
            started = await self.cli("record", "start", "--session", "a", timeout=60)
            rec_id = started.get("id")
            require(rec_id, f"record start: {self.clean(json.dumps(started))[:500]}")
            config = {"cell": CELL, "cells": CELLS, "rowYs": list(ROW_YS), "id": "a", "selector": PANEL_CANVAS}
            cpu0, ws0, _ = meter.read()
            mono0 = time.monotonic()
            for page in pages.values():
                await page.evaluate(SAMPLER_JS, config)
            t0 = time.time()
            until = t0 + self.window_s
            during = {label: [] for label in clocks}
            meter_samples = [(mono0, cpu0, ws0)]
            await asyncio.gather(self.cpu_loop(meter, until, meter_samples),
                                 *[self.clock_loop(page, until, during[label]) for label, page in clocks.items()])
            await asyncio.sleep(max(0.0, until - time.time()))
            cpu1, ws1, _ = meter.read()
            mono1 = time.monotonic()
            raw = {label: await page.evaluate(STOP_JS) for label, page in pages.items()}
            t1 = time.time()
            _, _, peak_ws = meter.read()
            stopped = await self.cli("record", "stop", "--session", "a", timeout=60)
            require(stopped.get("changed") is True, f"record stop: {self.clean(json.dumps(stopped))[:500]}")
            manifest = await self.read_manifest(rec_dir, rec_id)
            meter_samples.append((mono1, cpu1, ws1))
        finally:
            meter.close()
        for label, data in raw.items():
            self.write_json(f"samples-{tag}-{label}.json", data)
        self.write_json(f"clock-during-{tag}.json", during)
        self.write_json(f"manifest-{tag}.json", manifest)
        return {"t0": t0, "t1": t1, "raw": raw, "during": during, "manifest": manifest, "cpu": (cpu0, cpu1),
                "mono": (mono0, mono1), "ws_samples": [s[2] for s in meter_samples], "ws_peak_lifetime": peak_ws}

    def assemble_pass(self, tag, fps, window, before, after, cross, ctx, painter_start, painter_end):
        t0, t1 = window["t0"], window["t1"]
        viewers, pooled_ages, pooled = {}, [], {"ticks_with_frame": 0, "ticks_decoded": 0, "torn_frames": 0, "distinct_frames": 0}
        clock = {}
        for label, data in window["raw"].items():
            ticks, ages = analyze_ticks(data)
            frames = [r for r in ctx["responses"][label] if t0 <= r["t"] <= t1]
            ticks.update({
                "frame_age_ms": dist(ages),
                "frames_received": len(frames),
                "frame_rate_hz": len(frames) / (t1 - t0) if t1 > t0 else None,
                "encode_ms": dist([r["encode_ms"] for r in frames]),
                "page_errors": ctx["page_errors"].get(label, []),
            })
            viewers[label] = ticks
            pooled_ages += ages
            for key in pooled:
                pooled[key] += ticks[key]
            clock[label] = {"all": analyze_clock(before[label] + window["during"][label] + after[label]),
                            "before_window": analyze_clock(before[label]), "after_window": analyze_clock(after[label])}
        pooled_out = {"frame_age_ms": dist(pooled_ages), **pooled,
                      "undecodable_ratio": (1 - pooled["ticks_decoded"] / pooled["ticks_with_frame"]) if pooled["ticks_with_frame"] else None}
        stats = window["manifest"]["stats"]
        captured, dropped = stats["frames_captured"], stats["frames_dropped"]
        recorder = {
            "rate_fps": fps, "trigger": window["manifest"].get("trigger"), "end_reason": window["manifest"].get("end_reason"),
            "duration_ms": window["manifest"].get("duration_ms"), "codec": window["manifest"].get("codec"),
            "frames_captured": captured, "frames_encoded": stats["frames_encoded"], "frames_dropped": dropped,
            "dropped_ratio": dropped / captured if captured else None, "gap_periods": stats["gap_periods"],
            "capture_to_take_ms_p50": stats["capture_to_take_ms_p50"], "capture_to_take_ms_p95": stats["capture_to_take_ms_p95"],
            "capture_to_take_ms_max": stats["capture_to_take_ms_max"], "encode_ms_mean": stats["encode_ms_mean"],
            "encode_call_ms_p50": stats["encode_call_ms_p50"], "encode_call_ms_p95": stats["encode_call_ms_p95"],
            "encode_call_ms_max": stats["encode_call_ms_max"], "events_lost": stats["events_lost"],
            "denominator": "frames_captured (captured frames replaced before the encoder took them; not every missed opportunity at the target rate)",
            "boundary": "recording started and stopped at the window edges; the warm-up share is the encoder start only",
        }
        cpu0, cpu1 = window["cpu"]
        wall = window["mono"][1] - window["mono"][0]
        daemon = {
            "cpu_seconds": cpu1 - cpu0, "wall_s": wall, "cpu_cores": (cpu1 - cpu0) / wall if wall > 0 else None,
            "working_set_window_peak_bytes": max(window["ws_samples"]),
            "working_set_peak_lifetime_bytes": window["ws_peak_lifetime"], "samples": len(window["ws_samples"]),
            "scope": "the daemon process only (RDP client, viewer encode and recorder); painter, RDP server and browsers are outside the figure",
        }
        painter = {"start": painter_start, "end": painter_end,
                   "draws_in_run": painter_end["draws"] - painter_start["draws"],
                   "late_wakeups_in_run": painter_end["late_wakeups"] - painter_start["late_wakeups"],
                   "max_draw_us": painter_end["max_draw_us"], "interval_ms": painter_end.get("interval_ms"),
                   "toggles_in_run": painter_end.get("toggles", 0) - painter_start.get("toggles", 0)}
        return {"window": {"start_unix_ms": round(t0 * 1000), "end_unix_ms": round(t1 * 1000), "measured_s": t1 - t0},
                "viewers": viewers, "pooled": pooled_out,
                "cross_check_ms": {**cross, "note": "taken after the window; not pooled with the pixel-clock samples"},
                "clock_offset_ms": clock, "recorder": recorder, "daemon": daemon, "painter": painter}

    # --- checks ------------------------------------------------------------------------------

    def check_pass(self, entry):
        viewers, fake = entry["viewers"], self.fake
        for label, v in viewers.items():
            require(not v["page_errors"], f"{label}: page errors {v['page_errors'][:2]}")
        pooled = entry["pooled"]
        for label, v in viewers.items():
            require(v["ticks_decoded"] > 0 and v["frame_age_ms"]["n"] > 0, f"{label}: no decoded sample")
        extra = {}
        if fake:
            for label, v in viewers.items():
                age = v["frame_age_ms"]
                require(v["undecodable_ticks"] == 0, f"{label}: {v['undecodable_ticks']} undecodable synthetic ticks")
                require(age["min"] >= FAKE_AGE_MS - 3, f"{label}: minimum age {age['min']} below the injected {FAKE_AGE_MS} ms")
                require(FAKE_AGE_MS <= age["p50"] <= FAKE_AGE_MS + FAKE_HOLD_MS + 250, f"{label}: p50 {age['p50']}")
                require(FAKE_AGE_MS <= age["p95"] <= FAKE_AGE_MS + FAKE_HOLD_MS + 250, f"{label}: p95 {age['p95']}")
                require(age["p95"] >= age["p50"] >= age["min"], f"{label}: percentiles out of order")
            extra = {"injected_age_ms": FAKE_AGE_MS, "hold_ms": FAKE_HOLD_MS}
        self.check("w1.frame_age_sampled_on_every_tick", pooled_n=pooled["frame_age_ms"]["n"],
                   undecodable_ratio=pooled["undecodable_ratio"], **extra)

        for label, v in viewers.items():
            require(v["distinct_frames"] > 0 and v["ticks_with_frame"] >= v["distinct_frames"], f"{label}: tick and frame counts")
            if fake:
                require(v["ticks_with_frame"] > 2 * v["distinct_frames"], f"{label}: ticks do not outnumber frames (no stale sampling)")
        self.check("w1.stale_frames_counted", ticks={k: v["ticks_with_frame"] for k, v in viewers.items()},
                   distinct_frames={k: v["distinct_frames"] for k, v in viewers.items()})

        for label, v in viewers.items():
            require(v["frames_received"] > 0 and v["encode_ms"]["n"] > 0, f"{label}: no frame response in the window")
            if fake:
                require(v["encode_ms"]["min"] == v["encode_ms"]["max"] == FAKE_ENCODE_MS, f"{label}: encode ms {v['encode_ms']}")
                rate = v["frame_rate_hz"]
                require(1000 / (FAKE_HOLD_MS + 250) <= rate <= 1000 / FAKE_HOLD_MS * 1.05, f"{label}: frame rate {rate}")
        self.check("w1.encode_time_and_frame_rate_per_viewer",
                   frame_rate_hz={k: v["frame_rate_hz"] for k, v in viewers.items()},
                   encode_ms_p50={k: v["encode_ms"]["p50"] for k, v in viewers.items()})

        for label, c in entry["clock_offset_ms"].items():
            require(c["all"]["samples"] > 0, f"{label}: no clock sample")
            if fake:
                # The stand-in clock server is slow; the bound applies to the samples that were fast enough.
                if c["all"]["samples_rtt_le_5ms"]:
                    require(c["all"]["max_abs_offset_ms"] <= 10, f"{label}: clock offset {c['all']['max_abs_offset_ms']} ms")
            else:
                require(c["all"]["samples_rtt_le_5ms"] > 0, f"{label}: no clock sample with a round trip of at most 5 ms")
        self.check("w1.clock_offset_bounded", max_abs_offset_ms={k: c["all"]["max_abs_offset_ms"] for k, c in entry["clock_offset_ms"].items()})

        cross = entry["cross_check_ms"]
        taken = {label: sum(1 for s in cross[label] if s) for label in viewers}
        if fake:
            for label in viewers:
                require(taken[label] == self.args.cross_checks, f"{label}: {taken[label]} cross-check samples")
                for sample in cross[label]:
                    require(0 <= sample["from_cli_start_ms"] <= FAKE_HOLD_MS + 1500, f"{label}: cross-check {sample}")
        elif not any(taken.values()):
            self.summary["unverified"].append("input-to-change cross-check (no key press showed in the viewer)")
        self.check("w1.cross_check_recorded", attempted=self.args.cross_checks, samples=taken)

        daemon = entry["daemon"]
        require(daemon["cpu_cores"] is not None and math.isfinite(daemon["cpu_cores"]) and daemon["cpu_cores"] >= 0,
                f"daemon cpu {daemon['cpu_cores']}")
        require(daemon["working_set_window_peak_bytes"] > 0 and daemon["samples"] >= 2, "daemon working set not sampled")
        self.check("w1.daemon_cpu_and_memory_sampled", cpu_cores=daemon["cpu_cores"], samples=daemon["samples"])

        recorder = entry["recorder"]
        require(recorder["frames_captured"] > 0, "the recorder captured no frame in the window")
        require(recorder["dropped_ratio"] == recorder["frames_dropped"] / recorder["frames_captured"], "dropped ratio")
        self.check("w1.recorder_counters_and_ratio", frames_captured=recorder["frames_captured"],
                   frames_dropped=recorder["frames_dropped"], dropped_ratio=recorder["dropped_ratio"])

    REQUIRED = {
        "pass": ("recorder_fps", "window_s", "warmup_s", "window", "viewers", "pooled", "cross_check_ms", "clock_offset_ms",
                 "recorder", "daemon", "painter", "browser", "resolution"),
        "viewer": ("frame_age_ms", "ticks_with_frame", "ticks_decoded", "undecodable_ratio", "distinct_frames", "torn_frames",
                   "torn_ticks", "frames_received", "frame_rate_hz", "encode_ms"),
        "recorder": ("frames_captured", "frames_encoded", "frames_dropped", "dropped_ratio"),
        "daemon": ("cpu_cores", "working_set_window_peak_bytes", "working_set_peak_lifetime_bytes"),
    }

    def check_result_fields(self, passes):
        missing = []
        for entry in passes:
            missing += [f"pass.{k}" for k in self.REQUIRED["pass"] if k not in entry]
            for label, v in entry["viewers"].items():
                missing += [f"{label}.{k}" for k in self.REQUIRED["viewer"] if k not in v]
            missing += [f"recorder.{k}" for k in self.REQUIRED["recorder"] if k not in entry["recorder"]]
            missing += [f"daemon.{k}" for k in self.REQUIRED["daemon"] if k not in entry["daemon"]]
            require(all(entry["pooled"]["frame_age_ms"].get(k, 0) is not None for k in ("n", "p50", "p95", "max")), "pooled percentiles")
        env = self.result["environment"]
        missing += [f"environment.{k}" for k in ("vcpu", "memory_mb", "resolution", "browser", "painter_interval_ms") if not env.get(k)]
        require(not missing, "result fields missing: " + ", ".join(missing))
        self.check("w1.result_has_every_field", passes=len(passes))

    # --- orchestration --------------------------------------------------------------------------

    def environment(self, passes):
        first = passes[0] if passes else {}
        stats = (first.get("painter") or {}).get("end") or {}
        return {
            "image_os": os.environ.get("ImageOS"), "image_version": os.environ.get("ImageVersion"),
            "platform": platform.platform(), "python": platform.python_version(),
            "vcpu": os.cpu_count(), "memory_mb": memory_mb(),
            "resolution": first.get("resolution"), "browser": first.get("browser"),
            "painter_interval_ms": stats.get("interval_ms"), "painter_interval_ms_requested": self.args.painter_interval_ms,
            "painter_scroll_step_px": stats.get("scroll_step"),
            "powershell_5_1": powershell_text("$PSVersionTable.PSVersion.ToString()"),
            "video": powershell_text("(Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }) -join '; '"),
            "recorder_rates_fps": self.rates, "cua": "disabled for the session",
            "graphics": "rdpilot product defaults",
        }

    async def execute(self):
        workloads = self.args.workload
        passes = []
        try:
            if self.fake:
                self.painter = FakePainter(self.args.painter_port, self.args.painter_interval_ms)
                self.frames = FakeFrames(FAKE_AGE_MS)
            if workloads in ("all", "w1"):
                for index, fps in enumerate(self.rates):
                    entry = await self.run_pass(fps, index)
                    self.check_pass(entry)
                    passes.append(entry)
                    self.result["workloads"]["w1"] = {"status": "measured", "passes": passes}
                self.result["environment"] = self.environment(passes)
                self.check_result_fields(passes)
            for key, entry in SKIPPED.items():
                if workloads in ("all", "w1", key):
                    self.result["workloads"][key] = dict(entry)
            self.check("w2_w4.skipped_with_reason", workloads=sorted(SKIPPED))
            if workloads in ("all", "w1"):
                self.summary["status"] = "passed"
                self.result["status"] = "measured"
            else:
                self.summary["status"] = "skipped"
                self.result["status"] = "skipped"
        except BaseException as error:
            self.summary["status"] = "failed"
            self.result["status"] = "failed"
            self.summary["failure"] = self.clean(f"{type(error).__name__}: {error}")
            raise
        finally:
            await self.stop_pass_processes()
            if self.painter:
                self.painter.close()
            for path in (self.output / "daemon.log", self.output / "view.stderr"):
                if path.exists():
                    path.write_text(self.clean(path.read_text(errors="replace")))
            self.write_json("perf-result.json", self.result)
            self.save()
            shutil.rmtree(self.temp, ignore_errors=True)
        self.scan_evidence()
        return 0 if self.summary["status"] == "passed" else 2


# --- self test -------------------------------------------------------------------------------

def inflate_png(data):
    """Raw scanlines of a stdlib-built RGB PNG (self test)."""
    require(data[:8] == b"\x89PNG\r\n\x1a\n", "png signature")
    position, idat = 8, b""
    while position < len(data):
        length, kind = struct.unpack(">I4s", data[position:position + 8])
        body = data[position + 8:position + 8 + length]
        require(struct.unpack(">I", data[position + 8 + length:position + 12 + length])[0] == zlib.crc32(kind + body), "png crc")
        if kind == b"IDAT":
            idat += body
        position += 12 + length
    return zlib.decompress(idat)  # raises on a wrong adler checksum


def selftest():
    require(crc16(b"123456789") == 0x29B1, "crc16 check value")
    for value in (0, 1, MODULUS - 1, 1760000000123456):
        bits = encode_bits(value)
        require(decode_bits(bits) == value & (MODULUS - 1), f"round trip {value}")
        flipped = list(bits)
        flipped[20] ^= 1
        require(decode_bits(flipped) == -2, "a flipped time cell fails the checksum")
        broken = list(bits)
        broken[0] ^= 1
        require(decode_bits(broken) == -1, "a broken sync fails")
    now_ms = 1760000000123
    require(full_micros(now_ms, (now_ms * 1000 - 5000) & (MODULUS - 1)) == now_ms * 1000 - 5000, "full_micros")
    require(full_micros((MODULUS + 2000) // 1000, MODULUS - 1000) == MODULUS - 1000, "full_micros across the modulus")
    require(full_micros((MODULUS + 2000) // 1000, 500) == MODULUS + 500, "full_micros after the modulus")
    values = list(range(1, 101))
    require((percentile(values, 0.5), percentile(values, 0.95), percentile(values, 1.0)) == (50, 95, 100), "percentiles")
    require(dist([])["n"] == 0 and dist([])["p95"] is None, "empty distribution")
    require(dist([1, 2, 3])["mean"] == 2, "mean")
    # Ticks: valid equal copies, a torn pair, a failed pair, no canvas.
    base = 1760000000000
    value = (base * 1000 - 40000) & (MODULUS - 1)
    data = {"ts": [0, 16, 33, 50, 66, 83], "now": [base] * 6, "seq": [1, 1, 2, 2, 2, 2],
            "a": [value, value, value, -1, -3, value], "b": [value, -2, value + 20000, -1, -3, -3], "cost": [0.1] * 6}
    summary, ages = analyze_ticks(data)
    require(summary["ticks_with_frame"] == 5 and summary["ticks_decoded"] == 4 and summary["undecodable_ticks"] == 1, "tick counts")
    require(summary["torn_ticks"] == 1 and summary["torn_frames"] == 1 and summary["distinct_frames"] == 2, "torn and distinct frames")
    require(len(ages) == 4 and all(abs(age - 40.0) < 1e-6 for age in ages), f"ages {ages}")
    offsets = analyze_clock([(100, 102, 101500, 2.0, 0, 0), (100, 130, 120000, 30.0, 0, 0)])
    require(offsets["samples_rtt_le_5ms"] == 1 and abs(offsets["median_offset_ms"] - 500 / 1000.0) < 1e-9, "clock offset")
    # The synthetic frame: valid PNG, both copies decode to the injected age, the block follows its state.
    frames = FakeFrames(FAKE_AGE_MS)
    for state, colour in ((False, BLOCK_OFF), (True, BLOCK_ON)):
        frames.block_on = state
        before = time.time_ns() // 1000
        raw = inflate_png(frames.png())
        after = time.time_ns() // 1000
        require(len(raw) == SCREEN_H * (1 + 3 * SCREEN_W), "png size")
        for y in ROW_YS:
            start = y * (1 + 3 * SCREEN_W) + 1
            cells = [1 if sum(raw[start + (i * CELL + CELL // 2) * 3:start + (i * CELL + CELL // 2) * 3 + 3]) >= 384 else 0
                     for i in range(CELLS)]
            painted = decode_bits(cells)
            elapsed = (painted - (before - FAKE_AGE_MS * 1000) + 1) % MODULUS
            require(painted >= 0 and elapsed <= after - before + 2, "synthetic bar code time")
        x, y = BLOCK_PROBE
        offset = y * (1 + 3 * SCREEN_W) + 1 + 3 * x
        require(tuple(raw[offset:offset + 3]) == colour, "block colour follows the state")
    meter = ProcessMeter(os.getpid())
    try:
        cpu_a, working_set, peak = meter.read()
        sum(i * i for i in range(300000))
        cpu_b, _, _ = meter.read()
    finally:
        meter.close()
    require(cpu_b >= cpu_a >= 0 and working_set > 0 and peak >= working_set, "process meter")
    print("selftest passed")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fake", action="store_true", help="offline run against the fake connector and synthetic frames")
    parser.add_argument("--selftest", action="store_true", help="check the bar code, statistics, synthetic frames and meter; no binaries")
    parser.add_argument("--bin-dir", help="directory with rdpilot and rdpilot-daemon")
    parser.add_argument("--bundle", help="live mode: daemon bundle_path directory")
    parser.add_argument("--output", help="new evidence directory; must not already exist")
    parser.add_argument("--workload", choices=["all", "w1", "w2", "w3", "w4"], default="all",
                        help="all or w1 measures W1; w2, w3 or w4 alone writes the skip entry and exits with code 2")
    parser.add_argument("--window-s", type=int, help="steady-state window in seconds (default 60; fake 5)")
    parser.add_argument("--warmup-s", type=int, help="warm-up before the window in seconds (default 30; fake 1)")
    parser.add_argument("--recorder-fps", help="comma-separated recorder rates, one pass each (default 4,8; fake 4)")
    parser.add_argument("--cross-checks", type=int, default=5, help="key presses for the input-to-change cross-check")
    parser.add_argument("--painter-port", type=int, default=47653)
    parser.add_argument("--painter-interval-ms", type=int, default=20)
    parser.add_argument("--painter-dir", help="live mode: directory with PixelClock.cs and painter.ps1 (for the Run dialog fallback)")
    parser.add_argument("--connect-timeout", type=int, default=600)
    parser.add_argument("--allow-no-tailnet", action="store_true",
                        help="accepted for the hosted wrapper; the perf viewer binds loopback only")
    parser.add_argument("--allow-debug", action="store_true", help="live mode: allow debug binaries")
    args = parser.parse_args()
    if args.selftest:
        try:
            return selftest()
        except PerfError as error:
            print(f"selftest failed: {error}")
            return 1
    if not args.bin_dir or not args.output:
        parser.error("--bin-dir and --output are required")
    if args.recorder_fps:
        try:
            rates = [float(v) for v in args.recorder_fps.split(",")]
        except ValueError:
            parser.error("--recorder-fps needs comma-separated numbers")
        if not rates or any(not 0.5 <= r <= 8.0 for r in rates):
            parser.error("--recorder-fps values must be within 0.5 and 8.0")
    if not args.fake and "debug" in Path(args.bin_dir).resolve().parts and not args.allow_debug:
        parser.error("live mode needs release binaries (cargo build --release --workspace)")
    os.umask(0o077)
    try:
        perf = Perf(args)
    except (PerfError, OSError) as error:
        print(f"Performance proof not started: {error}", flush=True)
        return 2
    try:
        code = asyncio.run(perf.execute())
    except (Exception, KeyboardInterrupt) as error:
        print(perf.clean(f"Performance proof failed: {type(error).__name__}: {error}"), flush=True)
        return 1
    print(f"Evidence: {perf.output / 'perf-result.json'}", flush=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
