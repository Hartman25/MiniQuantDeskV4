# =============================================================================
# Script guard: SYMBOL-INPUTS-PRODUCER-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/symbol_inputs.py
#   research-py/tests/test_scanner_symbol_inputs.py (integration coverage)
#
# Guards:
#  G1.  symbol_inputs.py exists
#  G2.  schema_version string "symbol-inputs-v1" present
#  G3.  approved_for_live=True absent (no assignment/field-default sets it true)
#  G4.  approved_for_live=False present (build/write/load all force false)
#  G5.  required public API present (build/write/load/extract + spec dataclass)
#  G6.  stable producer reason constants present
#  G7.  no broker/OMS/order route references
#  G8.  no network imports
#  G9.  no DB imports
#  G10. no subprocess
#  G11. no daemon/runtime imports
#  G12. no order/strategy signal endpoint references
#  G13. eligible_for_live absent from module
#  G14. premarket integration is exercised by the symbol_inputs test suite
#       (imports both symbol_inputs and premarket_revalidation)
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$ProducerModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\symbol_inputs.py'
$IntegrationTest = Join-Path $RepoRoot 'research-py\tests\test_scanner_symbol_inputs.py'

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

# Guard 1: symbol_inputs.py exists
if (-not (Test-Path $ProducerModule)) {
    Write-Host "FAIL [G1]: symbol_inputs.py does not exist at: $ProducerModule" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: symbol_inputs.py exists"

# Guard: integration test suite exists (required for G14)
if (-not (Test-Path $IntegrationTest)) {
    Write-Host "FAIL [G14-precondition]: test_scanner_symbol_inputs.py does not exist at: $IntegrationTest" -ForegroundColor Red
    exit 1
}

$Content = Get-Content -Path $ProducerModule -Raw -Encoding UTF8
$TestContent = Get-Content -Path $IntegrationTest -Raw -Encoding UTF8

# Guard 2: schema_version string present
Assert-Contains $Content 'symbol-inputs-v1' "G2-schema-version-string"
Write-Host "PASS [G2]: schema_version string 'symbol-inputs-v1' present"

# Guard 3: no Python assignment sets approved_for_live to True
Assert-NotContains $Content 'approved_for_live"] = True' "G3-no-live-true-assignment"
Assert-NotContains $Content 'approved_for_live: bool = True' "G3-no-live-true-field"
Write-Host "PASS [G3]: no approved_for_live=True assignment in symbol_inputs module"

# Guard 4: approved_for_live forced False at build/write/load layers
Assert-Contains $Content '"approved_for_live": False' "G4-build-forces-false"
Assert-Contains $Content 'safe["approved_for_live"] = False' "G4-write-forces-false"
Assert-Contains $Content 'data["approved_for_live"] = False' "G4-load-forces-false"
Write-Host "PASS [G4]: approved_for_live=False forced at build/write/load layers"

# Guard 5: required public API present
foreach ($symbol in @(
    'def build_symbol_inputs',
    'def write_symbol_inputs_artifact',
    'def load_symbol_inputs_artifact',
    'def extract_symbol_inputs_map',
    'def build_symbol_input_record',
    'class SymbolInputSpec'
)) {
    Assert-Contains $Content $symbol "G5-api-$($symbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G5]: required public API present"

# Guard 6: stable producer reason constants present
foreach ($reason in @(
    'symbol_input_liquidity_metrics_missing',
    'symbol_input_missing_latest_price'
)) {
    Assert-Contains $Content $reason "G6-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G6]: stable producer reason constants present"

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
Write-Host "PASS [G7]: no broker/OMS/order references in symbol_inputs module"

# Guard 8: no network imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp')) {
    Assert-NotContains $Content $forbidden "G8-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G8]: no network imports in symbol_inputs module"

# Guard 9: no DB imports
foreach ($forbidden in @('import psycopg', 'import sqlalchemy')) {
    Assert-NotContains $Content $forbidden "G9-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G9]: no DB imports in symbol_inputs module"

# Guard 10: no subprocess
Assert-NotContains $Content "import subprocess" "G10-no-subprocess"
Assert-NotContains $Content "os.system" "G10-no-os-system"
Write-Host "PASS [G10]: no subprocess in symbol_inputs module"

# Guard 11: no daemon/runtime imports
foreach ($forbidden in @('from mqk_daemon', 'import mqk_daemon', 'from mqk_runtime', 'import mqk_runtime')) {
    Assert-NotContains $Content $forbidden "G11-no-daemon-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G11]: no daemon/runtime imports in symbol_inputs module"

# Guard 12: no order/strategy signal endpoint references
foreach ($forbidden in @(
    '/api/v1/strategy/signal',
    'strategy/signal',
    'enqueue_outbox',
    'place_order'
)) {
    Assert-NotContains $Content $forbidden "G12-no-signal-endpoint-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G12]: no order/strategy signal endpoint references in symbol_inputs module"

# Guard 13: eligible_for_live absent from module
Assert-NotContains $Content 'eligible_for_live' "G13-no-eligible-for-live"
Write-Host "PASS [G13]: eligible_for_live absent from symbol_inputs module"

# Guard 14: symbol_inputs test suite exercises premarket integration
Assert-Contains $TestContent 'from mqk_research.scanner.symbol_inputs import' "G14-test-imports-symbol-inputs"
Assert-Contains $TestContent 'from mqk_research.scanner.premarket_revalidation import' "G14-test-imports-premarket-revalidation"
Assert-Contains $TestContent 'evaluate_premarket_watchlist' "G14-test-calls-premarket-evaluation"
Write-Host "PASS [G14]: symbol_inputs test suite imports and exercises premarket revalidation"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL SYMBOL-INPUTS-PRODUCER-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "SYMBOL-INPUTS-PRODUCER-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
