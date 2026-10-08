"""Real owned HTTP/process/PNG controls; no live RDP or Windows claim."""
import argparse
import asyncio
import base64
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from PIL import Image, __version__ as pillow_version
import bootstrap_desktop_observer as desktop
import test_live_proof as baseline

live = baseline.live
sys.path.insert(0, str(live.E2E))
import proof_support
SOURCE = {'source_commit': '1'*40, 'source_tree': '2'*40, 'cli_sha256': 'a'*64,
          'daemon_sha256': 'b'*64, 'bridge_sha256': baseline.HASH}


def png():
    stream = io.BytesIO(); Image.new('RGB', (16, 16), (7, 9, 11)).save(stream, 'PNG')
    return stream.getvalue()


def manifest(root):
    body = png(); (root/'bootstrap-a-01.png').write_bytes(body)
    result = desktop.empty_manifest(SOURCE)
    result.update(state='observed', os_session_id=7, requests=1, elapsed_ms=100,
                  cleanup={key: True for key in desktop.CLEANUP_KEYS})
    result['frames'] = [{'name': 'bootstrap-a-01.png', 'seq': 1, 'width': 16, 'height': 16,
        'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest(), 'elapsed_ms': 99}]
    return result


class ImageAdmissionTests(unittest.TestCase):
    def test_pinned_full_decoder_and_preallocation_controls(self):
        self.assertEqual(pillow_version, '12.3.0')
        body = png(); self.assertEqual(desktop.validate_png(body, 16, 16), (16, 16))
        for altered, width, height in ((body, True, 16), (body, 17, 16), (body[:20], 16, 16),
                                      (body[:-12], 16, 16), (body+b'private-trailer', 16, 16), (body[:29]+b'xxxx'+body[33:], 16, 16)):
            with self.subTest(width=width), self.assertRaises(Exception):
                desktop.validate_png(altered, width, height)
        huge = body[:16]+struct.pack('>II', desktop.PIXELS, 2)+body[24:]
        with patch.object(Image, 'open', side_effect=AssertionError('No allocation')):
            with self.assertRaises(ValueError):desktop.validate_png(huge, desktop.PIXELS, 2)
            with patch.object(desktop, 'PNG_BYTES', 10):
                with self.assertRaises(ValueError):desktop.validate_png(body, 16, 16)

    def test_both_projection_layers_refuse_malformed_source_frame_scope_and_scan(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); good = manifest(root)
            self.assertEqual(desktop.selected_files(root, good, SOURCE), ['bootstrap-a-01.png'])
            for field, value in (('state', 'unavailable'), ('state', 'observation_failed'), ('target', 'b'),
                                 ('requests', True), ('requests', 41), ('elapsed_ms', 300001),
                                 ('os_session_id', 0), ('events', ['private-token'])):
                bad = copy.deepcopy(good); bad[field] = value
                self.assertEqual(desktop.selected_files(root, bad, SOURCE), [])
                self.assertNotIn('private-token', json.dumps(desktop.safe_manifest(root, bad, SOURCE)))
            for field, value in (('seq', False), ('seq', 0), ('width', 'secret'), ('height', None),
                                 ('elapsed_ms', {}), ('sha256', '0'*64), ('bytes', 2), ('name', '../secret.png')):
                bad = copy.deepcopy(good); bad['frames'][0][field] = value
                self.assertEqual(desktop.selected_files(root, bad, SOURCE), [])
            bad = copy.deepcopy(good); bad['source']['cli_sha256'] = 'c'*64
            self.assertEqual(desktop.selected_files(root, bad, SOURCE), [])
            bad = copy.deepcopy(good); bad['cleanup']['children_joined'] = False
            self.assertEqual(desktop.selected_files(root, bad, SOURCE), [])
            self.assertEqual(desktop.safe_manifest(root, good, SOURCE, False)['frames'], [])
            for enabled in (False, True):
                (root/'bootstrap-desktop.json').write_text(json.dumps(good))
                (root/'ordinary.png').write_bytes(b'secret')
                if not (root/'bootstrap-a-01.png').exists():manifest(root)
                names = desktop.retain_failed_images(root, SOURCE, enabled)
                self.assertEqual(names, ['bootstrap-a-01.png'] if enabled else [])
                self.assertFalse((root/'ordinary.png').exists())
            (root/'bootstrap-desktop.json').write_text('{private-token')
            desktop.retain_failed_images(root, SOURCE, True)
            self.assertFalse(list(root.glob('*.png')))
            self.assertNotIn('private-token', (root/'bootstrap-desktop.json').read_text())

    def test_reparse_refused_at_actual_bounded_open(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); good = manifest(root)
            original = desktop.files.plain
            def plain(path, directory=False):
                if path.name == 'bootstrap-a-01.png':raise ValueError('Reparse')
                return original(path, directory)
            with patch.object(desktop.files, 'plain', side_effect=plain):
                self.assertEqual(desktop.selected_files(root, good, SOURCE), [])

    def test_wrong_mode_rejected_before_host_resources(self):
        for setup, cua in ((False, False), (True, False), (True, True)):
            with self.assertRaises(ValueError):
                baseline.host.mode_for(SimpleNamespace(setup_diagnostic=setup, cua_diagnostic=cua, bootstrap_desktop=True))


# Test-only source-built process substitution. PNG/server and all workers are real
# owned host processes; these controls do not impersonate Windows RDP proof.
VIEWER = '''import asyncio, base64, json, signal
BODY=base64.b64decode(%r)
async def main():
 stop=asyncio.Event(); loop=asyncio.get_running_loop(); seq=0
 signal.signal(signal.SIGINT, lambda *_: loop.call_soon_threadsafe(stop.set))
 async def handle(reader, writer):
  nonlocal seq
  try:
   await reader.readuntil(b'\\r\\n\\r\\n'); seq+=1
   header=f'HTTP/1.1 200 OK\\r\\nContent-Type: image/png\\r\\nContent-Length: {len(BODY)}\\r\\nx-frame-seq: {seq}\\r\\nx-frame-width: 16\\r\\nx-frame-height: 16\\r\\n\\r\\n'
   writer.write(header.encode()+BODY); await writer.drain()
  except Exception: pass
  finally:
   writer.close(); await writer.wait_closed()
 server=await asyncio.start_server(handle,'127.0.0.1',0)
 port=server.sockets[0].getsockname()[1]
 print(json.dumps({'urls':[f'http://127.0.0.1:{port}/?token='+ 'd'*64],'notices':[],'read_only':True}),flush=True)
 await stop.wait(); server.close(); await server.wait_closed()
asyncio.run(main())
'''


class ProcessHttpTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.directory = tempfile.TemporaryDirectory(); self.root = Path(self.directory.name)
        self.temp = self.root/'temp'; self.temp.mkdir(); self.output = self.root/'raw'; self.output.mkdir()
        self.bin = self.root/'bin'; self.bin.mkdir()
        self.source = dict(SOURCE)
        for name, key in (('rdpilot', 'cli_sha256'), ('rdpilot-daemon', 'daemon_sha256')):
            body = name.encode(); (self.bin/(name+'.exe' if os.name == 'nt' else name)).write_bytes(body)
            self.source[key] = hashlib.sha256(body).hexdigest()
        self.server_file = self.root/'viewer.py'; self.server_file.write_text(VIEWER % base64.b64encode(png()).decode())
        self.raw_spawn = asyncio.create_subprocess_exec
        self.identities = {42: {'pid': 42, 'created': '1', 'image': str(self.bin/'rdpilot-daemon.exe')}}
        self.created = []; self.decoder_started = asyncio.Event()
        self.decoder_delay = False; self.helper_delay = False
        self.config = {'source': self.source, 'sid': 'S-1-5-21-123', 'wts_script': 'fixed test script'}
        self.run = SimpleNamespace(temp=self.temp, output=self.output, bin=self.bin,
            env=dict(os.environ), daemon=SimpleNamespace(pid=42, returncode=None))
        self.observer = desktop.Observer(self.run, self.config, self.spawn, lambda pid: self.identities[pid], lambda record: None, proof_support)

    async def asyncTearDown(self):
        for proc in self.created:
            if proc.returncode is None:proc.kill()
            await proc.wait()
        self.directory.cleanup()

    async def spawn(self, *argv, **kwargs):
        product_image = None
        if argv[0] == str(self.bin/'rdpilot'):
            product_image = str(self.bin/'rdpilot.exe')
            argv = (sys.executable, str(self.server_file))
        elif argv[0] == 'powershell.exe':
            argv = (sys.executable, '-c', 'print(\'{"matches":[{"id":7,"state":0}]}\')')
        elif '--decode' in argv and self.decoder_delay:
            argv = (sys.executable, '-c', 'import time; time.sleep(3600)')
        elif '--interrupt-console' in argv and self.helper_delay:
            argv = (sys.executable, '-c', 'import time; time.sleep(3600)')
        proc = await self.raw_spawn(*argv, **kwargs); self.created.append(proc)
        self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': product_image or sys.executable}
        if '--decode' in argv or self.decoder_delay and argv[-1] == 'import time; time.sleep(3600)':
            self.decoder_started.set()
        return proc

    async def ready(self):
        await self.observer.start_viewer()
        self.observer.started = time.monotonic()

    async def test_real_worker_http_png_binding_and_complete_shutdown(self):
        await self.ready(); await self.observer.capture()
        raw = self.observer.manifest
        self.assertEqual(raw['state'], 'observed'); self.assertEqual(raw['os_session_id'], 7)
        self.assertEqual(await self.observer.stop(), True)
        self.assertTrue(all(p.returncode is not None for p in self.created))
        self.assertEqual(desktop.selected_files(self.output, raw, self.source), ['bootstrap-a-01.png'])
        self.assertFalse(self.observer.writers)
        self.assertTrue(all(raw['cleanup'].values()))
        # Parent closes epoch permanently, including late direct admission.
        self.observer.admit(2, 16, 16, png())
        self.assertEqual(len(raw['frames']), 1)

    async def test_actual_decode_is_killed_and_joined_when_connect_outcome_arrives(self):
        await self.ready(); self.decoder_delay = True
        self.observer.task = asyncio.create_task(self.observer.capture())
        await asyncio.wait_for(self.decoder_started.wait(), 2)
        started = time.monotonic()
        self.assertTrue(await self.observer.stop())
        self.assertLess(time.monotonic()-started, 2)
        self.assertTrue(self.observer.task.done())
        self.assertFalse(list(self.output.glob('*.png')))
        self.assertTrue(all(p.returncode is not None for p in self.created))

    async def test_registry_precedes_journal_and_cancellation_during_spawn_joins_child(self):
        seen = []
        self.observer.identity = lambda pid: {'pid': pid, 'created': '1', 'image': 'test-owned'}
        def journal(record):
            seen.append(record['pid'])
            self.assertTrue(any(p.pid == record['pid'] for p in self.observer.children))
            raise PermissionError('private-marker')
        self.observer.journal = journal
        with self.assertRaises(PermissionError):
            await self.observer.spawn(sys.executable, '-c', 'import time; time.sleep(3600)')
        self.assertTrue(seen); self.assertTrue(all(p.returncode is not None for p in self.created))
        self.observer.journal = lambda record: None
        original = self.observer.raw_spawn
        async def delayed(*argv, **kwargs):
            await asyncio.sleep(.05); return await original(*argv, **kwargs)
        self.observer.raw_spawn = delayed
        task = asyncio.create_task(self.observer.spawn(sys.executable, '-c', 'import time; time.sleep(3600)'))
        await asyncio.sleep(.01); task.cancel()
        with self.assertRaises(asyncio.CancelledError):await task
        self.assertTrue(all(p.returncode is not None for p in self.created))

    async def test_helper_timeout_and_first_cleanup_failure_join_all_owned_children(self):
        await self.ready(); self.observer.windows = True; self.helper_delay = True
        self.assertFalse(await self.observer.stop())
        self.assertTrue(all(p.returncode is not None for p in self.created))
        self.assertFalse(self.observer.manifest['cleanup']['viewer_ctrl_c'])
        self.assertFalse(list(self.output.glob('*.png')))

    async def test_binding_change_discards_prior_admitted_images(self):
        await self.ready(); await self.observer.capture()
        self.assertTrue(self.observer.manifest['frames'])
        async def changed(*argv, **kwargs):
            if argv[0] == 'powershell.exe':
                proc = await self.raw_spawn(sys.executable, '-c', 'print(\'{"matches":[{"id":9,"state":0}]}\')', **kwargs)
                self.created.append(proc)
                self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': sys.executable}
                return proc
            return await self.spawn(*argv, **kwargs)
        self.observer.raw_spawn = changed
        await self.observer.capture()
        self.assertTrue(self.observer.invalidated)
        self.assertFalse(list(self.output.glob('*.png')))
        await self.observer.stop()

    async def test_slow_http_whole_deadline_and_socket_close_on_cancellation(self):
        ended = asyncio.Event(); accepted = asyncio.Event(); tasks = set()
        async def peer(reader, writer):
            task = asyncio.current_task(); tasks.add(task)
            try:
                await reader.readuntil(b'\r\n\r\n'); accepted.set()
                # No header: cancellation must close the actual peer socket.
                self.assertEqual(await reader.read(), b'')
            finally:
                writer.close(); await writer.wait_closed(); ended.set(); tasks.discard(task)
        server = await asyncio.start_server(peer, '127.0.0.1', 0)
        self.observer.port = server.sockets[0].getsockname()[1]; self.observer.bearer = 'e'*64
        try:
            request = asyncio.create_task(self.observer.http_frame())
            await accepted.wait(); request.cancel()
            with self.assertRaises(asyncio.CancelledError):await request
            await asyncio.wait_for(ended.wait(), 1)
            self.assertFalse(self.observer.writers)
            accepted.clear(); ended.clear()
            with self.assertRaises(asyncio.TimeoutError):await self.observer.http_frame()
            await asyncio.wait_for(ended.wait(), 1)
        finally:
            server.close(); await server.wait_closed()
            await asyncio.gather(*tasks, return_exceptions=True)

    async def test_source_child_and_startup_scope_are_fail_closed(self):
        self.observer.config['source']['cli_sha256'] = '0'*64
        with self.assertRaises(ValueError):await self.observer.start_viewer()
        self.assertTrue(await self.observer.stop())
        self.assertFalse(self.observer.manifest['frames'])

    async def test_pending_wts_child_and_lifetime_timer_join_before_any_attach(self):
        await self.ready()
        pending = asyncio.Event()
        async def slow(*argv, **kwargs):
            if argv[0] == 'powershell.exe':
                proc = await self.raw_spawn(sys.executable, '-c', 'import time; time.sleep(3600)', **kwargs)
                self.created.append(proc)
                self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': sys.executable}
                pending.set(); return proc
            return await self.spawn(*argv, **kwargs)
        self.observer.raw_spawn = slow
        self.observer.task = asyncio.create_task(self.observer.capture())
        await pending.wait()
        self.assertTrue(await self.observer.stop())
        self.assertTrue(self.observer.task.done()); self.assertFalse(self.observer.manifest['frames'])
        self.assertTrue(all(p.returncode is not None for p in self.created))
        # Expiry uses the same joined stop even if the CLI has not returned.
        timer_temp = self.root/'timer-temp'; timer_temp.mkdir(); self.run.temp = timer_temp
        obj = desktop.Observer(self.run, self.config, self.spawn, lambda pid: self.identities[pid], lambda record: None, proof_support)
        async def held():await asyncio.sleep(3600)
        obj.pump = held
        with patch.object(desktop, 'LIFETIME', .02):
            obj.start(); await asyncio.sleep(.1)
            self.assertFalse(obj.open); self.assertTrue(obj.task.done())
            self.assertTrue(obj.timer.done()); self.assertTrue(obj.shutdown.done())
            self.assertTrue(await obj.stop())

    async def test_actual_http_refusals_never_select_error_body_or_bad_headers(self):
        bodies = [
            b'HTTP/1.1 302 Found\r\nLocation: http://invalid/secret\r\n\r\nprivate-marker',
            b'HTTP/1.1 204 No Content\r\n\r\n',
            b'HTTP/1.1 410 Gone\r\n\r\n',
            b'HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 1000000000\r\n\r\n',
            b'HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 100\r\nContent-Length: 100\r\n\r\n',
            b'HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\n\r\n',
            b'HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 100\r\nx-frame-seq: 1\r\nx-frame-width: 16\r\nx-frame-height: 16\r\n\r\npartial',
            b'HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 100\r\nx-frame-seq: private-marker\r\n\r\n']
        current = b''; peers = set()
        async def serve(reader, writer):
            peers.add(asyncio.current_task())
            try:
                await reader.readuntil(b'\r\n\r\n'); writer.write(current); await writer.drain()
            finally:
                writer.close(); await writer.wait_closed(); peers.discard(asyncio.current_task())
        server = await asyncio.start_server(serve, '127.0.0.1', 0)
        self.observer.port = server.sockets[0].getsockname()[1]; self.observer.bearer = 'e'*64
        try:
            for current in bodies:
                try:self.assertIsNone(await self.observer.http_frame())
                except (ValueError, asyncio.IncompleteReadError):pass
                self.assertFalse(self.observer.writers)
                self.assertFalse(self.observer.manifest['frames'])
                self.assertNotIn('private-marker', json.dumps(self.observer.manifest))
            self.observer.manifest['requests'] = 40
            with self.assertRaises(ValueError):await self.observer.http_frame()
            self.assertEqual(self.observer.manifest['requests'], 40)
        finally:
            server.close(); await server.wait_closed(); await asyncio.gather(*peers, return_exceptions=True)

    async def test_bad_startup_scope_and_actual_capture_budget(self):
        original = self.server_file.read_text()
        changes = [("'read_only':True", "'read_only':False"),
                   ('http://127.0.0.1:{port}', 'http://192.0.2.1:{port}'),
                   ("'d'*64", "'d'*63"),
                   ("'notices':[]", "'notices':['private-marker']"),
                   ("'read_only':True", "'read_only':True,'unexpected':'private-marker'")]
        for index, (before, after) in enumerate(changes):
            with self.subTest(startup=index):
                self.server_file.write_text(original.replace(before, after))
                if index == 0:
                    obj = self.observer
                else:
                    fresh_temp = self.root/f'startup-{index}'; fresh_temp.mkdir(); self.run.temp = fresh_temp
                    obj = desktop.Observer(self.run, self.config, self.spawn, lambda pid: self.identities[pid], lambda record: None, proof_support)
                with self.assertRaises(ValueError):await obj.start_viewer()
                self.assertFalse(await obj.stop())
                self.assertTrue(all(p.returncode is not None for p in self.created))
                self.assertFalse(obj.manifest['frames'])
                self.assertNotIn('private-marker', json.dumps(obj.manifest))
        self.server_file.write_text(original)
        fresh_temp = self.root/'second-temp'; fresh_temp.mkdir(); self.run.temp = fresh_temp
        obj = desktop.Observer(self.run, self.config, self.spawn, lambda pid: self.identities[pid], lambda record: None, proof_support)
        await obj.start_viewer()
        with patch.object(desktop, 'TOTAL_BYTES', 1):await obj.capture()
        self.assertEqual(obj.manifest['state'], 'budget_exhausted')
        self.assertFalse(obj.manifest['frames']); self.assertTrue(await obj.stop())

    async def test_first_fallback_failure_still_joins_other_children_and_invalidates_png(self):
        await self.ready(); await self.observer.capture()
        async def refused():raise PermissionError('private-marker')
        self.observer.interrupt = refused
        original = self.observer.kill_join; failures = []
        async def first(proc):
            if proc is self.observer.viewer and not failures:
                failures.append(proc.pid); raise PermissionError('private-marker')
            return await original(proc)
        self.observer.kill_join = first
        self.assertFalse(await self.observer.stop())
        self.assertTrue(failures); self.assertTrue(all(p.returncode is not None for p in self.created))
        self.assertFalse(list(self.output.glob('*.png')))
        self.assertNotIn('private-marker', json.dumps(self.observer.manifest))

    async def test_native_binding_envelope_refuses_absent_ambiguous_and_malformed(self):
        original = self.spawn
        for matches in ([], [{'id': 7, 'state': 0}, {'id': 8, 'state': 0}], [{'id': True, 'state': 0}], 'private-marker'):
            async def native(*argv, **kwargs):
                if argv[0] == 'powershell.exe':
                    proc = await self.raw_spawn(sys.executable, '-c', f'print({json.dumps({"matches":matches})!r})', **kwargs)
                    self.created.append(proc)
                    self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': sys.executable}
                    return proc
                return await original(*argv, **kwargs)
            self.observer.raw_spawn = native
            self.assertIsNone(await self.observer.binding())
            self.assertFalse(self.observer.manifest['frames'])
            self.assertNotIn('private-marker', json.dumps(self.observer.manifest))
        self.assertTrue(await self.observer.stop())

    async def test_port_errors_do_not_claim_closure_and_invalidated_pngs_never_promote(self):
        self.observer.port = 12345
        with patch.object(asyncio, 'open_connection', side_effect=PermissionError('private-marker')):
            self.assertFalse(await self.observer.ports_closed())
        with patch.object(asyncio, 'open_connection', side_effect=ConnectionRefusedError()):
            self.assertTrue(await self.observer.ports_closed())
        record = manifest(self.output)
        self.observer.manifest = record
        with patch.object(Path, 'unlink', side_effect=PermissionError('private-marker')):
            self.observer.invalidate()
        self.assertFalse(self.observer.manifest['frames'])
        self.assertEqual(desktop.selected_files(self.output, self.observer.manifest, SOURCE), [])
        self.assertNotIn('private-marker', json.dumps(self.observer.manifest))

    async def test_actual_full_wrapper_failure_success_boundary_timeout_and_cancel(self):
        for outcome in ('failure', 'success_boundary', 'timeout', 'cancel', 'scan_false', 'cleanup_failure'):
            with self.subTest(outcome=outcome):
                await self.measure_wrapper(outcome)

    async def measure_wrapper(self, outcome):
        private = self.root/('private-'+outcome); private.mkdir()
        marker = private/'decoder-started'
        for target in ('a', 'b'):
            (private/(target+'.json')).write_text(json.dumps({'host': '127.0.0.1', 'port': 3389,
                'username': 'owned', 'domain': 'fixture', 'password': 'secretcredential'}))
        config = {**self.config, 'mode': 'cua_diagnostic', 'fresh_profile_verified': True,
                  'username': 'owned', 'domain': 'fixture'}
        (private/'config.json').write_text(json.dumps(config))
        payload = {'bridge_live': True} if outcome == 'success_boundary' else {'error': {
            'code': 'internal', 'message': 'Internal: bridge bootstrap failed: '+proof_support.BOOTSTRAP_TIMEOUT+'launch_input_sent'}}
        cli_file = private/'cli.py'
        cli_file.write_text('import json, pathlib, time\n' +
            f'p=pathlib.Path({str(marker)!r})\n' +
            'deadline=time.monotonic()+5\nwhile not p.exists() and time.monotonic()<deadline: time.sleep(.01)\n' +
            ('time.sleep(60)\n' if outcome in ('timeout', 'cancel') else 'time.sleep(.2)\n') +
            f'print({json.dumps(payload)!r},flush=True)\nraise SystemExit({0 if outcome == "success_boundary" else 1})\n')
        instances = []; attach = []; products = []; original_factory = desktop.observed_run
        observer_class = desktop.Observer
        def observer_factory(run, handoff, spawn, identity, journal, support):
            obj = observer_class(run, handoff, spawn, lambda pid: self.identities[pid], journal, support)
            if outcome == 'cleanup_failure':
                original_stop = obj.stop
                async def failed_stop():
                    await original_stop()
                    obj.manifest['cleanup']['viewer_zero_exit'] = False
                    obj.invalidate()
                    return False
                obj.stop = failed_stop
            instances.append(obj); return obj
        def run_factory(base, factory):
            cls = original_factory(base, factory)
            init = cls.__init__
            def setup(run, args):
                init(run, args)
                if outcome == 'timeout':run.args.connect_timeout = .3
            cls.__init__ = setup
            async def attachment(run, target):
                obj = run.bootstrap_observer
                self.assertFalse(obj.open)
                self.assertTrue(obj.task.done()); self.assertTrue(obj.shutdown.done())
                self.assertTrue(all(p.returncode is not None for p in obj.children))
                attach.append(target)
                raise RuntimeError('Controlled stop after actual attach boundary')
            cls.attach = attachment
            return cls
        async def source_spawn(*argv, **kwargs):
            if argv[0] == str(self.bin/'rdpilot-daemon'):
                proc = await self.raw_spawn(sys.executable, '-c', 'import time; time.sleep(3600)', **kwargs)
                self.created.append(proc); products.append(proc)
                self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': str(self.bin/'rdpilot-daemon.exe')}
                return proc
            if len(argv) > 1 and argv[0] == str(self.bin/'rdpilot') and argv[1] == 'connect':
                proc = await self.raw_spawn(sys.executable, str(cli_file), **kwargs)
                self.created.append(proc); products.append(proc)
                self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': str(self.bin/'rdpilot.exe')}
                return proc
            if len(argv) > 1 and argv[0] == str(self.bin/'rdpilot') and argv[1] == 'disconnect':
                proc = await self.raw_spawn(sys.executable, '-c', "print('{}')", **kwargs)
                self.created.append(proc); products.append(proc)
                self.identities[proc.pid] = {'pid': proc.pid, 'created': '1', 'image': str(self.bin/'rdpilot.exe')}
                return proc
            if '--decode' in argv:
                marker.write_text('started')
            return await self.spawn(*argv, **kwargs)
        args = argparse.Namespace(proof='cua', bin_dir=str(self.bin), bundle=str(self.root/'bundle'),
            output=str(private/'raw'), artifacts=str(private/'artifacts'), source_bridge_sha256=baseline.HASH,
            journal=str(private/'processes.json'), credentials_a=str(private/'a.json'), credentials_b=str(private/'b.json'),
            bootstrap_desktop=True, bootstrap_desktop_config=str(private/'config.json'), timeout=1 if outcome == 'cancel' else 10)
        with patch.object(tempfile, 'tempdir', str(private)), patch.object(asyncio, 'create_subprocess_exec', side_effect=source_spawn), \
                patch.object(live, 'process_identity', side_effect=lambda pid: self.identities[pid]), \
                patch.object(desktop, 'Observer', side_effect=observer_factory), patch.object(desktop, 'observed_run', side_effect=run_factory), \
                patch.object(desktop, 'SAMPLES', (0, .03, .12, .2, .35)), \
                patch.object(live, 'scan_files', return_value=False) if outcome == 'scan_false' else patch.object(live, 'scan_files', wraps=live.scan_files):
            self.assertEqual(await live.execute(args), 1)
        artifact = json.loads((private/'artifacts/cua.json').read_text())
        obj = instances[0]
        self.assertTrue(all(p.returncode is not None for p in obj.children))
        self.assertTrue(all(p.returncode is not None for p in products))
        self.assertFalse(obj.open); self.assertFalse(obj.writers)
        self.assertEqual(attach, ['a'] if outcome == 'success_boundary' else [])
        self.assertEqual(artifact['status'], 'failed')
        self.assertNotIn('secretcredential', json.dumps(artifact)); self.assertNotIn('d'*64, json.dumps(artifact))
        if outcome != 'success_boundary':
            primary = artifact['harness_failure']
            self.assertEqual(primary['operation'], 'connect_cli')
            self.assertEqual(primary, args._primary_failure)
            if outcome == 'failure':self.assertEqual(primary['connect_observation']['producer'], 'sdk_bootstrap')
            if outcome == 'timeout':self.assertEqual(primary['exception_category'], 'timeout')
            if outcome == 'cancel':self.assertEqual(primary['exception_category'], 'other')
            fallback = live.failed_fallback(args, PermissionError('private-marker'))
            self.assertEqual(fallback['harness_failure'], primary)
        diag = json.loads((private/'artifacts/bootstrap-desktop.json').read_text())
        self.assertNotIn('secretcredential', json.dumps(diag)); self.assertNotIn('S-1-5-21', json.dumps(diag))
        admitted = desktop.selected_files(private/'artifacts', diag, self.source)
        if outcome == 'failure':self.assertTrue(admitted)
        if outcome in ('scan_false', 'cleanup_failure'):self.assertFalse(admitted)
        if outcome == 'cleanup_failure':
            self.assertEqual(artifact['secondary_failures'][0]['operation'], 'bootstrap_cleanup')
        self.assertEqual(sorted(p.name for p in (private/'artifacts').glob('*.png')), sorted(admitted))
        self.assertFalse(any(p.name.startswith('rdpilot-cua-') for p in private.iterdir()))


class ActualRunWrapperTests(unittest.IsolatedAsyncioTestCase):
    async def test_actual_cli_primary_frozen_before_observer_failure_and_no_ready_barrier(self):
        spec = importlib.util.spec_from_file_location('observer_actual_cua', live.E2E/'run-cua-e2e.py')
        module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        actions = []
        class ObserverFixture:
            support = proof_support
            def __init__(self, run):self.run = run
            def start(self):actions.append('observer_start')
            def close_epoch(self):self.open = False
            async def stop(self):
                actions.append('observer_stop')
                self.assert_primary = copy.deepcopy(self.run.primary_failure)
                await asyncio.sleep(.01); return False
        class Child:
            returncode = 1
            async def communicate(self):
                actions.append('actual_connect')
                return json.dumps({'error': {'code': 'internal', 'message': 'Internal: bridge bootstrap failed: '+proof_support.BOOTSTRAP_TIMEOUT+'launch_input_sent'}}).encode(), b'private-secret'
        observed = desktop.observed_run(module.Run, ObserverFixture)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for target in ('a', 'b'):
                (root/(target+'.json')).write_text(json.dumps({'username': 'owned', 'password': 'secretcredential'}))
            args = argparse.Namespace(bin_dir=str(root/'bin'), output=str(root/'out'), bundle=None,
                credentials_a=str(root/'a.json'), credentials_b=str(root/'b.json'))
            run = observed(args); run.operation = 'connect_cli'
            try:
                async def spawn(*argv, **kwargs):
                    self.assertEqual(argv[1], 'connect')
                    self.assertIn(argv[2], ('a', 'b'))
                    self.assertEqual(kwargs['env']['E2E_PASSWORD_'+argv[2].upper()], 'secretcredential')
                    return Child()
                with patch.object(asyncio, 'create_subprocess_exec', side_effect=spawn):
                    with self.assertRaises(module.ProofError):await run.cli('connect', 'a', target='a', timeout=7)
                self.assertEqual(actions, ['observer_start', 'actual_connect', 'observer_stop'])
                primary = run.primary_failure
                self.assertEqual(primary, run.bootstrap_observer.assert_primary)
                self.assertEqual(primary['connect_observation']['producer'], 'sdk_bootstrap')
                self.assertEqual(primary['cli_exit_code'], 1)
                self.assertEqual(run.cleanup_failures[0]['operation'], 'bootstrap_cleanup')
                with patch.object(asyncio, 'create_subprocess_exec', side_effect=spawn):
                    for target in ('a', 'b'):
                        with self.assertRaises(module.ProofError):await run.cli('connect', target, target=target, timeout=7)
                self.assertEqual(actions.count('observer_start'), 1)
                self.assertEqual(actions.count('observer_stop'), 1)
                self.assertEqual(run.primary_failure, primary)
                self.assertNotIn('private-secret', json.dumps(primary))
            finally:
                import shutil; shutil.rmtree(run.temp)


if __name__ == '__main__':
    unittest.main()
