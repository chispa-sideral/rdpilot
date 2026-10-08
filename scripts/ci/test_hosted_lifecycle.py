"""Local controls for shared lifecycle seams; no Windows/RDP capability claim."""
import argparse
import asyncio
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

import held_process
import hosted_desktop as host

sys.path.insert(0,str(host.live.E2E))
spec=importlib.util.spec_from_file_location('lifecycle_cua',host.live.E2E/'run-cua-e2e.py')
cua=importlib.util.module_from_spec(spec);spec.loader.exec_module(cua)


class RegistrationTests(unittest.TestCase):
    def test_real_sync_child_is_joined_on_any_identity_or_journal_failure(self):
        for failure in (PermissionError,ValueError,RuntimeError,KeyboardInterrupt):
            for stage in ('identity','journal'):
                with self.subTest(failure=failure,stage=stage),tempfile.TemporaryDirectory() as directory:
                    child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)'])
                    error=failure('private-marker')
                    def identity(pid):
                        if stage=='identity':raise error
                        return {'pid':pid,'created':'1','image':'private-image'}
                    class Journal:
                        def write_text(self,value):raise error
                    journal=Path(directory)/'owned.json' if stage=='identity' else Journal()
                    try:
                        with self.assertRaises(failure) as caught:
                            held_process.register_sync(child,identity,journal,[],timeout=3)
                        self.assertIs(caught.exception,error)
                        self.assertEqual(error.held_cleanup_failures,())
                        self.assertIsNotNone(child.returncode)
                    finally:
                        if child.poll() is None:child.kill()
                        child.wait(timeout=3)

    def test_parent_and_child_journals_do_not_replace_each_other(self):
        with tempfile.TemporaryDirectory() as directory:
            paths=[Path(directory)/name for name in ('wrapper.json','children.json')]
            children=[subprocess.Popen([sys.executable,'-c','pass']) for _ in paths]
            try:
                for child,path in zip(children,paths):
                    held_process.register_sync(child,lambda pid:{'pid':pid},path,[])
                self.assertEqual([json.loads(p.read_text())[0]['pid'] for p in paths],[c.pid for c in children])
            finally:
                for child in children:child.wait(timeout=3)

    def test_sync_join_is_attempted_after_kill_failure_and_primary_is_retained(self):
        actions=[];error=ValueError('private-primary')
        class Child:
            pid=1
            def kill(self):actions.append('kill');raise PermissionError('private-kill')
            def wait(self,timeout):
                actions.append('join');self.asserted_timeout=timeout
                raise subprocess.TimeoutExpired('private-command',timeout)
        child=Child()
        def identity(pid):raise error
        with self.assertRaises(ValueError) as caught:
            held_process.register_sync(child,identity,None,[],timeout=0.1)
        self.assertIs(caught.exception,error)
        self.assertEqual(actions,['kill','join'])
        self.assertGreaterEqual(child.asserted_timeout,0)
        self.assertLessEqual(child.asserted_timeout,0.1)
        self.assertEqual(error.held_cleanup_failures,('kill','join'))


class AsyncRegistrationTests(unittest.IsolatedAsyncioTestCase):
    async def test_real_async_child_is_joined_on_any_registration_failure(self):
        for failure in (PermissionError,ValueError,RuntimeError,asyncio.CancelledError):
            for stage in ('identity','journal'):
                with self.subTest(failure=failure,stage=stage):
                    child=await asyncio.create_subprocess_exec(sys.executable,'-c','import time; time.sleep(120)')
                    error=failure('private-marker')
                    def identity(pid):
                        if stage=='identity':raise error
                        return {'pid':pid}
                    class Journal:
                        def write_text(self,value):raise error
                    try:
                        with self.assertRaises(failure) as caught:
                            await held_process.register_async(child,identity,Journal(),[],timeout=3)
                        self.assertIs(caught.exception,error)
                        self.assertEqual(error.held_cleanup_failures,())
                        self.assertIsNotNone(child.returncode)
                    finally:
                        if child.returncode is None:child.kill()
                        await asyncio.wait_for(child.wait(),3)

    async def test_async_join_timeout_and_kill_failure_preserve_primary(self):
        actions=[];error=ValueError('private-primary')
        class Child:
            pid=1
            def kill(self):actions.append('kill');raise PermissionError()
            async def wait(self):actions.append('join');await asyncio.Future()
        def identity(pid):raise error
        with self.assertRaises(ValueError) as caught:
            await held_process.register_async(Child(),identity,None,[],timeout=0.01)
        self.assertIs(caught.exception,error)
        self.assertEqual(actions,['kill','join'])
        self.assertEqual(error.held_cleanup_failures,('kill','join'))

    async def test_actual_wrapper_selects_registration_primary_and_stop_secondary(self):
        for kill_failure in (False,True):
            with self.subTest(kill_failure=kill_failure),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);private=root/'private';private.mkdir()
                for target in ('a','b'):(private/(target+'.json')).write_text(json.dumps({'host':'127.0.0.1','password':'private-password'}))
                args=argparse.Namespace(proof='cua',bin_dir=str(root/'bin'),bundle=str(root/'bundle'),
                    output=str(private/'raw'),artifacts=str(root/'artifacts'),source_bridge_sha256='a'*64,
                    journal=str(private/'children.json'),credentials_a=str(private/'a.json'),
                    credentials_b=str(private/'b.json'),timeout=10)
                actions=[]
                class Child:
                    pid=42;returncode=None
                    def kill(self):
                        actions.append('kill')
                        if kill_failure:raise PermissionError('private-stop-marker')
                        self.returncode=1
                    async def wait(self):actions.append('join');self.returncode=1;return 1
                    async def communicate(self):self.returncode=0;return b'{}',b''
                async def spawn(*argv,**kwargs):return Child()
                identities=[]
                def identity(pid):
                    identities.append(pid)
                    if len(identities)==1:raise ValueError('private-registration-marker')
                    return {'pid':pid,'created':'1','image':'private-image'}
                with patch.object(tempfile,'tempdir',str(private)),patch.object(asyncio,'create_subprocess_exec',side_effect=spawn),patch.object(host.live,'os',SimpleNamespace(name='nt')),patch.object(host.live,'process_identity',side_effect=identity):
                    self.assertEqual(await host.live.execute(args),1)
                selected=json.loads((root/'artifacts/cua.json').read_text())
                self.assertEqual(actions,['kill','join'])
                self.assertEqual(selected['harness_failure'],{'operation':'daemon_start','exception_category':'other'})
                self.assertEqual(bool(selected.get('secondary_failures')),kill_failure)
                self.assertNotIn('private-',json.dumps(selected))
                self.assertEqual(selected['status'],'failed')


class RequiredProfileTests(unittest.TestCase):
    def measure(self,profiles,**kwargs):
        actions=[]
        with tempfile.TemporaryDirectory() as directory:
            journal=Path(directory)/'journal.json';journal.write_text('[]')
            users=[{'name':label,'sid':'S-1-5-21-'+str(i)} for i,label in enumerate(('a','b'))]
            def control(script,values=None,timeout=30):
                if script==host.PROFILE:
                    actions.append(('profile',values['name']))
                    return {'profile_was_present':profiles[values['name']]}
                if script==host.REMOVE_USER:actions.append(('account',values['name']))
                if script.startswith('$ok='):return True
                return False
            with patch.object(host,'powershell',side_effect=control):
                result=host.cleanup_suite(users,journal,'owned-cache',**kwargs)
            self.assertEqual(actions,[('profile','a'),('account','a'),('profile','b'),('account','b')])
            return result

    def test_legacy_all_or_none_and_single_required_profile_in_one_pass(self):
        for kwargs in ({},{'require_profiles':False},{'require_profiles':True,'required_profile_indices':set()}):
            result=self.measure({'a':True,'b':False},**kwargs)
            self.assertTrue(all(result.values()))
            self.assertFalse(any(k.endswith('_profile_was_present') for k in result))
        legacy=self.measure({'a':True,'b':False},require_profiles=True)
        self.assertFalse(legacy['user_1_profile_was_present'])
        self.assertFalse(legacy['user_1_profile_guest_removed'])
        specific=self.measure({'a':True,'b':False},required_profile_indices={0})
        self.assertTrue(all(specific.values()))
        self.assertTrue(specific['user_0_profile_was_present'])
        self.assertNotIn('user_1_profile_was_present',specific)
        absent=self.measure({'a':False,'b':False},required_profile_indices={0})
        self.assertFalse(absent['user_0_profile_was_present'])
        self.assertFalse(absent['user_0_profile_guest_removed'])

    def test_bad_required_index_refused_before_cleanup(self):
        for indices in ({True},{-1},{2},{'0'},[0,False]):
            with patch.object(host,'powershell') as native:
                with self.assertRaises(ValueError):host.cleanup_suite([{}],None,'cache',required_profile_indices=indices)
                native.assert_not_called()

    def test_legacy_required_journal_is_retained_even_with_no_users(self):
        with tempfile.TemporaryDirectory() as directory,patch.object(host,'powershell',return_value=True):
            result=host.cleanup_suite([],Path(directory)/'missing.json','cache',require_profiles=True)
            self.assertFalse(result['owned_processes_removed'])
            self.assertTrue(result['native_cache_removed'])


class RunLifecycleTests(unittest.IsolatedAsyncioTestCase):
    async def test_full_default_hook_sequence_and_fault_skip_remain(self):
        for skip in (False,True):
            run=object.__new__(cua.Run);run.args=argparse.Namespace(skip_faults=skip)
            run.summary={'unverified':[]};actions=[]
            for name in ('connect','attach','fixture','transfer','native_recovery','isolation','job_test','fault','loss'):
                async def action(*args,name=name):actions.append((name,*args))
                setattr(run,name,action)
            await run.proof_body()
            expected=[(name,target,*(['before_faults'] if name=='native_recovery' else [])) for target in ('a','b') for name in ('connect','attach','fixture','transfer','native_recovery')]
            expected += [('isolation',),('job_test',)]
            if not skip:expected += [('fault','kill'),('fault','stall'),('loss',)]
            self.assertEqual(actions,expected)
            self.assertEqual(run.summary['unverified'],['Fault injection explicitly skipped'] if skip else [])

    async def test_startup_keeps_original_command_environment_and_delay(self):
        run=object.__new__(cua.Run);run.bin=Path('/owned/bin');run.env={'PRIVATE':'retained'}
        log=object();child=object()
        with patch.object(asyncio,'create_subprocess_exec',new=AsyncMock(return_value=child)) as spawn,patch.object(asyncio,'sleep',new=AsyncMock()) as sleep:
            await run.start_daemon(log)
        spawn.assert_awaited_once_with(str(run.bin/'rdpilot-daemon'),env=run.env,stdout=log,stderr=log)
        sleep.assert_awaited_once_with(0.3)
        self.assertIs(run.daemon,child)
        self.assertEqual(run.operation,'daemon_start_wait')

    async def test_inherited_finalizer_keeps_primary_and_runs_independent_actions(self):
        for failure in ('startup','body'):
            with self.subTest(failure=failure),tempfile.TemporaryDirectory() as directory:
                root=Path(directory)
                for target in ('a','b'):(root/(target+'.json')).write_text(json.dumps({'password':'private-password'}))
                run=cua.Run(argparse.Namespace(bin_dir=str(root/'bin'),output=str(root/'raw'),bundle=None,
                    credentials_a=str(root/'a.json'),credentials_b=str(root/'b.json')))
                actions=[];primary=ValueError('private-primary')
                class Relay:
                    def __init__(self,cred):pass
                    async def start(self):actions.append('relay_start');return 1
                    async def close(self):actions.append('relay_cleanup');raise PermissionError('private-secondary')
                class Endpoint:
                    async def stop(self):actions.append('endpoint_cleanup');raise PermissionError()
                class Daemon:
                    returncode=None
                    def terminate(self):actions.append('daemon_terminate')
                    async def wait(self):actions.append('daemon_join');self.returncode=0
                async def start(log):
                    run.operation='daemon_start';run.daemon=Daemon();run.endpoints['a']=Endpoint()
                    if failure=='startup':raise primary
                async def body():run.operation='live_checks';raise primary
                async def cli(*args,**kwargs):actions.append('disconnect_cleanup');raise PermissionError()
                run.start_daemon=start;run.proof_body=body;run.cli=cli
                with patch.object(cua,'Relay',Relay):
                    with self.assertRaises(ValueError) as caught:await run.execute()
                self.assertIs(caught.exception,primary)
                self.assertEqual(run.primary_failure['exception_category'],'other')
                self.assertEqual(run.primary_failure['operation'],'daemon_start' if failure=='startup' else 'live_checks')
                self.assertEqual(actions,['relay_start','relay_start','endpoint_cleanup','disconnect_cleanup','disconnect_cleanup','daemon_terminate','daemon_join','relay_cleanup','relay_cleanup'])
                self.assertEqual([f['operation'] for f in run.cleanup_failures],['endpoint_cleanup','disconnect_cleanup','disconnect_cleanup','relay_cleanup','relay_cleanup'])
                self.assertFalse(run.temp.exists())
                self.assertEqual(json.loads((run.output/'summary.json').read_text())['status'],'failed')


if __name__=='__main__':unittest.main()
