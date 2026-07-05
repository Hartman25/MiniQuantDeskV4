# =============================================================================
# CRYPTO-DATA-01I -- CoinLore Network Verification Artifact Validator
# =============================================================================
# Validates docs/specs/crypto_data_01i_coinlore_network_verify.json against
# the contract its own evidence document
# (crypto_data_01i_coinlore_network_verify.md) establishes. Pure docs/JSON
# validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1]  JSON parses.
#   [2]  provider == "coinlore".
#   [3]  network_calls_made == true.
#   [4]  request_count <= 3.
#   [5]  credentials_used == false.
#   [6]  api_key_used == false.
#   [7]  db_mutation_made == false.
#   [8]  requires_db_migration == false.
#   [9]  trading_enabled / paper_trading_enabled / live_trading_enabled all false.
#   [10] BTC/USD and ETH/USD both listed in canonical_symbols.
#   [11] decision is one of PROCEED_TO_01J / PARTIAL_TICKER_ONLY /
#        BLOCKED_PROVIDER_UNFIT / BLOCKED_MORE_REQUESTS_REQUIRED.
#   [12] if decision == PROCEED_TO_01J, btc.verified and eth.verified are both true.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_data_01i_coinlore_verify.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir     = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot      = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$ArtifactPath  = Join-Path $RepoRoot "docs\specs\crypto_data_01i_coinlore_network_verify.json"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " CRYPTO-DATA-01I CoinLore Network Verification Validator"
Write-Host " File: $ArtifactPath"
Write-Host "============================================================"

if (-not (Test-Path $ArtifactPath)) {
    Show-Red "FAIL -- verification artifact not found at $ArtifactPath"
    exit 1
}

# =============================================================================
# [1] JSON parses.
# =============================================================================
Write-Host ""
Show-Info "--- [1] JSON parses ---"

try {
    $Json = Get-Content -Raw -Path $ArtifactPath | ConvertFrom-Json
    Show-Green "  OK -- JSON parsed successfully"
} catch {
    Show-Red "FAIL -- verification artifact is not valid JSON: $($_.Exception.Message)"
    exit 1
}

# =============================================================================
# [2] provider == "coinlore".
# =============================================================================
Write-Host ""
Show-Info "--- [2] provider == coinlore ---"

if ($Json.provider -ne "coinlore") {
    $Violations++
    Show-Red "  FAIL -- provider must be 'coinlore', got: $($Json.provider)"
} else {
    Show-Green "  OK -- provider='coinlore'"
}

# =============================================================================
# [3] network_calls_made == true.
# =============================================================================
Write-Host ""
Show-Info "--- [3] network_calls_made == true ---"

if ($Json.network_calls_made -ne $true) {
    $Violations++
    Show-Red "  FAIL -- network_calls_made must be true, got: $($Json.network_calls_made)"
} else {
    Show-Green "  OK -- network_calls_made is true"
}

# =============================================================================
# [4] request_count <= 3.
# =============================================================================
Write-Host ""
Show-Info "--- [4] request_count <= 3 ---"

$RequestCount = $Json.request_count
if ($null -eq $RequestCount -or $RequestCount -gt 3 -or $RequestCount -lt 0) {
    $Violations++
    Show-Red "  FAIL -- request_count must be between 0 and 3, got: $RequestCount"
} else {
    Show-Green "  OK -- request_count=$RequestCount"
}

# =============================================================================
# [5]/[6] credentials_used / api_key_used == false.
# =============================================================================
Write-Host ""
Show-Info "--- [5]/[6] credentials_used and api_key_used are false ---"

$CredentialViolations = 0
if ($Json.credentials_used -ne $false) {
    $CredentialViolations++
    $Violations++
    Show-Red "  FAIL -- credentials_used must be false, got: $($Json.credentials_used)"
}
if ($Json.api_key_used -ne $false) {
    $CredentialViolations++
    $Violations++
    Show-Red "  FAIL -- api_key_used must be false, got: $($Json.api_key_used)"
}
if ($CredentialViolations -eq 0) {
    Show-Green "  OK -- credentials_used and api_key_used both false"
}

# =============================================================================
# [7]/[8] db_mutation_made / requires_db_migration == false.
# =============================================================================
Write-Host ""
Show-Info "--- [7]/[8] db_mutation_made and requires_db_migration are false ---"

$DbViolations = 0
if ($Json.db_mutation_made -ne $false) {
    $DbViolations++
    $Violations++
    Show-Red "  FAIL -- db_mutation_made must be false, got: $($Json.db_mutation_made)"
}
if ($Json.requires_db_migration -ne $false) {
    $DbViolations++
    $Violations++
    Show-Red "  FAIL -- requires_db_migration must be false, got: $($Json.requires_db_migration)"
}
if ($DbViolations -eq 0) {
    Show-Green "  OK -- db_mutation_made and requires_db_migration both false"
}

# =============================================================================
# [9] trading_enabled / paper_trading_enabled / live_trading_enabled are false.
# =============================================================================
Write-Host ""
Show-Info "--- [9] trading-enablement flags are false ---"

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
# [11] decision is one of the allowed enum values.
# =============================================================================
Write-Host ""
Show-Info "--- [11] decision is a recognized value ---"

$AllowedDecisions = @(
    "PROCEED_TO_01J",
    "PARTIAL_TICKER_ONLY",
    "BLOCKED_PROVIDER_UNFIT",
    "BLOCKED_MORE_REQUESTS_REQUIRED"
)
$Decision = $Json.decision

if ($AllowedDecisions -notcontains $Decision) {
    $Violations++
    Show-Red "  FAIL -- decision must be one of $($AllowedDecisions -join ', '), got: $Decision"
} else {
    Show-Green "  OK -- decision='$Decision'"
}

# =============================================================================
# [12] if decision == PROCEED_TO_01J, both btc.verified and eth.verified must
#      be true.
# =============================================================================
Write-Host ""
Show-Info "--- [12] PROCEED_TO_01J requires both BTC and ETH verified ---"

if ($Decision -eq "PROCEED_TO_01J") {
    $BtcVerified = $Json.btc.verified
    $EthVerified = $Json.eth.verified
    if ($BtcVerified -ne $true -or $EthVerified -ne $true) {
        $Violations++
        Show-Red "  FAIL -- decision=PROCEED_TO_01J requires btc.verified and eth.verified both true (btc=$BtcVerified, eth=$EthVerified)"
    } else {
        Show-Green "  OK -- btc.verified and eth.verified both true"
    }
} else {
    Show-Green "  SKIP -- decision is not PROCEED_TO_01J, no additional verification requirement"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- verification artifact is contract-valid."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
