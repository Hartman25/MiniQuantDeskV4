# =============================================================================
# ASSET-CORE-05G -- Current Session Routing Audit Validator
# =============================================================================
# Pure docs/text validation of the per-instrument session-routing audit and
# (once written, in later phases) its closure decision doc. No network call,
# no provider/broker call, no DB connection, no daemon start, no cargo/npm
# build or test.
#
# Checks:
#   [1] Audit doc exists.
#   [2] Audit doc mentions ASSET-CORE-05.
#   [3] Audit doc mentions MarketCalendarProvider.
#   [4] Audit doc mentions per-instrument session routing.
#   [5] Audit doc states production trading/admission is unchanged.
#   [6] Audit doc states non-equity profiles remain model-only.
#   [7] Audit doc does not claim non-equity trading is enabled.
#   [8] Audit doc does not claim production cutover occurred.
#   [9] If present, closure decision doc (Phase D) carries an honest status.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_05g_session_routing_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec   = Join-Path $RepoRoot "docs\specs\asset_core_05g_current_session_routing_audit.md"
$PathClosureSpec = Join-Path $RepoRoot "docs\specs\asset_core_05j_session_routing_closure_decision.md"

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
Write-Host " ASSET-CORE-05G Session Routing Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Current session routing audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions ASSET-CORE-05 ---"
Test-ContentContains "audit doc mentions ASSET-CORE-05" $AuditContent "ASSET-CORE-05" | Out-Null

Write-Host ""
Show-Info "--- [3] Audit doc mentions MarketCalendarProvider ---"
Test-ContentContains "audit doc mentions MarketCalendarProvider" $AuditContent "MarketCalendarProvider" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions per-instrument session routing ---"
Test-ContentContains "audit doc mentions per-instrument session routing" $AuditContent "per-instrument session" | Out-Null

Write-Host ""
Show-Info "--- [5] Audit doc states production trading/admission is unchanged ---"
Test-ContentContains "audit doc states production admission unchanged" $AuditContent "admission behavior is unchanged" | Out-Null

Write-Host ""
Show-Info "--- [6] Audit doc states non-equity profiles remain model-only ---"
Test-ContentContains "audit doc states non-equity profiles remain model-only" $AuditContent "remain model-only" | Out-Null

Write-Host ""
Show-Info "--- [7]-[8] No forbidden cutover/trading-enablement claims ---"
$ContentLower = $null
if ($null -ne $AuditContent) { $ContentLower = $AuditContent.ToLowerInvariant() }

$ForbiddenPhrases = @(
    "production cutover is complete",
    "production cutover happened",
    "now consumes instrumentregistryv2",
    "crypto trading is enabled",
    "futures trading is enabled",
    "options trading is enabled",
    "forex trading is enabled",
    "non-equity trading is enabled",
    "trading is now enabled",
    "btc/usd.enabled was set to true",
    "btc/usd is now enabled"
)
$PhraseViolations = 0
foreach ($Phrase in $ForbiddenPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $PhraseViolations++
        $Violations++
        Show-Red "  FAIL -- audit doc contains forbidden claim: '$Phrase'"
    }
}
if ($PhraseViolations -eq 0) {
    Show-Green "  OK -- no forbidden cutover/trading-enablement claim found"
}

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
    Show-Green " ALL CHECKS PASSED -- ASSET-CORE-05G audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
