# =============================================================================
# DAILY-DATA-READINESS-AND-FRESHNESS-01A -- Current-Truth Audit Validator
# =============================================================================
# Pure docs/text validation of the Phase A audit + final contract doc. No
# network call, no provider/broker call, no DB connection, no daemon start,
# no cargo/npm build or test.
#
# Checks:
#   [1]  Audit/contract doc exists.
#   [2]  Doc mentions the patch group id.
#   [3]  Doc corrects the native_strategy.rs path (mqk-runtime, not mqk-daemon).
#   [4]  Doc names the strict evaluator module path.
#   [5]  Doc records the per-strategy minimum_completed_bars table.
#   [6]  Doc states the end_ts-is-bar-start timestamp finding.
#   [7]  Doc names the reused durable evidence table (sys_autonomous_session_events).
#   [8]  Doc states no new migration is required for Phase B/C.
#   [9]  Doc names the new read-only route.
#   [10] Doc states applicability is not keyed solely to BrokerKind::Alpaca.
#   [11] Doc does not claim any runtime/gate/GUI code was implemented in Phase A.
#   [12] Doc does not claim a live/broker/provider action occurred.
#   [13] Doc does not contain any of the corrected pass's stale current-truth claims.
#   [14] Doc contains every corrected-pass concept
#        (DAILY-DATA-READINESS-01A-CONTRACT-CORRECTION-01).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_daily_data_readiness_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec = Join-Path $RepoRoot "docs\specs\daily_data_readiness_01a_current_truth_and_contract.md"

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

function Test-ContentDoesNotContain {
    param([string]$Label, [string]$Content, [string]$Needle)
    if ($null -eq $Content -or $Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (forbidden stale needle found: '$Needle')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " DAILY-DATA-READINESS-01A Current-Truth Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit/contract doc exists ---"
$Content = $null
if (Test-FileExists "Phase A audit/contract spec" $PathAuditSpec) {
    $Content = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Doc mentions the patch group id ---"
Test-ContentContains "doc mentions patch group id" $Content "DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED" | Out-Null

Write-Host ""
Show-Info "--- [3] Doc corrects the native_strategy.rs path ---"
Test-ContentContains "doc corrects native_strategy.rs to mqk-runtime" $Content "mqk-runtime/src/native_strategy.rs" | Out-Null

Write-Host ""
Show-Info "--- [4] Doc names the strict evaluator module path ---"
Test-ContentContains "doc names daily_data_readiness.rs module" $Content "daily_data_readiness.rs" | Out-Null

Write-Host ""
Show-Info "--- [5] Doc records per-strategy minimum_completed_bars table ---"
Test-ContentContains "doc mentions minimum_completed_bars" $Content "minimum_completed_bars" | Out-Null
Test-ContentContains "doc mentions volatility_breakout 21-bar requirement" $Content "volatility_breakout" | Out-Null

Write-Host ""
Show-Info "--- [6] Doc states end_ts-is-bar-start finding ---"
Test-ContentContains "doc states end_ts is the bar-start timestamp" $Content "is bar-start" | Out-Null

Write-Host ""
Show-Info "--- [7] Doc names the reused durable evidence table ---"
Test-ContentContains "doc names sys_autonomous_session_events" $Content "sys_autonomous_session_events" | Out-Null

Write-Host ""
Show-Info "--- [8] Doc states no new migration required ---"
Test-ContentContains "doc states no new migration required" $Content "no new migration required" | Out-Null

Write-Host ""
Show-Info "--- [9] Doc names the new read-only route ---"
Test-ContentContains "doc names GET /api/v1/market-data/readiness" $Content "/api/v1/market-data/readiness" | Out-Null

Write-Host ""
Show-Info "--- [10] Doc states applicability is not keyed solely to BrokerKind::Alpaca ---"
Test-ContentContains "doc states applicability is not hardcoded to BrokerKind::Alpaca" $Content "not hardcoded to" | Out-Null

Write-Host ""
Show-Info "--- [11]-[12] No forbidden implementation/action claims ---"
$ContentLower = $null
if ($null -ne $Content) { $ContentLower = $Content.ToLowerInvariant() }

$ForbiddenPhrases = @(
    "gate is now enforced",
    "route is now live",
    "migration applied",
    "runtime start gate wired",
    "gui panel implemented",
    "provider api call was made",
    "broker call was made",
    "order was submitted",
    "daemon was started"
)
$PhraseViolations = 0
foreach ($Phrase in $ForbiddenPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $PhraseViolations++
        $Violations++
        Show-Red "  FAIL -- audit doc contains forbidden implementation/action claim: '$Phrase'"
    }
}
if ($PhraseViolations -eq 0) {
    Show-Green "  OK -- no forbidden implementation/action claim found"
}

Write-Host ""
Show-Info "--- [13] No stale current-truth claims (DAILY-DATA-READINESS-01A-CONTRACT-CORRECTION-01) ---"

# Each needle below is deliberately a specific, unquoted assertion-style
# fragment from the pre-correction (242de234) doc's own wording, not the
# corrected doc's citation-style references to what was wrong. The corrected
# doc is allowed to quote/describe the old claims when explaining the
# correction; it must never restate them as current, asserted truth.
Test-ContentDoesNotContain `
    "doc does not assert multi_symbol_config is unwired scaffolding" `
    $Content `
    "is inert scaffolding for a future" | Out-Null

Test-ContentDoesNotContain `
    "doc does not assert one global strategy is the only runtime assignment mapping" `
    $Content `
    "is currently no per-symbol strategy assignment wired" | Out-Null

Test-ContentDoesNotContain `
    "doc does not assert daily completion universally equals end_ts + 86400" `
    $Content `
    "can never be tested as ``end_ts" | Out-Null

Test-ContentDoesNotContain `
    "doc does not accept intraday continuity as PARTIAL-ready" `
    $Content `
    "is therefore **PARTIAL**" | Out-Null

Test-ContentDoesNotContain `
    "doc does not place the strict gate only after db_pool" `
    $Content `
    "is inserted immediately after the existing legacy freshness gate (after line" | Out-Null

Write-Host ""
Show-Info "--- [14] Corrected concepts are present ---"

$RequiredConcepts = @(
    "tick_strategy_dispatch_multi_symbol",
    "configured_strategy_id",
    "effective_runtime_strategy_id",
    "runtime_strategy_assignment_mismatch",
    "session-open-anchored",
    "CalendarCoverageState",
    "timeframe-aware",
    "pre-start-evidence",
    "StrategyMeta",
    "StrategyDataRequirements",
    "provider_capability_mismatch",
    "Asset-class resolution"
)
foreach ($Concept in $RequiredConcepts) {
    Test-ContentContains "doc contains corrected concept '$Concept'" $Content $Concept | Out-Null
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- DAILY-DATA-READINESS-01A audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
