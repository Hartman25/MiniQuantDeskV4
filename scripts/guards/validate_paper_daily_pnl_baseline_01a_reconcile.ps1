# =============================================================================
# PAPER-DAILY-PNL-BASELINE-01A -- Current Truth Reconcile Validator
# =============================================================================
# Pure docs/text validation of the paper daily P&L baseline current-truth
# reconcile doc. No network call, no provider/broker call, no DB connection,
# no daemon start, no cargo/npm build or test.
#
# Checks that
# docs/specs/paper_daily_pnl_baseline_01a_current_truth_reconcile.md exists
# and mentions the required patch identifiers, design decisions, and safety
# claims. Re-run after later phases to keep validating this phase's contract.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_daily_pnl_baseline_01a_reconcile.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathReconcile = Join-Path $RepoRoot "docs\specs\paper_daily_pnl_baseline_01a_current_truth_reconcile.md"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

function Test-FileExists {
    param([string]$Label, [string]$Path)
    if (Test-Path $Path) {
        Show-Green "  OK -- $Label found: $Path"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label not found: $Path"
        return $false
    }
}

function Test-ContentContains {
    param([string]$Label, [string]$Content, [string]$Needle)
    if ($null -ne $Content -and $Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (needle not found: '$Needle')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " PAPER-DAILY-PNL-BASELINE-01A Current Truth Reconcile Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Reconcile doc exists ---"
$ReconcileContent = $null
if (Test-FileExists "Paper daily pnl baseline 01A reconcile doc" $PathReconcile) {
    $ReconcileContent = Get-Content -Raw -Path $PathReconcile
}

Write-Host ""
Show-Info "--- [2] Doc mentions PAPER-DAILY-PNL-BASELINE-01 ---"
Test-ContentContains "doc mentions PAPER-DAILY-PNL-BASELINE-01" $ReconcileContent "PAPER-DAILY-PNL-BASELINE-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Doc mentions previous-session-close ---"
Test-ContentContains "doc mentions previous-session-close" $ReconcileContent "previous-session-close" | Out-Null

Write-Host ""
Show-Info "--- [4] Doc mentions sys_account_equity_baseline ---"
Test-ContentContains "doc mentions sys_account_equity_baseline" $ReconcileContent "sys_account_equity_baseline" | Out-Null

Write-Host ""
Show-Info "--- [5] Doc mentions daily_pnl ---"
Test-ContentContains "doc mentions daily_pnl" $ReconcileContent "daily_pnl" | Out-Null

Write-Host ""
Show-Info "--- [6] Doc mentions daily_pnl_truth_state ---"
Test-ContentContains "doc mentions daily_pnl_truth_state" $ReconcileContent "daily_pnl_truth_state" | Out-Null

Write-Host ""
Show-Info "--- [7] Doc states no fabricated baseline ---"
Test-ContentContains "doc states no fabricated baseline" $ReconcileContent "no fabricated" | Out-Null

Write-Host ""
Show-Info "--- [8] Doc states no live orders ---"
Test-ContentContains "doc states no live orders" $ReconcileContent "no live" | Out-Null

Write-Host ""
Show-Info "--- [9] Doc states no forced paper orders ---"
Test-ContentContains "doc states no forced paper orders" $ReconcileContent "forced paper order" | Out-Null

Write-Host ""
Show-Info "--- [10] Doc states no provider calls in tests ---"
Test-ContentContains "doc states no provider calls in tests" $ReconcileContent "no provider, broker, or network calls" | Out-Null

Write-Host ""
Show-Info "--- [11] Doc states no historical backfill ---"
Test-ContentContains "doc states no historical backfill" $ReconcileContent "no historical" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-DAILY-PNL-BASELINE-01A reconcile is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
