# =============================================================================
# PAPER-SMOKE-FOLLOWUP-01C -- STEP 9B Single-Symbol Watchlist Gate Validator
# =============================================================================
# Pure static text validation of Start-PaperTradingSmoke.ps1's STEP 9B gate.
# No network call, no provider/broker call, no DB connection, no daemon
# start, no cargo/npm build or test. Complements (does not replace)
# tests\script_guards\test_multi_symbol_smoke_runner_gate.ps1, which proves
# the original MULTI-SYMBOL-SMOKE-RUNNER-PREFLIGHT-GATE-01 invariants are
# still intact.
#
# Checks:
#   [1]  Target script exists.
#   [2]  STEP 9B section header present.
#   [3]  -MultiSymbolSmoke switch parameter is declared.
#   [4]  Single-symbol fallback branch exists (not_configured => proceed,
#        not a failure).
#   [5]  Honest status fields are reported for the single-symbol path
#        (watchlist_v2_status, watchlist_v2_required, single_symbol_smoke_symbol).
#   [6]  Multi-symbol explicit-required branch still fails closed on
#        schema_version != watchlist-v2.
#   [7]  A watchlist-v2 artifact that is configured but invalid still fails
#        closed even when -MultiSymbolSmoke is not passed (misconfiguration
#        is not silently ignored).
#   [8]  approved_for_live hard-invariant check remains present.
#   [9]  live_routing_enabled=false check remains present in the script
#        (STEP 8 identity verification untouched).
#   [10] No .env.local mutation was added to the script.
#   [11] STEP 9B remains read-only (no Invoke-DaemonPost call inside it).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_smoke_followup_01c_watchlist_gate.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathTarget = Join-Path $RepoRoot "scripts\windows\Start-PaperTradingSmoke.ps1"

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

Write-Host "============================================================"
Write-Host " PAPER-SMOKE-FOLLOWUP-01C Watchlist Gate Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Target script exists ---"
$Content = $null
if (Test-FileExists "Start-PaperTradingSmoke.ps1" $PathTarget) {
    $Content = Get-Content -Raw -Path $PathTarget
}

Write-Host ""
Show-Info "--- [2] STEP 9B section header present ---"
Test-ContentContains "STEP 9B section header present" $Content "STEP 9B: Multi-symbol smoke preflight gate" | Out-Null

Write-Host ""
Show-Info "--- [3] -MultiSymbolSmoke switch parameter is declared ---"
Test-ContentContains "-MultiSymbolSmoke switch declared" $Content "[switch]`$MultiSymbolSmoke" | Out-Null

Write-Host ""
Show-Info "--- [4] Single-symbol fallback branch exists ---"
Test-ContentContains "single-symbol fallback branch (not_configured)" $Content "wlStatusLabel -eq 'not_configured'" | Out-Null

Write-Host ""
Show-Info "--- [5] Honest status fields reported for single-symbol path ---"
Test-ContentContains "reports watchlist_v2_status=not_configured_single_symbol_mode" $Content "watchlist_v2_status=not_configured_single_symbol_mode" | Out-Null
Test-ContentContains "reports watchlist_v2_required=false" $Content "watchlist_v2_required=false" | Out-Null
Test-ContentContains "reports single_symbol_smoke_symbol" $Content "single_symbol_smoke_symbol=" | Out-Null

Write-Host ""
Show-Info "--- [6] Multi-symbol explicit-required branch still fails closed ---"
Test-ContentContains "MultiSymbolSmoke branch present" $Content "if (`$MultiSymbolSmoke) {" | Out-Null
Test-ContentContains "schema_version != watchlist-v2 still fails" $Content "MULTI_SYMBOL_SMOKE_BLOCKED_SCHEMA_NOT_V2" | Out-Null

Write-Host ""
Show-Info "--- [7] Configured-but-invalid watchlist-v2 still fails closed in single-symbol mode ---"
Test-ContentContains "configured-but-invalid still fails closed" $Content "MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_V2_CONFIGURED_BUT_INVALID" | Out-Null

Write-Host ""
Show-Info "--- [8] approved_for_live hard-invariant check remains present ---"
Test-ContentContains "approved_for_live hard invariant retained" $Content "MULTI_SYMBOL_SMOKE_BLOCKED_APPROVED_FOR_LIVE_TRUE" | Out-Null

Write-Host ""
Show-Info "--- [9] live_routing_enabled=false check remains present ---"
Test-ContentContains "live_routing_enabled check retained" $Content "live_routing_enabled -eq `$true" | Out-Null

Write-Host ""
Show-Info "--- [10] No .env.local mutation was added ---"
if ($null -ne $Content -and ($Content -match '(?i)Set-Content[^\n]*\.env\.local' -or $Content -match '(?i)Add-Content[^\n]*\.env\.local' -or $Content -match '(?i)Out-File[^\n]*\.env\.local')) {
    $script:Violations++
    Show-Red "  FAIL -- script appears to write to .env.local"
} else {
    Show-Green "  OK -- no .env.local write pattern found"
}

Write-Host ""
Show-Info "--- [11] STEP 9B remains read-only ---"
$step9bMatch = [regex]::Match($Content, '(?s)# STEP 9B: Multi-symbol smoke preflight gate.*?(?=# STEP 10: Clear halted lifecycle if needed)')
if ($step9bMatch.Success) {
    if ($step9bMatch.Value -notmatch 'Invoke-DaemonPost') {
        Show-Green "  OK -- STEP 9B block contains no Invoke-DaemonPost calls"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- STEP 9B block calls Invoke-DaemonPost (must remain read-only)"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- could not isolate STEP 9B block (between STEP 9B and STEP 10 headers)"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-SMOKE-FOLLOWUP-01C watchlist gate is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
