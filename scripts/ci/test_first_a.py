"""Protected FIRST-A policy controls; product/Windows effects are synthetic."""
import argparse
import asyncio
import copy
import contextlib
import hashlib
import importlib.util
import json
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

import first_a as policy
import hosted_desktop as host

sys.path.insert(0,str(host.live.E2E))
spec=importlib.util.spec_from_file_location('first_a_cua',host.live.E2E/'run-cua-e2e.py')
cua=importlib.util.module_from_spec(spec);spec.loader.exec_module(cua)
HASH='a'*64


def raw_identity(expected=HASH):
    archive='c'*64
    bundle='cua-driver-rs-v0.34.0-'+hashlib.sha256((expected+archive).encode()).hexdigest()[:16]
    return {'bridge_sha256':expected,'cua_sha256':'b'*64,'installed_bundle_id':bundle,
        'installed_under_localappdata':True,'session_id':1,
        'manifest':{'bridge_sha256':expected,'archive_sha256':archive,'cua_version':'0.34.0',
            'bundle_id':bundle,'archive_name':policy.files.ARCHIVE,'files':{'cua-driver.exe':'b'*64}}}


class IdentityTests(unittest.TestCase):
    def test_exact_installed_identity_and_all_manifest_controls(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'installed-a.json'
            raw=raw_identity();path.write_text(json.dumps(raw))
            identity=policy.installed_identity(path,HASH)
            self.assertEqual(set(identity),policy.IDENTITY_KEYS)
            for key,value in (('installed_under_localappdata',1),('session_id',True),('session_id',0),
                    ('session_id',1.5),('bridge_sha256','e'*64),('cua_sha256','e'*64),('installed_bundle_id','old'),('extra','private-marker')):
                changed=copy.deepcopy(raw);changed[key]=value;path.write_text(json.dumps(changed))
                with self.subTest(key=key,value=value),self.assertRaises(ValueError):policy.installed_identity(path,HASH)
            for key,value in (('bridge_sha256','e'*64),('archive_sha256','bad'),('cua_version','latest'),
                    ('archive_name','unknown.zip'),('bundle_id','cua-driver-rs-v0.34.0-'+'d'*16),('extra','private-marker'),
                    ('files',{'../cua-driver.exe':'b'*64}),('files',{'cua-driver.exe':'bad'})):
                changed=copy.deepcopy(raw);changed['manifest'][key]=value;path.write_text(json.dumps(changed))
                with self.subTest(manifest=key),self.assertRaises(ValueError):policy.installed_identity(path,HASH)
            for key in raw:
                changed=copy.deepcopy(raw);del changed[key];path.write_text(json.dumps(changed))
                with self.subTest(missing=key),self.assertRaises(ValueError):policy.installed_identity(path,HASH)
            path.write_text(json.dumps(raw)[:-1]+',"session_id":2}')
            with self.assertRaises(ValueError):policy.installed_identity(path,HASH)

    def test_positive_projection_and_exact_parent_admission(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'installed-a.json';path.write_text(json.dumps(raw_identity()))
            identity=policy.installed_identity(path,HASH)
            checks=[{'check':name,'passed':True} for name in policy.CHECKS]
            checks[-1].update({k:v for k,v in identity.items() if k!='installed_under_localappdata'})
            run=SimpleNamespace(output=Path(directory),checks=checks+[{'check':'evidence_contains_no_credentials','passed':True}],
                first_a_identity=identity,first_a_bridge_live=True)
            summary={'mode':'live','status':'passed','checks':checks,'unverified':['UAC/secure-desktop behavior is unsupported; no elevation guarantee']}
            selected=policy.project(summary,run,0,HASH,True)
            self.assertEqual(selected['status'],policy.PASSED);self.assertTrue(policy.admit(selected,HASH))
            for key,value in (('extra','private-marker'),('status','passed'),('completed_checks',[]),('outcome','validated')):
                altered=copy.deepcopy(selected);altered[key]=value
                self.assertFalse(policy.admit(altered,HASH))
            for key,value in (('os_session_id',True),('os_session_id',0),('archive_sha256','é'),('extra','private-marker'),('installed_under_localappdata',1),('bundle_id','stale')):
                altered=copy.deepcopy(selected);altered['identity'][0][key]=value
                self.assertFalse(policy.admit(altered,HASH))
            for code,scanned in ((1,True),(0,False)):
                self.assertEqual(policy.project(summary,run,code,HASH,scanned)['status'],'failed')
            for i in range(len(checks)):
                changed=copy.deepcopy(summary);changed['checks'][i]['passed']=1
                self.assertEqual(policy.project(changed,run,0,HASH,True)['status'],'failed')
            changed=copy.deepcopy(summary);changed['checks'][-1]['extra']='private-marker'
            self.assertEqual(policy.project(changed,run,0,HASH,True)['status'],'failed')
            run.first_a_bridge_live=1
            self.assertEqual(policy.project(summary,run,0,HASH,True)['status'],'failed')
            path.unlink()
            self.assertEqual(policy.project(summary,run,0,HASH,True)['status'],'failed')

    def test_negative_artifact_keeps_closed_primary_and_inconclusive_collection(self):
        primary={'operation':'connect_cli','exception_category':'proof_assertion','cli_exit_code':1}
        result=policy.project(None,SimpleNamespace(primary_failure=primary),1,HASH,False)
        self.assertEqual(result['outcome'],'first_a_failed')
        result=policy.project(None,SimpleNamespace(primary_failure={'operation':'first_a_attach','exception_category':'timeout'}),1,HASH,False)
        self.assertEqual(result['outcome'],'inconclusive')
        closed=policy.failed_selected({'status':'failed','raw':'private-marker','identity':[{'password':'private-marker'}],
            'harness_failure':{**primary,'message':'private-marker'},'secondary_failures':[{'message':'private-marker'}]})
        self.assertEqual(closed['harness_failure'],primary)
        self.assertNotIn('private-marker',json.dumps(closed))
        self.assertEqual(closed['status'],'failed')


class GuardTests(unittest.TestCase):
    def test_cli_and_in_process_conflicts_refused_before_allocation(self):
        for flags in (['--baseline-first-a','--cua-diagnostic'],['--baseline-first-a','--setup-diagnostic'],
                ['--baseline-first-a','--bootstrap-desktop'],['--baseline-first-a','--bootstrap-footprint']):
            with tempfile.TemporaryDirectory() as directory:
                out=Path(directory)/'unused'
                result=subprocess.run([sys.executable,str(Path(host.__file__)),'--initialize','--output',str(out),*flags],capture_output=True)
                self.assertNotEqual(result.returncode,0);self.assertFalse(out.exists())
        with tempfile.TemporaryDirectory() as directory:
            out=Path(directory)/'out'
            host.initialize(out,policy.MODE)
            self.assertEqual(policy.read_json(out/'artifacts/gate.json'),{'mode':policy.MODE,'status':'failed','failure_stage':'build_or_dependencies'})

    def test_stale_gate_source_or_extra_artifact_refused_before_private_resources(self):
        for failure in ('gate','source','extra'):
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as directory:
                out=Path(directory)/'out';host.initialize(out,policy.MODE)
                source={'source_commit':'1'*40};policy.write_json(out/'artifacts/source.json',source)
                if failure=='gate':policy.write_json(out/'artifacts/gate.json',{'status':'passed'})
                if failure=='source':policy.write_json(out/'artifacts/source.json',{})
                if failure=='extra':(out/'artifacts/stale.json').write_text('{}')
                args=SimpleNamespace(output=str(out),expected_commit='1'*40,baseline_first_a=True)
                with patch.object(policy,'verify_source',return_value=source),patch.object(host,'powershell') as native:
                    with self.assertRaises(ValueError):host.run(args)
                native.assert_not_called();self.assertFalse((out/'private').exists())

    def test_listener_attempt_budget_and_client_disposal(self):
        ticks=[0.0];calls=[]
        def connect(address,timeout):
            calls.append((address,timeout));ticks[0]+=timeout;raise OSError()
        def sleep(seconds):ticks[0]+=seconds
        with patch.object(policy.time,'monotonic',side_effect=lambda:ticks[0]),patch.object(policy.time,'sleep',side_effect=sleep),patch.object(policy.socket,'create_connection',side_effect=connect):
            with self.assertRaises(TimeoutError):policy.listener_ready()
        self.assertEqual(len(calls),30);self.assertEqual(ticks[0],60)
        self.assertTrue(all(address==('127.0.0.1',3389) and timeout<=1 for address,timeout in calls))
        actions=[]
        class Client:
            def __enter__(self):actions.append('enter');return self
            def __exit__(self,*args):actions.append('dispose')
        with patch.object(policy.socket,'create_connection',return_value=Client()):policy.listener_ready()
        self.assertEqual(actions,['enter','dispose'])

    def test_plain_scans_byte_count_deadline_and_secret_boundary(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);path=root/'raw.log';path.write_bytes(b'x'*65534+b'private-marker')
            self.assertFalse(policy.scan(root,[b'private-marker']))
            self.assertTrue(policy.scan(root,[b'absent']))
            with self.assertRaises(TimeoutError):policy.scan(root,[],time.monotonic()-1)
            path.write_bytes(b'x'*(16*1024*1024+1))
            with self.assertRaises(policy.files.BudgetExceeded):policy.scan(root,[])

    def test_frozen_source_needs_no_historical_objects_and_refuses_other_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            def git(*args):return subprocess.check_output(['git','-C',str(root),*args],stderr=subprocess.DEVNULL)
            git('init','-q')
            for name in ('Cargo.toml','Cargo.lock','crates/src.rs','scripts/ci/first_a.py','README.md'):
                path=root/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(name)
            def commit():
                git('add','.');git('-c','user.name=LocalTest','-c','user.email=local@example.invalid','commit','-qm','fixture')
                return git('rev-parse','HEAD').decode().strip()
            sha=commit();allow={'scripts/ci/first_a.py'}
            entries=git('ls-tree','-r','-z','HEAD').split(b'\0')
            digest=hashlib.sha256(b'\0'.join(e for e in entries if e and e.split(b'\t')[1].decode() not in allow)).hexdigest()
            objects={name:git('rev-parse','HEAD:'+name).decode().strip() for name in policy.PRODUCT}
            original=subprocess.check_output
            original_run=subprocess.run
            def output(argv,**kwargs):return original(['git','-C',str(root),*argv[1:]],**kwargs)
            def run(argv,**kwargs):return original_run(['git','-C',str(root),*argv[1:]],**kwargs)
            with patch.object(policy,'ALLOWLIST',allow),patch.object(policy,'PROTECTED_DIGEST',digest),patch.object(policy,'PRODUCT',objects),patch.object(policy.subprocess,'check_output',side_effect=output),patch.object(policy.subprocess,'run',side_effect=run):
                self.assertEqual(policy.verify_source(sha)['source_commit'],sha)
                (root/'scripts/ci/first_a.py').write_text('reviewed harness change')
                with self.assertRaises(subprocess.CalledProcessError):policy.verify_source(sha)
                sha=commit();self.assertEqual(policy.verify_source(sha)['source_commit'],sha)
                (root/'README.md').write_text('unreviewed input');sha=commit()
                with self.assertRaises(ValueError):policy.verify_source(sha)


class AdapterTests(unittest.IsolatedAsyncioTestCase):
    async def test_one_a_connect_attach_uses_current_provenance_and_no_other_phases(self):
        with tempfile.TemporaryDirectory() as directory:
            adapter=policy.run_class(cua.Run)
            run=object.__new__(adapter);run.output=Path(directory);run.args=SimpleNamespace(source_bridge_sha256=HASH)
            run.connect=AsyncMock(return_value={'bridge_live':True})
            async def attach(target):(run.output/'installed-a.json').write_text(json.dumps(raw_identity()))
            run.attach=AsyncMock(side_effect=attach)
            await run.proof_body()
            run.connect.assert_awaited_once_with('a');run.attach.assert_awaited_once_with('a')
            self.assertEqual(run.first_a_identity['bridge_sha256'],HASH)
            self.assertEqual(run.daemon_executable,'rdpilot-daemon.exe');self.assertEqual(run.daemon_start_wait,1)
            self.assertEqual((run.work_budget,run.cleanup_budget),(900,60))
            with self.assertRaises(ValueError):await run.proof_body()
            self.assertEqual(run.connect.await_count,1)

    async def test_shared_provenance_creates_owned_transfer_root_before_write_and_get(self):
        with tempfile.TemporaryDirectory() as directory:
            run=SimpleNamespace(args=SimpleNamespace(source_bridge_sha256=HASH,cua_version='0.34.0'),output=Path(directory),check=lambda *a,**k:None)
            actions=[]
            class Endpoint:
                async def tool(self,name,arguments):
                    actions.append('launch');script=arguments['script']
                    self_script=script
                    if self_script.index('New-Item -ItemType Directory -Force -Path $transferRoot')>=self_script.index('Set-Content'):raise AssertionError('prerequisite order')
            async def get(*args,**kwargs):
                actions.append('get');(run.output/'installed-a.json').write_text(json.dumps(raw_identity()))
            run.cli=get
            support=SimpleNamespace(psquote=lambda value:repr(value),powershell=lambda script:{'script':script},require=cua.require)
            await cua.proof_support.installed_identity(run,'a',Endpoint(),support)
            self.assertEqual(actions,['launch','get'])

    async def test_attach_deadline_joins_phase_and_truthy_bridge_never_admits(self):
        with tempfile.TemporaryDirectory() as directory:
            run=object.__new__(policy.run_class(cua.Run));run.output=Path(directory);run.args=SimpleNamespace(source_bridge_sha256=HASH)
            run.connect=AsyncMock(return_value={'bridge_live':1});run.attach=AsyncMock()
            with self.assertRaises(ValueError):await run.proof_body()
            run.attach.assert_not_awaited()
            run.connect=AsyncMock(return_value={'bridge_live':True});joined=[]
            async def attach(target):
                try:await asyncio.Future()
                finally:joined.append(target)
            run.attach=attach
            with patch.object(policy,'ATTACH_SECONDS',0.01):
                with self.assertRaises(TimeoutError):await run.proof_body()
            self.assertEqual(joined,['a']);self.assertEqual(run.operation,'first_a_attach')

    async def test_work_timeout_keeps_separate_finalizer_and_later_actions(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);private=root/'private';private.mkdir()
            for target in ('a','b'):(private/(target+'.json')).write_text(json.dumps({'host':'127.0.0.1','password':'private-password'}))
            args=argparse.Namespace(proof='first_a',bin_dir=str(root/'bin'),bundle=str(root/'bundle'),output=str(private/'raw'),
                artifacts=str(root/'artifacts'),source_bridge_sha256=HASH,journal=str(private/'children.json'),
                credentials_a=str(private/'a.json'),credentials_b=str(private/'b.json'),timeout=100)
            actions=[]
            original=policy.run_class
            def factory(base):
                class Timed(original(base)):
                    work_budget=0.01
                    async def start_daemon(self,log):self.operation='live_checks'
                    async def proof_body(self):
                        try:await asyncio.Future()
                        finally:actions.append('work_joined')
                    async def cli(self,*args,**kwargs):
                        await asyncio.sleep(0.02);actions.append(('disconnect',args[-1]));return {}
                return Timed
            with patch.object(tempfile,'tempdir',str(private)),patch.object(policy,'run_class',side_effect=factory):
                self.assertEqual(await host.live.execute(args),1)
            selected=policy.read_json(root/'artifacts/first_a.json')
            self.assertEqual(selected['harness_failure']['exception_category'],'timeout')
            self.assertEqual(selected['outcome'],'inconclusive')
            self.assertEqual(actions,['work_joined',('disconnect','a'),('disconnect','b')])
            self.assertFalse(any(p.name.startswith('rdpilot-cua-') for p in private.iterdir()))


class HostPolicyTests(unittest.TestCase):
    def measure(self,failure=None):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);out=root/'out';bins=root/'bin';bins.mkdir()
            for name in ('rdpilot','rdpilot-daemon','rdpilot-mcp','rdpilot-bridge'):(bins/(name+'.exe')).write_bytes(name.encode())
            host.initialize(out,policy.MODE)
            args=argparse.Namespace(output=str(out),bin_dir=str(bins),expected_commit='1'*40,baseline_first_a=True)
            actions=[];created=[];cleanup_deadlines=[]
            source={'source_commit':'1'*40,'source_tree':'2'*40,'reviewed_base':policy.BASE_COMMIT,'product_objects':policy.PRODUCT,'protected_source_sha256':policy.PROTECTED_DIGEST}
            policy.write_json(out/'artifacts/source.json',source)
            snapshot={'cache':str(root/'cache'),'registry':[{'path':'reg0','name':'a','present':True,'value':1},{'path':'reg1','name':'b','present':True,'value':1}],
                'service':{'state':'Stopped','start_mode':'Manual'},'os':'synthetic','image_os':'synthetic','image_version':'synthetic','computer':'synthetic'}
            def verify(expected):actions.append('verify_source');return source
            def control(script,values=None,timeout=30,**kwargs):
                actions.append('native')
                if script==host.PREFLIGHT:return snapshot
                if script==host.CREATE_USER:
                    created.append(values['name'])
                    if failure=='partial_account' and len(created)==2:raise host.HostActionError('access_denied')
                    return {'name':values['name'],'sid':'S-1-5-21-'+str(len(created))}
                if script==host.ENSURE_GROUP:return {'initially_member':False,'membership_verified':True}
                if script==host.ROLE:return {'rdp_member':True,'users_member':True,'administrator_member':False}
                if 'fresh_profile_verified' in script:return {'fresh_profile_verified':1 if failure=='fresh' else True}
                if 'Creation ownership marker mismatch' in script:return {'name':values['name'],'sid':'S-1-5-21-2'}
                if script==host.PROFILE:
                    cleanup_deadlines.append(host._native_owner['deadline'])
                    return {'profile_was_present':values['name']==created[0] and failure not in ('profile','demotion_write','demotion_unlink')}
                if script==host.REMOVE_USER:actions.append('account_removed')
                if values and values.get('path')=='reg0':
                    cleanup_deadlines.append(host._native_owner['deadline'])
                    if failure=='host_cleanup':raise host.HostActionError('access_denied')
                if script.startswith('$ok='):return True
                return False
            class Child:
                pid=42
                def wait(self,timeout):return 0
                def kill(self):actions.append('wrapper_kill')
            def spawn(command,**kwargs):
                self.assertEqual(command[2],'first_a')
                def option(name):return command[command.index(name)+1]
                bridge=option('--source-bridge-sha256');raw=raw_identity(bridge)
                identity=policy.files.manifest_identity(raw['manifest'],bridge)
                identity.update(os_session_id=1,installed_under_localappdata=True)
                selected={'mode':policy.MODE,'status':policy.PASSED,'completed_checks':list(policy.CHECKS),
                    'identity':[identity],'outcome':'qualified','failure_stage':'none'}
                if failure=='malformed':selected['identity'][0]['os_session_id']=True
                if failure=='failed_fields':selected={'status':'failed','raw':'private-marker'}
                (out/'artifacts/first_a.json').write_text(json.dumps(selected))
                Path(option('--journal')).write_text('[]')
                rawdir=Path(option('--output'));rawdir.mkdir();(rawdir/'summary.json').write_text('{}')
                if failure=='private_scan':
                    password=json.loads(Path(option('--credentials-a')).read_text())['password']
                    (rawdir/'console.log').write_text(password)
                actions.append('child')
                return Child()
            def acquisition(cache,expected):
                raw=raw_identity(expected)
                return {'state':'absent'} if failure=='acquisition' else {'state':'verified_source_bundle','bundle_count':1,**policy.files.manifest_identity(raw['manifest'],expected)}
            original_write=policy.write_json;original_scan=policy.scan;original_read=policy.read_json;original_unlink=Path.unlink
            def unlink(path,*args,**kwargs):
                if failure=='demotion_unlink' and path.name=='first_a.json':raise OSError('synthetic removal refusal')
                return original_unlink(path,*args,**kwargs)
            def write(path,payload,**kwargs):
                if failure in ('demotion_write','demotion_unlink') and Path(path).name=='first_a.json':raise OSError('synthetic demotion refusal')
                if failure=='metadata_write' and Path(path).name=='cleanup.json':raise OSError('synthetic write')
                if failure=='gate_write' and Path(path).name=='gate.json' and payload.get('status')==policy.PASSED:raise OSError('synthetic write')
                return original_write(path,payload,**kwargs)
            def scan(path,*args,**kwargs):
                if failure=='selected_scan' and Path(path)==out/'artifacts':raise OSError('synthetic scan')
                return original_scan(path,*args,**kwargs)
            def read(path,*args,**kwargs):
                if failure=='child_read' and Path(path).name=='first_a.json':raise OSError('synthetic read')
                return original_read(path,*args,**kwargs)
            def revision(argv,**kwargs):return ('2'*40 if argv[-1]=='HEAD^{tree}' else '1'*40)+'\n'
            with contextlib.redirect_stdout(io.StringIO()),patch.dict(os.environ),patch.object(Path,'unlink',autospec=True,side_effect=unlink),patch.object(policy,'write_json',side_effect=write),patch.object(policy,'scan',side_effect=scan),patch.object(policy,'read_json',side_effect=read),patch.object(policy,'verify_source',side_effect=verify),patch.object(policy,'listener_ready'),patch.object(host,'powershell',side_effect=control),patch.object(host,'measure_process_refusal'),patch.object(host.subprocess,'check_output',side_effect=revision),patch.object(host.subprocess,'Popen',side_effect=spawn),patch.object(host.live,'process_identity',return_value={'pid':42,'created':'1','image':'private-image'}),patch.object(host.acquisition_observer,'observe_cache',side_effect=acquisition):
                code=host.run(args)
            self.assertIsNone(host._native_owner)
            self.assertEqual(actions[0],'verify_source')
            self.assertFalse((out/'private').exists())
            if cleanup_deadlines:self.assertEqual(len(set(cleanup_deadlines)),1)
            return code,policy.read_json(out/'artifacts/gate.json'),policy.read_json(out/'artifacts/cleanup.json') if (out/'artifacts/cleanup.json').exists() else {},policy.read_json(out/'artifacts/first_a.json') if (out/'artifacts/first_a.json').exists() else None,actions

    def test_same_host_positive_requires_a_profile_and_b_is_cleaned_without_profile(self):
        code,gate,cleanup,selected,actions=self.measure()
        self.assertEqual(code,0);self.assertEqual(gate['status'],policy.PASSED)
        self.assertEqual(gate['completed_suites'],['first_a'])
        self.assertTrue(cleanup['first_a']['user_0_profile_was_present'])
        self.assertNotIn('user_1_profile_was_present',cleanup['first_a'])
        self.assertEqual(actions.count('account_removed'),2)
        self.assertEqual(selected['status'],policy.PASSED)

    def test_missing_profile_acquisition_scan_schema_or_cleanup_never_qualifies(self):
        for failure in ('profile','acquisition','private_scan','malformed','failed_fields','host_cleanup','partial_account','metadata_write','gate_write','selected_scan','child_read','fresh','demotion_write','demotion_unlink'):
            with self.subTest(failure=failure):
                code,gate,cleanup,selected,actions=self.measure(failure)
                self.assertEqual(code,1);self.assertEqual(gate['status'],'failed')
                self.assertEqual(actions.count('account_removed'),2)
                if selected:
                    self.assertNotIn('private-marker',json.dumps(selected))
                    if failure=='demotion_unlink':
                        self.assertEqual(selected['status'],policy.PASSED)
                        self.assertIn('stale_child_removal',gate['observation_failures'])
                        self.assertEqual(policy.failed_selected(selected)['status'],'failed')
                    else:self.assertEqual(selected['status'],'failed')
                if cleanup:self.assertIn('service',cleanup['host'])


class LocalChildControls(unittest.TestCase):
    def test_protected_native_actual_child_is_joined_on_timeout_or_registration_failure(self):
        original=subprocess.Popen
        for failure in ('timeout','identity','journal'):
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as directory:
                children=[];error=ValueError('identity primary')
                def spawn(command,**kwargs):
                    self.assertEqual(command[0],'powershell.exe')
                    child=original([sys.executable,'-c','import time;time.sleep(120)'],**kwargs)
                    children.append(child);return child
                def identity(pid):
                    if failure=='identity':raise error
                    return {'pid':pid,'created':'1','image':'synthetic'}
                class FailedJournal:
                    def write_text(self,value):raise error
                owner={'deadline':time.monotonic()+3,'stop_failed':False,'records':[],
                    'journal':FailedJournal() if failure=='journal' else Path(directory)/'native.json'}
                try:
                    with patch.object(host,'_native_owner',owner),patch.object(host.subprocess,'Popen',side_effect=spawn),patch.object(host.live,'process_identity',side_effect=identity):
                        with self.assertRaises(host.HostActionError if failure=='timeout' else ValueError) as caught:host.powershell('synthetic',timeout=0.02)
                    if failure=='timeout':self.assertEqual(caught.exception.code,'timeout')
                    else:self.assertIs(caught.exception,error)
                    self.assertFalse(owner['stop_failed']);self.assertIsNotNone(children[0].returncode)
                finally:
                    for child in children:
                        if child.poll() is None:child.kill()
                        child.wait(timeout=3)
                        child.stdout.close();child.stderr.close()

    def test_atomic_metadata_failures_preserve_red_and_no_temporary_is_selected(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'gate.json';red={'status':'failed'};policy.write_json(path,red)
            primary=OSError('replace primary')
            with patch.object(policy.os,'replace',side_effect=primary):
                with self.assertRaises(OSError) as caught:policy.write_json(path,{'status':policy.PASSED})
            self.assertIs(caught.exception,primary);self.assertEqual(policy.read_json(path),red)
            self.assertEqual([p.name for p in path.parent.iterdir()],['gate.json'])
            for payload,needles in (({'value':'x'*65536},()),({'value':'private-marker'},(b'private-marker',))):
                with self.assertRaises(ValueError):policy.write_json(path,payload,needles=needles)
                self.assertEqual(policy.read_json(path),red)
            path.write_text('{"status":"failed","status":"passed"}')
            with self.assertRaises(ValueError):policy.read_json(path)


class AsyncLocalChildControls(unittest.IsolatedAsyncioTestCase):
    async def test_protected_native_request_deadline_includes_unmatched_stream_and_all_sends(self):
        for stage in ('notifications','initial_send','refusal_send'):
            with self.subTest(stage=stage):
                run=SimpleNamespace(work_budget=900,clean=lambda value:value)
                endpoint=cua.Mcp(run,'a');endpoint.log=io.StringIO();cancelled=[];reads=[];sends=[]
                async def read():
                    reads.append(True)
                    await asyncio.sleep(0)
                    return json.dumps({'method':'server-request','id':'server'} if stage=='refusal_send' else {'method':'notification'}).encode()+b'\n'
                async def send(message):
                    sends.append(message)
                    if stage=='initial_send' or (stage=='refusal_send' and len(sends)==2):
                        try:await asyncio.Future()
                        finally:cancelled.append(True)
                endpoint.proc=SimpleNamespace(stdout=SimpleNamespace(readline=read));endpoint.send=send
                start=time.monotonic()
                with self.assertRaises(TimeoutError):await endpoint.request('initialize',{},timeout=0.01)
                self.assertLess(time.monotonic()-start,1)
                if stage=='notifications':self.assertGreater(len(reads),1)
                else:self.assertEqual(cancelled,[True])
                if stage=='initial_send':self.assertEqual(reads,[])
                if stage=='refusal_send':self.assertEqual(len(sends),2)
                endpoint.log.close()

    async def test_protected_native_request_fixed_65_ceiling_and_default_semantics(self):
        for protected in (False,True):
            clock=[0.0];reads=[];run=SimpleNamespace(clean=lambda value:value)
            if protected:run.work_budget=900
            endpoint=cua.Mcp(run,'a');endpoint.log=io.StringIO()
            async def send(message):clock[0]+=20
            async def read():
                reads.append(True);clock[0]+=20
                return json.dumps({'id':'a-1','result':{'serverInfo':{}}} if len(reads)==3 else {'method':'notification'}).encode()+b'\n'
            endpoint.send=send;endpoint.proc=SimpleNamespace(stdout=SimpleNamespace(readline=read))
            with patch.object(cua,'time',SimpleNamespace(monotonic=lambda:clock[0])):
                if protected:
                    with self.assertRaises(TimeoutError):await endpoint.request('initialize',{},timeout=1000)
                else:self.assertEqual(await endpoint.request('initialize',{},timeout=65),{'serverInfo':{}})
            self.assertEqual(clock[0],80);endpoint.log.close()

    async def test_actual_cli_child_cancel_and_join_failure_preserve_primary(self):
        original=asyncio.create_subprocess_exec
        for failure in ('cancel','join'):
            with self.subTest(failure=failure):
                children=[];started=asyncio.Event()
                async def spawn(*args,**kwargs):
                    child=await original(sys.executable,'-c','import time;time.sleep(120)',**kwargs)
                    children.append(child);started.set()
                    if failure=='join':
                        wait=child.wait
                        async def failed_wait():
                            await wait();raise ValueError('synthetic join report')
                        child.wait=failed_wait
                    return child
                run=object.__new__(cua.Run);run.env=dict(os.environ);run.bin=Path('/synthetic');run.credentials={};run.cleanup_failures=[]
                try:
                    with patch.object(cua.asyncio,'create_subprocess_exec',side_effect=spawn):
                        task=asyncio.create_task(run.cli('connect',timeout=0.02 if failure=='join' else 10))
                        await started.wait()
                        if failure=='cancel':task.cancel()
                        with self.assertRaises(asyncio.CancelledError if failure=='cancel' else TimeoutError):await task
                    self.assertIsNotNone(children[0].returncode)
                    self.assertEqual(len(run.cleanup_failures),1 if failure=='join' else 0)
                finally:
                    for child in children:
                        if child.returncode is None:child.kill()
                        try:await child.wait()
                        except ValueError:pass

    async def test_mcp_registration_failure_joins_actual_child_and_closes_stderr_and_log(self):
        original=asyncio.create_subprocess_exec
        with tempfile.TemporaryDirectory() as directory:
            children=[];streams=[];primary=ValueError('synthetic identity')
            async def spawn(*args,**kwargs):
                streams.append(kwargs['stderr'])
                child=await original(sys.executable,'-c','import time;time.sleep(120)',**kwargs)
                children.append(child)
                def identity(pid):raise primary
                await host.held_process.register_async(child,identity,Path(directory)/'journal.json',[],timeout=3)
            endpoint=cua.Mcp(SimpleNamespace(output=Path(directory),bin=Path('/synthetic'),env=dict(os.environ)),'a')
            try:
                with patch.object(cua.asyncio,'create_subprocess_exec',side_effect=spawn):
                    with self.assertRaises(ValueError) as caught:await endpoint.start()
                self.assertIs(caught.exception,primary);self.assertIsNotNone(children[0].returncode)
                self.assertTrue(streams[0].closed)
                await endpoint.stop();self.assertTrue(endpoint.log.closed)
            finally:
                for child in children:
                    if child.returncode is None:child.kill()
                    await child.wait()
                if endpoint.log:endpoint.log.close()

    async def test_actual_endpoint_kill_and_finite_join_preserve_grace_primary_and_close_log(self):
        with tempfile.TemporaryDirectory() as directory:
            child=await asyncio.create_subprocess_exec(sys.executable,'-c','import time;time.sleep(120)',stdin=asyncio.subprocess.PIPE)
            wait=child.wait;kill=child.kill;primary=TimeoutError('grace primary');calls=[]
            async def controlled_wait():
                calls.append('wait')
                if len(calls)==1:raise primary
                await wait();raise ValueError('synthetic post-kill join report')
            child.wait=controlled_wait
            endpoint=cua.Mcp(SimpleNamespace(cleanup_budget=60,cleanup_deadline=time.monotonic()+3,cleanup_failures=[]),'a')
            endpoint.proc=child;endpoint.log=open(Path(directory)/'endpoint.log','w')
            try:
                with self.assertRaises(TimeoutError) as caught:await endpoint.stop()
                self.assertIs(caught.exception,primary);self.assertIsNotNone(child.returncode)
                self.assertEqual(calls,['wait','wait']);self.assertTrue(endpoint.log.closed)
                self.assertEqual(len(endpoint.run.cleanup_failures),1)
                self.assertEqual(endpoint.run.cleanup_failures[0]['operation'],'endpoint_cleanup')
            finally:
                if child.returncode is None:kill()
                await wait();endpoint.log.close()


class WorkflowTests(unittest.TestCase):
    def test_manual_baseline_has_native_helper_and_source_gates_before_build(self):
        workflow=(Path(__file__).resolve().parents[2]/'.github/workflows/hosted-desktop-probe.yml').read_text()
        baseline=workflow.split('  baseline:',1)[1].split('  cua:',1)[0]
        self.assertIn("inputs.mode == 'baseline' && !inputs.bootstrap_desktop && !inputs.bootstrap_footprint",baseline)
        self.assertIn('python-version: \'3.12\'',baseline);self.assertIn('Pillow==12.3.0',baseline)
        build=baseline.index('cargo build --release --locked')
        for command in ('python -m unittest test_hosted_lifecycle test_first_a -v','test_guest_footprint_observer.py','--verify-source'):self.assertLess(baseline.index(command),build)
        self.assertLess(baseline.index('--initialize'),baseline.index('actions/setup-python'))
        self.assertIn("steps.proof.outcome == 'success'",baseline)
        self.assertIn("first_a.failed_selected(raw)",baseline)
        self.assertNotIn('continue-on-error',baseline)
        self.assertNotIn('if: ${{ inputs.bootstrap_footprint }}',baseline)


if __name__=='__main__':unittest.main()
