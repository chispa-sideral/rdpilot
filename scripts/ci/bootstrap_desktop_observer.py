"""Opt-in host-only reads of the first disposable bootstrap desktop.

Only the parent admits images. Every subprocess is held before journaling and
joined before the connect wrapper returns. This is diagnostic evidence, never
an RDP, guest provenance, or live-proof success assertion.
"""
import asyncio
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import re
import signal
import struct
import subprocess
import sys
import time
import warnings
from urllib.parse import urlsplit
import acquisition_observer as files

PNG_BYTES = 8 * 1024 * 1024
PIXELS = 16_000_000
TOTAL_BYTES = 160 * 1024 * 1024
SAMPLES = (2, 5, 9, 12, 15, 20, 30, 40, 50, 60, 75, 90, 120, 150, 180, 210, 240, 270, 300)
LIFETIME = 300
STOP_SECONDS = 40
HASH = re.compile(r'[0-9a-f]{64}\Z')
SOURCE_KEYS = ('source_commit', 'source_tree', 'cli_sha256', 'daemon_sha256', 'bridge_sha256')
CLEANUP_KEYS = ('epoch_closed', 'tasks_joined', 'children_joined', 'viewer_ctrl_c',
                'viewer_zero_exit', 'ports_closed', 'controller_survived', 'sibling_survived')
STATES = {'unavailable', 'observed', 'observation_failed', 'budget_exhausted'}
EVENTS = {'not_ready', 'binding_unavailable', 'binding_changed', 'unchanged', 'closed',
          'invalid_frame', 'timeout', 'unavailable', 'captured', 'missed', 'budget_exhausted'}


def integer(value, low=0, high=2**63-1):
    return type(value) is int and low <= value <= high


def read_plain(path, budget):
    with files.open_plain(Path(path), budget) as stream:
        body = stream.read(budget+1)
    if len(body) > budget:
        raise ValueError('File budget')
    return body


def validate_png(body, width, height):
    """Full pinned decoder, preceded by byte/IHDR/pixel allocation admission."""
    from PIL import Image
    if not isinstance(body, bytes) or not 33 <= len(body) <= PNG_BYTES:
        raise ValueError('PNG bytes')
    if body[:8] != b'\x89PNG\r\n\x1a\n' or body[8:16] != b'\x00\x00\x00\rIHDR':
        raise ValueError('PNG structure')
    dims = struct.unpack('>II', body[16:24])
    if (not integer(width, 1, PIXELS) or not integer(height, 1, PIXELS)
            or width * height > PIXELS or dims != (width, height)):
        raise ValueError('PNG dimensions')
    # Pillow stops at IEND; refuse any trailing or truncated chunk envelope.
    offset = 8; ended = False
    while offset < len(body):
        if offset+12 > len(body):
            raise ValueError('PNG chunk')
        length = struct.unpack('>I', body[offset:offset+4])[0]
        end = offset+length+12
        if end > len(body):
            raise ValueError('PNG chunk')
        if body[offset+4:offset+8] == b'IEND':
            if length != 0 or end != len(body):
                raise ValueError('PNG end')
            ended = True
        offset = end
    if not ended:
        raise ValueError('PNG end')
    with warnings.catch_warnings():
        warnings.simplefilter('error')
        with Image.open(io.BytesIO(body)) as image:
            if image.format != 'PNG' or image.size != dims or getattr(image, 'n_frames', 1) != 1:
                raise ValueError('PNG format')
            image.verify()
        with Image.open(io.BytesIO(body)) as image:
            image.load()
            if image.size != dims:
                raise ValueError('PNG decode')
    return dims


def source_valid(source):
    return (isinstance(source, dict) and set(source) == set(SOURCE_KEYS)
            and all(isinstance(source[k], str) and re.fullmatch(r'[a-f0-9]{40}', source[k])
                    for k in SOURCE_KEYS[:2])
            and all(isinstance(source[k], str) and HASH.fullmatch(source[k]) for k in SOURCE_KEYS[2:]))


def empty_manifest(source):
    return {'schema': 1, 'mode': 'cua_diagnostic', 'target': 'a', 'source': source,
            'state': 'unavailable', 'os_session_id': 0, 'requests': 0, 'elapsed_ms': 0,
            'events': [], 'frames': [], 'cleanup': {k: False for k in CLEANUP_KEYS}}


def selected_files(root, manifest, expected):
    """Independent admission used at both projection layers; fail closed."""
    try:
        if (not source_valid(expected) or not isinstance(manifest, dict)
                or set(manifest) != set(empty_manifest(expected))
                or manifest['schema'] != 1 or type(manifest['schema']) is not int
                or manifest['mode'] != 'cua_diagnostic' or manifest['target'] != 'a'
                or manifest['source'] != expected or manifest['state'] not in {'observed', 'budget_exhausted'}
                or not integer(manifest['requests'], 0, 40)
                or not integer(manifest['elapsed_ms'], 0, LIFETIME*1000)
                or not isinstance(manifest['events'], list) or len(manifest['events']) > 40
                or any(type(e) is not str or e not in EVENTS for e in manifest['events'])
                or not isinstance(manifest['cleanup'], dict)
                or set(manifest['cleanup']) != set(CLEANUP_KEYS)
                or any(v is not True for v in manifest['cleanup'].values())
                or not integer(manifest['os_session_id'], 1, 2**31-1)
                or not isinstance(manifest['frames'], list) or len(manifest['frames']) > 20):
            return []
        if len(manifest['frames']) > manifest['requests']:
            return []
        result = []; previous = 0; previous_time = 0; total = 0
        root = Path(root)
        files.ancestry(root)
        for index, frame in enumerate(manifest['frames'], 1):
            name = f'bootstrap-a-{index:02}.png'
            if (not isinstance(frame, dict) or set(frame) != {'name', 'seq', 'width', 'height', 'bytes', 'sha256', 'elapsed_ms'}
                    or frame['name'] != name or not integer(frame['seq'], previous+1)
                    or not integer(frame['bytes'], 33, PNG_BYTES)
                    or not integer(frame['elapsed_ms'], previous_time, manifest['elapsed_ms'])
                    or not isinstance(frame['sha256'], str) or not HASH.fullmatch(frame['sha256'])):
                return []
            path = root/name
            body = read_plain(path, PNG_BYTES)
            if len(body) != frame['bytes']:
                return []
            if hashlib.sha256(body).hexdigest() != frame['sha256']:
                return []
            validate_png(body, frame['width'], frame['height'])
            total += len(body)
            if total > TOTAL_BYTES:
                return []
            result.append(name); previous = frame['seq']; previous_time = frame['elapsed_ms']
        return result
    except Exception:
        return []


def safe_manifest(root, raw, expected, scanned=True):
    """No arbitrary manifest text survives even when its images are refused."""
    fallback = empty_manifest(expected if source_valid(expected) else {})
    fallback['state'] = 'observation_failed'
    try:
        if (not scanned or not source_valid(expected) or not isinstance(raw, dict)
                or set(raw) != set(fallback) or type(raw['schema']) is not int or raw['schema'] != 1
                or raw['mode'] != 'cua_diagnostic' or raw['target'] != 'a' or raw['source'] != expected
                or type(raw['state']) is not str or raw['state'] not in STATES
                or not integer(raw['os_session_id'], 0, 2**31-1)
                or not integer(raw['requests'], 0, 40) or not integer(raw['elapsed_ms'], 0, LIFETIME*1000)
                or not isinstance(raw['cleanup'], dict) or set(raw['cleanup']) != set(CLEANUP_KEYS)
                or any(type(v) is not bool for v in raw['cleanup'].values())
                or not isinstance(raw['events'], list) or len(raw['events']) > 40
                or any(type(e) is not str or e not in EVENTS for e in raw['events'])
                or not isinstance(raw['frames'], list) or len(raw['frames']) > 20
                or len(selected_files(root, raw, expected)) != len(raw['frames'])):
            return fallback
        return json.loads(json.dumps(raw))
    except Exception:
        return fallback


def retain_failed_images(root, expected, enabled):
    """Only an exact diagnostic manifest may exempt PNGs from failed cleanup."""
    root = Path(root); admitted = []
    try:
        path = root/'bootstrap-desktop.json'
        if enabled:
            raw = json.loads(read_plain(path, 64*1024), object_pairs_hook=files.unique_json)
            safe = safe_manifest(root, raw, expected)
            admitted = selected_files(root, safe, expected)
            path.write_text(json.dumps(safe, indent=2)+'\n')
    except Exception:
        if enabled:
            (root/'bootstrap-desktop.json').write_text(json.dumps(safe_manifest(root, None, expected)))
    if not enabled:
        (root/'bootstrap-desktop.json').unlink(missing_ok=True)
    for image in root.glob('*.png'):
        if image.name not in admitted:
            image.unlink()
    return admitted


class Observer:
    def __init__(self, run, config, spawn, identity, journal, console_support):
        self.run = run; self.config = config; self.raw_spawn = spawn
        self.identity = identity; self.journal = journal; self.support = console_support
        self.windows = os.name == 'nt'
        self.children = {}; self.writers = set(); self.viewer = None; self.sentinel = None
        self.port = None; self.bearer = None; self.session_id = None
        self.timer = None; self.shutdown = None
        self.task = None; self.open = True; self.started = time.monotonic()
        self.manifest = empty_manifest(config['source'])
        self.work = run.temp/'bootstrap-observer'; self.work.mkdir()
        self.invalidated = False; self.socket_failure = False
        self.no_window = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}

    async def kill_join(self, proc):
        if proc.returncode is None:
            record = self.children.get(proc)
            if record is not None and self.identity(proc.pid) != record:
                raise ValueError('Owned child identity changed')
            proc.kill()
        await asyncio.wait_for(proc.wait(), 5)

    async def spawn(self, *argv, **kwargs):
        # Shield creation itself: cancellation may arrive before a held object.
        creation = asyncio.create_task(self.raw_spawn(*argv, **kwargs))
        cancelled = False
        try:
            proc = await asyncio.shield(creation)
        except asyncio.CancelledError:
            proc = await creation; cancelled = True
        self.children[proc] = None
        try:
            record = self.identity(proc.pid)
            self.children[proc] = record
            if record is not None:
                self.journal(record)
            if cancelled:
                raise asyncio.CancelledError()
            return proc
        except BaseException:
            await self.kill_join(proc)
            raise

    async def child_output(self, proc, seconds=5):
        async def read():
            body = bytearray()
            while True:
                part = await proc.stdout.read(4096)
                if not part:
                    break
                body.extend(part)
                if len(body) > 64*1024:
                    raise ValueError('Child output budget')
            await proc.wait()
            if proc.returncode:
                raise ValueError('Child refused')
            return bytes(body)
        try:
            return await asyncio.wait_for(read(), seconds)
        finally:
            if proc.returncode is None:
                await self.kill_join(proc)

    def event(self, label):
        if len(self.manifest['events']) < 40:
            self.manifest['events'].append(label)

    def invalidate(self):
        self.invalidated = True; self.manifest['state'] = 'observation_failed'
        for frame in self.manifest['frames']:
            try:
                (self.run.output/frame['name']).unlink(missing_ok=True)
            except OSError:
                self.socket_failure = True
        self.manifest['frames'] = []

    async def binding(self):
        env = dict(self.run.env)
        for key in tuple(env):
            if key.casefold() == 'psmodulepath':
                env.pop(key)
        env['RDPILOT_HOST_CONTROL'] = json.dumps({'sid': self.config['sid']})
        proc = await self.spawn('powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand',
            base64.b64encode(self.config['wts_script'].encode('utf-16le')).decode(), env=env,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL, **self.no_window)
        raw = json.loads((await self.child_output(proc)).decode('utf-8-sig'))
        matches = raw.get('matches') if isinstance(raw, dict) else None
        if not isinstance(matches, list) or len(matches) != 1:
            if self.session_id is not None:
                self.event('binding_changed'); self.invalidate()
            return None
        row = matches[0]
        if not isinstance(row, dict) or set(row) != {'id', 'state'} or not integer(row['id'], 1, 2**31-1) or row['state'] != 0 or type(row['state']) is not int:
            return None
        if self.session_id is not None and row['id'] != self.session_id:
            self.event('binding_changed'); self.invalidate(); return None
        self.session_id = row['id']; self.manifest['os_session_id'] = row['id']
        return row['id']

    async def close_writer(self, writer):
        writer.close()
        try:
            await asyncio.wait_for(writer.wait_closed(), 1)
        except (OSError, asyncio.TimeoutError):
            writer.transport.abort()
            try:
                await asyncio.wait_for(writer.wait_closed(), 1)
            except (OSError, asyncio.TimeoutError):
                self.socket_failure = True
                return
        self.writers.discard(writer)

    async def http_frame(self):
        if not self.open or self.manifest['requests'] >= 40:
            raise ValueError('Closed capture epoch')
        async def request():
            writer = None
            try:
                reader, writer = await asyncio.open_connection('127.0.0.1', self.port, limit=16*1024)
                self.writers.add(writer)
                writer.write((f'GET /api/sessions/a/frame?after=0 HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\n'
                              f'Authorization: Bearer {self.bearer}\r\nConnection: close\r\n\r\n').encode('ascii'))
                await writer.drain()
                header = await reader.readuntil(b'\r\n\r\n')
                if len(header) > 16*1024:
                    raise ValueError('Headers budget')
                lines = header.decode('ascii').split('\r\n')
                status = lines[0].split(' ')
                if len(status) < 2 or status[0] not in ('HTTP/1.1', 'HTTP/1.0') or not re.fullmatch('[0-9]{3}', status[1]):
                    raise ValueError('Status')
                headers = {}
                for line in lines[1:]:
                    if not line:
                        continue
                    key, value = line.split(':', 1); key = key.lower()
                    if key in headers:
                        raise ValueError('Duplicate header')
                    headers[key] = value.strip()
                code = int(status[1])
                if code in (204, 410):
                    self.event('unchanged' if code == 204 else 'closed'); return None
                if code != 200:
                    self.event('unavailable'); return None
                length = headers.get('content-length', '')
                if (headers.get('content-type') != 'image/png' or 'transfer-encoding' in headers
                        or not re.fullmatch(r'[0-9]{1,8}', length) or not 33 <= int(length) <= PNG_BYTES):
                    raise ValueError('Response admission')
                values = []
                for key in ('x-frame-seq', 'x-frame-width', 'x-frame-height'):
                    value = headers.get(key, '')
                    if not re.fullmatch(r'[0-9]{1,19}', value) or not integer(int(value), 1):
                        raise ValueError('Frame headers')
                    values.append(int(value))
                body = await reader.readexactly(int(length))
                return (*values, body)
            finally:
                if writer is not None:
                    await self.close_writer(writer)
        self.manifest['requests'] += 1
        return await asyncio.wait_for(request(), 3)

    async def decode(self, body, width, height):
        path = self.work/'decode.png'; path.write_bytes(body)
        proc = await self.spawn(sys.executable, str(Path(__file__).resolve()), '--decode', str(path), str(width), str(height),
            env=self.run.env, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL, **self.no_window)
        result = json.loads(await self.child_output(proc, 5))
        if result != {'width': width, 'height': height}:
            raise ValueError('Decoder result')

    def admit(self, seq, width, height, body):
        elapsed = int((time.monotonic()-self.started)*1000)
        frames = self.manifest['frames']
        if not self.open or self.invalidated or elapsed > LIFETIME*1000:
            return
        if frames and seq <= frames[-1]['seq']:
            self.event('unchanged'); return
        if len(frames) >= 20 or sum(f['bytes'] for f in frames)+len(body) > TOTAL_BYTES:
            self.manifest['state'] = 'budget_exhausted'; self.event('budget_exhausted'); return
        # No await between the final epoch check and parent-only write.
        name = f'bootstrap-a-{len(frames)+1:02}.png'
        pending = self.run.output/(name+'.pending')
        pending.write_bytes(body)
        pending.replace(self.run.output/name)
        frames.append({'name': name, 'seq': seq, 'width': width, 'height': height,
                       'bytes': len(body), 'sha256': hashlib.sha256(body).hexdigest(), 'elapsed_ms': elapsed})
        self.manifest['state'] = 'observed'; self.event('captured')

    async def capture(self):
        if not self.open:
            return
        before = await self.binding()
        if not before or self.invalidated:
            self.event('binding_unavailable'); return
        result = await self.http_frame()
        if result is None:
            return
        seq, width, height, body = result
        # Admission caps precede starting the isolated decoder.
        if not integer(width, 1, PIXELS) or not integer(height, 1, PIXELS) or width*height > PIXELS:
            raise ValueError('Pixel budget')
        await self.decode(body, width, height)
        after = await self.binding()
        if before != after or not after:
            self.event('binding_changed'); self.invalidate(); return
        self.admit(seq, width, height, body)

    async def start_viewer(self):
        source = self.config['source']
        if not source_valid(source):
            raise ValueError('Source')
        identity_path = self.work/'source.json'
        identity_path.write_text(json.dumps({'source': source, 'bin': str(self.run.bin)}))
        checker = await self.spawn(sys.executable, str(Path(__file__).resolve()), '--source', str(identity_path),
            env=self.run.env, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL, **self.no_window)
        if json.loads(await self.child_output(checker, 5)) != {'source_verified': True}:
            raise ValueError('Build identity')
        if self.run.daemon is None or self.run.daemon.returncode is not None:
            raise ValueError('Held daemon')
        if os.name == 'nt':
            held = self.identity(self.run.daemon.pid)
            if Path(held['image']).resolve() != (self.run.bin/'rdpilot-daemon.exe').resolve():
                raise ValueError('Daemon identity')
        self.sentinel = await self.spawn(sys.executable, '-c', 'import time; time.sleep(3600)',
            stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL, **self.no_window)
        self.viewer = await self.spawn(str(self.run.bin/'rdpilot'), 'view', '--bind', 'loopback', '--read-only', '--json',
            env=self.run.env, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL,
            **self.support.viewer_process_options())
        if os.name == 'nt' and Path(self.children[self.viewer]['image']).resolve() != (self.run.bin/'rdpilot.exe').resolve():
            raise ValueError('Viewer identity')
        async def startup():
            raw = bytearray()
            while len(raw) <= 64*1024:
                part = await self.viewer.stdout.read(4096)
                if not part:
                    raise ValueError('Viewer unavailable')
                raw.extend(part)
                if len(raw) > 64*1024:
                    raise ValueError('Startup budget')
                try:
                    return json.loads(raw, object_pairs_hook=files.unique_json)
                except ValueError:
                    continue
            raise ValueError('Startup budget')
        started = await asyncio.wait_for(startup(), 15)
        if not isinstance(started, dict) or set(started) != {'urls', 'notices', 'read_only'} or started['read_only'] is not True or started['notices'] != []:
            raise ValueError('Viewer scope')
        urls = started['urls']
        if not isinstance(urls, list) or len(urls) != 1 or not isinstance(urls[0], str) or len(urls[0]) > 256:
            raise ValueError('Viewer URLs')
        url = urlsplit(urls[0])
        if (url.scheme != 'http' or url.hostname != '127.0.0.1' or url.username or url.password
                or url.path != '/' or url.fragment or not url.port or not re.fullmatch(r'token=[0-9a-f]{64}', url.query)
                or url.netloc != f'127.0.0.1:{url.port}'):
            raise ValueError('Loopback authority')
        self.port = url.port; self.bearer = url.query[6:]

    async def pump(self):
        try:
            await self.start_viewer()
            for offset in SAMPLES:
                if not self.open or self.invalidated or self.manifest['requests'] >= 40:
                    break
                delay = self.started+offset-time.monotonic()
                if delay < -3:
                    self.event('missed'); continue
                if delay > 0:
                    await asyncio.sleep(delay)
                if not self.open or time.monotonic()-self.started >= LIFETIME:
                    break
                try:
                    await self.capture()
                except asyncio.TimeoutError:
                    self.event('timeout')
                except (OSError, ValueError, KeyError, TypeError, asyncio.IncompleteReadError, asyncio.LimitOverrunError):
                    self.event('invalid_frame')
        except asyncio.CancelledError:
            raise
        except Exception:
            self.event('unavailable')

    def close_epoch(self):
        self.open = False
        self.manifest['elapsed_ms'] = min(LIFETIME*1000, int((time.monotonic()-self.started)*1000))

    def start(self):
        self.task = asyncio.create_task(self.pump())
        async def deadline():
            await asyncio.sleep(max(0, self.started+LIFETIME-time.monotonic()))
            self.close_epoch()
            if self.shutdown is None:
                self.shutdown = asyncio.create_task(self.bounded_stop())
        self.timer = asyncio.create_task(deadline())

    async def ports_closed(self):
        if self.port is None:
            return False
        async def wait():
            while True:
                try:
                    reader, writer = await asyncio.open_connection('127.0.0.1', self.port)
                except ConnectionRefusedError:
                    return True
                except OSError:
                    return False
                self.writers.add(writer); await self.close_writer(writer)
                await asyncio.sleep(0.1)
        try:
            return await asyncio.wait_for(wait(), 5)
        except asyncio.TimeoutError:
            return False

    async def interrupt(self):
        if self.windows:
            helper = await self.spawn(sys.executable, str(Path(self.support.__file__).resolve()), '--interrupt-console', str(self.viewer.pid),
                stdout=asyncio.subprocess.DEVNULL, stderr=asyncio.subprocess.DEVNULL, **self.no_window)
            try:
                if await asyncio.wait_for(helper.wait(), 5) != 0:
                    raise ValueError('Ctrl-C refused')
            finally:
                if helper.returncode is None:
                    await self.kill_join(helper)
        else:
            self.viewer.send_signal(signal.SIGINT)

    async def stop_viewer(self):
        cleanup = self.manifest['cleanup']
        if self.viewer is None:
            return
        try:
            if self.viewer.returncode is not None:
                raise ValueError('Viewer exited early')
            await self.interrupt()
            cleanup['viewer_ctrl_c'] = True
            cleanup['viewer_zero_exit'] = await asyncio.wait_for(self.viewer.wait(), 10) == 0
            cleanup['controller_survived'] = True
            cleanup['sibling_survived'] = self.sentinel is not None and self.sentinel.returncode is None
            cleanup['ports_closed'] = await self.ports_closed()
        except Exception:
            pass
        if not all(cleanup[k] for k in ('viewer_ctrl_c', 'viewer_zero_exit', 'ports_closed')):
            if self.viewer.returncode is None:
                record = self.children[self.viewer]
                if os.name == 'nt' and record is not None and self.identity(self.viewer.pid) != record:
                    raise ValueError('Fallback identity changed')
                await self.kill_join(self.viewer)
            # Fallback cannot promote graceful success.
            await self.ports_closed()

    async def _stop(self):
        cleanup = self.manifest['cleanup']
        cleanup['epoch_closed'] = True
        if self.task is not None:
            self.task.cancel()
            try:
                await self.task
                cleanup['tasks_joined'] = True
            except asyncio.CancelledError:
                cleanup['tasks_joined'] = True
            except Exception:
                cleanup['tasks_joined'] = self.task.done()
        else:
            cleanup['tasks_joined'] = True
        try:
            await self.stop_viewer()
        finally:
            await asyncio.gather(*(self.close_writer(w) for w in tuple(self.writers)), return_exceptions=True)
            results = await asyncio.gather(*(self.kill_join(p) for p in self.children), return_exceptions=True)
            cleanup['children_joined'] = all(not isinstance(r, BaseException) for r in results) and all(p.returncode is not None for p in self.children)
        quiescent = cleanup['tasks_joined'] and cleanup['children_joined'] and not self.writers and not self.socket_failure
        return quiescent and (self.viewer is None or all(cleanup.values()))

    async def stop(self):
        self.close_epoch()
        if self.shutdown is None:
            self.shutdown = asyncio.create_task(self.bounded_stop())
        return await asyncio.shield(self.shutdown)

    async def bounded_stop(self):
        if self.timer is not None and not self.timer.done():
            self.timer.cancel()
            await asyncio.gather(self.timer, return_exceptions=True)
        try:
            okay = await asyncio.wait_for(self._stop(), STOP_SECONDS-10)
        except BaseException:
            okay = False
            if self.task is not None:
                self.task.cancel()
            try:
                if self.task is not None:
                    await asyncio.wait_for(asyncio.gather(self.task, return_exceptions=True), 5)
            except BaseException:
                pass
            try:
                await asyncio.wait_for(asyncio.gather(
                    *(self.kill_join(p) for p in self.children),
                    *(self.close_writer(w) for w in tuple(self.writers)), return_exceptions=True), 5)
            except BaseException:
                pass
        if not okay:
            self.invalidate()
        return okay


def observed_run(base, factory):
    class ObservedRun(base):
        async def cli(self, *arguments, target=None, timeout=90, allow_failure=False):
            first = arguments[:2] == ('connect', 'a') and target == 'a' and not getattr(self, '_bootstrap_observed', False)
            if not first:
                return await super().cli(*arguments, target=target, timeout=timeout, allow_failure=allow_failure)
            self._bootstrap_observed = True
            self.bootstrap_observer = factory(self)
            self.bootstrap_observer.start()
            failed = False
            try:
                return await super().cli(*arguments, target=target, timeout=timeout, allow_failure=allow_failure)
            except BaseException as error:
                failed = True
                if self.primary_failure is None:
                    self.freeze_connect_failure(error)
                raise
            finally:
                self.bootstrap_observer.close_epoch()
                stop = asyncio.create_task(self.bootstrap_observer.stop())
                try:
                    okay = await asyncio.shield(stop)
                except asyncio.CancelledError:
                    okay = await stop
                    if not failed:
                        raise
                if not okay:
                    error = RuntimeError('Bootstrap observer cleanup failed')
                    self.cleanup_failures.append(self.bootstrap_observer.support.failure_detail(error, 'bootstrap_cleanup'))
                    if not failed:
                        self.operation = 'bootstrap_cleanup'
                        raise error
    return ObservedRun


if __name__ == '__main__':
    try:
        if len(sys.argv) == 3 and sys.argv[1] == '--source':
            config = json.loads(read_plain(Path(sys.argv[2]), 16*1024))
            source = config['source']
            if not source_valid(source):
                raise ValueError('Source identity')
            for name, key in (('rdpilot', 'cli_sha256'), ('rdpilot-daemon', 'daemon_sha256')):
                path = Path(config['bin'])/(name+'.exe' if os.name == 'nt' else name)
                with files.open_plain(path, 64*1024*1024) as stream:
                    actual, _ = files.digest(stream, 64*1024*1024)
                if actual != source[key]:
                    raise ValueError('Build identity')
            print(json.dumps({'source_verified': True}))
        elif len(sys.argv) == 5 and sys.argv[1] == '--decode':
            width, height = int(sys.argv[3]), int(sys.argv[4])
            validate_png(read_plain(Path(sys.argv[2]), PNG_BYTES), width, height)
            print(json.dumps({'width': width, 'height': height}))
        else:
            raise ValueError('Decoder arguments')
    except Exception:
        raise SystemExit(1)
