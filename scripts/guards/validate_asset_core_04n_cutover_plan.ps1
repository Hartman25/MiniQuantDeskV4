# =============================================================================
# ASSET-CORE-04N -- Cutover Test Plan and Migration Plan Validator
# =============================================================================
# Pure docs/text validation of the ASSET-CORE-04N cutover test plan and
# migration plan. No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1]  Plan doc exists.
#   [2]  Plan mentions DB migration.
#   [3]  Plan mentions rollback.
#   [4]  Plan mentions shadow mode.
#   [5]  Plan mentions paper-only cutover.
#   [6]  Plan mentions live equity parity.
#   [7]  Plan mentions fractional quantity.
#   [8]  Plan mentions margin.
#   [9]  Plan mentions multiplier.
#   [10] Plan mentions currency conversion.
#   [11] Plan states no live routing.
#   [12] Plan states no broker adapter change in this design patch.
#   [13] No forbidden trading-enabled / cutover-complete claim.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_04n_cutover_plan.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathPlanSpec = Join-Path $RepoRoot "docs\specs\asset_core_04n_cutover_test_and_migration_plan.md"

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
Write-Host " ASSET-CORE-04N Cutover Test/Migration Plan Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Plan doc exists ---"
$PlanContent = $null
if (Test-FileExists "Test/migration plan spec" $PathPlanSpec) {
    $PlanContent = Get-Content -Raw -Path $PathPlanSpec
}

Write-Host ""
Show-Info "--- [2]-[10] Plan doc mentions required terms ---"
Test-ContentContains "plan mentions DB migration"          $PlanContent "DB migration"          | Out-Null
Test-ContentContains "plan mentions rollback"              $PlanContent "rollback"               | Out-Null
Test-ContentContains "plan mentions shadow mode"           $PlanContent "shadow mode"             | Out-Null
Test-ContentContains "plan mentions paper-only cutover"    $PlanContent "paper-only"              | Out-Null
Test-ContentContains "plan mentions live equity parity"    $PlanContent "equity accounting parity" | Out-Null
Test-ContentContains "plan mentions fractional quantity"   $PlanContent "fractional"              | Out-Null
Test-ContentContains "plan mentions margin"                $PlanContent "margin"                  | Out-Null
Test-ContentContains "plan mentions multiplier"            $PlanContent "multiplier"              | Out-Null
Test-ContentContains "plan mentions currency conversion"   $PlanContent "currency"                | Out-Null

Write-Host ""
Show-Info "--- [11] Plan states no live routing ---"
Test-ContentContains "plan states no live routing was enabled" `
    $PlanContent "No live routing was enabled" | Out-Null

Write-Host ""
Show-Info "--- [12] Plan states no broker adapter change in this design patch ---"
Test-ContentContains "plan states no broker adapter change was made" `
    $PlanContent "No broker adapter change was made" | Out-Null

Write-Host ""
Show-Info "--- [13] No forbidden trading-enabled / cutover-complete claim ---"
$ContentLower = $null
if ($null -ne $PlanContent) { $ContentLower = $PlanContent.ToLowerInvariant() }

$ForbiddenPhrases = @(
    "crypto trading is enabled",
    "futures trading is enabled",
    "options trading is enabled",
    "forex trading is enabled",
    "rates trading is enabled",
    "trading is now enabled",
    "approved for live trading",
    "production cutover is complete",
    "production cutover is authorized",
    "asset-core-04 is fully closed",
    "migration has been applied"
)

$ForbiddenViolations = 0
foreach ($Phrase in $ForbiddenPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $ForbiddenViolations++
        $Violations++
        Show-Red "  FAIL -- plan doc contains forbidden claim: '$Phrase'"
    }
}
if ($ForbiddenViolations -eq 0) {
    Show-Green "  OK -- no forbidden trading-enabled / cutover-complete claim found"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- ASSET-CORE-04N cutover test/migration plan is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
