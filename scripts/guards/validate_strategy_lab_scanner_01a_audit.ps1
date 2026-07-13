# =============================================================================
# STRATEGY-LAB-SCANNER-01A -- Current Truth Audit Validator
# =============================================================================
# Pure docs/text validation of the strategy lab scanner foundation audit
# doc. No network call, no provider/broker call, no DB connection, no
# daemon start, no cargo/npm build or test.
#
# Checks that
# docs/specs/strategy_lab_scanner_01a_current_truth_audit.md exists and
# mentions the required patch identifiers, data/registry conventions, and
# safety claims. Re-run after later phases to keep validating this
# phase's contract.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_strategy_lab_scanner_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAudit = Join-Path $RepoRoot "docs\specs\strategy_lab_scanner_01a_current_truth_audit.md"

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
Write-Host " STRATEGY-LAB-SCANNER-01A Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$Content = $null
if (Test-FileExists "Strategy lab scanner 01A audit doc" $PathAudit) {
    $Content = Get-Content -Raw -Path $PathAudit
}

Write-Host ""
Show-Info "--- [2] Doc mentions STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01 ---"
Test-ContentContains "doc mentions patch group id" $Content "STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Doc mentions equities.json ---"
Test-ContentContains "doc mentions equities.json" $Content "equities.json" | Out-Null

Write-Host ""
Show-Info "--- [4] Doc mentions md_backup ---"
Test-ContentContains "doc mentions md_backup" $Content "md_backup" | Out-Null

Write-Host ""
Show-Info "--- [5] Doc states local data only ---"
Test-ContentContains "doc states local data only" $Content "local data only" | Out-Null

Write-Host ""
Show-Info "--- [6] Doc states no provider calls ---"
Test-ContentContains "doc states no provider calls" $Content "no provider calls" | Out-Null

Write-Host ""
Show-Info "--- [7] Doc states no broker calls ---"
Test-ContentContains "doc states no broker calls" $Content "no broker calls" | Out-Null

Write-Host ""
Show-Info "--- [8] Doc states no live orders ---"
Test-ContentContains "doc states no live orders" $Content "no live orders" | Out-Null

Write-Host ""
Show-Info "--- [9] Doc states no forced paper orders ---"
Test-ContentContains "doc states no forced paper orders" $Content "no forced paper orders" | Out-Null

Write-Host ""
Show-Info "--- [10] Doc states no strategy threshold changes ---"
Test-ContentContains "doc states no strategy threshold changes" $Content "no strategy threshold changes" | Out-Null

Write-Host ""
Show-Info "--- [11] Doc mentions scanner artifact ---"
Test-ContentContains "doc mentions scanner artifact" $Content "scanner artifact" | Out-Null

Write-Host ""
Show-Info "--- [12] Doc mentions ranked candidates ---"
Test-ContentContains "doc mentions ranked candidates" $Content "ranked candidates" | Out-Null

Write-Host ""
Show-Info "--- [13] Doc mentions truth_state ---"
Test-ContentContains "doc mentions truth_state" $Content "truth_state" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- STRATEGY-LAB-SCANNER-01A audit is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
