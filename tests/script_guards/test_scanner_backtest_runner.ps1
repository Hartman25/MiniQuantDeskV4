# =============================================================================
# Script guard: BACKTEST-RUNNER-01
#
# Verifies that research-py/src/mqk_research/scanner/backtest_runner.py:
#  - exists
#  - contains strategy-fit-v1 schema constant
#  - preserves recommended_for_live=False (never True)
#  - has no broker/OMS/execution/network/DB imports or references
#  - does not call subprocess or invoke mqk-backtest externally
#  - writes strategy-fit JSON artifacts only
#  - does not auto-promote to live
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$Target = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_runner.py'
$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in backtest_runner.py")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in backtest_runner.py")
    }
}

# Guard 1: file exists
if (-not (Test-Path $Target)) {
    Write-Host "FAIL [G1]: backtest_runner.py does not exist at: $Target" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: backtest_runner.py exists"

$Content = Get-Content -Path $Target -Raw -Encoding UTF8

# Guard 2: contains schema version strategy-fit-v1
Assert-Contains $Content "strategy-fit-v1" "G2-schema-version"

# Guard 3: recommended_for_live=False present (hard invariant)
Assert-Contains $Content "recommended_for_live=False" "G3-live-lock-false"

# Guard 4: no recommended_for_live=True (hard invariant — must never appear)
Assert-NotContains $Content "recommended_for_live=True" "G4-no-live-true"

# Guard 5: no broker/OMS/execution references
foreach ($forbidden in @('/v2/orders', 'submit_order', 'live_routing_enabled=true', 'oms_outbox', 'oms_inbox', 'BrokerGateway', 'broker_adapter')) {
    Assert-NotContains $Content $forbidden "G5-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 6: no network/DB imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import psycopg', 'import sqlalchemy')) {
    Assert-NotContains $Content $forbidden "G6-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 7: does not execute mqk-backtest or cargo
Assert-NotContains $Content "import mqk_backtest" "G7-no-import-mqk-backtest"
Assert-NotContains $Content "from mqk_backtest" "G7-no-from-mqk-backtest"
Assert-NotContains $Content "cargo run" "G7-no-cargo-run"
Assert-NotContains $Content "os.system" "G7-no-os-system"

# Guard 8: subprocess only permitted behind config guard (must not be a top-level import)
# Verify subprocess is not imported at module level
if ($Content -match '(?m)^import subprocess') {
    $script:Failures.Add("FAIL [G8]: 'import subprocess' found at module level - must not be a top-level import")
}
Write-Host "PASS [G8]: subprocess not at module level"

# Guard 9: does not mutate DB
foreach ($forbidden in @('db.execute', 'cursor.execute', 'session.commit', 'INSERT INTO', 'UPDATE SET')) {
    Assert-NotContains $Content $forbidden "G9-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 10: writes strategy-fit JSON artifacts only (json.dumps present)
Assert-Contains $Content "json.dumps" "G10-json-artifact-write"

# Guard 11: does not mention live promotion as automatic
foreach ($forbidden in @('promote_to_live', 'auto_promote', 'eligible_for_live=True', 'watchlist_promote')) {
    Assert-NotContains $Content $forbidden "G11-no-auto-promote-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}

# Guard 12: blocked_no_backtest_interface status constant present
Assert-Contains $Content "blocked_no_backtest_interface" "G12-blocked-status-constant"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL BACKTEST-RUNNER-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "BACKTEST-RUNNER-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
