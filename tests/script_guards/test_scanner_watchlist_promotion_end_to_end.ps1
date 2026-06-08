# =============================================================================
# Script guard: WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01
#
# Verifies that research-py/tests/test_scanner_watchlist_promotion_end_to_end_artifact_chain.py:
#  1. exists
#  2. names the patch (WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01)
#  3. references every schema in the chain (watchlist-v1, strategy-fit-v1,
#     risk-simulation-v1, symbol-inputs-v1, premarket-revalidation-v1)
#  4. contains the required passing-chain proof (scenario A)
#  5. contains all ten required fail-closed proof scenarios (B1-B10)
#  6. asserts approved_for_live=False / recommended_for_live is never true
#     anywhere in the chain (the single forged-input fixture for B7 is the
#     only permitted "approved_for_live=True" occurrence, and it is verified
#     to be the deliberate forged-input call, not a derived/output value)
#  7. has no broker/OMS/execution/daemon references
#  8. imports no network/DB/subprocess modules
#  9. does not invoke mqk-backtest, cargo, or os.system
# 10. does not mention live promotion / live routing / signal injection as real
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$Target = Join-Path $RepoRoot 'research-py\tests\test_scanner_watchlist_promotion_end_to_end_artifact_chain.py'
$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in test_scanner_watchlist_promotion_end_to_end_artifact_chain.py")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in test_scanner_watchlist_promotion_end_to_end_artifact_chain.py")
    }
}

# Guard 1: file exists
if (-not (Test-Path $Target)) {
    Write-Host "FAIL [G1]: test_scanner_watchlist_promotion_end_to_end_artifact_chain.py does not exist at: $Target" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: test_scanner_watchlist_promotion_end_to_end_artifact_chain.py exists"

$Content = Get-Content -Path $Target -Raw -Encoding UTF8

# Guard 2: names the patch
Assert-Contains $Content "WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01" "G2-patch-name"
Write-Host "PASS [G2]: names WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01"

# Guard 3: references every schema in the chain
foreach ($schema in @(
    'watchlist-v1',
    'strategy-fit-v1',
    'risk-simulation-v1',
    'symbol-inputs-v1',
    'premarket-revalidation-v1'
)) {
    Assert-Contains $Content $schema "G3-schema-$($schema -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G3]: references all five chain schemas"

# Guard 4: passing-chain proof scenario (A)
Assert-Contains $Content "class TestPassingArtifactChain" "G4-passing-chain-class"
Assert-Contains $Content "def test_full_chain_produces_approved_paper_watchlist" "G4-passing-chain-test"
Write-Host "PASS [G4]: passing-chain proof scenario present"

# Guard 5: all ten required fail-closed proof scenarios (B1-B10)
foreach ($cls in @(
    'TestFailClosedMissingStrategyFit',            # B1
    'TestFailClosedStrategyFitNotRecommended',     # B2
    'TestFailClosedRiskSimulationFails',           # B3
    'TestFailClosedOperatorReviewMissing',         # B4
    'TestFailClosedPremarketMissingSymbolInput',   # B5
    'TestFailClosedPremarketStaleBar',             # B6
    'TestFailClosedForgedLiveFlag',                # B7
    'TestFailClosedMismatchedStrategy',            # B8
    'TestFailClosedMismatchedSymbol',              # B9
    'TestFailClosedMissingOrMalformedSymbolInputs' # B10
)) {
    Assert-Contains $Content "class $cls" "G5-failclosed-$cls"
}
Write-Host "PASS [G5]: all ten fail-closed scenarios (B1-B10) present"

# Guard 6: approved_for_live / recommended_for_live / eligible_for_live are
# never asserted or constructed as true anywhere — except the single
# deliberate forged-input fixture for B7 (verified below).
foreach ($forbidden in @(
    '"approved_for_live": True',
    "'approved_for_live': True",
    'recommended_for_live=True',
    '"recommended_for_live": True',
    "'recommended_for_live': True",
    'eligible_for_live=True',
    '"eligible_for_live": True',
    "'eligible_for_live': True"
)) {
    Assert-NotContains $Content $forbidden "G6-no-live-true-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Every textual occurrence of "approved_for_live=True" must be either the
# deliberate forged-input fixture call for B7, or a comment/docstring line
# describing that proof (never live code constructing/asserting a true value).
$Lines = $Content -split "`r?`n"
$FixtureCallCount = 0
$LineNum = 0
foreach ($line in $Lines) {
    $LineNum++
    # Case-sensitive: the Python boolean literal is "True"; prose narration of
    # the same proof in docstrings/comments uses lowercase "true" and must not
    # be mistaken for a code construct.
    if ($line -cmatch [regex]::Escape('approved_for_live=True')) {
        $trimmed = $line.Trim()
        if ($trimmed -cmatch '_make_ranked_watchlist\(approved_for_live=True\)') {
            $FixtureCallCount++
        } elseif ($trimmed.StartsWith('#')) {
            # Comment narration referencing the forged-flag proof — not code.
        } else {
            $Failures.Add("FAIL [G6]: line $($LineNum): 'approved_for_live=True' appears outside the forged-input fixture call or a comment: $trimmed")
        }
    }
}
if ($FixtureCallCount -ne 1) {
    $Failures.Add("FAIL [G6]: expected exactly 1 forged-input '_make_ranked_watchlist(approved_for_live=True)' fixture call for B7; found $FixtureCallCount")
}
Write-Host "PASS [G6]: approved_for_live/recommended_for_live/eligible_for_live never true except the single B7 forged-input fixture"

# Guard 7: no broker/OMS/execution/daemon references
foreach ($forbidden in @(
    '/v2/orders',
    'submit_order',
    'live_routing_enabled=true',
    'oms_outbox',
    'oms_inbox',
    'BrokerGateway',
    'broker_adapter',
    'mqk_daemon',
    'mqk-daemon',
    'mqk_runtime',
    'mqk-runtime'
)) {
    Assert-NotContains $Content $forbidden "G7-no-broker-daemon-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G7]: no broker/OMS/execution/daemon references"

# Guard 8: no network/DB/subprocess imports
foreach ($forbidden in @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy',
    'import subprocess'
)) {
    Assert-NotContains $Content $forbidden "G8-no-network-db-subprocess-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G8]: no network/DB/subprocess imports"

# Guard 9: does not invoke mqk-backtest, cargo, or os.system
Assert-NotContains $Content "import mqk_backtest" "G9-no-import-mqk-backtest"
Assert-NotContains $Content "from mqk_backtest" "G9-no-from-mqk-backtest"
Assert-NotContains $Content "cargo run" "G9-no-cargo-run"
Assert-NotContains $Content "os.system" "G9-no-os-system"
Write-Host "PASS [G9]: no mqk-backtest, cargo, or os.system invocation"

# Guard 10: does not mention live promotion / live routing / signal injection as real
foreach ($forbidden in @(
    'promote_to_live',
    'auto_promote',
    'watchlist_promote_live',
    'inject_signal',
    'place_order',
    'route_to_live'
)) {
    Assert-NotContains $Content $forbidden "G10-no-live-promotion-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G10]: no live-promotion / live-routing / signal-injection references"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
