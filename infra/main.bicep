// rdpilot test target — declarative provisioning (ENV-01).
//
// Deploys, into a single resource group so `az group delete` cascades (Pitfall 5):
//   VNet + subnet, NSG (RDP 3389 + WinRM 5986 scoped to the dev IP),
//   Standard/Static public IP, NIC, and a WS2022 Datacenter Gen2 Desktop VM.
//
// All API versions are pinned explicitly (no "latest" API). Resource graph only —
// the auto-destroy Automation account + runbook are owned by Plan 04, not here.
//
// Scope: resourceGroup (default for `az deployment group create`).
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
// Outputs — connection details consumed by manage-env.ps1 / Validate-Target.ps1
// ---------------------------------------------------------------------------

output publicIp string = publicIp.properties.ipAddress
output adminUsername string = adminUsername
output rdpPort int = 3389
output winrmPort int = 5986
