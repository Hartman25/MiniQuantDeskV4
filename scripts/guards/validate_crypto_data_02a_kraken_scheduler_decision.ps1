# =============================================================================
# CRYPTO-DATA-02A -- Kraken Scheduler Rate-Limit Decision Validator
# =============================================================================
# Validates docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.json
# against the contract its own decision document
# (crypto_data_02a_kraken_scheduler_rate_limit_decision.md) establishes.
# Pure docs/JSON validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1]  JSON parses.
#   [2]  schema_version is exactly "crypto-data-02a-kraken-scheduler-rate-limit-decision-v1".
#   [3]  patch_id is exactly "CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01".
#   [4]  provider is exactly "kraken".
#   [5]  BTC/USD and ETH/USD are both listed in symbols.
#   [6]  scheduler_registration_status is exactly "not_registered".
#   [7]  min_seconds_between_pair_calls >= 2.
#   [8]  min_seconds_between_scheduled_runs >= 86400.
#   [9]  max_ohlc_calls_per_run <= 2.
#   [10] max_total_network_calls_per_run <= 4.
#   [11] concurrency is exactly "sequential_only".
#   [12] safety booleans are all in their required (safe) state.
#   [13] decision doc exists.
#   [14] decision doc does not claim crypto trading readiness.
#   [15] decision doc does not instruct registering a scheduled task.
#   [16] decision doc does not instruct setting paper_trading_enabled=true
#        or live_trading_enabled=true.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_data_02a_kraken_scheduler_decision.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$DecisionPath = Join-Path $RepoRoot "docs\specs\crypto_data_02a_kraken_scheduler_rate_limit_decision.json"
$DocPath      = Join-Path $RepoRoot "docs\specs\crypto_data_02a_kraken_scheduler_rate_limit_decision.md"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " CRYPTO-DATA-02A Kraken Scheduler Decision Artifact Validator"
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

$ExpectedSchemaVersion = "crypto-data-02a-kraken-scheduler-rate-limit-decision-v1"
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

$ExpectedPatchId = "CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01"
if ($Json.patch_id -ne $ExpectedPatchId) {
    $Violations++
    Show-Red "  FAIL -- patch_id must be '$ExpectedPatchId', got: $($Json.patch_id)"
} else {
    Show-Green "  OK -- patch_id='$($Json.patch_id)'"
}

# =============================================================================
# [4] provider.
# =============================================================================
Write-Host ""
Show-Info "--- [4] provider ---"

if ($Json.provider -ne "kraken") {
    $Violations++
    Show-Red "  FAIL -- provider must be 'kraken', got: $($Json.provider)"
} else {
    Show-Green "  OK -- provider='kraken'"
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
# [6] scheduler_registration_status.
# =============================================================================
Write-Host ""
Show-Info "--- [6] scheduler_registration_status ---"

if ($Json.scheduler_registration_status -ne "not_registered") {
    $Violations++
    Show-Red "  FAIL -- scheduler_registration_status must be 'not_registered', got: $($Json.scheduler_registration_status)"
} else {
    Show-Green "  OK -- scheduler_registration_status='not_registered'"
}

# =============================================================================
# [7] min_seconds_between_pair_calls >= 2.
# =============================================================================
Write-Host ""
Show-Info "--- [7] min_seconds_between_pair_calls >= 2 ---"

if ($Json.min_seconds_between_pair_calls -lt 2) {
    $Violations++
    Show-Red "  FAIL -- min_seconds_between_pair_calls must be >= 2, got: $($Json.min_seconds_between_pair_calls)"
} else {
    Show-Green "  OK -- min_seconds_between_pair_calls=$($Json.min_seconds_between_pair_calls)"
}

# =============================================================================
# [8] min_seconds_between_scheduled_runs >= 86400.
# =============================================================================
Write-Host ""
Show-Info "--- [8] min_seconds_between_scheduled_runs >= 86400 ---"

if ($Json.min_seconds_between_scheduled_runs -lt 86400) {
    $Violations++
    Show-Red "  FAIL -- min_seconds_between_scheduled_runs must be >= 86400, got: $($Json.min_seconds_between_scheduled_runs)"
} else {
    Show-Green "  OK -- min_seconds_between_scheduled_runs=$($Json.min_seconds_between_scheduled_runs)"
}

# =============================================================================
# [9] max_ohlc_calls_per_run <= 2.
# =============================================================================
Write-Host ""
Show-Info "--- [9] max_ohlc_calls_per_run <= 2 ---"

if ($Json.max_ohlc_calls_per_run -gt 2) {
    $Violations++
    Show-Red "  FAIL -- max_ohlc_calls_per_run must be <= 2, got: $($Json.max_ohlc_calls_per_run)"
} else {
    Show-Green "  OK -- max_ohlc_calls_per_run=$($Json.max_ohlc_calls_per_run)"
}

# =============================================================================
# [10] max_total_network_calls_per_run <= 4.
# =============================================================================
Write-Host ""
Show-Info "--- [10] max_total_network_calls_per_run <= 4 ---"

if ($Json.max_total_network_calls_per_run -gt 4) {
    $Violations++
    Show-Red "  FAIL -- max_total_network_calls_per_run must be <= 4, got: $($Json.max_total_network_calls_per_run)"
} else {
    Show-Green "  OK -- max_total_network_calls_per_run=$($Json.max_total_network_calls_per_run)"
}

# =============================================================================
# [11] concurrency is sequential_only.
# =============================================================================
Write-Host ""
Show-Info "--- [11] concurrency ---"

if ($Json.concurrency -ne "sequential_only") {
    $Violations++
    Show-Red "  FAIL -- concurrency must be 'sequential_only', got: $($Json.concurrency)"
} else {
    Show-Green "  OK -- concurrency='sequential_only'"
}

# =============================================================================
# [12] safety booleans in required (safe) state.
# =============================================================================
Write-Host ""
Show-Info "--- [12] safety booleans ---"

$RequiredTrue = @(
    "no_scheduled_task_registered", "no_daemon_job_added", "no_network_call_to_kraken_api",
    "no_db_write", "no_trading_enabled"
)
$RequiredFalse = @(
    "kraken_provider_enabled", "paper_trading_enabled", "live_trading_enabled"
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
# [13]-[16] decision doc checks.
# =============================================================================
Write-Host ""
Show-Info "--- [13] decision doc exists ---"

$DocContent = $null
if (-not (Test-Path $DocPath)) {
    $Violations++
    Show-Red "  FAIL -- decision doc not found at $DocPath"
} else {
    Show-Green "  OK -- decision doc found at $DocPath"
    $DocContent = Get-Content -Raw -Path $DocPath
}

Write-Host ""
Show-Info "--- [14] doc does not claim crypto trading readiness ---"

$TradingReadinessPhrases = @(
    "crypto trading is ready",
    "crypto trading readiness achieved",
    "ready for crypto trading",
    "crypto trading enabled in this patch",
    "approved for crypto trading"
)
$TradingViolations = 0
$DocLower = $null
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
Show-Info "--- [15] doc does not instruct registering a scheduled task ---"

$TaskRegistrationPhrases = @(
    "register the scheduled task",
    "register-scheduledtask",
    "run register-",
    "task is registered in this patch",
    "scheduler is registered"
)
$TaskViolations = 0
if ($DocLower) {
    foreach ($Phrase in $TaskRegistrationPhrases) {
        if ($DocLower.Contains($Phrase)) {
            $TaskViolations++
            $Violations++
            Show-Red "  FAIL -- decision doc instructs registering a scheduled task: '$Phrase'"
        }
    }
}
if ($TaskViolations -eq 0) {
    Show-Green "  OK -- no scheduled-task-registration instruction found"
}

Write-Host ""
Show-Info "--- [16] doc does not instruct enabling trading flags ---"

$TradingFlagPhrases = @(
    "set paper_trading_enabled=true",
    "set paper_trading_enabled to true",
    "set live_trading_enabled=true",
    "set live_trading_enabled to true"
)
$FlagViolations = 0
if ($DocLower) {
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
