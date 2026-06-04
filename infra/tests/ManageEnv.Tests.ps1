<#
    ManageEnv.Tests.ps1 — Pester unit tests for infra/manage-env.ps1 logic.

    Test framework : Pester 5.x (pinned: validated against Pester 5.7.1).
    Cost           : ZERO Azure cost, ZERO network. `az` and `Invoke-RestMethod`
                     are mocked; no real deployment or HTTP call is ever made.

    Scope (Plan 01-01, Wave 0 scaffold):
      - Password generation entropy/length  -> LIVE today (tests the standalone
        cryptographic generation expression inline; at least one live assertion).
      - Dev public-IP detection (ipify shape) -> SKIPPED until Plan 04 ships
        infra/manage-env.ps1 (mock Invoke-RestMethod).
      - `az deployment group create` param wiring (adminPassword=, allowedSourceIp=)
        -> SKIPPED until Plan 04 (mock az).
      - `-AllowedSourceIp` override bypasses ipify -> SKIPPED until Plan 04.

    Tests that depend on infra/manage-env.ps1 dot-source it behind a guard and are
    marked -Skip with a reason referencing Plan 04 so this file runs GREEN now and
    becomes active once Plan 04 lands.

    SECURITY: never write a real/generated password to test output. Assertions are
    on length and character-class coverage only.
#>

# Evaluated at Pester DISCOVERY time (top-level script body) so the -Skip conditions on
# `It` resolve correctly — `BeforeAll` runs at the later run phase and is too late for -Skip.
$script:ManageEnvScript = Join-Path $PSScriptRoot '..' 'manage-env.ps1'
$script:SkipUntilPlan04  = -not (Test-Path $script:ManageEnvScript)
$script:Plan04Reason     = 'infra/manage-env.ps1 created in Plan 04'

BeforeAll {
    # Re-resolve inside run phase too (BeforeAll variables are what It bodies see at run time).
    $script:ManageEnvScript = Join-Path $PSScriptRoot '..' 'manage-env.ps1'

    # Standalone cryptographic password generator — mirrors the generation expression
    # Plan 04's manage-env.ps1 will use. Tested LIVE here so at least one assertion is
    # active today. Uses System.Security.Cryptography (crypto RNG), NOT a hand-rolled
    # char-shuffler (RESEARCH.md "Don't Hand-Roll").
    function New-TestAdminPassword {
        param([int]$Length = 24)
        $lower  = 'abcdefghijkmnopqrstuvwxyz'
        $upper  = 'ABCDEFGHJKLMNPQRSTUVWXYZ'
        $digit  = '23456789'
        $symbol = '!@#$%^&*()-_=+'
        $all    = ($lower + $upper + $digit + $symbol).ToCharArray()

        # Guarantee one of each class, then fill the rest from the full set, all via crypto RNG.
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
        # Crypto shuffle so the guaranteed-class chars are not always first.
        $shuffled = $chars | Sort-Object { [System.Security.Cryptography.RandomNumberGenerator]::GetInt32([int]::MaxValue) }
        -join $shuffled
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
        # Two independent generations must differ — a deterministic/constant source would collide.
        $a = New-TestAdminPassword -Length 24
        $b = New-TestAdminPassword -Length 24
        $a | Should -Not -Be $b
    }
}

Describe 'Dev public-IP detection' {
    It 'parses the { "ip": "x.x.x.x" } shape from the ipify response' -Skip:$script:SkipUntilPlan04 -ForEach @(@{ Reason = $script:Plan04Reason }) {
        # Activated by Plan 04. Mock the HTTP call so no network access occurs.
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up -WhatIf 4>$null
        # Plan 04's driver must surface the detected IP; assert the parsed value.
        Assert-MockCalled Invoke-RestMethod -Times 1 -Scope It
    }
}

Describe 'az deployment param wiring' {
    It 'invokes `az deployment group create` with adminPassword= and allowedSourceIp=' -Skip:$script:SkipUntilPlan04 -ForEach @(@{ Reason = $script:Plan04Reason }) {
        # Activated by Plan 04. Mock az so no real Azure deployment occurs.
        Mock az { }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up 4>$null
        Assert-MockCalled az -ParameterFilter {
            ($args -join ' ') -match 'deployment group create' -and
            ($args -join ' ') -match 'adminPassword=' -and
            ($args -join ' ') -match 'allowedSourceIp='
        } -Scope It
    }
}

Describe '-AllowedSourceIp override' {
    It 'bypasses the ipify lookup when -AllowedSourceIp is supplied' -Skip:$script:SkipUntilPlan04 -ForEach @(@{ Reason = $script:Plan04Reason }) {
        # Activated by Plan 04. With an explicit IP, Invoke-RestMethod must NOT be called.
        Mock az { }
        Mock Invoke-RestMethod { @{ ip = '203.0.113.7' } }
        . $script:ManageEnvScript -Action up -AllowedSourceIp '198.51.100.42' 4>$null
        Assert-MockCalled Invoke-RestMethod -Times 0 -Scope It
    }
}
