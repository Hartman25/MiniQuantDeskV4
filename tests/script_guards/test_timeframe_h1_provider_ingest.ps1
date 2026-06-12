# =============================================================================
# Script guard: TIMEFRAME-H1-PROVIDER-INGEST-01
#
# Verifies provider-native 1h/H1 timeframe support is wired through:
#   core-rs/crates/mqk-md/src/lib.rs               (Timeframe::H1, parse, as_str,
#                                                    as_twelvedata_interval)
#   core-rs/crates/mqk-md/src/alpaca_provider.rs   (alpaca_timeframe -> "1Hour")
#   core-rs/crates/mqk-cli/src/commands/md.rs      (CHUNK_DAYS_1H, default overlap)
#   research-py/src/mqk_research/scanner/market_data_coverage.py
#                                                   (1h freshness config + branch)
#
# Also re-asserts existing 5m/1D market-data infra-only invariants: no daemon,
# no runtime/execution/OMS, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')

$MdLib = Join-Path $RepoRoot 'core-rs\crates\mqk-md\src\lib.rs'
$AlpacaProvider = Join-Path $RepoRoot 'core-rs\crates\mqk-md\src\alpaca_provider.rs'
$CliMd = Join-Path $RepoRoot 'core-rs\crates\mqk-cli\src\commands\md.rs'
$CoverageModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\market_data_coverage.py'
$CoverageTests = Join-Path $RepoRoot 'research-py\tests\test_scanner_market_data_coverage.py'
$ExportTests = Join-Path $RepoRoot 'research-py\tests\test_scanner_market_data_export.py'

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
Write-Host '=== TIMEFRAME-H1-PROVIDER-INGEST-01 static invariant tests ==='
Write-Host ''

foreach ($pathInfo in @(
    @{ Path = $MdLib; Label = 'H101-mqk-md-lib' },
    @{ Path = $AlpacaProvider; Label = 'H101-alpaca-provider' },
    @{ Path = $CliMd; Label = 'H101-cli-md' },
    @{ Path = $CoverageModule; Label = 'H101-coverage-module' },
    @{ Path = $CoverageTests; Label = 'H101-coverage-tests' },
    @{ Path = $ExportTests; Label = 'H101-export-tests' }
)) {
    if (-not (Test-Path $pathInfo.Path)) {
        Write-Host "FAIL [$($pathInfo.Label)]: file does not exist: $($pathInfo.Path)" -ForegroundColor Red
        exit 1
    }
}
Write-Host 'PASS [H101]: all required source/test files exist'

$MdLibContent = Get-Content -Path $MdLib -Raw -Encoding UTF8
$AlpacaContent = Get-Content -Path $AlpacaProvider -Raw -Encoding UTF8
$CliMdContent = Get-Content -Path $CliMd -Raw -Encoding UTF8
$CoverageContent = Get-Content -Path $CoverageModule -Raw -Encoding UTF8
$CoverageTestContent = Get-Content -Path $CoverageTests -Raw -Encoding UTF8
$ExportTestContent = Get-Content -Path $ExportTests -Raw -Encoding UTF8

# ---------------------------------------------------------------------------
# Rust: mqk-md Timeframe enum
# ---------------------------------------------------------------------------

Assert-Contains $MdLibContent 'H1,' 'H102-timeframe-h1-variant'
Assert-Contains $MdLibContent '"1h" | "h1" | "60m" => Ok(Timeframe::H1)' 'H102-parse-h1-aliases'
Assert-Contains $MdLibContent 'Timeframe::H1 => "1h"' 'H102-as-str-h1'
Write-Host 'PASS [H102]: mqk_md::Timeframe::H1 variant, parse aliases, and as_str present'

# 5m/1D unchanged canonical mappings still present.
Assert-Contains $MdLibContent 'Timeframe::M5 => "5m"' 'H103-m5-unchanged'
Assert-Contains $MdLibContent 'Timeframe::D1 => "1D"' 'H103-d1-unchanged'
Assert-Contains $MdLibContent '"5m" | "5min" | "5minute" => Ok(Timeframe::M5)' 'H103-m5-parse-unchanged'
Assert-Contains $MdLibContent '"1d" => Ok(Timeframe::D1)' 'H103-d1-parse-unchanged'
Write-Host 'PASS [H103]: 5m/1D canonical mappings unchanged'

# Unknown timeframe still fails closed via anyhow Err.
Assert-Contains $MdLibContent 'invalid timeframe' 'H104-unknown-timeframe-error'
Write-Host 'PASS [H104]: unknown timeframe still fails closed (anyhow Err)'

# ---------------------------------------------------------------------------
# Rust: provider request mapping
# ---------------------------------------------------------------------------

Assert-Contains $MdLibContent 'Timeframe::H1 => "1h"' 'H105-twelvedata-h1-interval'
Assert-Contains $AlpacaContent 'crate::Timeframe::H1 => "1Hour"' 'H105-alpaca-h1-interval'
Write-Host 'PASS [H105]: H1 maps to provider-native hourly interval (TwelveData "1h", Alpaca "1Hour")'

# ---------------------------------------------------------------------------
# Rust: CLI sync/ingest chunk + overlap handling
# ---------------------------------------------------------------------------

Assert-Contains $CliMdContent 'CHUNK_DAYS_1H' 'H106-chunk-days-1h-const'
Assert-Contains $CliMdContent 'mqk_md::Timeframe::H1 => CHUNK_DAYS_1H' 'H106-chunk-days-1h-arm'
Assert-Contains $CliMdContent 'mqk_md::Timeframe::H1 => 2' 'H106-default-overlap-1h-arm'
Write-Host 'PASS [H106]: mqk-cli md ingest-provider/sync-provider handle H1 chunk size and overlap'

# ---------------------------------------------------------------------------
# Rust: tests present
# ---------------------------------------------------------------------------

foreach ($testSymbol in @(
    'timeframe_parse_h1_aliases',
    'timeframe_h1_as_str_and_twelvedata_interval',
    'timeframe_m5_and_d1_unchanged'
)) {
    Assert-Contains $MdLibContent $testSymbol "H107-mqk-md-test-$($testSymbol -replace '[^a-zA-Z0-9]','_')"
}
Assert-Contains $AlpacaContent '"1Hour"' 'H107-alpaca-test-1hour'
Write-Host 'PASS [H107]: Rust H1 unit tests present in mqk-md'

# ---------------------------------------------------------------------------
# Python: market_data_coverage.py 1h freshness support
# ---------------------------------------------------------------------------

Assert-Contains $CoverageContent 'max_staleness_minutes_1h' 'H108-coverage-config-field'
Assert-Contains $CoverageContent 'if timeframe == "1h":' 'H108-coverage-freshness-branch'
Assert-Contains $CoverageContent '--max-staleness-minutes-1h' 'H108-coverage-cli-arg'
Write-Host 'PASS [H108]: market_data_coverage.py has 1h freshness config field, branch, and CLI arg'

# gap_count must remain None for 1h (and 5m) - not fabricated.
Assert-Contains $CoverageContent 'gap_count: Optional[int] = None' 'H109-gap-count-default-none'
Write-Host 'PASS [H109]: gap_count defaults to None for non-1D timeframes (including 1h) - not fabricated'

# ---------------------------------------------------------------------------
# Python: tests present
# ---------------------------------------------------------------------------

foreach ($testSymbol in @(
    'test_1h_during_market_hours_beyond_threshold_is_stale',
    'test_1h_outside_market_hours_is_fresh_regardless_of_staleness',
    'test_1h_freshness_default_and_configurable_threshold',
    'test_1h_gap_count_is_not_computable'
)) {
    Assert-Contains $CoverageTestContent $testSymbol "H110-coverage-test-$($testSymbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [H110]: market_data_coverage 1h freshness/gap-count tests present'

Assert-Contains $ExportTestContent 'test_1h_timeframe_writes_symbol_1h_csv' 'H111-export-test-1h'
Assert-Contains $ExportTestContent 'AAPL_1h.csv' 'H111-export-1h-filename'
Write-Host 'PASS [H111]: market_data_export 1h CSV export test present'

# ---------------------------------------------------------------------------
# Safety: approved_for_live / live literals remain false in coverage module
# ---------------------------------------------------------------------------

foreach ($forbidden in @(
    'approved_for_live=True',
    'approved_for_live: bool = True',
    'approved_for_live = True',
    'recommended_for_live=True',
    'eligible_for_live=True'
)) {
    Assert-NotContains $CoverageContent $forbidden "H112-no-live-true-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [H112]: no live/recommended/eligible true literals in market_data_coverage module'

# ---------------------------------------------------------------------------
# Safety: no daemon/runtime/execution/OMS/broker/order references introduced
# ---------------------------------------------------------------------------

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
    'oms_inbox',
    '/api/v1/strategy/signal',
    'strategy/signal',
    '/v2/orders',
    'submit_order',
    'place_order',
    'enqueue_outbox'
)) {
    Assert-NotContains $CoverageContent $forbidden "H113-coverage-no-runtime-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [H113]: market_data_coverage module has no daemon/runtime/OMS/broker/order references'

foreach ($forbidden in @('import subprocess', 'os.system')) {
    Assert-NotContains $CoverageContent $forbidden "H114-coverage-no-subprocess-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [H114]: market_data_coverage module has no subprocess or shell execution'

# ---------------------------------------------------------------------------
# Safety: diff scope - no daemon/runtime/execution/OMS/broker/portfolio crates touched
# ---------------------------------------------------------------------------

$OutOfScopePaths = @(
    'core-rs/crates/mqk-daemon',
    'core-rs/crates/mqk-runtime',
    'core-rs/crates/mqk-execution',
    'core-rs/crates/mqk-portfolio',
    'core-rs/crates/mqk-broker-alpaca',
    'core-rs/crates/mqk-broker-paper',
    'core-rs/crates/mqk-audit'
)

$ScopeDiff = @()
foreach ($p in $OutOfScopePaths) {
    try {
        $ScopeDiff += @(git -C $RepoRoot diff --name-only -- $p)
        $ScopeDiff += @(git -C $RepoRoot diff --cached --name-only -- $p)
    } catch {
        $script:Failures.Add("FAIL [H115]: could not inspect diff for ${p}: $_")
    }
}
$ScopeDiff = @($ScopeDiff | Where-Object { $_ })
if ($ScopeDiff.Count -eq 0) {
    Write-Host 'PASS [H115]: no daemon/runtime/execution/portfolio/broker/audit files modified or staged'
} else {
    $script:Failures.Add("FAIL [H115]: out-of-scope files modified/staged: $($ScopeDiff -join ', ')")
}

# ---------------------------------------------------------------------------
# Safety: no .env.local read/printed, no secrets
# ---------------------------------------------------------------------------

foreach ($content in @($CoverageContent, $MdLibContent, $AlpacaContent, $CliMdContent)) {
    Assert-NotContains $content '.env.local' "H116-no-env-local"
}
Write-Host 'PASS [H116]: no .env.local references in patched files'

Write-Host ''
if ($Failures.Count -eq 0) {
    Write-Host 'ALL TIMEFRAME-H1-PROVIDER-INGEST-01 GUARDS PASSED' -ForegroundColor Green
    exit 0
} else {
    foreach ($failure in $Failures) {
        Write-Host $failure -ForegroundColor Red
    }
    Write-Host ''
    Write-Host "TIMEFRAME-H1-PROVIDER-INGEST-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
