# =============================================================================
# AUTON-NO-TRADE-OFFHOURS-01A -- Current Truth Audit Validator
# =============================================================================
# Pure docs/text validation of the off-hours autonomous no-trade audit and
# (once written, in later phases) its closure decision doc. No network call,
# no provider/broker call, no DB connection, no daemon start, no cargo/npm
# build or test.
#
# Checks:
#   [1] Audit doc exists.
#   [2] Audit doc mentions AUTON-NO-TRADE-01.
#   [3] Audit doc mentions AUTON-NO-SIGNAL-OBS-01.
#   [4] Audit doc mentions outside session window.
#   [5] Audit doc mentions no active run.
#   [6] Audit doc mentions no signal generated.
#   [7] Audit doc states no paper/live order will be submitted.
#   [8] Audit doc states the market-hours proof remains separate.
#   [9] If present, closure decision doc (Phase E) carries an honest status.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_auton_no_trade_offhours_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec   = Join-Path $RepoRoot "docs\specs\auton_no_trade_offhours_01a_current_truth_audit.md"
$PathClosureSpec = Join-Path $RepoRoot "docs\specs\auton_no_trade_offhours_01e_closure_decision.md"

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
Write-Host " AUTON-NO-TRADE-OFFHOURS-01A Current Truth Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Current truth audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions AUTON-NO-TRADE-01 ---"
Test-ContentContains "audit doc mentions AUTON-NO-TRADE-01" $AuditContent "AUTON-NO-TRADE-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Audit doc mentions AUTON-NO-SIGNAL-OBS-01 ---"
Test-ContentContains "audit doc mentions AUTON-NO-SIGNAL-OBS-01" $AuditContent "AUTON-NO-SIGNAL-OBS-01" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions outside session window ---"
Test-ContentContains "audit doc mentions outside session window" $AuditContent "outside session window" | Out-Null

Write-Host ""
Show-Info "--- [5] Audit doc mentions no active run ---"
Test-ContentContains "audit doc mentions no active run" $AuditContent "no active run" | Out-Null

Write-Host ""
Show-Info "--- [6] Audit doc mentions no signal generated ---"
Test-ContentContains "audit doc mentions no signal generated" $AuditContent "no signal" | Out-Null

Write-Host ""
Show-Info "--- [7] Audit doc states no paper/live order will be submitted ---"
Test-ContentContains "audit doc states no paper order will be submitted" $AuditContent "No paper order will be submitted" | Out-Null
Test-ContentContains "audit doc states no live order will be submitted" $AuditContent "No live order will be submitted" | Out-Null

Write-Host ""
Show-Info "--- [8] Audit doc states market-hours proof remains separate ---"
Test-ContentContains "audit doc separates market-hours proof" $AuditContent "market-hours proof" | Out-Null

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
    Show-Info "  SKIP -- closure decision doc not yet written (expected before Phase E)"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTON-NO-TRADE-OFFHOURS-01A audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
