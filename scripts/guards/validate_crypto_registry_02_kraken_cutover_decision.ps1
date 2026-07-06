# =============================================================================
# CRYPTO-REGISTRY-02 -- Kraken Data Registry Cutover Decision Validator
# =============================================================================
# Validates docs/specs/crypto_registry_02_kraken_data_registry_cutover_decision.json
# against the contract its own decision document
# (crypto_registry_02_kraken_data_registry_cutover_decision.md) establishes.
# Pure docs/JSON validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1]  JSON parses.
#   [2]  schema_version is exactly "crypto-registry-02-kraken-cutover-decision-v1".
#   [3]  patch_id is exactly "CRYPTO-REGISTRY-02-KRAKEN-DATA-REGISTRY-CUTOVER-DECISION-01".
#   [4]  selected_provider is exactly "kraken".
#   [5]  BTC/USD and ETH/USD are both listed in symbols.
#   [6]  safety booleans are all in their required (safe) state.
#   [7]  decision doc exists.
#   [8]  decision doc does not claim crypto trading readiness.
#   [9]  decision doc does not claim scheduler readiness.
#   [10] decision doc does not instruct setting paper_trading_enabled=true
#        or live_trading_enabled=true.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_registry_02_kraken_cutover_decision.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$DecisionPath = Join-Path $RepoRoot "docs\specs\crypto_registry_02_kraken_data_registry_cutover_decision.json"
$DocPath      = Join-Path $RepoRoot "docs\specs\crypto_registry_02_kraken_data_registry_cutover_decision.md"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " CRYPTO-REGISTRY-02 Kraken Cutover Decision Artifact Validator"
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
# [2] schema_version.
# =============================================================================
Write-Host ""
Show-Info "--- [2] schema_version ---"

$ExpectedSchemaVersion = "crypto-registry-02-kraken-cutover-decision-v1"
if ($Json.schema_version -ne $ExpectedSchemaVersion) {
    $Violations++
    Show-Red "  FAIL -- schema_version must be '$ExpectedSchemaVersion', got: $($Json.schema_version)"
} else {
    Show-Green "  OK -- schema_version='$($Json.schema_version)'"
}

# =============================================================================
# [3] patch_id.
# =============================================================================
Write-Host ""
Show-Info "--- [3] patch_id ---"

$ExpectedPatchId = "CRYPTO-REGISTRY-02-KRAKEN-DATA-REGISTRY-CUTOVER-DECISION-01"
if ($Json.patch_id -ne $ExpectedPatchId) {
    $Violations++
    Show-Red "  FAIL -- patch_id must be '$ExpectedPatchId', got: $($Json.patch_id)"
} else {
    Show-Green "  OK -- patch_id='$($Json.patch_id)'"
}

# =============================================================================
# [4] selected_provider.
# =============================================================================
Write-Host ""
Show-Info "--- [4] selected_provider ---"

if ($Json.selected_provider -ne "kraken") {
    $Violations++
    Show-Red "  FAIL -- selected_provider must be 'kraken', got: $($Json.selected_provider)"
} else {
    Show-Green "  OK -- selected_provider='kraken'"
}

# =============================================================================
# [5] BTC/USD and ETH/USD listed in symbols.
# =============================================================================
Write-Host ""
Show-Info "--- [5] BTC/USD and ETH/USD listed in symbols ---"

$Symbols = @($Json.symbols)
$HasBtc = $Symbols -contains "BTC/USD"
$HasEth = $Symbols -contains "ETH/USD"

if (-not $HasBtc) {
    $Violations++
    Show-Red "  FAIL -- symbols missing BTC/USD"
}
if (-not $HasEth) {
    $Violations++
    Show-Red "  FAIL -- symbols missing ETH/USD"
}
if ($HasBtc -and $HasEth) {
    Show-Green "  OK -- symbols contains BTC/USD and ETH/USD"
}

# =============================================================================
# [6] safety booleans in required (safe) state.
# =============================================================================
Write-Host ""
Show-Info "--- [6] safety booleans ---"

$RequiredTrue = @(
    "no_scheduler", "no_network_call", "no_db_write", "no_trading_enabled",
    "no_provider_adapter_added", "no_db_migration", "no_config_flag_changed"
)
$RequiredFalse = @(
    "kraken_provider_enabled", "btc_usd_enabled", "eth_usd_enabled",
    "paper_trading_enabled", "live_trading_enabled"
)

$SafetyViolations = 0
foreach ($Flag in $RequiredTrue) {
    $Value = $Json.safety.$Flag
    if ($Value -ne $true) {
        $SafetyViolations++
        $Violations++
        Show-Red "  FAIL -- safety.$Flag must be true, got: $Value"
    }
}
foreach ($Flag in $RequiredFalse) {
    $Value = $Json.safety.$Flag
    if ($Value -ne $false) {
        $SafetyViolations++
        $Violations++
        Show-Red "  FAIL -- safety.$Flag must be false, got: $Value"
    }
}
if ($SafetyViolations -eq 0) {
    Show-Green "  OK -- all safety booleans in required state"
}

# =============================================================================
# [7]-[10] decision doc checks.
# =============================================================================
Write-Host ""
Show-Info "--- [7] decision doc exists ---"

$DocContent = $null
if (-not (Test-Path $DocPath)) {
    $Violations++
    Show-Red "  FAIL -- decision doc not found at $DocPath"
} else {
    Show-Green "  OK -- decision doc found at $DocPath"
    $DocContent = Get-Content -Raw -Path $DocPath
}

Write-Host ""
Show-Info "--- [8] doc does not claim crypto trading readiness ---"

$TradingReadinessPhrases = @(
    "crypto trading is ready",
    "crypto trading readiness achieved",
    "ready for crypto trading",
    "crypto trading enabled in this patch",
    "approved for crypto trading"
)
$TradingViolations = 0
if ($DocContent) {
    $DocLower = $DocContent.ToLowerInvariant()
    foreach ($Phrase in $TradingReadinessPhrases) {
        if ($DocLower.Contains($Phrase)) {
            $TradingViolations++
            $Violations++
            Show-Red "  FAIL -- decision doc contains forbidden trading-readiness claim: '$Phrase'"
        }
    }
}
if ($TradingViolations -eq 0) {
    Show-Green "  OK -- no crypto-trading-readiness claim found"
}

Write-Host ""
Show-Info "--- [9] doc does not claim scheduler readiness ---"

$SchedulerReadinessPhrases = @(
    "scheduler is ready",
    "scheduler added in this patch",
    "recurring sync enabled",
    "scheduled kraken sync is live",
    "kraken sync is scheduled"
)
$SchedulerViolations = 0
if ($DocContent) {
    foreach ($Phrase in $SchedulerReadinessPhrases) {
        if ($DocLower.Contains($Phrase)) {
            $SchedulerViolations++
            $Violations++
            Show-Red "  FAIL -- decision doc contains forbidden scheduler-readiness claim: '$Phrase'"
        }
    }
}
if ($SchedulerViolations -eq 0) {
    Show-Green "  OK -- no scheduler-readiness claim found"
}

Write-Host ""
Show-Info "--- [10] doc does not instruct enabling trading flags ---"

$TradingFlagPhrases = @(
    "set paper_trading_enabled=true",
    "set paper_trading_enabled to true",
    "set live_trading_enabled=true",
    "set live_trading_enabled to true"
)
$FlagViolations = 0
if ($DocContent) {
    foreach ($Phrase in $TradingFlagPhrases) {
        if ($DocLower.Contains($Phrase)) {
            $FlagViolations++
            $Violations++
            Show-Red "  FAIL -- decision doc instructs enabling a trading flag: '$Phrase'"
        }
    }
}
if ($FlagViolations -eq 0) {
    Show-Green "  OK -- no instruction to enable paper_trading_enabled/live_trading_enabled found"
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
