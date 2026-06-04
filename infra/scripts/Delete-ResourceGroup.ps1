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
    [string]$ResourceGroupName
)

$ErrorActionPreference = 'Stop'

# Do not persist the Az context to the runbook sandbox profile between jobs.
Disable-AzContextAutosave -Scope Process | Out-Null

# Authenticate as the Automation Account's system-assigned managed identity.
Connect-AzAccount -Identity | Out-Null

# Idempotent delete: if the RG is already gone (e.g. a prior `down` removed it),
# this is a no-op success rather than a failure.
if (Get-AzResourceGroup -Name $ResourceGroupName -ErrorAction SilentlyContinue) {
    Remove-AzResourceGroup -Name $ResourceGroupName -Force
    Write-Output "Deleted resource group '$ResourceGroupName'."
}
else {
    Write-Output "Resource group '$ResourceGroupName' not found — nothing to delete."
}
