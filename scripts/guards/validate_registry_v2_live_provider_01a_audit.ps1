# =============================================================================
# REGISTRY-V2-LIVE-PROVIDER-01A -- Current Non-Equity Provider Audit Validator
# =============================================================================
# Pure docs/text validation of the registry-v2 live-provider audit and (once
# written, in later phases) its boundary decision / preflight / closure docs.
# No network call, no provider/broker call, no DB connection, no daemon
# start, no cargo/npm build or test.
#
# Checks:
#   [1] Audit doc exists.
#   [2] Audit doc mentions prerequisite #4.
#   [3] Audit doc mentions Kraken (the selected candidate).
#   [4] Audit doc mentions BTC/USD and/or ETH/USD.
#   [5] Audit doc states no network call occurred.
#   [6] Audit doc states no DB mutation occurred.
#   [7] Audit doc does not claim prerequisite #4 is closed.
#   [8] Audit doc does not claim trading is enabled.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_registry_v2_live_provider_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec = Join-Path $RepoRoot "docs\specs\registry_v2_live_provider_01a_current_non_equity_provider_audit.md"

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
Write-Host " REGISTRY-V2-LIVE-PROVIDER-01A Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Current non-equity provider audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions prerequisite #4 ---"
Test-ContentContains "audit doc mentions prerequisite #4" $AuditContent "prerequisite #4" | Out-Null

Write-Host ""
Show-Info "--- [3] Audit doc mentions Kraken ---"
Test-ContentContains "audit doc mentions Kraken" $AuditContent "Kraken" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions BTC/USD and/or ETH/USD ---"
$MentionsBtc = ($null -ne $AuditContent -and $AuditContent.IndexOf("BTC/USD", [System.StringComparison]::OrdinalIgnoreCase) -ge 0)
$MentionsEth = ($null -ne $AuditContent -and $AuditContent.IndexOf("ETH/USD", [System.StringComparison]::OrdinalIgnoreCase) -ge 0)
if ($MentionsBtc -or $MentionsEth) {
    Show-Green "  OK -- audit doc mentions BTC/USD and/or ETH/USD"
} else {
    $Violations++
    Show-Red "  FAIL -- audit doc mentions neither BTC/USD nor ETH/USD"
}

Write-Host ""
Show-Info "--- [5] Audit doc states no network call occurred ---"
$HasNoNetworkClaim = $false
if ($null -ne $AuditContent) {
    if ($AuditContent.IndexOf("No network call", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $AuditContent.IndexOf("zero network calls", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $AuditContent.IndexOf("performs zero network calls", [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        $HasNoNetworkClaim = $true
    }
}
if ($HasNoNetworkClaim) {
    Show-Green "  OK -- audit doc states no network call occurred"
} else {
    $Violations++
    Show-Red "  FAIL -- audit doc does not state that no network call occurred"
}

Write-Host ""
Show-Info "--- [6] Audit doc states no DB mutation occurred ---"
$HasNoDbClaim = $false
if ($null -ne $AuditContent) {
    if ($AuditContent.IndexOf("No DB access", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $AuditContent.IndexOf("zero DB writes", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $AuditContent.IndexOf("zero db access", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $AuditContent.IndexOf("performs zero", [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        $HasNoDbClaim = $true
    }
}
if ($HasNoDbClaim) {
    Show-Green "  OK -- audit doc states no DB mutation occurred"
} else {
    $Violations++
    Show-Red "  FAIL -- audit doc does not state that no DB mutation occurred"
}

Write-Host ""
Show-Info "--- [7]-[8] No forbidden closure/trading-enablement claims ---"
$ContentLower = $null
if ($null -ne $AuditContent) { $ContentLower = $AuditContent.ToLowerInvariant() }

$ForbiddenPhrases = @(
    "prerequisite #4 is closed",
    "prerequisite #4 is now closed",
    "prerequisite #4 is satisfied",
    "live proof is complete",
    "live-network proof is complete",
    "crypto trading is enabled",
    "futures trading is enabled",
    "options trading is enabled",
    "forex trading is enabled",
    "non-equity trading is enabled",
    "trading is now enabled",
    "kraken is now enabled"
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
    Show-Green "  OK -- no forbidden closure/trading-enablement claim found"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- REGISTRY-V2-LIVE-PROVIDER-01A audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
