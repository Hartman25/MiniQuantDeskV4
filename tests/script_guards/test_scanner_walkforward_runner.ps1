# =============================================================================
# Script guard: WALKFORWARD-RUNNER-01
#
# Verifies walkforward_runner.py exists and satisfies hard invariants:
# - dry_run is the default mode
# - recommended_for_live=True is absent
# - recommended_for_live=False is present (hard invariant)
# - No broker/OMS/order route references
# - No daemon/runtime imports
# - No DB imports
# - No network imports
# - subprocess is isolated behind real mode (not at module level)
# - Aggregation includes validation_profit_factor, validation_trades,
#   sample_quality, parameter_stability_score
# - No live_routing_enabled=true
#
# Exit: 0 = all assertions pass, 1 = one or more assertions failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$TargetFile = "research-py\src\mqk_research\scanner\walkforward_runner.py"
$Failures = 0

function Assert-True {
    param([bool]$Condition, [string]$Label)
    if ($Condition) {
        Write-Host "  PASS: $Label" -ForegroundColor Green
    } else {
        Write-Host "  FAIL: $Label" -ForegroundColor Red
        $script:Failures++
    }
}

function Assert-FileContains {
    param([string]$Path, [string]$Pattern, [string]$Label)
    $hit = Select-String -Path $Path -Pattern $Pattern -Quiet
    Assert-True -Condition ($hit -eq $true) -Label $Label
}

function Assert-FileNotContains {
    param([string]$Path, [string]$Pattern, [string]$Label)
    $hit = Select-String -Path $Path -Pattern $Pattern -Quiet
    Assert-True -Condition ($hit -ne $true) -Label $Label
}

Write-Host ""
Write-Host "=== test_scanner_walkforward_runner.ps1 ==="

# Guard 1: file exists
Assert-True -Condition (Test-Path $TargetFile) `
    -Label "WR-01: walkforward_runner.py exists"

if (-not (Test-Path $TargetFile)) {
    Write-Host "  Cannot continue: file missing." -ForegroundColor Red
    exit 1
}

# Guard 2: dry_run is the default mode
Assert-FileContains -Path $TargetFile -Pattern 'mode.*=.*"dry_run"' `
    -Label "WR-02: dry_run is the default mode"

# Guard 3: recommended_for_live=True is absent (hard invariant)
Assert-FileNotContains -Path $TargetFile -Pattern 'recommended_for_live\s*=\s*True' `
    -Label "WR-03: recommended_for_live=True is absent"

# Guard 4: recommended_for_live=False is present
Assert-FileContains -Path $TargetFile -Pattern 'recommended_for_live.*False' `
    -Label "WR-04: recommended_for_live=False is enforced"

# Guard 5: no broker/OMS/order route references
Assert-FileNotContains -Path $TargetFile -Pattern '/v2/orders|submit_order|BrokerGateway|oms_outbox|mqk_broker' `
    -Label "WR-05: no broker/OMS references"

# Guard 6: no daemon/runtime imports
Assert-FileNotContains -Path $TargetFile -Pattern 'import mqk_daemon|import mqk_runtime|from mqk_execution|import mqk_execution' `
    -Label "WR-06: no daemon/runtime imports"

# Guard 7: no DB imports
Assert-FileNotContains -Path $TargetFile -Pattern 'import psycopg|import sqlalchemy|import asyncpg|from mqk_db|import mqk_db' `
    -Label "WR-07: no DB imports"

# Guard 8: no network imports
Assert-FileNotContains -Path $TargetFile -Pattern 'import requests|import urllib|import aiohttp|import http\.client|import httpx' `
    -Label "WR-08: no network imports"

# Guard 9: subprocess is only inside a function body (not at module top-level)
# Verify "import subprocess" appears only after "def " in the file
$lines = Get-Content $TargetFile
$inFunctionBody = $false
$moduleSubprocess = $false
foreach ($line in $lines) {
    if ($line -match '^\s*def ') { $inFunctionBody = $true }
    if ($line -match '^\s*import subprocess' -and -not $inFunctionBody) {
        $moduleSubprocess = $true
    }
}
Assert-True -Condition (-not $moduleSubprocess) `
    -Label "WR-09: subprocess not imported at module level"

# Guard 10: aggregation includes validation_profit_factor
Assert-FileContains -Path $TargetFile -Pattern 'validation_profit_factor' `
    -Label "WR-10: validation_profit_factor in aggregation"

# Guard 11: aggregation includes validation_trades
Assert-FileContains -Path $TargetFile -Pattern 'validation_trades' `
    -Label "WR-11: validation_trades in aggregation"

# Guard 12: sample_quality present
Assert-FileContains -Path $TargetFile -Pattern 'sample_quality' `
    -Label "WR-12: sample_quality field present"

# Guard 13: parameter_stability_score present
Assert-FileContains -Path $TargetFile -Pattern 'parameter_stability_score' `
    -Label "WR-13: parameter_stability_score field present"

# Guard 14: no live_routing_enabled=true
Assert-FileNotContains -Path $TargetFile -Pattern 'live_routing_enabled\s*=\s*[Tt]rue' `
    -Label "WR-14: no live_routing_enabled=true"

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "ALL WALKFORWARD RUNNER GUARDS PASSED: $TargetFile" -ForegroundColor Green
    exit 0
} else {
    Write-Host "FAILED: $Failures walkforward runner guards did not pass" -ForegroundColor Red
    exit 1
}
