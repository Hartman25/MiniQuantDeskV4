# =============================================================================
# STRATEGY-SCANNER-JOBS-GUI-01A -- Audit Doc Validator
# validate_strategy_scanner_jobs_gui_01a_audit.ps1
#
# Validates docs/specs/strategy_scanner_jobs_gui_01a_current_truth_audit.md
# mentions the required patch-group identity, artifact schema, and safety
# vocabulary before Phase B/C/D implementation begins. No code is compiled
# or executed; this is a documentation-content check only.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_strategy_scanner_jobs_gui_01a_audit.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$AuditDoc  = Join-Path $RepoRoot 'docs\specs\strategy_scanner_jobs_gui_01a_current_truth_audit.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' STRATEGY-SCANNER-JOBS-GUI-01A validate_strategy_scanner_jobs_gui_01a_audit'
Write-Host " Audit doc : $AuditDoc"
Write-Host '============================================================'

if (-not (Test-Path -LiteralPath $AuditDoc)) {
    Show-Fail "Audit doc not found: $AuditDoc"
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$text = [System.IO.File]::ReadAllText($AuditDoc)
Show-Green "Audit doc found"

$required = @(
    'STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01',
    'scan-strategies',
    'strategy_scans',
    'manifest.json',
    'candidates.json',
    'summary.json',
    'research evidence only',
    'no autonomous trading approval',
    'no provider calls',
    'no broker calls',
    'no live orders',
    'no forced paper orders',
    'no strategy threshold changes',
    'no DB migration'
)

Write-Host ''
Show-Info '--- Required terms present in audit doc ---'
foreach ($term in $required) {
    if ($text -match [regex]::Escape($term)) {
        Show-Green "found: '$term'"
    } else {
        Show-Fail "missing required term: '$term'"
    }
}

Write-Host ''
Write-Host '============================================================'
Write-Host ' Summary'
Write-Host '============================================================'

if ($Violations -eq 0) {
    Write-Host ' ALL CHECKS PASSED' -ForegroundColor Green
    exit 0
} else {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}
