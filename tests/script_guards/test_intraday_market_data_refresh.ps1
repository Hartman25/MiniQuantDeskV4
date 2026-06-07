# =============================================================================
# INTRADAY-5M-LIVE-BAR-INGESTION-01 -- Script static invariant tests
#
# Reads Refresh-IntradayMarketData.ps1 as text and asserts IMR01-IMR14.
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_intraday_market_data_refresh.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Refresh-IntradayMarketData.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Refresh-IntradayMarketData.ps1 static invariant tests ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# IMR01: Script exists
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'IMR01' "Script exists at scripts\windows\Refresh-IntradayMarketData.ps1"
} else {
    Fail 'IMR01' "Script NOT found at: $TargetScript"
}

$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# IMR02: Script is ASCII-safe
# ---------------------------------------------------------------------------
$nonAscii = [regex]::Matches($scriptText, '[^\x00-\x7E]')
if ($nonAscii.Count -eq 0) {
    Pass 'IMR02' "Script is ASCII-safe."
} else {
    Fail 'IMR02' "Script contains $($nonAscii.Count) non-ASCII character(s)."
}

# ---------------------------------------------------------------------------
# IMR03: Paper DB port 5440 guard present
# ---------------------------------------------------------------------------
if ($scriptText -match '5440') {
    Pass 'IMR03' "Paper DB port 5440 guard present."
} else {
    Fail 'IMR03' "Missing paper DB port 5440 guard."
}

# ---------------------------------------------------------------------------
# IMR04: TWELVEDATA_API_KEY presence checked; value not printed
# ---------------------------------------------------------------------------
$keyCheck = $scriptText -match 'TWELVEDATA_API_KEY'
$keyPrint = $scriptText -match 'Write-Host[^#]*\$env:TWELVEDATA_API_KEY'
if ($keyCheck -and -not $keyPrint) {
    Pass 'IMR04' "TWELVEDATA_API_KEY presence checked; value not printed."
} elseif (-not $keyCheck) {
    Fail 'IMR04' "TWELVEDATA_API_KEY not referenced."
} else {
    Fail 'IMR04' "Script appears to print TWELVEDATA_API_KEY value."
}

# ---------------------------------------------------------------------------
# IMR05: -CheckOnly parameter present
# ---------------------------------------------------------------------------
if ($scriptText -match '\[switch\]\s*\$CheckOnly') {
    Pass 'IMR05' "-CheckOnly parameter present."
} else {
    Fail 'IMR05' "-CheckOnly parameter missing."
}

# ---------------------------------------------------------------------------
# IMR06: -Once parameter present (single-run protection)
# ---------------------------------------------------------------------------
if ($scriptText -match '\[switch\]\s*\$Once') {
    Pass 'IMR06' "-Once parameter present."
} else {
    Fail 'IMR06' "-Once parameter missing."
}

# ---------------------------------------------------------------------------
# IMR07: -IntervalSeconds and -DurationSeconds present (loop protection)
# ---------------------------------------------------------------------------
$hasInterval  = $scriptText -match '\[int\]\s*\$IntervalSeconds'
$hasDuration  = $scriptText -match '\[int\]\s*\$DurationSeconds'
if ($hasInterval -and $hasDuration) {
    Pass 'IMR07' "-IntervalSeconds and -DurationSeconds parameters present."
} else {
    Fail 'IMR07' "Missing interval/duration protection parameters."
}

# ---------------------------------------------------------------------------
# IMR08: -MinCompletedBars parameter present
# ---------------------------------------------------------------------------
if ($scriptText -match '\[int\]\s*\$MinCompletedBars') {
    Pass 'IMR08' "-MinCompletedBars parameter present."
} else {
    Fail 'IMR08' "-MinCompletedBars parameter missing."
}

# ---------------------------------------------------------------------------
# IMR09: No broker order submission calls on non-comment lines
# Forbidden executable patterns: /v2/orders, submit_order, place.*order
# (comment lines that mention these for documentation are acceptable)
# ---------------------------------------------------------------------------
$nonCommentLines = ($scriptText -split "`n") | Where-Object { $_ -notmatch '^\s*#' }
$nonCommentText  = $nonCommentLines -join "`n"
$brokerOrderPatterns = @('/v2/orders', 'submit_order', 'place.*order', 'Invoke-.*[Oo]rder')
$brokerViolation = $false
foreach ($pat in $brokerOrderPatterns) {
    if ($nonCommentText -match $pat) {
        $brokerViolation = $true
        break
    }
}
if (-not $brokerViolation) {
    Pass 'IMR09' "No broker order submission calls found on non-comment lines."
} else {
    Fail 'IMR09' "Forbidden broker order pattern found on non-comment lines."
}

# ---------------------------------------------------------------------------
# IMR10: No OMS/outbox mutation calls on non-comment lines
# Forbidden: oms_outbox, oms_inbox, outbox_enqueue
# (arm_state and broker_order_map may appear in comments as safety documentation)
# ---------------------------------------------------------------------------
$omsPatterns = @('oms_outbox', 'oms_inbox', 'outbox_enqueue')
$omsViolation = $false
foreach ($pat in $omsPatterns) {
    if ($nonCommentText -match $pat) {
        $omsViolation = $true
        break
    }
}
if (-not $omsViolation) {
    Pass 'IMR10' "No OMS/outbox mutation patterns found on non-comment lines."
} else {
    Fail 'IMR10' "Forbidden OMS/outbox pattern found on non-comment lines."
}

# ---------------------------------------------------------------------------
# IMR11: No live-routing enabling patterns
# Forbidden: live_routing_enabled=true, MQK_DAEMON_ADAPTER_ID.*live, LiveCapital
# ---------------------------------------------------------------------------
$livePatterns = @('live_routing_enabled\s*=\s*true', 'MQK_DAEMON_ADAPTER_ID.*live', 'LiveCapital')
$liveViolation = $false
foreach ($pat in $livePatterns) {
    if ($scriptText -match $pat) {
        $liveViolation = $true
        break
    }
}
if (-not $liveViolation) {
    Pass 'IMR11' "No live routing enabling patterns found."
} else {
    Fail 'IMR11' "Forbidden live routing pattern found in script."
}

# ---------------------------------------------------------------------------
# IMR12: Uses mqk-cli md sync-provider (existing ingestion path)
# ---------------------------------------------------------------------------
if ($scriptText -match 'md sync-provider') {
    Pass 'IMR12' "Uses existing mqk-cli md sync-provider ingestion path."
} else {
    Fail 'IMR12' "Expected 'md sync-provider' call not found."
}

# ---------------------------------------------------------------------------
# IMR13: Evidence file written to exports/market_data/
# ---------------------------------------------------------------------------
if ($scriptText -match 'exports.market_data') {
    Pass 'IMR13' "Evidence written to exports/market_data/."
} else {
    Fail 'IMR13' "Expected exports/market_data evidence path not found."
}

# ---------------------------------------------------------------------------
# IMR14: Symbols driven by parameter, not hardcoded-only
# (Script must reference $sym or $symbolList, not just hardcoded 'AAPL')
# ---------------------------------------------------------------------------
if ($scriptText -match '\$sym\b' -or $scriptText -match '\$symbolList') {
    Pass 'IMR14' 'Symbols driven by parameter ($sym / $symbolList), not hardcoded-only.'
} else {
    Fail 'IMR14' 'Symbols do not appear to be parameterized.'
}

# ---------------------------------------------------------------------------
# IMR15: -Source parameter present (CURRENT-SESSION-5M-BAR-PROVIDER-01)
# ---------------------------------------------------------------------------
if ($scriptText -match '\[ValidateSet\(.*twelvedata.*alpaca') {
    Pass 'IMR15' "-Source parameter with ValidateSet(twelvedata, alpaca) present."
} else {
    Fail 'IMR15' "-Source ValidateSet parameter missing (expected twelvedata|alpaca)."
}

# ---------------------------------------------------------------------------
# IMR16: Alpaca key presence checked; value not printed
# (CURRENT-SESSION-5M-BAR-PROVIDER-01)
# ---------------------------------------------------------------------------
$alpacaKeyCheck = $scriptText -match 'ALPACA_API_KEY_PAPER'
$alpacaSecretCheck = $scriptText -match 'ALPACA_API_SECRET_PAPER'
$alpacaKeyPrint = $scriptText -match 'Write-Host[^#]*\$env:ALPACA_API_KEY_PAPER'
$alpacaSecretPrint = $scriptText -match 'Write-Host[^#]*\$env:ALPACA_API_SECRET_PAPER'
if ($alpacaKeyCheck -and $alpacaSecretCheck -and -not $alpacaKeyPrint -and -not $alpacaSecretPrint) {
    Pass 'IMR16' "ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER presence checked; values not printed."
} elseif (-not $alpacaKeyCheck -or -not $alpacaSecretCheck) {
    Fail 'IMR16' "Alpaca paper credentials not referenced in script."
} else {
    Fail 'IMR16' "Script appears to print Alpaca credential values."
}

# ---------------------------------------------------------------------------
# IMR17: Alpaca data endpoint only (no order endpoints) — non-comment lines
# (CURRENT-SESSION-5M-BAR-PROVIDER-01)
# Forbidden on non-comment lines: /v2/positions, broker.*submit, cancel.*order
# ---------------------------------------------------------------------------
$alpacaOrderPatterns = @('/v2/positions', 'broker.*submit', 'cancel.*order')
$alpacaOrderViolation = $false
foreach ($pat in $alpacaOrderPatterns) {
    if ($nonCommentText -match $pat) {
        $alpacaOrderViolation = $true
        break
    }
}
if (-not $alpacaOrderViolation) {
    Pass 'IMR17' "No Alpaca order/position endpoint calls on non-comment lines."
} else {
    Fail 'IMR17' "Forbidden Alpaca order/position endpoint pattern on non-comment lines."
}

# ---------------------------------------------------------------------------
# IMR18: PAPER-SMOKE-MD-REFRESH-FAIL-CLOSED-01 -- $allPassed fails closed on
# provider sync failure / missing key / low bar count / staleness, not just
# on post-refresh query failure.
# ---------------------------------------------------------------------------
$invokeOneRefreshBlock = [regex]::Match($scriptText, '(?s)function Invoke-OneRefresh \{.*?\n\}')
if ($invokeOneRefreshBlock.Success) {
    $refreshBody = $invokeOneRefreshBlock.Value

    $setsFailedOnSyncFailure  = $refreshBody -match "Provider sync failed[\s\S]*?\`$symFailed = \`$true"
    $setsFailedOnSyncThrow    = $refreshBody -match "Provider sync threw exception[\s\S]*?\`$symFailed = \`$true"
    $setsFailedOnMissingKey   = $refreshBody -match "Skipping provider sync[\s\S]*?\`$symFailed = \`$true"
    $setsFailedOnLowBarCount  = $refreshBody -match "Below MinCompletedBars[\s\S]*?\`$symFailed = \`$true"
    $setsFailedOnStaleness    = $refreshBody -match "Still stale by[\s\S]*?\`$symFailed = \`$true"
    $propagatesToAnyFailed    = $refreshBody -match '\$anyFailed = \$true'
    $allPassedDerivedFromFail = $refreshBody -match '\$allPassed\s*=\s*\(-not \$anyFailed\)'

    if ($setsFailedOnSyncFailure) {
        Pass 'IMR18a' "Provider sync failure (non-zero exit) marks readiness failed."
    } else {
        Fail 'IMR18a' "Provider sync failure does not mark readiness failed."
    }
    if ($setsFailedOnSyncThrow) {
        Pass 'IMR18b' "Provider sync exception marks readiness failed."
    } else {
        Fail 'IMR18b' "Provider sync exception does not mark readiness failed."
    }
    if ($setsFailedOnMissingKey) {
        Pass 'IMR18c' "Missing provider key/config marks readiness failed."
    } else {
        Fail 'IMR18c' "Missing provider key/config does not mark readiness failed."
    }
    if ($setsFailedOnLowBarCount) {
        Pass 'IMR18d' "Bar count below -MinCompletedBars marks readiness failed."
    } else {
        Fail 'IMR18d' "Low bar count does not mark readiness failed."
    }
    if ($setsFailedOnStaleness) {
        Pass 'IMR18e' "Staleness above -MaxStalenessMinutes marks readiness failed."
    } else {
        Fail 'IMR18e' "Staleness above threshold does not mark readiness failed."
    }
    if ($propagatesToAnyFailed -and $allPassedDerivedFromFail) {
        Pass 'IMR18f' 'Per-symbol failures propagate to $anyFailed and $allPassed = (-not $anyFailed).'
    } else {
        Fail 'IMR18f' 'Per-symbol failure does not provably propagate to $allPassed.'
    }
} else {
    Fail 'IMR18a' "Could not isolate Invoke-OneRefresh function body."
    Fail 'IMR18b' "Could not isolate Invoke-OneRefresh function body."
    Fail 'IMR18c' "Could not isolate Invoke-OneRefresh function body."
    Fail 'IMR18d' "Could not isolate Invoke-OneRefresh function body."
    Fail 'IMR18e' "Could not isolate Invoke-OneRefresh function body."
    Fail 'IMR18f' "Could not isolate Invoke-OneRefresh function body."
}

# ---------------------------------------------------------------------------
# IMR19: -Once mode exit code derives from Invoke-OneRefresh's $allPassed
# (the smoke-readiness gate); interval loop discards it (unchanged behavior).
# ---------------------------------------------------------------------------
$onceUsesReturn   = $scriptText -match '\$ok\s*=\s*Invoke-OneRefresh' -and $scriptText -match 'exit \$exitCode'
$loopDiscardsRet  = $scriptText -match '\$null\s*=\s*Invoke-OneRefresh'
if ($onceUsesReturn) {
    Pass 'IMR19a' "-Once mode derives its exit code from Invoke-OneRefresh's `$allPassed return value."
} else {
    Fail 'IMR19a' "-Once mode does not appear to gate its exit code on Invoke-OneRefresh's return value."
}
if ($loopDiscardsRet) {
    Pass 'IMR19b' "Interval loop discards Invoke-OneRefresh's return value (informational continue-on-warn preserved)."
} else {
    Fail 'IMR19b' "Interval loop does not discard Invoke-OneRefresh's return value -- loop behavior may have changed."
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "  ALL PASS  (25/25 assertions)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $Failures FAILURE(S)  -- see FAIL lines above" -ForegroundColor Red
    exit 1
}
