<#
    deploy-winrm.ps1 — WinRM FALLBACK deploy/launch/teardown helper for the
    REAL rdpilot-sensor.exe (SENSOR-02 SC3, Phase 5, D-5.7).

    Adapted near-verbatim from Phase 4's proven `deploy-responder.ps1`
    mechanism (T-04-08 live-validated), swapping the throwaway PowerShell
    responder payload for the real NativeAOT `rdpilot-sensor.exe` published in
    Plan 01 Task 3. This is the WinRM FALLBACK path (SC3) — independent of and
    NOT a substitute for the RDPDR PRIMARY path (SC2 mandatory, D-5.6;
    `Session::deploy_and_launch` in session.rs).

    Run this BEFORE the gated `sensor_winrm_deploy_and_ping_within_1s` live
    test. It:
      1. Reads `.secrets/connection.json` (same file + schema the Rust test
         harness reads via `tests/common/mod.rs` — host/user/password/rdpPort/
         winrmPort, written by `infra/manage-env.ps1 up`). The password is
         handed straight to a `PSCredential` and is NEVER printed or logged
         (T-04-06 / Security V5 — mirrors deploy-responder.ps1's convention;
         see T-05-09 in 05-04-PLAN.md's threat register).
      2. Opens a WinRM `New-PSSession` to the target using the same
         self-signed-lab-cert options Phase 1's tooling already uses
         (`New-PSSessionOption -SkipCACheck -SkipCNCheck`, `-UseSSL`,
         `-Authentication Negotiate` — see infra/tests/Validate-Target.ps1).
      3. Copies the REAL `rdpilot-sensor.exe` to the target
         (`Copy-Item -ToSession`) — NOT a `.ps1` script this time.
      4. ARMS it to launch INSIDE the INTERACTIVE RDP session (Session > 0),
         NOT WinRM's own session (always Session 0) — otherwise the sensor's
         own WTS P/Invoke channel-open has no client-side DVC listener to
         attach to (T-04-08, carried forward unchanged per D-5.7). Mechanism:
         a Scheduled Task, principal = the interactive logon user, trigger =
         "at log on" for that user, action = launch the copied
         `rdpilot-sensor.exe` DIRECTLY (no `pwsh -File` wrapper — the sensor
         is a native self-contained exe, SC1). When the SDK's RDP connection
         creates the interactive session (or the user is already logged in
         and the task fires immediately via an explicit kick-off below), the
         task starts the sensor there and its own retry loop opens the DVC
         channel.

    NOTE on session targeting (empirically adjustable, carried forward from
    D-4.2/T-04-08): a logon-trigger scheduled task is the recommended
    mechanism, but if the live run shows the sensor still landing in the
    wrong session (check with `(Get-Process -Id <pid>).SessionId` on the
    target), swap the trigger for a different one (e.g. an RDP
    session-connect event, Event ID 4778/25 in the
    Security/TerminalServices-RemoteConnectionManager logs) without changing
    anything else in this file's shape.

    LIVE-MEASURED LATENCY GOTCHA (04-03-SUMMARY.md, RESEARCH Pitfall 5,
    carried forward unchanged): the WinRM-registered AtLogOn scheduled task
    takes ~20-30s from interactive-session creation to actually launching the
    sensor (Task Scheduler's own logon-trigger latency) and does NOT refire on
    a bare RDP *reconnect* to an already-logged-on session — only a fresh
    logon (or the explicit `Start-ScheduledTask` kick below) fires it. The
    gated Rust test's bounded retry/setup budget (60s) absorbs this; the SC4
    "< 1s" bound is measured from the first successful ping AFTER that setup
    settles, not from task registration.

    Includes a `-Remove` teardown mode: unregisters the scheduled task and
    deletes the copied exe, leaving no residue on the VM between live-gate
    runs.

    Includes an OPT-IN `-AddAvExclusion` switch (D-5.5): adds a Windows
    Defender path exclusion for the copied sensor exe in the SAME
    `Invoke-Command` deploy block. Only pass this if the live gate actually
    surfaces an AV/EDR block (e.g. the copied exe silently disappears or the
    scheduled task's process never starts) — it is NOT enabled proactively.

    Not compiled, not run by `cargo` — a text fixture only.
#>
[CmdletBinding()]
param(
    [string]$ConnectionFile = (Join-Path $PSScriptRoot '../../../../.secrets/connection.json'),

    # Path to the published NativeAOT sensor exe on THIS machine (or the
    # Windows build host), to be copied to the target. Defaults to the
    # Plan 01 Task 3 NativeAOT publish output location.
    [string]$SensorExe = (Join-Path $PSScriptRoot '../../../../sensor/bin/Release/net8.0/win-x64/publish/rdpilot-sensor.exe'),

    # Destination path on the target where the sensor exe is copied.
    [string]$RemoteExePath = 'C:\rdpilot\rdpilot-sensor.exe',

    # Name of the scheduled task that arms the sensor at logon.
    [string]$TaskName = 'RdpilotSensorWinrm',

    # D-5.5: only add a Defender exclusion if the live gate actually surfaces
    # an AV/EDR block. Never enabled proactively.
    [switch]$AddAvExclusion,

    # Tear down instead of deploying: unregister the task and delete the
    # copied exe, leaving no residue on the target.
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

# Build the WinRM credential WITHOUT ever echoing the password (T-04-06 /
# T-05-09 — never interpolate $conn.password or $cred into any output line).
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
        Write-Host "Tearing down: unregistering scheduled task '$TaskName' and deleting '$RemoteExePath'..."
        Invoke-Command -Session $session -ArgumentList $TaskName, $RemoteExePath -ScriptBlock {
            param($TaskName, $RemoteExePath)
            $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
            if ($existing) {
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
            }
            if (Test-Path $RemoteExePath) {
                Remove-Item $RemoteExePath -Force
            }
        }
        Write-Host "Teardown complete — no residue left on the target."
        return
    }

    Write-Host "Copying rdpilot-sensor.exe to $targetHost`:$RemoteExePath..."
    if (-not (Test-Path $SensorExe)) {
        throw "Sensor exe not found locally: $SensorExe — publish it first (Plan 01 Task 3: 'dotnet publish -r win-x64 -p:PublishAot=true --self-contained' on a Windows host with the .NET 8 SDK)."
    }
    $remoteDir = Split-Path -Parent $RemoteExePath
    Invoke-Command -Session $session -ArgumentList $remoteDir -ScriptBlock {
        param($RemoteDir)
        if (-not (Test-Path $RemoteDir)) {
            New-Item -ItemType Directory -Path $RemoteDir -Force | Out-Null
        }
    }
    Copy-Item -ToSession $session -Path $SensorExe -Destination $RemoteExePath -Force

    if ($AddAvExclusion) {
        Write-Host "D-5.5: adding a Windows Defender path exclusion for '$RemoteExePath' (opt-in, only because the live gate surfaced an AV/EDR block)..."
        Invoke-Command -Session $session -ArgumentList $RemoteExePath -ScriptBlock {
            param($RemoteExePath)
            Add-MpPreference -ExclusionPath $RemoteExePath -ErrorAction SilentlyContinue
        }
    }

    Write-Host "Arming '$TaskName' to launch the sensor in the INTERACTIVE session (Session > 0) at logon for '$user'..."
    Invoke-Command -Session $session -ArgumentList $TaskName, $RemoteExePath, $user -ScriptBlock {
        param($TaskName, $RemoteExePath, $User)

        $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($existing) {
            Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        }

        # Action: launch the copied sensor exe DIRECTLY — it is a
        # self-contained NativeAOT native binary (SC1), no `pwsh -File`
        # wrapper needed. Runs as the interactive logon user (NOT SYSTEM,
        # NOT the WinRM caller's Session 0) so the sensor's WTS channel-open
        # resolves to the RDP session, not Session 0 (T-04-08).
        $action = New-ScheduledTaskAction -Execute $RemoteExePath

        # Trigger: at logon for this specific user — fires when the RDP
        # connection establishes the interactive session. If the user session
        # is already active when this task registers, also kick it off now via
        # Start-ScheduledTask so the live test doesn't have to wait for a
        # fresh logon (does NOT refire on a bare reconnect — RESEARCH
        # Pitfall 5, 04-03-SUMMARY.md).
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $User

        $principal = New-ScheduledTaskPrincipal -UserId $User -LogonType Interactive -RunLevel Limited

        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
            -Principal $principal -Description 'rdpilot-sensor.exe WinRM fallback launcher (SC3, Phase 5).' `
            -Force | Out-Null

        # The RDP session driving this WinRM call's own login may already be
        # active as an interactive logon — start the task immediately too, so
        # the sensor is running without requiring a fresh logon event.
        try {
            Start-ScheduledTask -TaskName $TaskName
        } catch {
            Write-Host "Start-ScheduledTask (immediate kick) failed - task will still fire at next logon: $($_.Exception.Message)"
        }
    }

    Write-Host "Sensor deployed and armed. Run the gated live test now:"
    Write-Host "  RDPILOT_LIVE=1 cargo test -p rdpilot sensor_winrm_deploy_and_ping_within_1s -- --ignored --test-threads=1"
    Write-Host "If it times out waiting for a pong, verify the sensor actually landed in the interactive session"
    Write-Host "(check '(Get-Process -Id <pid>).SessionId' on the target - 0 means the launch mechanism above needs adjusting, see the NOTE at the top of this file)."
    Write-Host "If the exe was silently quarantined/deleted, re-run with -AddAvExclusion (D-5.5)."
} finally {
    Remove-PSSession $session
}
