# =============================================================================
# Script guard: MARKET-DATA-COVERAGE-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/market_data_coverage.py
#   research-py/tests/test_scanner_market_data_coverage.py
#
# Read-only DB access, no daemon, no live calls, no .env.local, no secrets
# printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
$Module = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\market_data_coverage.py'
$TestFile = Join-Path $RepoRoot 'research-py\tests\test_scanner_market_data_coverage.py'

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

Write-Host ''
Write-Host '=== MARKET-DATA-COVERAGE-01 static invariant tests ==='
Write-Host "    Module: $Module"
Write-Host "    Tests : $TestFile"
Write-Host ''

if (-not (Test-Path $Module)) {
    Write-Host "FAIL [MDC01]: market_data_coverage.py does not exist at: $Module" -ForegroundColor Red
    exit 1
}
Write-Host 'PASS [MDC01]: market_data_coverage.py exists'

if (-not (Test-Path $TestFile)) {
    Write-Host "FAIL [MDC02]: test_scanner_market_data_coverage.py does not exist at: $TestFile" -ForegroundColor Red
    exit 1
}
Write-Host 'PASS [MDC02]: market_data_coverage tests exist'

$Content = Get-Content -Path $Module -Raw -Encoding UTF8
$TestContent = Get-Content -Path $TestFile -Raw -Encoding UTF8

Assert-Contains $Content 'market-data-coverage-v1' 'MDC03-schema'
foreach ($symbol in @(
    'class MarketDataCoverageConfig',
    'class MarketDataCoverageEntry',
    'class MarketDataCoverageResult',
    'def build_select_md_bars_coverage_statement',
    'def run_market_data_coverage_report',
    'def main'
)) {
    Assert-Contains $Content $symbol "MDC03-api-$($symbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC03]: schema version and public config/entry/result/runner API present'

foreach ($field in @(
    'symbols_ready_by_timeframe',
    'symbols_missing_by_timeframe',
    'row_count',
    'first_end_ts',
    'latest_end_ts',
    'latest_end_utc',
    'staleness_minutes',
    'is_fresh',
    'gap_count',
    'missing_reason',
    'ready_for_backtest',
    'ready_for_paper'
)) {
    Assert-Contains $Content $field "MDC04-field-$($field -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC04]: required coverage schema fields present'

foreach ($reason in @(
    'market_data_coverage_missing_database_url',
    'market_data_coverage_no_symbols_requested',
    'market_data_coverage_no_timeframes_requested',
    'market_data_coverage_database_query_failed',
    'market_data_coverage_no_rows_for_symbol',
    'market_data_coverage_no_coverage_data',
    'market_data_coverage_stale_data'
)) {
    Assert-Contains $Content $reason "MDC05-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC05]: stable reason constants present'

Assert-Contains $Content 'normalize_database_url' 'MDC06-reuses-url-normalizer'
Write-Host 'PASS [MDC06]: coverage module reuses market_data_export.normalize_database_url'

$UpperContent = $Content.ToUpperInvariant()
foreach ($forbiddenSql in @('INSERT ', 'UPDATE ', 'DELETE ', 'DROP ', 'ALTER ', 'TRUNCATE ', 'CREATE TABLE')) {
    Assert-NotContains $UpperContent $forbiddenSql "MDC07-read-only-sql-$($forbiddenSql -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC07]: no DB write SQL verbs in market_data_coverage module'

foreach ($forbidden in @(
    'from mqk_daemon',
    'import mqk_daemon',
    'from mqk_runtime',
    'import mqk_runtime',
    'from mqk_execution',
    'import mqk_execution',
    'BrokerGateway',
    'broker_adapter',
    'oms_outbox',
    'oms_inbox'
)) {
    Assert-NotContains $Content $forbidden "MDC08-no-daemon-oms-runtime-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC08]: no daemon/runtime/OMS/broker adapter imports or references'

foreach ($forbidden in @(
    '/api/v1/strategy/signal',
    'strategy/signal',
    '/v2/orders',
    'submit_order',
    'place_order',
    'enqueue_outbox'
)) {
    Assert-NotContains $Content $forbidden "MDC09-no-order-signal-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC09]: no order or strategy-signal endpoint references'

foreach ($forbidden in @('import subprocess', 'os.system')) {
    Assert-NotContains $Content $forbidden "MDC10-no-subprocess-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC10]: no subprocess or shell execution'

foreach ($forbidden in @(
    'approved_for_live=True',
    'approved_for_live: bool = True',
    'approved_for_live = True',
    'recommended_for_live=True',
    'eligible_for_live=True'
)) {
    Assert-NotContains $Content $forbidden "MDC11-no-live-true-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC11]: no live/recommended/eligible true literals in module'

foreach ($testSymbol in @(
    'TestCoverageHardInvariants',
    'TestCoverageBlocksFailClosed',
    'TestCoverageStatuses',
    'TestCoverageFreshness',
    'TestCoverageGapCount',
    'TestCoverageSchema',
    'test_cli_prints_blocked_json_without_database_url'
)) {
    Assert-Contains $TestContent $testSymbol "MDC12-test-$($testSymbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDC12]: required unit coverage anchors present (invariants, blocking, statuses, freshness, gaps, schema, CLI)'

$DaemonDiff = @()
try {
    $DaemonDiff = @(git -C $RepoRoot diff --name-only -- 'core-rs/crates/mqk-daemon')
    $DaemonDiff += @(git -C $RepoRoot diff --cached --name-only -- 'core-rs/crates/mqk-daemon')
} catch {
    $script:Failures.Add("FAIL [MDC13]: could not inspect daemon diff: $_")
}
if ($DaemonDiff.Count -eq 0) {
    Write-Host 'PASS [MDC13]: no daemon enforcement files are modified or staged'
} else {
    $script:Failures.Add("FAIL [MDC13]: daemon files modified/staged: $($DaemonDiff -join ', ')")
}

Write-Host ''
if ($Failures.Count -eq 0) {
    Write-Host 'ALL MARKET-DATA-COVERAGE-01 GUARDS PASSED' -ForegroundColor Green
    exit 0
} else {
    foreach ($failure in $Failures) {
        Write-Host $failure -ForegroundColor Red
    }
    Write-Host ''
    Write-Host "MARKET-DATA-COVERAGE-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
