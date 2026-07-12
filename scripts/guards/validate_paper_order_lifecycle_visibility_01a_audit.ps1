# =============================================================================
# PAPER-ORDER-LIFECYCLE-VIS-01A -- Current Truth Audit Validator
# =============================================================================
# Pure docs/text validation of the paper order lifecycle visibility audit
# doc. No network call, no provider/broker call, no DB connection, no
# daemon start, no cargo/npm build or test.
#
# Checks that
# docs/specs/paper_order_lifecycle_visibility_01a_current_truth_audit.md
# exists and mentions the required patch identifiers, durable tables, route
# contract, and safety claims. Re-run after later phases to keep validating
# this phase's contract.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_order_lifecycle_visibility_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAudit = Join-Path $RepoRoot "docs\specs\paper_order_lifecycle_visibility_01a_current_truth_audit.md"

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
Write-Host " PAPER-ORDER-LIFECYCLE-VIS-01A Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$Content = $null
if (Test-FileExists "Paper order lifecycle visibility 01A audit doc" $PathAudit) {
    $Content = Get-Content -Raw -Path $PathAudit
}

Write-Host ""
Show-Info "--- [2] Doc mentions PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY ---"
Test-ContentContains "doc mentions PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY" $Content "PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY" | Out-Null

Write-Host ""
Show-Info "--- [3] Doc mentions runs ---"
Test-ContentContains "doc mentions runs table" $Content "runs" | Out-Null

Write-Host ""
Show-Info "--- [4] Doc mentions strategy_signal_evaluations ---"
Test-ContentContains "doc mentions strategy_signal_evaluations" $Content "strategy_signal_evaluations" | Out-Null

Write-Host ""
Show-Info "--- [5] Doc mentions autonomous_no_trade_diagnostics ---"
Test-ContentContains "doc mentions autonomous_no_trade_diagnostics" $Content "autonomous_no_trade_diagnostics" | Out-Null

Write-Host ""
Show-Info "--- [6] Doc mentions oms_outbox ---"
Test-ContentContains "doc mentions oms_outbox" $Content "oms_outbox" | Out-Null

Write-Host ""
Show-Info "--- [7] Doc mentions oms_inbox ---"
Test-ContentContains "doc mentions oms_inbox" $Content "oms_inbox" | Out-Null

Write-Host ""
Show-Info "--- [8] Doc mentions run_id ---"
Test-ContentContains "doc mentions run_id" $Content "run_id" | Out-Null

Write-Host ""
Show-Info "--- [9] Doc mentions truth_state ---"
Test-ContentContains "doc mentions truth_state" $Content "truth_state" | Out-Null

Write-Host ""
Show-Info "--- [10] Doc states no live orders ---"
Test-ContentContains "doc states no order submission" $Content "No order submission" | Out-Null

Write-Host ""
Show-Info "--- [11] Doc states no forced paper orders / trading behavior change ---"
Test-ContentContains "doc states no trading behavior change" $Content "No trading behavior change" | Out-Null

Write-Host ""
Show-Info "--- [12] Doc states no provider calls in tests ---"
Test-ContentContains "doc states no provider/broker/network call" $Content "No order submission, no broker/provider/network call" | Out-Null

Write-Host ""
Show-Info "--- [13] Doc states no DB migration ---"
Test-ContentContains "doc states no DB migration" $Content "No DB migration" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-ORDER-LIFECYCLE-VIS-01A audit is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
