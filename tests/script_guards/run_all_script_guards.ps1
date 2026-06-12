# =============================================================================
# Script guard aggregator
# CI-SCRIPT-GUARDS-WIRE-GHA-01
#
# Runs all script guards in tests/script_guards/ and fails if any guard fails.
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\run_all_script_guards.ps1
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed or missing.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition

$Guards = @(
    'test_start_paper_trading_smoke.ps1',
    'test_premarket_market_data_prep.ps1',
    'test_premarket_data_scheduler.ps1',
    'test_check_unsafe_patterns_ps1.ps1',
    'test_deprecated_scripts_retired.ps1',
    'test_db_proof_bootstrap_coverage.ps1',
    'test_capture_paper_smoke_evidence.ps1',
    'test_launcher_arm_paper.ps1',
    'test_exp_engine_guard.ps1',
    'test_exp_penny_guard.ps1',
    'test_intraday_market_data_refresh.ps1',
    'test_aapl5m_market_smoke_runner.ps1',
    'test_paper_smoke_evidence_review.ps1',
    'test_paper_smoke_ledger_and_shutdown.ps1',
    'test_scanner_candidate_artifacts.ps1',
    'test_scanner_data_quality.ps1',
    'test_scanner_liquidity.ps1',
    'test_scanner_regime.ps1',
    'test_scanner_scoring.ps1',
    'test_scanner_selector.ps1',
    'test_scanner_backtest_queue.ps1',
    'test_scanner_backtest_runner.ps1',
    'test_scanner_backtest_gates.ps1',
    'test_scanner_watchlist_promotion.ps1',
    'test_paper_smoke_evidence_trade_flow.ps1',
    'test_discord_secret_safety.ps1',
    'test_scanner_backtest_runner_real.ps1',
    'test_scanner_backtest_validation.ps1',
    'test_scanner_risk_premarket.ps1',
    'test_watchlist_readonly_handoff.ps1',
    'test_autonomous_paper_status.ps1',
    'test_paper_smoke_automation_status.ps1',
    'test_paper_operator_status.ps1',
    'test_scanner_walkforward_runner.ps1',
    'test_watchlist_admission_dryrun.ps1',
    'test_scanner_symbol_inputs.ps1',
    'test_scanner_symbol_inputs_runner.ps1',
    'test_scanner_market_data_export.ps1',
    'test_scanner_market_data_export_db_proof.ps1',
    'test_scanner_market_data_coverage.ps1',
    'test_scanner_watchlist_promotion_end_to_end.ps1',
    'test_scanner_paper_readiness_runner.ps1',
    'test_discord_test_alert_offline.ps1',
    'test_discord_paper_readiness_alerts.ps1',
    'test_discord_evidence_review_alerts.ps1',
    'test_launch_veritas_ledger.ps1',
    'test_paper_premarket_validation.ps1',
    'test_gui_relaunch_during_smoke.ps1'
)

$Results = [System.Collections.Generic.List[object]]::new()

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard aggregator (CI-SCRIPT-GUARDS-WIRE-GHA-01)'
Write-Host '============================================================'

foreach ($guard in $Guards) {
    $guardPath = Join-Path $ScriptDir $guard
    Write-Host ''
    Write-Host "--- $guard ---"

    if (-not (Test-Path $guardPath)) {
        Write-Host "  MISSING: $guardPath" -ForegroundColor Red
        $Results.Add([pscustomobject]@{ Guard = $guard; ExitCode = 127; Status = 'MISSING' })
        continue
    }

    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $guardPath
    $guardExit = $LASTEXITCODE
    if ($null -eq $guardExit) { $guardExit = 0 }

    if ($guardExit -eq 0) {
        Write-Host "  GUARD PASSED: $guard" -ForegroundColor Green
        $Results.Add([pscustomobject]@{ Guard = $guard; ExitCode = 0; Status = 'PASSED' })
    } else {
        Write-Host "  GUARD FAILED: $guard (exit=$guardExit)" -ForegroundColor Red
        $Results.Add([pscustomobject]@{ Guard = $guard; ExitCode = $guardExit; Status = 'FAILED' })
    }
}

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard aggregator summary'
Write-Host '============================================================'
$Results | Format-Table -AutoSize | Out-String -Width 120 | Write-Host

$Failures = @($Results | Where-Object { $_.Status -ne 'PASSED' })

if ($Failures.Count -eq 0) {
    Write-Host 'ALL SCRIPT GUARDS PASSED' -ForegroundColor Green
    exit 0
} else {
    Write-Host "FAILED: $($Failures.Count) guard(s) did not pass" -ForegroundColor Red
    exit 1
}
