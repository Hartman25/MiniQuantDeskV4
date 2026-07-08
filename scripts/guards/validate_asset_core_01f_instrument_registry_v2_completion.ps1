# =============================================================================
# ASSET-CORE-01F -- Instrument Registry V2 Completion Audit Validator
# =============================================================================
# Pure docs/text validation of the ASSET-CORE-01A..01E instrument registry v2
# foundation closure audit. No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1]  Completion audit spec exists.
#   [2]  Audit doc mentions every sub-slice 01A through 01E.
#   [3]  Audit doc mentions InstrumentRegistryV2.
#   [4]  Audit doc mentions all seven schema categories (equity, ETF, crypto,
#        future, option, forex, rate/fixed income).
#   [5]  Audit doc states registry-v2 is not production trading truth.
#   [6]  Audit doc does not claim crypto/futures/options/forex/rates trading
#        is enabled.
#   [7]  Audit doc does not claim a production cutover happened.
#   [8]  Audit doc points to a true next patch.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_01f_instrument_registry_v2_completion.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec = Join-Path $RepoRoot "docs\specs\asset_core_01f_instrument_registry_v2_completion_audit.md"

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
Write-Host " ASSET-CORE-01F Instrument Registry V2 Completion Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Completion audit spec exists ---"
$AuditContent = $null
if (Test-FileExists "Completion audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions every sub-slice 01A..01E ---"
$SubSlices = @(
    "ASSET-CORE-01A", "ASSET-CORE-01B", "ASSET-CORE-01C",
    "ASSET-CORE-01D", "ASSET-CORE-01E"
)
$MissingSubSlices = 0
foreach ($SubSlice in $SubSlices) {
    if ($null -eq $AuditContent -or -not $AuditContent.Contains($SubSlice)) {
        $MissingSubSlices++
        $Violations++
        Show-Red "  FAIL -- audit doc does not mention $SubSlice"
    }
}
if ($MissingSubSlices -eq 0) {
    Show-Green "  OK -- audit doc mentions all five sub-slices"
}

Write-Host ""
Show-Info "--- [3] Audit doc mentions InstrumentRegistryV2 ---"
Test-ContentContains "audit doc mentions InstrumentRegistryV2" `
    $AuditContent "InstrumentRegistryV2" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions all seven schema categories ---"
$Categories = @("equity", "etf", "crypto", "future", "option", "forex", "rate")
$MissingCategories = 0
foreach ($Category in $Categories) {
    if ($null -eq $AuditContent -or $AuditContent.IndexOf($Category, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
        $MissingCategories++
        $Violations++
        Show-Red "  FAIL -- audit doc does not mention category '$Category'"
    }
}
if ($MissingCategories -eq 0) {
    Show-Green "  OK -- audit doc mentions all seven schema categories"
}

Write-Host ""
Show-Info "--- [5] Audit doc states registry-v2 is not production trading truth ---"
Test-ContentContains "audit doc states registry-v2 is not consumed by trading paths" `
    $AuditContent "is **not** consumed by any trading" | Out-Null

Write-Host ""
Show-Info "--- [6]-[7] No trading-enabled / production-cutover claims ---"
$ContentLower = $null
if ($null -ne $AuditContent) { $ContentLower = $AuditContent.ToLowerInvariant() }

$TradingEnabledPhrases = @(
    "crypto trading is enabled",
    "futures trading is enabled",
    "options trading is enabled",
    "forex trading is enabled",
    "rates trading is enabled",
    "trading is now enabled",
    "approved for live trading"
)
$CutoverPhrases = @(
    "production cutover is complete",
    "production cutover has happened",
    "registry v2 is now production",
    "cutover to registry v2 is complete"
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

$CutoverClaimViolations = 0
foreach ($Phrase in $CutoverPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $CutoverClaimViolations++
        $Violations++
        Show-Red "  FAIL -- audit doc contains forbidden production-cutover claim: '$Phrase'"
    }
}
if ($CutoverClaimViolations -eq 0) {
    Show-Green "  OK -- no forbidden production-cutover claim found"
}

Write-Host ""
Show-Info "--- [8] Audit doc names a next patch ---"
Test-ContentContains "audit doc names a recommended next patch" `
    $AuditContent "Recommended next patch" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- ASSET-CORE-01 completion audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
