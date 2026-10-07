"""Failure injection against the real required live gate and projection."""
import importlib.util
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
