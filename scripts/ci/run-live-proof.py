#!/usr/bin/env python3
"""Run existing live harnesses and select only validated, scanned evidence."""
import argparse
import asyncio
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

E2E = Path(__file__).resolve().parents[1] / 'e2e'
CHECKS = {
 'cua': tuple(t + '.' + c for t in ('a', 'b') for c in (
    'rdp_bundle_ready', 'native_initialize_tools', 'installed_source_bridge_and_cua_identity',
    'launch_discovery', 'screenshot_accessibility_type_click_verified', 'bidirectional_transfer',
    'native_recovery_before_faults')) + (
    'simultaneous_independent_os_sessions', 'a.actual_cua_kill_bounded_close',
    'a.actual_cua_stall_bounded_close', 'a.fresh_runtime_after_kill', 'a.fresh_runtime_after_stall',
    'a.native_recovery_after_kill', 'a.native_recovery_after_stall', 'a.native_recovery_while_stalled', 'b.survives_a_fault',
    'a.unplanned_rdp_loss_stale_endpoint_and_explicit_reconnect', 'evidence_contains_no_credentials'),
 'viewer': ('a.connected', 'b.connected', 'a.native_initialize_tools', 'b.native_initialize_tools',
    'a.installed_source_bridge_and_cua_identity', 'b.installed_source_bridge_and_cua_identity',
    'viewer_started', 'session_list_and_sessions_viewed', 'a.cua_change_visible_within_budget',
    'a.strip_shows_each_call_within_budget', 'strips_per_session_names_only', 'a.frame_rate_capped',
    'a.png_size_and_encode_time', 'b.png_size_and_encode_time', 'b.disconnect_shown',
    'a.close_shown_and_listed', 'unauthorised_requests_rejected_on_every_bound_address',
    'viewer_stopped_on_ctrl_c_and_ports_closed', 'windows_console_controller_and_sibling_survived', 'evidence_contains_no_token_or_credentials'),
 'takeover': ('notepad.connected', 'notepad.native_initialize_tools',
    'notepad.installed_source_bridge_and_cua_identity', 'notepad_launched_by_the_agent', 'viewer_started',
    'tools_list_advertises_takeover_on_acting_tools_only', 'tab1_takeover_and_typing',
    'agent_refused_during_the_lease', 'tab2_takeover_and_tab1_notice', 'agent_takeover_through_mcp',
    'cli_takeover_as_printed', 'closed_tab_returns_control', 'status_log_and_strip_show_every_change',
    'viewer_stopped_on_ctrl_c_and_ports_closed', 'viewer_started_read_only',
    'read_only_viewer_has_no_takeover_and_no_write_route',
    'evidence_contains_no_token_credential_lease_or_typed_text', 'windows_console_controller_and_sibling_survived'),
}
CHECKS={proof:('private_child_temporary_boundary_verified',)+checks for proof,checks in CHECKS.items()}
ALLOWED_UNVERIFIED = {
 'cua': {'UAC/secure-desktop behavior is unsupported; no elevation guarantee',
         'Windows Job containment executable not supplied to this run'},
 'viewer': {'tailnet access (no tailnet address bound)', 'rejection on the tailnet address (no tailnet address bound)'},
 'takeover': {'tailnet access (no tailnet address bound)'},
}
HEX = re.compile(r'[a-f0-9]{64}\Z')
BUNDLE = re.compile(r'cua-driver-rs-v0\.34\.0-[a-f0-9]{16}\Z')
OPERATIONS = {'harness','relay_start','daemon_start','daemon_start_wait','hosts_file','connect_cli','bridge_ready_assertion','live_checks'}


def failure_detail(error, operation=None, cli_exit=None):
    """Only fixed operation/type categories and numeric exit codes cross out."""
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


def project(proof, summary, returncode, expected_hash):
    checks = summary.get('checks', []) if isinstance(summary, dict) else []
    checks = checks if isinstance(checks, list) else []
    completed = [name for name in CHECKS[proof] if any(
        isinstance(c, dict) and c.get('check') == name and c.get('passed') is True for c in checks)]
    identities = []
    targets = ('notepad',) if proof == 'takeover' else ('a', 'b')
    for target in targets:
        found = next((c for c in checks if isinstance(c, dict) and c.get('check') == target + '.installed_source_bridge_and_cua_identity'), {})
        if (found.get('passed') is True and found.get('bridge_sha256') == expected_hash
            and HEX.fullmatch(str(expected_hash)) and found.get('cua_version') == '0.34.0'
            and HEX.fullmatch(str(found.get('cua_sha256'))) and HEX.fullmatch(str(found.get('archive_sha256')))
            and BUNDLE.fullmatch(str(found.get('bundle_id')))
            and type(found.get('os_session_id')) is int and found['os_session_id'] > 0):
            identities.append({k: found[k] for k in ('bridge_sha256','cua_version','cua_sha256','archive_sha256','bundle_id','os_session_id')})
    valid = isinstance(summary, dict) and summary.get('mode') == 'live' and summary.get('status') == 'passed'
    coverage = summary.get('unverified', []) if isinstance(summary, dict) else None
    valid = valid and isinstance(coverage, list) and all(isinstance(x, str) and x in ALLOWED_UNVERIFIED[proof] for x in coverage)
    valid = valid and len(identities) == len(targets)
    if len(targets) == 2:
        valid = valid and len({x['os_session_id'] for x in identities}) == 2
    if proof == 'viewer':
        valid = valid and isinstance(summary.get('canvas_vs_native'), dict)
    passed = bool(returncode == 0 and valid and len(completed) == len(CHECKS[proof]))
    return {'proof': proof, 'mode': 'live', 'status': 'passed' if passed else 'failed',
            'completed_checks': completed, 'identity': identities,
            'unverified': sorted(set(coverage) & ALLOWED_UNVERIFIED[proof]) if isinstance(coverage, list) and all(isinstance(x,str) for x in coverage) else [],
            'failure_stage': 'none' if passed else ('harness' if returncode else 'required_evidence')}


def process_identity(pid):
    """Read process creation time/image from a held Windows process handle."""
    from ctypes import wintypes
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel.QueryFullProcessImageNameW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)]
    handle = kernel.OpenProcess(0x1000, False, pid)
    if not handle:
        raise OSError('Process identity unavailable')
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        size = wintypes.DWORD(32768)
        image = ctypes.create_unicode_buffer(size.value)
        if not kernel.GetProcessTimes(handle, *[ctypes.byref(t) for t in times]) or not kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(size)):
            raise OSError('Process identity unavailable')
        return {'pid': pid, 'created': str((times[0].dwHighDateTime << 32) | times[0].dwLowDateTime), 'image': image.value}
    finally:
        kernel.CloseHandle(handle)


def scan_files(root, needles):
    return all(not any(n and n in p.read_bytes() for n in needles) for p in root.rglob('*') if p.is_file())


async def execute(args):
    sys.path.insert(0, str(E2E))
    filename = 'run-cua-e2e.py' if args.proof == 'cua' else 'run-' + args.proof + '-proof.py'
    spec = importlib.util.spec_from_file_location('hosted_live_harness', E2E / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    original = asyncio.create_subprocess_exec
    records = []
    journal = Path(args.journal)
    async def tracked(*argv, **kwargs):
        proc = await original(*argv, **kwargs)
        # Journal immediately; only these exact children can be fallback-stopped.
        if os.name == 'nt':
            try:
                records.append(process_identity(proc.pid))
                journal.write_text(json.dumps(records))
            except OSError:
                await asyncio.sleep(0)
                if proc.returncode is None:
                    proc.kill()
                    raise
        return proc
    asyncio.create_subprocess_exec = tracked
    harness_args = argparse.Namespace(bin_dir=args.bin_dir, bundle=args.bundle, output=args.output,
        credentials_a=args.credentials_a, credentials_b=args.credentials_b,
        source_bridge_sha256=args.source_bridge_sha256, cua_version='0.34.0',
        connect_timeout=600, windows_job_test=None, skip_faults=False, fake=False,
        bind='loopback', allow_no_tailnet=True, allow_debug=False, marker_region=None)
    run = (module.Run if args.proof == 'cua' else module.Proof)(harness_args)
    boundary=Path(args.output).resolve().parent
    if not Path(run.temp).resolve().is_relative_to(boundary):
        raise RuntimeError('Harness temporary files escaped private boundary')
    run.check('private_child_temporary_boundary_verified')
    code = 0;error_detail={}
    sentinel = await tracked(sys.executable, '-c', 'import time; time.sleep(3600)') if args.proof != 'cua' else None
    try:
        await asyncio.wait_for(run.execute(), args.timeout)
    except BaseException as error:
        code = 1
        error_detail=failure_detail(error,getattr(run,'failure_operation',None),getattr(run,'failed_cli_exit',None))
    finally:
        asyncio.create_subprocess_exec = original
        if sentinel:
            if code == 0 and sentinel.returncode is None:
                run.check('windows_console_controller_and_sibling_survived')
            else:
                code = 1
            if sentinel.returncode is None:
                sentinel.terminate()
                await asyncio.wait_for(sentinel.wait(), 5)
    # Recheck known secret bytes including leases/typed markers. Raw exceptions stay private.
    needles = [str(c['password']).encode() for c in run.credentials.values() if c.get('password')]
    needles += [str(x).encode() for x in getattr(run,'leases',[])]
    needles += [str(x).encode() for x in (getattr(run,'typed',None),getattr(run,'typed_marker',None),getattr(run,'token',None)) if x]
    scanned = scan_files(Path(args.output), needles)
    summary = json.loads((Path(args.output)/'summary.json').read_text())
    selected = project(args.proof, summary, code if scanned else 1, args.source_bridge_sha256)
    if selected['status']!='passed':selected['harness_failure']=error_detail
    artifact = Path(args.artifacts)
    artifact.mkdir(parents=True, exist_ok=True)
    (artifact / (args.proof + '.json')).write_text(json.dumps(selected, indent=2)+'\n')
    if selected['status'] == 'passed' and scanned:
        import shutil
        for p in Path(args.output).glob('*.png'):
            # Harness-selected filenames are local constants; publish only PNGs.
            if re.fullmatch(r'[a-zA-Z0-9._-]+\.png',p.name):
                shutil.copyfile(p, artifact/(args.proof+'-'+p.name))
    return 0 if selected['status'] == 'passed' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('proof', choices=CHECKS)
    for name in ('bin-dir','bundle','output','artifacts','source-bridge-sha256','journal'):
        parser.add_argument('--'+name, required=True)
    parser.add_argument('--credentials-a')
    parser.add_argument('--credentials-b')
    parser.add_argument('--timeout',type=int,default=900)
    args=parser.parse_args()
    try:
        return asyncio.run(execute(args))
    except BaseException as error:
        Path(args.artifacts).mkdir(parents=True,exist_ok=True)
        selected=project(args.proof,None,1,args.source_bridge_sha256)
        selected['harness_failure']=failure_detail(error)
        (Path(args.artifacts)/(args.proof+'.json')).write_text(json.dumps(selected))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
