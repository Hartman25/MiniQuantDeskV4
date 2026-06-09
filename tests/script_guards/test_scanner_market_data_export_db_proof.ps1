# =============================================================================
# Script guard: MARKET-DATA-EXPORT-DB-PROOF-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/market_data_export_db_proof.py
#   research-py/scripts/prove_market_data_export_db.py
#   research-py/tests/test_scanner_market_data_export_db_proof.py
#
# No live routing, no daemon calls, no DB mutations, no broker calls,
# no .env.local, no subprocess.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
$Module = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\market_data_export_db_proof.py'
$Script = Join-Path $RepoRoot 'research-py\scripts\prove_market_data_export_db.py'
$TestFile = Join-Path $RepoRoot 'research-py\tests\test_scanner_market_data_export_db_proof.py'

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
Write-Host '=== MARKET-DATA-EXPORT-DB-PROOF-01 static invariant tests ==='
Write-Host "    Module: $Module"
Write-Host "    Script: $Script"
Write-Host "    Tests : $TestFile"
Write-Host ''

foreach ($path in @($Module, $Script, $TestFile)) {
    if (-not (Test-Path $path)) {
        Write-Host "FAIL [MDEDBP01]: required file missing: $path" -ForegroundColor Red
        exit 1
    }
}
Write-Host 'PASS [MDEDBP01]: proof module, script, and tests exist'

$ModuleContent = Get-Content -Path $Module -Raw -Encoding UTF8
$ScriptContent = Get-Content -Path $Script -Raw -Encoding UTF8
$TestContent = Get-Content -Path $TestFile -Raw -Encoding UTF8
$ProofContent = $ModuleContent + "`n" + $ScriptContent

Assert-Contains $ProofContent 'market-data-export-db-proof-v1' 'MDEDBP02-schema'
Assert-Contains $ProofContent 'export_md_bars_to_bars_root' 'MDEDBP02-existing-exporter'
Assert-Contains $ProofContent 'normalize_bars_csv' 'MDEDBP02-existing-normalizer'
Assert-Contains $ProofContent 'class MarketDataExportDbProofConfig' 'MDEDBP02-config'
Assert-Contains $ProofContent 'class MarketDataExportDbProofResult' 'MDEDBP02-result'
Assert-Contains $ProofContent 'def run_market_data_export_db_proof' 'MDEDBP02-runner'
Assert-Contains $ProofContent 'def inspect_exported_csv' 'MDEDBP02-csv-inspection'
Write-Host 'PASS [MDEDBP02]: schema, API, exporter reuse, and normalizer reuse present'

$UpperProofContent = $ProofContent.ToUpperInvariant()
foreach ($forbiddenSql in @('INSERT ', 'UPDATE ', 'DELETE ', 'DROP ', 'ALTER ', 'TRUNCATE ', 'CREATE TABLE')) {
    Assert-NotContains $UpperProofContent $forbiddenSql "MDEDBP03-read-only-sql-$($forbiddenSql -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDEDBP03]: no DB write SQL verbs in proof module/script'

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
    Assert-NotContains $ProofContent $forbidden "MDEDBP04-no-daemon-oms-runtime-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDEDBP04]: no daemon/runtime/OMS/broker imports or references'

foreach ($forbidden in @(
    '/api/v1/strategy/signal',
    'strategy/signal',
    '/v2/orders',
    'submit_order',
    'place_order',
    'enqueue_outbox'
)) {
    Assert-NotContains $ProofContent $forbidden "MDEDBP05-no-order-signal-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDEDBP05]: no order or strategy-signal references'

foreach ($forbidden in @('import subprocess', 'os.system')) {
    Assert-NotContains $ProofContent $forbidden "MDEDBP06-no-subprocess-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDEDBP06]: no subprocess or shell execution'

Assert-NotContains $ProofContent '.env.local' 'MDEDBP07-no-env-local-read'
Assert-Contains $ProofContent 'MQK_PAPER_DB_URL' 'MDEDBP07-safe-env-fallback'
Write-Host 'PASS [MDEDBP07]: safe env fallback only; no .env.local read'

Assert-Contains $ProofContent 'REDACTED_DATABASE_URL' 'MDEDBP08-redaction-constant'
Assert-Contains $ProofContent 'def redact_database_url' 'MDEDBP08-redaction-helper'
Assert-Contains $ProofContent 'database_url=redact_database_url(config.database_url)' 'MDEDBP08-redacted-result-field'
Write-Host 'PASS [MDEDBP08]: DB URL redaction path present'

foreach ($forbidden in @(
    'approved_for_live=True',
    'approved_for_live: bool = True',
    'approved_for_live = True',
    'recommended_for_live=True',
    'eligible_for_live=True'
)) {
    Assert-NotContains $ProofContent $forbidden "MDEDBP09-no-live-true-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host 'PASS [MDEDBP09]: no live/recommended/eligible true literals in proof module/script'

Assert-Contains $TestContent 'MQK_RUN_DB_PROOF_TEST' 'MDEDBP10-real-db-env-gate'
Assert-Contains $TestContent 'MQK_PAPER_DB_URL' 'MDEDBP10-real-db-url-env'
Assert-Contains $TestContent 'skipUnless' 'MDEDBP10-real-db-skipped-by-default'
Write-Host 'PASS [MDEDBP10]: optional real DB test is env-gated/skipped by default'

Assert-Contains $TestContent 'test_no_writes_outside_caller_provided_bars_and_proof_paths' 'MDEDBP11-write-scope-test'
Assert-Contains $TestContent 'test_cli_redacts_database_url_in_printed_json' 'MDEDBP11-redaction-test'
Assert-Contains $TestContent 'test_paper_readiness_refuses_to_run_without_required_paths' 'MDEDBP11-paper-readiness-gate-test'
Write-Host 'PASS [MDEDBP11]: required unit coverage anchors present'

$ForbiddenDiff = @()
try {
    $ForbiddenDiff += @(git -C $RepoRoot diff --name-only -- 'core-rs/crates/mqk-daemon')
    $ForbiddenDiff += @(git -C $RepoRoot diff --cached --name-only -- 'core-rs/crates/mqk-daemon')
    $ForbiddenDiff += @(git -C $RepoRoot diff --name-only -- 'core-rs/crates/mqk-execution')
    $ForbiddenDiff += @(git -C $RepoRoot diff --cached --name-only -- 'core-rs/crates/mqk-execution')
} catch {
    $script:Failures.Add("FAIL [MDEDBP12]: could not inspect forbidden diffs: $_")
}
if ($ForbiddenDiff.Count -eq 0) {
    Write-Host 'PASS [MDEDBP12]: no daemon/execution files are modified or staged'
} else {
    $script:Failures.Add("FAIL [MDEDBP12]: forbidden files modified/staged: $($ForbiddenDiff -join ', ')")
}

Write-Host ''
if ($Failures.Count -eq 0) {
    Write-Host 'ALL MARKET-DATA-EXPORT-DB-PROOF-01 GUARDS PASSED' -ForegroundColor Green
    exit 0
} else {
    foreach ($failure in $Failures) {
        Write-Host $failure -ForegroundColor Red
    }
    Write-Host ''
    Write-Host "MARKET-DATA-EXPORT-DB-PROOF-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
