# =============================================================================
# Script guard: MARKET-DATA-EXPORT-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/market_data_export.py
#   research-py/tests/test_scanner_market_data_export.py
#
# No daemon, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
$Module = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\market_data_export.py'
$TestFile = Join-Path $RepoRoot 'research-py\tests\test_scanner_market_data_export.py'

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
Write-Host '=== MARKET-DATA-EXPORT-01 static invariant tests ==='
Write-Host "    Module: $Module"
Write-Host "    Tests : $TestFile"
Write-Host ''

if (-not (Test-Path $Module)) {
    Write-Host "FAIL [MDE01]: market_data_export.py does not exist at: $Module" -ForegroundColor Red
    exit 1
}
Write-Host 'PASS [MDE01]: market_data_export.py exists'

if (-not (Test-Path $TestFile)) {
    Write-Host "FAIL [MDE02]: test_scanner_market_data_export.py does not exist at: $TestFile" -ForegroundColor Red
    exit 1
}
Write-Host 'PASS [MDE02]: market_data_export tests exist'

$Content = Get-Content -Path $Module -Raw -Encoding UTF8
$TestContent = Get-Content -Path $TestFile -Raw -Encoding UTF8

foreach ($symbol in @(
    'class MarketDataExportConfig',
    'class MarketDataExportResult',
    'def export_md_bars_to_bars_root',
    'def build_select_md_bars_statement',
    'def write_symbol_timeframe_csv',
    'def normalize_db_row_to_csv_row',
    'def main'
)) {
    Assert-Contains $Content $symbol "MDE03-api-$($symbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE03]: public config/result/export API and helpers present'

foreach ($reason in @(
    'market_data_export_missing_database_url',
    'market_data_export_no_symbols_requested',
    'market_data_export_no_rows_for_symbol',
    'market_data_export_no_rows_exported',
    'market_data_export_database_query_failed',
    'market_data_export_output_path_escape'
)) {
    Assert-Contains $Content $reason "MDE04-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE04]: stable reason constants present'

Assert-Contains $TestContent 'normalize_bars_csv' 'MDE05-normalize-bars-csv-import'
Assert-Contains $TestContent 'test_exported_csv_is_parseable_by_symbol_inputs_runner' 'MDE05-normalize-bars-csv-compat-test'
Write-Host 'PASS [MDE05]: normalize_bars_csv compatibility test exists'

$UpperContent = $Content.ToUpperInvariant()
foreach ($forbiddenSql in @('INSERT ', 'UPDATE ', 'DELETE ', 'DROP ', 'ALTER ', 'TRUNCATE ', 'CREATE TABLE')) {
    Assert-NotContains $UpperContent $forbiddenSql "MDE06-read-only-sql-$($forbiddenSql -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE06]: no DB write SQL verbs in market_data_export module'

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
    Assert-NotContains $Content $forbidden "MDE07-no-daemon-oms-runtime-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE07]: no daemon/runtime/OMS/broker adapter imports or references'

foreach ($forbidden in @(
    '/api/v1/strategy/signal',
    'strategy/signal',
    '/v2/orders',
    'submit_order',
    'place_order',
    'enqueue_outbox'
)) {
    Assert-NotContains $Content $forbidden "MDE08-no-order-signal-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE08]: no order or strategy-signal endpoint references'

foreach ($forbidden in @('import subprocess', 'os.system')) {
    Assert-NotContains $Content $forbidden "MDE09-no-subprocess-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE09]: no subprocess or shell execution'

foreach ($forbidden in @(
    'approved_for_live=True',
    'approved_for_live: bool = True',
    'approved_for_live = True',
    'recommended_for_live=True',
    'eligible_for_live=True'
)) {
    Assert-NotContains $Content $forbidden "MDE10-no-live-true-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE10]: no live/recommended/eligible true literals in module'

$DaemonDiff = @()
try {
    $DaemonDiff = @(git -C $RepoRoot diff --name-only -- 'core-rs/crates/mqk-daemon')
    $DaemonDiff += @(git -C $RepoRoot diff --cached --name-only -- 'core-rs/crates/mqk-daemon')
} catch {
    $script:Failures.Add("FAIL [MDE11]: could not inspect daemon diff: $_")
}
if ($DaemonDiff.Count -eq 0) {
    Write-Host 'PASS [MDE11]: no daemon enforcement files are modified or staged'
} else {
    $script:Failures.Add("FAIL [MDE11]: daemon files modified/staged: $($DaemonDiff -join ', ')")
}

Assert-Contains $Content 'def normalize_database_url' 'MDE12-normalize-database-url'
Assert-Contains $Content 'postgresql+psycopg://' 'MDE12-psycopg-driver-rewrite'
Write-Host 'PASS [MDE12]: normalize_database_url rewrites bare postgres(ql):// to psycopg3 driver'

Assert-Contains $Content 'cast(:start_end_ts as bigint)' 'MDE13-cast-start-end-ts'
Assert-Contains $Content 'cast(:end_end_ts as bigint)' 'MDE13-cast-end-end-ts'
Write-Host 'PASS [MDE13]: time-window query params are cast to bigint (psycopg3 AmbiguousParameter fix)'

foreach ($testSymbol in @(
    'TestDatabaseUrlNormalization',
    'test_query_casts_nullable_time_window_params_to_bigint',
    'test_1d_timeframe_writes_symbol_1d_csv'
)) {
    Assert-Contains $TestContent $testSymbol "MDE14-test-$($testSymbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDE14]: URL normalization, SQL cast, and 1D export coverage tests present'

Write-Host ''
if ($Failures.Count -eq 0) {
    Write-Host 'ALL MARKET-DATA-EXPORT-01 GUARDS PASSED' -ForegroundColor Green
    exit 0
} else {
    foreach ($failure in $Failures) {
        Write-Host $failure -ForegroundColor Red
    }
    Write-Host ''
    Write-Host "MARKET-DATA-EXPORT-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
