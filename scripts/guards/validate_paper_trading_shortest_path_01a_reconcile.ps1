# =============================================================================
# PAPER-TRADING-SHORTEST-PATH-01A -- Turnover / Ledger Reconcile Validator
# =============================================================================
# Pure docs/text validation of the paper-trading shortest-path turnover
# reconcile doc. No network call, no provider/broker call, no DB connection,
# no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1]  Reconcile doc exists.
#   [2]  Reconcile doc mentions PAPER-TRADING-SHORTEST-PATH-01.
#   [3]  Reconcile doc mentions Vertus as watchlist only.
#   [4]  Reconcile doc mentions AI research analyst as future only.
#   [5]  Reconcile doc mentions random baseline.
#   [6]  Reconcile doc mentions MAE/MFE.
#   [7]  Reconcile doc mentions exit policy.
#   [8]  Reconcile doc mentions no new strategies now.
#   [9]  Reconcile doc mentions no live orders.
#   [10] Reconcile doc mentions no paper orders.
#   [11] Reconcile doc states no trading behavior changed.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_trading_shortest_path_01a_reconcile.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathReconcile = Join-Path $RepoRoot "docs\specs\paper_trading_shortest_path_01a_turnover_ledger_reconcile.md"

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
Write-Host " PAPER-TRADING-SHORTEST-PATH-01A Reconcile Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Reconcile doc exists ---"
$ReconcileContent = $null
if (Test-FileExists "Paper trading shortest path 01A reconcile doc" $PathReconcile) {
    $ReconcileContent = Get-Content -Raw -Path $PathReconcile
}

Write-Host ""
Show-Info "--- [2] Reconcile doc mentions PAPER-TRADING-SHORTEST-PATH-01 ---"
Test-ContentContains "reconcile doc mentions PAPER-TRADING-SHORTEST-PATH-01" $ReconcileContent "PAPER-TRADING-SHORTEST-PATH-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Reconcile doc mentions Vertus as watchlist only ---"
Test-ContentContains "reconcile doc mentions Vertus" $ReconcileContent "Vertus" | Out-Null
Test-ContentContains "reconcile doc mentions watchlist" $ReconcileContent "watchlist" | Out-Null

Write-Host ""
Show-Info "--- [4] Reconcile doc mentions AI research analyst as future only ---"
Test-ContentContains "reconcile doc mentions AI research analyst" $ReconcileContent "AI research analyst" | Out-Null

Write-Host ""
Show-Info "--- [5] Reconcile doc mentions random baseline ---"
Test-ContentContains "reconcile doc mentions random baseline" $ReconcileContent "random baseline" | Out-Null

Write-Host ""
Show-Info "--- [6] Reconcile doc mentions MAE/MFE ---"
Test-ContentContains "reconcile doc mentions MAE/MFE" $ReconcileContent "MAE/MFE" | Out-Null

Write-Host ""
Show-Info "--- [7] Reconcile doc mentions exit policy ---"
Test-ContentContains "reconcile doc mentions exit policy" $ReconcileContent "exit-policy" | Out-Null

Write-Host ""
Show-Info "--- [8] Reconcile doc mentions no new strategies now ---"
Test-ContentContains "reconcile doc mentions no new strategies" $ReconcileContent "no new strategy" | Out-Null

Write-Host ""
Show-Info "--- [9] Reconcile doc mentions no live orders ---"
Test-ContentContains "reconcile doc mentions no live orders" $ReconcileContent "No live orders" | Out-Null

Write-Host ""
Show-Info "--- [10] Reconcile doc mentions no paper orders ---"
Test-ContentContains "reconcile doc mentions no paper orders" $ReconcileContent "No paper orders" | Out-Null

Write-Host ""
Show-Info "--- [11] Reconcile doc states no trading behavior changed ---"
Test-ContentContains "reconcile doc states no trading behavior changed" $ReconcileContent "No trading behavior changed" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-TRADING-SHORTEST-PATH-01A reconcile is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
