// autodestroy.bicep — ENV-03 auto-destroy, SEPARATE-MANAGEMENT topology.
//
// Declares the PERSISTENT management-RG resources that delete the disposable
// TEST resource group on a daily schedule:
//   - an Automation Account with a SYSTEM-ASSIGNED managed identity
//   - a PowerShell runbook published from the in-repo Delete-ResourceGroup.ps1
//     (via publishContentLink to a short-lived read-only SAS blob minted by
//     manage-env.ps1 — pinned to the repo source, not "latest" off the internet)
//   - a daily schedule + a jobSchedule linking schedule -> runbook (passing the
//     TEST RG name as the runbook -ResourceGroupName parameter)
//
// This module is deployed (via a `module` block in main.bicep) into the
// management RG, which OUTLIVES the TEST RG so the job host survives the delete
// and reports clean status (RESEARCH.md Pitfall 4). The least-privilege role
// assignment (Contributor over the TEST RG only) lives in main.bicep at the
// TEST-RG scope and references this module's `principalId` output.
//
// All API versions pinned explicitly (no "latest").

targetScope = 'resourceGroup' // the MANAGEMENT resource group

@description('Automation Account name (management RG).')
param automationAccountName string

@description('Region for the management-RG resources.')
param location string = resourceGroup().location

@description('Name of the disposable TEST resource group the runbook deletes (passed as the runbook -ResourceGroupName parameter).')
param targetResourceGroupName string

@description('ISO-8601 datetime of the first daily schedule occurrence (TTL fire time). Must be in the future at deploy time.')
param autoDestroyStartTime string

@description('Read-only SAS blob URI of the published Delete-ResourceGroup.ps1 (the runbook body). Minted per-deploy by manage-env.ps1; Automation imports and stores the content at publish time, so the URI only needs to be reachable during deployment.')
param deleteRunbookContentUri string

var runbookName = 'Delete-ResourceGroup'
var scheduleName = 'daily-autodestroy'

resource automationAccount 'Microsoft.Automation/automationAccounts@2023-11-01' = {
  name: automationAccountName
  location: location
  identity: {
    type: 'SystemAssigned'
  }
  properties: {
    sku: {
      name: 'Basic'
    }
  }
}

resource runbook 'Microsoft.Automation/automationAccounts/runbooks@2023-11-01' = {
  parent: automationAccount
  name: runbookName
  location: location
  properties: {
    runbookType: 'PowerShell'
    logProgress: false
    logVerbose: false
    // Pinned to the in-repo Delete-ResourceGroup.ps1 source: manage-env.ps1
    // uploads it to a private blob and mints a short-lived read-only SAS URI,
    // which Automation fetches once at publish time. The runbook body runs
    // Connect-AzAccount -Identity + Remove-AzResourceGroup -Force; managed
    // identity only, never RunAs.
    publishContentLink: {
      uri: deleteRunbookContentUri
    }
  }
}

resource schedule 'Microsoft.Automation/automationAccounts/schedules@2023-11-01' = {
  parent: automationAccount
  name: scheduleName
  properties: {
    description: 'Daily auto-destroy of the disposable rdpilot TEST resource group (ENV-03 / D-04).'
    startTime: autoDestroyStartTime
    frequency: 'Day'
    interval: 1
  }
}

resource jobSchedule 'Microsoft.Automation/automationAccounts/jobSchedules@2023-11-01' = {
  parent: automationAccount
  name: guid(automationAccount.id, runbookName, scheduleName)
  properties: {
    runbook: {
      name: runbookName
    }
    schedule: {
      name: scheduleName
    }
    parameters: {
      ResourceGroupName: targetResourceGroupName
    }
  }
  dependsOn: [
    runbook
    schedule
  ]
}

@description('Principal ID of the Automation Account system-assigned managed identity. main.bicep grants this Contributor over the TEST RG only.')
output principalId string = automationAccount.identity.principalId
