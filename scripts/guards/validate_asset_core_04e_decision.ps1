# =============================================================================
# ASSET-CORE-04E -- Decision Artifact Contract Validator (PowerShell)
# =============================================================================
# Validates docs/specs/asset_core_04e_non_equity_mark_source_decision.json
# against the contract its own decision document (asset_core_04e_non_equity_
# mark_source_decision.md) establishes. Pure docs/JSON validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1] Required top-level fields are present.
#   [2] Every trading-enablement flag is exactly `false`.
#   [3] provider_network_calls_allowed is exactly `false`.
#   [4] Exactly one lane is chosen (chosen_lane/chosen_asset_class are
#       non-empty scalar strings, not arrays).
#   [5] recommended_next_patch is a non-empty string.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_asset_core_04e_decision.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot    = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$DecisionPath = Join-Path $RepoRoot "docs\specs\asset_core_04e_non_equity_mark_source_decision.json"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " ASSET-CORE-04E Decision Artifact Validator"
Write-Host " File: $DecisionPath"
Write-Host "============================================================"

if (-not (Test-Path $DecisionPath)) {
    Show-Red "FAIL -- decision artifact not found at $DecisionPath"
    exit 1
}

try {
    $Json = Get-Content -Raw -Path $DecisionPath | ConvertFrom-Json
} catch {
    Show-Red "FAIL -- decision artifact is not valid JSON: $($_.Exception.Message)"
    exit 1
}

# =============================================================================
# [1] Required top-level fields are present.
# =============================================================================
Write-Host ""
Show-Info "--- [1] Required fields present ---"

$RequiredFields = @(
    "patch_id", "truth_state", "chosen_lane", "chosen_asset_class",
    "chosen_symbols", "provider_network_calls_allowed", "trading_enabled",
    "paper_trading_enabled", "live_trading_enabled", "rejected_lanes",
    "recommended_next_patch"
)

foreach ($Field in $RequiredFields) {
    if (-not ($Json.PSObject.Properties.Name -contains $Field)) {
        $Violations++
        Show-Red "  FAIL -- missing required field: $Field"
    }
}
if ($Violations -eq 0) {
    Show-Green "  OK -- all $($RequiredFields.Count) required fields present"
}

# =============================================================================
# [2] Every trading-enablement flag is exactly `false`.
# =============================================================================
Write-Host ""
Show-Info "--- [2] Trading-enablement flags are false ---"

$EnablementFlags = @("trading_enabled", "paper_trading_enabled", "live_trading_enabled")
$EnablementViolations = 0

foreach ($Flag in $EnablementFlags) {
    $Value = $Json.$Flag
    if ($Value -ne $false) {
        $EnablementViolations++
        $Violations++
        Show-Red "  FAIL -- $Flag must be false, got: $Value"
    }
}
if ($EnablementViolations -eq 0) {
    Show-Green "  OK -- trading_enabled / paper_trading_enabled / live_trading_enabled all false"
}

# =============================================================================
# [3] provider_network_calls_allowed is exactly `false`.
# =============================================================================
Write-Host ""
Show-Info "--- [3] provider_network_calls_allowed is false ---"

if ($Json.provider_network_calls_allowed -ne $false) {
    $Violations++
    Show-Red "  FAIL -- provider_network_calls_allowed must be false, got: $($Json.provider_network_calls_allowed)"
} else {
    Show-Green "  OK -- provider_network_calls_allowed is false"
}

# =============================================================================
# [4] Exactly one lane is chosen.
# =============================================================================
Write-Host ""
Show-Info "--- [4] Exactly one lane is chosen ---"

$LaneFields = @("chosen_lane", "chosen_asset_class")
$LaneViolations = 0

foreach ($Field in $LaneFields) {
    $Value = $Json.$Field
    $IsScalarNonEmptyString = ($Value -is [string]) -and ($Value.Trim().Length -gt 0)
    if (-not $IsScalarNonEmptyString) {
        $LaneViolations++
        $Violations++
        Show-Red "  FAIL -- $Field must be a single non-empty string, got: $($Value | ConvertTo-Json -Compress)"
    }
}
if ($LaneViolations -eq 0) {
    Show-Green "  OK -- chosen_lane='$($Json.chosen_lane)', chosen_asset_class='$($Json.chosen_asset_class)' (exactly one of each)"
}

# =============================================================================
# [5] recommended_next_patch is a non-empty string.
# =============================================================================
Write-Host ""
Show-Info "--- [5] recommended_next_patch present ---"

$NextPatch = $Json.recommended_next_patch
if (-not (($NextPatch -is [string]) -and ($NextPatch.Trim().Length -gt 0))) {
    $Violations++
    Show-Red "  FAIL -- recommended_next_patch must be a non-empty string"
} else {
    Show-Green "  OK -- recommended_next_patch='$NextPatch'"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- decision artifact is contract-valid."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
