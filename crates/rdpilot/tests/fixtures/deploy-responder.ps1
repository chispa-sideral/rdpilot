<#
    deploy-responder.ps1 — WinRM deploy/launch/teardown helper for the
    THROWAWAY Phase 4 DVC responder (sensor-responder.ps1, D-4.1/D-4.5).

    Run this BEFORE the gated `sensor_ping_pong_under_500ms` live test. It:
      1. Reads `.secrets/connection.json` (same file + schema the Rust test
         harness reads via `tests/common/mod.rs` — host/user/password/rdpPort/
         winrmPort, written by `infra/manage-env.ps1 up`). The password is
         handed straight to a `PSCredential` and is NEVER printed or logged
         (T-04-06 — mirrors `infra/tests/Validate-Target.ps1`'s convention).
      2. Opens a WinRM `New-PSSession` to the target using the same
         self-signed-lab-cert options Phase 1's tooling already uses
         (`New-PSSessionOption -SkipCACheck -SkipCNCheck`, `-UseSSL`,
         `-Authentication Negotiate` — see infra/tests/Validate-Target.ps1).
      3. Copies `sensor-responder.ps1` to the target (`Copy-Item -ToSession`).
      4. ARMS it to launch INSIDE the INTERACTIVE RDP session (Session > 0),
         NOT WinRM's own session (always Session 0) — otherwise
         WTSVirtualChannelOpenEx has no client-side DVC listener to attach to
         (T-04-08). Mechanism: a Scheduled Task, principal = the interactive
         logon user, trigger = "at log on" for that user, action = launch
         `pwsh -File <copied path>`. When the SDK's RDP connection creates the
         interactive session (or the user is already logged in and the task
         fires immediately via an explicit kick-off below), the task starts
         the responder there and its own retry loop opens the DVC channel.

    NOTE on session targeting (empirically adjustable, D-4.2 Claude's
    discretion): a logon-trigger scheduled task is the recommended mechanism,
    but if the live run shows the responder still landing in the wrong
    session (check with `(Get-Process -Id <pid>).SessionId` on the target),
    swap the trigger for a different one (e.g. an RDP session-connect event,
    Event ID 4778/25 in the Security/TerminalServices-RemoteConnectionManager
    logs) without changing anything else in this file's shape.

    Includes a `-Remove` teardown mode: unregisters the scheduled task and
    deletes the copied script, keeping the responder genuinely throwaway
    (D-4.1) — no residue survives on the VM between live-gate runs.

    Not compiled, not run by `cargo` — a text fixture only. Deleted before
    Phase 5.
#>
[CmdletBinding()]
param(
    [string]$ConnectionFile = (Join-Path $PSScriptRoot '../../../../.secrets/connection.json'),

    # Path to the responder script on THIS machine, to be copied to the target.
    [string]$ResponderScript = (Join-Path $PSScriptRoot 'sensor-responder.ps1'),

    # Destination path on the target where the responder is copied.
    [string]$RemoteScriptPath = 'C:\rdpilot-sensor-responder.ps1',

    # Name of the scheduled task that arms the responder at logon.
    [string]$TaskName = 'RdpilotSensorResponder',

    # Tear down instead of deploying: unregister the task and delete the
    # copied script, leaving no residue on the target (D-4.1 throwaway).
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $ConnectionFile)) {
    throw "Connection file not found: $ConnectionFile — run 'pwsh infra/manage-env.ps1 up' first (writes .secrets/connection.json)."
}

# Same schema as tests/common/mod.rs and infra/tests/Validate-Target.ps1:
#   { "host": "...", "user": "...", "password": "...", "rdpPort": 3389, "winrmPort": 5986 }
$conn = Get-Content $ConnectionFile -Raw | ConvertFrom-Json
$targetHost = $conn.host
$winrmPort = if ($conn.winrmPort) { $conn.winrmPort } else { 5986 }
$user = $conn.user

if (-not $targetHost) {
    throw "'$ConnectionFile' has an empty 'host'. Is the environment up? Run 'pwsh infra/manage-env.ps1 up'."
}

Write-Host "Connecting to $targetHost`:$winrmPort as $user via WinRM..."

# Build the WinRM credential WITHOUT ever echoing the password (T-04-06).
$securePwd = ConvertTo-SecureString $conn.password -AsPlainText -Force
$cred = [System.Management.Automation.PSCredential]::new($user, $securePwd)
Remove-Variable securePwd

# Self-signed lab cert (Phase 1 convention — infra/tests/Validate-Target.ps1
# Pitfall 3): skip CA/CN validation, use SSL, Negotiate auth.
$soPss = New-PSSessionOption -SkipCACheck -SkipCNCheck
$session = New-PSSession -ComputerName $targetHost -Port $winrmPort -UseSSL `
    -Credential $cred -SessionOption $soPss -Authentication Negotiate

try {
    if ($Remove) {
        Write-Host "Tearing down: unregistering scheduled task '$TaskName' and deleting '$RemoteScriptPath'..."
        Invoke-Command -Session $session -ArgumentList $TaskName, $RemoteScriptPath -ScriptBlock {
            param($TaskName, $RemoteScriptPath)
            $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
            if ($existing) {
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
            }
            if (Test-Path $RemoteScriptPath) {
                Remove-Item $RemoteScriptPath -Force
            }
        }
        Write-Host "Teardown complete — the throwaway responder leaves no residue on the target (D-4.1)."
        return
    }

    Write-Host "Copying sensor-responder.ps1 to $targetHost`:$RemoteScriptPath..."
    if (-not (Test-Path $ResponderScript)) {
        throw "Responder script not found locally: $ResponderScript"
    }
    Copy-Item -ToSession $session -Path $ResponderScript -Destination $RemoteScriptPath -Force

    Write-Host "Arming '$TaskName' to launch the responder in the INTERACTIVE session (Session > 0) at logon for '$user'..."
    Invoke-Command -Session $session -ArgumentList $TaskName, $RemoteScriptPath, $user -ScriptBlock {
        param($TaskName, $RemoteScriptPath, $User)

        $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($existing) {
            Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        }

        # Action: launch pwsh with the copied responder script. Runs as the
        # interactive logon user (NOT SYSTEM, NOT the WinRM caller's Session 0)
        # so WTSVirtualChannelOpenEx(WTS_CURRENT_SESSION, ...) resolves to the
        # RDP session, not Session 0 (T-04-08).
        $action = New-ScheduledTaskAction -Execute 'pwsh.exe' `
            -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$RemoteScriptPath`""

        # Trigger: at logon for this specific user — fires when the RDP
        # connection establishes the interactive session. If the user session
        # is already active when this task registers, also kick it off now via
        # Start-ScheduledTask so the live test doesn't have to wait for a
        # fresh logon.
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $User

        $principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Limited

        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
            -Principal $principal -Description 'THROWAWAY Phase 4 DVC validation responder (D-4.1) - deleted before Phase 5.' `
            -Force | Out-Null

        # The RDP session driving this WinRM call's own login may already be
        # active as an interactive logon — start the task immediately too, so
        # a responder is running without requiring a fresh logon event.
        try {
            Start-ScheduledTask -TaskName $TaskName
        } catch {
            Write-Host "Start-ScheduledTask (immediate kick) failed - task will still fire at next logon: $($_.Exception.Message)"
        }
    }

    Write-Host "Responder deployed and armed. Run the gated live test now:"
    Write-Host "  RDPILOT_LIVE=1 cargo test -p rdpilot sensor_ping_pong_under_500ms -- --ignored --test-threads=1"
    Write-Host "If it times out waiting for a pong, verify the responder actually landed in the interactive session"
    Write-Host "(check '(Get-Process -Id <pid>).SessionId' on the target - 0 means the launch mechanism above needs adjusting, see the NOTE at the top of this file)."
} finally {
    Remove-PSSession $session
}
