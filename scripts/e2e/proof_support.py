"""Shared platform and installed-bundle checks for the existing live proofs."""
import asyncio
import base64
import json
import os
from pathlib import Path
import signal
import subprocess
import sys


def password_command(target):
    name = 'E2E_PASSWORD_' + target.upper()
    if os.name != 'nt':
        return 'printenv ' + name
    code = "[Console]::Write([Environment]::GetEnvironmentVariable('" + name + "'))"
    return 'powershell.exe -NoProfile -NonInteractive -EncodedCommand ' + base64.b64encode(code.encode('utf-16le')).decode('ascii')


def viewer_process_options():
    # Ctrl-C is enabled in this private console. A new process group disables it.
    return {'creationflags': subprocess.CREATE_NEW_CONSOLE} if os.name == 'nt' else {}


async def interrupt_viewer(proc):
    if os.name != 'nt':
        proc.send_signal(signal.SIGINT)
        return
    helper = await asyncio.create_subprocess_exec(
        sys.executable, str(Path(__file__).resolve()), '--interrupt-console', str(proc.pid),
        stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL,
        creationflags=subprocess.CREATE_NO_WINDOW)
    if await asyncio.wait_for(helper.wait(), 5) != 0:
        raise RuntimeError('Target-console Ctrl-C delivery failed')


async def installed_identity(run, target, endpoint, e2e):
    expected = getattr(run.args, 'source_bridge_sha256', None)
    if not expected:
        return
    name = 'installed-' + target + '.json'
    script = r"""
$ErrorActionPreference='Stop'
$walk=$PID;$bridge=$null;$cua=$null
for($i=0;$i -lt 12;$i++) {
 $p=Get-CimInstance Win32_Process -Filter "ProcessId=$walk"
 if(!$p){break}
 if($p.Name -eq 'cua-driver.exe'){$cua=$p.ExecutablePath}
 if($p.Name -eq 'rdpilot-bridge.exe'){$bridge=$p.ExecutablePath;break}
 $walk=[int]$p.ParentProcessId
}
if(!$bridge -or !$cua){throw 'Installed ancestry unavailable'}
$root=Split-Path $bridge
$manifest=Get-Content (Join-Path $root 'manifest.json') -Raw | ConvertFrom-Json
@{bridge_sha256=(Get-FileHash $bridge).Hash.ToLower();cua_sha256=(Get-FileHash $cua).Hash.ToLower();
 manifest=$manifest;installed_bundle_id=(Split-Path $root -Leaf);
 installed_under_localappdata=$root.StartsWith((Join-Path $env:LOCALAPPDATA 'rdpilot') + '\',[StringComparison]::OrdinalIgnoreCase);
 session_id=[Diagnostics.Process]::GetCurrentProcess().SessionId} |
 ConvertTo-Json -Depth 20 -Compress | Set-Content -Encoding UTF8 (Join-Path (Join-Path $env:TEMP 'rdpilot-transfer-root') NAME)
""".replace('NAME', e2e.psquote(name))
    await endpoint.tool('launch_app', e2e.powershell(script))
    local = run.output / name
    for _ in range(40):
        await run.cli('get', '--session', target, '--remote-name', name, '--local', str(local), allow_failure=True)
        if local.exists():
            break
        await asyncio.sleep(0.5)
    observed = json.loads(local.read_text(encoding='utf-8-sig'))
    manifest = observed['manifest']
    e2e.require(observed['bridge_sha256'] == expected == manifest['bridge_sha256'], 'Source/guest/manifest bridge mismatch')
    e2e.require(observed['installed_under_localappdata'] and observed['installed_bundle_id'] == manifest['bundle_id'], 'Installed bundle identity mismatch')
    cua = [sha for path, sha in manifest['files'].items() if path.lower().endswith('cua-driver.exe')]
    e2e.require(len(cua) == 1 and cua[0] == observed['cua_sha256'], 'Installed Cua identity mismatch')
    e2e.require(manifest['cua_version'] == run.args.cua_version, 'Pinned Cua version mismatch')
    run.check(target + '.installed_source_bridge_and_cua_identity', bridge_sha256=expected,
              cua_version=manifest['cua_version'], cua_sha256=observed['cua_sha256'],
              archive_sha256=manifest['archive_sha256'], bundle_id=manifest['bundle_id'],
              os_session_id=observed['session_id'])


def console_interrupt(pid):
    import ctypes
    import time
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.FreeConsole()
    if not kernel.AttachConsole(pid):
        return 1
    if not kernel.SetConsoleCtrlHandler(None, True):
        return 1
    if not kernel.GenerateConsoleCtrlEvent(0, 0):
        return 1
    time.sleep(0.2)
    kernel.FreeConsole()
    return 0


if __name__ == '__main__':
    raise SystemExit(console_interrupt(int(sys.argv[2])))
