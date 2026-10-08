"""Actual wrapper main distinguishes input failure from artifact emission failure."""
import asyncio
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import test_live_proof as existing

live=existing.live
SECRET='private-lease-token'
PASSWORD='secretcredential'


class MainFallbackStageTests(unittest.TestCase):
    def arguments(self,root):
        private=root/'private';private.mkdir()
        return private,[str(Path(live.__file__)),'cua','--bin-dir',str(root/'bin'),'--bundle',str(root/'bundle'),
            '--output',str(private/'raw'),'--artifacts',str(root/'artifacts'),'--source-bridge-sha256',existing.HASH,
            '--journal',str(private/'processes.json'),'--credentials-a',str(private/'missing-a.json'),
            '--credentials-b',str(private/'missing-b.json')]

    def assert_private(self,artifact):
        self.assertNotIn(SECRET,json.dumps(artifact))
        self.assertNotIn(PASSWORD,json.dumps(artifact))
        self.assertEqual(artifact['status'],'failed')
        self.assertNotIn('a.rdp_bundle_ready',artifact['completed_checks'])

    def test_actual_main_missing_constructor_input_has_no_artifact_write_failure(self):
        with tempfile.TemporaryDirectory(prefix=SECRET) as directory:
            root=Path(directory);private,argv=self.arguments(root)
            with patch.object(sys,'argv',argv),patch.object(tempfile,'tempdir',str(private)),patch.object(
                asyncio,'create_subprocess_exec',side_effect=AssertionError('No daemon or child may start')) as spawn:
                self.assertEqual(live.main(),1)
                spawn.assert_not_called()
            artifact=json.loads((root/'artifacts/cua.json').read_text())
            self.assert_private(artifact)
            self.assertEqual(artifact['harness_failure'],{'operation':'harness','exception_category':'file_not_found'})
            self.assertNotIn('secondary_failures',artifact)
            self.assertEqual([p.name for p in (root/'artifacts').iterdir()],['cua.json'])

    def test_actual_main_late_write_failure_preserves_primary_and_genuine_cleanup(self):
        with tempfile.TemporaryDirectory(prefix=SECRET) as directory:
            root=Path(directory);private,argv=self.arguments(root)
            for name in ('missing-a.json','missing-b.json'):
                (private/name).write_text(json.dumps({'host':'127.0.0.1','username':'standard','password':PASSWORD}))
            message=('Internal: bridge bootstrap failed: rdpilot-bridge did not start '
                     '(possible security prompt in the guest, or a bridge that does not speak bridge protocol 2); '
                     'stages=rdpdr_file_access,launch_input_sent')
            payload=json.dumps({'error':{'code':'internal','message':message},'private':SECRET}).encode()
            actions=[]
            class Child:
                pid=42
                def __init__(self,daemon=False):self.returncode=None if daemon else 1
                async def communicate(self):return payload,(SECRET+' '+PASSWORD).encode()
                async def wait(self):return self.returncode
                def kill(self):self.returncode=1
                def terminate(self):actions.append('daemon_cleanup');raise PermissionError(SECRET)
            async def spawn(*argv,**kwargs):
                if len(argv)>1 and argv[1]=='connect':(private/'raw/protected.png').write_bytes(SECRET.encode())
                return Child(Path(argv[0]).name=='rdpilot-daemon')
            original_write=Path.write_text
            def write(path,*args,**kwargs):
                if path==root/'artifacts/cua.json':
                    actions.append('artifact_write')
                    if actions.count('artifact_write')==1:raise PermissionError(SECRET)
                return original_write(path,*args,**kwargs)
            with patch.object(sys,'argv',argv),patch.object(tempfile,'tempdir',str(private)),patch.object(
                asyncio,'create_subprocess_exec',side_effect=spawn),patch.object(
                live,'process_identity',return_value={'pid':42,'created':'1','image':'fixture'}),patch.object(
                Path,'write_text',autospec=True,side_effect=write):
                self.assertEqual(live.main(),1)
            artifact=json.loads((root/'artifacts/cua.json').read_text());self.assert_private(artifact)
            primary=artifact['harness_failure']
            self.assertEqual(primary['operation'],'connect_cli');self.assertEqual(primary['cli_exit_code'],1)
            observed=primary['connect_observation']
            self.assertEqual(observed['cli_error_code'],'internal');self.assertEqual(observed['producer'],'sdk_bootstrap')
            self.assertEqual(observed['bootstrap_stages'],['rdpdr_file_access','launch_input_sent'])
            self.assertEqual(observed['daemon'],{'state':'alive'})
            self.assertEqual(artifact['secondary_failures'],[
                {'operation':'daemon_cleanup','exception_category':'access_denied'},
                {'operation':'artifact_write','exception_category':'access_denied'}])
            self.assertEqual(actions,['daemon_cleanup','artifact_write','artifact_write'])
            self.assertFalse(list((root/'artifacts').glob('*.png')))
            self.assertFalse(any(path.name.startswith('rdpilot-cua-') for path in private.iterdir()))


if __name__=='__main__':unittest.main()
