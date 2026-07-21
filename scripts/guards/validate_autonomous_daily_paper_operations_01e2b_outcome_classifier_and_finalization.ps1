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
#   [15] No coordinator integration was introduced (the coordinator source
#        file is untouched by this patch and never calls the E2B entry
#        point).
#   [16] No API route or GUI surface was introduced.
#   [17] README.md / README_TECHNICAL.md never claim Phase E, Bundle 3, an
#        unattended soak, or live-capital readiness as complete/started, and
#        record E2B as implementation-complete-awaiting-acceptance (not
#        accepted).
#   [18] The new E2B scenario test file exists and is nonempty.
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
Show-Info "--- [15] No coordinator integration was introduced ---"
$CoordinatorContent = $null
if (Test-Path $PathCoordinatorRs) {
    $CoordinatorContent = Get-Content -Raw -Path $PathCoordinatorRs
}
Test-ContentDoesNotContain "coordinator never calls classify_and_finalize_autonomous_daily_operation" $CoordinatorContent "classify_and_finalize_autonomous_daily_operation" | Out-Null
Test-ContentDoesNotContain "coordinator never references the outcome classifier module" $CoordinatorContent "autonomous_daily_outcome::" | Out-Null

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
Show-Info "--- [17] README truth: Phase E / Bundle 3 / soak / live-capital not overclaimed; E2B recorded correctly ---"
$ReadmeContent = $null
if (Test-FileExists "README.md" $PathReadme) {
    $ReadmeContent = Get-Content -Raw -Path $PathReadme
}
$ReadmeTechContent = $null
if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) {
    $ReadmeTechContent = Get-Content -Raw -Path $PathReadmeTech
}
$ForbiddenReadmeClaims = @(
    "Phase E: COMPLETE",
    "Phase E is complete",
    "Bundle 3: CLOSED",
    "Bundle 3 is complete",
    "E2B: ACCEPTED",
    "E2B is complete",
    "E2B: COMPLETE",
    "E3 implemented",
    "coordinator invokes the outcome classifier",
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
Test-ContentContains "README.md records E2B as implementation-complete-awaiting-acceptance" $ReadmeContent "Phase E2B (the strict evidence classifier consuming E2A's authorities" | Out-Null

Write-Host ""
Show-Info "--- [18] New E2B scenario test file exists and is nonempty ---"
if (Test-FileExists "E2B scenario test file" $PathE2BTest) {
    $E2BTestContent = Get-Content -Raw -Path $PathE2BTest
    if ($E2BTestContent.Length -gt 500) {
        Show-Green "  OK -- E2B test file is nonempty ($($E2BTestContent.Length) bytes)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E2B test file is suspiciously small"
    }
}

Write-Host ""
Show-Info "--- Ledger truth ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentContains "ledger records E2A as accepted" $LedgerContent "E2A: ACCEPTED" | Out-Null
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
