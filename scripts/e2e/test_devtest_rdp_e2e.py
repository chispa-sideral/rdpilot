#!/usr/bin/env python3
"""Offline cleanup classification tests for the DevTest lease runner."""
from __future__ import annotations

import importlib.util
import io
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from contextlib import redirect_stdout

RUNNER_PATH = Path(__file__).with_name('devtest-rdp-e2e.py')
SPEC = importlib.util.spec_from_file_location('devtest_rdp_e2e', RUNNER_PATH)
assert SPEC and SPEC.loader
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


def result(code: int = 0, stderr: str = '') -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(['az'], code, '', stderr)


class CleanupTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.state_path = Path(self.temporary.name) / 'lease.json'

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def state(self, *, rule: bool = True, resources: tuple[str, ...] = ()) -> None:
        value = {
            'lease_id': '00000000-0000-0000-0000-000000000001',
            'vm_name': 'lease-vm',
            'lab_id': '/subscriptions/s/resourceGroups/rg/providers/Microsoft.DevTestLab/labs/lab',
            'resources': list(resources),
        }
        if rule:
            value.update({
                'nsg_id': '/subscriptions/s/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg',
                'ingress_rule': 'lease-rdp',
            })
        RUNNER.save_state(self.state_path, value)
        private = RUNNER.private_dir(self.state_path)
        private.mkdir(mode=0o700)
        (private / 'connection.json').write_text('{"password":"private"}', encoding='utf-8')

    def cleanup_with_rest(self, responder, *, rule: bool = True, resources: tuple[str, ...] = (), az_responder=None) -> int:
        self.state(rule=rule, resources=resources)
        with patch.object(RUNNER, 'rest_result', side_effect=responder), \
             patch.object(RUNNER, 'az', side_effect=az_responder or (lambda *_args, **_kwargs: result(1, 'ResourceNotFound'))), \
             redirect_stdout(io.StringIO()):
            return RUNNER.cleanup(self.state_path, cleanup_timeout=1)

    def test_rule_explicit_404_is_confirmed_absence(self) -> None:
        status = self.cleanup_with_rest(lambda *_args, **_kwargs: result(0) if _args[0] == 'DELETE' else result(1, 'ResourceNotFound'))
        self.assertEqual(status, 0)
        self.assertFalse(self.state_path.exists())

    def test_rule_403_fails_closed_preserves_state_and_removes_private_material(self) -> None:
        def responder(method, url, *_args, **_kwargs):
            if 'securityRules' in url and method == 'GET':
                return result(1, 'AuthorizationFailed')
            return result(0) if method == 'DELETE' else result(1, 'ResourceNotFound')

        status = self.cleanup_with_rest(responder)
        self.assertEqual(status, 1)
        self.assertTrue(self.state_path.exists())
        self.assertFalse(RUNNER.private_dir(self.state_path).exists())

    def test_rule_transport_failure_fails_closed_and_preserves_state(self) -> None:
        def responder(method, url, *_args, **_kwargs):
            if 'securityRules' in url and method == 'GET':
                return result(1, 'connection reset by peer')
            return result(0) if method == 'DELETE' else result(1, 'ResourceNotFound')

        self.assertEqual(self.cleanup_with_rest(responder), 1)
        self.assertTrue(self.state_path.exists())

    def test_backing_resource_explicit_404_is_confirmed_absence(self) -> None:
        resource_id = '/subscriptions/s/resourceGroups/rg/providers/Microsoft.Compute/virtualMachines/owned'
        def az_responder(*args, **_kwargs):
            return result(0) if args[1] == 'delete' else result(1, 'ResourceNotFound')

        status = self.cleanup_with_rest(
            lambda method, *_args, **_kwargs: result(0) if method == 'DELETE' else result(1, 'ResourceNotFound'),
            rule=False, resources=(resource_id,), az_responder=az_responder)
        self.assertEqual(status, 0)
        self.assertFalse(self.state_path.exists())

    def test_backing_resource_403_fails_closed_and_preserves_state(self) -> None:
        resource_id = '/subscriptions/s/resourceGroups/rg/providers/Microsoft.Compute/virtualMachines/owned'
        def az_responder(*args, **_kwargs):
            return result(0) if args[1] == 'delete' else result(1, 'AuthorizationFailed')

        status = self.cleanup_with_rest(
            lambda method, *_args, **_kwargs: result(0) if method == 'DELETE' else result(1, 'ResourceNotFound'),
            rule=False, resources=(resource_id,), az_responder=az_responder)
        self.assertEqual(status, 1)
        self.assertTrue(self.state_path.exists())

    def test_polling_stops_at_deadline(self) -> None:
        observed = []
        with patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0, 10)), \
             patch.object(RUNNER.time, 'sleep'):
            self.assertFalse(RUNNER.wait_for_absence(lambda: observed.append('present') or 'present', 5))
        self.assertEqual(observed, ['present'])


class RequiredConfigTests(unittest.TestCase):
    def setUp(self) -> None:
        self.values = {
            'AZURE_DEVTEST_LABS_ID': '/subscriptions/s/resourceGroups/rg/providers/Microsoft.DevTestLab/labs/lab',
            'RDPILOT_DEVTEST_NSG_ID': '/subscriptions/s/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg',
            'RDPILOT_DEVTEST_FORMULA': 'formula',
        }

    def test_valid_arm_resource_ids_are_accepted(self) -> None:
        with patch.dict(RUNNER.os.environ, self.values, clear=True):
            self.assertEqual(RUNNER.required_config(), tuple(self.values.values()))

    def test_malformed_lab_provider_is_rejected(self) -> None:
        values = {**self.values, 'AZURE_DEVTEST_LABS_ID': self.values['AZURE_DEVTEST_LABS_ID'].replace('Microsoft.DevTestLab', 'Microsoft-DevTestLab')}
        with patch.dict(RUNNER.os.environ, values, clear=True), self.assertRaisesRegex(RUNNER.LeaseError, 'devtest-lab-id-invalid'):
            RUNNER.required_config()

    def test_malformed_nsg_provider_is_rejected(self) -> None:
        values = {**self.values, 'RDPILOT_DEVTEST_NSG_ID': self.values['RDPILOT_DEVTEST_NSG_ID'].replace('Microsoft.Network', 'Microsoft-Network')}
        with patch.dict(RUNNER.os.environ, values, clear=True), self.assertRaisesRegex(RUNNER.LeaseError, 'devtest-nsg-id-invalid'):
            RUNNER.required_config()


if __name__ == '__main__':
    unittest.main()
