# =============================================================================
# PAPER-SMOKE-FOLLOWUP-01A -- Current Findings Audit Validator
# =============================================================================
# Pure docs/text validation of the paper-smoke followup findings audit.
# No network call, no provider/broker call, no DB connection, no daemon
# start, no cargo/npm build or test.
#
# Checks:
#   [1]  Audit doc exists.
#   [2]  Audit doc mentions PAPER-SMOKE-FOLLOWUP-01.
#   [3]  Audit doc mentions STEP 9B.
#   [4]  Audit doc mentions watchlist-v2.
#   [5]  Audit doc mentions single-symbol.
#   [6]  Audit doc mentions intraday refresh.
#   [7]  Audit doc states no live orders.
#   [8]  Audit doc states no forced paper orders.
#   [9]  Audit doc states no daemon safety gate is weakened.
#   [10] Audit doc states generated evidence is not staged.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_paper_smoke_followup_01a_audit.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAudit = Join-Path $RepoRoot "docs\specs\paper_smoke_followup_01a_current_findings_audit.md"

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
Write-Host " PAPER-SMOKE-FOLLOWUP-01A Findings Audit Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Audit doc exists ---"
$AuditContent = $null
if (Test-FileExists "Paper smoke followup 01A audit doc" $PathAudit) {
    $AuditContent = Get-Content -Raw -Path $PathAudit
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions PAPER-SMOKE-FOLLOWUP-01 ---"
Test-ContentContains "audit doc mentions PAPER-SMOKE-FOLLOWUP-01" $AuditContent "PAPER-SMOKE-FOLLOWUP-01" | Out-Null

Write-Host ""
Show-Info "--- [3] Audit doc mentions STEP 9B ---"
Test-ContentContains "audit doc mentions STEP 9B" $AuditContent "STEP 9B" | Out-Null

Write-Host ""
Show-Info "--- [4] Audit doc mentions watchlist-v2 ---"
Test-ContentContains "audit doc mentions watchlist-v2" $AuditContent "watchlist-v2" | Out-Null

Write-Host ""
Show-Info "--- [5] Audit doc mentions single-symbol ---"
Test-ContentContains "audit doc mentions single-symbol" $AuditContent "single-symbol" | Out-Null

Write-Host ""
Show-Info "--- [6] Audit doc mentions intraday refresh ---"
Test-ContentContains "audit doc mentions intraday refresh" $AuditContent "intraday refresh" | Out-Null

Write-Host ""
Show-Info "--- [7] Audit doc states no live orders ---"
Test-ContentContains "audit doc states no live orders" $AuditContent "No live orders" | Out-Null

Write-Host ""
Show-Info "--- [8] Audit doc states no forced paper orders ---"
Test-ContentContains "audit doc states no forced paper orders" $AuditContent "No forced paper orders" | Out-Null

Write-Host ""
Show-Info "--- [9] Audit doc states no daemon safety gate is weakened ---"
Test-ContentContains "audit doc states no daemon safety gate weakened" $AuditContent "No gate weakening" | Out-Null

Write-Host ""
Show-Info "--- [10] Audit doc states generated evidence is not staged ---"
Test-ContentContains "audit doc states generated evidence not staged" $AuditContent "No generated evidence" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- PAPER-SMOKE-FOLLOWUP-01A audit is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
