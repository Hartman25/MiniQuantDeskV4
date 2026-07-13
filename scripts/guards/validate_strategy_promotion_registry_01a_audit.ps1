# =============================================================================
# STRATEGY-PROMOTION-REGISTRY-01A -- Audit/Contract Doc Validator
# validate_strategy_promotion_registry_01a_audit.ps1
#
# Validates docs/specs/strategy_promotion_registry_01a_current_truth_and_contract.md
# mentions the required patch-group identity, promotion-state vocabulary,
# and safety vocabulary before Phase B/C/D/E implementation begins, and
# does NOT contain phrasing that would authorize automatic promotion, live
# trading, wildcard identity, backfill from enabled strategies, or
# scanner-rank-only approval. No code is compiled or executed; this is a
# documentation-content check only.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_strategy_promotion_registry_01a_audit.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$AuditDoc  = Join-Path $RepoRoot 'docs\specs\strategy_promotion_registry_01a_current_truth_and_contract.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' STRATEGY-PROMOTION-REGISTRY-01A validate_strategy_promotion_registry_01a_audit'
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

$requiredTerms = @(
    'STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01',
    'shadow_approved',
    'paper_approved',
    'active_paper',
    'demoted',
    'retired',
    'rejected',
    'only `active_paper`',
    'evidence_fingerprint',
    'promotion_db_unavailable',
    'promotion_missing',
    'promotion_expired',
    'promotion_query_failed',
    'tradable_live: false',
    'config_identity_status',
    'unavailable_in_current_runtime',
    '0046',
    'sys_strategy_promotion_transitions'
)

Write-Host ''
Show-Info '--- Required terms present in audit doc ---'
foreach ($term in $requiredTerms) {
    if ($text -match [regex]::Escape($term)) {
        Show-Green "found: '$term'"
    } else {
        Show-Fail "missing required term: '$term'"
    }
}

# Forbidden-authorization phrases: presence of these (case-insensitive)
# would indicate the design accidentally grants an unsafe capability.
# Checked as substrings that should NEVER appear verbatim in the doc.
# NOTE: phrases here are deliberately specific (not generic substrings
# like "automatic promotion") so they do not collide with this doc's own
# negation sentences (e.g. "No automatic promotion: ...").
$forbiddenPhrases = @(
    'is automatically promoted',
    'automatically promotes',
    'enabled=true implies promotion',
    'enabled implies promotion',
    "symbol=`"*`"",
    "symbol = `"*`"",
    "timeframe=`"*`"",
    'backfill enabled strategies into',
    'grants live authorization',
    'authorizes live trading',
    'tradable_live: true'
)

Write-Host ''
Show-Info '--- Forbidden authorization phrases (must be absent) ---'
foreach ($phrase in $forbiddenPhrases) {
    if ($text -match [regex]::Escape($phrase)) {
        Show-Fail "forbidden phrase present: '$phrase'"
    } else {
        Show-Green "absent (safe): '$phrase'"
    }
}

# Positive safety-statement checks: the doc must explicitly assert these
# invariants somewhere, not merely avoid the forbidden phrases above.
$requiredSafetyStatements = @(
    'no automatic promotion',
    'never authorize a LIVE run',
    'No wildcards accepted',
    'no backfill'
)

Write-Host ''
Show-Info '--- Required safety statements present ---'
foreach ($stmt in $requiredSafetyStatements) {
    if ($text -match [regex]::Escape($stmt)) {
        Show-Green "found: '$stmt'"
    } else {
        Show-Fail "missing required safety statement: '$stmt'"
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
