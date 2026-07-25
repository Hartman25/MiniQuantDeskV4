# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01A -- Contract Doc Validator
# validate_durable_paper_portfolio_and_pnl_01a_contract.ps1
#
# Validates docs/specs/durable_paper_portfolio_and_pnl_01a_current_truth_and_contract.md
# locks the required patch identity, truth-state vocabulary, source-authority
# distinctions, and safety vocabulary before B4-B implementation begins, and
# does NOT contain phrasing that would authorize a duplicate fill ledger,
# synthetic opening fills, or synthesized snapshots masquerading as
# authoritative. No code is compiled or executed; this is a documentation-
# content check only.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01a_contract.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$ContractDoc = Join-Path $RepoRoot 'docs\specs\durable_paper_portfolio_and_pnl_01a_current_truth_and_contract.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01A validate_..._01a_contract'
Write-Host " Contract doc : $ContractDoc"
Write-Host '============================================================'

if (-not (Test-Path -LiteralPath $ContractDoc)) {
    Show-Fail "Contract doc not found: $ContractDoc"
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$text = [System.IO.File]::ReadAllText($ContractDoc)
Show-Green "Contract doc found"

$requiredTerms = @(
    'DURABLE-PAPER-PORTFOLIO-AND-PNL-01A-CURRENT-TRUTH-AND-CONTRACT',
    'oms_inbox',
    'mqk_portfolio',
    'accept_external_broker_snapshot',
    'last_applied_inbox_id',
    'accounting_epoch',
    'sys_paper_portfolio_snapshots',
    'sys_paper_portfolio_snapshot_positions',
    'sys_paper_portfolio_accounting_state',
    'BrokerSnapshotTruthSource',
    'external_alpaca',
    'synthetic_diagnostic',
    'active',
    'not_found',
    'snapshot_unavailable',
    'snapshot_stale',
    'fill_history_incomplete',
    'accounting_epoch_unavailable',
    'mark_unavailable',
    'baseline_unavailable',
    'reconcile_blocked',
    'db_unavailable',
    'query_failed',
    'unsupported_source',
    'durable-summary',
    'durable-positions',
    'durable-snapshots',
    'account_equity_baseline',
    'No production behavior change in this patch'
)

foreach ($term in $requiredTerms) {
    if ($text -notlike "*$term*") {
        Show-Fail "Missing required term: '$term'"
    } else {
        Show-Green "Found required term: '$term'"
    }
}

$forbiddenPhrases = @(
    'new fill ledger table',
    'synthesized snapshot.*authoritative',
    'live capital',
    'Bundle 5',
    'multi-symbol'
)

foreach ($phrase in $forbiddenPhrases) {
    if ($text -match $phrase) {
        Show-Fail "Forbidden phrase pattern matched: '$phrase' -- contract must not authorize this"
    } else {
        Show-Green "No forbidden phrase match for: '$phrase'"
    }
}

# The doc must explicitly conclude no new duplicate fill table is required.
if ($text -match 'no new fill.{0,20}table is required' -or $text -match 'No new fill table') {
    Show-Green "Contract explicitly concludes no new duplicate fill ledger is required"
} else {
    Show-Fail "Contract does not explicitly conclude that no new duplicate fill ledger table is required"
}

# Must explicitly forbid fabricating opening fills to balance the ledger.
if ($text -match 'never fabricated to balance the ledger' -or $text -match 'never.{0,20}fabricat') {
    Show-Green "Contract explicitly forbids fabricating opening fills"
} else {
    Show-Fail "Contract does not explicitly forbid fabricating opening fills"
}

Write-Host ''
if ($Violations -gt 0) {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " VALIDATION PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
