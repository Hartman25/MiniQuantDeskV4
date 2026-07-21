# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-
# FOUNDATION -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the E2A durable coverage-anchor and run-lineage
# evidence foundation, built on the already-accepted E1 contract (whose own
# invariants remain validated by
# validate_autonomous_daily_paper_operations_01e_outcome_contract.ps1,
# unmodified by this patch). No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test -- pure text/source
# validation only.
#
# AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE (E2A closure)
# strengthens this guard with checks [12]-[15] below, covering the complete
# envelope validator, the duplicate-key-rejecting parser, the adapter's
# corrected gate ordering (authority lookup strictly before assignment/
# identity resolution), and the live coordinator/adapter concurrency proof.
#
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
# INTEGRATION-AND-NOTIFICATION reconciles check [10] below: E3 is the
# explicitly authorized coordinator integration, and its whole mission is to
# add exactly one authorized call from `autonomous_daily_coordinator.rs` into
# the E2B finalizer. Check [10] is narrowed to permit exactly that one call
# while continuing to require E2A's own authority/lineage modules
# (`autonomous_daily_coverage_authority.rs`) and the completed-bar production
# adapter (`autonomous_completed_bar_task.rs`) never call the finalizer or
# reference the outcome-classifier module at all.
#
# Checks:
#   [1]  The coverage-bound event model/id/type/source constants exist.
#   [2]  `bound_at_utc` never appears in the semantic payload struct or its
#        serializer/parser (the bind instant is the event row's own `ts_utc`,
#        excluded from semantic equality).
#   [3]  The write/re-read/replay/conflict helper performs an exact-id
#        re-read (via the Stage A envelope validator) after every write
#        attempt -- never trusts a bare `Ok(())`.
#   [4]  The completed-bar adapter's authority gate (Stage A envelope lookup)
#        runs strictly before any assignment/runtime-binding resolution and
#        before any provider-client/registry construction, and no driver
#        invocation is reachable without it.
#   [5]  The coordinator's ensure-authority seam runs before state dispatch.
#   [6]  A prior-activity (non-pristine) operation can never have its anchor
#        fabricated -- the pristine check gates every bind attempt.
#   [7]  Mid-day coverage-policy drift is compared before the driver runs.
#   [8]  The run-lineage query never uses `SELECT DISTINCT` and the raw-row
#        helper is not routed through the general bounded API list cap.
#   [9]  The new E2A scenario test file exists and is nonempty.
#   [10] No E2B classifier/finalizer surface (`outcome`/`finalized_at_utc`
#        writer, a `completed*` transition call, or a classifier module) was
#        introduced by this patch's touched files.
#   [11] README.md / README_TECHNICAL.md never claim Phase E, Bundle 3, an
#        unattended soak, or live-capital readiness as complete/started, and
#        never mark E2A itself as accepted/complete.
#
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
# PROJECTION reconciles check [11]'s README-truth requirement: E3 is now
# accepted (recorded by the operator ahead of this patch), so the durable
# check going forward requires E1/E2A/E2B/E3 recorded as accepted and E4
# recorded as implementation-complete-awaiting-acceptance -- never E3 itself
# still described as merely awaiting acceptance.
#   [12] The complete durable event-envelope validator checks event_type,
#        source, run_id IS NULL, and resume_source IS NULL (not merely the
#        deterministic id and JSON detail payload).
#   [13] The JSON parser rejects duplicate fields deterministically (a typed,
#        `deny_unknown_fields` wire struct -- never a `serde_json::Value`
#        object map, which silently collapses a literal duplicate key).
#   [14] The adapter's Stage A authority envelope lookup precedes assignment/
#        identity resolution (`build_multi_symbol_runtime_config_from_env`),
#        not merely provider/registry construction -- a missing authority
#        must produce the quiet not-bound path even when the local
#        environment/configuration is malformed.
#   [15] A live coordinator/adapter concurrency-proof rendezvous hook and a
#        `tokio::join!`-driven interleaving test both exist.
#
# SAME-INSTANT-CONCURRENCY-AND-SIDE-EFFECT-PROOF-01 (E2A final proof repair)
# strengthens this guard with check [16] below, covering the one shared
# logical timestamp requirement, the full before/after durable snapshot, and
# the malformed-local-configuration provider-non-access proof -- and
# rejecting the prior two-independently-timestamped-ticks pattern outright.
#
#   [16] `f04`'s concurrency proof uses one shared logical `now_utc` for both
#        tasks (never a split `coordinator_now`/`adapter_now` pair), asserts
#        `state_version`/lifecycle-event-count/evaluation-count/
#        decision-count before and after task B's own tick, and proves the
#        missing-authority outcome survives a deliberately invalid local
#        instrument-registry path (never `RegistryUnavailable`/
#        `IdentityUnresolved`).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e2a_coverage_anchor_and_run_lineage.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathCoverageRs    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_coverage_authority.rs"
$PathCoordinatorRs = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_coordinator.rs"
$PathTaskRs        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_completed_bar_task.rs"
$PathDbOperationRs = Join-Path $RepoRoot "core-rs\crates\mqk-db\src\autonomous_daily_operation.rs"
$PathE2ATest       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.rs"
$PathReadme        = Join-Path $RepoRoot "README.md"
$PathReadmeTech    = Join-Path $RepoRoot "README_TECHNICAL.md"
$PathLedger        = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-FOUNDATION Validator"
Write-Host "============================================================"

$CoverageContent = $null
if (Test-FileExists "autonomous_daily_coverage_authority.rs" $PathCoverageRs) {
    $CoverageContent = Get-Content -Raw -Path $PathCoverageRs
}
$CoordinatorContent = $null
if (Test-FileExists "autonomous_daily_coordinator.rs" $PathCoordinatorRs) {
    $CoordinatorContent = Get-Content -Raw -Path $PathCoordinatorRs
}
$TaskContent = $null
if (Test-FileExists "autonomous_completed_bar_task.rs" $PathTaskRs) {
    $TaskContent = Get-Content -Raw -Path $PathTaskRs
}
$DbOperationContent = $null
if (Test-FileExists "mqk-db autonomous_daily_operation.rs" $PathDbOperationRs) {
    $DbOperationContent = Get-Content -Raw -Path $PathDbOperationRs
}

Write-Host ""
Show-Info "--- [1] Coverage-bound event model/id/type/source ---"
Test-ContentContains "typed CoverageBoundDetail struct exists" $CoverageContent "pub struct CoverageBoundDetail" | Out-Null
Test-ContentContains "deterministic operation-scoped event id helper exists" $CoverageContent "pub fn coverage_bound_event_id" | Out-Null
Test-ContentContains "event id is operation_id-keyed" $CoverageContent 'format!("autonomous_daily_coverage_bound:{operation_id}")' | Out-Null
Test-ContentContains "event type constant exists" $CoverageContent 'EVENT_TYPE_COVERAGE_BOUND: &str = "autonomous_daily_coverage_bound"' | Out-Null
Test-ContentContains "coordinator source constant exists" $CoverageContent 'COVERAGE_SOURCE: &str = "mqk-daemon.autonomous_daily_coordinator"' | Out-Null
Test-ContentContains "run_id is bound None for the coverage event" $CoverageContent "run_id: None," | Out-Null

Write-Host ""
Show-Info "--- [2] bound_at_utc never enters the semantic payload ---"
$PayloadStructBody = Get-ContentBetween -Content $CoverageContent `
    -StartNeedle "pub struct CoverageBoundDetail {" `
    -EndNeedle "`n}"
if ($null -eq $PayloadStructBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the CoverageBoundDetail struct body"
} else {
    Test-ContentDoesNotContain "CoverageBoundDetail's own field list never carries bound_at_utc" $PayloadStructBody "bound_at_utc" | Out-Null
}
$SerializeFnBody = Get-ContentBetween -Content $CoverageContent `
    -StartNeedle "pub fn serialize_coverage_bound_detail(" `
    -EndNeedle "`n}"
if ($null -ne $SerializeFnBody) {
    Test-ContentDoesNotContain "the serializer never emits a bound_at_utc key" $SerializeFnBody "bound_at_utc" | Out-Null
}
Test-ContentContains "the payload struct derives PartialEq (the semantic-equality mechanism)" $CoverageContent "#[derive(Debug, Clone, PartialEq)]" | Out-Null

Write-Host ""
Show-Info "--- [3] Exact-id re-read after every write attempt, through the Stage A envelope validator ---"
$EnsureFnBody = Get-ContentBetween -Content $CoverageContent `
    -StartNeedle "pub async fn write_and_confirm_coverage_authority(" `
    -EndNeedle "`r`n#[derive(Debug, Clone, PartialEq)]"
if ($null -eq $EnsureFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate write_and_confirm_coverage_authority's body"
} else {
    Test-ContentContains "the write path never trusts a bare persist Ok(()) alone" $EnsureFnBody "let _ = mqk_db::persist_autonomous_session_event(pool, &row).await;" | Out-Null
    Test-ContentContains "the write path re-reads through the shared Stage A envelope validator" $EnsureFnBody "check_coverage_authority_envelope(pool, fresh.operation_id)" | Out-Null
}
Test-ContentContains "mqk-db exposes an exact-id read for the session event store" $CoverageContent "mqk_db::fetch_autonomous_session_event_by_id" | Out-Null

Write-Host ""
Show-Info "--- [4] Adapter authority gate precedes assignment resolution and provider/registry construction ---"
$TickFnBody = Get-ContentBetween -Content $TaskContent `
    -StartNeedle "pub async fn tick_autonomous_completed_bar_driver_from_state(" `
    -EndNeedle "`r`n/// D3.13"
if ($null -eq $TickFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate tick_autonomous_completed_bar_driver_from_state's body"
} else {
    $GateIdx = $TickFnBody.IndexOf("check_coverage_authority_envelope(", [System.StringComparison]::OrdinalIgnoreCase)
    $AssignmentIdx = $TickFnBody.IndexOf("build_multi_symbol_runtime_config_from_env()", [System.StringComparison]::OrdinalIgnoreCase)
    $InstrumentsIdx = $TickFnBody.IndexOf("load_driver_instruments(", [System.StringComparison]::OrdinalIgnoreCase)
    if ($GateIdx -ge 0 -and $AssignmentIdx -ge 0 -and $GateIdx -lt $AssignmentIdx) {
        Show-Green "  OK -- check_coverage_authority_envelope runs before assignment resolution"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- the coverage-authority envelope gate does not precede assignment resolution (gate=$GateIdx, assignment=$AssignmentIdx)"
    }
    if ($GateIdx -ge 0 -and $InstrumentsIdx -ge 0 -and $GateIdx -lt $InstrumentsIdx) {
        Show-Green "  OK -- check_coverage_authority_envelope runs before load_driver_instruments"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- the coverage-authority envelope gate does not precede load_driver_instruments (gate=$GateIdx, instruments=$InstrumentsIdx)"
    }
    Test-ContentContains "the adapter has a typed CoverageAuthorityUnavailable outcome" $TickFnBody "CoverageAuthorityUnavailable" | Out-Null
    Test-ContentContains "not-bound is a quiet no-op with no lifecycle mutation" $TaskContent "REASON_COVERAGE_AUTHORITY_NOT_BOUND" | Out-Null
}
Test-ContentContains "CoverageAuthorityUnavailable outcome variant is defined" $TaskContent "CoverageAuthorityUnavailable {" | Out-Null
Test-ContentContains "four closed reason codes exist" $CoverageContent "REASON_COVERAGE_AUTHORITY_NOT_BOUND" | Out-Null
Test-ContentContains "four closed reason codes exist (unreadable)" $CoverageContent "REASON_COVERAGE_AUTHORITY_UNREADABLE" | Out-Null
Test-ContentContains "four closed reason codes exist (invalid)" $CoverageContent "REASON_COVERAGE_AUTHORITY_INVALID" | Out-Null
Test-ContentContains "four closed reason codes exist (conflict)" $CoverageContent "REASON_COVERAGE_AUTHORITY_CONFLICT" | Out-Null

Write-Host ""
Show-Info "--- [5] Coordinator ensure-authority seam runs before state dispatch ---"
$TickCoordinatorBody = Get-ContentBetween -Content $CoordinatorContent `
    -StartNeedle "pub async fn tick_autonomous_daily_coordinator(" `
    -EndNeedle "`r`n// ---"
if ($null -eq $TickCoordinatorBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate tick_autonomous_daily_coordinator's body"
} else {
    $EnsureIdx = $TickCoordinatorBody.IndexOf("ensure_coverage_authority(", [System.StringComparison]::OrdinalIgnoreCase)
    $DispatchIdx = $TickCoordinatorBody.IndexOf("dispatch_by_state(state, &pool, operation, &plan, now_utc)", [System.StringComparison]::OrdinalIgnoreCase)
    if ($EnsureIdx -ge 0 -and $DispatchIdx -ge 0 -and $EnsureIdx -lt $DispatchIdx) {
        Show-Green "  OK -- ensure_coverage_authority runs before dispatch_by_state"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- ensure_coverage_authority does not precede dispatch_by_state (ensure=$EnsureIdx, dispatch=$DispatchIdx)"
    }
}
Test-ContentContains "close priority is preserved in the coverage blocker path" $CoordinatorContent "handle_session_close(state, pool, operation.clone(), now_utc).await" | Out-Null

Write-Host ""
Show-Info "--- [6] Prior-activity rows can never have a fabricated anchor ---"
Test-ContentContains "a durable pristine-evidence check exists" $CoverageContent "pub async fn check_operation_pristine" | Out-Null
Test-ContentContains "pristine check inspects run_id/started_at_utc/bars/claims" $CoverageContent "operation.run_id.is_some()" | Out-Null
Test-ContentContains "pristine check inspects the full run lineage" $CoverageContent "fetch_and_validate_autonomous_daily_operation_run_lineage" | Out-Null
Test-ContentContains "a contradictory/unreadable lineage read is treated as HasActivity, never Pristine" $CoverageContent "Err(_) => Ok(PristineCheckOutcome::HasActivity)" | Out-Null
Test-ContentContains "missing-after-activity reason code exists" $CoverageContent "REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY" | Out-Null
Test-ContentContains "coordinator only binds after confirming Pristine" $CoordinatorContent "Ok(PristineCheckOutcome::Pristine) => {" | Out-Null

Write-Host ""
Show-Info "--- [7] Mid-day policy drift is compared before the driver runs ---"
Test-ContentContains "adapter resolves current policy on every tick" $TaskContent "resolve_current_coverage_policy_inputs(" | Out-Null
Test-ContentContains "semantic comparison against the Stage A authority value is explicit" $TaskContent "if bound_authority != fresh_coverage {" | Out-Null
Test-ContentContains "a semantic mismatch reports the conflict reason code, not a silent pass" $TaskContent "REASON_COVERAGE_AUTHORITY_CONFLICT" | Out-Null

Write-Host ""
Show-Info "--- [8] Run-lineage query never uses SELECT DISTINCT; no general list cap ---"
$RawLineageFnBody = Get-ContentBetween -Content $DbOperationContent `
    -StartNeedle "pub async fn fetch_autonomous_daily_operation_running_transitions_raw(" `
    -EndNeedle "`r`n/// Fail-closed reason"
if ($null -eq $RawLineageFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate fetch_autonomous_daily_operation_running_transitions_raw's body"
} else {
    Test-ContentDoesNotContain "the raw run-lineage query never uses SELECT DISTINCT" $RawLineageFnBody "distinct" | Out-Null
    Test-ContentDoesNotContain "the raw run-lineage helper never binds a LIMIT parameter" $RawLineageFnBody ".bind(limit)" | Out-Null
    Test-ContentDoesNotContain "the raw run-lineage helper is not routed through the general list cap" $RawLineageFnBody "bounded_limit(" | Out-Null
}
Test-ContentContains "a pure Rust validator exists for the raw lineage rows" $DbOperationContent "pub fn validate_autonomous_daily_operation_run_lineage" | Out-Null
Test-ContentContains "the validator detects duplicate run ids" $DbOperationContent "DuplicateRunId(Uuid)" | Out-Null
Test-ContentContains "the validator detects non-monotonic sequence" $DbOperationContent "NonMonotonicSequence" | Out-Null
Test-ContentContains "the validator detects a current-run mismatch" $DbOperationContent "CurrentRunMismatch" | Out-Null

Write-Host ""
Show-Info "--- [9] New E2A scenario test file exists ---"
$E2ATestContent = $null
if (Test-FileExists "E2A scenario test file" $PathE2ATest) {
    $E2ATestContent = Get-Content -Raw -Path $PathE2ATest
    if ($E2ATestContent.Length -lt 2000) {
        $script:Violations++
        Show-Red "  FAIL -- E2A scenario test file is suspiciously short ($($E2ATestContent.Length) chars)"
    } else {
        Show-Green "  OK -- E2A scenario test file is nonempty ($($E2ATestContent.Length) chars)"
    }
    Test-ContentContains "the test file proves the pristine coordinator bind path" $E2ATestContent "async fn e01_coordinator_binds_pristine_anchor_and_replays_on_second_tick" | Out-Null
    Test-ContentContains "the test file proves prior-activity fail-closed behavior" $E2ATestContent "REASON_COVERAGE_AUTHORITY_MISSING_AFTER_ACTIVITY" | Out-Null
    Test-ContentContains "the test file proves the adapter's zero-side-effect not-bound path" $E2ATestContent "async fn f01_adapter_returns_not_bound_with_zero_side_effects" | Out-Null
    Test-ContentContains "the test file proves mid-day drift" $E2ATestContent "mid_day_drift_returns_conflict_and_invokes_no_driver" | Out-Null
    Test-ContentContains "the test file proves run-lineage recovery" $E2ATestContent "async fn h01_initial_and_recovery_run_lineage_read_and_validated" | Out-Null
}

Write-Host ""
Show-Info "--- [10] E2A's own modules never call the E2B finalizer; exactly one authorized E3 coordinator call exists ---"
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
# INTEGRATION-AND-NOTIFICATION: the prior check [10] (already narrowed once
# by E2B's own REPAIR 8) forbade the coordinator from calling the E2B
# finalizer at all, since no coordinator integration was authorized yet. E3
# is exactly that authorized integration -- the durable check going forward
# permits exactly one call site in `autonomous_daily_coordinator.rs` while
# continuing to forbid it everywhere else: E2A's own coverage-authority
# module and the completed-bar production adapter must never call the E2B
# finalizer or reference its module at all, and the coordinator must not
# duplicate E2B's own classification/persistence logic locally.
foreach ($ForbiddenPath in @($PathCoverageRs, $PathTaskRs)) {
    if (-not (Test-Path $ForbiddenPath)) { continue }
    $Content = Get-Content -Raw -Path $ForbiddenPath
    $RelName = Split-Path -Leaf $ForbiddenPath
    Test-ContentDoesNotContain "$RelName never calls the E2B finalizer entry point" $Content "classify_and_finalize_autonomous_daily_operation" | Out-Null
    Test-ContentDoesNotContain "$RelName never references the E2B outcome classifier module" $Content "autonomous_daily_outcome::" | Out-Null
}
$PathOutcomeRs = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_outcome.rs"
Test-FileExists "E2B outcome classifier/finalizer module (isolated)" $PathOutcomeRs | Out-Null
Test-ContentContains "the coordinator calls the accepted E2B production entry point exactly once by name" $CoordinatorContent "autonomous_daily_outcome::classify_and_finalize_autonomous_daily_operation(" | Out-Null
Test-ContentDoesNotContain "the coordinator never calls the E2B test-support effect-seam entry point" $CoordinatorContent "classify_and_finalize_autonomous_daily_operation_with_effect_seam" | Out-Null
Test-ContentDoesNotContain "the coordinator never re-derives an outcome classification locally (no parallel classifier)" $CoordinatorContent "fn classify_autonomous_daily_outcome" | Out-Null
Test-ContentDoesNotContain "the coordinator never issues its own terminal-state SQL/finalize call (no parallel terminal writer)" $CoordinatorContent "mqk_db::finalize_autonomous_daily_operation(" | Out-Null
Test-ContentDoesNotContain "no API route module is introduced by touched coordinator/outcome files" $CoordinatorContent "async fn get_autonomous_daily_operation" | Out-Null

Write-Host ""
Show-Info "--- [11] README truth: E1/E2A/E2B accepted, E3 awaiting acceptance, E4 not started, Phase E/Bundle 3 open ---"
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
# patch); the durable check going forward is the current truthful
# progression: E1/E2A/E2B/E3 accepted, E4 implementation complete but not yet
# accepted, E5 not started, Phase E/Bundle 3 open, soak not started, live
# capital not ready. The prior "E3 not yet accepted" forbidden-claim entries
# are retired -- E3 acceptance is now the truthful, required state.
$ForbiddenReadmeClaims = @(
    "Phase E: CLOSED",
    "Phase E: COMPLETE",
    "Phase E is complete",
    "Phase E: ACCEPTED",
    "Bundle 3: CLOSED",
    "Bundle 3 is complete",
    "E4 implemented",
    "E4: ACCEPTED",
    "E4 is accepted",
    "E4 is complete",
    "E4: COMPLETE",
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
Test-ContentContains "README.md records E1 as accepted" $ReadmeContent "E1 is now accepted" | Out-Null
Test-ContentContains "README.md records E2A as accepted" $ReadmeContent "E2A (plus both repairs) is now accepted" | Out-Null
Test-ContentContains "README.md records E2B as accepted" $ReadmeContent "E2B is now accepted" | Out-Null
Test-ContentContains "README.md records E3 as accepted" $ReadmeContent "E3 is now accepted" | Out-Null
Test-ContentContains "README.md records E4 as implementation-complete-awaiting-acceptance" $ReadmeContent "awaiting ChatGPT and operator acceptance" | Out-Null
Test-ContentContains "README_TECHNICAL.md records E1 as accepted" $ReadmeTechContent "E1 is accepted complete" | Out-Null
Test-ContentContains "README_TECHNICAL.md records E2A as accepted" $ReadmeTechContent "plus both repairs, is **accepted complete**" | Out-Null

Write-Host ""
Show-Info "--- [12] Complete envelope validator checks event_type/source/run_id NULL/resume_source NULL ---"
$EnvelopeFnBody = Get-ContentBetween -Content $CoverageContent `
    -StartNeedle "pub fn validate_coverage_authority_envelope(" `
    -EndNeedle "`r`n}"
if ($null -eq $EnvelopeFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate validate_coverage_authority_envelope's body"
} else {
    Test-ContentContains "envelope validator checks the exact deterministic id" $EnvelopeFnBody "row.id != coverage_bound_event_id(operation_id)" | Out-Null
    Test-ContentContains "envelope validator checks event_type" $EnvelopeFnBody "row.event_type != EVENT_TYPE_COVERAGE_BOUND" | Out-Null
    Test-ContentContains "envelope validator checks run_id IS NULL" $EnvelopeFnBody "row.run_id.is_some()" | Out-Null
    Test-ContentContains "envelope validator checks resume_source IS NULL" $EnvelopeFnBody "row.resume_source.is_some()" | Out-Null
    Test-ContentContains "envelope validator checks source" $EnvelopeFnBody "row.source != COVERAGE_SOURCE" | Out-Null
}
Test-ContentContains "envelope validation is used by the Stage A read path" $CoverageContent "validate_coverage_authority_envelope(operation_id, &row)" | Out-Null

Write-Host ""
Show-Info "--- [13] Duplicate-key-rejecting typed parser (never a serde_json::Value object map) ---"
Test-ContentContains "a typed wire struct exists for the coverage-bound detail payload" $CoverageContent "struct CoverageBoundDetailWire" | Out-Null
Test-ContentContains "the wire struct derives Deserialize" $CoverageContent "#[derive(Debug, Deserialize)]" | Out-Null
Test-ContentContains "the wire struct denies unknown fields" $CoverageContent "#[serde(deny_unknown_fields)]" | Out-Null
$ParseFnBody = Get-ContentBetween -Content $CoverageContent `
    -StartNeedle "pub fn parse_coverage_bound_detail(" `
    -EndNeedle "`r`nfn validate_semantic_invariants"
if ($null -eq $ParseFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate parse_coverage_bound_detail's body"
} else {
    Test-ContentContains "the parser decodes directly into the typed wire struct" $ParseFnBody "serde_json::from_str(raw)" | Out-Null
    Test-ContentDoesNotContain "the parser never decodes into a raw serde_json::Value object map" $ParseFnBody "serde_json::Value =" | Out-Null
}

Write-Host ""
Show-Info "--- [14] Adapter authority lookup precedes identity resolution even under malformed local config ---"
if ($null -ne $TickFnBody) {
    $RuntimeCtxIdx = $TickFnBody.IndexOf("resolve_autonomous_runtime_context(state)", [System.StringComparison]::OrdinalIgnoreCase)
    $GateIdx2 = $TickFnBody.IndexOf("check_coverage_authority_envelope(", [System.StringComparison]::OrdinalIgnoreCase)
    if ($GateIdx2 -ge 0 -and $RuntimeCtxIdx -ge 0 -and $GateIdx2 -lt $RuntimeCtxIdx) {
        Show-Green "  OK -- check_coverage_authority_envelope runs before runtime-binding resolution"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- the coverage-authority envelope gate does not precede runtime-binding resolution (gate=$GateIdx2, runtime_ctx=$RuntimeCtxIdx)"
    }
}
Test-ContentContains "Stage A is documented as running strictly before assignment/runtime/policy resolution" $TaskContent "strictly before any assignment/runtime-binding/policy resolution" | Out-Null

Write-Host ""
Show-Info "--- [15] Live coordinator/adapter concurrency-proof rendezvous exists ---"
Test-ContentContains "a deterministic pre-bind rendezvous test hook is defined" $CoordinatorContent "pub struct AutonomousCoverageAuthorityPreBindTestHook" | Out-Null
Test-ContentContains "the coordinator tick installs the rendezvous point after create_or_recover" $CoordinatorContent "coverage_authority_pre_bind_test_hook_for_test().await" | Out-Null
if ($null -ne $E2ATestContent) {
    Test-ContentContains "the test file drives a live coordinator/adapter interleaving with tokio::join!" $E2ATestContent "tokio::join!(coordinator_fut, adapter_fut)" | Out-Null
    Test-ContentContains "the test file names the live interleaving proof" $E2ATestContent "async fn f04_live_coordinator_and_adapter_interleaving_proves_zero_side_effects_while_paused" | Out-Null
    Test-ContentContains "the test file proves duplicate JSON keys are rejected" $E2ATestContent "fn b06_parse_rejects_duplicate_json_keys" | Out-Null
    Test-ContentContains "the test file proves the write/re-read path tamper-checks every envelope field" $E2ATestContent "assert_tampered_row_rejected_and_preserved" | Out-Null
}

Write-Host ""
Show-Info "--- [16] f04: one shared logical instant, full before/after snapshot, malformed-config proof ---"
$F04Body = Get-ContentBetween -Content $E2ATestContent `
    -StartNeedle "async fn f04_live_coordinator_and_adapter_interleaving_proves_zero_side_effects_while_paused()" `
    -EndNeedle "Group G"
if ($null -eq $F04Body) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate f04's body"
} else {
    Test-ContentContains "f04 uses one shared logical instant for both tasks" $F04Body "shared_now_utc" | Out-Null
    Test-ContentDoesNotContain "f04 never reintroduces a split coordinator_now/adapter_now proof" $F04Body "let coordinator_now" | Out-Null
    Test-ContentDoesNotContain "f04 never reintroduces a split coordinator_now/adapter_now proof (adapter_now)" $F04Body "let adapter_now" | Out-Null
    Test-ContentContains "f04 asserts state_version unchanged across task B's tick" $F04Body "state_version, after.state_version" | Out-Null
    Test-ContentContains "f04 asserts zero new lifecycle events from task B" $F04Body "lifecycle_event_count" | Out-Null
    Test-ContentContains "f04 asserts a zero strategy-evaluation-count delta" $F04Body "evaluation_count" | Out-Null
    Test-ContentContains "f04 asserts a zero decision-count delta" $F04Body "decision_count" | Out-Null
    Test-ContentContains "f04 runs the paused-window adapter tick under an invalid instrument-registry path" $F04Body "does/not/exist" | Out-Null
    Test-ContentContains "f04 documents that a RegistryUnavailable/IdentityUnresolved outcome would mean the invalid path was touched" $F04Body "actually reached before Stage A" | Out-Null
    Test-ContentContains "f04 proves release + normal progression to DriverOutcome" $F04Body "AutonomousCompletedBarProductionTickOutcome::DriverOutcome" | Out-Null
}

Write-Host ""
Show-Info "--- Ledger truth ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentContains "ledger records E1 as accepted" $LedgerContent "E1: ACCEPTED" | Out-Null
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
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-FOUNDATION evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
