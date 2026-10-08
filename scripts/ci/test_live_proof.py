"""Failure injection against the real required live gate and projection."""
import importlib.util
import argparse
import asyncio
import hashlib
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('hosted_desktop',HERE/'hosted_desktop.py')
host=importlib.util.module_from_spec(spec);spec.loader.exec_module(host)
live=host.live
HASH='a'*64


def valid_summary(proof):
    checks=[{'check':name,'passed':True} for name in live.CHECKS[proof]]
    for n,c in enumerate(c for c in checks if c['check'].endswith('.installed_source_bridge_and_cua_identity')):
        c.update(bridge_sha256=HASH,cua_version='0.34.0',cua_sha256='b'*64,
                 archive_sha256='c'*64,bundle_id='cua-driver-rs-v0.34.0-'+'d'*16,os_session_id=n+1)
    return {'mode':'live','status':'passed','checks':checks,'unverified':[], 'canvas_vs_native':{'mean_abs_diff':5}}


class ProjectionTests(unittest.TestCase):
    def test_every_required_check_and_live_identity_are_enforced(self):
        for proof in live.CHECKS:
            summary=valid_summary(proof)
            self.assertEqual(live.project(proof,summary,0,HASH)['status'],'passed')
            for i in range(len(summary['checks'])):
                altered=valid_summary(proof);altered['checks'].pop(i)
                self.assertEqual(live.project(proof,altered,0,HASH)['status'],'failed')
                altered=valid_summary(proof);altered['checks'][i]['passed']=False
                self.assertEqual(live.project(proof,altered,0,HASH)['status'],'failed')
            for mode in ('fake',None):
                self.assertEqual(live.project(proof,{**summary,'mode':mode},0,HASH)['status'],'failed')
            self.assertEqual(live.project(proof,summary,1,HASH)['status'],'failed')
            self.assertEqual(live.project(proof,summary,0,'e'*64)['status'],'failed')
            for unverified in ('Fault injection explicitly skipped','canvas vs native screenshot comparison (Pillow not available)','unknown coverage'):
                self.assertEqual(live.project(proof,{**summary,'unverified':[unverified]},0,HASH)['status'],'failed')
            self.assertEqual(live.project(proof,{**summary,'unverified':list(live.ALLOWED_UNVERIFIED[proof])},0,HASH)['status'],'passed')

    def test_guest_manifest_cua_and_independent_session_evidence(self):
        for proof in live.CHECKS:
            for key,value in (('bridge_sha256','e'*64),('cua_version','latest'),('cua_sha256','bad'),('archive_sha256',None),('bundle_id','unsafe-token-path'),('os_session_id',False)):
                summary=valid_summary(proof)
                next(c for c in summary['checks'] if c['check'].endswith('.installed_source_bridge_and_cua_identity'))[key]=value
                self.assertEqual(live.project(proof,summary,0,HASH)['status'],'failed')
        for proof in ('viewer','cua'):
            summary=valid_summary(proof)
            for c in summary['checks']:
                if c['check'].endswith('.installed_source_bridge_and_cua_identity'):c['os_session_id']=1
            self.assertEqual(live.project(proof,summary,0,HASH)['status'],'failed')
        summary=valid_summary('viewer');summary.pop('canvas_vs_native')
        self.assertEqual(live.project('viewer',summary,0,HASH)['status'],'failed')

    def test_untrusted_failures_are_never_projected(self):
        secret='password-token-lease-typed-marker'
        for proof in live.CHECKS:
            summary=valid_summary(proof);summary['failure']=secret
            summary['checks'].append({'check':secret,'passed':True})
            summary['checks'][0]['raw_tool_result']=secret
            summary['status']='failed'
            artifact=json.dumps(live.project(proof,summary,1,HASH))
            self.assertNotIn(secret,artifact)
            for raw in (None,[],secret,{'checks':secret},{'unverified':[{}]}):
                self.assertEqual(live.project(proof,raw,1,HASH)['status'],'failed')
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);(root/'raw.txt').write_text(secret)
            self.assertFalse(live.scan_files(root,[secret.encode()]))


class EarlyHarnessFailureTests(unittest.IsolatedAsyncioTestCase):
    async def test_primary_failure_survives_cleanup_scan_and_missing_summary(self):
        for secondary in (None,'cleanup','scan','scan_false','summary','summary_invalid'):
            with self.subTest(secondary=secondary):await self.inject_failure(secondary)

    async def test_extended_primary_survives_all_secondary_boundaries(self):
        message=('Internal: bridge bootstrap failed: rdpilot-bridge did not start '
                 '(possible security prompt in the guest, or a bridge that does not speak bridge protocol 2); '
                 'stages=rdpdr_file_access,launch_input_sent')
        payload=json.dumps({'error':{'code':'internal','message':message},'private':'private-lease-token'}).encode()
        expected={'cli_error_code':'internal','producer':'sdk_bootstrap','bootstrap_state':'observed',
                  'bootstrap_stages':['rdpdr_file_access','launch_input_sent']}
        for secondary in (None,'cleanup','scan','scan_false','summary','summary_invalid'):
            with self.subTest(secondary=secondary):await self.inject_failure(secondary,payload,expected)
        await self.inject_failure(None,payload,observer_error=True)
        for error in (TimeoutError,asyncio.CancelledError):
            await self.inject_failure(None,communication_error=error)

    async def test_complete_source_envelopes_through_actual_cli_and_wrapper(self):
        fixtures=[('internal','Internal: '+prefix+'private-lease-token',producer) for prefix,producer in (
            ('connection or authentication failed: connect_begin failed: ','sdk_connection'),
            ('TLS or certificate error: ','sdk_tls'),('bridge bootstrap failed: ','sdk_bootstrap'),
            ('session/transport error: ','sdk_session'),('invalid configuration: ','sdk_configuration'),
            ('DVC transport error: ','sdk_dvc'),('bridge rejected the request: ','sdk_bridge_rejection'),
            ('connect failed: ','daemon_connect'),('io error: ','daemon_io'),
            ('config resolution failed: ','daemon_configuration'))]
        fixtures += [('internal','unexpected daemon response: private-lease-token','cli_response'),
                     ('config','configuration error: private-lease-token','config_other'),
                     ('internal','secret Internal: bridge bootstrap failed: private-lease-token','unavailable')]
        fixtures += [(code,prefix+'private-lease-token','unavailable') for code,prefix in (
            ('missing-config','missing required configuration: '),('transport','transport error: '),
            ('bundle-unavailable','BundleUnavailable: '),('daemon-unreachable','daemon unreachable: '),
            ('daemon-incompatible','daemon compatibility version mismatch: '),('duplicate-session','DuplicateSession: '))]
        fixtures += [('config',f'configuration error: PasswordCommand for {target} failed: '+reason,'password_command')
                     for target in ('a','b') for reason in ('empty output','output is not valid UTF-8',
                     'timed out after 30 s','could not start the shell: private-lease-token',
                     'could not read its output: private-lease-token','could not wait for it: private-lease-token','exit code: 5')]
        for code,message,producer in fixtures:
            with self.subTest(producer=producer):
                observed=await self.inject_failure(None,json.dumps({'error':{'code':code,'message':message}}).encode(),
                    {'cli_error_code':code,'producer':producer})
                if producer=='password_command':self.assertNotEqual(observed['password_helper'],'unavailable')


    async def inject_failure(self,secondary,payload=None,expected=None,observer_error=False,communication_error=None):
        actions=[]
        class Child:
            pid=42
            def __init__(self,code):self.returncode=code
            async def communicate(self):
                if communication_error:raise communication_error()
                return payload or b'',b'secretcredential private-lease-token'
            async def wait(self):return self.returncode
            def terminate(self):
                actions.append('terminate')
                if secondary=='cleanup':raise PermissionError('private-lease-token')
                self.returncode=0
            def kill(self):self.returncode=1
        async def spawn(*argv,**kwargs):
            if len(argv)>1 and argv[1]=='connect':
                hosts=Path(argv[argv.index('-F')+1]).read_text()
                self.assertEqual(hosts.count('  CuaVersion "cua-driver-rs-v0.34.0"\n'),2)
                (private/'raw/protected.png').write_bytes(b'private-lease-token')
            return Child(None if Path(argv[0]).name=='rdpilot-daemon' else (7 if argv[1]=='connect' else 0))
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);private=root/'private';private.mkdir()
            for target in ('a','b'):
                (private/(target+'.json')).write_text(json.dumps({'host':'127.0.0.1','port':3389,'username':'owned','password':'secretcredential'}))
            args=argparse.Namespace(proof='cua',bin_dir=str(root/'bin'),bundle=str(root/'bundle'),output=str(private/'raw'),
                artifacts=str(root/'artifacts'),source_bridge_sha256=HASH,journal=str(private/'processes.json'),
                credentials_a=str(private/'a.json'),credentials_b=str(private/'b.json'),timeout=10)
            original_scan=live.scan_files
            def scan(path,needles):
                actions.append('scan')
                if secondary=='scan':raise PermissionError('private-lease-token')
                if secondary=='scan_false':return False
                if secondary=='summary':(private/'raw/summary.json').unlink()
                if secondary=='summary_invalid':(private/'raw/summary.json').write_text('private-lease-token')
                return original_scan(path,needles)
            sys.path.insert(0,str(live.E2E));import proof_support
            original_observer=proof_support.cli_observation
            def observer(data):
                if observer_error:raise PermissionError('private-lease-token')
                return original_observer(data)
            with patch.object(tempfile,'tempdir',str(private)),patch.object(asyncio,'create_subprocess_exec',side_effect=spawn),patch.object(live,'scan_files',side_effect=scan),patch.object(live,'process_identity',return_value={'pid':42,'created':'1','image':'fixture'}),patch.object(proof_support,'cli_observation',side_effect=observer):
                self.assertEqual(await live.execute(args),1)
            selected=json.loads((root/'artifacts/cua.json').read_text())
            self.assertEqual(selected['status'],'failed')
            base={'operation':'connect_cli','exception_category':'timeout' if communication_error==TimeoutError else 'other' if communication_error else 'proof_assertion'}
            if not communication_error:base['cli_exit_code']=7
            self.assertEqual({k:v for k,v in selected['harness_failure'].items() if k!='connect_observation'},base)
            observed=selected['harness_failure']['connect_observation']
            if expected:
                self.assertEqual(observed['daemon'],{'state':'alive'})
                for target in ('a','b'):self.assertEqual(observed['relays'][target]['accepted'],0)
                for key,value in expected.items():self.assertEqual(observed[key],value)
            else:self.assertEqual(observed['cli_error_code'],'unavailable')
            self.assertEqual(selected['harness_failure'],args._primary_failure)
            if observer_error:self.assertEqual(observed['observation_state'],'observation_failed')
            fallback=live.failed_fallback(args,PermissionError('private-lease-token'))
            self.assertEqual(fallback['harness_failure'],selected['harness_failure'])
            self.assertNotIn('private-lease-token',json.dumps(fallback))
            self.assertNotIn('a.rdp_bundle_ready',selected['completed_checks'])
            self.assertNotIn('secretcredential',json.dumps(selected))
            self.assertNotIn('private-lease-token',json.dumps(selected))
            self.assertFalse(list((root/'artifacts').glob('*.png')))
            self.assertIn('scan',actions)
            self.assertFalse(any(p.name.startswith('rdpilot-cua-') for p in private.iterdir()))
            if secondary:
                self.assertTrue(selected['secondary_failures'])
            if secondary!='summary' and not communication_error:self.assertIn('private-lease-token',(private/'raw/summary.json').read_text())
            return observed


class PinnedCuaConfigurationTests(unittest.TestCase):
    def test_actual_hosts_writers_use_release_tag_and_keep_semantic_version(self):
        sys.path.insert(0,str(live.E2E))
        for proof,filename,constructor in (
            ('cua','run-cua-e2e.py','Run'),
            ('viewer','run-viewer-proof.py','Proof'),
            ('takeover','run-takeover-proof.py','Proof'),
        ):
            with self.subTest(proof=proof),tempfile.TemporaryDirectory() as directory:
                spec=importlib.util.spec_from_file_location('pin_'+proof,live.E2E/filename)
                module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
                writer=object.__new__(getattr(module,constructor))
                writer.temp=Path(directory)
                writer.args=argparse.Namespace(cua_version='0.34.0')
                hosts=writer.hosts_file({'a':{'host':'127.0.0.1','username':'standard'}}).read_text()
                self.assertIn('  CuaVersion "cua-driver-rs-v0.34.0"\n',hosts)
                self.assertNotIn('  CuaVersion "0.34.0"',hosts)
                self.assertEqual(writer.args.cua_version,'0.34.0')
                writer.args.cua_version=None
                self.assertNotIn('CuaVersion',writer.hosts_file({'a':{'host':'127.0.0.1','username':'standard'}}).read_text())


class OrchestrationModeTests(unittest.TestCase):
    def measure(self,mode,*,failure=None,cache_fixture=None):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);bins=root/'bin';bins.mkdir();output=root/'out'
            for name in ('rdpilot','rdpilot-daemon','rdpilot-mcp','rdpilot-bridge'):
                (bins/(name+'.exe')).write_bytes(name.encode())
            if failure=='binary':(bins/'rdpilot-mcp.exe').unlink()
            args=argparse.Namespace(output=str(output),bin_dir=str(bins),expected_commit='1'*40,
                setup_diagnostic=mode=='setup_diagnostic',cua_diagnostic=mode=='cua_diagnostic')
            host.initialize(output,mode)
            self.assertEqual(json.loads((output/'artifacts/gate.json').read_text())['mode'],mode)
            commands=[];actions=[]
            snapshot={'cache':str(root/'owned-cache'),'registry':[{'path':'reg0','name':'a','present':True,'value':1},{'path':'reg1','name':'b','present':True,'value':1}],
                'service':{'state':'Stopped','start_mode':'Manual'},'os':'Windows test boundary','image_os':'fixture','image_version':'fixture','computer':'fixture'}
            def control(script,values=None,timeout=30,**kwargs):
                actions.append((script,values))
                if script==host.PREFLIGHT:return snapshot
                if script==host.CREATE_USER:return {'name':values['name'],'sid':'S-1-5-21-'+str(len(actions))}
                if script==host.ENSURE_GROUP:return {'initially_member':False,'membership_verified':True}
                if script==host.ROLE:return {'rdp_member':True,'users_member':True,'administrator_member':False}
                if script==host.PROFILE:return {'profile_was_present':failure!='profile','profile_guest_removed':True}
                if script.startswith('$ok='):return True
                if "[bool]($p -and $p.Loaded)" in script:return False
                if "Diagnostic1!FixedHarmlessInput" in script:return {'converted':True,'security_module_major':3}
                if failure=='host_cleanup' and values and values.get('path')=='reg0':raise host.HostActionError('access_denied')
                return None
            class Child:
                pid=42
                def wait(self,timeout):
                    if failure=='timeout':raise subprocess.TimeoutExpired('private-command',timeout)
                    return 1 if failure in ('harness','observer','observer_write') else 0
            def spawn(command,**kwargs):
                commands.append(command)
                self.assertIn('run-live-proof.py',command[1])
                proof=command[2]
                def option(name):return command[command.index(name)+1]
                expected=hashlib.sha256((bins/'rdpilot-bridge.exe').read_bytes()).hexdigest()
                self.assertEqual(option('--source-bridge-sha256'),expected)
                self.assertEqual(Path(option('--bin-dir')),bins)
                self.assertEqual(list(Path(option('--bundle')).iterdir()),[Path(option('--bundle'))/'rdpilot-bridge.exe'])
                for key in ('--credentials-a','--credentials-b','--journal','--output'):
                    self.assertTrue(Path(option(key)).is_relative_to(output/'private'))
                self.assertEqual(json.loads(Path(option('--credentials-a')).read_text())['host'],'127.0.0.1')
                self.assertTrue(any('icacls' in script for script,_ in actions))
                summary=valid_summary(proof)
                for check in summary['checks']:
                    if check['check'].endswith('.installed_source_bridge_and_cua_identity'):check['bridge_sha256']=expected
                if failure=='checks':summary['checks']=[c for c in summary['checks'] if c['check']!='a.actual_cua_stall_bounded_close']
                if failure=='scan_check':summary['checks']=[c for c in summary['checks'] if c['check']!='evidence_contains_no_credentials']
                if failure in ('harness','observer','observer_write'):summary['status']='failed'
                (output/'artifacts'/(proof+'.json')).write_text(json.dumps(live.project(proof,summary,0,expected)))
                if failure=='artifact_missing':(output/'artifacts'/(proof+'.json')).unlink()
                if failure=='artifact_invalid':(output/'artifacts'/(proof+'.json')).write_text('private-lease-token')
                Path(option('--journal')).write_text('[]')
                (output/'artifacts'/'selected.png').write_bytes(b'fixture image')
                if cache_fixture:cache_fixture(Path(snapshot['cache']), (bins/'rdpilot-bridge.exe').read_bytes())
                return Child()
            original_observe=host.acquisition_observer.observe_cache
            original_write=Path.write_text
            def observe(cache,expected):
                actions.append(('observe_cache',None))
                if failure=='observer':raise PermissionError('private-lease-token')
                result=original_observe(cache,expected)
                actions.append(('observed_state',result['state']))
                return result
            def write(path,*argv,**kwargs):
                if path.name=='local-acquisition.json':
                    actions.append(('acquisition_write',None))
                    if failure=='observer_write':raise PermissionError('private-lease-token')
                return original_write(path,*argv,**kwargs)
            def identity(command,**kwargs):return ('2'*40 if command[-1]=='HEAD^{tree}' else ('3'*40 if failure=='identity' else '1'*40))+'\n'
            with contextlib.redirect_stdout(io.StringIO()),patch.dict(os.environ),patch.object(host,'powershell',side_effect=control),patch.object(host,'measure_process_refusal'),patch.object(host.subprocess,'check_output',side_effect=identity),patch.object(host.subprocess,'Popen',side_effect=spawn),patch.object(live,'process_identity',return_value={'pid':42,'created':'1','image':'owned'}),patch.object(host.acquisition_observer,'observe_cache',side_effect=observe),patch.object(Path,'write_text',autospec=True,side_effect=write):
                code=host.run(args)
            gate=json.loads((output/'artifacts/gate.json').read_text());cleanup=json.loads((output/'artifacts/cleanup.json').read_text())
            self.assertFalse((output/'private').exists())
            self.assertEqual(gate['mode'],mode)
            if failure:self.assertFalse((output/'artifacts/selected.png').exists())
            return code,gate,cleanup,commands,actions

    def test_actual_modes_keep_distinct_required_suites_and_success_labels(self):
        for mode,expected,status in (('live',['cua','viewer','takeover'],'passed'),('cua_diagnostic',['cua'],'cua_diagnostic_passed'),('setup_diagnostic',['host_setup'],'setup_passed')):
            with self.subTest(mode=mode):
                code,gate,cleanup,commands,_=self.measure(mode)
                self.assertEqual(code,0);self.assertEqual(gate['status'],status)
                self.assertEqual(gate['attempted_suites'],expected);self.assertEqual(gate['completed_suites'],expected)
                self.assertEqual([cmd[2] for cmd in commands],[] if mode=='setup_diagnostic' else expected)
                for proof in expected:
                    if mode!='setup_diagnostic':self.assertTrue(cleanup[proof]['user_0_profile_was_present'])

    def test_cua_failures_remain_red_and_continue_owned_cleanup(self):
        for failure in ('binary','identity','checks','scan_check','harness','timeout','profile','host_cleanup'):
            with self.subTest(failure=failure):
                code,gate,cleanup,commands,actions=self.measure('cua_diagnostic',failure=failure)
                self.assertEqual(code,1);self.assertEqual(gate['status'],'failed')
                self.assertNotIn('viewer',gate['attempted_suites']);self.assertNotIn('takeover',gate['attempted_suites'])
                if failure!='host_cleanup':self.assertEqual(gate['completed_suites'],[])
                if failure not in ('binary','identity'):
                    self.assertIn('service',cleanup['host']);self.assertTrue(cleanup['host']['service'])
                    self.assertTrue(cleanup['cua']['user_1_account'])
                if failure in ('checks','scan_check','harness','timeout'):self.assertEqual(gate['failure_stage'],'cua_harness')
                if failure=='profile':self.assertFalse(cleanup['cua']['user_0_profile_was_present'])
                if failure=='host_cleanup':self.assertFalse(cleanup['host']['registry_0']);self.assertTrue(cleanup['host']['registry_1'])

    def test_actual_cli_rejects_unknown_or_combined_modes(self):
        for flags in (['--setup-diagnostic','--cua-diagnostic'],['--unknown-diagnostic']):
            child=subprocess.run([sys.executable,str(Path(host.__file__)),'--output','unused',*flags],capture_output=True)
            self.assertNotEqual(child.returncode,0)

    def test_cli_initialization_keeps_diagnostic_label_before_build(self):
        for flag,mode in (('--cua-diagnostic','cua_diagnostic'),('--setup-diagnostic','setup_diagnostic')):
            with tempfile.TemporaryDirectory() as directory:
                output=Path(directory)/'initialized'
                child=subprocess.run([sys.executable,str(Path(host.__file__)),flag,'--initialize','--output',str(output)],capture_output=True)
                self.assertEqual(child.returncode,0)
                self.assertEqual(json.loads((output/'artifacts/gate.json').read_text()),{'mode':mode,'status':'failed','failure_stage':'build_or_dependencies'})


class OwnershipTests(unittest.TestCase):
    def test_actual_windows_process_refusals_are_required(self):
        self.assertIn('measure_process_refusal()',Path(host.__file__).read_text())
        self.assertIn('StartTime.ToUniversalTime().ToFileTimeUtc()',host.STOP_PROCESS)
        self.assertIn('$p.Path -ine $v.image',host.STOP_PROCESS)
        for proof in ('viewer','takeover'):
            self.assertIn('windows_console_controller_and_sibling_survived',live.CHECKS[proof])

    def test_corrupted_process_journal_cannot_skip_other_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            journal=Path(directory)/'pids.json';journal.write_text('corrupt')
            called=[]
            def control(script,values=None,timeout=30):
                called.append(script);return False
            with patch.object(host,'powershell',side_effect=control):
                results=host.cleanup_suite([{'name':'owned','sid':'S-1-5-21-42'}],journal,'owned-cache')
            self.assertFalse(results['owned_processes_removed'])
            self.assertTrue(results['user_0_profile_guest_removed'])
            self.assertTrue(results['native_cache_removed'])
            self.assertTrue(any('Remove-LocalUser' in s for s in called))

    def test_failed_or_timed_out_cleanup_does_not_prevent_later_cleanup(self):
        for error in (RuntimeError('private failure'),subprocess.TimeoutExpired('private operation',1)):
            calls=[]
            def failed():
                calls.append('first');raise error
            result=host.cleanup_actions([('first',failed),('second',lambda:calls.append('second'))])
            self.assertEqual(calls,['first','second']);self.assertEqual(result,{'first':False,'second':True})

    def test_owned_cleanup_invokes_all_stages_and_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            journal=Path(directory)/'pids.json';journal.write_text('[]')
            user={'name':'rdpowned','sid':'S-1-5-21-42'}
            scripts=[]
            def control(script,values=None,timeout=30):
                scripts.append(script);self.assertLessEqual(timeout,30)
                if script.startswith('$ok='):raise RuntimeError('early process failure')
                return False
            with patch.object(host,'powershell',side_effect=control):
                results=host.cleanup_suite([user],journal,'owned-cache')
            self.assertFalse(results['owned_processes_removed'])
            self.assertTrue(results['user_0_profile_guest_removed'])
            self.assertTrue(results['user_0_account'])
            self.assertTrue(results['native_cache_removed'])
            self.assertTrue(any('WTSEnumerateSessions' in s for s in scripts))
            self.assertIn('$profile.Special -or $profile.Loaded',host.PROFILE)
            self.assertIn('$u.SID.Value -ne $v.sid',host.REMOVE_USER)
            self.assertIn('StartTime.ToUniversalTime().ToFileTimeUtc()',host.STOP_PROCESS)
            self.assertIn('$p.Path -ine $v.image',host.STOP_PROCESS)

    def test_host_action_diagnostics_are_closed_categories(self):
        for raw in ('member_exists','parameter_binding','user_not_found','untrusted-password-lease-token'):
            child=subprocess.CompletedProcess([],1,raw.encode(),b'#< CLIXML\nprivate module output')
            with patch.object(host.subprocess,'run',return_value=child):
                with self.assertRaises(host.HostActionError) as caught:host.powershell('trusted-script')
            self.assertIn(caught.exception.code,host.ERROR_CODES)
            self.assertNotIn('untrusted',str(caught.exception))
        with patch.object(host.subprocess,'run',side_effect=subprocess.TimeoutExpired('secret-command',1)):
            with self.assertRaises(host.HostActionError) as caught:host.powershell('trusted-script')
        self.assertEqual(caught.exception.code,'timeout')

    def test_closed_error_envelope_survives_windows_stderr_clixml(self):
        detail={'code':'host_action','subaction':'new_local_user','exception_category':'local_accounts','hresult':-2146233088,'native_error':2224}
        child=subprocess.CompletedProcess([],1,json.dumps(detail).encode(),b'#< CLIXML\nprivate module output and credential')
        with patch.object(host.subprocess,'run',return_value=child):
            with self.assertRaises(host.HostActionError) as caught:host.powershell('trusted-script')
        self.assertEqual(caught.exception.detail,{key:value for key,value in detail.items() if key!='code'})
        self.assertNotIn('credential',str(caught.exception))

    def test_windows_powershell_child_uses_native_module_defaults(self):
        inherited={'PSMODULEPATH':'incompatible-core-modules','KEEP_PARENT':'unchanged'}
        child=subprocess.CompletedProcess([],0,b'true',b'')
        with patch.dict(os.environ,inherited),patch.object(host.subprocess,'run',return_value=child) as run:
            self.assertTrue(host.powershell('trusted-script'))
            self.assertFalse(any(key.casefold()=='psmodulepath' for key in run.call_args.kwargs['env']))
            self.assertEqual(run.call_args.kwargs['env']['KEEP_PARENT'],'unchanged')
            self.assertEqual(os.environ['PSMODULEPATH'],inherited['PSMODULEPATH'])
            host.powershell('trusted-script',inherit_module_path=True)
            self.assertEqual(run.call_args.kwargs['env']['PSMODULEPATH'],inherited['PSMODULEPATH'])

    def test_profile_presence_after_live_run_is_required(self):
        with tempfile.TemporaryDirectory() as directory:
            journal=Path(directory)/'pids.json';journal.write_text('[]')
            def control(script,values=None,timeout=30):
                if script.startswith('$ok='):return True
                if script==host.PROFILE:return {'profile_was_present':False,'profile_guest_removed':True}
                return False
            with patch.object(host,'powershell',side_effect=control):
                results=host.cleanup_suite([{'name':'owned','sid':'S-1-5-21-42'}],journal,'cache',require_profiles=True)
            self.assertFalse(results['user_0_profile_was_present'])
            self.assertFalse(results['user_0_profile_guest_removed'])

    def test_real_child_tempfile_stays_in_selected_private_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            private=Path(directory)/'private';private.mkdir()
            with patch.dict(os.environ,{'TMPDIR':str(Path(directory)/'unowned')}):
                env=host.private_environment(private)
            for name in ('TMPDIR','TEMP','TMP'):self.assertEqual(Path(env[name]),private.resolve())
            child=subprocess.run([sys.executable,'-c','import tempfile; print(tempfile.mkdtemp())'],env=env,capture_output=True,text=True,check=True)
            self.assertTrue(Path(child.stdout.strip()).resolve().is_relative_to(private.resolve()))
        for checks in live.CHECKS.values():self.assertIn('private_child_temporary_boundary_verified',checks)

    def test_setup_diagnostic_is_separate_from_required_live_gate(self):
        workflow=(HERE.parents[1]/'.github/workflows/hosted-desktop-probe.yml').read_text()
        self.assertIn('workflow_dispatch:',workflow)
        self.assertNotIn('  push:',workflow)
        self.assertIn('--setup-diagnostic',workflow)
        setup=workflow.split('  setup:\n')[1].split('  cua:\n')[0]
        self.assertNotIn('cargo build',setup)
        ci=(HERE.parents[1]/'.github/workflows/ci.yml').read_text()
        self.assertNotIn('--setup-diagnostic',ci)
        self.assertIn("'setup_passed'",Path(host.__file__).read_text())
        unsafe={'subaction':{'token':'secret'},'exception_category':'secret','hresult':'secret','native_error':True}
        error=host.HostActionError('unknown-secret-code',unsafe)
        self.assertEqual(error.code,'host_action')
        self.assertNotIn('secret',json.dumps(error.detail))

    def test_static_build_failure_artifact_is_initialized_before_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            output=Path(directory)/'output';host.initialize(output)
            self.assertEqual(json.loads((output/'artifacts/gate.json').read_text())['status'],'failed')
            self.assertFalse((output/'private').exists())
        workflow=(HERE.parents[1]/'.github/workflows/ci.yml').read_text().split('  desktop:\n')[1].split('  success:\n')[0]
        self.assertLess(workflow.index('Initialize safe desktop evidence'),workflow.index('Live browser dependencies'))
        self.assertIn('if-no-files-found: error',workflow)
        self.assertIn('/desktop-proof/artifacts/',workflow)
        self.assertNotIn('/private',workflow)


if __name__=='__main__':unittest.main()
