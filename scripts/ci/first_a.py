"""Fixed protected FIRST-A experiment policy; never a full live gate."""
import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import time

import acquisition_observer as files
import bootstrap_desktop_observer as desktop

MODE='protected_first_a'
PASSED='baseline_first_a_qualified'
WORK_SECONDS=900
ATTACH_SECONDS=240
FINALIZE_SECONDS=60
HOST_CLEANUP_SECONDS=600
PARENT_SECONDS=2400
BASE_COMMIT='0471227fa778191d9f7f5755ef8883d8c5fcc6a0'
PRODUCT={'crates':'858eb0f05fd44887c378e3ced52d42e9e4be944c',
         'Cargo.toml':'22e6654802e48a14685161c5b184a07bbafbce90',
         'Cargo.lock':'0266868b5a68b9a821272c2c37dbdf5ece05e6af'}
# Frozen from the reviewed047 tree; no historical commit fetch is needed.
ALLOWLIST={'.github/workflows/hosted-desktop-probe.yml',
    'scripts/ci/first_a.py','scripts/ci/test_first_a.py','scripts/ci/held_process.py',
    'scripts/ci/hosted_desktop.py','scripts/ci/run-live-proof.py',
    'scripts/ci/test_hosted_lifecycle.py','scripts/ci/test_live_proof.py',
    'scripts/ci/acquisition_observer.py','scripts/e2e/run-cua-e2e.py','scripts/e2e/proof_support.py'}
PROTECTED_DIGEST='5e4afd8f337ac57cab51ccdf33c335dfed26474e89f7510d34a4763b45c0be28'
CHECKS=('private_child_temporary_boundary_verified','a.rdp_bundle_ready',
        'a.native_initialize_tools','a.installed_source_bridge_and_cua_identity')
IDENTITY_KEYS={'bridge_sha256','archive_sha256','cua_sha256','cua_version','bundle_id',
               'os_session_id','installed_under_localappdata'}


def verify_source(expected_commit):
    commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
    tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip()
    if commit!=expected_commit or len(commit)!=40 or len(tree)!=40:
        raise ValueError('Source identity')
    subprocess.run(['git','diff','--quiet','HEAD','--'],check=True)
    entries=subprocess.check_output(['git','ls-tree','-r','-z','HEAD']).split(b'\0')
    protected=b'\0'.join(entry for entry in entries if entry and entry.split(b'\t',1)[1].decode() not in ALLOWLIST)
    if hashlib.sha256(protected).hexdigest()!=PROTECTED_DIGEST:raise ValueError('Protected source changed')
    for path,blob in PRODUCT.items():
        if subprocess.check_output(['git','rev-parse','HEAD:'+path],text=True).strip()!=blob:
            raise ValueError('Product identity')
    return {'source_commit':commit,'source_tree':tree,'reviewed_base':BASE_COMMIT,
            'product_objects':PRODUCT,'protected_source_sha256':PROTECTED_DIGEST}


def remaining(deadline, maximum):
    value=min(maximum,deadline-time.monotonic())
    if value<=0:raise TimeoutError('Protected phase exhausted')
    return value


def listener_ready():
    deadline=time.monotonic()+60
    for _ in range(30):
        try:
            with socket.create_connection(('127.0.0.1',3389),timeout=remaining(deadline,1)):
                return
        except OSError:pass
        time.sleep(remaining(deadline,1))
    raise TimeoutError('Protected listener unavailable')


def read_json(path, budget=64*1024):
    return json.loads(desktop.read_plain(path,budget).decode('utf-8-sig'),object_pairs_hook=files.unique_json)


def write_json(path,payload,*,needles=()):
    """Atomic bounded selected JSON; failed writes preserve the initialized red gate."""
    path=Path(path);body=json.dumps(payload,ensure_ascii=True).encode('utf-8')
    if len(body)>64*1024:raise ValueError('Selected JSON budget')
    if any(needle and needle in body for needle in needles):raise ValueError('Selected JSON credential')
    temporary=path.with_name('.'+path.name+'.tmp')
    try:
        with temporary.open('xb') as stream:stream.write(body)
        os.replace(temporary,path)
    finally:
        # A cleanup error must not replace the failed write or invalidate publication.
        try:temporary.unlink(missing_ok=True)
        except OSError:pass


def installed_identity(path,expected):
    raw=read_json(path,files.MAX_MANIFEST_BYTES)
    if not isinstance(raw,dict) or set(raw)!={'bridge_sha256','cua_sha256','manifest',
            'installed_bundle_id','installed_under_localappdata','session_id'}:
        raise ValueError('Installed schema')
    identity=files.manifest_identity(raw['manifest'],expected)
    if (raw['bridge_sha256']!=expected or raw['cua_sha256']!=identity['cua_sha256']
            or raw['installed_bundle_id']!=identity['bundle_id']
            or raw['installed_under_localappdata'] is not True
            or type(raw['session_id']) is not int or raw['session_id']<=0):
        raise ValueError('Installed identity')
    return {**identity,'os_session_id':raw['session_id'],'installed_under_localappdata':True}


def scan(root,needles,deadline=None):
    """Finite plain-file scans; guest output and paths never cross the boundary."""
    total=0;count=0
    for path in Path(root).rglob('*'):
        if deadline is not None:remaining(deadline,1)
        if path.is_dir():
            files.plain(path,directory=True)
            continue
        count+=1
        if count>256:raise ValueError('Evidence file count')
        with files.open_plain(path,16*1024*1024) as stream:
            tail=b''
            while data:=stream.read(64*1024):
                if deadline is not None:remaining(deadline,1)
                total+=len(data)
                if total>128*1024*1024:raise ValueError('Evidence byte budget')
                block=tail+data
                if any(n and n in block for n in needles):return False
                tail=block[-max((len(n) for n in needles),default=1):]
    return True


def run_class(base):
    class FirstA(base):
        daemon_executable='rdpilot-daemon.exe'
        daemon_start_wait=1
        cleanup_budget=FINALIZE_SECONDS
        work_budget=WORK_SECONDS

        async def proof_body(self):
            self.first_a_bridge_live=False
            identity_path=self.output/'installed-a.json'
            if identity_path.exists():raise ValueError('Stale identity')
            result=await self.connect('a')
            if result.get('bridge_live') is not True:raise ValueError('Live bridge identity')
            self.first_a_bridge_live=True
            self.operation='first_a_attach'
            async def attach_identity():
                await self.attach('a')
                self.first_a_identity=installed_identity(identity_path,self.args.source_bridge_sha256)
            await asyncio.wait_for(attach_identity(),max(0.001,ATTACH_SECONDS-5))

        def scan_evidence(self,needles):
            return scan(self.output,needles,self.cleanup_deadline)
    return FirstA


def project(summary,run,code,expected,scanned):
    selected={'mode':MODE,'status':'failed','completed_checks':[],'identity':[],
              'outcome':'inconclusive','failure_stage':'harness' if code else 'required_evidence'}
    primary=getattr(run,'primary_failure',None)
    if isinstance(primary,dict) and primary.get('operation') in ('connect_cli','bridge_ready_assertion') and primary.get('exception_category')=='proof_assertion':selected['outcome']='first_a_failed'
    try:
        if not scanned or code or not isinstance(summary,dict) or set(summary)!={'mode','status','checks','unverified'}:
            return selected
        if summary['mode']!='live' or summary['status']!='passed' or summary['unverified']!=['UAC/secure-desktop behavior is unsupported; no elevation guarantee']:
            return selected
        checks=summary['checks']
        if not isinstance(checks,list) or len(checks)!=len(CHECKS):return selected
        if any(not isinstance(c,dict) or c.get('check')!=name or c.get('passed') is not True for c,name in zip(checks,CHECKS)):
            return selected
        if not any(c.get('check')=='evidence_contains_no_credentials' and c.get('passed') is True for c in run.checks):return selected
        identity=installed_identity(run.output/'installed-a.json',expected)
        recorded=checks[-1]
        if set(recorded)!={'check','passed','bridge_sha256','archive_sha256','cua_sha256','cua_version','bundle_id','os_session_id'}:return selected
        if any(recorded[k]!=identity[k] for k in set(recorded)-{'check','passed'}):return selected
        if identity!=getattr(run,'first_a_identity',None) or getattr(run,'first_a_bridge_live',False) is not True:return selected
        selected.update(status=PASSED,completed_checks=list(CHECKS),identity=[identity],outcome='qualified',failure_stage='none')
    except (OSError,ValueError,TypeError,KeyError,RecursionError,files.BudgetExceeded):pass
    return selected


def admit(selected,expected):
    if not isinstance(selected,dict) or set(selected)!={'mode','status','completed_checks','identity','outcome','failure_stage'}:
        return False
    if (selected['mode']!=MODE or selected['status']!=PASSED or selected['completed_checks']!=list(CHECKS)
            or selected['outcome']!='qualified' or selected['failure_stage']!='none' or not isinstance(selected['identity'],list) or len(selected['identity'])!=1):return False
    identity=selected['identity'][0]
    if not isinstance(identity,dict) or set(identity)!=IDENTITY_KEYS:return False
    if not isinstance(expected,str) or not files.HASH.fullmatch(expected) or not all(isinstance(identity[k],str) and files.HASH.fullmatch(identity[k]) for k in ('bridge_sha256','cua_sha256','archive_sha256')):return False
    canonical='cua-driver-rs-v0.34.0-'+hashlib.sha256((expected+identity['archive_sha256']).encode('ascii')).hexdigest()[:16]
    return (identity['bridge_sha256']==expected
            and identity['cua_version']=='0.34.0' and identity['bundle_id']==canonical
            and type(identity['os_session_id']) is int and identity['os_session_id']>0
            and identity['installed_under_localappdata'] is True)


def failed_selected(raw):
    """Replace malformed failed artifacts with a fixed closed red projection."""
    import sys
    sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'e2e'))
    import proof_support
    safe=project(None,None,1,None,False)
    if not isinstance(raw,dict):return safe
    if raw.get('outcome')=='first_a_failed':safe['outcome']='first_a_failed'
    if raw.get('failure_stage') in ('harness','required_evidence','cleanup','artifact_write'):safe['failure_stage']=raw['failure_stage']
    if isinstance(raw.get('harness_failure'),dict):safe['harness_failure']=proof_support.select_failure_detail(raw['harness_failure'])
    if isinstance(raw.get('secondary_failures'),list):safe['secondary_failures']=[proof_support.select_failure_detail(x) for x in raw['secondary_failures'][:32]]
    return safe


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--verify-source',action='store_true',required=True)
    parser.add_argument('--expected-commit',required=True)
    parser.add_argument('--output',required=True)
    args=parser.parse_args()
    try:
        source=verify_source(args.expected_commit)
        write_json(Path(args.output)/'source.json',source)
    except BaseException:return 1
    return 0


if __name__=='__main__':raise SystemExit(main())
