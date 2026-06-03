# =============================================================================
# AAPL-5M-MARKET-SMOKE-RUNNER-01 -- Script static invariant tests
#
# Reads Run-AAPL5mMarketSmoke.ps1 as text and asserts AMR01-AMR14.
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_aapl5m_market_smoke_runner.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Run-AAPL5mMarketSmoke.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Run-AAPL5mMarketSmoke.ps1 static invariant tests (AMR01-AMR14) ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# AMR01: Runner script exists
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'AMR01' "Runner script exists at scripts\windows\Run-AAPL5mMarketSmoke.ps1"
} else {
    Fail 'AMR01' "Runner script NOT found at: $TargetScript"
}

$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# AMR02: Default RefreshSource is 'alpaca'
# ---------------------------------------------------------------------------
if ($scriptText -match "\`$RefreshSource\s*=\s*'alpaca'") {
    Pass 'AMR02' "RefreshSource defaults to 'alpaca'."
} else {
    Fail 'AMR02' "RefreshSource does not default to 'alpaca'."
}

# ---------------------------------------------------------------------------
# AMR03: Calls Refresh-IntradayMarketData.ps1
# ---------------------------------------------------------------------------
if ($scriptText -match 'Refresh-IntradayMarketData\.ps1') {
    Pass 'AMR03' "Script calls Refresh-IntradayMarketData.ps1."
} else {
    Fail 'AMR03' "Script does NOT call Refresh-IntradayMarketData.ps1."
}

# ---------------------------------------------------------------------------
# AMR04: Calls Start-PaperTradingSmoke.ps1 with -CheckOnly
# ---------------------------------------------------------------------------
if ($scriptText -match 'Start-PaperTradingSmoke\.ps1.*-CheckOnly|-CheckOnly.*Start-PaperTradingSmoke\.ps1|SmokeScript.*-CheckOnly|-CheckOnly.*SmokeScript') {
    Pass 'AMR04' "Script calls Start-PaperTradingSmoke.ps1 (or SmokeScript) with -CheckOnly."
} else {
    Fail 'AMR04' "Script does NOT call Start-PaperTradingSmoke.ps1 -CheckOnly."
}

# ---------------------------------------------------------------------------
# AMR05: Calls Start-PaperTradingSmoke.ps1 with -WatchSeconds
# ---------------------------------------------------------------------------
if ($scriptText -match 'Start-PaperTradingSmoke\.ps1.*-WatchSeconds|-WatchSeconds.*Start-PaperTradingSmoke\.ps1|SmokeScript.*-WatchSeconds|-WatchSeconds.*SmokeScript') {
    Pass 'AMR05' "Script calls Start-PaperTradingSmoke.ps1 (or SmokeScript) with -WatchSeconds."
} else {
    Fail 'AMR05' "Script does NOT call Start-PaperTradingSmoke.ps1 -WatchSeconds."
}

# ---------------------------------------------------------------------------
# AMR06: Calls Capture-PaperSmokeEvidence.ps1
# ---------------------------------------------------------------------------
if ($scriptText -match 'Capture-PaperSmokeEvidence\.ps1') {
    Pass 'AMR06' "Script calls Capture-PaperSmokeEvidence.ps1."
} else {
    Fail 'AMR06' "Script does NOT call Capture-PaperSmokeEvidence.ps1."
}

# ---------------------------------------------------------------------------
# AMR07: Sets MQK_STRATEGY_MD_TIMEFRAME to 5m
# ---------------------------------------------------------------------------
if ($scriptText -match "MQK_STRATEGY_MD_TIMEFRAME.*=.*\`$Timeframe|MQK_STRATEGY_MD_TIMEFRAME.*5m") {
    Pass 'AMR07' "Script sets MQK_STRATEGY_MD_TIMEFRAME (to Timeframe param, default 5m)."
} else {
    Fail 'AMR07' "Script does NOT set MQK_STRATEGY_MD_TIMEFRAME."
}

# ---------------------------------------------------------------------------
# AMR08: Sets conservative qty/notional env vars
# ---------------------------------------------------------------------------
$setsQty      = $scriptText -match 'MQK_STRATEGY_TARGET_QTY'
$setsMaxQty   = $scriptText -match 'MQK_STRATEGY_MAX_TARGET_QTY'
$setsNotional = $scriptText -match 'MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD'
if ($setsQty -and $setsMaxQty -and $setsNotional) {
    Pass 'AMR08' "Script sets MQK_STRATEGY_TARGET_QTY, MQK_STRATEGY_MAX_TARGET_QTY, MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD."
} else {
    Fail 'AMR08' "Script missing one or more conservative qty/notional env vars."
}

# ---------------------------------------------------------------------------
# AMR09: Does not reference /v2/orders (no direct broker order endpoint)
# ---------------------------------------------------------------------------
if ($scriptText -notmatch '/v2/orders') {
    Pass 'AMR09' "Script does not reference /v2/orders."
} else {
    Fail 'AMR09' "Script references /v2/orders -- manual broker order submission is forbidden."
}

# ---------------------------------------------------------------------------
# AMR10: Does not reference live_routing_enabled=true or any live-enable flag
# ---------------------------------------------------------------------------
if ($scriptText -notmatch 'live_routing_enabled\s*=\s*true|live_routing_enabled=true|--enable-live|enable_live') {
    Pass 'AMR10' "Script does not reference live routing enable flags."
} else {
    Fail 'AMR10' "Script references a live routing enable flag -- forbidden."
}

# ---------------------------------------------------------------------------
# AMR11: Does not print secrets
# ---------------------------------------------------------------------------
$secretPrints = @(
    'MQK_OPERATOR_TOKEN',
    'ALPACA_API_SECRET',
    'DISCORD_WEBHOOK_URL',
    'TWELVEDATA_API_KEY'
)
$secretViolations = @()
foreach ($secret in $secretPrints) {
    # Allow referencing the var name in comments or checks, but not printing its value.
    # Pattern: Write-Host / echo with the secret variable directly in the string output.
    if ($scriptText -match "Write-Host[^`n]*\`$$secret|echo[^`n]*\`$$secret") {
        $secretViolations += $secret
    }
}
if ($secretViolations.Count -eq 0) {
    Pass 'AMR11' "Script does not print secrets via Write-Host/echo."
} else {
    Fail 'AMR11' "Script may print secrets: $($secretViolations -join ', ')."
}

# ---------------------------------------------------------------------------
# AMR12: Has -CheckOnly parameter
# ---------------------------------------------------------------------------
if ($scriptText -match '\[switch\]\s*\$CheckOnly') {
    Pass 'AMR12' "Script has -CheckOnly switch parameter."
} else {
    Fail 'AMR12' "Script does NOT have -CheckOnly switch parameter."
}

# ---------------------------------------------------------------------------
# AMR13: Writes to logs\aapl5m_market_smoke
# ---------------------------------------------------------------------------
if ($scriptText -match 'logs\\aapl5m_market_smoke') {
    Pass 'AMR13' "Script writes to logs\aapl5m_market_smoke."
} else {
    Fail 'AMR13' "Script does NOT write to logs\aapl5m_market_smoke."
}

# ---------------------------------------------------------------------------
# AMR14: No signal injection reference
# ---------------------------------------------------------------------------
$signalInjectionPatterns = @(
    'POST.*strategy/signal',
    'strategy/signal',
    'Invoke-RestMethod.*signal',
    'inject.*signal.*route',
    'signal.*inject.*api'
)
# Check only non-comment lines
$nonCommentLines = ($scriptText -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
$signalViolations = @()
foreach ($pat in $signalInjectionPatterns) {
    if ($nonCommentLines -match $pat) {
        $signalViolations += $pat
    }
}
if ($signalViolations.Count -eq 0) {
    Pass 'AMR14' "Script has no signal injection reference."
} else {
    Fail 'AMR14' "Script references signal injection pattern(s): $($signalViolations -join '; ')."
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "ALL AMR GUARDS PASSED (14/14)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "FAILED: $Failures AMR guard(s) did not pass." -ForegroundColor Red
    exit 1
}
