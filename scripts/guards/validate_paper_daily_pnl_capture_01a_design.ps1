# =============================================================================
# PAPER-DAILY-PNL-CAPTURE-01A -- Current Truth & Action Design Validator
# =============================================================================
# Pure docs/text validation of the paper daily P&L baseline capture design
# doc. No network call, no provider/broker call, no DB connection, no
# daemon start, no cargo/npm build or test.
#
# Checks that
# docs/specs/paper_daily_pnl_capture_01a_current_truth_action_design.md
# exists and mentions the required patch identifiers, design decisions, and
# safety claims. Re-run after later phases to keep validating this phase's
# contract.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_daily_pnl_capture_01a_design.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathDesign = Join-Path $RepoRoot "docs\specs\paper_daily_pnl_capture_01a_current_truth_action_design.md"

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
Write-Host " PAPER-DAILY-PNL-CAPTURE-01A Design Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Design doc exists ---"
$Content = $null
if (Test-FileExists "Paper daily pnl capture 01A design doc" $PathDesign) {
    $Content = Get-Content -Raw -Path $PathDesign
}

Write-Host ""
Show-Info "--- [2] Doc mentions PAPER-DAILY-PNL-BASELINE-CAPTURE-01 ---"
Test-ContentContains "doc mentions PAPER-DAILY-PNL-BASELINE-CAPTURE-01" $Content "PAPER-DAILY-PNL-BASELINE-CAPTURE-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Doc mentions sys_account_equity_baseline ---"
Test-ContentContains "doc mentions sys_account_equity_baseline" $Content "sys_account_equity_baseline" | Out-Null

Write-Host ""
Show-Info "--- [4] Doc mentions upsert_account_equity_baseline ---"
Test-ContentContains "doc mentions upsert_account_equity_baseline" $Content "upsert_account_equity_baseline" | Out-Null

Write-Host ""
Show-Info "--- [5] Doc mentions broker_snapshot ---"
Test-ContentContains "doc mentions broker_snapshot" $Content "broker_snapshot" | Out-Null

Write-Host ""
Show-Info "--- [6] Doc mentions trading_date ---"
Test-ContentContains "doc mentions trading_date" $Content "trading_date" | Out-Null

Write-Host ""
Show-Info "--- [7] Doc mentions audit_event_id ---"
Test-ContentContains "doc mentions audit_event_id" $Content "audit_event_id" | Out-Null

Write-Host ""
Show-Info "--- [8] Doc states no fabricated baseline ---"
Test-ContentContains "doc states no fabricated baseline" $Content "No fabricated baseline" | Out-Null

Write-Host ""
Show-Info "--- [9] Doc states no live orders ---"
Test-ContentContains "doc states no live orders / no trading" $Content "No trading, no order submission" | Out-Null

Write-Host ""
Show-Info "--- [10] Doc states no forced paper orders ---"
Test-ContentContains "doc states no forced paper orders context (trading/order submission)" $Content "no order submission" | Out-Null

Write-Host ""
Show-Info "--- [11] Doc states no provider calls in tests ---"
Test-ContentContains "doc states no provider calls in tests" $Content "No provider, broker, or network call in any test" | Out-Null

Write-Host ""
Show-Info "--- [12] Doc states no historical backfill ---"
Test-ContentContains "doc states no historical backfill" $Content "No historical backfill" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-DAILY-PNL-CAPTURE-01A design is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
