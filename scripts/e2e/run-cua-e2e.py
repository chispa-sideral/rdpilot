#!/usr/bin/env python3
"""One live RDP-only Cua integration run; requires two independent Windows users.

Build rdpilot-cli, rdpilot-daemon, rdpilot-mcp first. Example:
  python scripts/e2e/run-cua-e2e.py --bin-dir target/debug \
    --bundle /path/bundle --credentials-a /private/a.json \
    --credentials-b /private/b.json --output /private/live-proof

Credentials JSON: host, port, username, password (optional domain). They are
passed only through subprocess environment, never arguments or evidence.
The output is owner-only and contains guest desktop content. This harness
provisions nothing. Two local TCP relays forward existing RDP access and allow
an exact-target connection loss; all guest control uses RDP/native Cua MCP.
"""
import argparse
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import time
import uuid

LIMIT = 16 * 1024 * 1024
POWERSHELL = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"


class ProofError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ProofError(message)


def psquote(text):
    return "'" + text.replace("'", "''") + "'"


def powershell(script):
    return {"path": POWERSHELL, "additional_arguments": [
        "-NoProfile", "-NonInteractive", "-STA", "-EncodedCommand",
        base64.b64encode(script.encode("utf-16le")).decode("ascii"),
    ]}


def objects(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from objects(child)
    elif isinstance(value, list):
        for child in value:
            yield from objects(child)


def structured(result):
    """Use native structured output, with the native text JSON fallback."""
    if "structuredContent" in result:
        return result["structuredContent"]
    for content in result.get("content", []):
        if content.get("type") == "text":
            try:
                return json.loads(content["text"])
            except (ValueError, KeyError):
                pass
    return result


class Relay:
    def __init__(self, credentials):
        self.host = credentials["host"]
        self.port = int(credentials.get("port", 3389))
        self.streams = set()
        self.server = None

    async def start(self):
        self.server = await asyncio.start_server(self.accept, "127.0.0.1", 0)
        return self.server.sockets[0].getsockname()[1]

    async def accept(self, reader, writer):
        upstream = None
        try:
            remote, upstream = await asyncio.wait_for(asyncio.open_connection(self.host, self.port), 15)
            self.streams.update((writer, upstream))

            async def copy(source, dest):
                while data := await source.read(64 * 1024):
                    dest.write(data)
                    await dest.drain()

            tasks = [asyncio.create_task(copy(reader, upstream)), asyncio.create_task(copy(remote, writer))]
            _, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in pending:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        except (OSError, asyncio.TimeoutError):
            pass
        finally:
            for stream in (writer, upstream):
                if stream is not None:
                    self.streams.discard(stream)
                    stream.close()

    async def drop(self):
        streams = list(self.streams)
        for stream in streams:
            stream.close()
        for stream in streams:
            try:
                await asyncio.wait_for(stream.wait_closed(), 3)
            except (OSError, asyncio.TimeoutError):
                pass

    async def close(self):
        await self.drop()
        if self.server:
            self.server.close()
            await self.server.wait_closed()


class Mcp:
    def __init__(self, run, target):
        self.run, self.target = run, target
        self.proc = None
        self.number = 0
        self.tools = {}
        self.label = "same-public-label-on-both-targets"
        self.log = None

    async def start(self):
        generation = uuid.uuid4().hex[:8]
        self.log = open(self.run.output / f"mcp-{self.target}-{generation}.jsonl", "w")
        stderr = open(self.run.output / f"mcp-{self.target}-{generation}.stderr", "w")
        self.proc = await asyncio.create_subprocess_exec(
            str(self.run.bin / "rdpilot-mcp"), "--session", self.target,
            env=self.run.env, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=stderr, limit=LIMIT + 1024,
        )
        stderr.close()
        result = await self.request("initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "rdpilot-live-proof", "version": "1"},
        })
        require("serverInfo" in result, "native initialize has no server identity")
        await self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        listing = await self.request("tools/list", {})
        self.tools = {tool["name"]: tool for tool in listing["tools"]}
        self.run.check(f"{self.target}.native_initialize_tools", tools=len(self.tools), server=result["serverInfo"], serialized_bytes=len(json.dumps(listing)))
        return self

    async def send(self, message):
        payload = json.dumps(message, separators=(",", ":")).encode() + b"\n"
        require(len(payload) <= LIMIT, "MCP input exceeds bounded line size")
        self.proc.stdin.write(payload)
        await asyncio.wait_for(self.proc.stdin.drain(), 5)

    async def request(self, method, params, timeout=65):
        self.number += 1
        request_id = f"{self.target}-{self.number}"
        request = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        self.log.write(self.run.clean(json.dumps({"request": request})) + "\n")
        self.log.flush()
        await self.send(request)
        deadline = time.monotonic() + timeout
        while True:
            line = await asyncio.wait_for(self.proc.stdout.readline(), max(0.1, deadline - time.monotonic()))
            require(line, f"{self.target}: MCP EOF during {method}")
            require(len(line) <= LIMIT, "oversized MCP output")
            message = json.loads(line)
            self.log.write(self.run.clean(json.dumps({"method": method, "response": message})) + "\n")
            self.log.flush()
            if "method" in message and "id" in message:
                # We advertised no client request capabilities; fail explicitly.
                await self.send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "client capability not offered"}})
                continue
            if message.get("id") == request_id and "method" not in message:
                require("error" not in message, f"native {method} error: {message.get('error')}")
                return message["result"]

    async def tool(self, name, arguments=None):
        require(name in self.tools, f"pinned Cua does not advertise {name}")
        args = dict(arguments or {})
        if "session" in self.tools[name].get("inputSchema", {}).get("properties", {}):
            args["session"] = self.label
        result = await self.request("tools/call", {"name": name, "arguments": args})
        require(not result.get("isError"), f"native {name} failed: {structured(result)}")
        return result

    async def closed(self, timeout=45):
        # Keep stdin open deliberately: dead runtime must close the endpoint anyway.
        await asyncio.wait_for(self.proc.wait(), timeout)
        require(self.proc.returncode != 0, "failed attachment exited as success")

    async def stop(self):
        if self.proc and self.proc.returncode is None:
            self.proc.stdin.close()
            try:
                await asyncio.wait_for(self.proc.wait(), 8)
            except asyncio.TimeoutError:
                self.proc.kill()
                await self.proc.wait()
        if self.log:
            self.log.close()


class Run:
    def __init__(self, args):
        self.args = args
        self.bin, self.output = Path(args.bin_dir).resolve(), Path(args.output).resolve()
        self.output.mkdir(parents=True, exist_ok=False, mode=0o700)
        self.token = uuid.uuid4().hex[:12]
        self.temp = Path(tempfile.mkdtemp(prefix="rdpilot-cua-"))
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("RDPILOT_") and k != "RUST_LOG"}
        for directory in ("runtime", "config", "share"):
            (self.temp / directory).mkdir(mode=0o700)
        self.env.update({
            "XDG_RUNTIME_DIR": str(self.temp / "runtime"), "XDG_CONFIG_HOME": str(self.temp / "config"),
            "RDPILOT_BUNDLE_PATH": str(Path(args.bundle).resolve()), "RDPILOT_SHARE_ROOT": str(self.temp / "share"),
            "RDPILOT_DAEMON_SINK_PATH": str(self.temp / "sessions.json"),
            "RDPILOT_DAEMON_IDLE_TIMEOUT_MS": "3600000", "RDPILOT_DAEMON_EMPTY_GRACE_MS": "3600000",
            "RDPILOT_DAEMON_DIAGNOSTICS_PATH": str(self.output / "daemon-stages.jsonl"),
        })
        self.credentials = {"a": json.loads(Path(args.credentials_a).read_text()), "b": json.loads(Path(args.credentials_b).read_text())}
        self.relays, self.endpoints, self.metadata = {}, {}, {}
        self.checks = []
        self.daemon = None
        self.summary = {"status": "running", "checks": self.checks, "unverified": ["UAC/secure-desktop behavior is unsupported; no elevation guarantee"]}

    def clean(self, text):
        for cred in self.credentials.values():
            if cred.get("password"):
                text = text.replace(cred["password"], "[REDACTED]")
        return text

    def check(self, name, **data):
        self.checks.append({"check": name, "passed": True, **data})
        self.save()
        print(name + ": passed", flush=True)

    def save(self):
        (self.output / "summary.json").write_text(self.clean(json.dumps(self.summary, indent=2)))

    async def cli(self, *arguments, target=None, timeout=90, allow_failure=False):
        env = dict(self.env)
        if target:
            env.update({"RDPILOT_" + key.upper(): str(value) for key, value in self.credentials[target].items()})
            env["RDPILOT_HOST"], env["RDPILOT_PORT"] = "127.0.0.1", str(self.relays[target][1])
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
        result = await self.cli("connect", "--name", target, "--accept-invalid-certs", target=target, timeout=self.args.connect_timeout)
        require(result.get("bridge_live"), "Connect did not report live bridge")
        self.check(f"{target}.rdp_bundle_ready")

    async def attach(self, target):
        endpoint = Mcp(self, target)
        self.endpoints[target] = endpoint
        await endpoint.start()
        return endpoint

    def filename(self, target, suffix):
        return f"proof-{self.token}-{target}-{suffix}"

    async def download(self, target, name, local, attempts=1):
        for attempt in range(attempts):
            response = await self.cli("get", "--session", target, "--remote-name", name, "--local", str(local), allow_failure=True)
            if "error" not in response and local.exists():
                return response
            if attempt + 1 < attempts:
                await asyncio.sleep(0.5)
        raise ProofError(f"{target}: expected guest output {name} unavailable")

    async def fixture(self, target):
        endpoint = self.endpoints[target]
        title = f"rdpilot-{self.token}-{target}"
        name = self.filename(target, "metadata-" + uuid.uuid4().hex[:6] + ".json")
        script = r"""
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$root=Join-Path $env:TEMP 'rdpilot-transfer-root'; New-Item -ItemType Directory -Force $root|Out-Null
$walk=$PID; $cua=0
for($i=0;$i -lt 12;$i++) {
 $proc=Get-CimInstance Win32_Process -Filter "ProcessId=$walk"
 if(!$proc){break}
 if($proc.Name -eq 'cua-driver.exe'){$cua=[int]$proc.ProcessId;break}
 $walk=[int]$proc.ParentProcessId
}
if(!$cua){throw 'Cua ancestor identity unavailable'}
$sid=[Diagnostics.Process]::GetCurrentProcess().SessionId
@{session_id=$sid;fixture_pid=$PID;cua_pid=$cua;title=TITLE} | ConvertTo-Json -Compress | Set-Content -Encoding UTF8 (Join-Path $root META)
$f=New-Object Windows.Forms.Form;$f.Text=TITLE;$f.Width=600;$f.Height=300;$f.StartPosition='CenterScreen'
$t=New-Object Windows.Forms.TextBox;$t.AccessibleName='Proof input';$t.Left=20;$t.Top=30;$t.Width=500
$b=New-Object Windows.Forms.Button;$b.Text='Proof button';$b.AccessibleName='Proof button';$b.Left=20;$b.Top=80;$b.Width=150
$l=New-Object Windows.Forms.Label;$l.Text="session:$sid";$l.Left=20;$l.Top=130;$l.Width=550
$b.Add_Click({$l.Text='clicked:'+ $t.Text})
$f.Controls.AddRange(@($t,$b,$l));[Windows.Forms.Application]::Run($f)
""".replace("TITLE", psquote(title)).replace("META", psquote(name))
        await endpoint.tool("launch_app", powershell(script))
        meta_path = self.output / name
        await self.download(target, name, meta_path, attempts=20)
        metadata = json.loads(meta_path.read_text(encoding="utf-8-sig"))
        self.metadata[target] = metadata
        window = None
        for _ in range(20):
            listing = structured(await endpoint.tool("list_windows"))
            for obj in objects(listing):
                if title in str(obj.get("title", obj.get("window_title", ""))):
                    wid = obj.get("window_id", obj.get("hwnd", obj.get("id")))
                    if wid is not None:
                        window = {"pid": int(obj.get("pid", metadata["fixture_pid"])), "window_id": int(wid)}
                        break
            if window:
                break
            await asyncio.sleep(0.5)
        require(window is not None, f"{target}: launched fixture window not discoverable")
        metadata["window"] = window
        await endpoint.tool("list_apps")
        self.check(f"{target}.launch_discovery", os_session_id=metadata["session_id"], fixture_pid=metadata["fixture_pid"], cua_pid=metadata["cua_pid"])
        await self.edit_fixture(target)

    async def observe(self, target):
        result = await self.endpoints[target].tool("get_window_state", {**self.metadata[target]["window"], "include_screenshot": True, "include_accessibility_tree": True})
        images = [item for item in result.get("content", []) if item.get("type") == "image"]
        require(images, f"{target}: observation has no embedded screenshot")
        png = base64.b64decode(images[0]["data"])
        require(png.startswith(b"\x89PNG\r\n\x1a\n"), "Cua screenshot is not PNG")
        (self.output / f"{target}-cua-window.png").write_bytes(png)
        state = structured(result)
        require(any("element_token" in obj for obj in objects(state)), "Cua observation has no accessibility elements")
        return state

    @staticmethod
    def element(state, label):
        candidates = [obj for obj in objects(state) if "element_token" in obj and label in json.dumps(obj)]
        require(candidates, f"fixture accessibility element {label} missing")
        return min(candidates, key=lambda obj: len(json.dumps(obj)))["element_token"]

    async def edit_fixture(self, target):
        endpoint, window = self.endpoints[target], self.metadata[target]["window"]
        state = await self.observe(target)
        text = f"{self.token}-target-{target}"
        await endpoint.tool("type_text", {**window, "element_token": self.element(state, "Proof input"), "text": text})
        state = await self.observe(target)
        await endpoint.tool("click", {**window, "element_token": self.element(state, "Proof button")})
        for _ in range(10):
            state = await self.observe(target)
            if "clicked:" + text in json.dumps(state):
                self.check(f"{target}.screenshot_accessibility_type_click_verified", text=text)
                return
            await asyncio.sleep(0.3)
        raise ProofError(f"{target}: click/type result not observable")

    async def isolation(self):
        require(self.metadata["a"]["session_id"] != self.metadata["b"]["session_id"], "targets share one actual Windows session")
        states = await asyncio.gather(self.observe("a"), self.observe("b"))
        for index, target in enumerate(("a", "b")):
            text = json.dumps(states[index])
            other = "b" if target == "a" else "a"
            require(f"{self.token}-target-{target}" in text and f"{self.token}-target-{other}" not in text, "cross-target desktop data")
        self.check("simultaneous_independent_os_sessions", a=self.metadata["a"]["session_id"], b=self.metadata["b"]["session_id"], cua_label="identical on both endpoints")

    async def transfer(self, target):
        name = self.filename(target, "bytes.bin")
        source, destination = self.output / (target + "-upload.bin"), self.output / (target + "-download.bin")
        data = bytes(range(256)) * 257
        source.write_bytes(data)
        upload = await self.cli("put", "--session", target, "--local", str(source), "--remote-name", name)
        download = await self.download(target, name, destination)
        require(destination.read_bytes() == data, "transfer byte mismatch")
        self.check(f"{target}.bidirectional_transfer", bytes=len(data), sha256=hashlib.sha256(data).hexdigest(), upload=upload, download=download)

    async def native_recovery(self, target, label):
        started = time.monotonic()
        await self.cli("ping", "--session", target, timeout=10)
        image = self.output / f"{target}-native-{label}.png"
        await self.cli("screenshot", "--session", target, "--output", str(image), timeout=10)
        await self.cli("input", "key", "--session", target, "--combo", "esc", timeout=10)
        require(image.read_bytes().startswith(b"\x89PNG"), "native recovery PNG missing")
        self.check(f"{target}.native_recovery_{label}", elapsed_seconds=round(time.monotonic()-started, 3))

    async def opposite_alive(self):
        state = await self.observe("b")
        require(f"{self.token}-target-b" in json.dumps(state), "opposite target lost its fixture state")
        self.check("b.survives_a_fault")

    async def fault(self, kind):
        endpoint = self.endpoints["a"]
        meta = self.metadata["a"]
        marker = self.filename("a", kind + ".json")
        prelude = f"""
$ErrorActionPreference='Stop';$id={int(meta['cua_pid'])};$sid={int(meta['session_id'])}
$p=Get-Process -Id $id
if($p.ProcessName -ne 'cua-driver' -or $p.SessionId -ne $sid){{throw 'exact Cua process identity changed'}}
$marker=Join-Path (Join-Path $env:TEMP 'rdpilot-transfer-root') {psquote(marker)}
Start-Sleep -Seconds 10
"""
        if kind == "kill":
            script = prelude + "@{pid=$id;session_id=$sid;fault='kill'}|ConvertTo-Json -Compress|Set-Content -Encoding UTF8 $marker; Stop-Process -Id $id -Force"
        else:
            script = prelude + r"""
Add-Type @'
using System;using System.Runtime.InteropServices;
public class FreezeCua {
 [DllImport("kernel32.dll")] public static extern IntPtr OpenThread(uint access,bool inherit,uint id);
 [DllImport("kernel32.dll")] public static extern uint SuspendThread(IntPtr thread);
 [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr handle);
}
'@
$count=0
foreach($t in $p.Threads){$h=[FreezeCua]::OpenThread(2,$false,$t.Id);if($h -eq [IntPtr]::Zero){throw 'OpenThread failed'};try{if([FreezeCua]::SuspendThread($h) -eq [uint32]::MaxValue){throw 'SuspendThread failed'};$count++}finally{[FreezeCua]::CloseHandle($h)|Out-Null}}
@{pid=$id;session_id=$sid;fault='stall';suspended_threads=$count}|ConvertTo-Json -Compress|Set-Content -Encoding UTF8 $marker
"""
        await endpoint.tool("launch_app", powershell(script))
        path = self.output / marker
        await self.download("a", marker, path, attempts=60)
        fault = json.loads(path.read_text(encoding="utf-8-sig"))
        if kind == "stall":
            require(fault.get("suspended_threads", 0) > 0, "no Cua thread suspended")
            pending = asyncio.create_task(endpoint.tool("list_windows"))
            await self.native_recovery("a", "while_stalled")
            try:
                await pending
                raise ProofError("suspended Cua unexpectedly completed request")
            except (ProofError, asyncio.TimeoutError, BrokenPipeError, ConnectionResetError) as error:
                if str(error) == "suspended Cua unexpectedly completed request":
                    raise
        await endpoint.closed(timeout=45)
        self.check(f"a.actual_cua_{kind}_bounded_close", **fault)
        await self.native_recovery("a", "after_" + kind)
        await self.opposite_alive()
        await endpoint.stop()
        await self.attach("a")
        await self.fixture("a")
        require(self.metadata["a"]["cua_pid"] != meta["cua_pid"], "reattach reused terminated runtime")
        self.check(f"a.fresh_runtime_after_{kind}")

    async def job_test(self):
        if not self.args.windows_job_test:
            self.summary["unverified"].append("Windows Job containment executable not supplied to this run")
            return
        target, executable = "a", Path(self.args.windows_job_test).resolve()
        name, result_name = self.filename(target, "job-test.exe"), self.filename(target, "job-result.json")
        await self.cli("put", "--session", target, "--local", str(executable), "--remote-name", name)
        script = f"""
$root=Join-Path $env:TEMP 'rdpilot-transfer-root';$exe=Join-Path $root {psquote(name)}
$text=& $exe 'runtime::windows_tests::job_closes_root_and_descendant_created_after_resume' '--exact' '--nocapture' 2>&1|Out-String
@{{exit_code=$LASTEXITCODE;output=$text}}|ConvertTo-Json -Compress|Set-Content -Encoding UTF8 (Join-Path $root {psquote(result_name)})
"""
        await self.endpoints[target].tool("launch_app", powershell(script))
        local = self.output / result_name
        await self.download(target, result_name, local, attempts=60)
        result = json.loads(local.read_text(encoding="utf-8-sig"))
        require(result["exit_code"] == 0 and "1 passed" in result["output"], "Windows Job test did not execute exactly one passing test")
        self.check("windows_job_root_and_descendant_containment", **result)

    async def loss(self):
        old = self.endpoints["a"]
        await self.relays["a"][0].drop()
        await old.closed(timeout=30)
        await self.opposite_alive()
        await self.cli("disconnect", "--session", "a", allow_failure=True)
        await self.connect("a")
        require(old.proc.returncode is not None, "stale endpoint remained attached after reconnect")
        await old.stop()
        await self.attach("a")
        await self.fixture("a")
        await self.opposite_alive()
        self.check("a.unplanned_rdp_loss_stale_endpoint_and_explicit_reconnect")

    async def execute(self):
        daemon_log = open(self.output / "daemon.log", "w")
        try:
            for target, credentials in self.credentials.items():
                relay = Relay(credentials)
                self.relays[target] = (relay, await relay.start())
            self.daemon = await asyncio.create_subprocess_exec(str(self.bin / "rdpilot-daemon"), env=self.env, stdout=daemon_log, stderr=daemon_log)
            await asyncio.sleep(0.3)
            for target in ("a", "b"):
                await self.connect(target)
                await self.attach(target)
                await self.fixture(target)
                await self.transfer(target)
            await self.isolation()
            await self.job_test()
            if not self.args.skip_faults:
                await self.fault("kill")
                await self.fault("stall")
                await self.loss()
            else:
                self.summary["unverified"].append("Fault injection explicitly skipped")
            self.summary["status"] = "passed"
        except BaseException as error:
            self.summary["status"] = "failed"
            self.summary["failure"] = self.clean(f"{type(error).__name__}: {error}")
            raise
        finally:
            for endpoint in self.endpoints.values():
                await endpoint.stop()
            for target in self.relays:
                try:
                    await self.cli("disconnect", "--session", target, timeout=10, allow_failure=True)
                except (OSError, asyncio.TimeoutError, ValueError):
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
            self.save()
            shutil.rmtree(self.temp)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bin-dir", required=True)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--credentials-a", required=True)
    parser.add_argument("--credentials-b", required=True)
    parser.add_argument("--output", required=True, help="new evidence directory; must not already exist")
    parser.add_argument("--connect-timeout", type=int, default=600)
    parser.add_argument("--windows-job-test", help="crossbuilt Windows bridge lib-test executable")
    parser.add_argument("--skip-faults", action="store_true", help="diagnostic subset only; explicitly leaves failure/reconnect checks unverified")
    args = parser.parse_args()
    os.umask(0o077)
    run = Run(args)
    try:
        asyncio.run(run.execute())
    except (Exception, KeyboardInterrupt) as error:
        print(run.clean(f"Live proof failed: {type(error).__name__}: {error}"), flush=True)
        return 1
    print(f"Evidence: {run.output / 'summary.json'}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
