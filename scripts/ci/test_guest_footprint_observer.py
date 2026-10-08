"""Actual file/held child controls. POSIX tests do not establish Windows capability."""
import argparse
import contextlib
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import guest_footprint_observer as observer
import test_live_proof as existing

SECRET = 'private-credential-footprint-control'
BRIDGE = b'candidate bridge'
CUA = b'candidate cua'
OWNED = {'name': 'rdp' + 'a' * 14, 'sid': 'S-1-5-21-123', 'marker': 'rdpilot-owned-' + 'b' * 32}


def sha(value):return hashlib.sha256(value).hexdigest()


def identity():
    source = {'source_commit': '1' * 40, 'source_tree': '2' * 40, 'bridge_sha256': sha(BRIDGE)}
    acquisition = {'state': 'verified_source_bundle', 'bundle_count': 1, 'bridge_sha256': sha(BRIDGE),
                   'archive_sha256': sha(b'archive'), 'cua_sha256': sha(CUA), 'cua_version': '0.34.0'}
    acquisition['bundle_id'] = 'cua-driver-rs-v0.34.0-' + sha((acquisition['bridge_sha256'] + acquisition['archive_sha256']).encode())[:16]
    return source, acquisition, observer.source_identity(source, acquisition)


def failure():
    observation = {'cli_error_code': 'internal', 'producer': 'sdk_bootstrap', 'connection_stage': 'unavailable',
                   'password_helper': 'unavailable', 'bootstrap_state': 'observed',
                   'bootstrap_stages': ['launch_input_attempted', 'launch_input_sent'], 'observation_state': 'observed',
                   'daemon': {'state': 'alive'}, 'relays': {}}
    for target, count in (('a', 1), ('b', 0)):
        observation['relays'][target] = {'state': 'observed', 'accepted': count, 'upstream_connected': count,
            'to_client_bytes': count, 'to_upstream_bytes': count, 'upstream_failure': 'none', 'copy_failure': 'none'}
    return {'proof': 'cua', 'mode': 'live', 'status': 'failed', 'failure_stage': 'harness',
            'completed_checks': sorted(observer.INITIAL_CHECKS), 'identity': [],
            'harness_failure': {'operation': 'connect_cli', 'exception_category': 'proof_assertion', 'cli_exit_code': 1,
                                'connect_observation': observation}}


def reader_command():
    # Qualified host-only test harness: exercise the actual low-level snapshot in
    # a real child on temporary files. Strict production worker admission is
    # exercised separately; no production flag or path rule is relaxed.
    return [sys.executable, '-c', "import json,os,runpy,sys; from pathlib import Path; sys.path.insert(0,str(Path(os.environ['RDPILOT_TEST_OBSERVER']).parent)); m=runpy.run_path(os.environ['RDPILOT_TEST_OBSERVER']); c=json.load(open(os.environ['RDPILOT_FOOTPRINT_CONFIG'])); print(json.dumps(m['snapshot'](c)))"]


def native_profile(path=None):
    expected = 'C:\\Users\\' + OWNED['name']
    return {**OWNED, 'expected': expected, 'profile': None if path is None else {
        'sid': OWNED['sid'], 'path': expected, 'special': False, 'loaded': True}}


class Fixture:
    def __init__(self, directory):
        self.path = Path(directory);self.profile = self.path/'owned-profile';self.profile.mkdir()
        self.cache = self.path/'cache';self.cache.mkdir()
        self.source, self.acquisition, self.expected = identity()
        self.manifest = {key: self.expected[key] for key in ('bundle_id', 'cua_version', 'bridge_sha256', 'archive_sha256')}
        self.manifest.update(archive_name=observer.files.ARCHIVE, files={'cua-driver.exe': sha(CUA), 'asset.dll': sha(b'asset')})
        self.local = self.cache/'bundles'/self.expected['bundle_id'];self.local.mkdir(parents=True)
        (self.local/'manifest.json').write_text(json.dumps(self.manifest))
        self.root = self.profile/'AppData'/'Local'/'rdpilot'
        self.config = {'source': self.expected, 'profile': str(self.profile), 'cache': str(self.cache), 'owned': dict(OWNED)}

    def product(self):self.root.mkdir(parents=True, exist_ok=True)
    def launcher(self, name='l0000000000001.exe', data=BRIDGE):
        self.product();path = self.root/name;path.write_bytes(data);return path
    def bundle(self):
        self.product();path = self.root/self.expected['bundle_id'];path.mkdir(exist_ok=True)
        (path/'manifest.json').write_text(json.dumps(self.manifest))
        (path/'rdpilot-bridge.exe').write_bytes(BRIDGE);(path/'cua-driver.exe').write_bytes(CUA)
        return path
    def sample(self):return observer.validate(observer.snapshot(self.config), self.expected)


class SelectionTests(unittest.TestCase):
    def test_only_closed_original_first_a_failure_is_eligible(self):
        selected = failure();self.assertTrue(observer.eligible(selected, 1))
        variants = []
        for key, value in (('status', 'passed'), ('proof', 'viewer'), ('identity', [{}]),
                           ('completed_checks', ['a.rdp_bundle_ready']), ('completed_checks', [True]),
                           ('completed_checks', list(observer.INITIAL_CHECKS) * 2)):
            changed = copy.deepcopy(selected);changed[key] = value;variants.append(changed)
        for key, value in (('operation', 'cleanup'), ('cli_exit_code', True), ('private', SECRET)):
            changed = copy.deepcopy(selected);changed['harness_failure'][key] = value;variants.append(changed)
        for key, value in (('producer', 'sdk_connection'), ('bootstrap_stages', [SECRET]), ('relays', {}), ('daemon', {})):
            changed = copy.deepcopy(selected);changed['harness_failure']['connect_observation'][key] = value;variants.append(changed)
        for key in ('accepted', 'upstream_connected', 'to_client_bytes', 'to_upstream_bytes', 'copy_failure'):
            changed = copy.deepcopy(selected)
            changed['harness_failure']['connect_observation']['relays']['b'][key] = 1 if key != 'copy_failure' else 'unknown'
            variants.append(changed)
        for changed in variants:self.assertFalse(observer.eligible(changed, 1), changed)
        for code in (0, True, None):self.assertFalse(observer.eligible(selected, code))
        self.assertEqual(selected, failure())

    def test_source_hashes_recomputed_and_types_closed(self):
        source, acquisition, expected = identity()
        for key in acquisition:
            changed = dict(acquisition);changed[key] = SECRET
            with self.assertRaises(ValueError):observer.source_identity(source, changed)
        with self.assertRaises(ValueError):observer.source_identity({**source, 'bridge_sha256': 'f' * 64}, acquisition)
        self.assertEqual(observer.source_identity(source, acquisition), expected)

    def test_exact_projection_blocks_positives_and_arbitrary_text(self):
        _, _, source = identity();valid = observer.empty(source)
        for change in ({'private': SECRET}, {'launcher_count': True}, {'elapsed_ms': 30001},
                       {'source': None}, {'launcher': 'installed'}, {'candidate_bundle': 'source_binaries_match'},
                       {'product_directory': SECRET}, {'worker_joined': 1}):
            with self.assertRaises((ValueError, TypeError)):observer.validate({**valid, **change}, source)
        self.assertNotIn(SECRET, json.dumps(valid))

    def test_profile_binding_refuses_other_missing_special_and_ambiguous_rows(self):
        binding = native_profile(True)
        self.assertEqual(observer.profile_binding(binding, OWNED), binding['expected'])
        self.assertIsNone(observer.profile_binding(native_profile(), OWNED))
        changes = [{'sid': 'S-1-5-21-456'}, {'name': 'someone'}, {'marker': 'wrong'}, {'expected': '\\\\server\\share'}, {'profile': []}]
        for key, value in (('sid', 'S-1-5-21-456'), ('special', True), ('loaded', 1), ('path', 'C:\\Users\\someone')):
            changes.append({'profile': {**binding['profile'], key: value}})
        for change in changes:
            with self.assertRaises(ValueError):observer.profile_binding({**binding, **change}, OWNED)

    def test_names_are_canonical_and_private(self):
        self.assertTrue(observer.launcher_name('l0000000000000.exe'))
        for name in ('lzzzzzzzzzzzzz.exe', 'l000000000000A.exe', 'l1.exe', 'l0000000000001.exe.extra'):self.assertFalse(observer.launcher_name(name))
        self.assertTrue(observer.staging_name('.staging-0-1'))
        for name in ('.staging-01-1', '.staging-1-0', '.staging-18446744073709551616-1', '.staging-1-4294967296'):self.assertFalse(observer.staging_name(name))
        for path in ('\\\\?\\UNC\\server\\share', '\\\\?\\GLOBALROOT\\Device\\x', '\\\\?\\Volume{a}\\x', '\\\\?\\C:\\a\\..\\x'):
            with self.assertRaises((OSError, ValueError)):observer.final_local(path)
        self.assertEqual(observer.final_local('\\\\?\\C:\\ordinary\\file'), 'c:\\ordinary\\file')


class ActualFileTests(unittest.TestCase):
    def setUp(self):self.temp = tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.fixture = Fixture(self.temp.name)

    def test_absence_directory_launcher_and_source_binaries_independent(self):
        f = self.fixture
        self.assertEqual(f.sample()['product_directory'], 'absent')
        f.product();self.assertEqual(f.sample()['product_directory'], 'present')
        self.assertEqual(f.sample()['launcher'], 'absent')
        launcher = f.launcher();self.assertEqual(f.sample()['launcher'], 'source_match')
        bundle = f.bundle();result = f.sample();self.assertEqual(result['candidate_bundle'], 'source_binaries_match')
        # asset.dll is deliberately not read: this is not a complete installation.
        self.assertFalse((bundle/'asset.dll').exists());self.assertNotIn('installed', json.dumps(result))
        launcher.unlink();self.assertEqual(f.sample()['launcher'], 'absent')
        self.assertEqual(f.sample()['candidate_bundle'], 'source_binaries_match')
        self.assertNotIn(str(f.path), json.dumps(result));self.assertNotIn(launcher.name, json.dumps(result))

    def test_partial_mismatching_and_ambiguous_residual_files(self):
        f = self.fixture;bundle = f.bundle();(bundle/'cua-driver.exe').unlink()
        self.assertEqual(f.sample()['candidate_bundle'], 'partial')
        (bundle/'cua-driver.exe').write_bytes(b'wrong-' + SECRET.encode())
        self.assertEqual(f.sample()['candidate_bundle'], 'mismatch')
        f.launcher(data=b'wrong');self.assertEqual(f.sample()['launcher'], 'mismatch')
        f.launcher('l0000000000002.exe');self.assertEqual(f.sample()['launcher'], 'ambiguous')
        self.assertNotIn(SECRET, json.dumps(f.sample()))

    def test_shallow_scope_types_casefold_and_count_refusal(self):
        f = self.fixture;f.launcher();(f.root/'.staging-7-123').mkdir()
        (f.root/'.staging-7-123'/'never-read').write_bytes(SECRET.encode())
        self.assertEqual(f.sample()['staging_count'], 1)
        (f.root/'unknown').write_bytes(b'x');self.assertEqual(f.sample()['outcome'], 'invalid');(f.root/'unknown').unlink()
        for number in range(2, 6):f.launcher('l' + str(number).rjust(13, '0') + '.exe')
        self.assertEqual(f.sample()['outcome'], 'budget')

    def test_actual_hardlinks_refuse_content(self):
        f = self.fixture;path = f.launcher();os.link(path, f.path/'hardlink')
        self.assertEqual(f.sample()['outcome'], 'invalid')
        with self.assertRaises(ValueError):
            with observer.Held(path, 1024):pass

    def test_actual_file_replacement_and_write_race_invalidate_held_read(self):
        f = self.fixture;path = f.launcher()
        with self.assertRaises(ValueError):
            with observer.Held(path, 1024) as held:
                path.write_bytes(b'changed while held');held.file.read()
        path.write_bytes(BRIDGE)
        with self.assertRaises(ValueError):
            with observer.Held(path, 1024) as held:
                replacement = f.path/'replacement';replacement.write_bytes(BRIDGE)
                os.replace(replacement, path);held.file.read()

    def test_shared_actual_byte_counter_and_manifest_admission(self):
        f = self.fixture;path = f.launcher();budget = observer.Budget(len(BRIDGE))
        self.assertEqual(observer.read(path, 1024, budget), BRIDGE)
        with self.assertRaises(observer.files.BudgetExceeded):observer.read(path, 1024, budget)
        (f.local/'manifest.json').write_text('{"files":{},"files":{}}')
        self.assertEqual(f.sample()['outcome'], 'invalid')
        (f.local/'manifest.json').write_bytes(b'x' * (observer.MANIFEST_LIMIT + 1))
        self.assertEqual(f.sample()['outcome'], 'budget')

    def test_actual_byte_ceiling_zero_exact_and_growth_never_reads_past_cap(self):
        f = self.fixture;path = f.launcher();budget = observer.Budget(0)
        with observer.Held(path, 1024) as held:
            self.assertIsInstance(held.file, io.FileIO)
            with self.assertRaises(observer.files.BudgetExceeded):budget.read(held, 1024)
            self.assertEqual(held.file.tell(), 0);self.assertEqual(budget.remaining, 0)
            self.assertEqual(os.lseek(held.file.fileno(), 0, os.SEEK_CUR), 0)
        budget = observer.Budget(len(BRIDGE) + len(CUA));second = f.path/'second';second.write_bytes(CUA)
        with observer.Held(path, 1024) as held:
            self.assertEqual(budget.read(held, 1024), BRIDGE);self.assertEqual(held.file.tell(), len(BRIDGE))
            self.assertEqual(os.lseek(held.file.fileno(), 0, os.SEEK_CUR), len(BRIDGE))
        with observer.Held(second, 1024) as held:
            self.assertEqual(budget.read(held, 1024), CUA);self.assertEqual(held.file.tell(), len(CUA))
            self.assertEqual(os.lseek(held.file.fileno(), 0, os.SEEK_CUR), len(CUA))
        self.assertEqual(budget.remaining, 0)
        with observer.Held(second, 1024) as held:
            with self.assertRaises(observer.files.BudgetExceeded):budget.read(held, 1024)
            self.assertEqual(held.file.tell(), 0)
            self.assertEqual(os.lseek(held.file.fileno(), 0, os.SEEK_CUR), 0)
        # Reproduce the exact underlying-descriptor control: 15 held bytes,
        # six bytes appended, budget15. The kernel descriptor must consume15,
        # not a buffered prefetch of21; held postmetadata must still refuse it.
        path.write_bytes(b'123456789012345');budget = observer.Budget(15)
        with self.assertRaises(ValueError):
            with observer.Held(path, 1024) as held:
                self.assertIsInstance(held.file, io.FileIO)
                with path.open('ab') as writer:writer.write(b'abcdef')
                self.assertEqual(budget.read(held, 1024), b'123456789012345')
                self.assertEqual(held.file.tell(), 15);self.assertEqual(budget.remaining, 0)
                self.assertEqual(os.lseek(held.file.fileno(), 0, os.SEEK_CUR), 15)

    def test_actual_reparse_escape_and_casefold_controls(self):
        f = self.fixture;f.product()
        if os.name == 'nt':
            # A real directory junction does not need Developer Mode privileges.
            target = f.path/'outside';target.mkdir()
            linked = f.root/'.staging-1-1'
            result = subprocess.run(['cmd.exe', '/d', '/c', 'mklink', '/J', str(linked), str(target)], capture_output=True)
            self.assertEqual(result.returncode, 0, 'Native reparse test setup failed')
            try:self.assertEqual(f.sample()['outcome'], 'invalid')
            finally:os.rmdir(linked)
        else:
            (f.root/'.staging-1-1').symlink_to(f.path, target_is_directory=True)
            self.assertEqual(f.sample()['outcome'], 'invalid');(f.root/'.staging-1-1').unlink()
            f.launcher();(f.root/'L0000000000001.EXE').write_bytes(BRIDGE)
            self.assertEqual(f.sample()['outcome'], 'invalid')
        # The held reader always uses native admission on Windows; failures cannot
        # fall back to the portable CRT implementation.
        if os.name == 'nt':
            path = f.launcher()
            with patch.object(observer, 'WindowsFile', side_effect=OSError(SECRET)):
                self.assertEqual(f.sample()['outcome'], 'unavailable')
            with self.assertRaises((ValueError, OSError)), observer.Held(path, 1024) as held:
                self.assertIsNotNone(held.native)
                held.file.read()
                # Share-write and share-delete allow the product's concurrent
                # writer/rename. Admission then rejects that race, not the writer.
                path.write_bytes(BRIDGE)
                renamed = path.with_suffix('.renamed');os.replace(path, renamed)
                with self.assertRaises((ValueError, OSError)):held.check()
                os.replace(renamed, path)


class ActualChildTests(unittest.TestCase):
    def runner(self, identity=None, register=None, total=3):
        value = observer.Runner(identity or (lambda pid: {'pid': pid}), register or (lambda record: None), total)
        self.addCleanup(value.close);return value

    def call(self, runner, source, seconds=1):return runner.call([sys.executable, '-c', source], dict(os.environ), seconds)

    def test_real_output_cap_timeout_and_join(self):
        for source, error in (("import os,time;os.write(1,b'x'*20000);time.sleep(60)", observer.files.BudgetExceeded),
                              ('import time;time.sleep(60)', TimeoutError),
                              ("print('not json')", ValueError)):
            with self.subTest(source=source):
                runner = self.runner()
                with self.assertRaises(error):self.call(runner, source, 0.2)
                self.assertTrue(runner.joined);self.assertTrue(all(child.poll() is not None for child in runner.children))
        runner = self.runner();self.assertEqual(self.call(runner, 'print(\'{"ok":true}\')'), {'ok': True})

    def test_concrete_child_held_before_journal_or_identity_failure(self):
        def fail(value):raise PermissionError(SECRET)
        for key in ('identity', 'register'):
            runner = self.runner(**{key: fail})
            with self.assertRaises(PermissionError):self.call(runner, 'import time;time.sleep(60)')
            self.assertEqual(len(runner.children), 1);self.assertIsNotNone(runner.children[0].poll());self.assertTrue(runner.joined)

    def test_first_termination_failure_still_kills_and_joins_real_child(self):
        runner = self.runner();spawn = observer.subprocess.Popen
        def wrapped(*args, **kwargs):
            child = spawn(*args, **kwargs)
            child.terminate = lambda: (_ for _ in ()).throw(PermissionError(SECRET))
            return child
        with patch.object(observer.subprocess, 'Popen', side_effect=wrapped):
            with self.assertRaises(TimeoutError):self.call(runner, 'import time;time.sleep(60)', 0.1)
        self.assertIsNotNone(runner.children[0].poll());self.assertTrue(runner.joined)

    def test_real_child_refused_wait_reports_unknown_join_without_positive(self):
        runner = self.runner(total=0.4);spawn = observer.subprocess.Popen;held = []
        def wrapped(*args, **kwargs):
            child = spawn(*args, **kwargs);held.append((child, child.wait))
            child.wait = lambda **kwargs: (_ for _ in ()).throw(TimeoutError())
            return child
        try:
            with patch.object(observer.subprocess, 'Popen', side_effect=wrapped):
                with self.assertRaises(RuntimeError):self.call(runner, 'import time;time.sleep(60)', 0.05)
            self.assertFalse(runner.joined)
        finally:
            for child, wait in held:
                child.kill();wait(timeout=2)

    def test_real_worker_config_and_manifest_handoff(self):
        with tempfile.TemporaryDirectory() as directory:
            f = Fixture(directory);f.launcher();f.bundle();config = f.path/'config.json';config.write_text(json.dumps(f.config))
            runner = self.runner();env = {**os.environ, 'RDPILOT_FOOTPRINT_CONFIG': str(config), 'RDPILOT_TEST_OBSERVER': observer.__file__}
            value = runner.call(reader_command(), env, 1)
            self.assertEqual(observer.validate(value, f.expected)['candidate_bundle'], 'source_binaries_match')
            self.assertNotIn(str(f.path), json.dumps(value));self.assertTrue(runner.joined)
            config.write_bytes(b'x' * (observer.CONFIG_LIMIT + 1))
            with self.assertRaises(OSError):runner.call([sys.executable, observer.__file__], env, 1)


class StrictWorkerAdmissionTests(unittest.TestCase):
    def test_actual_production_worker_refuses_source_owner_and_path_before_snapshot(self):
        with tempfile.TemporaryDirectory() as directory:
            f = Fixture(directory);f.launcher();f.bundle()
            profile = 'C:\\Users\\' + OWNED['name']
            local = 'C:\\Users\\runneradmin\\AppData\\Local';cache = local + '\\rdpilot\\cache'
            base = {'source': f.expected, 'owned': OWNED, 'profile': profile, 'cache': cache}
            anchors = {'source': f.expected, 'owned': OWNED, 'cache': cache}
            variants = []
            for key, value in (('private', SECRET), ('source_commit', SECRET), ('bridge_sha256', 1),
                               ('cua_version', '99.0.0'), ('bundle_id', '..'), ('cua_sha256', 'f' * 64)):
                variants.append({**base, 'source': {**f.expected, key: value}})
            for key, value in (('sid', SECRET), ('name', '../someone'), ('marker', False), ('private', SECRET)):
                variants.append({**base, 'owned': {**OWNED, key: value}})
            for value in (str(f.profile), 'C:\\Users\\someone', 'C:\\Users\\.\\' + OWNED['name'],
                          profile + '\\..', '\\\\server\\share', '\\\\?\\' + profile):
                variants.append({**base, 'profile': value})
            for value in (str(f.cache), cache + '\\..', local + '\\other\\cache'):
                variants.append({**base, 'cache': value})
            runner = observer.Runner(lambda pid: None, lambda record: None, total=15)
            self.addCleanup(runner.close)
            config = f.path/'strict-config.json'
            env = {**os.environ, 'SystemDrive': 'C:', 'LOCALAPPDATA': local,
                   'RDPILOT_FOOTPRINT_CONFIG': str(config), 'RDPILOT_FOOTPRINT_ANCHORS': json.dumps(anchors)}
            for variant in variants:
                config.write_text(json.dumps(variant))
                value = runner.call([sys.executable, observer.__file__], env, 2)
                self.assertEqual(value['outcome'], 'invalid');self.assertIsNone(value['source'])
                self.assertEqual(value['profile'], 'unavailable');self.assertEqual(value['launcher'], 'unavailable')
                self.assertNotIn(SECRET, json.dumps(value));self.assertTrue(runner.joined)
            # Actual process, strict entry and refusal are exercised above. This
            # structural admission control does not read a synthetic Windows path.
            with patch.dict(os.environ, env):
                self.assertEqual(observer.worker_config(base), base)
                altered = {**anchors, 'source': {**f.expected, 'source_tree': '3' * 40}}
                with patch.dict(os.environ, {'RDPILOT_FOOTPRINT_ANCHORS': json.dumps(altered)}):
                    with self.assertRaises(ValueError):observer.worker_config(base)
                with patch.object(observer, 'snapshot', side_effect=AssertionError('Content must not be read')) as snapshot:
                    config.write_text(json.dumps({**base, 'source': {**f.expected, 'private': SECRET}}))
                    with contextlib.redirect_stdout(io.TextIOWrapper(io.BytesIO(), encoding='utf-8')):
                        self.assertEqual(observer.worker(), 0)
                    snapshot.assert_not_called()
            self.assertIsNone(observer.empty({**f.expected, 'private': SECRET})['source'])


class ParentTests(unittest.TestCase):
    def setUp(self):self.temp = tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.f = Fixture(self.temp.name)

    def observe(self, selected=None, fresh=True, queries=None, worker=None, needles=()):
        f = self.f;self.events = []
        def native(runner, owned):
            self.events.append('query')
            return queries.pop(0) if queries else native_profile(True)
        real = observer.Runner.call
        def read(runner, command, env, seconds):
            self.events.append('worker')
            self.assertTrue(all(c.poll() is not None for c in runner.children))
            if worker is None:
                env = {**env, 'RDPILOT_TEST_OBSERVER': observer.__file__}
                return real(runner, reader_command(), env, seconds)
            return worker(runner)
        # Qualified fixture on every platform: substitute the native profile
        # binding and invoke the low-level reader in a real child. This fixture
        # establishes neither production worker admission nor Windows profile
        # ownership. Separate strict-entry and native held-file controls apply.
        with patch.object(observer, 'query', side_effect=native), patch.object(observer, 'profile_binding', side_effect=lambda value, owned: str(f.profile) if value.get('profile') else None), patch.object(observer.Runner, 'call', read):
            return observer.observe(selected or failure(), 1, f.source, f.acquisition, f.cache, OWNED, fresh, f.path,
                                    (lambda pid: None), (lambda record: None), needles)

    def test_real_worker_prepost_binding_and_no_positive_on_change(self):
        self.f.launcher();self.f.bundle()
        result, joined = self.observe();self.assertTrue(joined);self.assertEqual(result['launcher'], 'source_match')
        self.assertEqual(self.events, ['query', 'worker', 'query'])
        before = native_profile(True);after = native_profile(True);after['marker'] = SECRET
        result, joined = self.observe(queries=[before, after]);self.assertEqual(result['outcome'], 'unavailable');self.assertTrue(joined)
        self.assertEqual(result['launcher'], 'unavailable');self.assertNotIn(SECRET, json.dumps(result))

    def test_absent_profile_and_failed_freshness_do_not_read_worker(self):
        value, joined = self.observe(queries=[native_profile(), native_profile()]);self.assertEqual(value['profile'], 'absent')
        self.assertEqual(self.events, ['query', 'query']);self.assertTrue(joined)
        result, joined = self.observe(fresh=False);self.assertEqual(self.events, []);self.assertEqual(result['outcome'], 'unavailable')
        changed = failure();changed['completed_checks'].append('a.rdp_bundle_ready')
        result, joined = self.observe(selected=changed);self.assertEqual(self.events, []);self.assertEqual(result['outcome'], 'ineligible')

    def test_actual_parent_refuses_every_contradictory_worker_state_and_join(self):
        self.f.launcher();self.f.bundle();valid = observer.snapshot(self.f.config)
        variants = [
            {'profile': 'unavailable'}, {'product_directory': 'absent'}, {'launcher_count': 0},
            {'launcher': 'absent', 'launcher_count': 1}, {'launcher': 'ambiguous', 'launcher_count': 1},
            {'outcome': 'cleanup_failed'}, {'outcome': 'unavailable'}, {'worker_joined': False},
            {'profile': 'absent', 'product_directory': 'absent', 'launcher': 'absent',
             'candidate_bundle': 'absent', 'launcher_count': 0},
        ]
        for changes in variants:
            with self.subTest(changes=changes):
                result, joined = self.observe(worker=lambda runner: {**valid, **changes})
                self.assertEqual(result['outcome'], 'unavailable');self.assertTrue(joined)
                self.assertTrue(result['worker_joined']);self.assertEqual(result['product_directory'], 'unavailable')
                self.assertEqual(result['launcher_count'], 0)
        self.assertEqual(self.observe(worker=lambda runner: valid)[0]['candidate_bundle'], 'source_binaries_match')
        # Partial bundles and an independent missing launcher remain meaningful.
        partial = {**valid, 'launcher': 'absent', 'launcher_count': 0, 'candidate_bundle': 'partial'}
        self.assertEqual(self.observe(worker=lambda runner: partial)[0]['candidate_bundle'], 'partial')

    def test_native_query_refusal_and_invalid_source_admit_no_content(self):
        f = self.f
        with patch.object(observer, 'query', side_effect=PermissionError(SECRET)) as native:
            value, joined = observer.observe(failure(), 1, f.source, f.acquisition, f.cache, OWNED, True, f.path,
                lambda pid: None, lambda record: None, ())
            self.assertEqual(value['outcome'], 'unavailable');self.assertTrue(joined);self.assertEqual(native.call_count, 1)
        with patch.object(observer, 'query') as native:
            value, joined = observer.observe(failure(), 1, f.source, {'state': 'invalid'}, f.cache, OWNED, True, f.path,
                lambda pid: None, lambda record: None, ())
            self.assertEqual(value['outcome'], 'unavailable');self.assertTrue(joined);native.assert_not_called()
        self.assertNotIn(SECRET, json.dumps(value))

    def test_parent_closed_validation_credentials_and_unknown_join(self):
        self.f.launcher();self.f.bundle()
        value, joined = self.observe(needles=[self.f.expected['bundle_id'].encode()]);self.assertEqual(value['outcome'], 'unavailable')
        value, joined = self.observe(worker=lambda runner: {**observer.empty(self.f.expected), 'private': SECRET})
        self.assertEqual(value['outcome'], 'unavailable');self.assertNotIn(SECRET, json.dumps(value))
        def refused(runner):runner.joined = False;return observer.snapshot(self.f.config)
        value, joined = self.observe(worker=refused);self.assertFalse(joined);self.assertEqual(value['outcome'], 'cleanup_failed')
        self.assertEqual(value['launcher'], 'unavailable');self.assertEqual(self.events, ['query', 'worker'])


class HostSeamTests(unittest.TestCase):
    def measure(self, refusal=None):
        host = existing.host;fixture = existing.OrchestrationModeTests()
        events = [];selected = failure();original_namespace = argparse.Namespace
        real_write = Path.write_text
        original_cleanup = host.cleanup_suite
        def namespace(**kwargs):return original_namespace(**kwargs, bootstrap_footprint=True)
        def cleanup(*args, **kwargs):
            events.append('cleanup');return original_cleanup(*args, **kwargs)
        def write(path, content, *args, **kwargs):
            if path.name == 'cua.json':
                events.append('selected_write')
                content = json.dumps(selected)
            if path.name == 'guest-footprint.json':
                events.append('artifact')
                if refusal == 'artifact':raise PermissionError(SECRET)
            return real_write(path, content, *args, **kwargs)
        def observe(actual_selected, *args):
            self.assertEqual(actual_selected, selected);events.append('observer')
            self.assertNotIn('cleanup', events)
            if refusal == 'exception':raise PermissionError(SECRET)
            value = observer.empty(outcome='cleanup_failed' if refusal == 'join' else 'unavailable', joined=refusal != 'join')
            return value, refusal != 'join'
        # Use the actual host orchestration fixture. It supplies only native host
        # controls/wrapper completion; the full owned cleanup/finalizer still runs.
        with patch.object(existing.argparse, 'Namespace', side_effect=namespace), patch.object(host, 'cleanup_suite', side_effect=cleanup), patch.object(host.footprint, 'observe', side_effect=observe), patch.object(Path, 'write_text', new=write):
            code, gate, cleaned, commands, actions = fixture.measure('cua_diagnostic', failure='harness')
        self.assertEqual(code, 1);self.assertEqual(gate['status'], 'failed')
        self.assertEqual(selected, failure());self.assertEqual(len(commands), 1)
        self.assertEqual(events.count('selected_write'), 1)
        self.assertNotIn('--bootstrap-desktop', commands[0])
        self.assertLess(events.index('observer'), events.index('cleanup'))
        for key in ('user_0_account', 'user_1_account', 'native_cache_removed'):
            self.assertTrue(cleaned['cua'][key])
        for key in ('registry_0', 'registry_1', 'firewall', 'service'):
            self.assertTrue(cleaned['host'][key])
        self.assertTrue(cleaned['private_removed'])
        self.assertNotIn(SECRET, json.dumps(gate))
        return gate, cleaned

    def test_observer_refusals_and_write_failure_preserve_all_owned_cleanup(self):
        for refusal in (None, 'exception', 'artifact', 'join'):
            with self.subTest(refusal=refusal):
                gate, cleanup = self.measure(refusal)
                if refusal in ('join', 'exception'):
                    self.assertIn('guest_footprint_cleanup', gate['observation_failures'])
                    self.assertFalse(cleanup['guest_footprint']['worker_joined'])
                if refusal == 'artifact':self.assertIn('guest_footprint_artifact_write', gate['observation_failures'])
                if refusal == 'exception':self.assertIn('guest_footprint_observation', gate['observation_failures'])

    def test_actual_cli_invalid_observer_combinations_allocate_no_output(self):
        for flags in (['--bootstrap-footprint'], ['--setup-diagnostic', '--bootstrap-footprint'],
                      ['--cua-diagnostic', '--bootstrap-footprint', '--bootstrap-desktop']):
            with tempfile.TemporaryDirectory() as directory:
                output = Path(directory)/'never-created'
                child = subprocess.run([sys.executable, existing.host.__file__, '--initialize', '--output', str(output), *flags], capture_output=True)
                self.assertNotEqual(child.returncode, 0);self.assertFalse(output.exists())
