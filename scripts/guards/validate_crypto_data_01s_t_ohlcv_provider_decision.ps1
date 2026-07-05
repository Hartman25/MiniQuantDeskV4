# =============================================================================
# CRYPTO-DATA-01S-T -- OHLCV Provider Decision/Verification Artifact Validator
# =============================================================================
# Validates docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.json
# against the contract its own decision/evidence document
# (crypto_data_01s_t_ohlcv_provider_decision_verify.md) establishes. Pure
# docs/JSON validation:
#
#   - no network call, no provider/broker call, no DB connection
#   - no daemon start, no cargo build/test
#
# Checks:
#   [1]  JSON parses.
#   [2]  schema_version == "crypto-data-01s-t-ohlcv-provider-decision-v1".
#   [3]  patch_id == "CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED".
#   [4]  selected_provider is present and non-empty.
#   [5]  network_requests_used <= 6 and requests.Count == network_requests_used.
#   [6]  every request method is GET (no POST/PUT/DELETE).
#   [7]  no request is authenticated / uses a credential or API key.
#   [8]  BTC/USD and ETH/USD are both present and verified in symbols_verified.
#   [9]  every safety.* boolean is true.
#   [10] the decision doc (.md) exists.
#   [11] no forbidden implementation claims (provider adapter added, md_bars
#        write, DB migration, trading enabled) appear as true/affirmative
#        claims in the JSON or literal banned phrases in the doc.
#   [12] the decision doc does not claim CoinLore provides OHLCV.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_data_01s_t_ohlcv_provider_decision.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$ArtifactPath = Join-Path $RepoRoot "docs\specs\crypto_data_01s_t_ohlcv_provider_decision_verify.json"
$DocPath      = Join-Path $RepoRoot "docs\specs\crypto_data_01s_t_ohlcv_provider_decision_verify.md"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " CRYPTO-DATA-01S-T OHLCV Provider Decision Validator"
Write-Host " File: $ArtifactPath"
Write-Host "============================================================"

if (-not (Test-Path $ArtifactPath)) {
    Show-Red "FAIL -- artifact not found at $ArtifactPath"
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
    Show-Red "FAIL -- artifact is not valid JSON: $($_.Exception.Message)"
    exit 1
}

# =============================================================================
# [2] schema_version.
# =============================================================================
Write-Host ""
Show-Info "--- [2] schema_version ---"

$ExpectedSchema = "crypto-data-01s-t-ohlcv-provider-decision-v1"
if ($Json.schema_version -ne $ExpectedSchema) {
    $Violations++
    Show-Red "  FAIL -- schema_version must be '$ExpectedSchema', got: $($Json.schema_version)"
} else {
    Show-Green "  OK -- schema_version='$ExpectedSchema'"
}

# =============================================================================
# [3] patch_id.
# =============================================================================
Write-Host ""
Show-Info "--- [3] patch_id ---"

$ExpectedPatchId = "CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED"
if ($Json.patch_id -ne $ExpectedPatchId) {
    $Violations++
    Show-Red "  FAIL -- patch_id must be '$ExpectedPatchId', got: $($Json.patch_id)"
} else {
    Show-Green "  OK -- patch_id='$ExpectedPatchId'"
}

# =============================================================================
# [4] selected_provider present and non-empty.
# =============================================================================
Write-Host ""
Show-Info "--- [4] selected_provider present ---"

if ([string]::IsNullOrWhiteSpace($Json.selected_provider)) {
    $Violations++
    Show-Red "  FAIL -- selected_provider is missing or empty"
} else {
    Show-Green "  OK -- selected_provider='$($Json.selected_provider)'"
}

# =============================================================================
# [5] network_requests_used <= 6 and matches requests.Count.
# =============================================================================
Write-Host ""
Show-Info "--- [5] network_requests_used <= 6 and matches requests array ---"

$Requests = @($Json.requests)
$UsedCount = $Json.network_requests_used
$MaxAllowed = $Json.max_network_requests_allowed

if ($null -eq $UsedCount -or $UsedCount -lt 0 -or $UsedCount -gt 6) {
    $Violations++
    Show-Red "  FAIL -- network_requests_used must be between 0 and 6, got: $UsedCount"
} elseif ($Requests.Count -ne $UsedCount) {
    $Violations++
    Show-Red "  FAIL -- requests array length ($($Requests.Count)) does not match network_requests_used ($UsedCount)"
} elseif ($MaxAllowed -ne 6) {
    $Violations++
    Show-Red "  FAIL -- max_network_requests_allowed must be 6, got: $MaxAllowed"
} else {
    Show-Green "  OK -- network_requests_used=$UsedCount, requests.Count=$($Requests.Count), max_network_requests_allowed=6"
}

# =============================================================================
# [6] every request method is GET.
# =============================================================================
Write-Host ""
Show-Info "--- [6] every request method is GET ---"

$MethodViolations = 0
foreach ($Req in $Requests) {
    if ($Req.method -ne "GET") {
        $MethodViolations++
        $Violations++
        Show-Red "  FAIL -- request '$($Req.purpose)' uses forbidden method: $($Req.method)"
    }
}
if ($MethodViolations -eq 0) {
    Show-Green "  OK -- all $($Requests.Count) request(s) use GET"
}

# =============================================================================
# [7] no request is authenticated / uses a credential or API key.
# =============================================================================
Write-Host ""
Show-Info "--- [7] no authenticated/private request ---"

$AuthViolations = 0
foreach ($Req in $Requests) {
    if ($Req.authenticated -ne $false -or $Req.credential_used -ne $false -or $Req.api_key_used -ne $false) {
        $AuthViolations++
        $Violations++
        Show-Red "  FAIL -- request '$($Req.purpose)' is flagged authenticated/credentialed"
    }
    $UrlLower = ([string]$Req.url).ToLowerInvariant()
    if ($UrlLower -match "api_key|apikey|token=|secret=|password=") {
        $AuthViolations++
        $Violations++
        Show-Red "  FAIL -- request '$($Req.purpose)' URL appears to embed a credential: $($Req.url)"
    }
}
if ($AuthViolations -eq 0) {
    Show-Green "  OK -- no request is authenticated or embeds a credential"
}

# =============================================================================
# [8] BTC/USD and ETH/USD both present and verified.
# =============================================================================
Write-Host ""
Show-Info "--- [8] BTC/USD and ETH/USD verified ---"

$Symbols = @($Json.symbols_verified)
$Btc = $Symbols | Where-Object { $_.canonical_symbol -eq "BTC/USD" }
$Eth = $Symbols | Where-Object { $_.canonical_symbol -eq "ETH/USD" }

if (-not $Btc) {
    $Violations++
    Show-Red "  FAIL -- symbols_verified missing BTC/USD"
} elseif ($Btc.verified -ne $true) {
    $Violations++
    Show-Red "  FAIL -- BTC/USD present but verified != true"
}

if (-not $Eth) {
    $Violations++
    Show-Red "  FAIL -- symbols_verified missing ETH/USD"
} elseif ($Eth.verified -ne $true) {
    $Violations++
    Show-Red "  FAIL -- ETH/USD present but verified != true"
}

if ($Btc -and $Eth -and $Btc.verified -eq $true -and $Eth.verified -eq $true) {
    Show-Green "  OK -- BTC/USD and ETH/USD both present and verified"
}

# =============================================================================
# [9] every safety.* boolean is true.
# =============================================================================
Write-Host ""
Show-Info "--- [9] safety booleans all true ---"

$SafetyFields = @(
    "no_credentials", "no_db_write", "no_md_bars_write", "no_provider_adapter_added",
    "no_trading_enabled", "no_rust_gui_config_changed", "no_db_migration", "no_raw_responses_staged"
)
$SafetyViolations = 0
foreach ($Field in $SafetyFields) {
    $Value = $Json.safety.$Field
    if ($Value -ne $true) {
        $SafetyViolations++
        $Violations++
        Show-Red "  FAIL -- safety.$Field must be true, got: $Value"
    }
}
if ($SafetyViolations -eq 0) {
    Show-Green "  OK -- all safety booleans are true"
}

# =============================================================================
# [10] decision doc exists.
# =============================================================================
Write-Host ""
Show-Info "--- [10] decision doc exists ---"

if (-not (Test-Path $DocPath)) {
    $Violations++
    Show-Red "  FAIL -- decision doc not found at $DocPath"
    $DocContent = ""
} else {
    Show-Green "  OK -- decision doc found at $DocPath"
    $DocContent = Get-Content -Raw -Path $DocPath
}

# =============================================================================
# [11] no forbidden implementation claims.
# =============================================================================
Write-Host ""
Show-Info "--- [11] no forbidden implementation claims ---"

$ForbiddenPhrases = @(
    "provider adapter added in this patch",
    "adapter implemented in this patch",
    "md_bars write added",
    "db migration added",
    "trading enabled in this patch",
    "crypto trading enabled"
)
$ClaimViolations = 0
$DocLower = $DocContent.ToLowerInvariant()
foreach ($Phrase in $ForbiddenPhrases) {
    if ($DocLower.Contains($Phrase)) {
        $ClaimViolations++
        $Violations++
        Show-Red "  FAIL -- decision doc contains forbidden implementation claim: '$Phrase'"
    }
}
if ($Json.safety.no_provider_adapter_added -ne $true -or $Json.safety.no_db_migration -ne $true -or `
    $Json.safety.no_md_bars_write -ne $true -or $Json.safety.no_trading_enabled -ne $true) {
    $ClaimViolations++
    $Violations++
    Show-Red "  FAIL -- JSON safety flags contradict the no-implementation posture required by this patch"
}
if ($ClaimViolations -eq 0) {
    Show-Green "  OK -- no forbidden implementation claims found"
}

# =============================================================================
# [12] doc does not claim CoinLore provides OHLCV.
# =============================================================================
Write-Host ""
Show-Info "--- [12] doc does not claim CoinLore OHLCV ---"

$CoinloreOhlcvPhrases = @(
    "coinlore ohlcv verified",
    "coinlore provides ohlcv",
    "coinlore historical bars available",
    "coinlore is ohlcv"
)
$CoinloreViolations = 0
foreach ($Phrase in $CoinloreOhlcvPhrases) {
    if ($DocLower.Contains($Phrase)) {
        $CoinloreViolations++
        $Violations++
        Show-Red "  FAIL -- decision doc contains forbidden CoinLore-OHLCV claim: '$Phrase'"
    }
}
if ($CoinloreViolations -eq 0) {
    Show-Green "  OK -- no CoinLore-provides-OHLCV claim found"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- decision/verification artifact is contract-valid."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
