# =============================================================================
# ASSET-CORE-04 -- Live Ledger Boundary Audit / Closure Validator
# =============================================================================
# Pure docs/text validation of the ASSET-CORE-04 live ledger boundary audit
# and (once Phase E lands) closure decision. No network call, no
# provider/broker call, no DB connection, no daemon start, no cargo/npm
# build or test.
#
# Checks:
#   [1]  Boundary audit spec exists.
#   [2]  Audit doc mentions ASSET-CORE-04.
#   [3]  Audit doc mentions live accounting.
#   [4]  Audit doc mentions InstrumentEconomics.
#   [5]  Audit doc mentions multiplier.
#   [6]  Audit doc mentions margin.
#   [7]  Audit doc mentions NAV.
#   [8]  Audit doc mentions single currency.
#   [9]  Audit doc states no live/paper order was submitted.
#   [10] Audit doc states no non-equity trading is enabled.
#   [11] Audit doc states production consumption remains separate.
#   [12] No forbidden trading-enabled claim.
#   [13] Closure decision doc, if present, mentions ASSET-CORE-04 and does
#        not claim unconditional full closure.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_04_live_ledger_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec   = Join-Path $RepoRoot "docs\specs\asset_core_04_live_ledger_boundary_audit.md"
$PathClosureSpec = Join-Path $RepoRoot "docs\specs\asset_core_04_live_ledger_closure_decision.md"

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
Write-Host " ASSET-CORE-04 Live Ledger Boundary Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Boundary audit spec exists ---"
$AuditContent = $null
if (Test-FileExists "Boundary audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2]-[8] Audit doc mentions required terms ---"
Test-ContentContains "audit doc mentions ASSET-CORE-04"     $AuditContent "ASSET-CORE-04"     | Out-Null
Test-ContentContains "audit doc mentions live accounting"   $AuditContent "live accounting"   | Out-Null
Test-ContentContains "audit doc mentions InstrumentEconomics" $AuditContent "InstrumentEconomics" | Out-Null
Test-ContentContains "audit doc mentions multiplier"        $AuditContent "multiplier"        | Out-Null
Test-ContentContains "audit doc mentions margin"            $AuditContent "margin"            | Out-Null
Test-ContentContains "audit doc mentions NAV"                $AuditContent "NAV"               | Out-Null
Test-ContentContains "audit doc mentions single currency"   $AuditContent "single currency"   | Out-Null

Write-Host ""
Show-Info "--- [9] Audit doc states no live/paper order was submitted ---"
Test-ContentContains "audit doc states no live or paper order was submitted" `
    $AuditContent "No live or paper order was submitted" | Out-Null

Write-Host ""
Show-Info "--- [10] Audit doc states no non-equity trading is enabled ---"
Test-ContentContains "audit doc states no non-equity asset class was enabled" `
    $AuditContent "No non-equity asset class was enabled" | Out-Null

Write-Host ""
Show-Info "--- [11] Audit doc states production consumption remains separate ---"
Test-ContentContains "audit doc states production consumption remains separate" `
    $AuditContent "Production consumption" | Out-Null

Write-Host ""
Show-Info "--- [12] No forbidden trading-enabled claim ---"
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
    "asset-core-04 is fully closed",
    "live accounting now consumes instrumenteconomics"
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

Write-Host ""
Show-Info "--- [13] Closure decision doc (if present) is honest about scope ---"
if (Test-Path $PathClosureSpec) {
    $ClosureContent = Get-Content -Raw -Path $PathClosureSpec
    Test-ContentContains "closure doc mentions ASSET-CORE-04" $ClosureContent "ASSET-CORE-04" | Out-Null

    $ClosureLower = $ClosureContent.ToLowerInvariant()
    $UnconditionalClosurePhrases = @(
        "asset-core-04: closed`n",
        "asset-core-04 is fully closed",
        "asset-core-04: closed / production-complete"
    )
    $UnconditionalViolations = 0
    foreach ($Phrase in $UnconditionalClosurePhrases) {
        if ($ClosureLower.Contains($Phrase)) {
            $UnconditionalViolations++
            $Violations++
            Show-Red "  FAIL -- closure doc claims unconditional full closure: '$Phrase'"
        }
    }
    if ($UnconditionalViolations -eq 0) {
        Show-Green "  OK -- closure doc does not claim unconditional full closure"
    }
} else {
    Show-Info "  SKIP -- closure decision doc not yet written (Phase E not reached)"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- ASSET-CORE-04 live ledger boundary evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
