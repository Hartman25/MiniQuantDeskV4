# =============================================================================
# Script guard: WATCHLIST-PREMKT-RISK-BUNDLE-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/risk_simulation.py
#   research-py/src/mqk_research/scanner/premarket_revalidation.py
#   research-py/src/mqk_research/scanner/watchlist_promotion.py (integration)
#
# Guards:
#  G1.  risk_simulation.py exists
#  G2.  premarket_revalidation.py exists
#  G3.  approved_for_live=True absent from both modules
#  G4.  approved_for_live=False present in both modules
#  G5.  Risk failure reason strings present
#  G6.  Premarket failure reason strings present
#  G7.  No broker/OMS/order route references in risk or premarket modules
#  G8.  No network imports in risk or premarket modules
#  G9.  No DB imports in risk or premarket modules
#  G10. No subprocess in risk or premarket modules
#  G11. No daemon/runtime imports in risk or premarket modules
#  G12. watchlist_promotion.py still forces max_symbols_to_trade=1
#  G13. watchlist_promotion.py still forces max_concurrent_positions=1
#  G14. approved_for_live live lock remains false in watchlist_promotion.py
#  G15. approved_for_autonomous_paper only in watchlist_promotion (not risk/premarket except forced false)
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$RiskModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\risk_simulation.py'
$PremarketModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\premarket_revalidation.py'
$WatchlistModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\watchlist_promotion.py'

$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found")
    }
}

# Guard 1: risk_simulation.py exists
if (-not (Test-Path $RiskModule)) {
    Write-Host "FAIL [G1]: risk_simulation.py does not exist at: $RiskModule" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: risk_simulation.py exists"

# Guard 2: premarket_revalidation.py exists
if (-not (Test-Path $PremarketModule)) {
    Write-Host "FAIL [G2]: premarket_revalidation.py does not exist at: $PremarketModule" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G2]: premarket_revalidation.py exists"

$RiskContent = Get-Content -Path $RiskModule -Raw -Encoding UTF8
$PremarketContent = Get-Content -Path $PremarketModule -Raw -Encoding UTF8
$WatchlistContent = Get-Content -Path $WatchlistModule -Raw -Encoding UTF8

# Guard 3: No Python assignment sets approved_for_live to True in risk/premarket modules
# Check for dict-key assignment pattern (not docstring mentions of the value)
Assert-NotContains $RiskContent 'approved_for_live"] = True' "G3-risk-no-live-true-assignment"
Assert-NotContains $RiskContent 'approved_for_live: bool = True' "G3-risk-no-live-true-field"
Assert-NotContains $PremarketContent 'approved_for_live"] = True' "G3-premarket-no-live-true-assignment"
Assert-NotContains $PremarketContent 'approved_for_live: bool = True' "G3-premarket-no-live-true-field"
Write-Host "PASS [G3]: no approved_for_live=True assignment in risk or premarket modules"

# Guard 4: approved_for_live defaults False (config field) and is forced False (apply function)
Assert-Contains $RiskContent "approved_for_live: bool = False" "G4-risk-config-live-false"
Assert-Contains $PremarketContent "approved_for_live: bool = False" "G4-premarket-config-live-false"
Assert-Contains $RiskContent 'approved_for_live"] = False' "G4-risk-apply-forces-false"
Assert-Contains $PremarketContent 'approved_for_live"] = False' "G4-premarket-apply-forces-false"
Write-Host "PASS [G4]: approved_for_live=False present in risk and premarket modules"

# Guard 5: Risk failure reason strings present in risk_simulation.py
foreach ($reason in @(
    'risk_no_candidates',
    'risk_strategy_fit_missing',
    'risk_notional_limit_failed',
    'risk_daily_loss_failed',
    'risk_drawdown_stress_failed',
    'risk_concentration_failed',
    'risk_regime_mismatch',
    'risk_cost_adjusted_edge_failed',
    'risk_live_approval_forbidden',
    'risk_missing_required_metric'
)) {
    Assert-Contains $RiskContent $reason "G5-risk-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G5]: all risk failure reason strings present"

# Guard 6: Premarket failure reason strings present in premarket_revalidation.py
foreach ($reason in @(
    'premarket_watchlist_schema_invalid',
    'premarket_mode_not_paper',
    'premarket_live_approval_forbidden',
    'premarket_symbol_suppressed',
    'premarket_data_quality_failed',
    'premarket_liquidity_failed',
    'premarket_regime_incompatible',
    'premarket_bar_stale',
    'premarket_spread_too_wide',
    'premarket_slippage_too_high',
    'premarket_rvol_too_low',
    'premarket_price_too_low',
    'premarket_symbol_input_missing'
)) {
    Assert-Contains $PremarketContent $reason "G6-premarket-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G6]: all premarket failure reason strings present"

# Guard 7: No broker/OMS/order route references
foreach ($target in @($RiskContent, $PremarketContent)) {
    $label = if ($target -eq $RiskContent) { "risk" } else { "premarket" }
    foreach ($forbidden in @(
        '/v2/orders',
        'submit_order',
        'live_routing_enabled=true',
        'oms_outbox',
        'oms_inbox',
        'BrokerGateway',
        'broker_adapter'
    )) {
        Assert-NotContains $target $forbidden "G7-$label-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')"
    }
}
Write-Host "PASS [G7]: no broker/OMS/order references in risk or premarket modules"

# Guard 8: No network imports
foreach ($target in @($RiskContent, $PremarketContent)) {
    $label = if ($target -eq $RiskContent) { "risk" } else { "premarket" }
    foreach ($forbidden in @(
        'import requests',
        'import urllib',
        'import http.client',
        'import aiohttp'
    )) {
        Assert-NotContains $target $forbidden "G8-$label-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
    }
}
Write-Host "PASS [G8]: no network imports in risk or premarket modules"

# Guard 9: No DB imports
foreach ($target in @($RiskContent, $PremarketContent)) {
    $label = if ($target -eq $RiskContent) { "risk" } else { "premarket" }
    foreach ($forbidden in @('import psycopg', 'import sqlalchemy')) {
        Assert-NotContains $target $forbidden "G9-$label-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
    }
}
Write-Host "PASS [G9]: no DB imports in risk or premarket modules"

# Guard 10: No subprocess
foreach ($target in @($RiskContent, $PremarketContent)) {
    $label = if ($target -eq $RiskContent) { "risk" } else { "premarket" }
    Assert-NotContains $target "import subprocess" "G10-$label-no-subprocess"
    Assert-NotContains $target "os.system" "G10-$label-no-os-system"
}
Write-Host "PASS [G10]: no subprocess in risk or premarket modules"

# Guard 11: No daemon/runtime imports
foreach ($target in @($RiskContent, $PremarketContent)) {
    $label = if ($target -eq $RiskContent) { "risk" } else { "premarket" }
    foreach ($forbidden in @('from mqk_daemon', 'import mqk_daemon', 'from mqk_runtime', 'import mqk_runtime')) {
        Assert-NotContains $target $forbidden "G11-$label-no-daemon-$($forbidden -replace '[^a-zA-Z0-9]','_')"
    }
}
Write-Host "PASS [G11]: no daemon/runtime imports in risk or premarket modules"

# Guard 12: watchlist_promotion.py still forces max_symbols_to_trade=1
Assert-Contains $WatchlistContent 'max_symbols_to_trade"] = 1' "G12-promo-max-symbols-1"
Write-Host "PASS [G12]: watchlist_promotion forces max_symbols_to_trade=1"

# Guard 13: watchlist_promotion.py still forces max_concurrent_positions=1
Assert-Contains $WatchlistContent 'max_concurrent_positions"] = 1' "G13-promo-max-concurrent-1"
Write-Host "PASS [G13]: watchlist_promotion forces max_concurrent_positions=1"

# Guard 14: live lock remains false in watchlist_promotion.py
Assert-Contains $WatchlistContent 'approved_for_live=False' "G14-promo-live-lock-false"
Assert-NotContains $WatchlistContent 'approved_for_live=True' "G14-promo-no-live-true"
Write-Host "PASS [G14]: watchlist_promotion approved_for_live remains false"

# Guard 15: approved_for_autonomous_paper is only in watchlist_promotion (not risk/premarket)
if ($RiskContent -match 'approved_for_autonomous_paper') {
    $script:Failures.Add("FAIL [G15]: 'approved_for_autonomous_paper' found in risk_simulation.py (should only be in watchlist_promotion)")
}
if ($PremarketContent -match 'approved_for_autonomous_paper') {
    $script:Failures.Add("FAIL [G15]: 'approved_for_autonomous_paper' found in premarket_revalidation.py (should only be in watchlist_promotion)")
}
Assert-Contains $WatchlistContent 'approved_for_autonomous_paper' "G15-promo-has-autonomous-paper-field"
Write-Host "PASS [G15]: approved_for_autonomous_paper only in watchlist_promotion"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL WATCHLIST-PREMKT-RISK-BUNDLE-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "WATCHLIST-PREMKT-RISK-BUNDLE-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
