"""One opt-in, read-only host snapshot after the original first-A bootstrap fails.

Only residual candidate bytes are established; no execution or installation claim.
All profile/source inputs and child output remain in the caller's private boundary.
"""
import base64
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import time

import acquisition_observer as files

OUTPUT_LIMIT = 16 * 1024
CONFIG_LIMIT = 2 * 1024 * 1024
CONTENT_LIMIT = 194 * 1024 * 1024
BINARY_LIMIT = 64 * 1024 * 1024
MANIFEST_LIMIT = 1024 * 1024
_spec = importlib.util.spec_from_file_location('footprint_proof_support', Path(__file__).resolve().parents[1]/'e2e'/'proof_support.py')
_support = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_support)
INITIAL_CHECKS = {'private_child_temporary_boundary_verified', 'evidence_contains_no_credentials'}
STATES = {
    'profile': {'absent', 'present', 'unavailable', 'invalid'},
    'product_directory': {'absent', 'present', 'unavailable', 'invalid'},
    'launcher': {'absent', 'source_match', 'mismatch', 'ambiguous', 'unavailable', 'invalid'},
    'candidate_bundle': {'absent', 'source_binaries_match', 'partial', 'mismatch', 'unavailable', 'invalid'},
}
OUTCOMES = {'observed', 'ineligible', 'unavailable', 'invalid', 'budget', 'cleanup_failed'}
SOURCE_KEYS = {'source_commit', 'source_tree', 'bridge_sha256', 'archive_sha256', 'cua_sha256', 'cua_version', 'bundle_id'}


def eligible(selected, code):
    """Refuse malformed/later failures before any native profile or file read."""
    if type(code) is not int or code == 0 or not isinstance(selected, dict):return False
    if selected.get('proof') != 'cua' or selected.get('status') != 'failed' or selected.get('failure_stage') != 'harness':return False
    checks = selected.get('completed_checks')
    if (not isinstance(checks, list) or any(not isinstance(x, str) for x in checks)
            or len(set(checks)) != len(checks) or not set(checks) <= INITIAL_CHECKS
            or selected.get('identity') != []):return False
    primary = selected.get('harness_failure')
    if not isinstance(primary, dict) or primary.get('operation') != 'connect_cli' or _support.select_failure_detail(primary) != primary:return False
    if primary.get('exception_category') != 'proof_assertion' or type(primary.get('cli_exit_code')) is not int or primary['cli_exit_code'] == 0:return False
    observation = primary.get('connect_observation')
    if not isinstance(observation, dict) or observation.get('producer') != 'sdk_bootstrap' or observation.get('cli_error_code') != 'internal' or observation.get('observation_state') != 'observed':return False
    relays = observation.get('relays')
    if not isinstance(relays, dict) or set(relays) != {'a', 'b'}:return False
    if _support.select_relay_observation(relays['a']) != relays['a'] or relays['a'].get('state') != 'observed':return False
    if not isinstance(observation.get('daemon'), dict) or observation['daemon'].get('state') not in ('alive', 'exited'):return False
    b = relays['b']
    keys = {'state', 'accepted', 'upstream_connected', 'to_client_bytes', 'to_upstream_bytes', 'upstream_failure', 'copy_failure'}
    return (isinstance(b, dict) and set(b) == keys and b['state'] == 'observed'
            and all(type(b[k]) is int and b[k] == 0 for k in ('accepted', 'upstream_connected', 'to_client_bytes', 'to_upstream_bytes'))
            and b['upstream_failure'] == b['copy_failure'] == 'none')


def source_identity(source, acquisition):
    if not isinstance(source, dict) or not isinstance(acquisition, dict):raise ValueError()
    if acquisition.get('state') != 'verified_source_bundle' or type(acquisition.get('bundle_count')) is not int or acquisition['bundle_count'] != 1:raise ValueError()
    identity = {k: source.get(k) if k.startswith('source_') else acquisition.get(k) for k in SOURCE_KEYS}
    if source.get('bridge_sha256') != identity['bridge_sha256']:raise ValueError()
    for key in ('source_commit', 'source_tree'):
        if not isinstance(identity[key], str) or not re.fullmatch('[a-f0-9]{40}', identity[key]):raise ValueError()
    for key in ('bridge_sha256', 'archive_sha256', 'cua_sha256'):
        if not isinstance(identity[key], str) or not files.HASH.fullmatch(identity[key]):raise ValueError()
    expected = 'cua-driver-rs-v0.34.0-' + hashlib.sha256((identity['bridge_sha256'] + identity['archive_sha256']).encode('ascii')).hexdigest()[:16]
    if identity['cua_version'] != '0.34.0' or identity['bundle_id'] != expected:raise ValueError()
    return identity


def empty(source=None, outcome='unavailable', joined=True):
    return {'schema': 1, 'mode': 'cua_diagnostic', 'target': 'a', 'source': source,
            'outcome': outcome, **{k: 'unavailable' for k in STATES},
            'launcher_count': 0, 'staging_count': 0, 'elapsed_ms': 0, 'worker_joined': joined}


def validate(value, source):
    template = empty(source)
    if not isinstance(value, dict) or set(value) != set(template):raise ValueError()
    if type(value['schema']) is not int or value['schema'] != 1 or value['mode'] != 'cua_diagnostic' or value['target'] != 'a' or value['source'] != source:raise ValueError()
    if value['outcome'] not in OUTCOMES or type(value['worker_joined']) is not bool:raise ValueError()
    for key, allowed in STATES.items():
        if not isinstance(value[key], str) or value[key] not in allowed:raise ValueError()
    for key, limit in (('launcher_count', 4), ('staging_count', 4), ('elapsed_ms', 30000)):
        if type(value[key]) is not int or not 0 <= value[key] <= limit:raise ValueError()
    positive = value['launcher'] == 'source_match' or value['candidate_bundle'] == 'source_binaries_match'
    if positive and (value['outcome'] != 'observed' or value['profile'] != 'present' or value['product_directory'] != 'present' or not value['worker_joined']):raise ValueError()
    return json.loads(json.dumps(value))


def signature(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_nlink, info.st_mtime_ns, info.st_ctime_ns)


def ancestors(path):
    files.ancestry(path)
    return tuple((str(p), p.lstat().st_dev, p.lstat().st_ino) for p in (*reversed(path.parents), path))


def ordinary_local(path):
    """Only ordinary absolute drive paths; do not normalize device namespaces."""
    value = str(path)
    if not re.fullmatch(r'[A-Za-z]:\\[^:]*', value) or '/' in value or '\\..\\' in value or '\\.\\' in value:raise ValueError()
    return os.path.normcase(value)


def final_local(value):
    if not value.startswith('\\\\?\\'):raise OSError()
    return ordinary_local(value[4:])


class WindowsFile:
    """An actual share-delete, non-following Windows open, kept through the read."""
    def __init__(self, path):
        from ctypes import wintypes as w
        self.w = w
        self.kernel = k = ctypes.WinDLL('kernel32', use_last_error=True)
        k.CreateFileW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, ctypes.c_void_p, w.DWORD, w.DWORD, w.HANDLE]
        k.CreateFileW.restype = w.HANDLE
        k.CloseHandle.argtypes = [w.HANDLE]
        k.GetFileType.argtypes = [w.HANDLE];k.GetFileType.restype = w.DWORD
        k.GetFinalPathNameByHandleW.argtypes = [w.HANDLE, w.LPWSTR, w.DWORD, w.DWORD]
        k.GetFinalPathNameByHandleW.restype = w.DWORD
        class Info(ctypes.Structure):
            _fields_ = [('attributes', w.DWORD), ('creation', w.FILETIME), ('access', w.FILETIME), ('write', w.FILETIME),
                        ('volume', w.DWORD), ('size_high', w.DWORD), ('size_low', w.DWORD), ('links', w.DWORD), ('index_high', w.DWORD), ('index_low', w.DWORD)]
        self.Info = Info
        k.GetFileInformationByHandle.argtypes = [w.HANDLE, ctypes.POINTER(Info)]
        k.GetFileInformationByHandle.restype = w.BOOL
        self.path = ordinary_local(path)
        self.handle = k.CreateFileW(str(path), 0x80000000, 1 | 2 | 4, None, 3, 0x00200000, None)
        if self.handle == ctypes.c_void_p(-1).value:raise OSError()
        self.transferred = False

    def state(self):
        info = self.Info()
        if not self.kernel.GetFileInformationByHandle(self.handle, ctypes.byref(info)) or self.kernel.GetFileType(self.handle) != 1:raise OSError()
        if info.attributes & (0x400 | 0x10) or info.links != 1:raise ValueError()
        buffer = ctypes.create_unicode_buffer(32768)
        count = self.kernel.GetFinalPathNameByHandleW(self.handle, buffer, len(buffer), 0)
        if not 0 < count < len(buffer) or final_local(buffer.value) != self.path:raise ValueError()
        return (info.volume, info.index_high, info.index_low, info.links, info.attributes,
                info.size_high, info.size_low, info.write.dwHighDateTime, info.write.dwLowDateTime,
                info.creation.dwHighDateTime, info.creation.dwLowDateTime)

    def stream(self):
        import msvcrt
        fd = msvcrt.open_osfhandle(self.handle, os.O_RDONLY | os.O_BINARY)
        self.transferred = True
        try:return os.fdopen(fd, 'rb')
        except BaseException:os.close(fd);raise

    def close(self):
        if not self.transferred:self.kernel.CloseHandle(self.handle)


class Held:
    def __init__(self, path, limit):
        self.path = Path(path);self.limit = limit;self.native = None;self.file = None

    def __enter__(self):
        try:
            self.lineage = ancestors(self.path.parent)
            self.before = signature(files.plain(self.path))
            if self.before[3] != 1:raise ValueError()
            if self.before[2] > self.limit:raise files.BudgetExceeded()
            if os.name == 'nt':
                self.native = WindowsFile(self.path)
                self.native_before = self.native.state()
                self.file = self.native.stream()
            else:self.file = files.open_plain(self.path, self.limit)
            self.check()
            return self
        except BaseException:
            self.close();raise

    def check(self):
        if ancestors(self.path.parent) != self.lineage:raise ValueError()
        if signature(files.plain(self.path)) != self.before or signature(os.fstat(self.file.fileno())) != self.before:raise ValueError()
        if self.native:
            if self.native.state() != self.native_before:raise ValueError()
            # Reopen the named path under identical Windows sharing and compare its
            # native identity as well as the original held final local path.
            named = WindowsFile(self.path)
            try:
                if named.state() != self.native_before:raise ValueError()
            finally:named.close()

    def close(self):
        if self.file:self.file.close()
        if self.native:self.native.close()

    def __exit__(self, kind, value, traceback):
        try:
            if kind is None:self.check()
        finally:self.close()


class Budget:
    def __init__(self, limit=CONTENT_LIMIT):self.remaining = limit
    def read(self, held, limit, digest=False):
        total = 0;blocks = [];hashed = hashlib.sha256()
        while True:
            count = min(1024 * 1024, limit - total + 1, self.remaining + 1)
            block = held.file.read(count)
            self.remaining -= len(block);total += len(block)
            if self.remaining < 0 or total > limit:raise files.BudgetExceeded()
            if not block:break
            if digest:hashed.update(block)
            else:blocks.append(block)
        return hashed.hexdigest() if digest else b''.join(blocks)


def read(path, limit, budget, digest=False):
    with Held(path, limit) as held:return budget.read(held, limit, digest)


def manifest(data, source):
    value = json.loads(data, object_pairs_hook=files.unique_json)
    if not isinstance(value, dict) or set(value) != {'bundle_id', 'cua_version', 'archive_name', 'archive_sha256', 'bridge_sha256', 'files'}:raise ValueError()
    for key in ('bundle_id', 'cua_version', 'archive_sha256', 'bridge_sha256'):
        if type(value[key]) is not str or value[key] != source[key]:raise ValueError()
    if value['archive_name'] != files.ARCHIVE:raise ValueError()
    table = value['files']
    if not isinstance(table, dict) or not 1 <= len(table) <= files.MAX_ENTRIES or table.get('cua-driver.exe') != source['cua_sha256']:raise ValueError()
    if len({key.casefold() for key in table}) != len(table):raise ValueError()
    for key, hashed in table.items():
        if not files.name_valid(key) or key.casefold() in ('manifest.json', 'rdpilot-bridge.exe') or type(hashed) is not str or not files.HASH.fullmatch(hashed):raise ValueError()
    return value


def launcher_name(name):
    if not re.fullmatch(r'l[0-9a-z]{13}\.exe', name):return False
    number = int(name[1:14], 36)
    if number >= 2 ** 64:return False
    digits = '0123456789abcdefghijklmnopqrstuvwxyz';converted = ''
    while number:converted = digits[number % 36] + converted;number //= 36
    return (converted or '0').rjust(13, '0') == name[1:14]


def staging_name(name):
    match = re.fullmatch(r'\.staging-(0|[1-9][0-9]{0,19})-([1-9][0-9]{0,9})', name)
    return bool(match and int(match[1]) < 2 ** 64 and int(match[2]) < 2 ** 32)


def directory_state(path):
    """Verify every existing ancestor, including when the final path is absent."""
    for part in (*reversed(path.parents), path):
        try:files.plain(part, True)
        except FileNotFoundError:return False
    return True


def snapshot(config):
    source = config['source'];result = empty(source);result['profile'] = 'present'
    budget = Budget()
    try:
        profile = Path(config['profile']);cache = Path(config['cache'])
        if os.name == 'nt':ordinary_local(profile);ordinary_local(cache)
        ancestors(profile)
        expected = manifest(read(cache/'bundles'/source['bundle_id']/'manifest.json', MANIFEST_LIMIT, budget), source)
        root = profile/'AppData'/'Local'/'rdpilot'
        if not directory_state(root):
            result.update(outcome='observed', product_directory='absent', launcher='absent', candidate_bundle='absent')
            return result
        lineage = ancestors(root)
        root_identity = root.lstat().st_dev, root.lstat().st_ino
        result['product_directory'] = 'present'
        found = files.entries(root, 16)
        if len({p.name.casefold() for p in found}) != len(found):raise ValueError()
        launchers = [];stages = []
        for path in found:
            if launcher_name(path.name):
                if files.plain(path).st_nlink != 1:raise ValueError()
                launchers.append(path)
            elif staging_name(path.name):
                files.plain(path, True);stages.append(path)
            elif path.name == source['bundle_id']:files.plain(path, True)
            else:raise ValueError()
        if len(launchers) > 4 or len(stages) > 4:raise files.BudgetExceeded()
        result['launcher_count'] = len(launchers);result['staging_count'] = len(stages)
        if not launchers:result['launcher'] = 'absent'
        elif len(launchers) > 1:result['launcher'] = 'ambiguous'
        else:result['launcher'] = 'source_match' if read(launchers[0], BINARY_LIMIT, budget, True) == source['bridge_sha256'] else 'mismatch'
        bundle = root/source['bundle_id']
        if not directory_state(bundle):result['candidate_bundle'] = 'absent'
        else:
            matches = [];missing = False
            for name, limit, target in (('manifest.json', MANIFEST_LIMIT, None), ('rdpilot-bridge.exe', BINARY_LIMIT, source['bridge_sha256']), ('cua-driver.exe', BINARY_LIMIT, source['cua_sha256'])):
                try:
                    data = read(bundle/name, limit, budget, target is not None)
                    matches.append(manifest(data, source) == expected if target is None else data == target)
                except FileNotFoundError:missing = True
            result['candidate_bundle'] = 'mismatch' if False in matches else 'partial' if missing else 'source_binaries_match'
        if ancestors(root) != lineage or (root.lstat().st_dev, root.lstat().st_ino) != root_identity:raise ValueError()
        result['outcome'] = 'observed'
    except files.BudgetExceeded:result = empty(source, 'budget');result['profile'] = 'present'
    except (ValueError, TypeError, KeyError, UnicodeError, RecursionError):result = empty(source, 'invalid');result['profile'] = 'present'
    except (OSError, RuntimeError):result = empty(source);result['profile'] = 'present'
    return result


PROFILE_QUERY = r'''
$ErrorActionPreference='Stop';$v=$env:RDPILOT_FOOTPRINT_QUERY|ConvertFrom-Json
if($v.sid.Length -gt 184 -or $v.name.Length -gt 32 -or $v.marker.Length -gt 128){throw 'Invalid control'}
$u=@(Get-LocalUser -SID $v.sid)
if($u.Count -ne 1 -or $u[0].SID.Value -cne $v.sid -or $u[0].Name -cne $v.name -or $u[0].Description -cne $v.marker){throw 'Ownership unavailable'}
$expected=Join-Path (Join-Path $env:SystemDrive 'Users') $v.name
$p=@(Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'")
if($p.Count -gt 1){throw 'Ambiguous profile'}
$r=@{sid=$u[0].SID.Value;name=$u[0].Name;marker=$u[0].Description;expected=$expected;profile=$null}
if($p.Count -eq 1){
 if($p[0].SID -cne $v.sid -or $p[0].Special -or $p[0].LocalPath -ine $expected -or $p[0].LocalPath.Length -gt 512){throw 'Profile unavailable'}
 $r.profile=@{sid=$p[0].SID;path=$p[0].LocalPath;special=[bool]$p[0].Special;loaded=[bool]$p[0].Loaded}
}
$j=$r|ConvertTo-Json -Depth 4 -Compress
if([Text.Encoding]::UTF8.GetByteCount($j) -gt 16384){throw 'Query budget'}
[Console]::Out.Write($j)
'''


def profile_binding(value, owned):
    if not isinstance(value, dict) or set(value) != {'sid', 'name', 'marker', 'expected', 'profile'}:raise ValueError()
    if any(value[k] != owned[k] for k in ('sid', 'name', 'marker')):raise ValueError()
    if not isinstance(value['expected'], str) or len(value['expected']) > 512:raise ValueError()
    expected = ordinary_local(value['expected'])
    drive = expected[:2]
    if expected != ordinary_local(drive + '\\Users\\' + owned['name']):raise ValueError()
    profile = value['profile']
    if profile is None:return None
    if (not isinstance(profile, dict) or set(profile) != {'sid', 'path', 'special', 'loaded'}
            or profile['sid'] != owned['sid'] or profile['special'] is not False or type(profile['loaded']) is not bool
            or not isinstance(profile['path'], str) or ordinary_local(profile['path']) != expected):raise ValueError()
    return value['expected']


def pipe_bytes(child, count):
    if os.name != 'nt':return os.read(child.stdout.fileno(), count)
    import msvcrt
    from ctypes import wintypes as w
    k = ctypes.WinDLL('kernel32', use_last_error=True)
    k.PeekNamedPipe.argtypes = [w.HANDLE, ctypes.c_void_p, w.DWORD, ctypes.c_void_p, ctypes.POINTER(w.DWORD), ctypes.c_void_p]
    available = w.DWORD()
    if not k.PeekNamedPipe(msvcrt.get_osfhandle(child.stdout.fileno()), None, 0, None, ctypes.byref(available), None):
        if ctypes.get_last_error() == 109:return b''
        raise OSError()
    return os.read(child.stdout.fileno(), min(count, available.value)) if available.value else None


class Runner:
    """Concrete held children, capped receiving, and a common forced-join reserve."""
    def __init__(self, identity, register, total=30):
        self.start = time.monotonic();self.end = self.start + total
        self.work_end = self.end - min(5, total / 2)
        self.identity = identity;self.register = register;self.children = [];self.joined = True

    def stop(self, child):
        # Concrete Popen owns the Windows process handle; never act on a reused PID.
        try:
            if child.poll() is None:child.terminate()
        except BaseException:pass
        try:
            child.wait(timeout=min(0.25, max(0, self.end - time.monotonic())))
        except BaseException:
            try:child.kill()
            except BaseException:pass
            try:child.wait(timeout=max(0, self.end - time.monotonic()))
            except BaseException:self.joined = False
        try:
            if child.poll() is None:self.joined = False
        except BaseException:self.joined = False
        try:child.stdout.close()
        except BaseException:pass

    def call(self, command, env, seconds):
        deadline = min(self.work_end, time.monotonic() + seconds)
        if deadline <= time.monotonic():raise TimeoutError()
        child = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.children.append(child)  # Hold immediately, before all fallible work.
        try:
            record = self.identity(child.pid)
            if record is not None:self.register(record)
            if os.name != 'nt':os.set_blocking(child.stdout.fileno(), False)
            received = bytearray()
            while True:
                if time.monotonic() >= deadline:raise TimeoutError()
                try:block = pipe_bytes(child, min(4096, OUTPUT_LIMIT + 1 - len(received)))
                except BlockingIOError:block = None
                if block:
                    received.extend(block)
                    if len(received) > OUTPUT_LIMIT:raise files.BudgetExceeded()
                elif block == b'' and child.poll() is not None:break
                else:time.sleep(0.005)
            if child.wait(timeout=max(0, deadline - time.monotonic())) != 0:raise OSError()
            return json.loads(received.decode('utf-8-sig'), object_pairs_hook=files.unique_json)
        finally:
            self.stop(child)
            if not self.joined:raise RuntimeError()

    def close(self):
        for child in self.children:self.stop(child)


def query(runner, owned):
    env = {k: v for k, v in os.environ.items() if k.casefold() != 'psmodulepath'}
    env['RDPILOT_FOOTPRINT_QUERY'] = json.dumps({k: owned[k] for k in ('sid', 'name', 'marker')})
    return runner.call(['powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand', base64.b64encode(PROFILE_QUERY.encode('utf-16le')).decode()], env, 5)


def observe(selected, code, source, acquisition, cache, owned, fresh, private, identity, register, needles):
    start = time.monotonic();expected = None;runner = None;result = empty(outcome='ineligible')
    try:
        if not eligible(selected, code):return result, True
        expected = source_identity(source, acquisition);result = empty(expected)
        if (fresh is not True or not isinstance(owned, dict)
                or not re.fullmatch(r'S-1-[0-9-]{1,180}', str(owned.get('sid', '')))
                or not re.fullmatch(r'rdp[a-f0-9]{14}', str(owned.get('name', '')))
                or not re.fullmatch(r'rdpilot-owned-[a-f0-9]{32}', str(owned.get('marker', '')))):raise ValueError()
        runner = Runner(identity, register)
        before = query(runner, owned);profile = profile_binding(before, owned)
        if profile is None:result['profile'] = 'absent';result['outcome'] = 'observed'
        else:
            config = json.dumps({'source': expected, 'profile': profile, 'cache': str(cache)}).encode()
            if len(config) > CONFIG_LIMIT:raise files.BudgetExceeded()
            handoff = Path(private)/'guest-footprint-config.json';handoff.write_bytes(config)
            env = dict(os.environ);env['RDPILOT_FOOTPRINT_CONFIG'] = str(handoff)
            candidate = runner.call([sys.executable, str(Path(__file__).resolve())], env, 15)
            # Every child is already joined before the post-profile query.
            if not runner.joined:raise RuntimeError()
            result = validate(candidate, expected)
        after = query(runner, owned);after_profile = profile_binding(after, owned)
        if (profile != after_profile or any(before[k] != after[k] for k in ('sid', 'name', 'marker', 'expected'))):raise ValueError()
        if result['profile'] != ('absent' if profile is None else 'present'):raise ValueError()
        result = validate(result, expected)
        encoded = json.dumps(result).encode()
        if any(n and n in encoded for n in needles):raise ValueError()
    except files.BudgetExceeded:result = empty(expected, 'budget')
    except BaseException:result = empty(expected)
    finally:
        if runner:runner.close()
    joined = not runner or runner.joined
    if not joined:result = empty(expected, 'cleanup_failed', False)
    result['elapsed_ms'] = min(30000, int((time.monotonic() - start) * 1000))
    return result, joined


def worker():
    try:
        path = Path(os.environ['RDPILOT_FOOTPRINT_CONFIG'])
        with Held(path, CONFIG_LIMIT) as held:
            config = json.loads(Budget(CONFIG_LIMIT).read(held, CONFIG_LIMIT), object_pairs_hook=files.unique_json)
        if not isinstance(config, dict) or set(config) != {'source', 'profile', 'cache'}:raise ValueError()
        result = snapshot(config)
        encoded = json.dumps(validate(result, config['source'])).encode()
        if len(encoded) > OUTPUT_LIMIT:raise ValueError()
        sys.stdout.buffer.write(encoded);return 0
    except BaseException:return 1


if __name__ == '__main__':raise SystemExit(worker())
