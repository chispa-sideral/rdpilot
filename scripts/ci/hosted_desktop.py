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

HERE = Path(__file__).resolve().parent
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
    try:result = subprocess.run(['powershell.exe','-NoProfile','-NonInteractive','-EncodedCommand',
        base64.b64encode(source.encode('utf-16le')).decode()], env=env, capture_output=True, timeout=timeout)
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


def cleanup_suite(users, journal, cache, require_profiles=False):
    actions=[]
    profile_presence={}
    def stop_processes():
        if require_profiles and not journal.exists():raise RuntimeError('Required process journal absent')
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
            if require_profiles and not present:raise RuntimeError('Required live profile was not observed')
        actions += [('user_'+str(i)+'_sessions',logoff),('user_'+str(i)+'_profile_guest_removed',remove_profile),('user_'+str(i)+'_account',lambda user=user:powershell(REMOVE_USER,user,20))]
    def clear_cache():
        powershell("if(Test-Path $v.path){Remove-Item $v.path -Recurse -Force};if(Test-Path $v.path){throw 'Cache remains'}",{'path':cache},20)
    actions.append(('native_cache_removed',clear_cache))
    results=cleanup_actions(actions)
    # Presence is required only after a completed live proof, not failed setup.
    if require_profiles:results.update(profile_presence)
    return results


def measure_process_refusal():
    child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(120)'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    record=live.process_identity(child.pid)
    try:
        for field,value in (('created',str(int(record['created'])+1)),('image','unowned.exe')):
            refused=False
            try:powershell(STOP_PROCESS,{**record,field:value},10)
            except RuntimeError:refused=True
            if not refused or child.poll() is not None:raise RuntimeError('Owned process refusal was not proven')
    finally:
        powershell(STOP_PROCESS,record,15)
        child.wait(timeout=5)


def private_environment(private):
    return {**os.environ,**{key:str(Path(private).resolve()) for key in ('TMPDIR','TEMP','TMP')}}


def initialize(output):
    output=Path(output)
    output.mkdir(parents=True,exist_ok=False)
    artifacts=output/'artifacts';artifacts.mkdir()
    (artifacts/'gate.json').write_text(json.dumps({'status':'failed','failure_stage':'build_or_dependencies'}))


def run(args):
    output=Path(args.output).resolve();artifacts=output/'artifacts';private=output/'private'
    if not artifacts.is_dir() or private.exists():
        raise RuntimeError('Initialize new gate output first')
    private.mkdir()
    powershell("& icacls $v.path /inheritance:r /grant:r \"$($env:USERDOMAIN)\\$($env:USERNAME):(OI)(CI)F\"|Out-Null;if($LASTEXITCODE){throw 'Private ACL failed'}",{'path':str(private)})
    os.environ.update(private_environment(private))
    snapshot=None;results={};suites=[];attempted=[];users=[];journal=private/'processes.json';rule='RdpilotDesktop-'+secrets.token_hex(8)
    stage='preflight';failure_detail={};failure_code='none';passed=False;setup_attempted=False;roles=[];memberships=[];creation_observations=[]
    try:
        snapshot=powershell(PREFLIGHT)
        measure_process_refusal()
        commit=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
        tree=subprocess.check_output(['git','rev-parse','HEAD^{tree}'],text=True).strip()
        if commit!=args.expected_commit or not all(len(x)==40 and all(c in '0123456789abcdef' for c in x) for x in (commit,tree)):
            raise RuntimeError('Source identity mismatch')
        diagnostic=getattr(args,'setup_diagnostic',False)
        bin_dir=Path(args.bin_dir or '.').resolve()
        hashes={} if diagnostic else {name:hashlib.sha256((bin_dir/(name+'.exe')).read_bytes()).hexdigest() for name in ('rdpilot','rdpilot-daemon','rdpilot-mcp','rdpilot-bridge')}
        environment={k:snapshot[k] for k in ('os','image_os','image_version')}
        # Values below are from local OS/image metadata, never guest/tool text.
        environment.update(source_commit=commit,source_tree=tree,binary_sha256=hashes,
            auth='NLA/CredSSP; explicit self-signed TLS acceptance',graphics='product default',account_role='standard',
            target='loopback',cua_version='0.34.0',process_identity_refusals_verified=True)
        (artifacts/'environment.json').write_text(json.dumps(environment,indent=2))
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
        deadline=time.monotonic()+45*60
        for proof in (('cua',) if diagnostic else live.CHECKS):
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
            child_env=dict(os.environ)
            for label,cred in credentials.items():
                prefix='RDPILOT_TAKE_' if proof=='takeover' else 'RDPILOT_VIEW_'+label.upper()+'_'
                child_env.update({prefix+k.upper():str(v) for k,v in cred.items()})
            stage=proof+'_harness'
            attempted.append(proof)
            command=[sys.executable,str(HERE/'run-live-proof.py'),proof,'--bin-dir',str(bin_dir),'--bundle',str(bundle),
                '--output',str(private/(proof+'-raw')),'--artifacts',str(artifacts),'--source-bridge-sha256',hashes['rdpilot-bridge'],
                '--journal',str(journal),'--credentials-a',str(private/'a.json'),'--credentials-b',str(private/'b.json')]
            remaining=min(900,int(deadline-time.monotonic()))
            if remaining<=0:raise TimeoutError('Work budget expired')
            with (private/(proof+'-console.log')).open('wb') as raw:
                child=subprocess.Popen(command,env=child_env,stdout=raw,stderr=raw)
                # Held wrapper PID is recorded too; timeout cannot orphan its children.
                record=live.process_identity(child.pid)
                try:code=child.wait(timeout=remaining+30)
                except subprocess.TimeoutExpired:
                    powershell(STOP_PROCESS,record,15);code=1
            selected=json.loads((artifacts/(proof+'.json')).read_text()) if (artifacts/(proof+'.json')).exists() else {'status':'failed'}
            stage=proof+'_cleanup'
            try:checks=cleanup_suite(users,journal,snapshot['cache'],require_profiles=(code==0 and selected['status']=='passed'))
            except BaseException:checks={'suite_cleanup':False}
            results[proof]=checks
            creation_observations.extend({'suite':proof,'present':u.get('creation_present')} for u in users)
            users=[]
            if code or selected['status']!='passed':
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
        try:
            shutil.rmtree(private)
            removed=not private.exists()
        except OSError:removed=False
        results['private_removed']=removed
        clean=removed and all(all(c.values()) for c in results.values() if isinstance(c,dict))
        passed=passed and clean and suites==(['host_setup'] if getattr(args,'setup_diagnostic',False) else list(live.CHECKS))
        if not passed:
            for screenshot in artifacts.glob('*.png'):
                screenshot.unlink()
        (artifacts/'account-roles.json').write_text(json.dumps({'roles':roles,'memberships':memberships,'creation_observations':creation_observations},indent=2))
        (artifacts/'cleanup.json').write_text(json.dumps(results,indent=2))
        (artifacts/'gate.json').write_text(json.dumps({'mode':'setup_diagnostic' if getattr(args,'setup_diagnostic',False) else 'live', 'status':('setup_passed' if getattr(args,'setup_diagnostic',False) else 'passed') if passed else 'failed','failure_stage':'none' if passed else (stage if stage!='none' else 'cleanup'),'attempted_suites':attempted,'completed_suites':suites,'failure_code':failure_code if not passed else 'none','host_failure':failure_detail if not passed else {}},indent=2))
    print('Hosted desktop gate '+('passed' if passed else 'failed')+'; stage: '+stage,flush=True)
    return 0 if passed else 1


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',required=True)
    parser.add_argument('--initialize',action='store_true')
    parser.add_argument('--setup-diagnostic',action='store_true',help='host setup only; cannot produce a live gate pass')
    parser.add_argument('--bin-dir')
    parser.add_argument('--expected-commit')
    args=parser.parse_args()
    if args.initialize:
        initialize(args.output);return 0
    try:return run(args)
    except BaseException:
        print('Hosted desktop gate failed before setup; safe build-stage artifact retained.')
        return 1


if __name__=='__main__':
    raise SystemExit(main())
