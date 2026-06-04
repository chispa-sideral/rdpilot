// rdpilot test target — declarative provisioning (ENV-01).
//
// Deploys, into a single resource group so `az group delete` cascades (Pitfall 5):
//   VNet + subnet, NSG (RDP 3389 + WinRM 5986 scoped to the dev IP),
//   Standard/Static public IP, NIC, and a WS2022 Datacenter Gen2 Desktop VM.
//
// All API versions are pinned explicitly (no "latest" API).
//
// AUTO-DESTROY TOPOLOGY (ENV-03): SEPARATE-MANAGEMENT (Task 1 decision).
// The Automation Account + runbook + daily schedule live in a PERSISTENT
// management resource group that OUTLIVES the disposable TEST RG. Its
// system-assigned managed identity is granted Contributor over the TEST RG
// ONLY (tightest least-privilege — never the full-access subscription role).
// Because the
// deleter is not inside the RG it deletes, the job host survives the delete
// and the final job status reports cleanly (avoids self-delete caveat,
// RESEARCH.md Pitfall 4). The management RG is NOT torn down by `down`.
//
// Scope: resourceGroup (default for `az deployment group create`). This template
// is deployed INTO the TEST RG; the management-RG resources are declared via a
// module scoped to the management RG (see below).
targetScope = 'resourceGroup'

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

@secure()
@description('Admin/automation account password. Passed by manage-env.ps1 at up-time; never defaulted, never logged, never committed (D-06). @secure() keeps it out of ARM deployment history.')
param adminPassword string

@description('Local admin / automation username on the VM.')
param adminUsername string = 'rdpadmin'

@description('Developer public IP, detected at up-time by manage-env.ps1 (D-05). NSG scopes RDP/WinRM to this single source only — never the open internet.')
param allowedSourceIp string

@description('VM size. Default Standard_B2ms (D-02). One-flag switch to Standard_D2s_v5 if burst throttling distorts later-phase latency (Pitfall 6).')
param vmSize string = 'Standard_B2ms'

@description('Azure region. Defaults to the resource group location.')
param location string = resourceGroup().location

@description('Public URL where Configure-Target.ps1 is published at deploy time (raw URL or storage-blob URL). Supplied by manage-env.ps1 in Plan 04; consumed by the CustomScriptExtension fileUris.')
param scriptUri string

// ---------------------------------------------------------------------------
// Auto-destroy parameters (ENV-03, separate-management topology)
// ---------------------------------------------------------------------------

@description('Name of the PERSISTENT management resource group that holds the auto-destroy Automation Account + runbook. This RG is NOT torn down by `down`; it outlives the disposable TEST RG. Created/managed by manage-env.ps1.')
param managementResourceGroupName string = 'rdpilot-mgmt'

@description('Name of the Automation Account (in the management RG) that runs the auto-destroy runbook.')
param automationAccountName string = 'rdpilot-autodestroy'

@description('Daily auto-destroy fire time (TTL). ISO-8601 datetime for the FIRST schedule occurrence; the schedule then recurs every 1 day. Must be in the future at deploy time. Defaults to ~1 day after deployment.')
param autoDestroyStartTime string = dateTimeAdd(utcNow(), 'P1D')

@description('Read-only SAS blob URI of the published Delete-ResourceGroup.ps1 (the auto-destroy runbook body). Minted per-deploy by manage-env.ps1 from the in-repo source, so the runbook is pinned to the repo rather than fetched from "latest" off the internet. Only needs to be reachable during deployment (Automation imports the content at publish time).')
param deleteRunbookContentUri string

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------

resource nsg 'Microsoft.Network/networkSecurityGroups@2024-05-01' = {
  name: 'rdpilot-nsg'
  location: location
  properties: {
    securityRules: [
      {
        name: 'allow-rdp'
        properties: {
          priority: 1000
          access: 'Allow'
          direction: 'Inbound'
          protocol: 'Tcp'
          sourcePortRange: '*'
          destinationPortRange: '3389'
          sourceAddressPrefix: allowedSourceIp
          destinationAddressPrefix: '*'
        }
      }
      {
        name: 'allow-winrm-https'
        properties: {
          priority: 1010
          access: 'Allow'
          direction: 'Inbound'
          protocol: 'Tcp'
          sourcePortRange: '*'
          destinationPortRange: '5986'
          sourceAddressPrefix: allowedSourceIp
          destinationAddressPrefix: '*'
        }
      }
    ]
  }
}

resource vnet 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: 'rdpilot-vnet'
  location: location
  properties: {
    addressSpace: {
      addressPrefixes: [
        '10.0.0.0/16'
      ]
    }
    subnets: [
      {
        name: 'default'
        properties: {
          addressPrefix: '10.0.0.0/24'
          networkSecurityGroup: {
            id: nsg.id
          }
        }
      }
    ]
  }
}

resource publicIp 'Microsoft.Network/publicIPAddresses@2024-05-01' = {
  name: 'rdpilot-pip'
  location: location
  sku: {
    name: 'Standard'
  }
  properties: {
    publicIPAllocationMethod: 'Static'
  }
}

resource nic 'Microsoft.Network/networkInterfaces@2024-05-01' = {
  name: 'rdpilot-nic'
  location: location
  properties: {
    ipConfigurations: [
      {
        name: 'ipconfig1'
        properties: {
          privateIPAllocationMethod: 'Dynamic'
          subnet: {
            id: vnet.properties.subnets[0].id
          }
          publicIPAddress: {
            id: publicIp.id
          }
        }
      }
    ]
  }
}

// ---------------------------------------------------------------------------
// Virtual machine — WS2022 Datacenter Gen2 Desktop Experience (D-01)
// ---------------------------------------------------------------------------

resource vm 'Microsoft.Compute/virtualMachines@2024-07-01' = {
  name: 'rdpilot-vm'
  location: location
  properties: {
    hardwareProfile: {
      vmSize: vmSize
    }
    osProfile: {
      computerName: 'rdpilot-vm'
      adminUsername: adminUsername
      adminPassword: adminPassword
      // NLA is the Azure default — do NOT disable it here. It is verified
      // post-deploy in Validate-Target.ps1, not configured (Pitfall 2).
      windowsConfiguration: {
        provisionVMAgent: true
        enableAutomaticUpdates: true
      }
    }
    storageProfile: {
      imageReference: {
        publisher: 'MicrosoftWindowsServer'
        offer: 'WindowsServer'
        sku: '2022-datacenter-g2' // Gen2 Desktop Experience. NOT '-core', NOT '-azure-edition' (D-01).
        version: 'latest'
      }
      osDisk: {
        createOption: 'FromImage'
        managedDisk: {
          storageAccountType: 'StandardSSD_LRS'
        }
      }
    }
    networkProfile: {
      networkInterfaces: [
        {
          id: nic.id
        }
      ]
    }
  }
}

// ---------------------------------------------------------------------------
// In-guest configuration — one CustomScriptExtension runs one idempotent script.
//
// The script body lives in infra/scripts/Configure-Target.ps1 (authored in Plan 03,
// not here). It performs ALL in-guest hardening in a single idempotent pass:
// WinRM HTTPS listener + firewall rule, default-user-hive 96 DPI (LogPixels=96 /
// Win8DpiScaling=1) + RemoteDesktop_SuppressWhenMinimized, HKLM SuppressWhenMinimized,
// and a SHA-256-verified 7-Zip install — with no reboot.
//
// CSE is used deliberately over the deployment-script resource type: that type
// runs in a managed container and CANNOT touch the guest registry/WinRM, so it is
// the wrong tool for in-guest config (RESEARCH.md L93). The `scriptUri` is
// set at deploy time by manage-env.ps1 (Plan 04). This resource DEFINES the
// CSE -> script contract that Plan 03 implements (filename, no-arg invocation).
// ---------------------------------------------------------------------------

resource configureTarget 'Microsoft.Compute/virtualMachines/extensions@2024-07-01' = {
  parent: vm
  name: 'Configure-Target'
  location: location
  properties: {
    publisher: 'Microsoft.Compute'
    type: 'CustomScriptExtension'
    typeHandlerVersion: '1.10'
    autoUpgradeMinorVersion: true
    settings: {
      fileUris: [
        scriptUri
      ]
    }
    protectedSettings: {
      commandToExecute: 'powershell -ExecutionPolicy Unrestricted -File Configure-Target.ps1'
    }
  }
}

// ---------------------------------------------------------------------------
// Auto-destroy (ENV-03) — SEPARATE-MANAGEMENT topology.
//
// The Automation Account + runbook + daily schedule are declared in the
// PERSISTENT management RG via a module scoped to that RG (it outlives the TEST
// RG, so the deleter survives the delete and reports clean job status —
// RESEARCH.md Pitfall 4). The role assignment below grants that account's
// managed identity Contributor over the TEST RG (this deployment's scope) ONLY
// — the tightest least-privilege grant. NEVER the subscription full-access role.
// ---------------------------------------------------------------------------

// Built-in role definition: Contributor (can manage/delete resources, but NOT
// grant access). Referenced by its well-known GUID, scoped to the TEST RG.
var contributorRoleId = subscriptionResourceId(
  'Microsoft.Authorization/roleDefinitions',
  'b24988ac-6180-42a0-ab88-20f7382dd24c'
)

module autoDestroy 'modules/autodestroy.bicep' = {
  name: 'autodestroy'
  scope: resourceGroup(managementResourceGroupName)
  params: {
    automationAccountName: automationAccountName
    location: location
    targetResourceGroupName: resourceGroup().name
    autoDestroyStartTime: autoDestroyStartTime
    deleteRunbookContentUri: deleteRunbookContentUri
  }
}

// Contributor scoped to THIS (TEST) resource group only — tightest privilege.
resource autoDestroyRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = {
  name: guid(resourceGroup().id, automationAccountName, 'Contributor')
  properties: {
    roleDefinitionId: contributorRoleId
    principalId: autoDestroy.outputs.principalId
    principalType: 'ServicePrincipal'
  }
}

// ---------------------------------------------------------------------------
// Outputs — connection details consumed by manage-env.ps1 / Validate-Target.ps1
// ---------------------------------------------------------------------------

output publicIp string = publicIp.properties.ipAddress
output adminUsername string = adminUsername
output rdpPort int = 3389
output winrmPort int = 5986
