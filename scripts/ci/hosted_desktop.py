"""Owned local Windows RDP setup, serial existing proofs and bounded cleanup.

No provider provisioning; requires an administrator and an unused rdpilot host.
Raw output and credentials stay inside the pre-created owner-only ACL boundary.
"""
import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import time
import held_process
import acquisition_observer
import guest_footprint_observer as footprint
import bootstrap_desktop_observer as desktop

HERE = Path(__file__).resolve().parent
_native_owner=None
spec = importlib.util.spec_from_file_location('live_proof', HERE/'run-live-proof.py')
live = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live)


ERROR_CODES={'host_action','member_exists','parameter_binding','group_not_found','user_not_found','access_denied','native_error','timeout','invalid_host_json','invalid_password','invalid_name','invalid_parameters','account_exists','internal_error','command_not_found','module_autoload','secure_argument','secure_crypto'}

SUBACTIONS={'script','account_preflight','secure_password','new_local_user','sid_readback','sid_journal'}
EXCEPTION_CATEGORIES={'access_denied','argument','parameter_binding','local_accounts','io','other','action_preference','invocation','security','command_not_found','crypto','runtime','invalid_operation','not_supported'}

class HostActionError(RuntimeError):
    def __init__(self,code,detail=None):
        self.code=code if code in ERROR_CODES else 'host_action'
        raw=detail if isinstance(detail,dict) else {}
        self.detail={'subaction':raw.get('subaction') if isinstance(raw.get('subaction'),str) and raw.get('subaction') in SUBACTIONS else 'script',
            'exception_category':raw.get('exception_category') if isinstance(raw.get('exception_category'),str) and raw.get('exception_category') in EXCEPTION_CATEGORIES else 'other'}
        for key in ('hresult','native_error','ntstatus'):
            value=raw.get(key)
            if type(value) is int and -(2**31)<=value<2**32:self.detail[key]=value
        super().__init__('Private host action failed')


def powershell(script, values=None, timeout=30, *, inherit_module_path=False):
    env = dict(os.environ)
    # A pwsh parent can export Core modules incompatible with Windows PS 5.1.
    # Let this native child construct its own standard module search path.
    if not inherit_module_path:
        for key in tuple(env):
            if key.casefold()=='psmodulepath':env.pop(key)
    env['RDPILOT_HOST_CONTROL'] = json.dumps(values or {})
    source = "$ErrorActionPreference='Stop';$v=$env:RDPILOT_HOST_CONTROL|ConvertFrom-Json;$subaction='script';try{"+script+r'''
}catch{
 # Cmdlets may wrap the actual LocalAccounts error in ActionPreferenceStop.
 $record=$_
 for($i=0;$i -lt 4;$i++){
  $nested=$record.Exception.ErrorRecord
  if(!$nested -or [object]::ReferenceEquals($nested,$record)){break}
  $record=$nested
 }
 $exception=$record.Exception
 $code=switch -Wildcard ($record.FullyQualifiedErrorId){
  'MemberExists*'{'member_exists'} '*ParameterBinding*'{'parameter_binding'}
  'GroupNotFound*'{'group_not_found'} 'UserNotFound*'{'user_not_found'}
  'InvalidPassword*'{'invalid_password'} 'InvalidName*'{'invalid_name'} 'InvalidParameters*'{'invalid_parameters'}
  'NameInUse*'{'account_exists'} 'UserExists*'{'account_exists'} 'Internal*'{'internal_error'}
  'CouldNotAutoLoad*'{'module_autoload'}
  'ImportSecureString_InvalidArgument_CryptographicError*'{'secure_crypto'}
  'ImportSecureString_InvalidArgument,*'{'secure_argument'}
  '*CommandNotFound*'{'command_not_found'}
  '*AccessDenied*'{'access_denied'} '*Win32*'{'native_error'} default{'host_action'}
 }
 $category=switch -Wildcard ($exception.GetType().FullName){
  '*UnauthorizedAccess*'{'access_denied'} '*Argument*'{'argument'} '*ParameterBinding*'{'parameter_binding'}
  '*ActionPreferenceStop*'{'action_preference'} '*Invocation*'{'invocation'} '*SecurityException'{'security'}
  '*CommandNotFoundException'{'command_not_found'} '*CryptographicException'{'crypto'}
  '*RuntimeException'{'runtime'} '*InvalidOperationException'{'invalid_operation'} '*NotSupportedException'{'not_supported'}
  '*LocalAccounts*'{'local_accounts'} '*IOException*'{'io'} default{'other'}
 }
 if($exception.GetType().BaseType.FullName -eq 'Microsoft.PowerShell.Commands.LocalAccountsException'){$category='local_accounts'}
 $detail=@{code=$code;subaction=$subaction;exception_category=$category;hresult=$exception.HResult}
 if($null -ne $exception.NativeErrorCode){$detail.native_error=$exception.NativeErrorCode}
 if($null -ne $exception.StatusCode){$detail.ntstatus=[long]$exception.StatusCode}
 # Windows PowerShell may prepend module-import CLIXML to stderr. Keep the
 # trusted closed envelope on stdout; the nonzero exit still marks failure.
 [Console]::Out.Write(($detail|ConvertTo-Json -Compress));exit 1
}
'''
    command=['powershell.exe','-NoProfile','-NonInteractive','-EncodedCommand',base64.b64encode(source.encode('utf-16le')).decode()]
    try:
        if _native_owner is None:result = subprocess.run(command,env=env,capture_output=True,timeout=timeout)
        else:
            import first_a
            bounded=first_a.remaining(_native_owner['deadline'],timeout)
            child=subprocess.Popen(command,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
            try:
                held_process.register_sync(child,live.process_identity,_native_owner['journal'],_native_owner['records'],timeout=max(0,min(15,_native_owner['deadline']-time.monotonic())))
            except BaseException as error:
                if getattr(error,'held_cleanup_failures',()):_native_owner['stop_failed']=True
                raise
            try:out,err=child.communicate(timeout=bounded)
            except BaseException:
                if held_process.abort_sync(child,max(0,min(15,_native_owner['deadline']-time.monotonic()))):_native_owner['stop_failed']=True
                raise
            if len(out)>64*1024 or len(err)>64*1024:raise HostActionError('invalid_host_json')
            result=subprocess.CompletedProcess(command,child.returncode,out,err)
    except subprocess.TimeoutExpired:raise HostActionError('timeout')
    if result.returncode:
        data=result.stdout.decode('utf-8-sig',errors='replace').strip()
        try:detail=json.loads(data)
        except ValueError:detail={}
        raise HostActionError(detail.get('code',data) if isinstance(detail,dict) else 'host_action',detail)
    data = result.stdout.decode('utf-8-sig', errors='replace').strip()
    try:return json.loads(data) if data else None
    except ValueError:raise HostActionError('invalid_host_json')


# WTS enumeration avoids localized quser parsing; every logoff is SID-bound.
WTS = r'''
Add-Type @'
using System; using System.Runtime.InteropServices;
public class OwnedSessions {
 [StructLayout(LayoutKind.Sequential)] public struct Info { public int Id; public IntPtr Name; public int State; }
 [DllImport("wtsapi32.dll")] public static extern bool WTSEnumerateSessions(IntPtr h,int r,int v,out IntPtr p,out int n);
 [DllImport("wtsapi32.dll",CharSet=CharSet.Unicode)] public static extern bool WTSQuerySessionInformation(IntPtr h,int id,int cls,out IntPtr p,out int n);
 [DllImport("wtsapi32.dll")] public static extern void WTSFreeMemory(IntPtr p);
 [DllImport("wtsapi32.dll")] public static extern bool WTSLogoffSession(IntPtr h,int id,bool wait);
 public static string Query(int id,int cls){IntPtr p;int n;if(!WTSQuerySessionInformation(IntPtr.Zero,id,cls,out p,out n))throw new Exception("Session query failed");try{return Marshal.PtrToStringUni(p);}finally{WTSFreeMemory(p);}}
}
'@
$ptr=[IntPtr]::Zero;$count=0
if(![OwnedSessions]::WTSEnumerateSessions([IntPtr]::Zero,0,1,[ref]$ptr,[ref]$count)){throw 'Session enumeration failed'}
$matched=@()
try {
 $size=[Runtime.InteropServices.Marshal]::SizeOf([type][OwnedSessions+Info])
 for($i=0;$i -lt $count;$i++) {
  $s=[Runtime.InteropServices.Marshal]::PtrToStructure([IntPtr]::Add($ptr,$i*$size),[type][OwnedSessions+Info])
  $user=[OwnedSessions]::Query($s.Id,5);$domain=[OwnedSessions]::Query($s.Id,7)
  if(!$user){continue}
  $sid=([Security.Principal.NTAccount]::new($domain,$user)).Translate([Security.Principal.SecurityIdentifier]).Value
  if($sid -eq $v.sid){$matched+=$s.Id}
 }
} finally {[OwnedSessions]::WTSFreeMemory($ptr)}
'''

PREFLIGHT = r'''
$cache=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'rdpilot\cache'
if(Test-Path $cache){throw 'Existing cache is unowned'}
if(@(Get-ChildItem '\\.\pipe\'|Where-Object Name -eq 'rdpilot-daemon').Count){throw 'Existing daemon pipe is unowned'}
if(@(Get-CimInstance Win32_Process|Where-Object Name -in @('rdpilot-daemon.exe','rdpilot-mcp.exe','rdpilot.exe')).Count){throw 'Existing product process is unowned'}
$ts='HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server';$tcp=Join-Path $ts 'WinStations\RDP-Tcp'
$regs=@()
foreach($entry in @(@($ts,'fDenyTSConnections'),@($tcp,'UserAuthentication'))) {
 $p=(Get-ItemProperty $entry[0]).PSObject.Properties[$entry[1]]
 $regs+=@{path=$entry[0];name=$entry[1];present=($null -ne $p);value=if($p){$p.Value}else{$null}}
}
$svc=Get-CimInstance Win32_Service -Filter "Name='TermService'"
@{cache=$cache;registry=$regs;service=@{state=$svc.State;start_mode=$svc.StartMode};
 os=(Get-CimInstance Win32_OperatingSystem).Caption;image_os=$env:ImageOS;image_version=$env:ImageVersion;computer=$env:COMPUTERNAME}|ConvertTo-Json -Depth 8 -Compress
'''
SETUP = r'''
Set-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server' fDenyTSConnections 0
Set-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp' UserAuthentication 1
if((Get-CimInstance Win32_Service -Filter "Name='TermService'").StartMode -eq 'Disabled'){Set-Service TermService -StartupType Manual}
Start-Service TermService
New-NetFirewallRule -Name $v.rule -DisplayName 'rdpilot local desktop proof' -Direction Inbound -Action Allow -Protocol TCP -LocalPort 3389 -RemoteAddress '127.0.0.1'|Out-Null
'''
CREATE_USER = r'''
$subaction='account_preflight'
if(Get-LocalUser $v.name -ErrorAction SilentlyContinue){throw 'Unowned account exists'}
$subaction='secure_password'
$secure=ConvertTo-SecureString $v.password -AsPlainText -Force
$subaction='new_local_user'
$u=New-LocalUser -Name $v.name -Description $v.marker -Password $secure -PasswordNeverExpires
$subaction='sid_readback'
$record=@{name=$u.Name;sid=$u.SID.Value}
$subaction='sid_journal'
$record|ConvertTo-Json -Compress|Set-Content -Encoding UTF8 $v.journal
$record|ConvertTo-Json -Compress
'''
ENSURE_GROUP = r'''
$u=Get-LocalUser $v.name
if($u.SID.Value -ne $v.sid){throw 'Account identity changed'}
$members=@(Get-LocalGroupMember -SID $v.group_sid)
$initially=@($members|Where-Object {$_.SID.Value -eq $v.sid}).Count -gt 0
if(!$initially){Add-LocalGroupMember -SID $v.group_sid -Member $u}
$now=@(Get-LocalGroupMember -SID $v.group_sid|Where-Object {$_.SID.Value -eq $v.sid}).Count -gt 0
if(!$now){throw 'Required group membership absent'}
@{initially_member=$initially;membership_verified=$now}|ConvertTo-Json -Compress
'''
ROLE = r'''
$u=Get-LocalUser $v.name
if($u.SID.Value -ne $v.sid -or !$u.Enabled){throw 'Account role identity mismatch'}
$rdp=@(Get-LocalGroupMember -SID 'S-1-5-32-555'|Where-Object {$_.SID.Value -eq $v.sid}).Count -gt 0
$users=@(Get-LocalGroupMember -SID 'S-1-5-32-545'|Where-Object {$_.SID.Value -eq $v.sid}).Count -gt 0
$admin=@(Get-LocalGroupMember -SID 'S-1-5-32-544'|Where-Object {$_.SID.Value -eq $v.sid}).Count -gt 0
if(!$rdp -or !$users -or $admin){throw 'Standard user membership mismatch'}
@{rdp_member=$rdp;users_member=$users;administrator_member=$admin}|ConvertTo-Json -Compress
'''

PROFILE = r'''
$profile=Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'"
$present=$null -ne $profile
if($profile){
 if($profile.Special -or $profile.Loaded){throw 'Profile is special or loaded'}
 $path=$profile.LocalPath
 if((Split-Path $path -Parent) -ne (Join-Path $env:SystemDrive 'Users') -or (Split-Path $path -Leaf) -ne $v.name){throw 'Unowned profile path'}
 $profile|Remove-CimInstance
 if(Test-Path $path){throw 'Profile/guest files remain'}
}
if(Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'"){throw 'Profile row remains'}
if(Test-Path (Join-Path (Join-Path $env:SystemDrive 'Users') $v.name)){throw 'Owned guest directory remains'}
@{profile_was_present=$present;profile_guest_removed=$true}|ConvertTo-Json -Compress
'''
REMOVE_USER = r'''
if(Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'"){throw 'Profile row remains before account removal'}
if(Test-Path (Join-Path (Join-Path $env:SystemDrive 'Users') $v.name)){throw 'Guest profile directory remains before account removal'}
$u=Get-LocalUser $v.name -ErrorAction SilentlyContinue
if($u){if($u.SID.Value -ne $v.sid){throw 'Account identity changed'};Remove-LocalUser -SID $u.SID}
if(Get-LocalUser $v.name -ErrorAction SilentlyContinue){throw 'Account remains'}
'''
STOP_PROCESS = r'''
$p=Get-Process -Id $v.pid -ErrorAction SilentlyContinue
if($p){
 $handle=$p.Handle
 if($p.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -ne $v.created -or $p.Path -ine $v.image){throw 'Process identity changed'}
 $p.Kill();if(!$p.WaitForExit(10000)){throw 'Owned process remains'}
}
'''


def cleanup_actions(actions):
    """Every independently bounded action is attempted even after failure."""
    results = {}
    for name, action in actions:
        try:
            action()
            results[name] = True
        except BaseException:
            results[name] = False
    return results


def stop_owned_processes(journal, required=False):
    if required and not journal.exists():raise RuntimeError('Required process journal absent')
    records=json.loads(journal.read_text()) if journal.exists() else []
    values = powershell(r'''$ok=$true
foreach($record in $v.records){try{
 $p=Get-Process -Id $record.pid -ErrorAction SilentlyContinue
 if($p){$handle=$p.Handle
  if($p.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -ne $record.created -or $p.Path -ine $record.image){throw 'Process identity changed'}
  $p.Kill();if(!$p.WaitForExit(1000)){throw 'Owned process remains'}
 }
}catch{$ok=$false}}
$ok|ConvertTo-Json
''', {'records':records},30)
    if values is not True:raise RuntimeError('Owned process cleanup failed')


def cleanup_suite(users, journal, cache, require_profiles=False, *, required_profile_indices=None):
    required = set(range(len(users))) if require_profiles else set()
    if required_profile_indices is not None:
        indices = tuple(required_profile_indices)
        if any(type(i) is not int or i < 0 or i >= len(users) for i in indices):
            raise ValueError('Invalid required profile index')
        required = set(indices)
    require_journal = require_profiles if required_profile_indices is None else bool(required)
    actions=[]
    profile_presence={}
    def stop_processes():
        stop_owned_processes(journal,require_journal)
    actions.append(('owned_processes_removed',stop_processes))
    for i,user in enumerate(users):
        def reconcile(user=user):
            if 'sid' in user:
                user['creation_present']=True
                return
            observed=powershell(r'''
$u=Get-LocalUser $v.name -ErrorAction SilentlyContinue
if(!$u){@{absent=$true}|ConvertTo-Json -Compress}else{
 if($u.Description -ne $v.marker){throw 'Creation ownership marker mismatch'}
 @{name=$u.Name;sid=$u.SID.Value}|ConvertTo-Json -Compress
}
''',user,10)
            user['creation_present']=not observed.get('absent',False)
            if observed.get('absent'):user['sid']='S-1-0-0'
            else:user.update(observed)
        actions.append(('user_'+str(i)+'_creation_reconciled',reconcile))
        def logoff(user=user):
            powershell(WTS+r'''
foreach($id in $matched){if(![OwnedSessions]::WTSLogoffSession([IntPtr]::Zero,$id,$false)){throw 'Owned logoff failed'}}
''',user,15)
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                loaded=powershell("$p=Get-CimInstance Win32_UserProfile -Filter \"SID='$($v.sid)'\";[bool]($p -and $p.Loaded)|ConvertTo-Json",user,5)
                if not loaded:
                    powershell(WTS+"if($matched.Count){throw 'Owned session remains'}",user,5)
                    return
                time.sleep(0.5)
            raise RuntimeError('Profile stayed loaded')
        def remove_profile(user=user,index=i):
            observed=powershell(PROFILE,user,20)
            present=isinstance(observed,dict) and observed.get('profile_was_present') is True
            profile_presence['user_'+str(index)+'_profile_was_present']=present
            if index in required and not present:raise RuntimeError('Required live profile was not observed')
        actions += [('user_'+str(i)+'_sessions',logoff),('user_'+str(i)+'_profile_guest_removed',remove_profile),('user_'+str(i)+'_account',lambda user=user:powershell(REMOVE_USER,user,20))]
    def clear_cache():
        powershell("if(Test-Path $v.path){Remove-Item $v.path -Recurse -Force};if(Test-Path $v.path){throw 'Cache remains'}",{'path':cache},20)
    actions.append(('native_cache_removed',clear_cache))
    results=cleanup_actions(actions)
    # Presence is required only after a completed live proof, not failed setup.
    for i in sorted(required):
        key='user_'+str(i)+'_profile_was_present'
        if key in profile_presence:results[key]=profile_presence[key]
    return results


def measure_process_refusal():
    child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    if _native_owner is None:record=live.process_identity(child.pid)
    else:
        try:record=held_process.register_sync(child,live.process_identity,_native_owner['journal'],_native_owner['records'])
        except BaseException as error:
            if getattr(error,'held_cleanup_failures',()):_native_owner['stop_failed']=True
            raise
    try:
        for field,value in (('created',str(int(record['created'])+1)),('image','unowned.exe')):
            refused=False
            try:powershell(STOP_PROCESS,{**record,field:value},10)
            except RuntimeError:refused=True
            if not refused or child.poll() is not None:raise RuntimeError('Owned process refusal was not proven')
    finally:
        try:
            powershell(STOP_PROCESS,record,15)
            child.wait(timeout=5)
        except BaseException:
            if _native_owner is not None:
                if held_process.abort_sync(child,max(0,min(15,_native_owner['deadline']-time.monotonic()))):_native_owner['stop_failed']=True
            raise


def private_environment(private):
    return {**os.environ,**{key:str(Path(private).resolve()) for key in ('TMPDIR','TEMP','TMP')}}


def mode_for(args):
    setup=getattr(args,'setup_diagnostic',False);cua=getattr(args,'cua_diagnostic',False)
    baseline=getattr(args,'baseline_first_a',False)
    if sum(bool(x) for x in (setup,cua,baseline))>1:raise ValueError('Diagnostic modes are mutually exclusive')
    desktop_mode=getattr(args, 'bootstrap_desktop', False);footprint_mode=getattr(args, 'bootstrap_footprint', False)
    if desktop_mode and footprint_mode:raise ValueError('Bootstrap observers are mutually exclusive')
    if (desktop_mode or footprint_mode) and not cua:
        raise ValueError('Bootstrap observation requires Cua diagnostic')
    return 'protected_first_a' if baseline else 'setup_diagnostic' if setup else 'cua_diagnostic' if cua else 'live'


def initialize(output,mode='live'):
    if mode not in ('live','setup_diagnostic','cua_diagnostic','protected_first_a'):raise ValueError('Unknown mode')
    output=Path(output)
    output.mkdir(parents=True,exist_ok=False)
    artifacts=output/'artifacts';artifacts.mkdir()
    (artifacts/'gate.json').write_text(json.dumps({'mode':mode,'status':'failed','failure_stage':'build_or_dependencies'}))


def run(args):
    global _native_owner
    mode=mode_for(args)
    baseline=mode=='protected_first_a'
    if baseline:
        import first_a
        source=first_a.verify_source(args.expected_commit)
        parent_deadline=time.monotonic()+first_a.PARENT_SECONDS
    expected=['first_a'] if baseline else ['host_setup'] if mode=='setup_diagnostic' else ['cua'] if mode=='cua_diagnostic' else list(live.CHECKS)
    output=Path(args.output).resolve();artifacts=output/'artifacts';private=output/'private'
    if not artifacts.is_dir() or private.exists():
        raise RuntimeError('Initialize new gate output first')
    if baseline:
        if first_a.read_json(artifacts/'gate.json')!={'mode':first_a.MODE,'status':'failed','failure_stage':'build_or_dependencies'}:
            raise ValueError('Initialize fresh red baseline')
        if first_a.read_json(artifacts/'source.json')!=source or {p.name for p in artifacts.iterdir()}!={'gate.json','source.json'}:
            raise ValueError('Prebuild source binding unavailable')
    private.mkdir()
    acl="& icacls $v.path /inheritance:r /grant:r \"$($env:USERDOMAIN)\\$($env:USERNAME):(OI)(CI)F\"|Out-Null;if($LASTEXITCODE){throw 'Private ACL failed'}"
    if not baseline:
        powershell(acl,{'path':str(private)})
        os.environ.update(private_environment(private))
    snapshot=None;results={};suites=[];attempted=[];users=[];journal=private/'processes.json';rule='RdpilotDesktop-'+secrets.token_hex(8);observation_failures=[]
    wrapper=None;wrapper_joined=True;wrapper_stop_attempted=False;cleanup_started=False
    observer_source = {}
    stage='preflight';failure_detail={};failure_code='none';passed=False;setup_attempted=False;roles=[];memberships=[];creation_observations=[]
    try:
        if baseline:
            _native_owner={'journal':private/'native-processes.json','records':[],
                'deadline':min(parent_deadline,time.monotonic()+600),'stop_failed':False}
            powershell(acl,{'path':str(private)})
            os.environ.update(private_environment(private))
        snapshot=powershell(PREFLIGHT)
        measure_process_refusal()
        commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
        tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip()
        if commit!=args.expected_commit or not all(len(x)==40 and all(c in '0123456789abcdef' for c in x) for x in (commit,tree)):
            raise RuntimeError('Source identity mismatch')
        diagnostic=mode=='setup_diagnostic'
        bin_dir=Path(args.bin_dir or '.').resolve()
        hashes={} if diagnostic else {name:hashlib.sha256((bin_dir/(name+'.exe')).read_bytes()).hexdigest() for name in ('rdpilot','rdpilot-daemon','rdpilot-mcp','rdpilot-bridge')}
        environment={k:snapshot[k] for k in ('os','image_os','image_version')}
        # Values below are from local OS/image metadata, never guest/tool text.
        environment.update(source_commit=commit,source_tree=tree,binary_sha256=hashes,
            auth='NLA/CredSSP; explicit self-signed TLS acceptance',graphics='product default',account_role='standard',
            target='loopback',cua_version='0.34.0',process_identity_refusals_verified=True)
        if baseline:environment.update(source,recipe='protected-earlier-startup',bounds_seconds={'listener':60,'work':900,'attach':240,'finalizer':60,'wrapper_stop':15,'host_cleanup':600,'parent':2400})
        if baseline:first_a.write_json(artifacts/'environment.json',environment)
        else:(artifacts/'environment.json').write_text(json.dumps(environment,indent=2))
        observer_source = {'source_commit': commit, 'source_tree': tree,
            'cli_sha256': hashes.get('rdpilot'), 'daemon_sha256': hashes.get('rdpilot-daemon'),
            'bridge_sha256': hashes.get('rdpilot-bridge')}
        if diagnostic:
            conversion=r'''$secure=ConvertTo-SecureString 'Diagnostic1!FixedHarmlessInput' -AsPlainText -Force
@{converted=($secure.Length -gt 0);security_module_major=(Get-Command ConvertTo-SecureString).Module.Version.Major}|ConvertTo-Json -Compress'''
            measurements={}
            for label,inherited in (('inherited',True),('native_default',False)):
                try:measurements[label]={'passed':True,**powershell(conversion,inherit_module_path=inherited)}
                except HostActionError as error:measurements[label]={'passed':False,'failure_code':error.code,'host_failure':error.detail}
            (artifacts/'security-module-control.json').write_text(json.dumps(measurements,indent=2))
            if measurements['native_default'].get('converted') is not True:raise RuntimeError('Native Security module conversion failed')
        bundle=private/'bundle';bundle.mkdir()
        if not diagnostic:shutil.copyfile(bin_dir/'rdpilot-bridge.exe',bundle/'rdpilot-bridge.exe')
        stage='setup';setup_attempted=True;powershell(SETUP,{'rule':rule})
        if baseline:first_a.listener_ready()
        deadline=time.monotonic()+45*60
        for proof in (('cua',) if diagnostic else expected):
            stage=proof+'_create_user';users=[];journal=private/(proof+'-processes.json');credentials={}
            for label in (('a',) if proof=='takeover' else ('a','b')):
                name='rdp'+secrets.token_hex(7)
                password='Rdp1!'+secrets.token_urlsafe(24)+'aA1!'
                print('::add-mask::'+password,flush=True)
                stage=proof+'_create_user'
                user={'name':name,'marker':'rdpilot-owned-'+secrets.token_hex(16)}
                users.append(user)
                created=powershell(CREATE_USER,{**user,'password':password,'journal':str(private/('created-'+name+'.json'))})
                user.update(created)
                # Journal SID before groups/credentials or any other fallible action.
                (private/'users.json').write_text(json.dumps(users))
                for group,sid in (('remote','S-1-5-32-555'),('users','S-1-5-32-545')):
                    stage=proof+'_group_'+group
                    member=powershell(ENSURE_GROUP,{**user,'group_sid':sid})
                    if (not isinstance(member,dict) or type(member.get('initially_member')) is not bool or member.get('membership_verified') is not True):
                        raise RuntimeError('Group evidence unavailable')
                    memberships.append({'proof':proof,'group':group,**member})
                stage=proof+'_account_role'
                role=powershell(ROLE,user)
                if role!={'rdp_member':True,'users_member':True,'administrator_member':False}:raise RuntimeError('Role evidence unavailable')
                roles.append({'proof':proof,**role})
                stage=proof+'_credentials'
                credentials[label]={'host':'127.0.0.1','port':3389,'username':name,'domain':snapshot['computer'],'password':password}
                (private/(label+'.json')).write_text(json.dumps(credentials[label]))
            if diagnostic:
                attempted.append('host_setup')
                checks=cleanup_suite(users,journal,snapshot['cache'])
                results['host_setup']=checks
                creation_observations.extend({'suite':'host_setup','present':u.get('creation_present')} for u in users)
                users=[]
                if not all(checks.values()):raise RuntimeError('Setup cleanup failed')
                suites.append('host_setup')
                continue
            if baseline:
                stage='first_a_fresh_accounts'
                for owned in users:
                    powershell(WTS+"if($matched.Count){throw 'Fresh user already has a session'}",owned,5)
                    fresh=powershell(r'''if(Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'"){throw 'Fresh profile already exists'}
if(Test-Path (Join-Path (Join-Path $env:SystemDrive 'Users') $v.name)){throw 'Fresh profile directory already exists'}
@{fresh_profile_verified=$true}|ConvertTo-Json -Compress''',owned,5)
                    if not isinstance(fresh,dict) or set(fresh)!={'fresh_profile_verified'} or fresh['fresh_profile_verified'] is not True:raise RuntimeError('Fresh profile evidence unavailable')
            child_env=dict(os.environ)
            for label,cred in credentials.items():
                prefix='RDPILOT_TAKE_' if proof=='takeover' else 'RDPILOT_VIEW_'+label.upper()+'_'
                child_env.update({prefix+k.upper():str(v) for k,v in cred.items()})
            stage=proof+'_harness'
            attempted.append(proof)
            command=[sys.executable,str(HERE/'run-live-proof.py'),proof,'--bin-dir',str(bin_dir),'--bundle',str(bundle),
                '--output',str(private/(proof+'-raw')),'--artifacts',str(artifacts),'--source-bridge-sha256',hashes['rdpilot-bridge'],
                '--journal',str(journal),'--credentials-a',str(private/'a.json'),'--credentials-b',str(private/'b.json')]
            fresh_footprint=False
            if getattr(args, 'bootstrap_footprint', False):
                try:
                    owned=users[0]
                    if owned.get('name')!=credentials['a']['username']:raise ValueError('Fresh target ownership unavailable')
                    powershell(WTS+"if($matched.Count){throw 'Fresh user already has a session'}", owned, 5)
                    fresh=powershell(r'''$u=@(Get-LocalUser -SID $v.sid)
if($u.Count -ne 1 -or $u[0].SID.Value -cne $v.sid -or $u[0].Name -cne $v.name -or $u[0].Description -cne $v.marker){throw 'Fresh ownership unavailable'}
if(Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'"){throw 'Fresh profile already exists'}
if(Test-Path (Join-Path (Join-Path $env:SystemDrive 'Users') $v.name)){throw 'Fresh profile directory already exists'}
@{fresh_profile_verified=$true}|ConvertTo-Json -Compress''', owned, 5)
                    fresh_footprint=isinstance(fresh,dict) and set(fresh)=={'fresh_profile_verified'} and fresh['fresh_profile_verified'] is True
                except Exception:observation_failures.append('guest_footprint_baseline')
            if getattr(args, 'bootstrap_desktop', False):
                owned = users[0]
                powershell(WTS+"if($matched.Count){throw 'Fresh user already has a session'}", owned, 5)
                powershell(r'''if(Get-CimInstance Win32_UserProfile -Filter "SID='$($v.sid)'"){throw 'Fresh profile already exists'}
if(Test-Path (Join-Path (Join-Path $env:SystemDrive 'Users') $v.name)){throw 'Fresh profile directory already exists'}''', owned, 5)
                # Same native enumeration/translation as cleanup; only read active IDs.
                binding = "$ErrorActionPreference='Stop';$v=$env:RDPILOT_HOST_CONTROL|ConvertFrom-Json;" + WTS.replace(
                    '$matched+=$s.Id', '$matched+=@{id=$s.Id;state=$s.State}')
                binding += "@{matches=@($matched)}|ConvertTo-Json -Depth 4 -Compress"
                handoff = private/'bootstrap-desktop-config.json'
                handoff.write_text(json.dumps({'source': observer_source, 'sid': owned['sid'],
                    'username': credentials['a']['username'], 'domain': credentials['a']['domain'],
                    'wts_script': binding, 'mode': mode, 'fresh_profile_verified': True}))
                command += ['--bootstrap-desktop', '--bootstrap-desktop-config', str(handoff)]
            remaining=min(1000,int(parent_deadline-time.monotonic()-first_a.HOST_CLEANUP_SECONDS-15)) if baseline else min(900,int(deadline-time.monotonic()))
            if remaining<=0:raise TimeoutError('Work budget expired')
            with (private/(proof+'-console.log')).open('wb') as raw:
                child=subprocess.Popen(command,env=child_env,stdout=raw,stderr=raw)
                wrapper=child;wrapper_joined=False;wrapper_stop_attempted=False
                # Parent and child own distinct journals; child cannot replace ours.
                try:
                    held_process.register_sync(child,live.process_identity,private/(proof+'-wrapper-processes.json'),[])
                except BaseException as error:
                    wrapper_stop_attempted=True
                    wrapper_joined=not getattr(error,'held_cleanup_failures',('join',))
                    raise
                try:
                    code=child.wait(timeout=remaining+30)
                    wrapper_joined=True
                except subprocess.TimeoutExpired:
                    wrapper_stop_attempted=True
                    failures=held_process.abort_sync(child,15)
                    wrapper_joined=not failures
                    if failures:observation_failures.append('wrapper_cleanup')
                    code=1
            try:
                selected=first_a.read_json(artifacts/(proof+'.json')) if baseline else json.loads((artifacts/(proof+'.json')).read_text())
                if not isinstance(selected,dict) or selected.get('status') not in ((first_a.PASSED,'failed') if baseline else ('passed','failed')):
                    raise ValueError('Invalid selected status')
                if baseline and selected['status']==first_a.PASSED and not first_a.admit(selected,hashes['rdpilot-bridge']):raise ValueError('Baseline positive admission')
                if baseline and selected['status']=='failed':
                    selected=first_a.failed_selected(selected)
                    (artifacts/(proof+'.json')).write_text(json.dumps(selected))
            except Exception:
                selected={'status':'failed'}
                observation_failures.append('selected_artifact_read')
                # Replace an unreadable selected artifact with a closed failure.
                try:(artifacts/(proof+'.json')).write_text(json.dumps(first_a.project(None,None,1,hashes['rdpilot-bridge'],False) if baseline else live.project(proof,None,1,hashes['rdpilot-bridge'])))
                except Exception:
                    observation_failures.append('selected_artifact_write')
                    try:(artifacts/(proof+'.json')).unlink(missing_ok=True)
                    except OSError:observation_failures.append('selected_artifact_remove')
            if proof=='cua' and (code or selected['status']!='passed'):
                # Only the initially absent, run-owned cache; observe before cleanup.
                try:acquisition=acquisition_observer.observe_cache(snapshot['cache'],hashes['rdpilot-bridge'])
                except Exception:
                    acquisition={'state':'observation_failed'}
                    observation_failures.append('acquisition_observation')
                try:(artifacts/'local-acquisition.json').write_text(json.dumps(acquisition,indent=2)+'\n')
                except Exception:observation_failures.append('acquisition_artifact_write')
                if getattr(args, 'bootstrap_footprint', False):
                    def register_footprint(record):
                        records=json.loads(journal.read_text()) if journal.exists() else []
                        if not isinstance(records,list):raise ValueError('Invalid owned journal')
                        records.append(record);journal.write_text(json.dumps(records))
                    observed=footprint.empty();joined=True
                    try:
                        observed,joined=footprint.observe(selected,code,observer_source,acquisition,snapshot['cache'],users[0],
                            fresh_footprint,private,live.process_identity,register_footprint,
                            [str(c['password']).encode() for c in credentials.values()])
                    except BaseException:
                        # A refused observer never interrupts the independent suite cleanup.
                        joined=False;observed=footprint.empty(outcome='cleanup_failed',joined=False)
                        observation_failures.append('guest_footprint_observation')
                    results['guest_footprint']={'worker_joined':joined}
                    if not joined:observation_failures.append('guest_footprint_cleanup')
                    try:(artifacts/'guest-footprint.json').write_text(json.dumps(observed,indent=2)+'\n')
                    except Exception:observation_failures.append('guest_footprint_artifact_write')
            stage=proof+'_cleanup'
            if baseline:
                positive_child=code==0 and selected['status']==first_a.PASSED
                _native_owner['deadline']=min(parent_deadline,time.monotonic()+first_a.HOST_CLEANUP_SECONDS)
                cleanup_started=True
                try:
                    acquisition=acquisition_observer.observe_cache(snapshot['cache'],hashes['rdpilot-bridge'])
                    first_a.write_json(artifacts/'local-acquisition.json',acquisition)
                    if selected['status']==first_a.PASSED:
                        identity=selected['identity'][0]
                        if acquisition.get('state')!='verified_source_bundle' or any(acquisition.get(k)!=identity[k] for k in ('bridge_sha256','cua_sha256','cua_version','archive_sha256','bundle_id')):
                            raise ValueError('Acquisition binding')
                except BaseException:code=1;observation_failures.append('baseline_acquisition')
            succeeded=selected['status']==(first_a.PASSED if baseline else 'passed')
            try:checks=cleanup_suite(users,journal,snapshot['cache'],require_profiles=(not baseline and code==0 and succeeded),required_profile_indices=({0} if positive_child else set()) if baseline else None)
            except BaseException:checks={'suite_cleanup':False}
            results[proof]=checks
            creation_observations.extend({'suite':proof,'present':u.get('creation_present')} for u in users)
            users=[]
            if code or not succeeded:
                stage=proof+'_harness'
                raise RuntimeError('Required proof or cleanup failed')
            if not all(checks.values()):raise RuntimeError('Required proof or cleanup failed')
            suites.append(proof)
        passed=True;stage='none'
    except BaseException as error:
        passed=False
        failure_code=error.code if isinstance(error,HostActionError) else 'required_gate'
        failure_detail=error.detail if isinstance(error,HostActionError) else {}
    finally:
        if baseline and _native_owner is not None:
            # Cleanup gets its own reserve after setup/child failure too.
            if not cleanup_started:_native_owner['deadline']=min(parent_deadline,time.monotonic()+first_a.HOST_CLEANUP_SECONDS)
        if wrapper is not None:
            if not wrapper_joined and not wrapper_stop_attempted:
                wrapper_joined=not held_process.abort_sync(wrapper,15)
            results['wrapper']={'owned_stop_and_join':wrapper_joined}
        if snapshot is not None:
            if users:
                try:results['unfinished_suite']=cleanup_suite(users,journal,snapshot['cache'])
                except BaseException:results['unfinished_suite']={'suite_cleanup':False}
                creation_observations.extend({'suite':'unfinished','present':u.get('creation_present')} for u in users)
            actions=[]
            if setup_attempted:
                for i,reg in enumerate(snapshot['registry']):
                    actions.append(('registry_'+str(i),lambda reg=reg:powershell(r'''
if($v.present){Set-ItemProperty $v.path -Name $v.name -Value $v.value}else{Remove-ItemProperty $v.path -Name $v.name -ErrorAction SilentlyContinue}
$p=(Get-ItemProperty $v.path).PSObject.Properties[$v.name]
if(($null -ne $p) -ne $v.present -or ($v.present -and $p.Value -ne $v.value)){throw 'Registry restoration failed'}
''',reg,10)))
                actions.append(('firewall',lambda:powershell("Get-NetFirewallRule -Name $v.rule -ErrorAction SilentlyContinue|Remove-NetFirewallRule;if(Get-NetFirewallRule -Name $v.rule -ErrorAction SilentlyContinue){throw 'Firewall rule remains'}",{'rule':rule},15)))
                actions.append(('service',lambda:powershell(r'''
if($v.state -eq 'Stopped'){Stop-Service TermService -Force}
$mode=switch($v.start_mode){'Auto'{'Automatic'} 'Disabled'{'Disabled'} default{'Manual'}}
Set-Service TermService -StartupType $mode
$s=Get-CimInstance Win32_Service -Filter "Name='TermService'"
if($s.State -ne $v.state -or $s.StartMode -ne $v.start_mode){throw 'Service restoration failed'}
''',snapshot['service'],20)))
            results['host']=cleanup_actions(actions)
        if baseline and _native_owner is not None:
            results['native']=cleanup_actions([('owned_processes_removed',lambda:stop_owned_processes(_native_owner['journal']))])
            results['native']['all_stop_joins_verified']=not _native_owner['stop_failed']
            results['native']['cleanup_within_reserve']=time.monotonic()<=_native_owner['deadline']
            try:
                needles=[str(c['password']).encode() for c in locals().get('credentials',{}).values()]
                scans=[first_a.scan(path,needles,_native_owner['deadline']) for path in private.glob('*-raw')]
                for path in private.glob('*-console.log'):
                    scans.append(not any(n in desktop.read_plain(path,16*1024*1024) for n in needles))
                results['native']['private_evidence_scan']=all(scans)
            except BaseException:results['native']['private_evidence_scan']=False
        try:
            shutil.rmtree(private)
            removed=not private.exists()
        except OSError:removed=False
        results['private_removed']=removed
        if baseline and _native_owner is not None:
            results['native']['cleanup_within_reserve']=time.monotonic()<=_native_owner['deadline']
            cleanup_deadline=_native_owner['deadline']
            _native_owner=None
        clean=removed and all(all(c.values()) for c in results.values() if isinstance(c,dict))
        passed=passed and clean and suites==expected
        if not passed:
            desktop.retain_failed_images(artifacts, observer_source, getattr(args, 'bootstrap_desktop', False))
        roles_artifact={'roles':roles,'memberships':memberships,'creation_observations':creation_observations}
        if baseline:
            try:
                first_a.write_json(artifacts/'account-roles.json',roles_artifact)
                first_a.write_json(artifacts/'cleanup.json',results)
            except BaseException:passed=False;observation_failures.append('selected_artifact_write')
        else:
            (artifacts/'account-roles.json').write_text(json.dumps(roles_artifact,indent=2))
            (artifacts/'cleanup.json').write_text(json.dumps(results,indent=2))
        if baseline:
            try:passed=passed and first_a.scan(artifacts,needles,cleanup_deadline)
            except BaseException:passed=False;observation_failures.append('selected_scan')
            if not passed and (artifacts/'first_a.json').exists():
                try:
                    item=first_a.read_json(artifacts/'first_a.json')
                    if item.get('status')==first_a.PASSED:
                        item.update(status='failed',outcome='inconclusive',failure_stage='cleanup')
                    first_a.write_json(artifacts/'first_a.json',first_a.failed_selected(item))
                except BaseException:
                    try:first_a.write_json(artifacts/'first_a.json',first_a.project(None,None,1,hashes.get('rdpilot-bridge'),False))
                    except BaseException:
                        observation_failures.append('selected_artifact_write')
                        try:(artifacts/'first_a.json').unlink(missing_ok=True)
                        except OSError:observation_failures.append('stale_child_removal')
        gate={'mode':mode, 'status':{'live':'passed','setup_diagnostic':'setup_passed','cua_diagnostic':'cua_diagnostic_passed','protected_first_a':'baseline_first_a_qualified'}[mode] if passed else 'failed','failure_stage':'none' if passed else (stage if stage!='none' else 'cleanup'),'attempted_suites':attempted,'completed_suites':suites,'failure_code':failure_code if not passed else 'none','host_failure':failure_detail if not passed else {},'observation_failures':observation_failures}
        if baseline:
            try:
                # Check all existing selected files before atomically publishing green.
                if not first_a.scan(artifacts,needles,cleanup_deadline):raise ValueError('Selected final scan')
                first_a.write_json(artifacts/'gate.json',gate,needles=needles)
            except BaseException:
                passed=False;gate.update(status='failed',failure_stage='artifact_write_or_scan',failure_code='required_gate')
                try:first_a.write_json(artifacts/'gate.json',gate)
                except BaseException:pass
                try:first_a.write_json(artifacts/'first_a.json',first_a.project(None,None,1,None,False))
                except BaseException:
                    try:(artifacts/'first_a.json').unlink(missing_ok=True)
                    except OSError:observation_failures.append('stale_child_removal')
        else:(artifacts/'gate.json').write_text(json.dumps(gate,indent=2))
    print('Hosted desktop gate '+('passed' if passed else 'failed')+'; stage: '+stage,flush=True)
    return 0 if passed else 1


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',required=True)
    parser.add_argument('--initialize',action='store_true')
    modes=parser.add_mutually_exclusive_group()
    modes.add_argument('--setup-diagnostic',action='store_true',help='host setup only; cannot produce a live gate pass')
    modes.add_argument('--cua-diagnostic',action='store_true',help='full Cua proof only; cannot produce a live gate pass')
    modes.add_argument('--baseline-first-a',action='store_true',help='protected-earlier-startup FIRST-A capability only')
    parser.add_argument('--bin-dir')
    parser.add_argument('--expected-commit')
    parser.add_argument('--bootstrap-footprint', action='store_true', help='read-only failed first-A guest files; Cua diagnostic only')
    parser.add_argument('--bootstrap-desktop', action='store_true', help='private first-A desktop snapshots; Cua diagnostic only')
    args=parser.parse_args()
    if args.initialize:
        initialize(args.output,mode_for(args));return 0
    try:return run(args)
    except BaseException:
        print('Hosted desktop gate failed before setup; safe build-stage artifact retained.')
        return 1


if __name__=='__main__':
    raise SystemExit(main())
