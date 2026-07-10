# =============================================================================
# PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01A -- Current Truth Audit Validator
# =============================================================================
# Pure docs/text validation of the paper P&L operator-visibility audit.
# No network call, no provider/broker call, no DB connection, no daemon
# start, no cargo/npm build or test.
#
# Checks that docs/specs/paper_pnl_operator_visibility_01a_current_truth_audit.md
# exists and mentions the required patch identifiers, evidence values, mark
# source, and safety claims.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_pnl_operator_visibility_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAudit = Join-Path $RepoRoot "docs\specs\paper_pnl_operator_visibility_01a_current_truth_audit.md"

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
Write-Host " PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01A Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Paper PnL operator visibility 01A audit doc" $PathAudit) {
    $AuditContent = Get-Content -Raw -Path $PathAudit
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01 ---"
Test-ContentContains "audit doc mentions PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01" $AuditContent "PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Audit doc mentions AAPL ---"
Test-ContentContains "audit doc mentions AAPL" $AuditContent "AAPL" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions qty=3 ---"
Test-ContentContains "audit doc mentions qty=3" $AuditContent "qty=3" | Out-Null

Write-Host ""
Show-Info "--- [5] Audit doc mentions avg_price=314.81 ---"
Test-ContentContains "audit doc mentions avg_price=314.81" $AuditContent "avg_price=314.81" | Out-Null

Write-Host ""
Show-Info "--- [6] Audit doc mentions mark_price ---"
Test-ContentContains "audit doc mentions mark_price" $AuditContent "mark_price" | Out-Null

Write-Host ""
Show-Info "--- [7] Audit doc mentions unrealized_pnl ---"
Test-ContentContains "audit doc mentions unrealized_pnl" $AuditContent "unrealized_pnl" | Out-Null

Write-Host ""
Show-Info "--- [8] Audit doc mentions daily_pnl ---"
Test-ContentContains "audit doc mentions daily_pnl" $AuditContent "daily_pnl" | Out-Null

Write-Host ""
Show-Info "--- [9] Audit doc mentions md_bars ---"
Test-ContentContains "audit doc mentions md_bars" $AuditContent "md_bars" | Out-Null

Write-Host ""
Show-Info "--- [10] Audit doc states no live orders ---"
Test-ContentContains "audit doc states no live orders" $AuditContent "No live orders" | Out-Null

Write-Host ""
Show-Info "--- [11] Audit doc states no forced paper orders ---"
Test-ContentContains "audit doc states no forced paper orders" $AuditContent "No forced paper orders" | Out-Null

Write-Host ""
Show-Info "--- [12] Audit doc states no strategy threshold changes ---"
Test-ContentContains "audit doc states no strategy threshold changes" $AuditContent "No strategy threshold changes" | Out-Null

Write-Host ""
Show-Info "--- [13] Audit doc states no provider calls in tests ---"
Test-ContentContains "audit doc states no provider calls in tests" $AuditContent "No provider calls in tests" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01A audit is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
