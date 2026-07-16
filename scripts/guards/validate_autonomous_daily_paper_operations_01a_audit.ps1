# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01A -- Current-Truth Audit Validator
# =============================================================================
# Pure docs/text validation of the Phase A audit + binding contract doc. No
# network call, no provider/broker call, no DB connection, no daemon start,
# no cargo/npm build or test.
#
# Checks:
#   [1]  Audit/contract doc exists.
#   [2]  Doc mentions the bundle id and starting HEAD baseline.
#   [3]  Doc references the session controller source (session_controller.rs).
#   [4]  Doc documents an authoritative session/calendar contract.
#   [5]  Doc documents a deterministic durable daily-operation identity.
#   [6]  Doc documents typed retry/backoff classification.
#   [7]  Doc documents an exactly-once completed-bar dispatch contract.
#   [8]  Doc documents explicit provider-call authorization.
#   [9]  Doc documents a no-trade evidence hierarchy.
#   [10] Doc documents task supervision / liveness watchdogs.
#   [11] Doc documents the PAPER-only boundary.
#   [12] Doc keeps Bundle 2 strict daily-data readiness as a prerequisite.
#   [13] Doc excludes per-symbol bootstrap, 1h, live-capital, P&L, and soak
#        execution from this bundle's scope.
#   [14] Doc contains Phase B-G checkpoints.
#   [15] Doc does not claim any runtime/gate/GUI code was implemented in
#        Phase A, and does not claim a live/broker/provider/DB action
#        occurred as part of this patch.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01a_current_truth_and_contract.md"

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
        Show-Red "  FAIL -- $Label (forbidden needle found: '$Needle')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01A Current-Truth Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit/contract doc exists ---"
$Content = $null
if (Test-FileExists "Phase A audit/contract spec" $PathAuditSpec) {
    $Content = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Doc mentions bundle id and starting HEAD baseline ---"
Test-ContentContains "doc mentions bundle id" $Content "AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED" | Out-Null
Test-ContentContains "doc records starting HEAD baseline" $Content "ee026f5f" | Out-Null

Write-Host ""
Show-Info "--- [3] Doc references session controller source ---"
Test-ContentContains "doc references session_controller.rs" $Content "session_controller.rs" | Out-Null

Write-Host ""
Show-Info "--- [4] Doc documents authoritative session/calendar contract ---"
Test-ContentContains "doc documents MarketCalendarProvider authority" $Content "MarketCalendarProvider" | Out-Null
Test-ContentContains "doc documents the calendar duplication gap" $Content "three independent call sites" | Out-Null

Write-Host ""
Show-Info "--- [5] Doc documents deterministic durable daily-operation identity ---"
Test-ContentContains "doc documents operation_id UUIDv5 convention" $Content "operation_id" | Out-Null
Test-ContentContains "doc documents Uuid::new_v5 reuse (not new_v4)" $Content "Uuid::new_v5" | Out-Null
Test-ContentContains "doc documents sys_autonomous_daily_operations table" $Content "sys_autonomous_daily_operations" | Out-Null

Write-Host ""
Show-Info "--- [6] Doc documents typed retry/backoff classification ---"
Test-ContentContains "doc documents transient/terminal retry classification" $Content "Transient" | Out-Null
Test-ContentContains "doc documents backoff schedule" $Content "backoff" | Out-Null

Write-Host ""
Show-Info "--- [7] Doc documents exactly-once completed-bar dispatch contract ---"
Test-ContentContains "doc documents bar ticker blind-timer gap" $Content "blind" | Out-Null
Test-ContentContains "doc documents exactly-once dispatch requirement" $Content "exactly-once" | Out-Null

Write-Host ""
Show-Info "--- [8] Doc documents explicit provider-call authorization ---"
Test-ContentContains "doc documents provider-call coordinator seam gap" $Content "market_data_feed_poll_once" | Out-Null

Write-Host ""
Show-Info "--- [9] Doc documents no-trade evidence hierarchy ---"
Test-ContentContains "doc documents strategy_signal_evaluations as evidence source" $Content "strategy_signal_evaluations" | Out-Null
Test-ContentContains "doc documents evidence hierarchy" $Content "evidence hierarchy" | Out-Null

Write-Host ""
Show-Info "--- [10] Doc documents task supervision / liveness watchdogs ---"
Test-ContentContains "doc documents watchdog gap" $Content "watchdog" | Out-Null
Test-ContentContains "doc documents reconcile-tick JoinHandle discarded" $Content "JoinHandle" | Out-Null

Write-Host ""
Show-Info "--- [11] Doc documents PAPER-only boundary ---"
Test-ContentContains "doc documents PAPER-only boundary" $Content "PAPER-only" | Out-Null

Write-Host ""
Show-Info "--- [12] Doc keeps Bundle 2 strict readiness as prerequisite ---"
Test-ContentContains "doc references daily_data_readiness.rs as prerequisite" $Content "daily_data_readiness.rs" | Out-Null
Test-ContentContains "doc references DAILY-DATA-READINESS-01C-ENFORCEMENT-01 gate" $Content "DAILY-DATA-READINESS-01C-ENFORCEMENT-01" | Out-Null

Write-Host ""
Show-Info "--- [13] Doc excludes out-of-scope work ---"
Test-ContentContains "doc excludes per-symbol strategy bootstrap" $Content "per-symbol strategy bootstrap" | Out-Null
Test-ContentContains "doc excludes 1h provider support" $Content "1h" | Out-Null
Test-ContentContains "doc excludes live-capital operation" $Content "live-capital operation" | Out-Null
Test-ContentContains "doc excludes P&L calculation" $Content "P&L" | Out-Null
Test-ContentContains "doc excludes the 10-20 session soak" $Content "soak" | Out-Null
Test-ContentContains "doc names next bundle as DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED" $Content "DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED" | Out-Null

Write-Host ""
Show-Info "--- [14] Doc contains Phase B-G checkpoints ---"
$PhaseMarkers = @("Phase B", "Phase C", "Phase D", "Phase E", "Phase F", "Phase G")
foreach ($Marker in $PhaseMarkers) {
    Test-ContentContains "doc references '$Marker'" $Content $Marker | Out-Null
}

Write-Host ""
Show-Info "--- [15] No forbidden implementation/action claims ---"
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
    "daemon was started",
    "table was created",
    "migration was added"
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

Test-ContentContains "doc states Phase A makes no production-code change" $Content "makes no production-code" | Out-Null
Test-ContentContains "doc states Phase A makes no migration change" $Content "or migration change" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01A audit evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
