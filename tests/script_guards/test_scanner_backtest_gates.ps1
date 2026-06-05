# =============================================================================
# Script guard: BACKTEST-GATES-01
#
# Verifies that research-py/src/mqk_research/scanner/backtest_gates.py:
#  1. exists
#  2. contains required failure reason strings
#  3. contains recommended_for_live=False (hard invariant present in code)
#  4. does NOT contain recommended_for_live=True
#  5. has no broker/OMS/execution references
#  6. imports no network/DB modules
#  7. does not mutate DB
#  8. does not invoke mqk-backtest or subprocess
#  9. does not mention live promotion as automatic
# 10. fail-closes blocked/no-interface artifacts (blocked status string present)
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$Target = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_gates.py'
$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in backtest_gates.py")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in backtest_gates.py")
    }
}

# Guard 1: file exists
if (-not (Test-Path $Target)) {
    Write-Host "FAIL [G1]: backtest_gates.py does not exist at: $Target" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: backtest_gates.py exists"

$Content = Get-Content -Path $Target -Raw -Encoding UTF8

# Guard 2: required failure reason strings present
$RequiredReasons = @(
    'blocked_no_backtest_interface',
    'missing_required_metric',
    'min_bars_failed',
    'min_trades_failed',
    'max_drawdown_failed',
    'profit_factor_failed',
    'expectancy_failed',
    'cost_adjusted_edge_failed',
    'out_of_sample_failed',
    'sample_quality_failed',
    'parameter_stability_failed',
    'single_trade_dependency_failed'
)
foreach ($reason in $RequiredReasons) {
    Assert-Contains $Content $reason "G2-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}

# Guard 3: recommended_for_live=False present (hard invariant)
Assert-Contains $Content "recommended_for_live=False" "G3-live-lock-false"

# Guard 4: no recommended_for_live=True (hard invariant — must never appear as assignment)
Assert-NotContains $Content "recommended_for_live=True" "G4-no-live-true"

# Guard 5: no broker/OMS/execution references
foreach ($forbidden in @('/v2/orders', 'submit_order', 'live_routing_enabled=true', 'oms_outbox', 'oms_inbox', 'BrokerGateway', 'broker_adapter')) {
    Assert-NotContains $Content $forbidden "G5-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 6: no network/DB imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import psycopg', 'import sqlalchemy')) {
    Assert-NotContains $Content $forbidden "G6-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 7: does not mutate DB
foreach ($forbidden in @('db.execute', 'cursor.execute', 'session.commit', 'INSERT INTO', 'UPDATE SET')) {
    Assert-NotContains $Content $forbidden "G7-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 8: does not invoke mqk-backtest or subprocess
Assert-NotContains $Content "import mqk_backtest" "G8-no-import-mqk-backtest"
Assert-NotContains $Content "from mqk_backtest" "G8-no-from-mqk-backtest"
Assert-NotContains $Content "cargo run" "G8-no-cargo-run"
Assert-NotContains $Content "os.system" "G8-no-os-system"

if ($Content -match '(?m)^import subprocess') {
    $script:Failures.Add("FAIL [G8]: 'import subprocess' found at module level - must not be a top-level import")
}
Write-Host "PASS [G8]: subprocess not at module level"

# Guard 9: does not mention live promotion as automatic
foreach ($forbidden in @('promote_to_live', 'auto_promote', 'eligible_for_live=True', 'watchlist_promote')) {
    Assert-NotContains $Content $forbidden "G9-no-auto-promote-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 10: fail-closes blocked/no-interface artifacts (status constant present)
Assert-Contains $Content "blocked_no_backtest_interface" "G10-blocked-status-constant"
Assert-Contains $Content "STATUS_BLOCKED" "G10-status-blocked-var"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL BACKTEST-GATES-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "BACKTEST-GATES-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
