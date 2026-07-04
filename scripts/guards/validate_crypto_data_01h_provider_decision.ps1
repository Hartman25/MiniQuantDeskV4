# =============================================================================
# CRYPTO-DATA-01H -- Live Crypto Provider Decision Artifact Validator
# =============================================================================
# Validates docs/specs/crypto_data_01h_live_provider_decision.json against the
# contract its own decision document
# (crypto_data_01h_live_provider_decision.md) establishes. Pure docs/JSON
# validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1] JSON parses.
#   [2] trading_enabled / paper_trading_enabled / live_trading_enabled are
#       exactly `false`.
#   [3] network_calls_made is exactly `false`.
#   [4] db_mutation_made is exactly `false`.
#   [5] requires_db_migration is exactly `false`.
#   [6] chosen_first_verification_provider is a non-empty string.
#   [7] chosen_first_verification_patch is a non-empty string.
#   [8] chosen_first_implementation_patch is a non-empty string.
#   [9] provider_candidates array is non-empty.
#   [10] BTC/USD and ETH/USD are both listed in canonical_symbols.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_data_01h_provider_decision.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot    = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$DecisionPath = Join-Path $RepoRoot "docs\specs\crypto_data_01h_live_provider_decision.json"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " CRYPTO-DATA-01H Provider Decision Artifact Validator"
Write-Host " File: $DecisionPath"
Write-Host "============================================================"

if (-not (Test-Path $DecisionPath)) {
    Show-Red "FAIL -- decision artifact not found at $DecisionPath"
    exit 1
}

# =============================================================================
# [1] JSON parses.
# =============================================================================
Write-Host ""
Show-Info "--- [1] JSON parses ---"

try {
    $Json = Get-Content -Raw -Path $DecisionPath | ConvertFrom-Json
    Show-Green "  OK -- JSON parsed successfully"
} catch {
    Show-Red "FAIL -- decision artifact is not valid JSON: $($_.Exception.Message)"
    exit 1
}

# =============================================================================
# [2] Trading-enablement flags are false.
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
# [3] network_calls_made is false.
# =============================================================================
Write-Host ""
Show-Info "--- [3] network_calls_made is false ---"

if ($Json.network_calls_made -ne $false) {
    $Violations++
    Show-Red "  FAIL -- network_calls_made must be false, got: $($Json.network_calls_made)"
} else {
    Show-Green "  OK -- network_calls_made is false"
}

# =============================================================================
# [4] db_mutation_made is false.
# =============================================================================
Write-Host ""
Show-Info "--- [4] db_mutation_made is false ---"

if ($Json.db_mutation_made -ne $false) {
    $Violations++
    Show-Red "  FAIL -- db_mutation_made must be false, got: $($Json.db_mutation_made)"
} else {
    Show-Green "  OK -- db_mutation_made is false"
}

# =============================================================================
# [5] requires_db_migration is false.
# =============================================================================
Write-Host ""
Show-Info "--- [5] requires_db_migration is false ---"

if ($Json.requires_db_migration -ne $false) {
    $Violations++
    Show-Red "  FAIL -- requires_db_migration must be false, got: $($Json.requires_db_migration)"
} else {
    Show-Green "  OK -- requires_db_migration is false"
}

# =============================================================================
# [6] chosen_first_verification_provider is a non-empty string.
# =============================================================================
Write-Host ""
Show-Info "--- [6] chosen_first_verification_provider present ---"

$VerifyProvider = $Json.chosen_first_verification_provider
if (-not (($VerifyProvider -is [string]) -and ($VerifyProvider.Trim().Length -gt 0))) {
    $Violations++
    Show-Red "  FAIL -- chosen_first_verification_provider must be a non-empty string"
} else {
    Show-Green "  OK -- chosen_first_verification_provider='$VerifyProvider'"
}

# =============================================================================
# [7] chosen_first_verification_patch is a non-empty string.
# =============================================================================
Write-Host ""
Show-Info "--- [7] chosen_first_verification_patch present ---"

$VerifyPatch = $Json.chosen_first_verification_patch
if (-not (($VerifyPatch -is [string]) -and ($VerifyPatch.Trim().Length -gt 0))) {
    $Violations++
    Show-Red "  FAIL -- chosen_first_verification_patch must be a non-empty string"
} else {
    Show-Green "  OK -- chosen_first_verification_patch='$VerifyPatch'"
}

# =============================================================================
# [8] chosen_first_implementation_patch is a non-empty string.
# =============================================================================
Write-Host ""
Show-Info "--- [8] chosen_first_implementation_patch present ---"

$ImplPatch = $Json.chosen_first_implementation_patch
if (-not (($ImplPatch -is [string]) -and ($ImplPatch.Trim().Length -gt 0))) {
    $Violations++
    Show-Red "  FAIL -- chosen_first_implementation_patch must be a non-empty string"
} else {
    Show-Green "  OK -- chosen_first_implementation_patch='$ImplPatch'"
}

# =============================================================================
# [9] provider_candidates array is non-empty.
# =============================================================================
Write-Host ""
Show-Info "--- [9] provider_candidates non-empty ---"

$Candidates = @($Json.provider_candidates)
if ($Candidates.Count -eq 0) {
    $Violations++
    Show-Red "  FAIL -- provider_candidates must contain at least one entry"
} else {
    Show-Green "  OK -- provider_candidates has $($Candidates.Count) entries"
}

# =============================================================================
# [10] BTC/USD and ETH/USD are both listed in canonical_symbols.
# =============================================================================
Write-Host ""
Show-Info "--- [10] BTC/USD and ETH/USD listed in canonical_symbols ---"

$Symbols = @($Json.canonical_symbols)
$HasBtc = $Symbols -contains "BTC/USD"
$HasEth = $Symbols -contains "ETH/USD"

if (-not $HasBtc) {
    $Violations++
    Show-Red "  FAIL -- canonical_symbols missing BTC/USD"
}
if (-not $HasEth) {
    $Violations++
    Show-Red "  FAIL -- canonical_symbols missing ETH/USD"
}
if ($HasBtc -and $HasEth) {
    Show-Green "  OK -- canonical_symbols contains BTC/USD and ETH/USD"
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
