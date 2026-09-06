#!/usr/bin/env python3
"""Offline cleanup classification tests for the DevTest lease runner."""
from __future__ import annotations

import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from contextlib import redirect_stderr, redirect_stdout

RUNNER_PATH = Path(__file__).with_name('devtest-rdp-e2e.py')
SPEC = importlib.util.spec_from_file_location('devtest_rdp_e2e', RUNNER_PATH)
assert SPEC and SPEC.loader
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


def result(code: int = 0, stderr: str = '', stdout: str = '') -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(['az'], code, stdout, stderr)


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


class BackingVmVisibilityTests(unittest.TestCase):
    compute_id = '/subscriptions/s/resourceGroups/rg/providers/Microsoft.Compute/virtualMachines/owned'

    def vm_record(self) -> str:
        return '{"networkProfile":{"networkInterfaces":[{"id":"nic","primary":true}]},"storageProfile":{"osDisk":{"managedDisk":{"id":"disk"}},"dataDisks":[]}}'

    def nic_record(self) -> str:
        return '{"ipConfigurations":[{"primary":true,"subnet":{"id":"subnet"},"privateIPAddress":"10.0.0.4"}]}'

    def test_explicit_404_retries_exact_compute_vm_then_uses_strict_parser(self) -> None:
        responses = [result(1, 'ResourceNotFound'), result(stdout=self.vm_record()), result(stdout=self.nic_record())]
        calls = []

        def az_responder(*args, **_kwargs):
            calls.append(args)
            return responses.pop(0)

        with patch.object(RUNNER, 'az', side_effect=az_responder), \
             patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0, 0, 1, 1)), \
             patch.object(RUNNER.time, 'sleep') as sleep:
            resources, subnet_id, private_ip = RUNNER.resource_ids(self.compute_id, 15 * 60)

        self.assertEqual(resources, [self.compute_id, 'nic', 'disk'])
        self.assertEqual((subnet_id, private_ip), ('subnet', '10.0.0.4'))
        self.assertEqual(calls[:2], [('vm', 'show', '--ids', self.compute_id, '--output', 'json'),
                                     ('vm', 'show', '--ids', self.compute_id, '--output', 'json')])
        sleep.assert_called_once_with(RUNNER.BACKING_VM_VISIBILITY_POLL_INTERVAL)

    def test_404_retry_stops_at_named_deadline(self) -> None:
        with patch.object(RUNNER, 'az', return_value=result(1, 'HTTP 404')), \
             patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0, 120)), \
             patch.object(RUNNER.time, 'sleep') as sleep, \
             self.assertRaisesRegex(RUNNER.LeaseError, 'devtest-backing-vm-not-ready'):
            RUNNER.resource_ids(self.compute_id, 15 * 60)
        sleep.assert_not_called()

    def test_authorization_failure_is_not_retried(self) -> None:
        with patch.object(RUNNER, 'az', return_value=result(1, 'AuthorizationFailed')) as az_mock, \
             patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0)), \
             patch.object(RUNNER.time, 'sleep') as sleep, \
             self.assertRaisesRegex(RUNNER.LeaseError, 'azure-command-failed:vm'):
            RUNNER.resource_ids(self.compute_id, 15 * 60)
        az_mock.assert_called_once()
        sleep.assert_not_called()

    def test_transport_failure_is_not_retried(self) -> None:
        with patch.object(RUNNER, 'az', return_value=result(1, 'connection reset')) as az_mock, \
             patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0)), \
             patch.object(RUNNER.time, 'sleep') as sleep, \
             self.assertRaisesRegex(RUNNER.LeaseError, 'azure-command-failed:vm'):
            RUNNER.resource_ids(self.compute_id, 15 * 60)
        az_mock.assert_called_once()
        sleep.assert_not_called()

    def test_malformed_vm_output_is_not_retried(self) -> None:
        with patch.object(RUNNER, 'az', return_value=result(stdout='{')) as az_mock, \
             patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0)), \
             self.assertRaisesRegex(RUNNER.LeaseError, 'devtest-backing-resource-invalid'):
            RUNNER.resource_ids(self.compute_id, 15 * 60)
        az_mock.assert_called_once()

    def test_non404_failure_classification_is_fixed_and_redacted(self) -> None:
        cases = (
            (result(1, 'AuthorizationFailed mocked-secret'), 'authorization'),
            (result(1, 'connection reset from host.internal'), 'transport'),
            (result(124, 'azure-command-timeout private-password'), 'timeout'),
            (result(1, 'ServiceUnavailable /subscriptions/mock'), 'other'),
        )
        for completed, expected in cases:
            self.assertEqual(RUNNER.classify_backing_vm_failure(completed), expected)

    def test_non404_failure_retains_generic_reason_and_fixed_class(self) -> None:
        with patch.object(RUNNER, 'az', return_value=result(1, 'AuthorizationFailed secret-user')) as az_mock, \
             patch.object(RUNNER.time, 'monotonic', side_effect=(0, 0)), \
             patch.object(RUNNER.time, 'sleep') as sleep, \
             self.assertRaises(RUNNER.BackingVmReadError) as raised:
            RUNNER.resource_ids(self.compute_id, 15 * 60)
        self.assertEqual(str(raised.exception), 'azure-command-failed:vm')
        self.assertEqual(raised.exception.failure_class, 'authorization')
        az_mock.assert_called_once()
        sleep.assert_not_called()


class PhaseDiagnosticsTests(unittest.TestCase):
    protected_values = ('/subscriptions/mock/resourceGroups/secret-rg', 'host.internal', 'secret-user', 'private-password')

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.state_path = Path(self.temporary.name) / 'lease.json'
        self.config = (
            '/subscriptions/s/resourceGroups/rg/providers/Microsoft.DevTestLab/labs/lab',
            '/subscriptions/s/resourceGroups/rg/providers/Microsoft.Network/networkSecurityGroups/nsg',
            'formula',
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def args(self):
        return argparse_namespace(self.state_path)

    def run_failure(self, phase: str, action, reason: str = 'safe-failure',
                    failure_class: str | None = None) -> tuple[int, str, str]:
        output, errors = io.StringIO(), io.StringIO()
        with patch.object(RUNNER, 'required_config', return_value=self.config), \
             patch.object(RUNNER, 'cleanup', return_value=0), \
             patch.object(RUNNER, 'rest', side_effect=action), \
             redirect_stdout(output), redirect_stderr(errors):
            status = RUNNER.run(self.args())
        event = json.loads(errors.getvalue())
        self.assertEqual(event['event'], 'failed')
        self.assertEqual(event['phase'], phase)
        self.assertEqual(event['reason'], reason)
        self.assertEqual(event.get('failure_class'), failure_class)
        self.assertEqual(errors.getvalue().count('"phase"'), 1)
        for value in self.protected_values:
            self.assertNotIn(value, output.getvalue() + errors.getvalue())
        return status, output.getvalue(), errors.getvalue()

    def test_provisioning_failure_has_one_fixed_phase_label(self) -> None:
        status, _output, _errors = self.run_failure('provisioning', lambda *_args, **_kwargs: (_ for _ in ()).throw(RUNNER.LeaseError('safe-failure')))
        self.assertEqual(status, 1)

    def test_resource_discovery_failure_has_one_fixed_phase_label(self) -> None:
        def action(*_args, **_kwargs):
            return {'properties': {'formulaContent': {'properties': {}}}}

        with patch.object(RUNNER, 'wait_vm', return_value={'properties': {'computeId': 'compute', 'fqdn': 'host.internal'}}), \
             patch.object(RUNNER, 'resource_ids', side_effect=RUNNER.LeaseError('safe-failure')):
            status, _output, _errors = self.run_failure('resource-discovery', action)
        self.assertEqual(status, 1)

    def test_backing_vm_class_is_emitted_only_with_resource_discovery_failure(self) -> None:
        def action(*_args, **_kwargs):
            return {'properties': {'formulaContent': {'properties': {}}}}

        with patch.object(RUNNER, 'wait_vm', return_value={'properties': {'computeId': 'compute', 'fqdn': 'host.internal'}}), \
             patch.object(RUNNER, 'resource_ids', side_effect=RUNNER.BackingVmReadError('transport')):
            status, _output, errors = self.run_failure(
                'resource-discovery', action, 'azure-command-failed:vm', 'transport')
        self.assertEqual(status, 1)
        self.assertNotIn('host.internal', errors)
        self.assertNotIn('secret-user', errors)
        self.assertNotIn('private-password', errors)

    def test_nsg_failure_has_one_fixed_phase_label(self) -> None:
        def action(*_args, **_kwargs):
            return {'properties': {'formulaContent': {'properties': {}}}}

        with patch.object(RUNNER, 'wait_vm', return_value={'properties': {'computeId': 'compute', 'fqdn': 'host.internal'}}), \
             patch.object(RUNNER, 'resource_ids', return_value=([], 'subnet', '10.0.0.4')), \
             patch.object(RUNNER, 'nsg_bound_to_subnet', return_value=False):
            status, _output, _errors = self.run_failure(
                'nsg', action, 'configured-nsg-not-bound-to-lease-subnet')
        self.assertEqual(status, 1)

    def test_rdp_failure_has_one_fixed_phase_label(self) -> None:
        def action(*_args, **_kwargs):
            return {'properties': {'formulaContent': {'properties': {}}}}

        with patch.object(RUNNER, 'wait_vm', return_value={'properties': {'computeId': 'compute', 'fqdn': 'host.internal'}}), \
             patch.object(RUNNER, 'resource_ids', return_value=([], 'subnet', '10.0.0.4')), \
             patch.object(RUNNER, 'nsg_bound_to_subnet', return_value=True), \
             patch.object(RUNNER, 'source_cidr', return_value='198.51.100.1/32'), \
             patch.object(RUNNER.subprocess, 'run', side_effect=RUNNER.LeaseError('safe-failure')):
            status, _output, _errors = self.run_failure('rdp', action)
        self.assertEqual(status, 1)

    def test_cleanup_failure_has_only_deletion_phase_marker(self) -> None:
        state = {
            'lease_id': '00000000-0000-0000-0000-000000000001',
            'vm_name': 'lease-vm',
            'lab_id': '/subscriptions/s/resourceGroups/rg/providers/Microsoft.DevTestLab/labs/lab',
            'resources': [],
        }
        RUNNER.save_state(self.state_path, state)
        output = io.StringIO()
        with patch.object(RUNNER, 'rest_result', return_value=result(1, 'AuthorizationFailed')), \
             redirect_stdout(output):
            self.assertEqual(RUNNER.cleanup(self.state_path, cleanup_timeout=1), 1)
        event = json.loads(output.getvalue())
        self.assertEqual(event['event'], 'cleanup-failed')
        self.assertEqual(event['phase'], 'deletion')
        self.assertEqual(output.getvalue().count('"phase"'), 1)
        for value in self.protected_values:
            self.assertNotIn(value, output.getvalue())


def argparse_namespace(state_path: Path):
    return type('Args', (), {
        'state': state_path,
        'lease_expiry': 60,
        'provision_timeout': 60,
        'cleanup_timeout': 60,
    })()


if __name__ == '__main__':
    unittest.main()
