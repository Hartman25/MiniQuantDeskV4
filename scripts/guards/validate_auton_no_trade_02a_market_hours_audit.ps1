# =============================================================================
# AUTON-NO-TRADE-02A -- Market-Hours Preflight Audit Validator
# =============================================================================
# Pure docs/text validation of the market-hours no-trade preflight audit and
# (once written, in later phases) its closure decision doc. No network call,
# no provider/broker call, no DB connection, no daemon start, no cargo/npm
# build or test.
#
# Checks:
#   [1] Audit doc exists.
#   [2] Audit doc mentions AUTON-NO-TRADE-02.
#   [3] Audit doc mentions market-hours proof.
#   [4] Audit doc mentions paper order attempt.
#   [5] Audit doc mentions durable explanation.
#   [6] Audit doc mentions strategy_signal_evaluations.
#   [7] Audit doc mentions autonomous_no_trade_diagnostics.
#   [8] Audit doc states no live order will be submitted.
#   [9] If present, closure decision doc (Phase D) carries an honest status.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_auton_no_trade_02a_market_hours_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec   = Join-Path $RepoRoot "docs\specs\auton_no_trade_02a_market_hours_preflight_audit.md"
$PathClosureSpec = Join-Path $RepoRoot "docs\specs\auton_no_trade_02d_market_hours_closure_decision.md"

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
Write-Host " AUTON-NO-TRADE-02A Market-Hours Preflight Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Market-hours preflight audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions AUTON-NO-TRADE-02 ---"
Test-ContentContains "audit doc mentions AUTON-NO-TRADE-02" $AuditContent "AUTON-NO-TRADE-02" | Out-Null

Write-Host ""
Show-Info "--- [3] Audit doc mentions market-hours proof ---"
Test-ContentContains "audit doc mentions market-hours proof" $AuditContent "market-hours proof" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions paper order attempt ---"
Test-ContentContains "audit doc mentions paper order attempt" $AuditContent "paper order attempt" | Out-Null

Write-Host ""
Show-Info "--- [5] Audit doc mentions durable explanation ---"
Test-ContentContains "audit doc mentions durable explanation" $AuditContent "durable" | Out-Null
Test-ContentContains "audit doc mentions explanation" $AuditContent "explanation" | Out-Null

Write-Host ""
Show-Info "--- [6] Audit doc mentions strategy_signal_evaluations ---"
Test-ContentContains "audit doc mentions strategy_signal_evaluations" $AuditContent "strategy_signal_evaluations" | Out-Null

Write-Host ""
Show-Info "--- [7] Audit doc mentions autonomous_no_trade_diagnostics ---"
Test-ContentContains "audit doc mentions autonomous_no_trade_diagnostics" $AuditContent "autonomous_no_trade_diagnostics" | Out-Null

Write-Host ""
Show-Info "--- [8] Audit doc states no live order will be submitted ---"
Test-ContentContains "audit doc states no live order will be submitted" $AuditContent "No live order will be submitted" | Out-Null

Write-Host ""
Show-Info "--- [9] Closure decision doc (if present) carries an honest status ---"
if (Test-Path $PathClosureSpec) {
    $ClosureContent = Get-Content -Raw -Path $PathClosureSpec
    $HonestLabels = @(
        "CLOSED_LOCAL",
        "PARTIAL",
        "OPEN"
    )
    $HasHonestLabel = $false
    foreach ($Label in $HonestLabels) {
        if ($ClosureContent.Contains($Label)) {
            $HasHonestLabel = $true
        }
    }
    if ($HasHonestLabel) {
        Show-Green "  OK -- closure decision doc carries a recognized honest status label"
    } else {
        $Violations++
        Show-Red "  FAIL -- closure decision doc exists but carries no recognized status label"
    }
} else {
    Show-Info "  SKIP -- closure decision doc not yet written (expected before Phase D)"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTON-NO-TRADE-02A audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
