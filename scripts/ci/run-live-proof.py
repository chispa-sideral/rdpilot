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
import time
import bootstrap_desktop_observer as desktop
import held_process

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


def failure_detail(error, operation=None, cli_exit=None):
    """Only fixed operation/type categories and numeric exit codes cross out."""
    sys.path.insert(0,str(E2E))
    import proof_support
    return proof_support.failure_detail(error,operation,cli_exit)


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
    import proof_support
    args._primary_failure={}
    args._secondary_failures=[]
    args._failure_stage='harness'
    baseline=args.proof=='first_a'
    if baseline:
        import first_a
        if getattr(args,'bootstrap_desktop',False) or getattr(args,'bootstrap_desktop_config',None):raise ValueError('Baseline observers refused')
    filename = 'run-cua-e2e.py' if args.proof in ('cua','first_a') else 'run-' + args.proof + '-proof.py'
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
                await held_process.register_async(proc, process_identity, journal, records)
            except BaseException as error:
                if getattr(error, 'held_cleanup_failures', ()):
                    args._secondary_failures.append(proof_support.failure_detail(RuntimeError(), 'daemon_cleanup'))
                raise
        return proc
    harness_args = argparse.Namespace(bin_dir=args.bin_dir, bundle=args.bundle, output=args.output,
        credentials_a=args.credentials_a, credentials_b=args.credentials_b,
        source_bridge_sha256=args.source_bridge_sha256, cua_version='0.34.0',
        connect_timeout=600, windows_job_test=None, skip_faults=False, fake=False,
        bind='loopback', allow_no_tailnet=True, allow_debug=False, marker_region=None)
    observed = getattr(args, 'bootstrap_desktop', False)
    if observed and args.proof != 'cua':
        raise ValueError('Bootstrap observation requires Cua diagnostic')
    config = None
    run_class = first_a.run_class(module.Run) if baseline else module.Run if args.proof == 'cua' else module.Proof
    if observed:
        config_path = Path(args.bootstrap_desktop_config)
        if not config_path.resolve().is_relative_to(Path(args.output).resolve().parent):
            raise ValueError('Private observation handoff')
        config = json.loads(desktop.read_plain(config_path, 16*1024), object_pairs_hook=desktop.files.unique_json)
        if (set(config) != {'source', 'sid', 'username', 'domain', 'wts_script', 'mode', 'fresh_profile_verified'}
                or config['mode'] != 'cua_diagnostic' or config['fresh_profile_verified'] is not True
                or not desktop.source_valid(config['source'])
                or config['source']['bridge_sha256'] != args.source_bridge_sha256
                or not isinstance(config['sid'], str) or not re.fullmatch(r'S-1-[0-9-]{1,180}', config['sid'])
                or not isinstance(config['wts_script'], str) or len(config['wts_script']) > 8192):
            raise ValueError('Observation handoff')
        def register(record):
            records.append(record)
            journal.write_text(json.dumps(records))
        def factory(run):
            if config['username'] != run.credentials['a']['username'] or config['domain'] != run.credentials['a'].get('domain'):
                raise ValueError('Target identity handoff')
            return desktop.Observer(run, config, original, process_identity if os.name == 'nt' else lambda pid: None, register, proof_support)
        run_class = desktop.observed_run(module.Run, factory)
    elif getattr(args, 'bootstrap_desktop_config', None):
        raise ValueError('Unexpected observation handoff')
    run = run_class(harness_args)
    boundary=Path(args.output).resolve().parent
    if not Path(run.temp).resolve().is_relative_to(boundary):
        raise RuntimeError('Harness temporary files escaped private boundary')
    run.check('private_child_temporary_boundary_verified')
    asyncio.create_subprocess_exec = tracked
    code = 0;error_detail={}
    sentinel = None
    try:
        sentinel = await tracked(sys.executable, '-c', 'import time; time.sleep(3600)') if args.proof not in ('cua','first_a') else None
        await asyncio.wait_for(run.execute(), first_a.WORK_SECONDS+first_a.FINALIZE_SECONDS+5 if baseline else args.timeout)
    except BaseException as error:
        code = 1
        primary=getattr(run,'primary_failure',None)
        error_detail=proof_support.select_failure_detail(primary) if primary else failure_detail(error,getattr(run,'failure_operation',None),getattr(run,'failed_cli_exit',None))
        args._primary_failure=proof_support.select_failure_detail(error_detail)
    finally:
        asyncio.create_subprocess_exec = original
        if sentinel:
            if code == 0 and sentinel.returncode is None:
                run.check('windows_console_controller_and_sibling_survived')
            else:
                code = 1
            if sentinel.returncode is None:
                try:
                    sentinel.terminate()
                    await asyncio.wait_for(sentinel.wait(), 5)
                except BaseException as error:
                    code=1;args._secondary_failures.append(failure_detail(error,'daemon_cleanup'))
                    if await held_process.abort_async(sentinel,5):args._secondary_failures.append(failure_detail(RuntimeError(),'daemon_cleanup'))
    # Recheck known secret bytes including leases/typed markers. Raw exceptions stay private.
    needles = [str(c['password']).encode() for c in run.credentials.values() if c.get('password')]
    needles += [str(x).encode() for x in getattr(run,'leases',[])]
    needles += [str(x).encode() for x in (getattr(run,'typed',None),getattr(run,'typed_marker',None),getattr(run,'token',None)) if x]
    observer = getattr(run, 'bootstrap_observer', None)
    if observer and observer.bearer:
        needles.append(observer.bearer.encode())
    secondary=[proof_support.select_failure_detail(x) for x in args._secondary_failures + getattr(run,'cleanup_failures',[])]
    if secondary:code=1
    try:
        scanned = first_a.scan(Path(args.output),needles,time.monotonic()+30) if baseline else scan_files(Path(args.output), needles)
        if not scanned:raise RuntimeError('Private evidence scan failed')
    except BaseException as error:
        scanned=False;code=1;secondary.append(failure_detail(error,'evidence_scan'))
    try:summary = first_a.read_json(Path(args.output)/'summary.json') if baseline else json.loads((Path(args.output)/'summary.json').read_text())
    except BaseException as error:
        summary=None;code=1;secondary.append(failure_detail(error,'summary_read'))
    selected = first_a.project(summary,run,code,args.source_bridge_sha256,scanned) if baseline else project(args.proof, summary, code if scanned else 1, args.source_bridge_sha256)
    succeeded=selected['status']==(first_a.PASSED if baseline else 'passed')
    if not succeeded:selected['harness_failure']=error_detail
    if secondary:selected['secondary_failures']=secondary
    args._secondary_failures=[proof_support.select_failure_detail(x) for x in secondary]
    args._failure_stage='artifact_write'
    artifact = Path(args.artifacts)
    artifact.mkdir(parents=True, exist_ok=True)
    if baseline:first_a.write_json(artifact/(args.proof+'.json'),selected)
    else:(artifact / (args.proof + '.json')).write_text(json.dumps(selected, indent=2)+'\n')
    if baseline and not first_a.scan(artifact,needles,time.monotonic()+10):raise RuntimeError('Selected baseline evidence scan')
    if selected['status'] == 'passed' and scanned:
        import shutil
        for p in Path(args.output).glob('*.png'):
            # Harness-selected filenames are local constants; publish only PNGs.
            if observed and re.fullmatch(r'bootstrap-a-(?:0[1-9]|1[0-9]|20)\.png', p.name):
                continue  # Diagnostic files use only their separately validated manifest.
            if re.fullmatch(r'[a-zA-Z0-9._-]+\.png',p.name):
                shutil.copyfile(p, artifact/(args.proof+'-'+p.name))
    if observed:
        raw = observer.manifest if observer else None
        expected = config['source']
        diagnostic = desktop.safe_manifest(Path(args.output), raw, expected, scanned)
        import shutil
        for name in desktop.selected_files(Path(args.output), diagnostic, expected):
            shutil.copyfile(Path(args.output)/name, artifact/name)
        (artifact/'bootstrap-desktop.json').write_text(json.dumps(diagnostic, indent=2)+'\n')
    args._failure_stage='harness'
    return 0 if succeeded else 1


def failed_fallback(args,error):
    import proof_support
    if args.proof=='first_a':
        import first_a
        selected=first_a.project(None,None,1,args.source_bridge_sha256,False)
    else:selected=project(args.proof,None,1,args.source_bridge_sha256)
    primary=getattr(args,'_primary_failure',None)
    stage='artifact_write' if getattr(args,'_failure_stage',None)=='artifact_write' else 'harness'
    selected['harness_failure']=proof_support.select_failure_detail(primary) if primary else failure_detail(error,stage)
    secondary=[proof_support.select_failure_detail(x) for x in getattr(args,'_secondary_failures',[])]
    if primary:secondary.append(failure_detail(error,stage))
    if secondary:selected['secondary_failures']=secondary
    return selected


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('proof', choices=(*CHECKS,'first_a'))
    for name in ('bin-dir','bundle','output','artifacts','source-bridge-sha256','journal'):
        parser.add_argument('--'+name, required=True)
    parser.add_argument('--credentials-a')
    parser.add_argument('--credentials-b')
    parser.add_argument('--timeout',type=int,default=900)
    parser.add_argument('--bootstrap-desktop', action='store_true')
    parser.add_argument('--bootstrap-desktop-config')
    args=parser.parse_args()
    try:
        return asyncio.run(execute(args))
    except BaseException as error:
        Path(args.artifacts).mkdir(parents=True,exist_ok=True)
        selected=failed_fallback(args,error)
        if args.proof=='first_a':
            import first_a
            first_a.write_json(Path(args.artifacts)/(args.proof+'.json'),selected)
        else:(Path(args.artifacts)/(args.proof+'.json')).write_text(json.dumps(selected))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
