<#
    manage-env.ps1 — rdpilot disposable test-environment driver (ENV-02).

    Brings the Azure Windows test target up and down on demand (D-05/D-06):
      up   : pre-flight az auth check -> detect dev public IP -> generate a strong
             crypto-random admin password -> create the TEST resource group ->
             PUBLISH the in-guest + runbook scripts to a private blob with a
             short-lived READ-ONLY SAS (the CSE/runbook wiring) -> deploy main.bicep
             -> write gitignored .secrets/connection.json.
      down : delete the TEST resource group (cascades — Bicep has no destroy,
             Pitfall 5). The persistent management RG is NOT torn down.

    AUTO-DESTROY TOPOLOGY: separate-management. The Automation Account + runbook
    live in a PERSISTENT management RG (default rdpilot-mgmt) that outlives the
    TEST RG; its managed identity is granted Contributor over the TEST RG only.
    `up` ensures the management RG exists; `down` leaves it in place.

    SUBSCRIPTION: never hard-coded. All `az` calls run against the caller's
    currently-active subscription (`az account show`). Switch with `az account set
    --subscription <id>` before running if needed.

    SECRETS: the generated admin password and the SAS tokens are credentials.
    They are NEVER echoed to stdout or logs and NEVER committed — the password is
    written only to the gitignored .secrets/connection.json (D-06).
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('up', 'down')]
    [string]$Action,

    [string]$Rg = 'rdpilot-test',

    [string]$Location = 'westeurope',

    [string]$AllowedSourceIp,

    [string]$ManagementRg = 'rdpilot-mgmt',

    [string]$AutomationAccountName = 'rdpilot-autodestroy',

    # VM size for the test target. Must be available + unrestricted in $Location
    # (verified by the up-preflight before any deploy). Override when the default is
    # capacity-restricted in the region (e.g. -VmSize Standard_B2s_v2).
    [string]$VmSize = 'Standard_B2ms',

    # down only: when set, fire the RG delete async (--no-wait) and return
    # immediately. Default (omitted) BLOCKS until the RG is fully deleted, so a
    # subsequent `up` never collides with an in-flight deprovisioning.
    [switch]$NoWait
)

$ErrorActionPreference = 'Stop'

# Resolve the repo-relative infra paths from this script's own location so the
# driver works regardless of the caller's current directory.
$InfraRoot = $PSScriptRoot
$TemplateFile = Join-Path $InfraRoot 'main.bicep'
$ConfigureScript = Join-Path $InfraRoot 'scripts' 'Configure-Target.ps1'
$DeleteRunbookScript = Join-Path $InfraRoot 'scripts' 'Delete-ResourceGroup.ps1'

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

function New-StrongPassword {
    <#
        Cryptographically strong admin password (>= 24 chars, all four character
        classes). Uses [System.Web.Security.Membership]::GeneratePassword where
        available, otherwise a RandomNumberGenerator-based generator. NOT a
        hand-rolled char-shuffler (RESEARCH.md "Don't Hand-Roll"). The value is
        returned in-memory only and never written to console/logs.
    #>
    param([int]$Length = 24)

    try {
        Add-Type -AssemblyName System.Web -ErrorAction Stop
        # 24 chars, 6 non-alphanumeric — meets Azure's complexity policy.
        $candidate = [System.Web.Security.Membership]::GeneratePassword($Length, 6)
        if ($candidate.Length -ge $Length) { return $candidate }
    }
    catch {
        # System.Web unavailable (e.g. PS 7 on non-Windows) — fall through to RNG.
    }

    $lower = 'abcdefghijkmnopqrstuvwxyz'
    $upper = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
    $digit = '23456789'
    $symbol = '!@#$%^&*()-_=+'
    $all = ($lower + $upper + $digit + $symbol).ToCharArray()

    $chars = [System.Collections.Generic.List[char]]::new()
    $chars.Add($lower[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($lower.Length)])
    $chars.Add($upper[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($upper.Length)])
    $chars.Add($digit[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($digit.Length)])
    $chars.Add($symbol[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($symbol.Length)])
    for ($i = $chars.Count; $i -lt $Length; $i++) {
        $chars.Add($all[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32($all.Length)])
    }
    $shuffled = $chars | Sort-Object { [System.Security.Cryptography.RandomNumberGenerator]::GetInt32([int]::MaxValue) }
    -join $shuffled
}

function Get-DevPublicIp {
    <#
        Detect the developer's current public IP (D-05). Returns the -AllowedSourceIp
        override verbatim if supplied (validated as IPv4), otherwise queries ipify
        over HTTPS. The NSG in main.bicep scopes RDP/WinRM to this single source.
    #>
    param([string]$Override)

    if ($Override) {
        if ($Override -notmatch '^(\d{1,3}\.){3}\d{1,3}$') {
            throw "Supplied -AllowedSourceIp '$Override' is not a valid IPv4 address."
        }
        return $Override
    }

    $resp = Invoke-RestMethod -Uri 'https://api.ipify.org?format=json'
    $ip = $resp.ip
    if (-not $ip -or $ip -notmatch '^(\d{1,3}\.){3}\d{1,3}$') {
        throw "ipify returned an unexpected IP value. Re-run with -AllowedSourceIp <your-ip>."
    }
    return $ip
}

function Get-RgProvisioningState {
    <#
        Returns the provisioningState of $Name ('Succeeded', 'Deleting', etc.), or
        $null if the RG does not exist. Read-only; never throws on a missing RG.
    #>
    param([Parameter(Mandatory)][string]$Name)
    $state = az group show -n $Name --query 'properties.provisioningState' -o tsv 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $state) { return $null }
    return $state.Trim()
}

function Get-ViableVmSizes {
    <#
        Returns up to 6 VM sizes available + UNRESTRICTED (no 'Location' restriction)
        in $Location, as alternatives to a restricted choice. A 'Zone'-only
        restriction is fine (the deployment is non-zonal). Read-only az query.
        Prefers small B-series then Dxs_v5/v3 so the user stays in-region (EU).
    #>
    param([Parameter(Mandatory)][string]$Location)

    $json = az vm list-skus -l $Location --resource-type virtualMachines --all -o json 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $json) { return @() }
    $skus = $json | ConvertFrom-Json

    $prefer = '^Standard_(B2s_v2|B2ls_v2|B2ts_v2|B2|B1|D2s_v5|D2as_v5|D2s_v3|D2as_v4)'
    $skus |
        Where-Object {
            $_.name -match $prefer -and
            -not ($_.restrictions | Where-Object { $_.type -eq 'Location' })
        } |
        Select-Object -ExpandProperty name -Unique |
        Sort-Object |
        Select-Object -First 6
}

function Test-VmSizeAvailable {
    <#
        Returns $true if $VmSize is offered in $Location with NO 'Location'
        restriction (a 'Zone'-only restriction is acceptable for a non-zonal VM).
        Read-only az query. Returns $false if the size is absent or Location-restricted.
    #>
    param(
        [Parameter(Mandatory)][string]$VmSize,
        [Parameter(Mandatory)][string]$Location
    )
    $json = az vm list-skus -l $Location --resource-type virtualMachines --size $VmSize --all -o json 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $json) { return $false }
    $skus = $json | ConvertFrom-Json
    $match = $skus | Where-Object { $_.name -eq $VmSize }
    if (-not $match) { return $false }
    # Available unless there is a hard Location restriction in the region.
    return -not ($match.restrictions | Where-Object { $_.type -eq 'Location' })
}

# ---------------------------------------------------------------------------
# down — tear down the TEST RG (cascades). Management RG is left in place.
# ---------------------------------------------------------------------------

if ($Action -eq 'down') {
    Write-Host "Deleting TEST resource group '$Rg' (the management RG '$ManagementRg' is left in place)..."

    if ($NoWait) {
        # Async: fire and return. The next `up` may collide with the in-flight delete —
        # use the default (blocking) mode if you intend to re-provision immediately.
        az group delete -n $Rg --yes --no-wait | Out-Null
        Write-Host "Teardown started for '$Rg' (async, --no-wait). Deletion continues in the background."
        return
    }

    # Default: BLOCK until the RG is fully gone, so a subsequent `up` never hits a
    # 'ResourceGroupBeingDeleted' deprovisioning collision. `az group delete` (without
    # --no-wait) already blocks server-side; we then poll to confirm and show progress.
    Write-Host "Waiting for deletion to complete (this can take several minutes)..."
    az group delete -n $Rg --yes | Out-Null

    # Confirm the RG is actually gone (poll provisioningState until absent).
    for ($i = 0; $i -lt 60; $i++) {
        $state = Get-RgProvisioningState -Name $Rg
        if (-not $state) {
            Write-Host "Resource group '$Rg' is fully deleted."
            return
        }
        Write-Host "  ... still deleting (state: $state)"
        Start-Sleep -Seconds 10
    }
    throw "Timed out waiting for '$Rg' to delete. Check the Azure portal; re-run -Action down when ready."
}

# ---------------------------------------------------------------------------
# up — provision the TEST environment.
# ---------------------------------------------------------------------------

# =============================================================================
# UP-PREFLIGHT — all checks run BEFORE any resource is created. Any failure aborts
# with a clear, actionable message and a non-zero exit (so no money is spent and no
# false "is up" is printed). Order: auth -> RG-state guard -> VM-SKU capacity ->
# (light) provider registration.
# =============================================================================

Write-Host "=== Preflight checks for '$Action' ==="

# (P1) Authenticated az session against the caller's active subscription (A7). Check
# the exit code, not the output, so a logged-out session fails fast. No subscription
# is ever hard-coded — whatever `az account show` reports is what we deploy into.
$acct = az account show --query '{name:name,id:id}' -o json 2>$null
if ($LASTEXITCODE -ne 0 -or -not $acct) {
    throw "Not logged in — run 'az login' (and 'az account set --subscription <id>' if needed) before 'up'."
}
$acctObj = $acct | ConvertFrom-Json
Write-Host "  [OK]  Authenticated. Subscription: $($acctObj.name) ($($acctObj.id))"
Write-Host "  Target region: '$Location' | Test RG: '$Rg' | Management RG: '$ManagementRg' | VM size: '$VmSize'"

# (P2) RG-state guard. Deploying into an RG that is mid-delete fails with
# 'ResourceGroupBeingDeleted'. A leftover 'Succeeded' RG should be torn down first
# rather than silently re-used (it may hold stale resources / an old NSG scope).
$rgState = Get-RgProvisioningState -Name $Rg
if ($rgState -and $rgState -match 'Delet|deprovision') {
    throw "RG '$Rg' is being deleted (state: $rgState). Wait for deletion to finish, or run '-Action down' (default WAITs) first, then re-run 'up'."
}
if ($rgState -eq 'Succeeded') {
    throw "RG '$Rg' already exists (state: Succeeded). Run '-Action down' to remove the leftover environment first, then re-run 'up' (re-using a dirty RG is not supported)."
}
Write-Host "  [OK]  Test RG '$Rg' is clear (no conflicting environment)."

# (P3) VM-SKU capacity preflight. westeurope frequently 'Location'-restricts the
# default Standard_B2ms for a subscription ('SkuNotAvailable' at deploy time). Catch
# it BEFORE wasting a deploy and suggest in-region alternatives.
if (-not (Test-VmSizeAvailable -VmSize $VmSize -Location $Location)) {
    $alts = Get-ViableVmSizes -Location $Location
    $altMsg = if ($alts) { ($alts -join ', ') } else { '(none found — try another region)' }
    throw "VM size '$VmSize' is unavailable/restricted in '$Location' (SkuNotAvailable). Re-run with one of these available sizes: $altMsg`n  e.g. -VmSize $($alts | Select-Object -First 1)"
}
Write-Host "  [OK]  VM size '$VmSize' is available in '$Location'."

# (P4) Light provider-registration check. A non-registered provider fails the deploy
# late; warn (don't block) so the user can register and retry.
$requiredProviders = @('Microsoft.Compute', 'Microsoft.Network', 'Microsoft.Storage', 'Microsoft.Automation')
foreach ($ns in $requiredProviders) {
    $regState = az provider show -n $ns --query 'registrationState' -o tsv 2>$null
    if ($LASTEXITCODE -eq 0 -and $regState -and $regState -ne 'Registered') {
        Write-Warning "Resource provider '$ns' is '$regState' (not Registered). Run 'az provider register -n $ns' if the deploy fails."
    }
}
Write-Host "=== Preflight passed ==="

# Detect the dev IP (or use the override) BEFORE generating secrets / deploying.
$devIp = Get-DevPublicIp -Override $AllowedSourceIp

# -WhatIf short-circuit: stop here after IP detection without creating resources,
# generating secrets, or deploying. Lets the env be validated cheaply / in tests.
if ($WhatIfPreference -or -not $PSCmdlet.ShouldProcess($Rg, 'provision rdpilot test environment')) {
    Write-Host "[WhatIf] Would provision RG '$Rg' in '$Location' (VM size '$VmSize') scoped to source IP '$devIp'. No resources created."
    return
}

# Strong admin password — generated in-memory, never echoed.
$adminPwd = New-StrongPassword -Length 24

# Ensure both resource groups exist. The management RG is PERSISTENT (not torn
# down by `down`); the TEST RG is disposable.
Write-Host "Creating resource groups in '$Location' ('$ManagementRg' [persistent], '$Rg' [test])..."
az group create -n $ManagementRg -l $Location | Out-Null
az group create -n $Rg -l $Location | Out-Null

# -----------------------------------------------------------------------------
# PUBLISH the scripts the deployment depends on (the CSE + runbook wiring).
#
# Without a reachable scriptUri the CustomScriptExtension is a silent no-op and
# ALL ENV-01 in-guest hardening fails. We publish Configure-Target.ps1 (for the
# CSE) AND Delete-ResourceGroup.ps1 (for the auto-destroy runbook) to a private
# blob container in the TEST RG, then mint short-lived READ-ONLY single-blob SAS
# full-URIs. The storage account lives in $Rg so `down` tears it down with
# everything else. The SAS tokens are credentials — never echoed.
# -----------------------------------------------------------------------------

Write-Host "Publishing CSE + runbook scripts to a private blob (read-only SAS)..."
$rand = -join ((1..8) | ForEach-Object { '0123456789abcdefghijklmnopqrstuvwxyz'[[System.Security.Cryptography.RandomNumberGenerator]::GetInt32(36)] })
$storageAccount = "rdpilotcse$rand"
$container = 'cse'

az storage account create `
    --name $storageAccount `
    --resource-group $Rg `
    --location $Location `
    --sku Standard_LRS `
    --kind StorageV2 `
    --min-tls-version TLS1_2 `
    --allow-blob-public-access false | Out-Null

# Resolve a storage key for the create/upload/SAS calls (account-key auth — the
# account is brand-new and private). The key is a credential; capture it without
# emitting it to the console.
$storageKey = az storage account keys list --account-name $storageAccount --resource-group $Rg --query "[0].value" -o tsv

# Private container (no public access).
az storage container create `
    --name $container `
    --account-name $storageAccount `
    --account-key $storageKey `
    --public-access off | Out-Null

# Upload both scripts.
az storage blob upload `
    --account-name $storageAccount `
    --account-key $storageKey `
    --container-name $container `
    --file $ConfigureScript `
    --name 'Configure-Target.ps1' `
    --overwrite | Out-Null

az storage blob upload `
    --account-name $storageAccount `
    --account-key $storageKey `
    --container-name $container `
    --file $DeleteRunbookScript `
    --name 'Delete-ResourceGroup.ps1' `
    --overwrite | Out-Null

# Short-lived (~1h) READ-ONLY single-blob SAS full-URIs. --permissions r only;
# never write/list. The full URI (blob URL + SAS query) is captured into the
# deployment params. These are credentials — never echoed.
$sasExpiry = (Get-Date).ToUniversalTime().AddHours(1).ToString('yyyy-MM-ddTHH:mmZ')

$scriptUri = az storage blob generate-sas `
    --account-name $storageAccount `
    --account-key $storageKey `
    --container-name $container `
    --name 'Configure-Target.ps1' `
    --permissions r `
    --expiry $sasExpiry `
    --https-only `
    --full-uri `
    -o tsv

$runbookUri = az storage blob generate-sas `
    --account-name $storageAccount `
    --account-key $storageKey `
    --container-name $container `
    --name 'Delete-ResourceGroup.ps1' `
    --permissions r `
    --expiry $sasExpiry `
    --https-only `
    --full-uri `
    -o tsv

if (-not $scriptUri) {
    throw "Failed to mint a SAS URI for Configure-Target.ps1 — the CSE would no-op. Aborting before deploy."
}
if (-not $runbookUri) {
    throw "Failed to mint a SAS URI for Delete-ResourceGroup.ps1 — the auto-destroy runbook would be empty. Aborting before deploy."
}

# -----------------------------------------------------------------------------
# Deploy. adminPassword is passed as a @secure() param (kept out of deployment
# history); allowedSourceIp scopes the NSG; scriptUri wires the CSE; the runbook
# URI + management-RG params wire the auto-destroy. Never echo the password/SAS.
# -----------------------------------------------------------------------------

# SAFE parameter passing (Pitfall: the password — and any value with cmd-special
# chars like & | < > ( ) ^ or spaces — must NEVER reach a command line). `az` on
# Windows is a .cmd batch shim, so a bareword `adminPassword=$adminPwd` is re-parsed
# by cmd.exe and breaks on special chars ("No se esperaba X en este momento"); it
# also exposes the secret in the process command line. Instead we serialise ALL
# parameters into an ARM parameters JSON file (PowerShell ConvertTo-Json — no shell
# interpolation, no cmd re-parsing) and pass it with `--parameters @file`. The temp
# file lives under .secrets-adjacent temp and is deleted in `finally`.
$deployName = 'main'
$paramsFile = Join-Path ([System.IO.Path]::GetTempPath()) ("rdpilot-deploy-{0}.json" -f ([guid]::NewGuid().ToString('N')))

$armParams = [ordered]@{
    '$schema'        = 'https://schema.management.azure.com/schemas/2019-04-01/deploymentParameters.json#'
    contentVersion = '1.0.0.0'
    parameters     = [ordered]@{
        adminPassword               = @{ value = $adminPwd }
        allowedSourceIp             = @{ value = $devIp }
        scriptUri                   = @{ value = $scriptUri }
        deleteRunbookContentUri     = @{ value = $runbookUri }
        managementResourceGroupName = @{ value = $ManagementRg }
        automationAccountName       = @{ value = $AutomationAccountName }
        vmSize                      = @{ value = $VmSize }
    }
}

try {
    # Write the params file with restrictive defaults; it carries the password + SAS,
    # so it is created in the per-user temp dir and removed immediately after deploy.
    $armParams | ConvertTo-Json -Depth 5 | Set-Content -Path $paramsFile -Encoding UTF8

    Write-Host "Deploying VM size '$VmSize' into '$Rg' (this typically takes ~5-10 min)..."
    az deployment group create `
        -g $Rg `
        -n $deployName `
        --template-file $TemplateFile `
        --parameters "@$paramsFile" | Out-Null

    if ($LASTEXITCODE -ne 0) {
        # Surface a real failure: no false "is up". The RG is likely partially
        # provisioned, so direct the user to clean it before retrying.
        throw "az deployment group create FAILED (exit $LASTEXITCODE). The TEST RG '$Rg' may be partially provisioned — run 'pwsh infra/manage-env.ps1 -Action down' to clean it up before retrying."
    }
    Write-Host "Deployment '$deployName' succeeded."
}
finally {
    # The params file carries credentials — always remove it, even on failure.
    if (Test-Path $paramsFile) { Remove-Item $paramsFile -Force -ErrorAction SilentlyContinue }
}

# Read the deployment outputs for the connection file.
$publicIp = az deployment group show -g $Rg -n $deployName --query "properties.outputs.publicIp.value" -o tsv 2>$null
if (-not $publicIp) {
    $publicIp = az network public-ip list -g $Rg --query "[0].ipAddress" -o tsv
}
if (-not $publicIp) {
    throw "Deployment '$deployName' completed but no public IP could be read from outputs or the RG. The connection file would be unusable; aborting before writing it."
}
Write-Host "Deployment succeeded, public IP $publicIp."

# -----------------------------------------------------------------------------
# PUBLISH the auto-destroy runbook (ENV-03). The Bicep runbook resource carries a
# publishContentLink (the read-only SAS to Delete-ResourceGroup.ps1), but ARM does
# not reliably promote that imported content to the Published state — the runbook
# can land as a DRAFT, so the daily schedule would fire jobs against unpublished
# content (no-op/failure). Explicitly publishing here guarantees the runbook ends
# up Published with the deployed content on EVERY up. This is idempotent: publishing
# an already-published / unchanged runbook is a harmless no-op, so it never hard-fails
# a re-run. The runbook name MUST match the one in autodestroy.bicep.
# -----------------------------------------------------------------------------
$runbookName = 'Delete-ResourceGroup'
Write-Host "Publishing auto-destroy runbook '$runbookName' in '$AutomationAccountName'..."
az automation runbook publish `
    --automation-account-name $AutomationAccountName `
    --resource-group $ManagementRg `
    --name $runbookName | Out-Null
if ($LASTEXITCODE -ne 0) {
    # Verify whether it is already Published before treating this as fatal (the publish
    # CLI can return non-zero when there is no new draft to promote). az automation
    # runbook show returns `state` at the TOP level (not under .properties).
    $rbState = az automation runbook show `
        --automation-account-name $AutomationAccountName `
        --resource-group $ManagementRg `
        --name $runbookName `
        --query 'state' -o tsv 2>$null
    if ($rbState -ne 'Published') {
        throw "Failed to publish auto-destroy runbook '$runbookName' (state: '$rbState'). The daily schedule would fire against an unpublished runbook. Re-run 'up' or publish manually: az automation runbook publish --automation-account-name $AutomationAccountName --resource-group $ManagementRg --name $runbookName"
    }
    Write-Host "  Runbook '$runbookName' is already Published (nothing to promote)."
} else {
    Write-Host "  Runbook '$runbookName' published."
}

# -----------------------------------------------------------------------------
# Write the gitignored connection file (D-06). The password lands ONLY here.
# -----------------------------------------------------------------------------
Write-Host "Writing connection file..."

$secretsDir = Join-Path (Split-Path $InfraRoot -Parent) '.secrets'
New-Item -ItemType Directory -Force -Path $secretsDir | Out-Null
$connectionFile = Join-Path $secretsDir 'connection.json'

[ordered]@{
    host     = $publicIp
    user     = 'rdpadmin'
    password = $adminPwd
    rdpPort  = 3389
    winrmPort = 5986
} | ConvertTo-Json | Set-Content -Path $connectionFile -Encoding UTF8

# Surface connection info WITHOUT the password (it is in the gitignored file only).
Write-Host "Environment '$Rg' is up. Public IP: $publicIp"
Write-Host "Connection details (incl. password) written to: $connectionFile (gitignored)."
Write-Host "Run 'pwsh infra/manage-env.ps1 -Action down' to tear it down."
