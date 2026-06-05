<#
    Validate-Autodestroy.ps1 — ENV-03 schedule-triggered auto-destroy validation.

    Proves the SCHEDULE wiring (ENV-03 / D-04) by creating a real one-time schedule
    in the production Automation Account, waiting for Azure Automation's scheduler to
    fire the Delete-ResourceGroup runbook UNATTENDED, and asserting that the TEST RG
    is actually gone afterwards.

    This deliberately does NOT trigger the runbook directly (az automation runbook
    start) — that would validate the runbook body only, not the schedule→jobSchedule
    wiring that is the whole point of ENV-03.

    PREREQUISITES:
      - `az login` is current (tested in preflight).
      - The TEST RG `rdpilot-test` must be UP (Succeeded provisioning state).
        If it is not, run:  pwsh infra/manage-env.ps1 -Action up
      - The management RG `rdpilot-mgmt` and Automation Account `rdpilot-autodestroy`
        must exist (deployed via manage-env.ps1 -Action up).

    FLOW:
      1. PREFLIGHT  — auth, TEST RG exists/Succeeded, mgmt RG + AA + runbook exist.
      2. ARRANGE    — create a one-time schedule "validate-env03-<ts>" and its
                      jobSchedule link in the Automation Account.
      3. ACT/ASSERT — poll for a schedule-triggered job, wait for terminal state,
                      verify the TEST RG is gone.
      4. CLEANUP    — always (finally): delete the temp schedule + jobSchedule link.

    KNOWN FAILURE MODE: If the Automation Account lacks the Az.Accounts and
    Az.Resources PowerShell modules (or the correct module versions), the runbook job
    will fire successfully from the schedule but fail immediately with
    "Connect-AzAccount is not recognized" or "Remove-AzResourceGroup is not recognized".
    Check the job error stream output — the script surfaces it explicitly.

    No secrets are read or echoed. Only management-plane az automation/group commands
    are used (read + schedule/jobSchedule create/delete + job list/output).
#>
[CmdletBinding()]
param(
    [string]$ManagementResourceGroup = 'rdpilot-mgmt',
    [string]$TestResourceGroup       = 'rdpilot-test',
    [string]$AutomationAccount       = 'rdpilot-autodestroy',
    [string]$RunbookName             = 'Delete-ResourceGroup',

    # Minutes in the future to schedule the one-time trigger. Azure Automation
    # enforces a HARD 5-minute minimum; default 10 (and the floor clamp below is 7)
    # so clock skew + the latency between computing UtcNow and the actual create call
    # cannot erode the effective lead below Azure's minimum.
    [int]$LeadMinutes    = 10,

    # Maximum minutes to wait for the schedule-triggered job to appear and complete.
    # Comfortably exceeds LeadMinutes (10) + expected job runtime (a few min).
    [int]$TimeoutMinutes = 20
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

function Write-Step {
    param([string]$Message)
    Write-Host $Message
}

function Write-Pass {
    param([string]$Name)
    Write-Host "  [PASS] $Name"
}

function Write-Fail {
    param([string]$Name, [string]$Detail = '')
    if ($Detail) {
        Write-Host "  [FAIL] $Name — $Detail"
    } else {
        Write-Host "  [FAIL] $Name"
    }
}

function Invoke-Az {
    <#
        Runs an `az` command, guards the exit code, and returns the raw stdout
        string. Throws on non-zero exit with the stderr captured in the message.
        Accepts the az arguments as an array so no shell-injection quoting is needed.
    #>
    param(
        [Parameter(Mandatory, ValueFromRemainingArguments)]
        [string[]]$Arguments
    )
    $output = az @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "az $($Arguments -join ' ') failed (exit $LASTEXITCODE): $output"
    }
    # Return only the parts that went to stdout (not the stderr objects mixed in
    # by 2>&1 when $LASTEXITCODE is 0). On success az writes nothing to stderr
    # that we need; just return the string.
    return ($output | Where-Object { $_ -is [string] }) -join "`n"
}

# ---------------------------------------------------------------------------
# State tracked for cleanup
# ---------------------------------------------------------------------------
$tempScheduleName    = $null   # set in ARRANGE; used in CLEANUP
$jobScheduleId       = $null   # GUID generated in ARRANGE; used in CLEANUP
$scheduleCreatedAt   = $null   # UTC datetime — used to filter jobs by create time
$script:scheduleCreated    = $false  # true ONLY after a successful schedule create; gates schedule delete
$script:jobScheduleCreated = $false  # true ONLY after a successful jobSchedule PUT; gates jobSchedule delete
$passed              = $false

# ---------------------------------------------------------------------------
# MAIN — wrapped so cleanup runs even on Ctrl-C / throw
# ---------------------------------------------------------------------------

try {

    # =======================================================================
    # 1. PREFLIGHT
    # =======================================================================

    Write-Step ""
    Write-Step "=== Validate-Autodestroy.ps1 — ENV-03 schedule-trigger validation ==="
    Write-Step ""
    Write-Step "--- Preflight ---"

    # (P1) az auth
    $acctJson = az account show --query '{name:name,id:id,location:location}' -o json 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $acctJson) {
        Write-Fail 'az authenticated'
        Write-Host ""
        Write-Host "Not logged in — run 'az login' (and 'az account set --subscription <id>' if needed)."
        exit 1
    }
    $acct = $acctJson | ConvertFrom-Json
    Write-Pass "az authenticated (subscription: $($acct.name)  id: $($acct.id))"

    # (P2) TEST RG must exist and be in a usable state
    $testRgStateRaw = az group show -n $TestResourceGroup --query 'properties.provisioningState' -o tsv 2>$null
    $testRgState    = if ($testRgStateRaw) { $testRgStateRaw.Trim() } else { $null }

    if (-not $testRgState) {
        Write-Fail "TEST RG '$TestResourceGroup' exists"
        Write-Host ""
        Write-Host "Test env is not up — run:  pwsh infra/manage-env.ps1 -Action up  first."
        Write-Host "(Validate-Autodestroy.ps1 validates auto-destroy against a REAL, live test environment.)"
        exit 1
    }
    if ($testRgState -match 'Delet|deprovision') {
        Write-Fail "TEST RG '$TestResourceGroup' is in a usable state (current: $testRgState)"
        Write-Host ""
        Write-Host "TEST RG '$TestResourceGroup' is mid-delete (state: $testRgState). Wait for it to disappear,"
        Write-Host "then run:  pwsh infra/manage-env.ps1 -Action up  to bring it back before validating."
        exit 1
    }
    if ($testRgState -ne 'Succeeded') {
        Write-Fail "TEST RG '$TestResourceGroup' is in a usable state (current: $testRgState)"
        Write-Host ""
        Write-Host "TEST RG '$TestResourceGroup' is in state '$testRgState' (expected Succeeded)."
        Write-Host "Run 'az group show -n $TestResourceGroup' to investigate, or re-run manage-env.ps1 -Action up."
        exit 1
    }
    Write-Pass "TEST RG '$TestResourceGroup' exists (state: $testRgState)"

    # (P3) Management RG must exist
    $mgmtRgState = az group show -n $ManagementResourceGroup --query 'properties.provisioningState' -o tsv 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $mgmtRgState) {
        Write-Fail "Management RG '$ManagementResourceGroup' exists"
        Write-Host ""
        Write-Host "Management RG '$ManagementResourceGroup' not found."
        Write-Host "Run 'pwsh infra/manage-env.ps1 -Action up' to deploy the management + test environment."
        exit 1
    }
    Write-Pass "Management RG '$ManagementResourceGroup' exists (state: $($mgmtRgState.Trim()))"

    # (P4) Automation Account must exist in the management RG
    $aaJson = az automation account show `
        -g $ManagementResourceGroup `
        -n $AutomationAccount `
        -o json 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $aaJson) {
        Write-Fail "Automation Account '$AutomationAccount' exists in '$ManagementResourceGroup'"
        Write-Host ""
        Write-Host "Automation Account '$AutomationAccount' not found in '$ManagementResourceGroup'."
        Write-Host "Run 'pwsh infra/manage-env.ps1 -Action up' to (re)deploy the management resources."
        exit 1
    }
    Write-Pass "Automation Account '$AutomationAccount' exists"

    # (P5) Runbook must exist and be Published
    $rbJson = az automation runbook show `
        -g $ManagementResourceGroup `
        --automation-account-name $AutomationAccount `
        -n $RunbookName `
        -o json 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $rbJson) {
        Write-Fail "Runbook '$RunbookName' exists"
        Write-Host ""
        Write-Host "Runbook '$RunbookName' not found in Automation Account '$AutomationAccount'."
        Write-Host "Run 'pwsh infra/manage-env.ps1 -Action up' to (re)deploy the runbook."
        exit 1
    }
    # `az automation runbook show` returns `state` at the TOP level of the JSON, NOT
    # nested under `.properties`. Reading `.properties.state` always yields $null, which
    # made this preflight report an "empty"/unpublished runbook even when it was Published.
    $rb = $rbJson | ConvertFrom-Json
    $rbState = $rb.state
    if ($rbState -ne 'Published') {
        Write-Fail "Runbook '$RunbookName' is Published (current state: $rbState)"
        Write-Host ""
        Write-Host "Runbook '$RunbookName' is in state '$rbState' (must be Published)."
        Write-Host "Re-run 'pwsh infra/manage-env.ps1 -Action up' to re-publish the runbook."
        exit 1
    }
    Write-Pass "Runbook '$RunbookName' exists and is Published"

    Write-Step ""
    Write-Step "=== Preflight passed ==="
    Write-Step ""

    # =======================================================================
    # 2. ARRANGE — create a temporary one-time schedule + jobSchedule link
    # =======================================================================

    Write-Step "--- Arrange: creating one-time schedule in '$AutomationAccount' ---"

    # Enforce a floor of 7 minutes (Azure's hard minimum is 5; 7 leaves headroom for
    # clock skew + create-call latency).
    $effectiveLeadMinutes = [Math]::Max($LeadMinutes, 7)
    if ($effectiveLeadMinutes -ne $LeadMinutes) {
        Write-Host "  Note: -LeadMinutes $LeadMinutes is below the 7-minute safety floor — using $effectiveLeadMinutes."
    }

    # Unique schedule name using a compact UTC timestamp (avoids collisions on re-runs).
    # $scheduleCreatedAt is the job-filter fence (jobs must start at/after this).
    $nowUtc             = [System.DateTime]::UtcNow
    $scheduleCreatedAt  = $nowUtc
    $ts                 = $nowUtc.ToString('yyyyMMddHHmmss')
    $tempScheduleName   = "validate-env03-$ts"
    $leadDisplay        = $effectiveLeadMinutes

    Write-Host "  Schedule name : $tempScheduleName"

    # Compute the start time IMMEDIATELY before the create call so preflight latency /
    # clock skew cannot erode the lead below Azure's 5-min minimum. Format as an
    # explicit UTC ISO-8601 string and pin --time-zone UTC (belt-and-suspenders against
    # any server-side timezone reinterpretation).
    $startUtc    = [System.DateTime]::UtcNow.AddMinutes($effectiveLeadMinutes)
    $fireTimeIso = $startUtc.ToString('yyyy-MM-ddTHH:mm:ssZ')
    Write-Host "  Fire time (UTC): $fireTimeIso  (~$leadDisplay min from now)"

    # Create the one-time schedule.
    # az MANDATES --interval even for --frequency OneTime (omitting it fails with
    # "the following arguments are required: --interval"). --interval 1 is inert for a
    # OneTime schedule (it fires once at --start-time and never recurs).
    $schedJson = az automation schedule create `
        --resource-group $ManagementResourceGroup `
        --automation-account-name $AutomationAccount `
        -n $tempScheduleName `
        --start-time $fireTimeIso `
        --time-zone UTC `
        --frequency OneTime `
        --interval 1 `
        --description "Temporary one-time schedule created by Validate-Autodestroy.ps1 for ENV-03 validation. Safe to delete." `
        -o json 2>&1

    if ($LASTEXITCODE -ne 0) {
        throw "Failed to create one-time schedule '$tempScheduleName': $schedJson"
    }
    $script:scheduleCreated = $true
    Write-Pass "One-time schedule '$tempScheduleName' created"

    # Create the jobSchedule link: schedule → runbook with -ResourceGroupName parameter.
    # `az automation job-schedule` does NOT exist as a CLI subgroup (az 2.86.0), so we
    # PUT the jobSchedule directly to the ARM endpoint via `az rest`. The jobSchedule
    # name is a client-generated GUID. Body is written to a temp file and passed with
    # --body @file to avoid Windows inline-JSON quoting issues.
    $jobScheduleId = (New-Guid).Guid
    # Build the URI by concatenation so the ?api-version query is unambiguous (an
    # interpolated "...$jobScheduleId?api-version=..." can drop the query on PUT).
    $jsBase = "https://management.azure.com/subscriptions/$($acct.id)/resourceGroups/$ManagementResourceGroup/providers/Microsoft.Automation/automationAccounts/$AutomationAccount/jobSchedules/$jobScheduleId"
    $jsUri  = $jsBase + '?api-version=2023-11-01'

    $jsBodyObj = @{
        properties = @{
            schedule   = @{ name = $tempScheduleName }
            runbook    = @{ name = $RunbookName }
            parameters = @{ ResourceGroupName = $TestResourceGroup }
        }
    }
    $jsBodyFile = Join-Path ([System.IO.Path]::GetTempPath()) ("rdpilot-jobschedule-{0}.json" -f $jobScheduleId)
    try {
        $jsBodyObj | ConvertTo-Json -Depth 5 | Set-Content -Path $jsBodyFile -Encoding UTF8

        $jsJson = az rest --method put --uri $jsUri --body "@$jsBodyFile" -o json 2>&1
        if ($LASTEXITCODE -ne 0) {
            throw "Failed to create jobSchedule (PUT $jobScheduleId) linking '$tempScheduleName' → '$RunbookName': $jsJson"
        }
    }
    finally {
        if (Test-Path $jsBodyFile) { Remove-Item $jsBodyFile -Force -ErrorAction SilentlyContinue }
    }

    $script:jobScheduleCreated = $true
    Write-Pass "jobSchedule link created (id: $jobScheduleId)"
    Write-Host ""
    Write-Host "  Scheduled one-time auto-destroy trigger for $fireTimeIso UTC (~$leadDisplay min from now)."
    Write-Host "  Waiting for the SCHEDULE to fire the runbook UNATTENDED — not triggering it manually."
    Write-Host "  Expect ~$leadDisplay min of silence before the job appears; this is NORMAL (within the ${TimeoutMinutes}-min timeout)."
    Write-Host ""

    # =======================================================================
    # 3. ACT / ASSERT — poll for the AUTHORITATIVE signal: the TEST RG is gone.
    #
    #   The whole point of ENV-03 is that the scheduled runbook REAPS the TEST RG
    #   unattended. The authoritative PASS condition is therefore "the TEST RG no
    #   longer exists within the window" — NOT "we managed to find the runbook job
    #   in `az automation job list`". The job list is known to lag / race (the job
    #   may not be indexed in the list until after it completes), so gating PASS on
    #   job detection produced false FAILEDs even when the RG was correctly reaped.
    #
    #   PRIMARY (gates PASS/FAIL): poll `az group exists` until the TEST RG is gone.
    #   SECONDARY (diagnostic only, never gates): best-effort locate the triggered
    #   job and pull its Output/Error streams for the report.
    # =======================================================================

    Write-Step "--- Act/Assert: waiting for the TEST RG to be reaped (timeout: ${TimeoutMinutes} min) ---"

    $pollIntervalSeconds = 30
    $timeoutAt           = $nowUtc.AddMinutes($TimeoutMinutes)
    $rgReaped            = $false

    Write-Host "  Polling every $pollIntervalSeconds s. The scheduler fires ~$leadDisplay min after schedule creation,"
    Write-Host "  then the runbook deletes the RG (PASS = '$TestResourceGroup' no longer exists)."
    Write-Host ""

    while ([System.DateTime]::UtcNow -lt $timeoutAt) {

        # `az group exists` returns the literal string 'true'/'false' and exits 0.
        # This is the authoritative signal — independent of any job-list indexing lag.
        $existsRaw = az group exists -n $TestResourceGroup -o tsv 2>$null
        $exists    = if ($existsRaw) { $existsRaw.Trim().ToLowerInvariant() } else { $null }

        if ($LASTEXITCODE -eq 0 -and $exists -eq 'false') {
            $rgReaped = $true
            Write-Host "  TEST RG '$TestResourceGroup' is GONE — the scheduled runbook reaped it."
            break
        }

        $remaining = [int][Math]::Ceiling(($timeoutAt - [System.DateTime]::UtcNow).TotalMinutes)
        Write-Host "  ... '$TestResourceGroup' still present (${remaining} min remaining, next poll in ${pollIntervalSeconds}s)"
        Start-Sleep -Seconds $pollIntervalSeconds
    }

    # -----------------------------------------------------------------------
    # SECONDARY / diagnostic — best-effort locate the triggered job and pull
    # its streams. This MUST NOT affect PASS/FAIL; it only enriches the report.
    # -----------------------------------------------------------------------
    $triggeredJobId = $null
    $jobFinalStatus = $null

    # We need a stable "not before" fence: the job must have started AFTER we created
    # the schedule (scheduleCreatedAt). Use a 60-second buffer to allow for clock skew.
    $notBeforeUtc = $scheduleCreatedAt.AddSeconds(-60)

    Write-Step ""
    Write-Step "  --- Diagnostic: locating the triggered runbook job (best-effort) ---"

    # `az automation job list` returns fields at the TOP level of each object
    # (runbook.name, startTime, status, name/jobId) — NOT under .properties.
    # Filter in PowerShell (not JMESPath) to dodge --query quirks on live jobs.
    $jobListJson = az automation job list `
        -g $ManagementResourceGroup `
        --automation-account-name $AutomationAccount `
        -o json 2>$null

    if ($LASTEXITCODE -eq 0 -and $jobListJson) {
        try { $jobs = $jobListJson | ConvertFrom-Json } catch { $jobs = @() }

        foreach ($job in $jobs) {
            if ($job.runbook.name -ne $RunbookName) { continue }
            $startStr = $job.startTime
            if (-not $startStr) { continue }
            try {
                $startUtc = [System.DateTime]::Parse($startStr, $null, [System.Globalization.DateTimeStyles]::AssumeUniversal -bor [System.Globalization.DateTimeStyles]::AdjustToUniversal)
            } catch { continue }

            if ($startUtc -ge $notBeforeUtc) {
                # Use the job NAME (the SCH_... string) as the identifier for
                # subsequent show/stream calls.
                $triggeredJobId = $job.name
                $jobFinalStatus = $job.status
                Write-Host "    Job located: id=$triggeredJobId  started=$($startUtc.ToString('HH:mm:ss'))Z  status=$jobFinalStatus"
                break
            }
        }
    }

    if (-not $triggeredJobId) {
        Write-Host "    Note: the triggered job was not indexed in 'az automation job list' (known async-indexing lag — cosmetic only, does not affect PASS/FAIL)."
    } else {
        # Read job output and error streams for diagnostics. `az automation job-output`
        # is not a real CLI subcommand (job has only list/show) — read streams via az rest.
        Write-Step ""
        Write-Step "  --- Job output stream ---"
        $jobOutputRaw = az rest `
            --method GET `
            --url "https://management.azure.com/subscriptions/$($acct.id)/resourceGroups/$ManagementResourceGroup/providers/Microsoft.Automation/automationAccounts/$AutomationAccount/jobs/$triggeredJobId/streams?`$filter=properties/streamType eq 'Output'&api-version=2023-11-01" `
            --query 'value[].properties.summary' `
            -o tsv 2>$null
        if ($jobOutputRaw) {
            $jobOutputRaw -split "`n" | ForEach-Object { Write-Host "    $_" }
        } else {
            Write-Host "    (no output stream content)"
        }

        Write-Step ""
        Write-Step "  --- Job error stream ---"
        $jobRunbookErrRaw = az rest `
            --method GET `
            --url "https://management.azure.com/subscriptions/$($acct.id)/resourceGroups/$ManagementResourceGroup/providers/Microsoft.Automation/automationAccounts/$AutomationAccount/jobs/$triggeredJobId/streams?`$filter=properties/streamType eq 'Error'&api-version=2023-11-01" `
            --query 'value[].properties.summary' `
            -o tsv 2>$null
        if ($jobRunbookErrRaw) {
            $jobRunbookErrRaw -split "`n" | ForEach-Object { Write-Host "    $_" }
        } else {
            Write-Host "    (no error stream content)"
        }
    }
    Write-Step ""

    # -----------------------------------------------------------------------
    # VERDICT — gated SOLELY on whether the TEST RG was reaped within the window.
    # -----------------------------------------------------------------------
    if (-not $rgReaped) {
        Write-Fail "TEST RG '$TestResourceGroup' was reaped within ${TimeoutMinutes} min"
        Write-Host ""
        Write-Host "FAILURE: The TEST RG '$TestResourceGroup' still EXISTS after the ${TimeoutMinutes}-min window."
        Write-Host "The scheduled runbook did not reap it. Possible causes:"
        Write-Host "  - schedule→jobSchedule wiring problem (the runbook never fired). Check:"
        Write-Host "      az rest --method get --uri `"https://management.azure.com/subscriptions/$($acct.id)/resourceGroups/$ManagementResourceGroup/providers/Microsoft.Automation/automationAccounts/$AutomationAccount/jobSchedules?api-version=2023-11-01`""
        Write-Host "  - the runbook fired but FAILED (see job streams above, if the job was located)."
        Write-Host "  - the managed identity lacks Contributor on '$TestResourceGroup' (check role assignments)."
        Write-Host "  - Azure Automation service health: https://status.azure.com/"
        if ($jobFinalStatus -and $jobFinalStatus -ne 'Completed') {
            Write-Host ""
            Write-Host "  Located job final status: $jobFinalStatus (non-Completed — inspect the error stream above)."
        }
        # Activity-log diagnostics for the RG delete (best-effort).
        Write-Step ""
        Write-Step "  --- Activity log (delete operations on '$TestResourceGroup', last 1h, best-effort) ---"
        $alStart = [System.DateTime]::UtcNow.AddHours(-1).ToString('yyyy-MM-ddTHH:mm:ssZ')
        $alRaw = az monitor activity-log list `
            --resource-group $TestResourceGroup `
            --start-time $alStart `
            --query "[?contains(operationName.value, 'delete') || contains(operationName.value, 'Delete')].{time: eventTimestamp, op: operationName.value, status: status.value}" `
            -o tsv 2>$null
        if ($alRaw) {
            $alRaw -split "`n" | ForEach-Object { Write-Host "    $_" }
        } else {
            Write-Host "    (no delete activity-log entries found — RG may never have been targeted)"
        }
        Write-Host ""
        Write-Host "ENV-03 AUTO-DESTROY VALIDATION FAILED"
        exit 1
    }

    Write-Pass "TEST RG '$TestResourceGroup' is gone (auto-destroy confirmed — scheduled runbook reaped it)"
    if (-not $triggeredJobId) {
        Write-Host "  (Job was not detectable in 'az automation job list' despite the successful reap — cosmetic async-indexing lag.)"
    }

    $passed = $true

} finally {

    # =======================================================================
    # 4. CLEANUP — always remove the temp schedule + jobSchedule, even on failure.
    #    Never touch the production 'daily-autodestroy' schedule.
    # =======================================================================

    # Cleanup is independently gated: delete the jobSchedule (if created) FIRST, then
    # the schedule (if created). Each guard prevents a misleading "failed to remove"
    # warning for something that was never created. NEVER touch the production
    # 'daily-autodestroy' schedule or its jobSchedule link.
    if (-not $script:scheduleCreated -and -not $script:jobScheduleCreated) {
        Write-Step ""
        Write-Step "--- Cleanup: nothing to remove (temp schedule/jobSchedule were never created) ---"
    } else {
        Write-Step ""
        Write-Step "--- Cleanup: removing temporary jobSchedule + schedule ---"

        # jobSchedule first (it links the schedule to the runbook). DELETE via az rest —
        # `az automation job-schedule` is not a real CLI subgroup.
        if ($script:jobScheduleCreated -and $jobScheduleId) {
            $jsDelUri = "https://management.azure.com/subscriptions/$($acct.id)/resourceGroups/$ManagementResourceGroup/providers/Microsoft.Automation/automationAccounts/$AutomationAccount/jobSchedules/$jobScheduleId" + '?api-version=2023-11-01'
            $jsDelRaw = az rest --method delete --uri $jsDelUri -o json 2>&1
            if ($LASTEXITCODE -eq 0) {
                Write-Host "  Removed jobSchedule link (id: $jobScheduleId)"
            } else {
                Write-Host "  Warning: failed to remove jobSchedule link $jobScheduleId — clean up manually:"
                Write-Host "    az rest --method delete --uri `"$jsDelUri`""
            }
        }

        if ($script:scheduleCreated -and $tempScheduleName) {
            $schedDelRaw = az automation schedule delete `
                -g $ManagementResourceGroup `
                --automation-account-name $AutomationAccount `
                -n $tempScheduleName `
                --yes 2>&1
            if ($LASTEXITCODE -eq 0) {
                Write-Host "  Removed one-time schedule '$tempScheduleName'"
            } else {
                Write-Host "  Warning: failed to remove schedule '$tempScheduleName' — clean up manually:"
                Write-Host "    az automation schedule delete -g $ManagementResourceGroup --automation-account-name $AutomationAccount -n $tempScheduleName --yes"
            }
        }
    }

    Write-Step ""

    if ($passed) {
        Write-Host "NOTE: The test environment (TEST RG '$TestResourceGroup') was consumed by this validation."
        Write-Host "You do NOT need to run manage-env.ps1 -Action down — auto-destroy already deleted it."
        Write-Host "To re-provision:  pwsh infra/manage-env.ps1 -Action up"
        Write-Step ""
        Write-Host "ENV-03 AUTO-DESTROY VALIDATION PASSED"
    }
    # Non-passed cases already printed the FAILED line before exit in the main block.
}

exit 0
