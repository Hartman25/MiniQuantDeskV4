# =============================================================================
# Script guard: SYMBOL-INPUTS-RUNNER-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/symbol_inputs_runner.py
#   research-py/tests/test_scanner_symbol_inputs_runner.py (integration coverage)
#
# Guards:
#  G1.  symbol_inputs_runner.py exists
#  G2.  required public API present (config/result/runner/CLI + pure helpers)
#  G3.  approved_for_live=True absent (no assignment/field-default sets it true)
#  G4.  approved_for_live forced False at config and result __post_init__
#  G5.  forged watchlist approved_for_live=True does not propagate
#       (reported via watchlist_live_approval_forbidden reason constant)
#  G6.  stable runner reason constants present
#  G7.  no broker/OMS/order route references
#  G8.  no network imports
#  G9.  no DB imports
#  G10. no subprocess
#  G11. no daemon/runtime imports
#  G12. no order/strategy signal endpoint references
#  G13. eligible_for_live absent from module
#  G14. runner test suite imports both the runner and the producer, and
#       exercises the premarket-revalidation consumer chain end-to-end
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$RunnerModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\symbol_inputs_runner.py'
$IntegrationTest = Join-Path $RepoRoot 'research-py\tests\test_scanner_symbol_inputs_runner.py'

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

# Guard 1: symbol_inputs_runner.py exists
if (-not (Test-Path $RunnerModule)) {
    Write-Host "FAIL [G1]: symbol_inputs_runner.py does not exist at: $RunnerModule" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: symbol_inputs_runner.py exists"

# Guard: integration test suite exists (required for G14)
if (-not (Test-Path $IntegrationTest)) {
    Write-Host "FAIL [G14-precondition]: test_scanner_symbol_inputs_runner.py does not exist at: $IntegrationTest" -ForegroundColor Red
    exit 1
}

$Content = Get-Content -Path $RunnerModule -Raw -Encoding UTF8
$TestContent = Get-Content -Path $IntegrationTest -Raw -Encoding UTF8

# Guard 2: required public API present
foreach ($symbol in @(
    'class SymbolInputsRunnerConfig',
    'class SymbolInputsRunnerResult',
    'def run_symbol_inputs_producer',
    'def load_symbols_from_watchlist',
    'def resolve_bars_path',
    'def load_bars_for_symbol',
    'def load_liquidity_metrics_for_symbol',
    'def main'
)) {
    Assert-Contains $Content $symbol "G2-api-$($symbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G2]: required public API present"

# Guard 3: no Python assignment sets approved_for_live to True
Assert-NotContains $Content 'approved_for_live"] = True' "G3-no-live-true-assignment"
Assert-NotContains $Content 'approved_for_live: bool = True' "G3-no-live-true-field"
Assert-NotContains $Content 'approved_for_live = True' "G3-no-live-true-bare-assignment"
Write-Host "PASS [G3]: no approved_for_live=True assignment in symbol_inputs_runner module"

# Guard 4: approved_for_live forced False at config and result layers
Assert-Contains $Content 'object.__setattr__(self, "approved_for_live", False)' "G4-post-init-forces-false"
Write-Host "PASS [G4]: approved_for_live=False forced via __post_init__ on config and result"

# Guard 5: forged watchlist approval does not propagate
Assert-Contains $Content 'watchlist_live_approval_forbidden' "G5-forbidden-flag-present"
Assert-Contains $Content 'REASON_WATCHLIST_LIVE_APPROVAL_FORBIDDEN' "G5-forbidden-reason-present"
Write-Host "PASS [G5]: forged approved_for_live=True is reported, never propagated"

# Guard 6: stable runner reason constants present
foreach ($reason in @(
    'symbol_inputs_runner_no_input_source',
    'symbol_inputs_runner_watchlist_load_failed',
    'symbol_inputs_runner_no_symbols_resolved',
    'symbol_inputs_runner_bars_root_missing',
    'symbol_inputs_runner_malformed_bars_file',
    'symbol_inputs_runner_liquidity_sidecar_malformed',
    'symbol_inputs_runner_output_write_failed',
    'symbol_inputs_runner_watchlist_live_approval_forbidden'
)) {
    Assert-Contains $Content $reason "G6-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G6]: stable runner reason constants present"

# Guard 7: no broker/OMS/order route references
foreach ($forbidden in @(
    '/v2/orders',
    'submit_order',
    'live_routing_enabled=true',
    'oms_outbox',
    'oms_inbox',
    'BrokerGateway',
    'broker_adapter'
)) {
    Assert-NotContains $Content $forbidden "G7-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G7]: no broker/OMS/order references in symbol_inputs_runner module"

# Guard 8: no network imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import httpx') ) {
    Assert-NotContains $Content $forbidden "G8-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G8]: no network imports in symbol_inputs_runner module"

# Guard 9: no DB imports
foreach ($forbidden in @('import psycopg', 'import sqlalchemy', 'import asyncpg', 'from mqk_db', 'import mqk_db') ) {
    Assert-NotContains $Content $forbidden "G9-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G9]: no DB imports in symbol_inputs_runner module"

# Guard 10: no subprocess
Assert-NotContains $Content "import subprocess" "G10-no-subprocess"
Assert-NotContains $Content "os.system" "G10-no-os-system"
Write-Host "PASS [G10]: no subprocess in symbol_inputs_runner module"

# Guard 11: no daemon/runtime imports
foreach ($forbidden in @('from mqk_daemon', 'import mqk_daemon', 'from mqk_runtime', 'import mqk_runtime', 'from mqk_execution', 'import mqk_execution') ) {
    Assert-NotContains $Content $forbidden "G11-no-daemon-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G11]: no daemon/runtime imports in symbol_inputs_runner module"

# Guard 12: no order/strategy signal endpoint references
foreach ($forbidden in @(
    '/api/v1/strategy/signal',
    'strategy/signal',
    'enqueue_outbox',
    'place_order'
)) {
    Assert-NotContains $Content $forbidden "G12-no-signal-endpoint-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G12]: no order/strategy signal endpoint references in symbol_inputs_runner module"

# Guard 13: eligible_for_live absent from module
Assert-NotContains $Content 'eligible_for_live' "G13-no-eligible-for-live"
Write-Host "PASS [G13]: eligible_for_live absent from symbol_inputs_runner module"

# Guard 14: runner test suite imports runner + producer and exercises premarket consumer chain
Assert-Contains $TestContent 'from mqk_research.scanner.symbol_inputs_runner import' "G14-test-imports-runner"
Assert-Contains $TestContent 'from mqk_research.scanner.symbol_inputs import' "G14-test-imports-producer"
Assert-Contains $TestContent 'evaluate_premarket_watchlist' "G14-test-calls-premarket-evaluation"
Write-Host "PASS [G14]: symbol_inputs_runner test suite imports runner+producer and exercises premarket revalidation"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL SYMBOL-INPUTS-RUNNER-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "SYMBOL-INPUTS-RUNNER-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
