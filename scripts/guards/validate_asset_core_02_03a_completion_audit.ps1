# =============================================================================
# ASSET-CORE-02-03A -- Order Intent V2 / Asset Risk Router Completion Audit Validator
# =============================================================================
# Pure docs/text validation of the ASSET-CORE-02 / ASSET-CORE-03 current
# completion audit. No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1] Completion audit spec exists.
#   [2] Audit doc mentions ASSET-CORE-02.
#   [3] Audit doc mentions ASSET-CORE-03.
#   [4] Audit doc mentions OrderIntentV2.
#   [5] Audit doc mentions ExecutionIntentV2.
#   [6] Audit doc mentions Gate 0.
#   [7] Audit doc mentions the routing guard.
#   [8] Audit doc states no trading is enabled.
#   [9] Audit doc states no live/paper order is submitted.
#   [10] Audit doc states production consumption remains separate.
#   [11] Audit doc does not contain a forbidden trading-enabled claim.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_02_03a_completion_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec = Join-Path $RepoRoot "docs\specs\asset_core_02_03a_current_completion_audit.md"

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
Write-Host " ASSET-CORE-02-03A Completion Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Completion audit spec exists ---"
$AuditContent = $null
if (Test-FileExists "Completion audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2]-[7] Audit doc mentions required terms ---"
Test-ContentContains "audit doc mentions ASSET-CORE-02" $AuditContent "ASSET-CORE-02" | Out-Null
Test-ContentContains "audit doc mentions ASSET-CORE-03" $AuditContent "ASSET-CORE-03" | Out-Null
Test-ContentContains "audit doc mentions OrderIntentV2" $AuditContent "OrderIntentV2" | Out-Null
Test-ContentContains "audit doc mentions ExecutionIntentV2" $AuditContent "ExecutionIntentV2" | Out-Null
Test-ContentContains "audit doc mentions Gate 0" $AuditContent "Gate 0" | Out-Null
Test-ContentContains "audit doc mentions the routing guard" $AuditContent "routing guard" | Out-Null

Write-Host ""
Show-Info "--- [8] Audit doc states no trading is enabled ---"
Test-ContentContains "audit doc states no trading is enabled" `
    $AuditContent "No trading is enabled" | Out-Null

Write-Host ""
Show-Info "--- [9] Audit doc states no live/paper order is submitted ---"
Test-ContentContains "audit doc states no broker/provider/network call was made" `
    $AuditContent "No broker, provider, or network call" | Out-Null

Write-Host ""
Show-Info "--- [10] Audit doc states production consumption remains separate ---"
Test-ContentContains "audit doc states production consumption remains separate" `
    $AuditContent "Production consumption" | Out-Null

Write-Host ""
Show-Info "--- [11] No forbidden trading-enabled claim ---"
$ContentLower = $null
if ($null -ne $AuditContent) { $ContentLower = $AuditContent.ToLowerInvariant() }

$TradingEnabledPhrases = @(
    "crypto trading is enabled",
    "futures trading is enabled",
    "options trading is enabled",
    "forex trading is enabled",
    "rates trading is enabled",
    "trading is now enabled",
    "approved for live trading",
    "production cutover is complete",
    "registry v2 is now production"
)

$TradingClaimViolations = 0
foreach ($Phrase in $TradingEnabledPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $TradingClaimViolations++
        $Violations++
        Show-Red "  FAIL -- audit doc contains forbidden trading-enabled claim: '$Phrase'"
    }
}
if ($TradingClaimViolations -eq 0) {
    Show-Green "  OK -- no forbidden trading-enabled claim found"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- ASSET-CORE-02-03A completion audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
