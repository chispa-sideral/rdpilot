<#
    Delete-ResourceGroup.ps1 — Azure Automation runbook body (auto-destroy, ENV-03).

    Topology: SEPARATE-MANAGEMENT. This runbook lives in a persistent management
    Automation Account that OUTLIVES its target. It deletes the disposable TEST
    resource group on a daily schedule so a forgotten box cannot run up cost even
    if the developer machine is off (D-04). Because the deleter is not inside the
    RG it deletes, the job host survives the delete and the final job status is
    reported cleanly (avoids the self-delete caveat — RESEARCH.md Pitfall 4).

    Auth: SYSTEM-ASSIGNED MANAGED IDENTITY only. RunAs accounts are retired
    (Sep 2023) — do NOT use them. The identity is granted Contributor scoped to
    the TEST resource group ONLY (tightest least-privilege) via a roleAssignment
    in main.bicep — never the subscription full-access role.

    Invoked by the daily jobSchedule in main.bicep, which passes the target RG
    name as the -ResourceGroupName parameter.
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$ResourceGroupName,

    # Retry tuning for transient RBAC-propagation / authorization failures.
    [int]$MaxAttempts = 5,
    [int]$RetryDelaySeconds = 45
)

$ErrorActionPreference = 'Stop'

# Do not persist the Az context to the runbook sandbox profile between jobs.
Disable-AzContextAutosave -Scope Process | Out-Null

# True if the error looks like a transient RBAC-propagation / authorization failure.
# When the role assignment was created very recently (e.g. seconds before this run,
# as in the validation window), ARM can return 403 Forbidden / "does not have
# authorization" mid-operation even though the grant exists and becomes effective
# moments later. For the REAL daily schedule (fires +1 day after deploy) this is a
# non-issue, but the retry makes the safety net robust either way.
function Test-TransientAuthError {
    param([System.Management.Automation.ErrorRecord]$ErrorRecord)
    $text = "$($ErrorRecord.Exception.Message)"
    return ($text -match '(?i)403|Forbidden|AuthorizationFailed|does not have authorization|RBAC|not authorized')
}

# Retry-with-backoff wrapper. Treats an already-deleted RG as success (idempotent),
# retries on transient auth failures, and re-throws non-transient errors immediately.
function Invoke-WithRetry {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$Description,
        [int]$MaxAttempts,
        [int]$RetryDelaySeconds
    )
    for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
        try {
            Write-Output "[$Description] attempt $attempt of $MaxAttempts..."
            & $Action
            Write-Output "[$Description] succeeded on attempt $attempt."
            return
        }
        catch {
            $isTransient = Test-TransientAuthError -ErrorRecord $_
            if ($attempt -ge $MaxAttempts -or -not $isTransient) {
                Write-Output "[$Description] failed on attempt $attempt (transient=$isTransient): $($_.Exception.Message)"
                throw
            }
            Write-Output "[$Description] transient failure on attempt ${attempt}: $($_.Exception.Message). Retrying in $RetryDelaySeconds s..."
            Start-Sleep -Seconds $RetryDelaySeconds
        }
    }
}

# Authenticate as the Automation Account's system-assigned managed identity. The
# identity / token endpoint can itself be momentarily unavailable on a cold sandbox,
# so wrap it in the same retry.
Invoke-WithRetry -Description 'Connect-AzAccount (managed identity)' -MaxAttempts $MaxAttempts -RetryDelaySeconds $RetryDelaySeconds -Action {
    Connect-AzAccount -Identity -ErrorAction Stop | Out-Null
}

# Idempotent, retry-tolerant delete. If the RG is already gone (a prior `down` or a
# previous attempt that actually completed server-side despite a client-side error),
# that is a no-op SUCCESS rather than a failure.
Invoke-WithRetry -Description "Remove-AzResourceGroup '$ResourceGroupName'" -MaxAttempts $MaxAttempts -RetryDelaySeconds $RetryDelaySeconds -Action {
    if (Get-AzResourceGroup -Name $ResourceGroupName -ErrorAction SilentlyContinue) {
        Remove-AzResourceGroup -Name $ResourceGroupName -Force -ErrorAction Stop | Out-Null
        Write-Output "Deleted resource group '$ResourceGroupName'."
    }
    else {
        Write-Output "Resource group '$ResourceGroupName' not found — nothing to delete (idempotent success)."
    }
}
