# =============================================================================
# Script guard: BT-WALKFORWARD-VALIDATION-BUNDLE-01
#
# Verifies that metric normalization, validation field mapping, and walk-forward
# split planner satisfy safety invariants:
#
# backtest_bridge.py:
#  1. recommended_for_live=True absent.
#  2. recommended_for_live=False present.
#  3. Validation field strings present: validation_profit_factor, validation_trades,
#     largest_trade_profit_fraction, sample_quality, parameter_stability_score.
#  4. validation_metrics_missing is conditional (not unconditional).
#  5. No broker/OMS/order route references.
#  6. No network imports.
#  7. No DB imports.
#  8. Subprocess still isolated behind real mode (not at module level).
#  9. No full runtime/daemon import.
# 10. percent_0_100 scale constant present.
# 11. auto scale constant present (for legacy fixture testing only).
#
# backtest_runner.py:
# 12. validation_profit_factor field present in StrategyFitResult.
# 13. validation_trades field present in StrategyFitResult.
# 14. largest_trade_profit_fraction field present in StrategyFitResult.
# 15. recommended_for_live=True absent.
#
# walkforward.py:
# 16. Module exists.
# 17. WalkForwardConfig present.
# 18. WalkForwardSplit present.
# 19. build_date_splits present.
# 20. No subprocess.
# 21. No broker/OMS references.
# 22. No network imports.
# 23. WALKFORWARD-RUNNER-01 OPEN noted in source (not claimed closed).
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$BridgeTarget   = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_bridge.py'
$RunnerTarget   = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\backtest_runner.py'
$WalkfwdTarget  = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\walkforward.py'
$Failures       = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label, [string]$File)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in $File")
    } else {
        Write-Host "PASS [$Label]"
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label, [string]$File)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in $File")
    } else {
        Write-Host "PASS [$Label]"
    }
}

# ---------------------------------------------------------------------------
# backtest_bridge.py guards
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- backtest_bridge.py ---"

if (-not (Test-Path $BridgeTarget)) {
    Write-Host "FAIL [G1-bridge-exists]: backtest_bridge.py not found" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1-bridge-exists]"
$Bridge = Get-Content -Path $BridgeTarget -Raw -Encoding UTF8

# G2: recommended_for_live=True never present
Assert-NotContains $Bridge "recommended_for_live=True" "G2-no-live-true" "backtest_bridge.py"

# G3: recommended_for_live=False present
Assert-Contains $Bridge "recommended_for_live: bool = False" "G3-live-lock-false" "backtest_bridge.py"

# G4: validation field strings present in mapper
foreach ($field in @('validation_profit_factor', 'validation_trades',
                     'largest_trade_profit_fraction', 'sample_quality',
                     'parameter_stability_score')) {
    Assert-Contains $Bridge $field "G4-validation-field-$field" "backtest_bridge.py"
}

# G5: validation_metrics_missing is conditional (uses if/conditional; not unconditional append)
# The constant must exist but must NOT be in a plain "failure_reasons.append(REASON_VALIDATION_METRICS_MISSING)"
# without an if guard. We verify the constant is present and that a conditional block exists.
Assert-Contains $Bridge "REASON_VALIDATION_METRICS_MISSING" "G5-validation-reason-const" "backtest_bridge.py"
if ($Bridge -match '(?m)^\s+failure_reasons\.append\(REASON_VALIDATION_METRICS_MISSING\)') {
    # Must be preceded by an if statement on the same or previous line(s)
    # Simple check: verify that "if " appears in the surrounding context
    if ($Bridge -notmatch 'if .*(validation_profit_factor|validation_trades).*:\s*\r?\n\s+failure_reasons\.append\(REASON_VALIDATION_METRICS_MISSING\)') {
        # More flexible check: the append is inside an if block
        if ($Bridge -match 'if validation_missing') {
            Write-Host "PASS [G5-conditional-validation-missing]"
        } else {
            $script:Failures.Add("FAIL [G5-conditional-validation-missing]: validation_metrics_missing append must be conditional")
        }
    } else {
        Write-Host "PASS [G5-conditional-validation-missing]"
    }
} else {
    $script:Failures.Add("FAIL [G5-conditional-validation-missing]: REASON_VALIDATION_METRICS_MISSING append not found")
}

# G6: no broker/OMS references
foreach ($forbidden in @('/v2/orders', 'submit_order', 'BrokerGateway', 'oms_outbox', 'oms_inbox', 'broker_adapter')) {
    Assert-NotContains $Bridge $forbidden "G6-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')" "backtest_bridge.py"
}

# G7: no network imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import psycopg', 'import sqlalchemy')) {
    Assert-NotContains $Bridge $forbidden "G7-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')" "backtest_bridge.py"
}

# G8: subprocess NOT at module level
if ($Bridge -match '(?m)^import subprocess') {
    $script:Failures.Add("FAIL [G8-subprocess-not-module-level]: import subprocess at module level in backtest_bridge.py")
} else {
    Write-Host "PASS [G8-subprocess-not-module-level]"
}

# G9: no daemon/runtime imports
foreach ($forbidden in @('import mqk_daemon', 'import mqk_runtime', 'import mqk_execution')) {
    Assert-NotContains $Bridge $forbidden "G9-no-runtime-$($forbidden -replace '[^a-zA-Z0-9]','_')" "backtest_bridge.py"
}

# G10: percent_0_100 scale constant
Assert-Contains $Bridge "PCT_SCALE_PERCENT_0_100" "G10-pct-scale-const" "backtest_bridge.py"

# G11: auto scale constant
Assert-Contains $Bridge "PCT_SCALE_AUTO" "G11-auto-scale-const" "backtest_bridge.py"

# ---------------------------------------------------------------------------
# backtest_runner.py guards
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- backtest_runner.py ---"

if (-not (Test-Path $RunnerTarget)) {
    $script:Failures.Add("FAIL [R1-runner-exists]: backtest_runner.py not found")
} else {
    $Runner = Get-Content -Path $RunnerTarget -Raw -Encoding UTF8
    Write-Host "PASS [R1-runner-exists]"

    # R2: validation fields in StrategyFitResult
    foreach ($field in @('validation_profit_factor', 'validation_trades', 'largest_trade_profit_fraction')) {
        Assert-Contains $Runner $field "R2-strategyfit-field-$field" "backtest_runner.py"
    }

    # R3: recommended_for_live=True never present
    Assert-NotContains $Runner "recommended_for_live=True" "R3-no-live-true" "backtest_runner.py"
}

# ---------------------------------------------------------------------------
# walkforward.py guards
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- walkforward.py ---"

if (-not (Test-Path $WalkfwdTarget)) {
    $script:Failures.Add("FAIL [W1-walkfwd-exists]: walkforward.py not found at: $WalkfwdTarget")
} else {
    $Walkfwd = Get-Content -Path $WalkfwdTarget -Raw -Encoding UTF8
    Write-Host "PASS [W1-walkfwd-exists]"

    Assert-Contains $Walkfwd "WalkForwardConfig"   "W2-wf-config"  "walkforward.py"
    Assert-Contains $Walkfwd "WalkForwardSplit"    "W3-wf-split"   "walkforward.py"
    Assert-Contains $Walkfwd "build_date_splits"   "W4-wf-builder" "walkforward.py"

    # W5: no subprocess
    if ($Walkfwd -match '(?m)^import subprocess') {
        $script:Failures.Add("FAIL [W5-no-subprocess]: import subprocess at module level in walkforward.py")
    } else {
        Write-Host "PASS [W5-no-subprocess]"
    }

    # W6: no broker/OMS
    foreach ($forbidden in @('/v2/orders', 'submit_order', 'BrokerGateway', 'oms_outbox')) {
        Assert-NotContains $Walkfwd $forbidden "W6-no-broker-$($forbidden -replace '[^a-zA-Z0-9]','_')" "walkforward.py"
    }

    # W7: no network imports
    foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import psycopg', 'import sqlalchemy')) {
        Assert-NotContains $Walkfwd $forbidden "W7-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')" "walkforward.py"
    }

    # W8: WALKFORWARD-RUNNER-01 OPEN mentioned (not claiming closed runner exists)
    Assert-Contains $Walkfwd "WALKFORWARD-RUNNER-01" "W8-runner-open-noted" "walkforward.py"
}

# ---------------------------------------------------------------------------
# Report results
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL BT-WALKFORWARD-VALIDATION-BUNDLE-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "BT-WALKFORWARD-VALIDATION-BUNDLE-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
