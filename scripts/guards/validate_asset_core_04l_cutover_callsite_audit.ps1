# =============================================================================
# ASSET-CORE-04L -- Production Cutover Callsite Audit Validator
# =============================================================================
# Pure docs/text validation of the ASSET-CORE-04L production cutover
# callsite audit. No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1]  Audit doc exists.
#   [2]  Audit doc mentions ASSET-CORE-04.
#   [3]  Audit doc mentions production cutover.
#   [4]  Audit doc mentions live accounting.
#   [5]  Audit doc mentions risk enforcement.
#   [6]  Audit doc mentions broker/order routing.
#   [7]  Audit doc mentions DB migration.
#   [8]  Audit doc mentions fractional quantity.
#   [9]  Audit doc mentions currency conversion.
#   [10] Audit doc mentions margin model.
#   [11] Audit doc states no live/paper order will be submitted.
#   [12] Audit doc states no non-equity trading is enabled.
#   [13] Audit doc states no code cutover is performed.
#   [14] No forbidden trading-enabled / cutover-complete claim.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_04l_cutover_callsite_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec = Join-Path $RepoRoot "docs\specs\asset_core_04l_production_cutover_callsite_audit.md"

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
Write-Host " ASSET-CORE-04L Production Cutover Callsite Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Callsite audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2]-[10] Audit doc mentions required terms ---"
Test-ContentContains "audit doc mentions ASSET-CORE-04"       $AuditContent "ASSET-CORE-04"       | Out-Null
Test-ContentContains "audit doc mentions production cutover"  $AuditContent "production cutover"  | Out-Null
Test-ContentContains "audit doc mentions live accounting"     $AuditContent "live accounting"     | Out-Null
Test-ContentContains "audit doc mentions risk enforcement"    $AuditContent "risk enforcement"    | Out-Null
Test-ContentContains "audit doc mentions broker/order routing" $AuditContent "order routing"      | Out-Null
Test-ContentContains "audit doc mentions DB migration"         $AuditContent "DB migration"        | Out-Null
Test-ContentContains "audit doc mentions fractional quantity"  $AuditContent "fractional"          | Out-Null
Test-ContentContains "audit doc mentions currency conversion"  $AuditContent "currency conversion" | Out-Null
Test-ContentContains "audit doc mentions margin model"         $AuditContent "margin"              | Out-Null

Write-Host ""
Show-Info "--- [11] Audit doc states no live/paper order will be submitted ---"
Test-ContentContains "audit doc states no live or paper order was submitted" `
    $AuditContent "No live or paper order was submitted" | Out-Null

Write-Host ""
Show-Info "--- [12] Audit doc states no non-equity trading is enabled ---"
Test-ContentContains "audit doc states no non-equity asset class was enabled" `
    $AuditContent "No non-equity asset class was enabled" | Out-Null

Write-Host ""
Show-Info "--- [13] Audit doc states no code cutover is performed ---"
Test-ContentContains "audit doc states no production source file behavior was modified" `
    $AuditContent "source file" | Out-Null
Test-ContentContains "audit doc states behavior was not modified" `
    $AuditContent "behavior was modified" | Out-Null

Write-Host ""
Show-Info "--- [14] No forbidden trading-enabled / cutover-complete claim ---"
$ContentLower = $null
if ($null -ne $AuditContent) { $ContentLower = $AuditContent.ToLowerInvariant() }

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
    "live accounting now consumes instrumenteconomics"
)

$ForbiddenViolations = 0
foreach ($Phrase in $ForbiddenPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $ForbiddenViolations++
        $Violations++
        Show-Red "  FAIL -- audit doc contains forbidden claim: '$Phrase'"
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
    Show-Green " ALL CHECKS PASSED -- ASSET-CORE-04L callsite audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
