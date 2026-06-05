<#
    DeleteResourceGroup.Tests.ps1 — Pester unit tests for the auto-destroy runbook
    body (infra/scripts/Delete-ResourceGroup.ps1).

    Cost: ZERO Azure, ZERO network. All Az cmdlets (Connect-AzAccount,
    Get-AzResourceGroup, Remove-AzResourceGroup, Disable-AzContextAutosave) and
    Start-Sleep are mocked. The runbook is dot-sourced so its top-level auth+delete
    flow runs against the mocks.

    Focus: the retry-with-backoff resilience added for the RBAC-propagation race —
      - transient 403 / AuthorizationFailed is retried, then succeeds
      - a non-transient error throws immediately (no wasted retries)
      - an already-deleted RG is an idempotent success
#>

$script:RunbookScript = Join-Path $PSScriptRoot '..' 'scripts' 'Delete-ResourceGroup.ps1'

BeforeAll {
    $script:RunbookScript = Join-Path $PSScriptRoot '..' 'scripts' 'Delete-ResourceGroup.ps1'

    # Az cmdlets do not exist in the test environment; define no-op stubs so Mock can
    # intercept them. (Mock requires the command to exist.)
    function Disable-AzContextAutosave { param([string]$Scope) }
    function Connect-AzAccount { param([switch]$Identity, [string]$ErrorAction) }
    function Get-AzResourceGroup { param([string]$Name, [string]$ErrorAction) }
    function Remove-AzResourceGroup { param([string]$Name, [switch]$Force, [string]$ErrorAction) }
}

Describe 'Delete-ResourceGroup runbook: retry resilience' {

    BeforeEach {
        # Shared inert mocks; per-test overrides below.
        Mock Disable-AzContextAutosave {}
        Mock Connect-AzAccount {}
        Mock Start-Sleep {}                       # never actually sleep in tests
        Mock Get-AzResourceGroup { @{ ResourceGroupName = 'rdpilot-test' } }  # RG exists
        Mock Remove-AzResourceGroup {}
    }

    It 'retries a transient 403 on the delete, then succeeds' {
        # Fail the first two delete attempts with a transient 403, succeed on the 3rd.
        # Use a script-scoped counter via Set-Variable into the test (script) scope so
        # the mock body and the assertions share the same state.
        $script:rmAttempt = 0
        Mock Remove-AzResourceGroup {
            $script:rmAttempt++
            if ($script:rmAttempt -lt 3) {
                throw "Long running operation failed with status 'Forbidden'. StatusCode: 403"
            }
        }

        { & $script:RunbookScript -ResourceGroupName 'rdpilot-test' -RetryDelaySeconds 0 6>$null } |
            Should -Not -Throw
        Should -Invoke Remove-AzResourceGroup -Times 3   # 2 failures + 1 success
        Should -Invoke Start-Sleep -Times 2              # two backoffs before success
    }

    It 'does NOT retry a non-transient error — throws on the first attempt' {
        Mock Remove-AzResourceGroup {
            throw "BadRequest: the resource group name is invalid"
        }

        { & $script:RunbookScript -ResourceGroupName 'rdpilot-test' -RetryDelaySeconds 0 6>$null } |
            Should -Throw
        Should -Invoke Remove-AzResourceGroup -Times 1   # no retry for non-transient
        Should -Invoke Start-Sleep -Times 0
    }

    It 'treats an already-deleted RG as idempotent success (no Remove call)' {
        Mock Get-AzResourceGroup { $null }        # RG already gone
        Mock Remove-AzResourceGroup {}

        { & $script:RunbookScript -ResourceGroupName 'rdpilot-test' -RetryDelaySeconds 0 6>$null } |
            Should -Not -Throw
        Should -Invoke Remove-AzResourceGroup -Times 0
    }

    It 'gives up after MaxAttempts of persistent transient failures' {
        Mock Remove-AzResourceGroup {
            throw "AuthorizationFailed: does not have authorization to perform action"
        }

        { & $script:RunbookScript -ResourceGroupName 'rdpilot-test' -MaxAttempts 3 -RetryDelaySeconds 0 6>$null } |
            Should -Throw
        Should -Invoke Remove-AzResourceGroup -Times 3
    }
}
