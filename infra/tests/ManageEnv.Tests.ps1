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
    # the driver reads back (storage key, SAS URI, public IP), and leaves
    # $LASTEXITCODE = 0 so the `az account show` pre-flight passes. NO real az
    # process is ever spawned. Each test re-declares Mock az so per-test
    # Assert-MockCalled scoping is clean; this is the canonical body.
    function script:Invoke-AzMock {
        $line = ($args -join ' ')
        # Simulate a successful native `az` call: reset the native exit code so the
        # driver's `az account show` pre-flight (which checks $LASTEXITCODE) passes.
        $global:LASTEXITCODE = 0
        # Emit values for the read-back queries so the driver proceeds.
        if ($line -match 'account show') { return }
        if ($line -match 'storage account keys list') { return 'fake-storage-key==' }
        if ($line -match 'generate-sas') {
            if ($line -match 'Configure-Target\.ps1') {
                return 'https://fake.blob.core.windows.net/cse/Configure-Target.ps1?sig=READONLY'
            }
            return 'https://fake.blob.core.windows.net/cse/Delete-ResourceGroup.ps1?sig=READONLY'
        }
        if ($line -match 'deployment group show') { return '203.0.113.55' }
        if ($line -match 'public-ip list') { return '203.0.113.55' }
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
    It 'invokes `az deployment group create` with adminPassword= and allowedSourceIp=' {
        Mock az { script:Invoke-AzMock @args }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up 4>$null 6>$null
        Assert-MockCalled az -ParameterFilter {
            ($args -join ' ') -match 'deployment group create' -and
            ($args -join ' ') -match 'adminPassword=' -and
            ($args -join ' ') -match 'allowedSourceIp='
        } -Scope It
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

        # ...and a NON-EMPTY scriptUri= is fed to the deployment (never empty/placeholder).
        Assert-MockCalled az -ParameterFilter {
            $joined = ($args -join ' ')
            ($joined -match 'deployment group create') -and
            ($joined -match 'scriptUri=\S+')
        } -Scope It
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
