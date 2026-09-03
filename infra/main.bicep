// CI-only Azure DevTest Labs foundation for the final RDP E2E gate.
// It deliberately has no standing inbound allow rule. The E2E runner creates
// and removes one lease-owned RDP /32 rule for the VM's private address.
targetScope = 'resourceGroup'

@description('Azure DevTest Labs instance used only by GitHub Actions E2E.')
param labName string = 'rdpilot-ci'

@description('Formula selected by the GitHub Actions DevTest runner.')
param formulaName string = 'rdpilot-rdp-e2e'

@description('Virtual network name dedicated to the lab workload.')
param virtualNetworkName string = 'rdpilot-ci-vnet'

@description('Subnet that holds ephemeral DevTest VMs.')
param subnetName string = 'devtest'

@description('Address range of the DevTest virtual network.')
param virtualNetworkPrefix string = '10.42.0.0/16'

@description('Address range of the DevTest VM subnet.')
param subnetPrefix string = '10.42.1.0/24'

@description('NSG used exclusively by the DevTest VM subnet.')
param workloadNsgName string = 'rdpilot-ci-workload-nsg'

@description('Windows VM size encoded in the formula.')
param vmSize string = 'Standard_D2s_v5'

param location string = resourceGroup().location

resource workloadNsg 'Microsoft.Network/networkSecurityGroups@2024-05-01' = {
  name: workloadNsgName
  location: location
}

resource virtualNetwork 'Microsoft.Network/virtualNetworks@2024-05-01' = {
  name: virtualNetworkName
  location: location
  properties: {
    addressSpace: {
      addressPrefixes: [
        virtualNetworkPrefix
      ]
    }
    subnets: [
      {
        name: subnetName
        properties: {
          addressPrefix: subnetPrefix
          networkSecurityGroup: {
            id: workloadNsg.id
          }
        }
      }
    ]
  }
}

resource lab 'Microsoft.DevTestLab/labs@2018-09-15' = {
  name: labName
  location: location
  properties: {
    labStorageType: 'Standard'
  }
}

resource labVirtualNetwork 'Microsoft.DevTestLab/labs/virtualNetworks@2018-09-15' = {
  parent: lab
  name: virtualNetworkName
  properties: {
    description: 'GitHub Actions DevTest E2E network'
    externalProviderResourceId: virtualNetwork.id
    subnetOverrides: [
      {
        labSubnetName: subnetName
        resourceId: '${virtualNetwork.id}/subnets/${subnetName}'
        useInVmCreationPermission: 'Allow'
        usePublicIpAddressPermission: 'Allow'
      }
    ]
  }
}

resource rdpFormula 'Microsoft.DevTestLab/labs/formulas@2018-09-15' = {
  parent: lab
  name: formulaName
  location: location
  properties: {
    description: 'Short-lived GitHub Actions RDP E2E target'
    formulaContent: {
      name: formulaName
      location: location
      properties: {
        size: vmSize
        allowClaim: false
        disallowPublicIpAddress: false
        labVirtualNetworkId: labVirtualNetwork.id
        labSubnetName: subnetName
        storageType: 'Standard'
        galleryImageReference: {
          publisher: 'MicrosoftWindowsServer'
          offer: 'WindowsServer'
          sku: '2022-datacenter-g2'
          osType: 'Windows'
          version: 'latest'
        }
      }
    }
    osType: 'Windows'
  }
}

output devtestLabsId string = lab.id
output devtestFormula string = rdpFormula.name
output devtestWorkloadNsgId string = workloadNsg.id
