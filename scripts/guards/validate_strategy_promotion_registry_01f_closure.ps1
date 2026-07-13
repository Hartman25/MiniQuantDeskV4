# =============================================================================
# STRATEGY-PROMOTION-REGISTRY-01F -- Closure Doc Validator
# validate_strategy_promotion_registry_01f_closure.ps1
#
# Validates docs/specs/strategy_promotion_registry_01f_closure_decision.md
# proves the required closure facts (durable enforcement, real DB-backed
# proof, no forbidden files/actions) before this patch group may be marked
# CLOSED_LOCAL, and that it does not claim closure while also admitting a
# hard safety violation. No code is compiled or executed; this is a
# documentation-content check only -- the actual test proof is run
# separately (see this patch group's VALIDATION section).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_strategy_promotion_registry_01f_closure.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$ClosureDoc = Join-Path $RepoRoot 'docs\specs\strategy_promotion_registry_01f_closure_decision.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' STRATEGY-PROMOTION-REGISTRY-01F validate_strategy_promotion_registry_01f_closure'
Write-Host " Closure doc : $ClosureDoc"
Write-Host '============================================================'

if (-not (Test-Path -LiteralPath $ClosureDoc)) {
    Show-Fail "Closure doc not found: $ClosureDoc"
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$text = [System.IO.File]::ReadAllText($ClosureDoc)
Show-Green "Closure doc found"

$requiredTerms = @(
    # STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01: corrected from
    # "...COMBINED: CLOSED_LOCAL" -- the closure doc claimed CLOSED_LOCAL
    # for the whole bundle while its own §5/§9 admitted configuration-
    # fingerprint identity binding was PARTIAL, an internally
    # inconsistent closure claim. The doc's own "Final status" banner now
    # honestly reads PARTIAL; this guard must check for the corrected
    # value, not re-assert the defect it exists to catch.
    'STRATEGY-PROMOTION-REGISTRY-AND-RUNTIME-ENFORCEMENT-01-COMBINED: PARTIAL',
    '0046_strategy_promotion_registry.sql',
    'Gate 3b',
    'Gate 2b',
    'promotion_gate::evaluate_paper_promotion_gate',
    'scenario_strategy_promotion_closure_proof_01f.rs',
    'active_paper',
    'tradable_live = false',
    'config_identity_status = "unavailable_in_current_runtime"',
    'PARTIAL',
    '0 leftover rows',
    'never trusts a caller',
    'no daemon process was started',
    'never touched'
)

Write-Host ''
Show-Info '--- Required terms present in closure doc ---'
foreach ($term in $requiredTerms) {
    if ($text -match [regex]::Escape($term)) {
        Show-Green "found: '$term'"
    } else {
        Show-Fail "missing required term: '$term'"
    }
}

# Forbidden claims: presence of any of these would mean the doc is
# claiming something the mission's hard safety rules forbid.
$forbiddenPhrases = @(
    'live order was submitted',
    'paper order was manually submitted',
    'daemon runtime was started',
    'execution was armed',
    'port 5440 was migrated',
    'paper DB was migrated',
    'broker call was made',
    'tradable_live = true',
    'tradable_live: true'
)

Write-Host ''
Show-Info '--- Forbidden closure claims (must be absent) ---'
foreach ($phrase in $forbiddenPhrases) {
    if ($text -match [regex]::Escape($phrase)) {
        Show-Fail "forbidden claim present: '$phrase'"
    } else {
        Show-Green "absent (safe): '$phrase'"
    }
}

# The doc must not claim "CLOSED_LOCAL" while also stating that the
# closure test failed. This is a lightweight self-consistency check.
if (($text -match [regex]::Escape('CLOSED_LOCAL')) -and ($text -match 'closure proof (test )?failed')) {
    Show-Fail "doc claims CLOSED_LOCAL while also stating the closure proof failed"
} else {
    Show-Green "no CLOSED_LOCAL / closure-proof-failed contradiction found"
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
