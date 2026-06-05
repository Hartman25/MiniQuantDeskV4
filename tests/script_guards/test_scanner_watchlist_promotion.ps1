# =============================================================================
# Script guard: WATCHLIST-PROMO-01
#
# Verifies that research-py/src/mqk_research/scanner/watchlist_promotion.py:
#  1. exists
#  2. contains "watchlist-v1" schema constant
#  3. contains approved_for_live=False (hard invariant present in code)
#  4. does NOT contain approved_for_live=True
#  5. contains approved_for_autonomous_paper
#  6. operator_review_approved defaults False
#  7. risk_simulation_passed defaults False
#  8. does NOT write to config/watchlists
#  9. has no broker/OMS/execution references
# 10. imports no network/DB modules
# 11. does not mutate DB
# 12. does not invoke mqk-backtest or subprocess
# 13. does not mention live promotion as automatic
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$Target = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\watchlist_promotion.py'
$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in watchlist_promotion.py")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in watchlist_promotion.py")
    }
}

# Guard 1: file exists
if (-not (Test-Path $Target)) {
    Write-Host "FAIL [G1]: watchlist_promotion.py does not exist at: $Target" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: watchlist_promotion.py exists"

$Content = Get-Content -Path $Target -Raw -Encoding UTF8

# Guard 2: contains watchlist-v1 schema string
Assert-Contains $Content "watchlist-v1" "G2-schema-version"
Write-Host "PASS [G2]: contains 'watchlist-v1'"

# Guard 3: approved_for_live=False present (hard invariant)
Assert-Contains $Content "approved_for_live=False" "G3-live-lock-false"
Write-Host "PASS [G3]: contains 'approved_for_live=False'"

# Guard 4: no approved_for_live=True assignment
Assert-NotContains $Content "approved_for_live=True" "G4-no-live-true"
Write-Host "PASS [G4]: no 'approved_for_live=True'"

# Guard 5: approved_for_autonomous_paper is referenced
Assert-Contains $Content "approved_for_autonomous_paper" "G5-autonomous-paper-field"
Write-Host "PASS [G5]: contains 'approved_for_autonomous_paper'"

# Guard 6: operator_review_approved defaults False
Assert-Contains $Content "operator_review_approved: bool = False" "G6-operator-review-default-false"
Write-Host "PASS [G6]: operator_review_approved defaults False"

# Guard 7: risk_simulation_passed defaults False
Assert-Contains $Content "risk_simulation_passed: bool = False" "G7-risk-simulation-default-false"
Write-Host "PASS [G7]: risk_simulation_passed defaults False"

# Guard 8: does NOT write to config/watchlists
Assert-NotContains $Content "config/watchlist" "G8-no-config-watchlists"
Write-Host "PASS [G8]: no config/watchlists write path"

# Guard 9: no broker/OMS/execution references
foreach ($forbidden in @(
    '/v2/orders',
    'submit_order',
    'live_routing_enabled=true',
    'oms_outbox',
    'oms_inbox',
    'BrokerGateway',
    'broker_adapter'
)) {
    Assert-NotContains $Content $forbidden "G9-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G9]: no broker/OMS/execution references"

# Guard 10: no network/DB imports
foreach ($forbidden in @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy'
)) {
    Assert-NotContains $Content $forbidden "G10-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G10]: no network/DB imports"

# Guard 11: does not mutate DB
foreach ($forbidden in @('db.execute', 'cursor.execute', 'session.commit', 'INSERT INTO', 'UPDATE SET')) {
    Assert-NotContains $Content $forbidden "G11-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G11]: no DB mutations"

# Guard 12: does not invoke mqk-backtest or subprocess
Assert-NotContains $Content "import mqk_backtest" "G12-no-import-mqk-backtest"
Assert-NotContains $Content "from mqk_backtest" "G12-no-from-mqk-backtest"
Assert-NotContains $Content "cargo run" "G12-no-cargo-run"
Assert-NotContains $Content "os.system" "G12-no-os-system"

if ($Content -match '(?m)^import subprocess') {
    $script:Failures.Add("FAIL [G12]: 'import subprocess' found at module level")
}
Write-Host "PASS [G12]: no mqk-backtest or subprocess"

# Guard 13: does not mention live promotion as automatic
foreach ($forbidden in @(
    'promote_to_live',
    'auto_promote',
    'eligible_for_live=True',
    'watchlist_promote_live'
)) {
    Assert-NotContains $Content $forbidden "G13-no-auto-promote-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G13]: no automatic live promotion references"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL WATCHLIST-PROMO-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "WATCHLIST-PROMO-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
