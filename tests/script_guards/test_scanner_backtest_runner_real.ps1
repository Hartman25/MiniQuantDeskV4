# =============================================================================
# Script guard: BACKTEST-BRIDGE-BUNDLE-01
#
# Verifies that research-py/src/mqk_research/scanner/backtest_bridge.py:
#  1. exists
#  2. contains strategy-fit-v1 / metrics-schema-version reference
#  3. preserves recommended_for_live=False (never True)
#  4. has no broker/OMS/execution/network/DB imports or references
#  5. subprocess import is NOT at module level
#  6. parser handles metrics.json (parse_mqk_metrics_json present)
#  7. backtest failure reasons are stable string constants
#  8. live routing is never enabled
#  9. validation_metrics_missing reason always appended (no false out-of-sample pass)
#
# Also verifies backtest_runner.py has not grown forbidden patterns from bridging:
#  - subprocess still not at module level
#  - recommended_for_live=True still absent
#  - no broker/OMS/execution imports added
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$BridgeTarget = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_bridge.py'
$RunnerTarget = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_runner.py'
$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label, [string]$File)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in $File")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label, [string]$File)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in $File")
    }
}

# ---------------------------------------------------------------------------
# Guard 1: backtest_bridge.py exists
# ---------------------------------------------------------------------------
if (-not (Test-Path $BridgeTarget)) {
    Write-Host "FAIL [G1]: backtest_bridge.py does not exist at: $BridgeTarget" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: backtest_bridge.py exists"

$Bridge = Get-Content -Path $BridgeTarget -Raw -Encoding UTF8

# Guard 2: schema version reference (METRICS_SCHEMA_VERSION = 1)
Assert-Contains $Bridge "METRICS_SCHEMA_VERSION" "G2-schema-version" "backtest_bridge.py"

# Guard 3: recommended_for_live=False enforced (dataclass field default)
Assert-Contains $Bridge "recommended_for_live: bool = False" "G3-live-lock-false" "backtest_bridge.py"

# Guard 4: recommended_for_live=True must never appear
Assert-NotContains $Bridge "recommended_for_live=True" "G4-no-live-true" "backtest_bridge.py"

# Guard 5: no broker/OMS/execution references
foreach ($forbidden in @('/v2/orders', 'submit_order', 'BrokerGateway', 'oms_outbox', 'oms_inbox', 'live_routing_enabled=true', 'broker_adapter')) {
    Assert-NotContains $Bridge $forbidden "G5-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')" "backtest_bridge.py"
}

# Guard 6: no network/DB imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import psycopg', 'import sqlalchemy')) {
    Assert-NotContains $Bridge $forbidden "G6-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')" "backtest_bridge.py"
}

# Guard 7: subprocess NOT at module level (no line starting with 'import subprocess')
if ($Bridge -match '(?m)^import subprocess') {
    $script:Failures.Add("FAIL [G7]: 'import subprocess' found at module level in backtest_bridge.py - must be inside a function")
} else {
    Write-Host "PASS [G7]: subprocess not at module level in backtest_bridge.py"
}

# Guard 8: subprocess IS present (inside a function - deferred import)
Assert-Contains $Bridge "import subprocess" "G8-subprocess-deferred-present" "backtest_bridge.py"

# Guard 9: parse_mqk_metrics_json function present
Assert-Contains $Bridge "parse_mqk_metrics_json" "G9-parser-function" "backtest_bridge.py"

# Guard 10: metrics_file_missing failure reason constant present
Assert-Contains $Bridge "metrics_file_missing" "G10-metrics-file-missing-reason" "backtest_bridge.py"

# Guard 11: metrics_schema_invalid failure reason constant present
Assert-Contains $Bridge "metrics_schema_invalid" "G11-metrics-schema-invalid-reason" "backtest_bridge.py"

# Guard 12: backtest_command_failed failure reason constant present
Assert-Contains $Bridge "backtest_command_failed" "G12-command-failed-reason" "backtest_bridge.py"

# Guard 13: expectancy_basis_missing failure reason constant present
Assert-Contains $Bridge "expectancy_basis_missing" "G13-expectancy-basis-reason" "backtest_bridge.py"

# Guard 14: validation_metrics_missing reason always appended (present in source)
Assert-Contains $Bridge "validation_metrics_missing" "G14-validation-missing-reason" "backtest_bridge.py"

# Guard 15: build_mqk_backtest_command function present
Assert-Contains $Bridge "build_mqk_backtest_command" "G15-command-builder" "backtest_bridge.py"

# Guard 16: map_metrics_to_strategy_fit function present
Assert-Contains $Bridge "map_metrics_to_strategy_fit" "G16-metric-mapper" "backtest_bridge.py"

# Guard 17: run_backtest_for_entry function present
Assert-Contains $Bridge "run_backtest_for_entry" "G17-entry-runner" "backtest_bridge.py"

# Guard 18: no cargo run
Assert-NotContains $Bridge "cargo run" "G18-no-cargo-run" "backtest_bridge.py"

# Guard 19: no os.system
Assert-NotContains $Bridge "os.system" "G19-no-os-system" "backtest_bridge.py"

Write-Host ""
Write-Host "--- backtest_runner.py regression checks ---"

# ---------------------------------------------------------------------------
# Regression checks on backtest_runner.py (must not gain forbidden patterns)
# ---------------------------------------------------------------------------
if (-not (Test-Path $RunnerTarget)) {
    $script:Failures.Add("FAIL [R1]: backtest_runner.py not found at: $RunnerTarget")
} else {
    $Runner = Get-Content -Path $RunnerTarget -Raw -Encoding UTF8

    # R1: subprocess still not at module level
    if ($Runner -match '(?m)^import subprocess') {
        $script:Failures.Add("FAIL [R1]: 'import subprocess' now at module level in backtest_runner.py")
    } else {
        Write-Host "PASS [R1]: subprocess not at module level in backtest_runner.py"
    }

    # R2: no recommended_for_live=True
    Assert-NotContains $Runner "recommended_for_live=True" "R2-no-live-true" "backtest_runner.py"

    # R3: no broker references added
    foreach ($forbidden in @('/v2/orders', 'submit_order', 'BrokerGateway', 'oms_outbox', 'oms_inbox')) {
        Assert-NotContains $Runner $forbidden "R3-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')" "backtest_runner.py"
    }

    # R4: still has schema version strategy-fit-v1
    Assert-Contains $Runner "strategy-fit-v1" "R4-schema-version" "backtest_runner.py"

    # R5: still has blocked_no_backtest_interface constant
    Assert-Contains $Runner "blocked_no_backtest_interface" "R5-blocked-status" "backtest_runner.py"

    # R6: bridge_config parameter present (new real-mode seam)
    Assert-Contains $Runner "bridge_config" "R6-bridge-config-param" "backtest_runner.py"
}

# ---------------------------------------------------------------------------
# Report results
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL BACKTEST-BRIDGE-BUNDLE-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "BACKTEST-BRIDGE-BUNDLE-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
