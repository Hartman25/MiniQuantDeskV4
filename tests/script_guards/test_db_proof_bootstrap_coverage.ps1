# =============================================================================
# CI-PROOF-COVERAGE-GUARD-01 -- DB proof bootstrap coverage guard
#
# Proves CBP01-CBP13: promoted scenario files are represented in
# scripts/db_proof_bootstrap.sh as mandatory cargo test lines, required
# --include-ignored flags are present, and live-pending scenarios are
# documented in comments but NOT promoted to cargo test lines.
#
# No daemon, no DB, no live calls. Pure static content checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_db_proof_bootstrap_coverage.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir       = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot        = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$BootstrapScript = Join-Path $RepoRoot 'scripts\db_proof_bootstrap.sh'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== DB proof bootstrap coverage guard (CI-PROOF-COVERAGE-GUARD-01) ==="
Write-Host ""

# ---------------------------------------------------------------------------
# CBP01: scripts/db_proof_bootstrap.sh exists.
# ---------------------------------------------------------------------------
Write-Host "  -- CBP01: bootstrap script exists --"

$bootstrapText  = ''
$bootstrapLines = @()

if (Test-Path $BootstrapScript) {
    Pass 'CBP01' "scripts/db_proof_bootstrap.sh exists"
    $bootstrapText  = Get-Content -Path $BootstrapScript -Raw
    $bootstrapLines = Get-Content -Path $BootstrapScript
} else {
    Fail 'CBP01' "scripts/db_proof_bootstrap.sh NOT found -- guard cannot continue"
    Write-Host ""
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------------------
# Helpers
#
# Get-CargoTestLines: non-comment lines that invoke cargo test for a scenario.
#   A "cargo test line" must not start with optional whitespace + '#', must
#   contain 'cargo' followed by 'test', and must name the scenario.
#
# Get-CommentMentionLines: comment lines that name a scenario.
#   Used to verify live-pending scenarios have an explicit deferral note.
# ---------------------------------------------------------------------------

function Get-CargoTestLines {
    param([string]$Scenario, [string[]]$Lines)
    $Lines | Where-Object {
        $_ -notmatch '^\s*#' -and
        $_ -match 'cargo\s+test' -and
        $_ -match [regex]::Escape($Scenario)
    }
}

function Get-CommentMentionLines {
    param([string]$Scenario, [string[]]$Lines)
    $Lines | Where-Object {
        $_ -match '^\s*#' -and
        $_ -match [regex]::Escape($Scenario)
    }
}

# ---------------------------------------------------------------------------
# CBP02-CBP05: Promoted scenarios appear as cargo test lines.
#
# These four scenarios were promoted to the mandatory DB proof lane in
# CI-DBPROOF-NEWSCENARIOS-01. Each must have an active cargo test invocation
# in db_proof_bootstrap.sh (not only a comment).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CBP02-CBP05: promoted scenarios present as cargo test lines --"

$PromotedScenarios = @(
    'scenario_signal_to_outbox_unit_proof_01',
    'scenario_durable_arm_after_baseline_adoption_01',
    'scenario_broker_position_baseline_adoption_01',
    'scenario_deadman_after_start_01'
)
$PromotedIds = @('CBP02', 'CBP03', 'CBP04', 'CBP05')

for ($i = 0; $i -lt $PromotedScenarios.Count; $i++) {
    $scenario = $PromotedScenarios[$i]
    $id       = $PromotedIds[$i]
    $hits     = Get-CargoTestLines -Scenario $scenario -Lines $bootstrapLines
    if ($hits) {
        Pass $id "$scenario is present as a cargo test line"
    } else {
        Fail $id "$scenario is NOT present as a cargo test line in db_proof_bootstrap.sh -- add it or explicitly defer with a reason comment"
    }
}

# ---------------------------------------------------------------------------
# CBP06-CBP07: Required --include-ignored flags are present.
#
# scenario_signal_to_outbox_unit_proof_01: mixed pure + DB-backed (#[ignore])
# tests. Without --include-ignored the DB-backed tests are silently skipped.
#
# scenario_deadman_after_start_01: all five tests are #[ignore] DB-backed.
# Without --include-ignored none of the DAS01-DAS05 proofs run.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CBP06-CBP07: --include-ignored flags on required scenarios --"

$IgnoredRequired = @(
    'scenario_signal_to_outbox_unit_proof_01',
    'scenario_deadman_after_start_01'
)
$IgnoredIds = @('CBP06', 'CBP07')

for ($i = 0; $i -lt $IgnoredRequired.Count; $i++) {
    $scenario = $IgnoredRequired[$i]
    $id       = $IgnoredIds[$i]
    $hits     = Get-CargoTestLines -Scenario $scenario -Lines $bootstrapLines |
                Where-Object { $_ -match '--include-ignored' }
    if ($hits) {
        Pass $id "$scenario cargo test line uses --include-ignored"
    } else {
        Fail $id "$scenario cargo test line is missing --include-ignored -- DB-backed tests will be silently skipped without it"
    }
}

# ---------------------------------------------------------------------------
# CBP08-CBP10, CBP14: Live-pending scenarios are NOT promoted to cargo test lines.
#
# CBP08-CBP10: fill-dedup / fast-fill chain -- depends on fill-dedup live smoke proof.
# CBP14: terminal-fill settle window -- depends on RECONCILE-DRIFT-AFTER-TERMINAL-FILL-01 live smoke.
# None may appear as cargo test invocations until the respective live smoke proof is complete.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CBP08-CBP10 CBP14: live-pending scenarios not promoted to cargo test lines --"

$LivePendingScenarios = @(
    'scenario_recovery_quarantine_after_ack_fill_01',
    'scenario_reconcile_drift_after_fast_fill_01',
    'scenario_fill_dedup_ws_rest_precision_01',
    'scenario_reconcile_after_terminal_fill_01'
)
$LivePendingIds = @('CBP08', 'CBP09', 'CBP10', 'CBP14')

for ($i = 0; $i -lt $LivePendingScenarios.Count; $i++) {
    $scenario = $LivePendingScenarios[$i]
    $id       = $LivePendingIds[$i]
    $hits     = Get-CargoTestLines -Scenario $scenario -Lines $bootstrapLines
    if (-not $hits) {
        Pass $id "$scenario is not a cargo test line (held as live-pending)"
    } else {
        Fail $id "$scenario appears as a cargo test line -- it is live-pending and must not be promoted until the live smoke proof is complete"
    }
}

# ---------------------------------------------------------------------------
# CBP11-CBP13, CBP15: Live-pending scenarios ARE documented in comments.
#
# A scenario that is absent from db_proof_bootstrap.sh with no comment is an
# invisible coverage gap. Each live-pending scenario must appear by name in at
# least one comment line so the deferral is explicit and reviewable.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CBP11-CBP13 CBP15: live-pending scenarios documented in comments --"

$DeferIds = @('CBP11', 'CBP12', 'CBP13', 'CBP15')

for ($i = 0; $i -lt $LivePendingScenarios.Count; $i++) {
    $scenario = $LivePendingScenarios[$i]
    $id       = $DeferIds[$i]
    $hits     = Get-CommentMentionLines -Scenario $scenario -Lines $bootstrapLines
    if ($hits) {
        Pass $id "$scenario is named in a comment in db_proof_bootstrap.sh (deferral is explicit)"
    } else {
        Fail $id "$scenario is absent from db_proof_bootstrap.sh with no deferral comment -- add a comment explaining why it is not yet promoted"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL CBP INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
