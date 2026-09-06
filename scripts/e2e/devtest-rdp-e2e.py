#!/usr/bin/env python3
"""Run and clean up one bounded GitHub Actions DevTest Labs RDP E2E lease.

The state file intentionally has no credentials. It is written before the
DevTest VM request so a cancelled job can run the exact same cleanup command.
"""
from __future__ import annotations

import argparse
import copy
import ipaddress
import json
import os
import re
import secrets
import signal
import stat
import subprocess
import sys
import time
import uuid
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

API_VERSION = '2018-09-15'
NETWORK_API_VERSION = '2024-05-01'
REQUIRED_ENV = ('AZURE_DEVTEST_LABS_ID', 'RDPILOT_DEVTEST_NSG_ID', 'RDPILOT_DEVTEST_FORMULA')
LAB_ID = re.compile(r'^/subscriptions/[^/]+/resourceGroups/[^/]+/providers/Microsoft\.DevTestLab/labs/[^/]+$', re.I)
NOT_FOUND = re.compile(r'(?:\bResourceNotFound\b|\bHTTP\s*404\b|\bStatus\s*Code:\s*404\b)', re.I)
BACKING_VM_VISIBILITY_TIMEOUT = 120
BACKING_VM_VISIBILITY_POLL_INTERVAL = 5
PHASES = frozenset(('provisioning', 'resource-discovery', 'nsg', 'rdp', 'deletion'))
BACKING_VM_FAILURE_CLASSES = frozenset(('authorization', 'transport', 'timeout', 'other'))
AUTHORIZATION_FAILURE = re.compile(r'\b(?:authorization\w*|forbidden|unauthorized)\b', re.I)
TIMEOUT_FAILURE = re.compile(r'\b(?:timeout|timed out|deadline exceeded)\b', re.I)
TRANSPORT_FAILURE = re.compile(r'\b(?:connection|connect|network|dns|socket|tls|proxy|reset|unreachable)\b', re.I)


class LeaseError(RuntimeError):
    """A deliberately non-secret operational failure."""


class BackingVmReadError(LeaseError):
    """A fail-closed backing-VM read error with a fixed redacted class."""

    def __init__(self, failure_class: str) -> None:
        if failure_class not in BACKING_VM_FAILURE_CLASSES:
            raise ValueError('invalid backing VM failure class')
        super().__init__('azure-command-failed:vm')
        self.failure_class = failure_class


def fail(message: str) -> None:
    raise LeaseError(message)


def emit_phase_failure(lease_id: str, phase: str, reason: str,
                       failure_class: str | None = None) -> None:
    """Emit one fixed, redacted phase label with an existing safe failure reason."""
    if phase not in PHASES:
        raise AssertionError('invalid phase')
    if failure_class is not None and failure_class not in BACKING_VM_FAILURE_CLASSES:
        raise AssertionError('invalid backing VM failure class')
    event: dict[str, str] = {'event': 'failed', 'lease_id': lease_id, 'phase': phase, 'reason': reason}
    if failure_class is not None:
        event['failure_class'] = failure_class
    print(json.dumps(event), file=sys.stderr)


def duration(value: str) -> int:
    match = re.fullmatch(r'([1-9][0-9]*)(s|m|h)', value)
    if not match:
        raise argparse.ArgumentTypeError('use a positive duration such as 30s, 20m, or 1h')
    return int(match.group(1)) * {'s': 1, 'm': 60, 'h': 3600}[match.group(2)]


def az(*args: str, input_file: Path | None = None, check: bool = True,
       timeout: float | None = None) -> subprocess.CompletedProcess[str]:
    command = ['az', *args, '--only-show-errors']
    if input_file is not None:
        command.extend(['--body', f'@{input_file}'])
    try:
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=timeout)
    except subprocess.TimeoutExpired:
        result = subprocess.CompletedProcess(command, 124, '', 'azure-command-timeout')
    if check and result.returncode:
        # Do not echo Azure's response: it may contain deployment context.
        fail(f'azure-command-failed:{args[0] if args else "unknown"}')
    return result


def rest_result(method: str, url: str, body: Path | None = None,
                timeout: float | None = None) -> subprocess.CompletedProcess[str]:
    """Return an Azure REST result without conflating failure with absence."""
    return az('rest', '--method', method, '--url', url, '--output', 'json', input_file=body,
              check=False, timeout=timeout)


def is_confirmed_not_found(result: subprocess.CompletedProcess[str]) -> bool:
    """Only an explicit Azure 404 is safe to treat as an absent owned resource."""
    return result.returncode != 0 and bool(NOT_FOUND.search(result.stderr))


def classify_backing_vm_failure(result: subprocess.CompletedProcess[str]) -> str:
    """Classify only a non-404 exact Compute VM read without retaining Azure output."""
    if result.returncode == 124 or TIMEOUT_FAILURE.search(result.stderr):
        return 'timeout'
    if AUTHORIZATION_FAILURE.search(result.stderr):
        return 'authorization'
    if TRANSPORT_FAILURE.search(result.stderr):
        return 'transport'
    return 'other'


def rest(method: str, url: str, body: Path | None = None) -> Any:
    result = rest_result(method, url, body)
    if result.returncode:
        fail(f'azure-command-failed:{method.lower()}')
    if not result.stdout.strip():
        return {}
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        fail('azure-json-invalid')
        raise AssertionError from exc


def owner_file(path: Path, content: str) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.with_name(f'.{path.name}.{secrets.token_hex(4)}.tmp')
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
        stream.write(content)
    temporary.replace(path)
    os.chmod(path, 0o600)


def load_state(path: Path) -> dict[str, Any]:
    if path.is_symlink() or not path.is_file() or stat.S_IMODE(path.stat().st_mode) != 0o600:
        fail('lease-state-must-be-owner-0600')
    try:
        state = json.loads(path.read_text(encoding='utf-8'))
    except (OSError, json.JSONDecodeError) as exc:
        fail('lease-state-invalid')
        raise AssertionError from exc
    if not isinstance(state.get('lease_id'), str) or not isinstance(state.get('vm_name'), str):
        fail('lease-state-invalid')
    return state


def save_state(path: Path, state: dict[str, Any]) -> None:
    owner_file(path, json.dumps(state, sort_keys=True) + '\n')


def private_dir(state_path: Path) -> Path:
    return state_path.with_name(f'{state_path.name}.private')


def remove_private(state_path: Path) -> None:
    directory = private_dir(state_path)
    if directory.is_symlink():
        fail('private-state-is-symlink')
    if not directory.exists():
        return
    for child in directory.iterdir():
        if child.is_file() and not child.is_symlink():
            child.unlink()
    directory.rmdir()


def required_config() -> tuple[str, str, str]:
    values = tuple(os.environ.get(name, '').strip() for name in REQUIRED_ENV)
    if not all(values):
        fail('devtest-configuration-missing')
    if not LAB_ID.fullmatch(values[0]):
        fail('devtest-lab-id-invalid')
    if not re.fullmatch(r'/subscriptions/[^/]+/resourceGroups/[^/]+/providers/Microsoft\.Network/networkSecurityGroups/[^/]+', values[1], re.I):
        # Require a resource ID rather than accepting a name that could resolve elsewhere.
        fail('devtest-nsg-id-invalid')
    return values


def source_cidr() -> str:
    for url in ('https://ifconfig.co/ip', 'https://api.ipify.org'):
        result = subprocess.run(['curl', '--fail', '--silent', '--show-error', '--ipv4', url],
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=15)
        if result.returncode == 0:
            try:
                address = ipaddress.IPv4Address(result.stdout.strip())
                if address.is_global:
                    return f'{address}/32'
            except ipaddress.AddressValueError:
                pass
    fail('source-ipv4-unavailable')
    raise AssertionError


def visible_backing_vm(compute_id: str, provision_timeout: int) -> subprocess.CompletedProcess[str]:
    """Read the exact owned backing VM after DevTest reports it ready.

    DevTest completion can precede Compute's first readable observation. Only an
    explicit not-found response is retried; every other Azure failure remains
    fail-closed.
    """
    deadline = time.monotonic() + min(BACKING_VM_VISIBILITY_TIMEOUT, provision_timeout)
    while True:
        vm = az('vm', 'show', '--ids', compute_id, '--output', 'json', check=False,
                timeout=max(1, deadline - time.monotonic()))
        if vm.returncode == 0:
            return vm
        if not is_confirmed_not_found(vm):
            raise BackingVmReadError(classify_backing_vm_failure(vm))
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            fail('devtest-backing-vm-not-ready')
        time.sleep(min(BACKING_VM_VISIBILITY_POLL_INTERVAL, remaining))


def resource_ids(compute_id: str, provision_timeout: int) -> tuple[list[str], str, str]:
    vm = visible_backing_vm(compute_id, provision_timeout)
    try:
        record = json.loads(vm.stdout)
        nics = record['networkProfile']['networkInterfaces']
        nic_id = next(item['id'] for item in nics if item.get('primary', len(nics) == 1))
        nic = json.loads(az('network', 'nic', 'show', '--ids', nic_id, '--output', 'json').stdout)
        ip_config = next(item for item in nic['ipConfigurations'] if item.get('primary', len(nic['ipConfigurations']) == 1))
        subnet_id = ip_config['subnet']['id']
        private_ip = ip_config['privateIPAddress']
        public_ids = [item['publicIPAddress']['id'] for item in nic.get('ipConfigurations', [])
                      if item.get('publicIPAddress', {}).get('id')]
        disks = [record.get('storageProfile', {}).get('osDisk', {}).get('managedDisk', {}).get('id')]
        disks.extend(item.get('managedDisk', {}).get('id') for item in record.get('storageProfile', {}).get('dataDisks', []))
    except (KeyError, StopIteration, TypeError, json.JSONDecodeError) as exc:
        fail('devtest-backing-resource-invalid')
        raise AssertionError from exc
    if not isinstance(private_ip, str):
        fail('devtest-private-ip-invalid')
    return [item for item in [compute_id, nic_id, *public_ids, *disks] if isinstance(item, str)], subnet_id, private_ip


def nsg_bound_to_subnet(nsg_id: str, subnet_id: str) -> bool:
    result = az('network', 'vnet', 'subnet', 'show', '--ids', subnet_id, '--output', 'json')
    try:
        subnet = json.loads(result.stdout)
        bound = subnet.get('networkSecurityGroup', {}).get('id', '')
    except json.JSONDecodeError:
        return False
    return isinstance(bound, str) and bound.rstrip('/').lower() == nsg_id.rstrip('/').lower()


def write_json(path: Path, value: dict[str, Any]) -> None:
    owner_file(path, json.dumps(value) + '\n')


def wait_vm(lab_id: str, vm_name: str, timeout: int) -> dict[str, Any]:
    url = f'https://management.azure.com{lab_id}/virtualmachines/{vm_name}?api-version={API_VERSION}'
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        record = rest('GET', url)
        properties = record.get('properties', {})
        if properties.get('provisioningState') == 'Succeeded' and properties.get('computeId'):
            return record
        time.sleep(20)
    fail('devtest-provisioning-timeout')
    raise AssertionError


def delete_resource(resource_id: str, timeout: float) -> str:
    result = az('resource', 'delete', '--ids', resource_id, check=False, timeout=timeout)
    if result.returncode == 0 or is_confirmed_not_found(result):
        return 'requested'
    return 'failed'


def observe_resource(resource_id: str, timeout: float) -> str:
    result = az('resource', 'show', '--ids', resource_id, '--output', 'none', check=False,
                timeout=timeout)
    if result.returncode == 0:
        return 'present'
    if is_confirmed_not_found(result):
        return 'absent'
    return 'failed'


def observe_rest(url: str, timeout: float) -> str:
    result = rest_result('GET', url, timeout=timeout)
    if result.returncode == 0:
        return 'present'
    if is_confirmed_not_found(result):
        return 'absent'
    return 'failed'


def wait_for_absence(observe: Any, deadline: float) -> bool:
    """Poll one exact resource; unknown Azure results fail closed, never become absence."""
    while time.monotonic() < deadline:
        state = observe()
        if state == 'absent':
            return True
        if state != 'present':
            return False
        time.sleep(min(5, max(0, deadline - time.monotonic())))
    return False


def remaining_timeout(deadline: float) -> float:
    return max(1, deadline - time.monotonic())


def cleanup(state_path: Path, cleanup_timeout: int = 10 * 60) -> int:
    state = load_state(state_path)
    deadline = time.monotonic() + cleanup_timeout
    failures: list[str] = []
    nsg_id = state.get('nsg_id')
    rule = state.get('ingress_rule')
    if isinstance(nsg_id, str) and isinstance(rule, str):
        rule_url = f'https://management.azure.com{nsg_id}/securityRules/{rule}?api-version={NETWORK_API_VERSION}'
        deleted = rest_result('DELETE', rule_url, timeout=remaining_timeout(deadline))
        if deleted.returncode and not is_confirmed_not_found(deleted):
            failures.append('ingress-rule-delete-failed')
        elif not wait_for_absence(lambda: observe_rest(rule_url, remaining_timeout(deadline)), deadline):
            failures.append('ingress-cleanup-failed')

    lab_id = state.get('lab_id')
    vm_name = state.get('vm_name')
    if isinstance(lab_id, str) and isinstance(vm_name, str):
        vm_url = f'https://management.azure.com{lab_id}/virtualmachines/{vm_name}?api-version={API_VERSION}'
        deleted = rest_result('DELETE', vm_url, timeout=remaining_timeout(deadline))
        if deleted.returncode and not is_confirmed_not_found(deleted):
            failures.append('devtest-vm-delete-failed')
        elif not wait_for_absence(lambda: observe_rest(vm_url, remaining_timeout(deadline)), deadline):
            failures.append('devtest-vm-cleanup-failed')

    for resource_id in state.get('resources', []):
        if isinstance(resource_id, str) and delete_resource(resource_id, remaining_timeout(deadline)) == 'failed':
            failures.append('backing-resource-delete-failed')

    for resource_id in state.get('resources', []):
        if not isinstance(resource_id, str):
            continue
        if not wait_for_absence(lambda resource_id=resource_id: observe_resource(
                resource_id, remaining_timeout(deadline)), deadline):
            failures.append('backing-resource-remains')

    try:
        remove_private(state_path)
    except LeaseError:
        failures.append('private-state-remove-failed')

    if failures:
        print(json.dumps({
            'event': 'cleanup-failed',
            'lease_id': state['lease_id'],
            'phase': 'deletion',
            'failures': sorted(set(failures)),
        }))
        return 1
    state_path.unlink(missing_ok=True)
    print(json.dumps({'event': 'down', 'lease_id': state['lease_id']}))
    return 0


def run(args: argparse.Namespace) -> int:
    lab_id, nsg_id, formula_name = required_config()
    state_path = args.state.resolve()
    if state_path.exists():
        fail('lease-state-already-exists')
    lease_id, vm_name = str(uuid.uuid4()), f'rdp{secrets.token_hex(6)}'
    state = {'lease_id': lease_id, 'vm_name': vm_name, 'lab_id': lab_id, 'nsg_id': nsg_id, 'resources': []}
    # This checkpoint is deliberately before the VM request and is the signal/EXIT recovery contract.
    save_state(state_path, state)
    print(json.dumps({'event': 'lease', 'lease_id': lease_id}, sort_keys=True), flush=True)

    interrupted = False
    def on_signal(_signum: int, _frame: Any) -> None:
        nonlocal interrupted
        interrupted = True
        raise KeyboardInterrupt
    signal.signal(signal.SIGINT, on_signal)
    signal.signal(signal.SIGTERM, on_signal)

    result = 1
    phase = 'provisioning'
    try:
        formula = rest('GET', f'https://management.azure.com{lab_id}/formulas/{formula_name}?api-version={API_VERSION}')
        content = formula.get('properties', {}).get('formulaContent')
        if not isinstance(content, dict) or not isinstance(content.get('properties'), dict):
            fail('devtest-formula-invalid')
        private = private_dir(state_path)
        private.mkdir(mode=0o700)
        password = 'Aa1!' + secrets.token_urlsafe(24)
        payload = copy.deepcopy(content)
        payload['name'] = vm_name
        payload['properties'].update({
            'userName': f'dtl{lease_id.replace("-", "")[:8]}',
            'password': password,
            'expirationDate': (datetime.now(UTC) + timedelta(seconds=args.lease_expiry)).strftime('%Y-%m-%dT%H:%M:%SZ'),
            'tags': {'rdpilot-lease-id': lease_id},
        })
        body = private / 'create.json'
        write_json(body, payload)
        rest('PUT', f'https://management.azure.com{lab_id}/virtualmachines/{vm_name}?api-version={API_VERSION}', body)
        vm = wait_vm(lab_id, vm_name, args.provision_timeout)
        properties = vm.get('properties', {})
        compute_id = properties.get('computeId')
        host = properties.get('fqdn')
        if not isinstance(compute_id, str) or not isinstance(host, str):
            fail('devtest-connection-coordinates-missing')

        phase = 'resource-discovery'
        resources, subnet_id, private_ip = resource_ids(compute_id, args.provision_timeout)
        # Persist all backing resources before source-IP discovery or NSG mutation.
        state['resources'] = resources
        save_state(state_path, state)
        phase = 'nsg'
        if not nsg_bound_to_subnet(nsg_id, subnet_id):
            fail('configured-nsg-not-bound-to-lease-subnet')

        source = source_cidr()
        rule = f'rdpilot-{lease_id.replace("-", "")[:12]}-rdp'
        priority = 2000 + int(lease_id.replace('-', '')[:4], 16) % 900
        rule_body = private / 'rule.json'
        write_json(rule_body, {'properties': {
            'protocol': 'Tcp', 'sourceAddressPrefix': source, 'sourcePortRange': '*',
            'destinationAddressPrefix': private_ip, 'destinationPortRange': '3389',
            'access': 'Allow', 'priority': priority, 'direction': 'Inbound',
        }})
        state['ingress_rule'] = rule
        save_state(state_path, state)
        rest('PUT', f'https://management.azure.com{nsg_id}/securityRules/{rule}?api-version={NETWORK_API_VERSION}', rule_body)

        phase = 'rdp'
        connection = private / 'connection.json'
        write_json(connection, {'host': host, 'user': payload['properties']['userName'], 'password': password, 'rdpPort': 3389})
        command = ['bash', str(Path(__file__).with_name('run-rdp-e2e.sh')), str(connection)]
        completed = subprocess.run(command, cwd=Path(__file__).parents[2], env=os.environ.copy())
        result = completed.returncode
    except KeyboardInterrupt:
        print(json.dumps({'event': 'interrupted', 'lease_id': lease_id}))
        result = 130 if interrupted else 1
    except LeaseError as error:
        failure_class = error.failure_class if isinstance(error, BackingVmReadError) else None
        emit_phase_failure(lease_id, phase, str(error), failure_class)
        result = 1
    finally:
        cleanup_result = cleanup(state_path, args.cleanup_timeout)
        if cleanup_result:
            result = cleanup_result
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    run_parser = commands.add_parser('run', help='create, exercise, and release one DevTest lease')
    run_parser.add_argument('--state', type=Path, required=True, help='owner-only non-secret lease state path')
    run_parser.add_argument('--lease-expiry', type=duration, default=45 * 60)
    run_parser.add_argument('--provision-timeout', type=duration, default=15 * 60)
    run_parser.add_argument('--cleanup-timeout', type=duration, default=10 * 60)
    cleanup_parser = commands.add_parser('cleanup', help='release the exact lease recorded in state')
    cleanup_parser.add_argument('--state', type=Path, required=True)
    cleanup_parser.add_argument('--cleanup-timeout', type=duration, default=10 * 60)
    args = parser.parse_args()
    try:
        return run(args) if args.command == 'run' else cleanup(args.state, args.cleanup_timeout)
    except LeaseError as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
