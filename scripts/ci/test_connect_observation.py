"""Source-envelope, real relay and native assembly observer controls."""
import asyncio
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import acquisition_observer as acquisition
import test_live_proof as existing

sys.path.insert(0,str(existing.live.E2E))
import proof_support as support
spec=importlib.util.spec_from_file_location('observed_cua',existing.live.E2E/'run-cua-e2e.py')
cua=importlib.util.module_from_spec(spec);spec.loader.exec_module(cua)
SECRET='private-lease-token-secretcredential'


def envelope(code,message):
    return json.dumps({'error':{'code':code,'message':message},'private':SECRET}).encode()


class EnvelopeTests(unittest.TestCase):
    def test_codes_types_budgets_and_anchored_producers(self):
        for code in support.CLI_CODES:
            self.assertEqual(support.cli_observation(envelope(code,SECRET))['cli_error_code'],code)
        malformed=(None,b'',b'[]',b'{',b'{"error":true}',b'{"error":{"code":"internal"}}',
                   envelope('x',SECRET),envelope('internal',SECRET*(support.MESSAGE_BUDGET//len(SECRET)+1)),
                   b' '* (support.JSON_BUDGET+1),b'{"error":{},"error":{"code":"internal","message":"x"}}')
        for payload in malformed:self.assertEqual(support.cli_observation(payload)['cli_error_code'],'unavailable')
        for message in ('bridge bootstrap failed: '+SECRET,' Internal: bridge bootstrap failed: '+SECRET,
                        'Internal: '+SECRET+' bridge bootstrap failed: x','Internal: bridge bootstrap failedX: x',
                        'configuration error: PasswordCommand for a failed: empty output'):
            self.assertEqual(support.cli_observation(envelope('internal',message))['producer'],'unavailable')
        self.assertEqual(support.cli_observation(envelope('config','Internal: bridge bootstrap failed: x'))['producer'],'unavailable')

    def test_controlled_password_producer_source_forms_only(self):
        fixtures={'empty output':'empty_output','output is not valid UTF-8':'invalid_utf8',
                  'timed out after 30 s':'timeout','could not start the shell: '+SECRET:'shell_start',
                  'could not read its output: '+SECRET:'output_read','could not wait for it: '+SECRET:'wait',
                  'no stdout':'no_stdout','exit code: 5':'nonzero_exit','exit status: 6':'nonzero_exit'}
        for target in ('a','b'):
            for reason,category in fixtures.items():
                found=support.cli_observation(envelope('config',f'configuration error: PasswordCommand for {target} failed: {reason}'))
                self.assertEqual(found['producer'],'password_command');self.assertEqual(found['password_helper'],category)
                self.assertNotIn(SECRET,json.dumps(found))
        for reason in ('empty output '+SECRET,'timed out after 31 s','exit code: -1','exit code: 4294967296','exit status: True'):
            self.assertEqual(support.cli_observation(envelope('config','configuration error: PasswordCommand for a failed: '+reason))['password_helper'],'unavailable')
        self.assertEqual(support.cli_observation(envelope('config','configuration error: PasswordCommand for private failed: empty output'))['producer'],'config_other')

    def test_source_timeout_stages_and_connection_substages(self):
        prefix='Internal: bridge bootstrap failed: '+support.BOOTSTRAP_TIMEOUT
        for suffix,expected in [('none',[]),(','.join(sorted(support.BOOTSTRAP_STAGES)),sorted(support.BOOTSTRAP_STAGES))]:
            found=support.cli_observation(envelope('internal',prefix+suffix))
            self.assertEqual(found['bootstrap_state'],'observed');self.assertEqual(found['bootstrap_stages'],expected)
        for suffix in ('','none,','none,'+SECRET,'ping_sent,ping_sent','unknown','ping_sent,',SECRET):
            self.assertEqual(support.cli_observation(envelope('internal',prefix+suffix))['bootstrap_state'],'unavailable')
        for message in (prefix.replace('protocol 2','protocol 1')+'none','Internal: bridge bootstrap failed: input failed'):
            self.assertEqual(support.cli_observation(envelope('internal',message))['bootstrap_state'],'unavailable')
        for prefix,stage in [('connect_begin failed: ','negotiation'),('connect_finalize failed: ','finalize')]:
            self.assertEqual(support.cli_observation(envelope('internal','Internal: connection or authentication failed: '+prefix+SECRET))['connection_stage'],stage)

    def test_projected_snapshot_rejects_untrusted_metadata(self):
        for value in (True,-1,2**63,SECRET,{},None):
            raw={'state':'observed','accepted':value,'upstream_connected':0,'to_upstream_bytes':0,'to_client_bytes':0}
            self.assertEqual(support.select_relay_observation(raw),{'state':'unavailable'})
        for stages in ([SECRET],['ping_sent']*10,[True],['ping_sent','ping_sent']):
            result=support.select_connect_observation({'bootstrap_state':'observed','bootstrap_stages':stages,'private':SECRET})
            self.assertEqual(result['bootstrap_state'],'unavailable');self.assertNotIn(SECRET,json.dumps(result))
        raw={'bootstrap_state':'observed','bootstrap_stages':['ping_sent'],'relays':{'a':{'state':'observed','accepted':1,
            'upstream_connected':1,'to_upstream_bytes':2,'to_client_bytes':3,'errno':True,'winerror':2**32}},'private':SECRET}
        frozen=support.select_connect_observation(raw);raw['bootstrap_stages'].append(SECRET)
        self.assertEqual(frozen['bootstrap_stages'],['ping_sent']);self.assertNotIn('errno',frozen['relays']['a'])
        self.assertNotIn('winerror',frozen['relays']['a']);self.assertEqual(frozen['relays']['b'],{'state':'unavailable'})


class RealRelayTests(unittest.IsolatedAsyncioTestCase):
    async def wait_until(self,condition):
        async with asyncio.timeout(2):
            while not condition():await asyncio.sleep(0.01)

    async def test_real_forwarding_eof_drop_and_snapshot(self):
        finished=asyncio.Event()
        async def echo(reader,writer):
            try:
                while data:=await reader.read(65536):writer.write(data);await writer.drain()
            finally:writer.close();await writer.wait_closed();finished.set()
        server=await asyncio.start_server(echo,'127.0.0.1',0)
        relay=cua.Relay({'host':'127.0.0.1','port':server.sockets[0].getsockname()[1]})
        try:
            port=await relay.start();reader,writer=await asyncio.open_connection('127.0.0.1',port)
            payload=b'diagnostic bytes'*1000;writer.write(payload);await writer.drain()
            self.assertEqual(await reader.readexactly(len(payload)),payload)
            await self.wait_until(lambda:relay.observation['to_client_bytes']==len(payload))
            observed=support.select_relay_observation(relay.observation)
            self.assertEqual([observed[k] for k in ('accepted','upstream_connected','to_upstream_bytes','to_client_bytes')],[1,1,len(payload),len(payload)])
            await relay.drop();self.assertEqual(await reader.read(),b'');writer.close();await writer.wait_closed()
            await self.wait_until(lambda:not relay.streams);await finished.wait()
            self.assertEqual(observed['to_upstream_bytes'],len(payload))
        finally:await relay.close();server.close();await server.wait_closed()

    async def test_real_refused_upstream_and_eof(self):
        closed=await asyncio.start_server(lambda r,w:w.close(),'127.0.0.1',0)
        port=closed.sockets[0].getsockname()[1];closed.close();await closed.wait_closed()
        relay=cua.Relay({'host':'127.0.0.1','port':port})
        try:
            local=await relay.start();reader,writer=await asyncio.open_connection('127.0.0.1',local)
            self.assertEqual(await asyncio.wait_for(reader.read(),2),b'')
            found=support.select_relay_observation(relay.observation)
            self.assertEqual(found['accepted'],1);self.assertEqual(found['upstream_connected'],0)
            self.assertEqual(found['upstream_failure'],'os_error');self.assertEqual(found['to_upstream_bytes'],0)
            writer.close();await writer.wait_closed()
        finally:await relay.close()

    async def test_actual_copier_error_closes_streams_without_counting_failed_drain(self):
        connected=asyncio.Event()
        async def hold(reader,writer):
            connected.set()
            try:await reader.read()
            finally:writer.close();await writer.wait_closed()
        upstream=await asyncio.start_server(hold,'127.0.0.1',0)
        relay=cua.Relay({'host':'127.0.0.1','port':upstream.sockets[0].getsockname()[1]})
        original=asyncio.open_connection
        class FailedDrain:
            def __init__(self,writer):self.writer=writer
            def write(self,data):self.writer.write(data)
            async def drain(self):raise OSError('private-lease-token')
            def close(self):self.writer.close()
        async def failing(*args,**kwargs):
            reader,writer=await original(*args,**kwargs)
            return reader,FailedDrain(writer)
        try:
            port=await relay.start()
            with patch.object(asyncio,'open_connection',side_effect=failing):
                reader,writer=await original('127.0.0.1',port)
                writer.write(b'control');await writer.drain();await connected.wait()
                await self.wait_until(lambda:relay.observation['copy_failure']=='os_error')
                self.assertEqual(await asyncio.wait_for(reader.read(),2),b'')
            self.assertEqual(relay.observation['to_upstream_bytes'],0)
            writer.close();await writer.wait_closed();await self.wait_until(lambda:not relay.streams)
        finally:await relay.close();upstream.close();await upstream.wait_closed()


def assembly(cache,bridge=b'source bridge'):
    archive=io.BytesIO()
    contents={'cua-driver.exe':b'MZ pinned helper','sdk.dll':b'sdk'}
    with zipfile.ZipFile(archive,'w',zipfile.ZIP_DEFLATED) as zipped:
        for name,data in contents.items():zipped.writestr(name,data)
    zipped=archive.getvalue();bridge_hash=hashlib.sha256(bridge).hexdigest();archive_hash=hashlib.sha256(zipped).hexdigest()
    identity='cua-driver-rs-v0.34.0-'+hashlib.sha256((bridge_hash+archive_hash).encode()).hexdigest()[:16]
    path=cache/'bundles'/identity;path.mkdir(parents=True)
    manifest={'bundle_id':identity,'cua_version':'0.34.0','archive_name':acquisition.ARCHIVE,
              'archive_sha256':archive_hash,'bridge_sha256':bridge_hash,
              'files':{name:hashlib.sha256(data).hexdigest() for name,data in contents.items()}}
    (path/'rdpilot-bridge.exe').write_bytes(bridge);(path/acquisition.ARCHIVE).write_bytes(zipped)
    (path/'manifest.json').write_text(json.dumps(manifest))
    return path,manifest,bridge_hash


class AcquisitionTests(unittest.TestCase):
    def test_valid_native_representation_and_absent_cache_are_distinct(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'cache';expected='a'*64
            self.assertEqual(acquisition.observe_cache(root,expected),{'state':'absent','bundle_count':0})
            path,manifest,expected=assembly(root)
            before={p.name:p.read_bytes() for p in path.iterdir()}
            found=acquisition.observe_cache(root,expected)
            self.assertEqual(found['state'],'verified_source_bundle');self.assertEqual(found['cua_sha256'],manifest['files']['cua-driver.exe'])
            self.assertEqual(found['bundle_id'],manifest['bundle_id']);self.assertFalse((path/'cua-driver.exe').exists())
            self.assertEqual(before,{p.name:p.read_bytes() for p in path.iterdir()})

    def test_mismatch_malformed_ambiguity_and_budget_never_claim_verified(self):
        for damage in ('bridge','archive','manifest','version','id','table','source','duplicate','extra','symlink','budget','table_case'):
            with self.subTest(damage=damage),tempfile.TemporaryDirectory() as directory:
                cache=Path(directory)/'cache';path,manifest,expected=assembly(cache)
                if damage=='bridge':(path/'rdpilot-bridge.exe').write_bytes(b'other')
                elif damage=='archive':(path/acquisition.ARCHIVE).write_bytes(b'not a zip')
                elif damage=='manifest':(path/'manifest.json').write_text(SECRET)
                elif damage=='version':manifest['cua_version']='0.35.0'
                elif damage=='id':manifest['bundle_id']='cua-driver-rs-v0.34.0-'+'a'*16
                elif damage=='table':manifest['files']['cua-driver.exe']='a'*64
                elif damage=='source':expected='a'*64
                elif damage=='duplicate':(cache/'bundles'/'other').mkdir()
                elif damage=='extra':manifest['private']=SECRET
                elif damage=='symlink':(path/'rdpilot-bridge.exe').unlink();(path/'rdpilot-bridge.exe').symlink_to(Path(directory)/'unowned')
                elif damage=='table_case':manifest['files']['CUA-DRIVER.EXE']=manifest['files']['cua-driver.exe']
                if damage in ('version','id','table','extra','table_case'):(path/'manifest.json').write_text(json.dumps(manifest))
                if damage=='budget':
                    with patch.object(acquisition,'MAX_MANIFEST_BYTES',1):found=acquisition.observe_cache(cache,expected)
                    self.assertEqual(found['state'],'observation_failed')
                else:found=acquisition.observe_cache(cache,expected)
                self.assertNotEqual(found['state'],'verified_source_bundle');self.assertNotIn(SECRET,json.dumps(found))
                self.assertTrue(path.exists(),'observer never removes invalid assembly')

    def test_central_admission_limits_precede_zip_metadata_parsing(self):
        for damage in ('central_budget','entry_budget','zip64'):
            with self.subTest(damage=damage),tempfile.TemporaryDirectory() as directory:
                cache=Path(directory)/'cache';path,manifest,expected=assembly(cache)
                if damage in ('zip64','entry_budget'):
                    data=bytearray((path/acquisition.ARCHIVE).read_bytes())
                    offset=data.rfind(b'PK\x05\x06')
                    if damage=='zip64':struct.pack_into('<H',data,offset+10,65535)
                    else:struct.pack_into('<2H',data,offset+8,5000,5000)
                    archive_hash=hashlib.sha256(data).hexdigest();manifest['archive_sha256']=archive_hash
                    identity='cua-driver-rs-v0.34.0-'+hashlib.sha256((expected+archive_hash).encode()).hexdigest()[:16]
                    manifest['bundle_id']=identity
                    (path/acquisition.ARCHIVE).write_bytes(data);(path/'manifest.json').write_text(json.dumps(manifest))
                    renamed=path.with_name(identity);path.rename(renamed)
                with patch.object(acquisition.zipfile,'ZipFile',side_effect=AssertionError('metadata parser must not run')):
                    if damage=='central_budget':
                        with patch.object(acquisition,'MAX_CENTRAL_BYTES',1):found=acquisition.observe_cache(cache,expected)
                    else:found=acquisition.observe_cache(cache,expected)
                self.assertEqual(found,{'state':'observation_failed'})

    def test_whole_observation_hash_directory_and_expansion_budgets(self):
        for limit in ('MAX_BRIDGE_BYTES','MAX_ARCHIVE_BYTES','MAX_EXPANDED_BYTES'):
            with self.subTest(limit=limit),tempfile.TemporaryDirectory() as directory:
                cache=Path(directory)/'cache';path,manifest,expected=assembly(cache)
                with patch.object(acquisition,limit,1):found=acquisition.observe_cache(cache,expected)
                self.assertEqual(found,{'state':'observation_failed'});self.assertTrue(path.exists())
        with tempfile.TemporaryDirectory() as directory:
            cache=Path(directory)/'cache';path,manifest,expected=assembly(cache)
            for index in range(acquisition.MAX_DIRECTORIES):(cache/'bundles'/str(index)).mkdir()
            self.assertEqual(acquisition.observe_cache(cache,expected),{'state':'observation_failed'})

    def test_symlink_root_parent_reparse_and_read_error_are_safe(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);cache=root/'cache';path,manifest,expected=assembly(cache)
            link=root/'link';link.symlink_to(cache,target_is_directory=True)
            self.assertEqual(acquisition.observe_cache(link,expected)['state'],'invalid')
            parent=root/'parent';parent.symlink_to(root,target_is_directory=True)
            self.assertEqual(acquisition.observe_cache(parent/'cache',expected)['state'],'invalid')
            with patch.object(acquisition,'open_plain',side_effect=PermissionError(SECRET)):
                self.assertEqual(acquisition.observe_cache(cache,expected),{'state':'observation_failed'})
            # Exercise the Windows attribute check without a Windows filesystem.
            observed=path.stat()
            class Reparse:
                st_file_attributes=0x400
                st_mode=observed.st_mode
            with patch.object(Path,'lstat',return_value=Reparse()):
                self.assertEqual(acquisition.observe_cache(cache,expected)['state'],'invalid')


class ActualOrchestratorObservationTests(unittest.TestCase):
    def test_failed_cua_observes_owned_cache_before_all_cleanup(self):
        controller=existing.OrchestrationModeTests()
        for failure in ('harness','observer','observer_write','artifact_missing','artifact_invalid'):
            with self.subTest(failure=failure):
                code,gate,cleanup,commands,actions=controller.measure('cua_diagnostic',failure=failure,cache_fixture=assembly)
                self.assertEqual(code,1);self.assertEqual(gate['completed_suites'],[])
                labels=[label for label,_ in actions]
                self.assertLess(labels.index('observe_cache'),labels.index(existing.host.PROFILE))
                self.assertLess(labels.index('acquisition_write'),labels.index(existing.host.PROFILE))
                if failure!='observer':self.assertIn(('observed_state','verified_source_bundle'),actions)
                self.assertTrue(all(cleanup['cua'].values()));self.assertTrue(all(cleanup['host'].values()))
                self.assertTrue(cleanup['private_removed'])
                self.assertNotIn(SECRET,json.dumps(gate))
                expected=[] if failure=='harness' else ['selected_artifact_read' if failure.startswith('artifact_') else 'acquisition_observation' if failure=='observer' else 'acquisition_artifact_write']
                self.assertEqual(gate['observation_failures'],expected)


if __name__=='__main__':unittest.main()
