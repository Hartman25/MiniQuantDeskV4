# =============================================================================
# PAPER-SMOKE-FOLLOWUP-01D -- Intraday Refresh Loop Smoke Support Validator
# =============================================================================
# Pure static text validation. No network call, no provider/broker call, no
# DB connection, no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1]  Target script (Start-PaperTradingSmoke.ps1) exists.
#   [2]  -StartIntradayRefreshLoop switch parameter is declared.
#   [3]  -RequireIntradayRefresh switch parameter is declared.
#   [4]  STEP 8C (refresh loop launch) section exists.
#   [5]  STEP 14C (refresh readiness gate) section exists.
#   [6]  Refresh-loop launch is gated behind -StartIntradayRefreshLoop (not
#        unconditional) -- proves default behavior adds no network activity.
#   [7]  Refresh-readiness gate is gated behind -RequireIntradayRefresh (not
#        unconditional).
#   [8]  STEP 14C polls the existing intraday-refresh status route.
#   [9]  STEP 14C fails closed on truth_state != active, stale evidence, and
#        all_passed != true.
#   [10] No .env.local mutation was added.
#   [11] Runbook docs/runbooks/intraday_market_data_refresh.md exists and
#        documents both flags.
#   [12] market_hours_proof_sweep_01.md validator still passes (regression
#        check on the Phase B fix).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_smoke_followup_01d_intraday_refresh.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathTarget  = Join-Path $RepoRoot "scripts\windows\Start-PaperTradingSmoke.ps1"
$PathRunbook = Join-Path $RepoRoot "docs\runbooks\intraday_market_data_refresh.md"
$PathMhpsGuard = Join-Path $RepoRoot "scripts\guards\validate_market_hours_proof_sweep_01.ps1"

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
Write-Host " PAPER-SMOKE-FOLLOWUP-01D Intraday Refresh Support Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Target script exists ---"
$Content = $null
if (Test-FileExists "Start-PaperTradingSmoke.ps1" $PathTarget) {
    $Content = Get-Content -Raw -Path $PathTarget
}

Write-Host ""
Show-Info "--- [2] -StartIntradayRefreshLoop switch parameter is declared ---"
Test-ContentContains "-StartIntradayRefreshLoop switch declared" $Content "[switch]`$StartIntradayRefreshLoop" | Out-Null

Write-Host ""
Show-Info "--- [3] -RequireIntradayRefresh switch parameter is declared ---"
Test-ContentContains "-RequireIntradayRefresh switch declared" $Content "[switch]`$RequireIntradayRefresh" | Out-Null

Write-Host ""
Show-Info "--- [4] STEP 8C (refresh loop launch) section exists ---"
Test-ContentContains "STEP 8C section present" $Content "STEP 8C: Intraday refresh loop" | Out-Null

Write-Host ""
Show-Info "--- [5] STEP 14C (refresh readiness gate) section exists ---"
Test-ContentContains "STEP 14C section present" $Content "STEP 14C: Intraday refresh readiness" | Out-Null

Write-Host ""
Show-Info "--- [6] Refresh-loop launch gated behind -StartIntradayRefreshLoop ---"
$step8cMatch = [regex]::Match($Content, '(?s)# STEP 8C: Intraday refresh loop.*?(?=# STEP 9: Verify Alpaca WS live)')
if ($step8cMatch.Success -and $step8cMatch.Value -match [regex]::Escape('if ($StartIntradayRefreshLoop) {') -and $step8cMatch.Value -match [regex]::Escape('Start-Process')) {
    Show-Green "  OK -- Start-Process call is inside an 'if (`$StartIntradayRefreshLoop)' guard"
} else {
    $script:Violations++
    Show-Red "  FAIL -- could not confirm Start-Process is gated behind -StartIntradayRefreshLoop"
}

Write-Host ""
Show-Info "--- [7] Refresh-readiness gate gated behind -RequireIntradayRefresh ---"
$step14cMatch = [regex]::Match($Content, '(?s)# STEP 14C: Intraday refresh readiness.*?(?=# STEP 15: Start runtime)')
if ($step14cMatch.Success -and $step14cMatch.Value -match [regex]::Escape('if ($RequireIntradayRefresh) {')) {
    Show-Green "  OK -- readiness gate is inside an 'if (`$RequireIntradayRefresh)' guard"
} else {
    $script:Violations++
    Show-Red "  FAIL -- could not confirm readiness gate is gated behind -RequireIntradayRefresh"
}

Write-Host ""
Show-Info "--- [8] STEP 14C polls the existing intraday-refresh status route ---"
if ($step14cMatch.Success -and $step14cMatch.Value -match [regex]::Escape("/api/v1/market-data/intraday-refresh/status")) {
    Show-Green "  OK -- STEP 14C calls GET /api/v1/market-data/intraday-refresh/status"
} else {
    $script:Violations++
    Show-Red "  FAIL -- STEP 14C must call GET /api/v1/market-data/intraday-refresh/status"
}

Write-Host ""
Show-Info "--- [9] STEP 14C fails closed on truth_state, staleness, and all_passed ---"
if ($step14cMatch.Success) {
    $step14cText = $step14cMatch.Value
    if ($step14cText -match [regex]::Escape("INTRADAY_REFRESH_BLOCKED_TRUTH_STATE") -and
        $step14cText -match [regex]::Escape("INTRADAY_REFRESH_BLOCKED_STALE_EVIDENCE") -and
        $step14cText -match [regex]::Escape("INTRADAY_REFRESH_BLOCKED_NOT_ALL_PASSED")) {
        Show-Green "  OK -- all three fail-closed blocker codes present in STEP 14C"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- STEP 14C must contain INTRADAY_REFRESH_BLOCKED_TRUTH_STATE / _STALE_EVIDENCE / _NOT_ALL_PASSED"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- could not isolate STEP 14C block"
}

Write-Host ""
Show-Info "--- [10] No .env.local mutation was added ---"
if ($null -ne $Content -and ($Content -match '(?i)Set-Content[^\n]*\.env\.local' -or $Content -match '(?i)Add-Content[^\n]*\.env\.local' -or $Content -match '(?i)Out-File[^\n]*\.env\.local')) {
    $script:Violations++
    Show-Red "  FAIL -- script appears to write to .env.local"
} else {
    Show-Green "  OK -- no .env.local write pattern found"
}

Write-Host ""
Show-Info "--- [11] Intraday refresh runbook exists and documents both flags ---"
$RunbookContent = $null
if (Test-FileExists "intraday_market_data_refresh.md runbook" $PathRunbook) {
    $RunbookContent = Get-Content -Raw -Path $PathRunbook
}
Test-ContentContains "runbook documents -StartIntradayRefreshLoop" $RunbookContent "StartIntradayRefreshLoop" | Out-Null
Test-ContentContains "runbook documents -RequireIntradayRefresh" $RunbookContent "RequireIntradayRefresh" | Out-Null

Write-Host ""
Show-Info "--- [12] market_hours_proof_sweep_01.md validator still passes (regression check) ---"
if (Test-Path $PathMhpsGuard) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File $PathMhpsGuard | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Show-Green "  OK -- validate_market_hours_proof_sweep_01.ps1 still passes"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- validate_market_hours_proof_sweep_01.ps1 regressed (exit $LASTEXITCODE)"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- validate_market_hours_proof_sweep_01.ps1 not found"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-SMOKE-FOLLOWUP-01D intraday refresh support is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
