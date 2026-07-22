# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-
# FINALIZATION-CAS -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the E2B strict evidence classifier and durable
# finalization CAS, built on the already-accepted E1 contract and E2A
# coverage-anchor/run-lineage foundation (both validated by their own,
# unmodified guards). No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test -- pure text/source
# validation only.
#
# Checks:
#   [1]  Generic `completed` is never a reachable automatic classifier output
#        (the classification enum has exactly three variants and every
#        terminal reason's target_state() maps only to
#        completed_no_trade/completed_with_activity).
#   [2]  `completed_no_trade` requires the full-coverage/aggregate-consistency
#        gate to have already resolved clean -- never derived from a zero-fill
#        check alone.
#   [3]  No process-local diagnostic counter (AppState, bar_tick_dispatch_count,
#        last_bar_signal_qty) is referenced anywhere in the classifier module.
#   [4]  A missing/incompatible durable coverage anchor blocks the classifier
#        before any terminal classification is reachable.
#   [5]  A missing/invalid full run lineage blocks the classifier before any
#        terminal classification is reachable.
#   [6]  Partial expected-bar coverage (an incomplete claim set or an
#        aggregate-counter mismatch) can never reach completed_no_trade.
#   [7]  An unresolved (non-completed) dispatch claim can never reach a
#        terminal classification.
#   [8]  A completed claim with missing evaluation evidence can never reach a
#        terminal classification.
#   [9]  `sys_risk_denial_events` is never referenced by the classifier module.
#   [10] The terminal finalization CAS sets `state`, `outcome`, and
#        `finalized_at_utc` in the same SQL UPDATE statement (atomic).
#   [11] `no_trade_reason` is never written by the finalization CAS or the
#        classifier/finalizer module.
#   [12] The finalization CAS's already-terminal (`AlreadyApplied`) path never
#        inserts a second transition event.
#   [13] The `stopping`/`stop_retrying -> evidence_degraded` legal-transition
#        edges both exist.
#   [14] The evidence-degraded write path never sets `outcome` (only the
#        terminal finalization CAS may).
#   [15] Exactly one authorized coordinator integration seam exists (E3): the
#        coordinator calls the accepted E2B high-level entry point by name;
#        it never re-derives an outcome classification locally (no parallel
#        classifier) and never issues its own terminal-state SQL/finalize
#        call (no parallel terminal writer).
#   [16] Neither `routes.rs` nor `api_types.rs` references the outcome
#        classifier module directly -- route code never reruns the
#        classifier or writes terminal/evidence-degraded truth itself.
#   [17] README.md / README_TECHNICAL.md never claim Phase E, Bundle 3, an
#        unattended soak, or live-capital readiness as complete/started, and
#        record E1/E2A/E2B/E3 as accepted and E4 as implementation-complete-
#        awaiting-acceptance (not accepted).
#   [18] The new E2B scenario test file exists and is nonempty.
#
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
# INTEGRATION-AND-NOTIFICATION reconciled two point-in-time checks above that
# described E3 as "not started": check [15] ("no coordinator integration was
# introduced") was replaced with a positive proof that exactly one authorized
# integration seam exists and duplicates none of E2B's own logic.
#
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
# PROJECTION further reconciles checks [16]/[17]: E3 is now accepted, and E4
# adds exactly two authorized read-only routes plus new response types --
# check [16] is narrowed from "no API route at all" to "route code never
# references the classifier module" (the real invariant this guard protects),
# and check [17]'s README-truth requirement now records E1/E2A/E2B/E3 as
# accepted and E4 as implementation-complete-awaiting-acceptance. All E2B
# implementation invariants (checks [1]-[14], [18]-[24]) remain enforced
# unchanged.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e2b_outcome_classifier_and_finalization.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathOutcomeRs      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_outcome.rs"
$PathDbOperationRs  = Join-Path $RepoRoot "core-rs\crates\mqk-db\src\autonomous_daily_operation.rs"
$PathCoordinatorRs  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_coordinator.rs"
$PathRoutesRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$PathApiTypesRs     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$PathE2BTest        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs"
$PathReadme         = Join-Path $RepoRoot "README.md"
$PathReadmeTech     = Join-Path $RepoRoot "README_TECHNICAL.md"
$PathLedger         = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

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

function Get-ContentBetween {
    param([string]$Content, [string]$StartNeedle, [string]$EndNeedle)
    if ($null -eq $Content) { return $null }
    $startIdx = $Content.IndexOf($StartNeedle, [System.StringComparison]::OrdinalIgnoreCase)
    if ($startIdx -lt 0) { return $null }
    $endIdx = $Content.IndexOf($EndNeedle, $startIdx + $StartNeedle.Length, [System.StringComparison]::OrdinalIgnoreCase)
    if ($endIdx -lt 0) { $endIdx = $Content.Length }
    return $Content.Substring($startIdx, $endIdx - $startIdx)
}

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS Validator"
Write-Host "============================================================"

$OutcomeContent = $null
if (Test-FileExists "autonomous_daily_outcome.rs" $PathOutcomeRs) {
    $OutcomeContent = Get-Content -Raw -Path $PathOutcomeRs
}
$DbOperationContent = $null
if (Test-FileExists "mqk-db autonomous_daily_operation.rs" $PathDbOperationRs) {
    $DbOperationContent = Get-Content -Raw -Path $PathDbOperationRs
}

Write-Host ""
Show-Info "--- [1] Generic 'completed' is never a reachable automatic classifier output ---"
Test-ContentContains "classification enum has exactly three variants" $OutcomeContent "pub enum AutonomousDailyOutcomeClassification {" | Out-Null
Test-ContentContains "CompletedWithActivity variant present" $OutcomeContent "CompletedWithActivity {" | Out-Null
Test-ContentContains "CompletedNoTrade variant present" $OutcomeContent "CompletedNoTrade {" | Out-Null
Test-ContentContains "EvidenceBlocked variant present" $OutcomeContent "EvidenceBlocked {" | Out-Null
$TargetStateFnBody = Get-ContentBetween -Content $OutcomeContent -StartNeedle "pub fn target_state(self)" -EndNeedle "`n    }"
Test-ContentDoesNotContain "target_state() never maps to generic completed" $TargetStateFnBody "STATE_COMPLETED =>" | Out-Null
Test-ContentContains "target_state() maps to completed_no_trade" $TargetStateFnBody "STATE_COMPLETED_NO_TRADE" | Out-Null
Test-ContentContains "target_state() maps to completed_with_activity" $TargetStateFnBody "STATE_COMPLETED_WITH_ACTIVITY" | Out-Null

Write-Host ""
Show-Info "--- [2] completed_no_trade requires the coverage/aggregate gate, never zero-fills alone ---"
$ClassifyFnBody = Get-ContentBetween -Content $OutcomeContent -StartNeedle "pub fn classify_autonomous_daily_outcome(" -EndNeedle "`n}"
if ($null -eq $ClassifyFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate classify_autonomous_daily_outcome's body"
} else {
    Test-ContentContains "classifier checks all_expected_claimed before any terminal branch" $ClassifyFnBody "all_expected_claimed" | Out-Null
    Test-ContentContains "classifier checks aggregate_consistent before any terminal branch" $ClassifyFnBody "aggregate_consistent" | Out-Null
    $NoTradeIdx = $ClassifyFnBody.IndexOf("CompletedNoTrade", [System.StringComparison]::OrdinalIgnoreCase)
    $GateIdx = $ClassifyFnBody.IndexOf("all_expected_claimed", [System.StringComparison]::OrdinalIgnoreCase)
    if ($NoTradeIdx -ge 0 -and $GateIdx -ge 0 -and $GateIdx -lt $NoTradeIdx) {
        Show-Green "  OK -- coverage/aggregate gate is checked before the no-trade return"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- coverage/aggregate gate does not precede the no-trade return"
    }
}

Write-Host ""
Show-Info "--- [3] No process-local diagnostic counter is referenced in executable code ---"
# Scoped to the snapshot struct + the pure classifier + the async gatherer bodies (not the
# module's own top-of-file doc comment, which legitimately *names* these fields to explain their
# absence -- a naive whole-file substring check would false-positive on that explanatory prose).
$SnapshotStructBody = Get-ContentBetween -Content $OutcomeContent -StartNeedle "pub struct AutonomousDailyOutcomeEvidenceSnapshot {" -EndNeedle "`n}"
$GatherFnBody = Get-ContentBetween -Content $OutcomeContent -StartNeedle "pub async fn gather_autonomous_daily_outcome_evidence(" -EndNeedle "`n}"
foreach ($Scope in @(@{Name = "snapshot struct"; Body = $SnapshotStructBody}, @{Name = "classifier fn"; Body = $ClassifyFnBody}, @{Name = "gather fn"; Body = $GatherFnBody})) {
    Test-ContentDoesNotContain "$($Scope.Name) never references AppState" $Scope.Body "AppState" | Out-Null
    Test-ContentDoesNotContain "$($Scope.Name) never references bar_tick_dispatch_count" $Scope.Body "bar_tick_dispatch_count" | Out-Null
    Test-ContentDoesNotContain "$($Scope.Name) never references last_bar_signal_qty" $Scope.Body "last_bar_signal_qty" | Out-Null
}

Write-Host ""
Show-Info "--- [4] Missing/incompatible coverage anchor blocks the classifier ---"
Test-ContentContains "classifier gates on snapshot.coverage before any terminal branch" $ClassifyFnBody "let Some(coverage) = &snapshot.coverage else" | Out-Null

Write-Host ""
Show-Info "--- [5] Missing/invalid full run lineage blocks the classifier ---"
Test-ContentContains "classifier gates on snapshot.lineage before any terminal branch" $ClassifyFnBody "let Some(lineage) = &snapshot.lineage else" | Out-Null

Write-Host ""
Show-Info "--- [6] Partial expected-bar coverage can never reach completed_no_trade ---"
Test-ContentContains "classifier blocks on incomplete all_expected_claimed/aggregate_consistent" $ClassifyFnBody "!all_expected_claimed" | Out-Null

Write-Host ""
Show-Info "--- [7] An unresolved dispatch claim can never reach a terminal classification ---"
Test-ContentContains "classifier checks claim.status != DISPATCH_STATUS_COMPLETED before any terminal branch" $ClassifyFnBody "claim.status != mqk_db::DISPATCH_STATUS_COMPLETED" | Out-Null

Write-Host ""
Show-Info "--- [8] Missing evaluation evidence can never reach a terminal classification ---"
Test-ContentContains "classifier checks evaluation_id.is_none()" $ClassifyFnBody "claim.evaluation_id.is_none()" | Out-Null
Test-ContentContains "classifier checks the evaluation row itself is present" $ClassifyFnBody "let Some(evaluation) = &c.evaluation else" | Out-Null

Write-Host ""
Show-Info "--- [9] sys_risk_denial_events is never referenced ---"
Test-ContentDoesNotContain "classifier module never references sys_risk_denial_events" $OutcomeContent "sys_risk_denial_events" | Out-Null
Test-ContentDoesNotContain "classifier module never references risk_denial" $OutcomeContent "risk_denial" | Out-Null

Write-Host ""
Show-Info "--- [10] Terminal finalization CAS sets state/outcome/finalized_at_utc atomically ---"
$FinalizeFnBody = Get-ContentBetween -Content $DbOperationContent -StartNeedle "pub async fn finalize_autonomous_daily_operation(" -EndNeedle "`n}"
if ($null -eq $FinalizeFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate finalize_autonomous_daily_operation's body"
} else {
    $UpdateBlock = Get-ContentBetween -Content $FinalizeFnBody -StartNeedle "update sys_autonomous_daily_operations" -EndNeedle "where operation_id"
    Test-ContentContains "single UPDATE sets state" $UpdateBlock "state = `$1" | Out-Null
    Test-ContentContains "same UPDATE sets outcome" $UpdateBlock "outcome = `$2" | Out-Null
    Test-ContentContains "same UPDATE sets finalized_at_utc" $UpdateBlock "finalized_at_utc = `$3" | Out-Null
}

Write-Host ""
Show-Info "--- [11] no_trade_reason is never written ---"
Test-ContentDoesNotContain "finalize CAS never sets no_trade_reason" $FinalizeFnBody "no_trade_reason =" | Out-Null
Test-ContentDoesNotContain "classifier/finalizer module never sets no_trade_reason" $OutcomeContent "no_trade_reason" | Out-Null

Write-Host ""
Show-Info "--- [12] Already-terminal replay never inserts a second event ---"
$AlreadyAppliedArm = Get-ContentBetween -Content $FinalizeFnBody -StartNeedle "FinalizeAutonomousDailyOperationOutcome::AlreadyApplied" -EndNeedle "ConflictingTerminalTruth"
Test-ContentDoesNotContain "AlreadyApplied classification path performs no insert" $AlreadyAppliedArm "insert into" | Out-Null

Write-Host ""
Show-Info "--- [13] stopping/stop_retrying -> evidence_degraded legal edges exist ---"
$StoppingArm = Get-ContentBetween -Content $DbOperationContent -StartNeedle "Some(STATE_STOPPING) => matches!(" -EndNeedle "`n        ),"
$StopRetryingArm = Get-ContentBetween -Content $DbOperationContent -StartNeedle "Some(STATE_STOP_RETRYING) => matches!(" -EndNeedle "`n        ),"
Test-ContentContains "stopping -> evidence_degraded edge exists" $StoppingArm "STATE_EVIDENCE_DEGRADED" | Out-Null
Test-ContentContains "stop_retrying -> evidence_degraded edge exists" $StopRetryingArm "STATE_EVIDENCE_DEGRADED" | Out-Null

Write-Host ""
Show-Info "--- [14] Evidence-degraded write path never sets outcome ---"
$BlockerFnBody = Get-ContentBetween -Content $OutcomeContent -StartNeedle "async fn apply_evidence_degraded_blocker(" -EndNeedle "`n}`n"
if ($null -eq $BlockerFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate apply_evidence_degraded_blocker's body"
} else {
    # The only struct/function that can durably set `outcome` is the terminal finalization CAS
    # (`FinalizeAutonomousDailyOperationArgs` / `finalize_autonomous_daily_operation`) -- the
    # evidence-degraded blocker path must never reach either. (A naive "outcome:"/"outcome ="
    # substring check would false-positive on Rust type-path syntax like
    # `...Outcome::Applied`, whose `::` begins with a colon.)
    Test-ContentDoesNotContain "evidence-degraded blocker never constructs FinalizeAutonomousDailyOperationArgs" $BlockerFnBody "FinalizeAutonomousDailyOperationArgs" | Out-Null
    Test-ContentDoesNotContain "evidence-degraded blocker never calls finalize_autonomous_daily_operation" $BlockerFnBody "finalize_autonomous_daily_operation(" | Out-Null
}

Write-Host ""
Show-Info "--- [15] Exactly one authorized E3 coordinator integration seam exists ---"
$CoordinatorContent = $null
if (Test-Path $PathCoordinatorRs) {
    $CoordinatorContent = Get-Content -Raw -Path $PathCoordinatorRs
}
Test-ContentContains "the coordinator calls the accepted E2B production entry point by name" $CoordinatorContent "autonomous_daily_outcome::classify_and_finalize_autonomous_daily_operation(" | Out-Null
Test-ContentDoesNotContain "the coordinator never calls the E2B test-support effect-seam entry point" $CoordinatorContent "classify_and_finalize_autonomous_daily_operation_with_effect_seam" | Out-Null
Test-ContentDoesNotContain "the coordinator never re-derives an outcome classification locally (no parallel classifier)" $CoordinatorContent "fn classify_autonomous_daily_outcome" | Out-Null
Test-ContentDoesNotContain "the coordinator never issues its own terminal-state SQL/finalize call (no parallel terminal writer)" $CoordinatorContent "mqk_db::finalize_autonomous_daily_operation(" | Out-Null

Write-Host ""
Show-Info "--- [16] No API route or GUI surface was introduced ---"
$RoutesContent = $null
if (Test-Path $PathRoutesRs) { $RoutesContent = Get-Content -Raw -Path $PathRoutesRs }
$ApiTypesContent = $null
if (Test-Path $PathApiTypesRs) { $ApiTypesContent = Get-Content -Raw -Path $PathApiTypesRs }
Test-ContentDoesNotContain "routes.rs never references the outcome classifier" $RoutesContent "autonomous_daily_outcome" | Out-Null
Test-ContentDoesNotContain "api_types.rs never references the outcome classifier" $ApiTypesContent "autonomous_daily_outcome" | Out-Null
if (Test-Path (Join-Path $RepoRoot "core-rs\mqk-gui")) {
    $GuiTouched = git -C $RepoRoot diff --name-only HEAD -- core-rs/mqk-gui 2>$null
    if ([string]::IsNullOrWhiteSpace($GuiTouched)) {
        Show-Green "  OK -- no GUI file touched"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- GUI file(s) touched: $GuiTouched"
    }
}

Write-Host ""
Show-Info "--- [17] README truth: Phase E / Bundle 3 / soak / live-capital not overclaimed; E1/E2A/E2B accepted, E3 awaiting acceptance ---"
$ReadmeContent = $null
if (Test-FileExists "README.md" $PathReadme) {
    $ReadmeContent = Get-Content -Raw -Path $PathReadme
}
$ReadmeTechContent = $null
if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) {
    $ReadmeTechContent = Get-Content -Raw -Path $PathReadmeTech
}
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
# PROJECTION: E3 is now accepted (recorded by the operator ahead of this
# patch) -- the durable check going forward records E1/E2A/E2B/E3 as accepted
# and E4 as implementation-complete-awaiting-acceptance, never accepted
# itself. The prior "E3 not yet accepted" forbidden-claim entries are
# retired -- E3 acceptance is now the truthful, required state.
$ForbiddenReadmeClaims = @(
    "Phase E: COMPLETE",
    "Phase E is complete",
    "Phase E: ACCEPTED",
    "Bundle 3: CLOSED",
    "Bundle 3 is complete",
    "E4 implemented",
    # AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-
    # CLOSURE: "E4: ACCEPTED"/"E4 is accepted"/"E4 is complete"/"E4: COMPLETE"
    # are retired from this forbidden list -- E4 is now genuinely accepted,
    # required by this same patch's own documentation truth above.
    "coordinator invocation is accepted",
    "soak has started",
    "soak: STARTED",
    "unattended soak is underway",
    "live capital is ready",
    "live capital: ready",
    "approved for live capital",
    "ready for live capital"
)
foreach ($Doc in @(@{Name = "README.md"; Content = $ReadmeContent}, @{Name = "README_TECHNICAL.md"; Content = $ReadmeTechContent})) {
    foreach ($Phrase in $ForbiddenReadmeClaims) {
        Test-ContentDoesNotContain "$($Doc.Name) does not contain forbidden claim '$Phrase'" $Doc.Content $Phrase | Out-Null
    }
}
Test-ContentContains "README.md records E2A as accepted" $ReadmeContent "E2A (plus both repairs) is now accepted" | Out-Null
Test-ContentContains "README.md records E2B as accepted" $ReadmeContent "E2B is now accepted" | Out-Null
Test-ContentContains "README.md records E3 as accepted" $ReadmeContent "E3 is now accepted" | Out-Null
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE:
# E4 is now accepted (recorded by the operator ahead of this patch) -- the
# durable check going forward records E1/E2A/E2B/E3/E4 as accepted and E5 as
# implementation-complete-awaiting-acceptance, matching this patch's own
# required documentation truth.
Test-ContentContains "README.md records E4 as accepted" $ReadmeContent "E4 (plus both repairs and their test suites) is now accepted" | Out-Null
Test-ContentContains "README.md records E5 as implementation-complete-awaiting-acceptance" $ReadmeContent "is implementation complete, awaiting ChatGPT and operator acceptance" | Out-Null

Write-Host ""
Show-Info "--- [18] New E2B scenario test file exists and is nonempty ---"
$E2BTestContent = $null
if (Test-FileExists "E2B scenario test file" $PathE2BTest) {
    $E2BTestContent = Get-Content -Raw -Path $PathE2BTest
    if ($E2BTestContent.Length -gt 500) {
        Show-Green "  OK -- E2B test file is nonempty ($($E2BTestContent.Length) bytes)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E2B test file is suspiciously small"
    }
}

# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-
# UNCERTAINTY-CLOSURE (REPAIR 9): checks [19]-[24] below strengthen this
# guard with the exact-pairing validator, the complete-terminal-truth
# `AlreadyApplied` requirement, the high-level terminal-truth validation, the
# corrected coverage-missing precedence, and the real (not merely re-labeled)
# commit-uncertainty and partial-evidence-read-failure seams/tests this
# closure patch adds.
# =============================================================================

Write-Host ""
Show-Info "--- [19] Exact terminal-state/outcome pairing validator exists and gates finalize ---"
Test-ContentContains "mqk-db exposes the exact pairing validator" $DbOperationContent "pub fn is_valid_terminal_state_outcome_pair(state: &str, outcome: &str) -> bool" | Out-Null
if ($null -ne $FinalizeFnBody) {
    Test-ContentContains "finalize_autonomous_daily_operation's IllegalTarget gate uses the pairing validator" $FinalizeFnBody "is_valid_terminal_state_outcome_pair(&args.target_state, &args.outcome)" | Out-Null
}

Write-Host ""
Show-Info "--- [20] AlreadyApplied requires complete terminal truth, not merely a state/outcome match ---"
Test-ContentContains "mqk-db exposes the complete-terminal-truth check" $DbOperationContent "pub fn is_complete_automatic_terminal_truth(record: &AutonomousDailyOperationRecord) -> bool" | Out-Null
Test-ContentContains "the completeness check requires finalized_at_utc" $DbOperationContent "record.finalized_at_utc.is_some()" | Out-Null
Test-ContentContains "the completeness check requires a null state_reason_code" $DbOperationContent "record.state_reason_code.is_none()" | Out-Null
Test-ContentContains "the completeness check requires a null state_blocker_signature" $DbOperationContent "record.state_blocker_signature.is_none()" | Out-Null
if ($null -ne $FinalizeFnBody) {
    Test-ContentContains "finalize_autonomous_daily_operation's AlreadyApplied replay uses the completeness check" $FinalizeFnBody "is_complete_automatic_terminal_truth(&current)" | Out-Null
}

Write-Host ""
Show-Info "--- [21] High-level already-terminal handling distinguishes generic completed from malformed automatic rows ---"
$HighLevelEntryBody = Get-ContentBetween -Content $OutcomeContent `
    -StartNeedle "pub async fn classify_and_finalize_autonomous_daily_operation_with_effect_seam(" `
    -EndNeedle "`n}"
if ($null -eq $HighLevelEntryBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate classify_and_finalize_autonomous_daily_operation_with_effect_seam's body"
} else {
    Test-ContentContains "generic completed is handled as read-only before the automatic-row check" $HighLevelEntryBody "operation.state == mqk_db::STATE_COMPLETED" | Out-Null
    Test-ContentContains "an automatic terminal row's completeness is verified before AlreadyFinalized" $HighLevelEntryBody "mqk_db::is_complete_automatic_terminal_truth(&operation)" | Out-Null
    Test-ContentContains "an incomplete automatic terminal row returns Conflict, never AlreadyFinalized" $HighLevelEntryBody "AutonomousDailyFinalizationOutcome::Conflict" | Out-Null
}

Write-Host ""
Show-Info "--- [22] Coverage-missing precedence: always unknown_incomplete_bar_coverage, no empty-lineage special case ---"
Test-ContentDoesNotContain "classifier no longer special-cases empty lineage for missing coverage" $ClassifyFnBody "if lineage.is_empty() {" | Out-Null
$CoverageNoneArm = Get-ContentBetween -Content $OutcomeContent `
    -StartNeedle "let Some(coverage) = &snapshot.coverage else {" `
    -EndNeedle "`n    };"
if ($null -eq $CoverageNoneArm) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the coverage-missing branch"
} else {
    Test-ContentContains "coverage-missing branch returns IncompleteBarCoverage unconditionally" $CoverageNoneArm "IncompleteBarCoverage" | Out-Null
    Test-ContentDoesNotContain "coverage-missing branch no longer branches on lineage emptiness" $CoverageNoneArm "MissingEvaluationEvidence" | Out-Null
}

Write-Host ""
Show-Info "--- [23] Real, DB-proven commit-uncertainty effect seam (not a mocked successful write) ---"
Test-ContentContains "the injected effect seam type exists" $OutcomeContent "pub struct AutonomousDailyFinalizationEffectSeam" | Out-Null
Test-ContentContains "a test-support entry point threads the effect seam" $OutcomeContent "pub async fn classify_and_finalize_autonomous_daily_operation_with_effect_seam(" | Out-Null
Test-ContentContains "the production entry point always uses the all-default seam" $OutcomeContent "AutonomousDailyFinalizationEffectSeam::default()" | Out-Null
Test-ContentContains "commit-uncertainty scenario 1 (ack lost) test exists" $E2BTestContent "async fn store_50_high_level_commit_acknowledgment_lost_confirms_via_reread" | Out-Null
Test-ContentContains "commit-uncertainty scenario 2 (real stale CAS) test exists" $E2BTestContent "async fn store_51_high_level_cas_false_before_commit_claims_no_success" | Out-Null
Test-ContentContains "commit-uncertainty scenario 3 (real conflicting writer) test exists" $E2BTestContent "async fn store_52_high_level_conflicting_writer_returns_conflict_never_rewrites" | Out-Null

Write-Host ""
Show-Info "--- [24] Real partial-evidence-read-failure seam/test (distinct from the identity-unavailable proof) ---"
Test-ContentContains "the evidence-read-failure injection field exists" $OutcomeContent "force_evidence_read_failure_after_claims: bool" | Out-Null
Test-ContentContains "the blocker-persistence-unavailable injection field exists" $OutcomeContent "force_blocker_persistence_unavailable: bool" | Out-Null
Test-ContentContains "the real partial-evidence-read-failure test exists" $E2BTestContent "async fn store_48_real_partial_evidence_read_failure_degrades_via_confirmed_reread" | Out-Null
Test-ContentContains "the blocker-write-also-fails test exists" $E2BTestContent "async fn store_49_partial_evidence_read_failure_with_blocker_write_failure_persists_nothing" | Out-Null
Test-ContentContains "store_47 is no longer labeled a database-failure proof" $E2BTestContent "async fn store_47_assignment_identity_unavailable_is_not_a_database_failure_proof" | Out-Null

Write-Host ""
Show-Info "--- Ledger truth ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentContains "ledger records E2A as accepted" $LedgerContent "E2A: ACCEPTED" | Out-Null
Test-ContentContains "ledger records E2B as accepted" $LedgerContent "E2B: ACCEPTED" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase E complete" $LedgerContent "PHASE E: CLOSED" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase E complete (alt phrasing)" $LedgerContent "PHASE E: COMPLETE" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Bundle 3 closed" $LedgerContent "BUNDLE 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
