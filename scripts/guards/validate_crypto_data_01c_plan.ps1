# =============================================================================
# CRYPTO-DATA-01C -- Provider Ingest + Scheduler Plan Artifact Validator
# =============================================================================
# Validates docs/specs/crypto_data_01c_provider_ingest_scheduler_plan.json
# against the contract its own design document
# (crypto_data_01c_provider_ingest_scheduler_design.md) establishes. Pure
# docs/JSON validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1] Required top-level fields are present.
#   [2] Every trading-enablement flag is exactly `false`.
#   [3] default_off is exactly `true`.
#   [4] requires_db_migration is exactly `false`.
#   [5] requires_network_calls_in_next_patch / next_patch_network_calls_allowed
#       are both exactly `false`.
#   [6] recommended_next_patch is a non-empty string.
#   [7] chosen_source_lane is a non-empty scalar string (exactly one lane).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_data_01c_plan.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$PlanPath   = Join-Path $RepoRoot "docs\specs\crypto_data_01c_provider_ingest_scheduler_plan.json"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " CRYPTO-DATA-01C Plan Artifact Validator"
Write-Host " File: $PlanPath"
Write-Host "============================================================"

if (-not (Test-Path $PlanPath)) {
    Show-Red "FAIL -- plan artifact not found at $PlanPath"
    exit 1
}

try {
    $Json = Get-Content -Raw -Path $PlanPath | ConvertFrom-Json
} catch {
    Show-Red "FAIL -- plan artifact is not valid JSON: $($_.Exception.Message)"
    exit 1
}

# =============================================================================
# [1] Required top-level fields are present.
# =============================================================================
Write-Host ""
Show-Info "--- [1] Required fields present ---"

$RequiredFields = @(
    "patch_id", "truth_state", "chosen_source_lane", "chosen_provider",
    "canonical_symbols", "storage_target", "storage_key",
    "requires_db_migration", "requires_network_calls_in_next_patch",
    "next_patch_network_calls_allowed", "trading_enabled",
    "paper_trading_enabled", "live_trading_enabled", "default_off",
    "operator_explicit_run_required", "scheduler_implementation_deferred",
    "recommended_next_patch", "rejected_or_deferred_sources",
    "required_evidence_fields", "required_fail_closed_states"
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
# [3] default_off is exactly `true`.
# =============================================================================
Write-Host ""
Show-Info "--- [3] default_off is true ---"

if ($Json.default_off -ne $true) {
    $Violations++
    Show-Red "  FAIL -- default_off must be true, got: $($Json.default_off)"
} else {
    Show-Green "  OK -- default_off is true"
}

# =============================================================================
# [4] requires_db_migration is exactly `false`.
# =============================================================================
Write-Host ""
Show-Info "--- [4] requires_db_migration is false ---"

if ($Json.requires_db_migration -ne $false) {
    $Violations++
    Show-Red "  FAIL -- requires_db_migration must be false, got: $($Json.requires_db_migration)"
} else {
    Show-Green "  OK -- requires_db_migration is false"
}

# =============================================================================
# [5] No network calls are allowed in or by this design patch's next step.
# =============================================================================
Write-Host ""
Show-Info "--- [5] No network calls allowed in next patch ---"

$NetworkFlags = @("requires_network_calls_in_next_patch", "next_patch_network_calls_allowed")
$NetworkViolations = 0

foreach ($Flag in $NetworkFlags) {
    $Value = $Json.$Flag
    if ($Value -ne $false) {
        $NetworkViolations++
        $Violations++
        Show-Red "  FAIL -- $Flag must be false, got: $Value"
    }
}
if ($NetworkViolations -eq 0) {
    Show-Green "  OK -- requires_network_calls_in_next_patch / next_patch_network_calls_allowed both false"
}

# =============================================================================
# [6] recommended_next_patch is a non-empty string.
# =============================================================================
Write-Host ""
Show-Info "--- [6] recommended_next_patch present ---"

$NextPatch = $Json.recommended_next_patch
if (-not (($NextPatch -is [string]) -and ($NextPatch.Trim().Length -gt 0))) {
    $Violations++
    Show-Red "  FAIL -- recommended_next_patch must be a non-empty string"
} else {
    Show-Green "  OK -- recommended_next_patch='$NextPatch'"
}

# =============================================================================
# [7] chosen_source_lane is exactly one non-empty scalar string.
# =============================================================================
Write-Host ""
Show-Info "--- [7] Exactly one source lane is chosen ---"

$Lane = $Json.chosen_source_lane
$IsScalarNonEmptyString = ($Lane -is [string]) -and ($Lane.Trim().Length -gt 0)
if (-not $IsScalarNonEmptyString) {
    $Violations++
    Show-Red "  FAIL -- chosen_source_lane must be a single non-empty string, got: $($Lane | ConvertTo-Json -Compress)"
} else {
    Show-Green "  OK -- chosen_source_lane='$Lane'"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- plan artifact is contract-valid."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
