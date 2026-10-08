"""Shared platform and installed-bundle checks for the existing live proofs."""
import asyncio
import base64
import json
import os
import re
from pathlib import Path
import signal
import subprocess
import sys

OPERATIONS = {'harness','relay_start','daemon_start','daemon_start_wait','hosts_file','connect_cli','bridge_ready_assertion','live_checks',
              'endpoint_cleanup','disconnect_cleanup','daemon_cleanup','relay_cleanup','log_cleanup','summary_write','harness_scan','temporary_cleanup','evidence_scan','summary_read','connect_observation','artifact_write','bootstrap_cleanup','first_a_attach'}
CATEGORIES = {'timeout','file_not_found','access_denied','invalid_json','os_error','proof_assertion','other'}


def failure_detail(error, operation=None, cli_exit=None):
    """Freeze only closed fields; exception messages and payloads stay private."""
    if isinstance(error, TimeoutError):category='timeout'
    elif isinstance(error, FileNotFoundError):category='file_not_found'
    elif isinstance(error, PermissionError):category='access_denied'
    elif isinstance(error, json.JSONDecodeError):category='invalid_json'
    elif isinstance(error, OSError):category='os_error'
    elif type(error).__name__=='ProofError':category='proof_assertion'
    else:category='other'
    detail={'operation':operation if isinstance(operation,str) and operation in OPERATIONS else 'harness','exception_category':category}
    if type(cli_exit) is int and -(2**31)<=cli_exit<2**32:detail['cli_exit_code']=cli_exit
    return detail


def select_failure_detail(raw):
    raw=raw if isinstance(raw,dict) else {}
    result={'operation':raw.get('operation') if isinstance(raw.get('operation'),str) and raw['operation'] in OPERATIONS else 'harness',
            'exception_category':raw.get('exception_category') if isinstance(raw.get('exception_category'),str) and raw['exception_category'] in CATEGORIES else 'other'}
    value=raw.get('cli_exit_code')
    if type(value) is int and -(2**31)<=value<2**32:result['cli_exit_code']=value
    if isinstance(raw.get("connect_observation"),dict):
        result["connect_observation"]=select_connect_observation(raw["connect_observation"])
    return result


# Observer budgets never affect the CLI result or proof acceptance.
JSON_BUDGET = 64 * 1024
MESSAGE_BUDGET = 16 * 1024
COUNTER_MAX = 2**63 - 1
CLI_CODES = {'config','missing-config','transport','internal','bundle-unavailable',
             'daemon-unreachable','daemon-incompatible','duplicate-session'}
PRODUCERS = {'unavailable','config_other','password_command','sdk_connection','sdk_tls',
             'sdk_bootstrap','sdk_session','sdk_configuration','sdk_dvc','sdk_bridge_rejection',
             'daemon_connect','daemon_io','daemon_configuration','cli_response'}
HELPERS = {'unavailable','empty_output','invalid_utf8','timeout','shell_start','output_read',
           'wait','no_stdout','nonzero_exit'}
BOOTSTRAP_STAGES = {'rdpdr_file_access','rdpdr_file_read','launch_input_attempted','launch_input_sent',
                    'dvc_channel_created','dvc_channel_open','version_received','ping_sent','pong_received'}
BOOTSTRAP_TIMEOUT = ('rdpilot-bridge did not start (possible security prompt in the guest, or a bridge '
                     'that does not speak bridge protocol 2); stages=')


def bounded_int(value, minimum=0, maximum=COUNTER_MAX):
    return type(value) is int and minimum <= value <= maximum


def closed(value, allowed, default='unavailable'):
    return value if isinstance(value,str) and len(value)<=max(map(len,allowed)) and value in allowed else default


def cli_observation(stdout):
    """Select only the existing complete CLI Display envelope, never its text."""
    empty={'cli_error_code':'unavailable','producer':'unavailable','connection_stage':'unavailable',
           'password_helper':'unavailable','bootstrap_state':'unavailable'}
    if not isinstance(stdout,bytes) or len(stdout)>JSON_BUDGET:return empty
    try:
        def pairs(items):
            result={}
            for key,value in items:
                if key in result:raise ValueError('Duplicate diagnostic JSON key')
                result[key]=value
            return result
        payload=json.loads(stdout,object_pairs_hook=pairs)
    except (ValueError,UnicodeError,RecursionError):return empty
    error=payload.get('error') if isinstance(payload,dict) else None
    if not isinstance(error,dict):return empty
    code,message=error.get('code'),error.get('message')
    if (not isinstance(code,str) or len(code)>max(map(len,CLI_CODES)) or code not in CLI_CODES
        or not isinstance(message,str) or len(message.encode('utf-8',errors='replace'))>MESSAGE_BUDGET):return empty
    result={**empty,'cli_error_code':code}
    if code=='config' and message.startswith('configuration error: '):
        result['producer']='config_other'
        for target in ('a','b'):
            prefix=f'configuration error: PasswordCommand for {target} failed: '
            if message.startswith(prefix):
                result['producer']='password_command'
                reason=message[len(prefix):]
                fixed={'empty output':'empty_output','output is not valid UTF-8':'invalid_utf8',
                       'timed out after 30 s':'timeout','no stdout':'no_stdout'}
                result['password_helper']=fixed.get(reason,'unavailable')
                for prefix,label in (('could not start the shell: ','shell_start'),
                                     ('could not read its output: ','output_read'),('could not wait for it: ','wait')):
                    if reason.startswith(prefix):result['password_helper']=label
                match=re.fullmatch(r'exit code: ([0-9]{1,10})|exit status: ([0-9]{1,10})',reason)
                if match:
                    value=int(next(x for x in match.groups() if x is not None))
                    if bounded_int(value,1,2**32-1):
                        result.update(password_helper='nonzero_exit',helper_exit_code=value)
    elif code=='internal':
        if message.startswith('unexpected daemon response: '):result['producer']='cli_response'
        elif message.startswith('Internal: '):
            inner=message[len('Internal: '):]
            prefixes={'connection or authentication failed: ':'sdk_connection','TLS or certificate error: ':'sdk_tls',
                      'bridge bootstrap failed: ':'sdk_bootstrap','session/transport error: ':'sdk_session',
                      'invalid configuration: ':'sdk_configuration','DVC transport error: ':'sdk_dvc',
                      'bridge rejected the request: ':'sdk_bridge_rejection','connect failed: ':'daemon_connect',
                      'io error: ':'daemon_io','config resolution failed: ':'daemon_configuration'}
            for prefix,label in prefixes.items():
                if not inner.startswith(prefix):continue
                result['producer']=label
                tail=inner[len(prefix):]
                if label=='sdk_connection':
                    for prefix,stage in (('connect_begin failed: ','negotiation'),('connect_finalize failed: ','finalize')):
                        if tail.startswith(prefix):result['connection_stage']=stage
                elif label=='sdk_bootstrap' and tail.startswith(BOOTSTRAP_TIMEOUT):
                    suffix=tail[len(BOOTSTRAP_TIMEOUT):]
                    stages=suffix.split(',')
                    if suffix=='none':result.update(bootstrap_state='observed',bootstrap_stages=[])
                    elif (len(stages)<=9 and len(set(stages))==len(stages)
                          and all(x in BOOTSTRAP_STAGES for x in stages)):
                        result.update(bootstrap_state='observed',bootstrap_stages=stages)
                break
    return result


def select_connect_observation(raw):
    raw=raw if isinstance(raw,dict) else {}
    result={'cli_error_code':closed(raw.get('cli_error_code'),CLI_CODES),
            'producer':closed(raw.get('producer'),PRODUCERS),
            'connection_stage':closed(raw.get('connection_stage'),{'negotiation','finalize'}),
            'password_helper':closed(raw.get('password_helper'),HELPERS),'bootstrap_state':'unavailable'}
    helper=raw.get('helper_exit_code')
    if result['password_helper']=='nonzero_exit' and bounded_int(helper,1,2**32-1):result['helper_exit_code']=helper
    stages=raw.get('bootstrap_stages')
    if (raw.get('bootstrap_state')=='observed' and isinstance(stages,list) and len(stages)<=9
        and all(closed(x,BOOTSTRAP_STAGES,None) is not None for x in stages) and len(set(stages))==len(stages)):
        result.update(bootstrap_state='observed',bootstrap_stages=list(stages))
    relays=raw.get('relays')
    if isinstance(relays,dict):
        result['relays']={target:select_relay_observation(relays.get(target)) for target in ('a','b')}
    daemon=raw.get('daemon')
    if isinstance(daemon,dict):
        result['daemon']={'state':closed(daemon.get('state'),{'alive','exited'})}
        if result['daemon']['state']=='exited' and bounded_int(daemon.get('exit_code'),-(2**31),2**32-1):
            result['daemon']['exit_code']=daemon['exit_code']
    result['observation_state']=closed(raw.get('observation_state'),{'observed','observation_failed'})
    return result


def select_relay_observation(raw):
    raw=raw if isinstance(raw,dict) else {}
    keys=('accepted','upstream_connected','to_upstream_bytes','to_client_bytes')
    if raw.get('state')!='observed' or not all(bounded_int(raw.get(k)) for k in keys):return {'state':'unavailable'}
    result={'state':'observed',**{k:raw[k] for k in keys},
            'upstream_failure':closed(raw.get('upstream_failure'),{'none','timeout','os_error','unknown'}),
            'copy_failure':closed(raw.get('copy_failure'),{'none','os_error','unknown'})}
    for key in ('errno','winerror'):
        if bounded_int(raw.get(key),-(2**31),2**32-1):result[key]=raw[key]
    return result


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
$transferRoot=Join-Path $env:TEMP 'rdpilot-transfer-root'
New-Item -ItemType Directory -Force -Path $transferRoot | Out-Null
@{bridge_sha256=(Get-FileHash $bridge).Hash.ToLower();cua_sha256=(Get-FileHash $cua).Hash.ToLower();
 manifest=$manifest;installed_bundle_id=(Split-Path $root -Leaf);
 installed_under_localappdata=$root.StartsWith((Join-Path $env:LOCALAPPDATA 'rdpilot') + '\',[StringComparison]::OrdinalIgnoreCase);
 session_id=[Diagnostics.Process]::GetCurrentProcess().SessionId} |
 ConvertTo-Json -Depth 20 -Compress | Set-Content -Encoding UTF8 (Join-Path $transferRoot NAME)
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
