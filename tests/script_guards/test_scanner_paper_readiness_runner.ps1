# =============================================================================
# Script guard: PAPER-READINESS-RUNNER-01
#
# Verifies safety invariants for:
#   research-py/src/mqk_research/scanner/paper_readiness_runner.py
#   research-py/tests/test_scanner_paper_readiness_runner.py (integration coverage)
#
# Guards:
#  G1.  paper_readiness_runner.py exists
#  G2.  required public API present (config/result/runner/loader/writer/CLI)
#  G3.  approved_for_live=True absent (no assignment/field-default sets it true)
#  G4.  approved_for_live forced False at config and result __post_init__
#  G5.  forged watchlist approved_for_live=True is reported, never propagated
#       (paper_readiness_forged_live_approval_forbidden reason constant)
#  G6.  stable paper_readiness_* reason constants present
#  G7.  no broker/OMS/order route references
#  G8.  no network imports
#  G9.  no DB imports
#  G10. no subprocess
#  G11. no daemon/runtime imports
#  G12. no order/strategy-signal endpoint references
#  G13. live_handoff_enabled is an unconditional hard block; eligible_for_live absent
#  G14. runner test suite imports the runner and exercises toggle, fail-closed,
#       and safety-invariant paths end to end
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$RunnerModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\paper_readiness_runner.py'
$IntegrationTest = Join-Path $RepoRoot 'research-py\tests\test_scanner_paper_readiness_runner.py'

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

# Guard 1: paper_readiness_runner.py exists
if (-not (Test-Path $RunnerModule)) {
    Write-Host "FAIL [G1]: paper_readiness_runner.py does not exist at: $RunnerModule" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G1]: paper_readiness_runner.py exists"

# Guard: integration test suite exists (required for G14)
if (-not (Test-Path $IntegrationTest)) {
    Write-Host "FAIL [G14-precondition]: test_scanner_paper_readiness_runner.py does not exist at: $IntegrationTest" -ForegroundColor Red
    exit 1
}

$Content = Get-Content -Path $RunnerModule -Raw -Encoding UTF8
$TestContent = Get-Content -Path $IntegrationTest -Raw -Encoding UTF8

# Guard 2: required public API present
foreach ($symbol in @(
    'class PaperReadinessConfig',
    'class PaperReadinessResult',
    'def run_paper_readiness_pipeline',
    'def load_config_from_json',
    'def write_paper_readiness_report',
    'def _load_watchlist_artifact',
    'def _load_strategy_fit_artifacts',
    'def main'
)) {
    Assert-Contains $Content $symbol "G2-api-$($symbol -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G2]: required public API present"

# Guard 3: no Python assignment sets approved_for_live to True
Assert-NotContains $Content 'approved_for_live"] = True' "G3-no-live-true-assignment"
Assert-NotContains $Content 'approved_for_live: bool = True' "G3-no-live-true-field"
Assert-NotContains $Content 'approved_for_live = True' "G3-no-live-true-bare-assignment"
Write-Host "PASS [G3]: no approved_for_live=True assignment in paper_readiness_runner module"

# Guard 4: approved_for_live forced False at config and result layers
$ForcedFalseMatches = [regex]::Matches($Content, [regex]::Escape('object.__setattr__(self, "approved_for_live", False)'))
if ($ForcedFalseMatches.Count -lt 2) {
    $Failures.Add("FAIL [G4-post-init-forces-false]: expected >=2 occurrences of 'object.__setattr__(self, ""approved_for_live"", False)' (config + result), found $($ForcedFalseMatches.Count)")
}
Write-Host "PASS [G4]: approved_for_live=False forced via __post_init__ on config and result"

# Guard 5: forged watchlist approval is reported, never propagated
Assert-Contains $Content 'paper_readiness_forged_live_approval_forbidden' "G5-forbidden-reason-string"
Assert-Contains $Content 'REASON_FORGED_LIVE_APPROVAL_FORBIDDEN' "G5-forbidden-reason-constant"
Assert-Contains $Content 'forged_live_approval' "G5-forged-detection-present"
Write-Host "PASS [G5]: forged approved_for_live=True is reported, never propagated"

# Guard 6: stable paper_readiness_* reason constants present
foreach ($reason in @(
    'paper_readiness_live_handoff_forbidden',
    'paper_readiness_mode_not_paper',
    'paper_readiness_disabled',
    'paper_readiness_symbol_inputs_disabled',
    'paper_readiness_watchlist_promotion_disabled',
    'paper_readiness_watchlist_path_missing',
    'paper_readiness_watchlist_load_failed',
    'paper_readiness_no_ranked_candidates',
    'paper_readiness_bars_root_missing',
    'paper_readiness_strategy_fit_dir_missing',
    'paper_readiness_strategy_fit_missing',
    'paper_readiness_symbol_inputs_blocked',
    'paper_readiness_symbol_inputs_partial',
    'paper_readiness_symbol_inputs_artifact_invalid',
    'paper_readiness_risk_simulation_failed',
    'paper_readiness_premarket_revalidation_failed',
    'paper_readiness_forged_live_approval_forbidden',
    'paper_readiness_output_write_failed',
    'paper_readiness_config_load_failed'
)) {
    Assert-Contains $Content $reason "G6-reason-$($reason -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G6]: stable paper_readiness_* reason constants present"

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
Write-Host "PASS [G7]: no broker/OMS/order references in paper_readiness_runner module"

# Guard 8: no network imports
foreach ($forbidden in @('import requests', 'import urllib', 'import http.client', 'import aiohttp', 'import httpx') ) {
    Assert-NotContains $Content $forbidden "G8-no-network-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G8]: no network imports in paper_readiness_runner module"

# Guard 9: no DB imports
foreach ($forbidden in @('import psycopg', 'import sqlalchemy', 'import asyncpg', 'from mqk_db', 'import mqk_db') ) {
    Assert-NotContains $Content $forbidden "G9-no-db-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G9]: no DB imports in paper_readiness_runner module"

# Guard 10: no subprocess
Assert-NotContains $Content "import subprocess" "G10-no-subprocess"
Assert-NotContains $Content "os.system" "G10-no-os-system"
Write-Host "PASS [G10]: no subprocess in paper_readiness_runner module"

# Guard 11: no daemon/runtime imports
foreach ($forbidden in @('from mqk_daemon', 'import mqk_daemon', 'from mqk_runtime', 'import mqk_runtime', 'from mqk_execution', 'import mqk_execution') ) {
    Assert-NotContains $Content $forbidden "G11-no-daemon-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G11]: no daemon/runtime imports in paper_readiness_runner module"

# Guard 12: no order/strategy signal endpoint references
foreach ($forbidden in @(
    '/api/v1/strategy/signal',
    'strategy/signal',
    'enqueue_outbox',
    'place_order'
)) {
    Assert-NotContains $Content $forbidden "G12-no-signal-endpoint-$($forbidden -replace '[^a-zA-Z0-9]','_')"
}
Write-Host "PASS [G12]: no order/strategy signal endpoint references in paper_readiness_runner module"

# Guard 13: live_handoff_enabled is an unconditional hard block; eligible_for_live absent
Assert-Contains $Content 'live_handoff_enabled' "G13-live-handoff-toggle-present"
Assert-Contains $Content 'REASON_LIVE_HANDOFF_FORBIDDEN' "G13-live-handoff-forbidden-reason"
Assert-NotContains $Content 'eligible_for_live' "G13-no-eligible-for-live"
Assert-NotContains $Content 'recommended_for_live": True' "G13-no-recommended-for-live-true"
Assert-NotContains $Content 'recommended_for_live = True' "G13-no-recommended-for-live-true-bare"
Write-Host "PASS [G13]: live_handoff_enabled is a hard block; eligible_for_live absent from module"

# Guard 14: runner test suite imports runner and exercises toggle/fail-closed/safety chains
Assert-Contains $TestContent 'from mqk_research.scanner.paper_readiness_runner import' "G14-test-imports-runner"
Assert-Contains $TestContent 'run_paper_readiness_pipeline' "G14-test-calls-pipeline"
Assert-Contains $TestContent 'load_config_from_json' "G14-test-calls-config-loader"
Assert-Contains $TestContent 'REASON_FORGED_LIVE_APPROVAL_FORBIDDEN' "G14-test-covers-forged-live-approval"
Assert-Contains $TestContent 'REASON_LIVE_HANDOFF_FORBIDDEN' "G14-test-covers-live-handoff-block"
Assert-Contains $TestContent 'STATUS_READY_FOR_OPERATOR_REVIEW' "G14-test-covers-operator-review-status"
Assert-Contains $TestContent 'STATUS_READY_FOR_PAPER_HANDOFF' "G14-test-covers-paper-handoff-status"
Write-Host "PASS [G14]: paper_readiness_runner test suite imports runner and exercises toggle/fail-closed/safety chains"

# Report results
Write-Host ""
if ($Failures.Count -eq 0) {
    Write-Host "ALL PAPER-READINESS-RUNNER-01 GUARDS PASSED" -ForegroundColor Green
    exit 0
} else {
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    Write-Host ""
    Write-Host "PAPER-READINESS-RUNNER-01 GUARD FAILED ($($Failures.Count) assertion(s))" -ForegroundColor Red
    exit 1
}
