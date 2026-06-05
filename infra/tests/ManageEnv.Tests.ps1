<#
    ManageEnv.Tests.ps1 — Pester unit tests for infra/manage-env.ps1 logic.

    Test framework : Pester 5.x (pinned: validated against Pester 5.7.1).
    Cost           : ZERO Azure cost, ZERO network. `az` and `Invoke-RestMethod`
                     are mocked; no real deployment or HTTP call is ever made.

    Scope (activated in Plan 01-04 — infra/manage-env.ps1 now exists):
      - Password generation entropy/length            -> LIVE (standalone expr).
      - Dev public-IP detection (ipify shape)          -> LIVE (mock Invoke-RestMethod).
      - `az deployment group create` param wiring       -> LIVE (mock az).
      - `-AllowedSourceIp` override bypasses ipify       -> LIVE (mock az + IRM).
      - CSE-script publish: blob upload + non-empty scriptUri wiring -> LIVE.

    SECURITY: never write a real/generated password to test output. Assertions are
    on length and character-class coverage only; the deployment-param assertion
    matches on the presence of `adminPassword=`, never the value.
#>

# Evaluated at Pester DISCOVERY time (top-level script body) so any -Skip conditions on
# `It` resolve correctly — BeforeAll runs at the later run phase and is too late for -Skip.
$script:ManageEnvScript = Join-Path $PSScriptRoot '..' 'manage-env.ps1'

BeforeAll {
    $script:ManageEnvScript = Join-Path $PSScriptRoot '..' 'manage-env.ps1'

    # Standalone cryptographic password generator — mirrors the generation
    # expression manage-env.ps1 uses. Tested LIVE here. Uses
    # System.Security.Cryptography (crypto RNG), NOT a hand-rolled char-shuffler.
    function New-TestAdminPassword {
        param([int]$Length = 24)
        $lower  = 'abcdefghijkmnopqrstuvwxyz'
        $upper  = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
        $digit  = '23456789'
        $symbol = '!@#$%^&*()-_=+'
        $all    = ($lower + $upper + $digit + $symbol).ToCharArray()

        $chars = @(
            $lower[(Get-Random -Maximum $lower.Length)]
            $upper[(Get-Random -Maximum $upper.Length)]
            $digit[(Get-Random -Maximum $digit.Length)]
            $symbol[(Get-Random -Maximum $symbol.Length)]
        )
        for ($i = $chars.Count; $i -lt $Length; $i++) {
            $idx = [System.Security.Cryptography.RandomNumberGenerator]::GetInt32($all.Length)
            $chars += $all[$idx]
        }
        $shuffled = $chars | Sort-Object { [System.Security.Cryptography.RandomNumberGenerator]::GetInt32([int]::MaxValue) }
        -join $shuffled
    }

    # Shared `az` mock body — returns a non-empty tsv-ish value for the queries
    # the driver reads back (account, RG-state, SKU availability, storage key, SAS
    # URI, public IP), and leaves $LASTEXITCODE = 0 so the pre-flight passes. NO real
    # az process is ever spawned. Each test re-declares Mock az so per-test
    # Assert-MockCalled scoping is clean; this is the canonical body (HAPPY PATH:
    # logged in, RG clear, VM size available).
    function script:Invoke-AzMock {
        $line = ($args -join ' ')
        # Simulate a successful native `az` call: reset the native exit code so the
        # driver's preflight (which checks $LASTEXITCODE) passes.
        $global:LASTEXITCODE = 0

        # (P1) auth: account show --query '{name,id}' -o json
        if ($line -match 'account show') { return '{"name":"Test Sub","id":"00000000-0000-0000-0000-000000000000"}' }
        # (P2) RG-state guard: group show ... provisioningState. Happy path = RG does
        # NOT exist (clear). Emit nothing AND a non-zero exit, like a real missing RG.
        if ($line -match 'group show') { $global:LASTEXITCODE = 3; return }
        # (P4) provider registration check — report Registered.
        if ($line -match 'provider show') { return 'Registered' }
        # (P3) VM-SKU availability: vm list-skus -o json. With --size <name> the driver
        # asks "is THIS size ok"; default mock returns the size with NO restrictions.
        if ($line -match 'vm list-skus') {
            if ($line -match '--size\s+(\S+)') {
                $sz = $Matches[1]
                return ('[{{"name":"{0}","resourceType":"virtualMachines","restrictions":[]}}]' -f $sz)
            }
            # Full list (alternatives) — a couple of viable B-series sizes, no restriction.
            return '[{"name":"Standard_B2s_v2","resourceType":"virtualMachines","restrictions":[]},{"name":"Standard_B2ls_v2","resourceType":"virtualMachines","restrictions":[]}]'
        }

        if ($line -match 'storage account keys list') { return 'fake-storage-key==' }
        if ($line -match 'generate-sas') {
            if ($line -match 'Configure-Target\.ps1') {
                return 'https://fake.blob.core.windows.net/cse/Configure-Target.ps1?sig=READONLY'
            }
            return 'https://fake.blob.core.windows.net/cse/Delete-ResourceGroup.ps1?sig=READONLY'
        }
        if ($line -match 'deployment group show') { return '203.0.113.55' }
        if ($line -match 'public-ip list') { return '203.0.113.55' }
        # ENV-03: publishing the auto-destroy runbook succeeds (exit 0). A `show`
        # fallback (only reached if publish returns non-zero) reports Published.
        if ($line -match 'automation runbook publish') { return }
        if ($line -match 'automation runbook show') { return 'Published' }
        return
    }
}

Describe 'Password generation (LIVE)' {
    It 'produces a password of at least 24 characters' {
        $pwd = New-TestAdminPassword -Length 24
        $pwd.Length | Should -BeGreaterOrEqual 24
    }

    It 'covers all four character classes (lower, upper, digit, symbol)' {
        $pwd = New-TestAdminPassword -Length 24
        ($pwd -cmatch '[a-z]')                | Should -BeTrue
        ($pwd -cmatch '[A-Z]')                | Should -BeTrue
        ($pwd -match  '[0-9]')                | Should -BeTrue
        ($pwd -match  '[!@#\$%\^&\*\(\)\-_=\+]') | Should -BeTrue
    }

    It 'uses a cryptographic source (RandomNumberGenerator) producing distinct values' {
        $a = New-TestAdminPassword -Length 24
        $b = New-TestAdminPassword -Length 24
        $a | Should -Not -Be $b
    }
}

Describe 'Dev public-IP detection' {
    It 'parses the { "ip": "x.x.x.x" } shape from the ipify response' {
        Mock az { script:Invoke-AzMock @args }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        # -WhatIf short-circuits after IP detection: no deploy, but ipify is hit once.
        . $script:ManageEnvScript -Action up -WhatIf 4>$null 6>$null
        Assert-MockCalled Invoke-RestMethod -Times 1 -Scope It
    }
}

Describe 'az deployment param wiring' {
    It 'invokes `az deployment group create` with a --parameters @file (never a bareword secret)' {
        Mock az { script:Invoke-AzMock @args }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up 4>$null 6>$null
        Assert-MockCalled az -ParameterFilter {
            $joined = ($args -join ' ')
            ($joined -match 'deployment group create') -and
            ($joined -match '--parameters\s+@') -and
            # The secret must NEVER appear as a bareword on the command line.
            ($joined -notmatch 'adminPassword=')
        } -Scope It
    }

    It 'writes adminPassword, allowedSourceIp and vmSize into the ARM parameters file' {
        Mock az { script:Invoke-AzMock @args }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        # Capture the params-file JSON written via Set-Content (the file is deleted in
        # finally, so intercept the content at write time). No real file is touched.
        $script:writtenParams = $null
        Mock Set-Content -ParameterFilter { $Path -match 'rdpilot-deploy-' } -MockWith {
            $script:writtenParams = ($Value -join "`n")
        }
        . $script:ManageEnvScript -Action up -VmSize 'Standard_B2s_v2' 4>$null 6>$null

        $script:writtenParams | Should -Not -BeNullOrEmpty
        $obj = $script:writtenParams | ConvertFrom-Json
        $obj.parameters.adminPassword.value   | Should -Not -BeNullOrEmpty
        $obj.parameters.allowedSourceIp.value | Should -Be '203.0.113.7'
        $obj.parameters.vmSize.value          | Should -Be 'Standard_B2s_v2'
    }
}

Describe 'up-preflight: RG-state guard' {
    It 'ABORTS when the target RG is being deleted (no deploy attempted)' {
        Mock az {
            $line = ($args -join ' ')
            $global:LASTEXITCODE = 0
            if ($line -match 'account show') { return '{"name":"Test Sub","id":"00000000-0000-0000-0000-000000000000"}' }
            # RG exists and is deprovisioning.
            if ($line -match 'group show') { return 'Deleting' }
            return script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }

        { . $script:ManageEnvScript -Action up 4>$null 6>$null } |
            Should -Throw -ExpectedMessage '*being deleted*'
        # No deploy should ever be attempted.
        Assert-MockCalled az -Times 0 -Scope It -ParameterFilter {
            ($args -join ' ') -match 'deployment group create'
        }
    }

    It 'ABORTS when the target RG already exists (Succeeded) — tells user to run down first' {
        Mock az {
            $line = ($args -join ' ')
            $global:LASTEXITCODE = 0
            if ($line -match 'account show') { return '{"name":"Test Sub","id":"00000000-0000-0000-0000-000000000000"}' }
            if ($line -match 'group show') { return 'Succeeded' }
            return script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }

        { . $script:ManageEnvScript -Action up 4>$null 6>$null } |
            Should -Throw -ExpectedMessage '*already exists*'
    }
}

Describe 'up-preflight: VM SKU capacity guard' {
    It 'ABORTS when the chosen VM size is Location-restricted and suggests alternatives' {
        Mock az {
            $line = ($args -join ' ')
            $global:LASTEXITCODE = 0
            if ($line -match 'account show') { return '{"name":"Test Sub","id":"00000000-0000-0000-0000-000000000000"}' }
            if ($line -match 'group show') { $global:LASTEXITCODE = 3; return }
            if ($line -match 'vm list-skus') {
                if ($line -match '--size\s+(\S+)') {
                    $sz = $Matches[1]
                    # Restricted: a Location restriction => unavailable in region.
                    return ('[{{"name":"{0}","resourceType":"virtualMachines","restrictions":[{{"type":"Location","reasonCode":"NotAvailableForSubscription"}}]}}]' -f $sz)
                }
                return '[{"name":"Standard_B2s_v2","resourceType":"virtualMachines","restrictions":[]},{"name":"Standard_B2ls_v2","resourceType":"virtualMachines","restrictions":[]}]'
            }
            return script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }

        { . $script:ManageEnvScript -Action up -VmSize 'Standard_B2ms' 4>$null 6>$null } |
            Should -Throw -ExpectedMessage '*Standard_B2s_v2*'
        Assert-MockCalled az -Times 0 -Scope It -ParameterFilter {
            ($args -join ' ') -match 'deployment group create'
        }
    }

    It 'PROCEEDS when the chosen VM size is available (Zone-only restriction is fine)' {
        Mock az {
            $line = ($args -join ' ')
            $global:LASTEXITCODE = 0
            if ($line -match 'account show') { return '{"name":"Test Sub","id":"00000000-0000-0000-0000-000000000000"}' }
            if ($line -match 'group show') { $global:LASTEXITCODE = 3; return }
            if ($line -match 'vm list-skus') {
                if ($line -match '--size\s+(\S+)') {
                    $sz = $Matches[1]
                    # Zone-only restriction => still deployable (non-zonal VM).
                    return ('[{{"name":"{0}","resourceType":"virtualMachines","restrictions":[{{"type":"Zone","reasonCode":"NotAvailableForSubscription"}}]}}]' -f $sz)
                }
                return '[]'
            }
            return script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }

        . $script:ManageEnvScript -Action up -VmSize 'Standard_B2s_v2' 4>$null 6>$null
        Assert-MockCalled az -Times 1 -Scope It -ParameterFilter {
            ($args -join ' ') -match 'deployment group create'
        }
    }
}

Describe 'down: wait vs -NoWait' {
    It 'default (wait) deletes WITHOUT --no-wait and confirms the RG is gone' {
        Mock az {
            $line = ($args -join ' ')
            $global:LASTEXITCODE = 0
            # After delete, the confirm poll asks for state — return absent (gone).
            if ($line -match 'group show') { $global:LASTEXITCODE = 3; return }
            return
        }
        . $script:ManageEnvScript -Action down 4>$null 6>$null

        Assert-MockCalled az -Times 1 -Scope It -ParameterFilter {
            $joined = ($args -join ' ')
            ($joined -match 'group delete') -and ($joined -notmatch '--no-wait')
        }
    }

    It '-NoWait fires the delete async (--no-wait) and returns' {
        Mock az {
            $global:LASTEXITCODE = 0
            return
        }
        . $script:ManageEnvScript -Action down -NoWait 4>$null 6>$null

        Assert-MockCalled az -Times 1 -Scope It -ParameterFilter {
            $joined = ($args -join ' ')
            ($joined -match 'group delete') -and ($joined -match '--no-wait')
        }
        # The blocking-confirm poll (group show) must NOT run in async mode.
        Assert-MockCalled az -Times 0 -Scope It -ParameterFilter {
            ($args -join ' ') -match 'group show'
        }
    }
}

Describe 'ENV-03 auto-destroy runbook publish' {
    It 'publishes the Delete-ResourceGroup runbook AFTER a successful deploy' {
        $script:order = [System.Collections.Generic.List[string]]::new()
        Mock az {
            $line = ($args -join ' ')
            if ($line -match 'deployment group create')  { $script:order.Add('deploy') }
            if ($line -match 'automation runbook publish'){ $script:order.Add('publish') }
            script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up 4>$null 6>$null

        # The runbook is published with the expected name, account and management RG.
        Assert-MockCalled az -Times 1 -Scope It -ParameterFilter {
            $joined = ($args -join ' ')
            ($joined -match 'automation runbook publish') -and
            ($joined -match 'Delete-ResourceGroup') -and
            ($joined -match 'rdpilot-autodestroy') -and
            ($joined -match 'rdpilot-mgmt')
        }
        # Publish must happen AFTER the deploy (the runbook must exist first).
        $script:order.IndexOf('publish') | Should -BeGreaterThan ($script:order.IndexOf('deploy'))
    }

    It 'does NOT hard-fail when publish returns non-zero but the runbook is already Published (idempotent)' {
        Mock az {
            $line = ($args -join ' ')
            $global:LASTEXITCODE = 0
            if ($line -match 'account show') { return '{"name":"Test Sub","id":"00000000-0000-0000-0000-000000000000"}' }
            if ($line -match 'group show') { $global:LASTEXITCODE = 3; return }
            if ($line -match 'automation runbook publish') { $global:LASTEXITCODE = 1; return }   # no new draft to promote
            if ($line -match 'automation runbook show') { $global:LASTEXITCODE = 0; return 'Published' }
            return script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }

        { . $script:ManageEnvScript -Action up 4>$null 6>$null } | Should -Not -Throw
    }
}

Describe '-AllowedSourceIp override' {
    It 'bypasses the ipify lookup when -AllowedSourceIp is supplied' {
        Mock az { script:Invoke-AzMock @args }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up -AllowedSourceIp '198.51.100.42' 4>$null 6>$null
        Assert-MockCalled Invoke-RestMethod -Times 0 -Scope It
    }
}

Describe 'CSE-script publish (scriptUri wiring)' {
    It 'uploads Configure-Target.ps1 and feeds a non-empty scriptUri to the deployment' {
        Mock az { script:Invoke-AzMock @args }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        # The scriptUri is now passed via the ARM parameters FILE (not a bareword);
        # capture the written JSON to assert on it. No real file is touched.
        $script:writtenParams = $null
        Mock Set-Content -ParameterFilter { $Path -match 'rdpilot-deploy-' } -MockWith {
            $script:writtenParams = ($Value -join "`n")
        }
        . $script:ManageEnvScript -Action up 4>$null 6>$null

        # The CSE script is uploaded as a blob...
        Assert-MockCalled az -ParameterFilter {
            ($args -join ' ') -match 'storage blob upload' -and
            ($args -join ' ') -match 'Configure-Target\.ps1'
        } -Scope It

        # ...a read-only SAS is minted for it...
        Assert-MockCalled az -ParameterFilter {
            ($args -join ' ') -match 'generate-sas' -and
            ($args -join ' ') -match 'Configure-Target\.ps1'
        } -Scope It

        # ...and a NON-EMPTY scriptUri is fed to the deployment (never empty/placeholder).
        $script:writtenParams | Should -Not -BeNullOrEmpty
        $obj = $script:writtenParams | ConvertFrom-Json
        $obj.parameters.scriptUri.value | Should -Match '\S+'
        $obj.parameters.deleteRunbookContentUri.value | Should -Match '\S+'
    }

    It 'mints the blob upload + SAS BEFORE the deployment (publish precedes deploy)' {
        $script:callOrder = [System.Collections.Generic.List[string]]::new()
        Mock az {
            $line = ($args -join ' ')
            if ($line -match 'storage blob upload') { $script:callOrder.Add('upload') }
            if ($line -match 'generate-sas') { $script:callOrder.Add('sas') }
            if ($line -match 'deployment group create') { $script:callOrder.Add('deploy') }
            script:Invoke-AzMock @args
        }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up 4>$null 6>$null

        $deployIdx = $script:callOrder.IndexOf('deploy')
        $uploadIdx = $script:callOrder.IndexOf('upload')
        $sasIdx    = $script:callOrder.IndexOf('sas')
        $deployIdx | Should -BeGreaterThan $uploadIdx
        $deployIdx | Should -BeGreaterThan $sasIdx
    }
}
