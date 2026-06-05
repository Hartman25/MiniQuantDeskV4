# =============================================================================
# Script guard: BACKTEST-QUEUE-01
#
# Verifies that research-py/src/mqk_research/scanner/backtest_queue.py:
#  - exists
#  - contains the required schema, strategy IDs, and policy constants
#  - preserves recommended_for_live=False (never True)
#  - has no broker/OMS/execution/network/DB imports or references
#  - does not invoke mqk-backtest or cargo
#  - writes JSON artifacts only
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$Target = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_queue.py'
$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in backtest_queue.py")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in backtest_queue.py")
    }
}

# Guard 1: file exists
if (-not (Test-Path $Target)) {
    Write-Host "FAIL [G1]: backtest_queue.py does not exist at: $Target" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: backtest_queue.py exists"

$Content = Get-Content -Path $Target -Raw -Encoding UTF8

# Guard 2: contains schema version
Assert-Contains $Content "backtest-queue-v1" "G2-schema-version"

# Guard 3: required strategy IDs present
foreach ($sid in @('intraday_scalper', 'volatility_breakout', 'mean_reversion', 'swing_momentum', 'vwap_mean_reversion', 'opening_range_fade')) {
    Assert-Contains $Content $sid "G3-strategy-$sid"
}

# Guard 4: CONSERVATIVE_WORST_CASE present
Assert-Contains $Content "CONSERVATIVE_WORST_CASE" "G4-ambiguity-policy"

# Guard 5: slippage_x2 present
Assert-Contains $Content "slippage_x2" "G5-stress-profile"

# Guard 6: recommended_for_live=False present (hard invariant)
Assert-Contains $Content "recommended_for_live=False" "G6-live-lock-false"

# Guard 7: no recommended_for_live=True (hard invariant — must never be True)
Assert-NotContains $Content "recommended_for_live=True" "G7-no-live-true"

# Guard 8: no broker/OMS/execution references
foreach ($forbidden in @('/v2/orders', 'submit_order', 'live_routing_enabled=true', 'oms_outbox', 'oms_inbox', 'BrokerGateway', 'broker_adapter')) {
    Assert-NotContains $Content $forbidden "G8-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 9: no network/DB imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import psycopg', 'import sqlalchemy')) {
    Assert-NotContains $Content $forbidden "G9-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 10: does not import or execute mqk-backtest or cargo run
# (Comments/docstrings may reference "mqk-backtest" descriptively; check for executable patterns)
Assert-NotContains $Content "import mqk_backtest" "G10-no-import-mqk-backtest"
Assert-NotContains $Content "from mqk_backtest" "G10-no-from-mqk-backtest"
Assert-NotContains $Content "subprocess" "G10-no-subprocess"
Assert-NotContains $Content "cargo run" "G10-no-cargo-run"
Assert-NotContains $Content "os.system" "G10-no-os-system"

# Guard 11: does not mutate DB (no DB write calls)
foreach ($forbidden in @('db.execute', 'cursor.execute', 'session.commit', 'INSERT INTO', 'UPDATE SET')) {
    Assert-NotContains $Content $forbidden "G11-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 12: writes JSON artifacts (json.dumps present)
Assert-Contains $Content "json.dumps" "G12-json-artifact-write"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL BACKTEST-QUEUE-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "BACKTEST-QUEUE-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
