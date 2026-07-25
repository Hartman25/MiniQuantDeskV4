# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-
# INTEGRATION-AND-NOTIFICATION -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the E3 coordinator finalization integration and
# outcome/evidence-degraded notification wiring, built on the already-
# accepted E1 contract, E2A coverage-anchor/run-lineage foundation, and E2B
# strict classifier/finalization CAS (all validated by their own,
# independently-maintained guards). No network call, no provider/broker
# call, no DB connection, no daemon start, no cargo/npm build or test --
# pure text/source validation only.
#
# Checks:
#   [1]  `handle_stopping` requires `operation.stopped_at_utc.is_some()`
#        before routing into finalization -- stopped_at_utc is never
#        optional/inferred.
#   [2]  The matching-local-runtime fact is derived from
#        `AppState::locally_owned_run_id()` compared against
#        `operation.run_id` -- never `locally_started`, a process-local bar
#        counter, or a GUI-facing field.
#   [3]  The coordinator never re-derives an outcome classification locally
#        (no parallel `classify_autonomous_daily_outcome`-shaped function).
#   [4]  The coordinator never issues its own terminal-state SQL/finalize
#        call and never constructs `FinalizeAutonomousDailyOperationArgs`
#        directly (no parallel terminal writer).
#   [5]  The coordinator's own blocker-persistence seam
#        (`persist_autonomous_daily_finalization_blocker`) is a thin
#        wrapper around E2B's existing `apply_evidence_degraded_blocker` --
#        it introduces no second CAS/signature/re-read algorithm.
#   [6]  The completed-bar production adapter/driver never calls the E2B
#        finalizer.
#   [7]  No direct `evidence_degraded -> completed*` transition edge exists
#        -- recovery remains routed through the existing
#        `evidence_degraded -> stopping` edge only.
#   [8]  The terminal outcome notification is gated on E2B's `Finalized`
#        result only, never on `AlreadyFinalized` -- a terminal replay can
#        never notify again.
#   [9]  The evidence-degraded warning is gated on `newly_applied` -- an
#        unchanged blocker can never notify again.
#   [10] `DatabaseUnavailable`/`Conflict` finalization results never trigger
#        a notification call.
#   [11] Notification calls in `log_coordinator_outcome` occur only after
#        the durable coordinator tick has already returned (i.e. only inside
#        the outcome-logging function itself, never before or during the
#        CAS write path).
#   [12] No raw error/debug text enters a notification payload -- critical-
#        alert/run-status payload construction in the new coordinator code
#        never formats an `anyhow::Error`/`{:?}` into `summary`/`detail`.
#   [13] No real Discord/network access is introduced by the new E3 test
#        file -- it never calls `DiscordNotifier::from_env` and never
#        constructs a non-loopback (`discord.com`) webhook URL.
#   [14] AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
#        PROJECTION reconciliation: the obsolete "no E4 route at all" check
#        is replaced by a durable equivalent -- exactly the two authorized
#        E4 read-only routes exist on the public router, neither one invokes
#        the E3 coordinator's finalization entry points, and no GUI file is
#        touched.
#   [15] README.md / README_TECHNICAL.md never mark E3, Phase E, or Bundle 3
#        as accepted/complete, and record E1/E2A/E2B as accepted with E3
#        implementation-complete-awaiting-acceptance.
#   [16] The new E3 spec doc and scenario test file both exist and are
#        nonempty.
#
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-
# GATE-REPAIR-01 (added checks -- closes the confirmed defect where
# handle_outcome_finalization's config/runtime-context-resolution failure
# branches could persist an evidence_degraded blocker and notify while a
# matching local runtime was still active):
#   [17] `handle_outcome_finalization` returns `AwaitingOutcomeFinalization`
#        from an early `if context.matching_local_runtime_active` gate,
#        strictly before its config resolution, runtime-context resolution,
#        blocker-persistence, and classify-and-finalize calls -- never merely
#        as a side effect deep inside one of those branches.
#   [18] `persist_autonomous_daily_finalization_blocker` requires a caller-
#        supplied `AutonomousDailyFinalizationContext` and refuses to write
#        (returns `NotEligible`, zero DB calls) via a shared eligibility gate
#        checked strictly before its real `apply_evidence_degraded_blocker`
#        write -- defense-in-depth so this seam can never become a second way
#        to bypass finalization eligibility. Every coordinator call site
#        threads the context through.
#   [19] The required coordinator-level DB-backed proof test
#        (`ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification`)
#        exists by exact name in the E3 scenario test file.
#   [20] The required direct E2B store-level proof test for the hardened
#        wrapper (`store_59_persist_finalization_blocker_refuses_when_matching_runtime_active`)
#        exists by exact name, calls the wrapper directly with a matching-
#        runtime-active context, and asserts `NotEligible`.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1: PowerShell 7.3+ turns native-command
# stderr text (e.g. git's routine "LF will be replaced by CRLF" autocrlf
# notice) into terminating ErrorRecords under $ErrorActionPreference = "Stop",
# even when the stream is redirected with 2>$null -- this fires as soon as a
# GUI file is genuinely present in the working tree (e.g. during Phase F),
# which is exactly this check's own subject. Disable that promotion for this
# script only; it does not change $LASTEXITCODE handling, which every check
# below relies on explicitly.
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathCoordinatorRs  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_coordinator.rs"
$PathOutcomeRs      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_outcome.rs"
$PathSessionCtrlRs  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\session_controller.rs"
$PathDbOperationRs  = Join-Path $RepoRoot "core-rs\crates\mqk-db\src\autonomous_daily_operation.rs"
$PathDriverRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_completed_bar_driver.rs"
$PathTaskRs         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_completed_bar_task.rs"
$PathRoutesRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$PathApiTypesRs     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$PathE3Spec         = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.md"
$PathE3Test         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_outcome_coordinator_integration_01.rs"
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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION Validator"
Write-Host "============================================================"

$CoordinatorContent = $null
if (Test-FileExists "autonomous_daily_coordinator.rs" $PathCoordinatorRs) {
    $CoordinatorContent = Get-Content -Raw -Path $PathCoordinatorRs
}
$OutcomeContent = $null
if (Test-FileExists "autonomous_daily_outcome.rs" $PathOutcomeRs) {
    $OutcomeContent = Get-Content -Raw -Path $PathOutcomeRs
}
$SessionCtrlContent = $null
if (Test-FileExists "session_controller.rs" $PathSessionCtrlRs) {
    $SessionCtrlContent = Get-Content -Raw -Path $PathSessionCtrlRs
}
$DbOperationContent = $null
if (Test-FileExists "mqk-db autonomous_daily_operation.rs" $PathDbOperationRs) {
    $DbOperationContent = Get-Content -Raw -Path $PathDbOperationRs
}
$DriverContent = $null
if (Test-Path $PathDriverRs) { $DriverContent = Get-Content -Raw -Path $PathDriverRs }
$TaskContent = $null
if (Test-Path $PathTaskRs) { $TaskContent = Get-Content -Raw -Path $PathTaskRs }

Write-Host ""
Show-Info "--- [1] handle_stopping requires stopped_at_utc before routing into finalization ---"
$HandleStoppingBody = Get-ContentBetween -Content $CoordinatorContent `
    -StartNeedle "pub async fn handle_stopping(" `
    -EndNeedle "`npub async fn retry_stop("
if ($null -eq $HandleStoppingBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate handle_stopping's body"
} else {
    Test-ContentContains "handle_stopping checks operation.stopped_at_utc.is_some() before finalizing" $HandleStoppingBody "if operation.stopped_at_utc.is_some() {" | Out-Null
    Test-ContentContains "the stopped_at_utc branch routes into handle_outcome_finalization" $HandleStoppingBody "handle_outcome_finalization(state, pool, operation, now_utc).await" | Out-Null
}
Test-ContentContains "the evidence_degraded routing arm also gates on stopped_at_utc" $CoordinatorContent "mqk_db::STATE_EVIDENCE_DEGRADED if operation.stopped_at_utc.is_some() =>" | Out-Null

Write-Host ""
Show-Info "--- [2] matching-local-runtime fact is derived from locally_owned_run_id vs operation.run_id ---"
$MatchingFnBody = Get-ContentBetween -Content $CoordinatorContent `
    -StartNeedle "async fn matching_local_runtime_active(" `
    -EndNeedle "`n}"
if ($null -eq $MatchingFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate matching_local_runtime_active's body"
} else {
    Test-ContentContains "matching_local_runtime_active reads AppState::locally_owned_run_id" $MatchingFnBody "state.locally_owned_run_id().await" | Out-Null
    Test-ContentContains "matching_local_runtime_active compares against operation.run_id" $MatchingFnBody "operation.run_id" | Out-Null
    Test-ContentDoesNotContain "matching_local_runtime_active never reads locally_started" $MatchingFnBody "locally_started" | Out-Null
    Test-ContentDoesNotContain "matching_local_runtime_active never reads a bar dispatch counter" $MatchingFnBody "bar_tick_dispatch_count" | Out-Null
}
Test-ContentContains "handle_outcome_finalization threads the fact into AutonomousDailyFinalizationContext" $CoordinatorContent "matching_local_runtime_active: matching_local_runtime_active(state, &operation).await" | Out-Null

Write-Host ""
Show-Info "--- [3] No parallel classifier in the coordinator ---"
Test-ContentDoesNotContain "coordinator never defines its own classify_autonomous_daily_outcome-shaped function" $CoordinatorContent "fn classify_autonomous_daily_outcome" | Out-Null
Test-ContentDoesNotContain "coordinator never references AutonomousDailyOutcomeClassification directly" $CoordinatorContent "AutonomousDailyOutcomeClassification" | Out-Null

Write-Host ""
Show-Info "--- [4] No parallel terminal writer in the coordinator ---"
Test-ContentDoesNotContain "coordinator never calls mqk_db::finalize_autonomous_daily_operation directly" $CoordinatorContent "mqk_db::finalize_autonomous_daily_operation(" | Out-Null
Test-ContentDoesNotContain "coordinator never constructs FinalizeAutonomousDailyOperationArgs directly" $CoordinatorContent "FinalizeAutonomousDailyOperationArgs" | Out-Null

Write-Host ""
Show-Info "--- [5] persist_autonomous_daily_finalization_blocker is a thin wrapper, not a second algorithm ---"
$PersistBlockerFnBody = Get-ContentBetween -Content $OutcomeContent `
    -StartNeedle "pub async fn persist_autonomous_daily_finalization_blocker(" `
    -EndNeedle "`n}"
if ($null -eq $PersistBlockerFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate persist_autonomous_daily_finalization_blocker's body"
} else {
    Test-ContentContains "the seam delegates to the existing apply_evidence_degraded_blocker" $PersistBlockerFnBody "apply_evidence_degraded_blocker(" | Out-Null
    Test-ContentDoesNotContain "the seam never issues its own transition/refresh SQL call" $PersistBlockerFnBody "mqk_db::transition_autonomous_daily_operation(" | Out-Null
    Test-ContentDoesNotContain "the seam never issues its own refresh-blocker SQL call" $PersistBlockerFnBody "mqk_db::refresh_autonomous_daily_operation_blocker(" | Out-Null
}
Test-ContentContains "the coordinator calls the seam by name for policy-resolution failure" $CoordinatorContent "persist_autonomous_daily_finalization_blocker(" | Out-Null

Write-Host ""
Show-Info "--- [6] The completed-bar production adapter/driver never calls the E2B finalizer ---"
Test-ContentDoesNotContain "autonomous_completed_bar_driver.rs never calls the E2B finalizer" $DriverContent "classify_and_finalize_autonomous_daily_operation" | Out-Null
Test-ContentDoesNotContain "autonomous_completed_bar_driver.rs never references the outcome classifier module" $DriverContent "autonomous_daily_outcome::" | Out-Null
Test-ContentDoesNotContain "autonomous_completed_bar_task.rs never calls the E2B finalizer" $TaskContent "classify_and_finalize_autonomous_daily_operation" | Out-Null
Test-ContentDoesNotContain "autonomous_completed_bar_task.rs never references the outcome classifier module" $TaskContent "autonomous_daily_outcome::" | Out-Null

Write-Host ""
Show-Info "--- [7] No direct evidence_degraded -> completed* edge; recovery routes through stopping only ---"
$EvidenceDegradedArm = Get-ContentBetween -Content $DbOperationContent `
    -StartNeedle "Some(STATE_EVIDENCE_DEGRADED) => matches!(" `
    -EndNeedle "`n        ),"
if ($null -eq $EvidenceDegradedArm) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the evidence_degraded legal-transition arm"
} else {
    Test-ContentDoesNotContain "evidence_degraded's legal edges never include completed_no_trade" $EvidenceDegradedArm "STATE_COMPLETED_NO_TRADE" | Out-Null
    Test-ContentDoesNotContain "evidence_degraded's legal edges never include completed_with_activity" $EvidenceDegradedArm "STATE_COMPLETED_WITH_ACTIVITY" | Out-Null
    Test-ContentDoesNotContain "evidence_degraded's legal edges never include generic completed" $EvidenceDegradedArm "STATE_COMPLETED |" | Out-Null
}
Test-ContentContains "recovery reuses the existing evidence_degraded -> stopping edge" $OutcomeContent "new_state: mqk_db::STATE_STOPPING.to_string()," | Out-Null

Write-Host ""
Show-Info "--- [8] Terminal outcome notification is gated on Finalized only, never AlreadyFinalized ---"
$FinalizedArm = Get-ContentBetween -Content $SessionCtrlContent `
    -StartNeedle "Outcome::OutcomeFinalized {" `
    -EndNeedle "`n        }`n"
if ($null -eq $FinalizedArm) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the OutcomeFinalized notification arm"
} else {
    Test-ContentContains "OutcomeFinalized sends a notify_run_status call" $FinalizedArm "notify_run_status(" | Out-Null
    Test-ContentContains "the terminal notification uses the stable, documented event string" $FinalizedArm "autonomous.daily_operation.outcome" | Out-Null
}
$AlreadyFinalizedArm = Get-ContentBetween -Content $SessionCtrlContent `
    -StartNeedle "Outcome::OutcomeAlreadyFinalized {" `
    -EndNeedle "`n        }`n"
if ($null -eq $AlreadyFinalizedArm) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the OutcomeAlreadyFinalized projection arm"
} else {
    Test-ContentDoesNotContain "OutcomeAlreadyFinalized never sends any notification" $AlreadyFinalizedArm "discord_notifier" | Out-Null
}

Write-Host ""
Show-Info "--- [9] Evidence-degraded warning is gated on newly_applied ---"
$EvidenceDegradedNotifyArm = Get-ContentBetween -Content $SessionCtrlContent `
    -StartNeedle "Outcome::OutcomeEvidenceDegraded {" `
    -EndNeedle "`n        }`n"
if ($null -eq $EvidenceDegradedNotifyArm) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the OutcomeEvidenceDegraded notification arm"
} else {
    Test-ContentContains "the warning is issued inside an if *newly_applied guard" $EvidenceDegradedNotifyArm "if *newly_applied {" | Out-Null
    Test-ContentContains "the warning uses notify_critical_alert at severity warning" $EvidenceDegradedNotifyArm 'severity: "warning".to_string()' | Out-Null
    Test-ContentContains "the warning uses the documented alert_class" $EvidenceDegradedNotifyArm "autonomous.daily_operation.evidence_degraded" | Out-Null
}

Write-Host ""
Show-Info "--- [10] DatabaseUnavailable/Conflict finalization results never notify ---"
$DbUnavailableArm = Get-ContentBetween -Content $SessionCtrlContent `
    -StartNeedle "Outcome::OutcomeFinalizationDatabaseUnavailable => {" `
    -EndNeedle "`n        }`n"
$ConflictArm = Get-ContentBetween -Content $SessionCtrlContent `
    -StartNeedle "Outcome::OutcomeFinalizationConflict => {" `
    -EndNeedle "`n        }`n"
Test-ContentDoesNotContain "OutcomeFinalizationDatabaseUnavailable never sends any notification" $DbUnavailableArm "discord_notifier" | Out-Null
Test-ContentDoesNotContain "OutcomeFinalizationConflict never sends any notification" $ConflictArm "discord_notifier" | Out-Null

Write-Host ""
Show-Info "--- [11] Notification happens only after the durable tick has already returned ---"
$ProductionTickBody = Get-ContentBetween -Content $SessionCtrlContent `
    -StartNeedle "pub async fn run_durable_session_controller_tick(" `
    -EndNeedle "`n}"
if ($null -eq $ProductionTickBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate run_durable_session_controller_tick's body"
} else {
    $TickCallIdx = $ProductionTickBody.IndexOf("tick_autonomous_daily_coordinator(", [System.StringComparison]::OrdinalIgnoreCase)
    $LogCallIdx = $ProductionTickBody.IndexOf("log_coordinator_outcome(", [System.StringComparison]::OrdinalIgnoreCase)
    if ($TickCallIdx -ge 0 -and $LogCallIdx -ge 0 -and $TickCallIdx -lt $LogCallIdx) {
        Show-Green "  OK -- the coordinator tick completes before log_coordinator_outcome (and therefore any notification) runs"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- log_coordinator_outcome does not strictly follow the coordinator tick call (tick=$TickCallIdx, log=$LogCallIdx)"
    }
}
Test-ContentDoesNotContain "handle_outcome_finalization never sends a notification itself (notification is session_controller's job only)" (Get-ContentBetween -Content $CoordinatorContent -StartNeedle "async fn handle_outcome_finalization(" -EndNeedle "`n}`n`n/// E3.5") "discord_notifier" | Out-Null

Write-Host ""
Show-Info "--- [12] No raw error/debug text enters a notification payload ---"
foreach ($Arm in @(@{Name = "OutcomeFinalized"; Body = $FinalizedArm}, @{Name = "OutcomeEvidenceDegraded"; Body = $EvidenceDegradedNotifyArm})) {
    Test-ContentDoesNotContain "$($Arm.Name)'s payload never formats a raw {:?} debug value" $Arm.Body "{:?}" | Out-Null
    Test-ContentDoesNotContain "$($Arm.Name)'s payload never references a raw err/anyhow value" $Arm.Body "err.to_string()" | Out-Null
}

Write-Host ""
Show-Info "--- [13] No real Discord/network access is introduced by the new E3 test file ---"
$E3TestContent = $null
if (Test-Path $PathE3Test) { $E3TestContent = Get-Content -Raw -Path $PathE3Test }
Test-ContentDoesNotContain "the E3 test file never calls DiscordNotifier::from_env" $E3TestContent "DiscordNotifier::from_env" | Out-Null
Test-ContentDoesNotContain "the E3 test file never constructs a discord.com webhook URL" $E3TestContent "discord.com" | Out-Null
Test-ContentContains "the E3 test file overrides discord_notifier with a loopback sink or a no-op" $E3TestContent "DiscordNotifier::from_url" | Out-Null

Write-Host ""
Show-Info "--- [14] Exactly the two authorized E4 read-only routes exist; neither invokes coordinator finalization; no GUI ---"
$RoutesContent = $null
if (Test-Path $PathRoutesRs) { $RoutesContent = Get-Content -Raw -Path $PathRoutesRs }
$ApiTypesContent = $null
if (Test-Path $PathApiTypesRs) { $ApiTypesContent = Get-Content -Raw -Path $PathApiTypesRs }
$PathE4RouteRs = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\autonomous_daily_operations.rs"
$E4RouteContent = $null
if (Test-FileExists "E4 route module" $PathE4RouteRs) {
    $E4RouteContent = Get-Content -Raw -Path $PathE4RouteRs
}
Test-ContentContains "routes.rs mounts the single daily-operation GET route" $RoutesContent '"/api/v1/autonomous/daily-operation",' | Out-Null
Test-ContentContains "routes.rs mounts the history daily-operations GET route" $RoutesContent '"/api/v1/autonomous/daily-operations",' | Out-Null
Test-ContentDoesNotContain "routes.rs never mounts a POST/PUT/PATCH/DELETE for the single E4 route" $RoutesContent 'post(autonomous_daily_operation)' | Out-Null
Test-ContentDoesNotContain "routes.rs never mounts a POST/PUT/PATCH/DELETE for the history E4 route" $RoutesContent 'post(autonomous_daily_operations)' | Out-Null
Test-ContentDoesNotContain "the E4 route module never calls the E2B finalizer" $E4RouteContent "classify_and_finalize_autonomous_daily_operation(" | Out-Null
Test-ContentDoesNotContain "the E4 route module never calls the terminal finalization CAS directly" $E4RouteContent "finalize_autonomous_daily_operation(" | Out-Null
Test-ContentDoesNotContain "the E4 route module never calls a coordinator tick function" $E4RouteContent "tick_autonomous_daily_coordinator(" | Out-Null
Test-ContentDoesNotContain "the E4 route module never sends a notification" $E4RouteContent "discord_notifier" | Out-Null
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1: the original "no GUI file touched"
# working-tree check below was a point-in-time E3-scope assertion, valid
# only while Phase F (GUI work) was not yet authorized. Phase F is now open
# and legitimately adds/edits GUI files on every subsequent patch -- a live
# uncommitted-working-tree check here would misattribute F1/F2/F3's own
# authorized GUI work to this guard's unrelated E3 scope. E3's/E4's real
# invariants (above) are unchanged; GUI-scope protection for Phase F itself
# now lives in
# validate_autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.ps1
# (and its F2/F3 successors).
Show-Green "  OK -- GUI-touch check superseded by Phase F1's own dedicated guard (Phase F now open)"

Write-Host ""
Show-Info "--- [15] README truth: E1/E2A/E2B/E3 accepted, E4 implementation-complete-awaiting-acceptance, Phase E/Bundle 3 open ---"
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
# patch) -- the durable check going forward requires E1/E2A/E2B/E3 recorded
# as accepted and E4 recorded as implementation-complete-awaiting-acceptance,
# never accepted itself. The prior "E3 not yet accepted" forbidden-claim
# entries are retired -- E3 acceptance is now the truthful, required state.
#
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-
# INTEGRITY-REPAIR: "Phase E: ACCEPTED" is retired from this forbidden list --
# Phase E is now genuinely accepted-complete, truthfully recorded in
# README.md's own status block ("PHASE E: ACCEPTED -- COMPLETE"); this check
# was calibrated while Phase E was still open and would otherwise
# permanently misfire on that now-required, truthful status line.
$ForbiddenReadmeClaims = @(
    "Phase E: CLOSED",
    "Phase E: COMPLETE",
    "Phase E is complete",
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
Test-ContentContains "README.md records E1 as accepted" $ReadmeContent "E1 is now accepted" | Out-Null
Test-ContentContains "README.md records E2A as accepted" $ReadmeContent "E2A (plus both repairs) is now accepted" | Out-Null
Test-ContentContains "README.md records E2B as accepted" $ReadmeContent "E2B is now accepted" | Out-Null
Test-ContentContains "README.md records E3 as accepted" $ReadmeContent "E3 is now accepted" | Out-Null
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE:
# E4 is now accepted (recorded by the operator ahead of this patch) -- the
# durable check going forward records E1/E2A/E2B/E3/E4 as accepted and E5 as
# implementation-complete-awaiting-acceptance, matching this patch's own
# required documentation truth.
Test-ContentContains "README.md records E4 as accepted" $ReadmeContent "E4 (plus both repairs and their test suites) is now accepted" | Out-Null
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-
# INTEGRITY-REPAIR: E5 is now genuinely accepted (Phase E accepted complete
# in full) -- the prior needle ("is implementation complete, awaiting
# ChatGPT and operator acceptance") was calibrated while E5 itself was still
# open, and coincidentally also matched unrelated F2/F3 status prose
# elsewhere in README.md rather than actually asserting E5's own status. The
# durable check going forward requires the truthful, current wording.
Test-ContentContains "README.md records E5 as accepted" $ReadmeContent "E5 (plus this" | Out-Null

Write-Host ""
Show-Info "--- [16] New E3 spec doc and scenario test file exist and are nonempty ---"
if (Test-FileExists "E3 spec doc" $PathE3Spec) {
    $E3SpecContent = Get-Content -Raw -Path $PathE3Spec
    if ($E3SpecContent.Length -gt 2000) {
        Show-Green "  OK -- E3 spec doc is nonempty ($($E3SpecContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E3 spec doc is suspiciously short"
    }
}
if (Test-FileExists "E3 scenario test file" $PathE3Test) {
    if ($E3TestContent.Length -gt 2000) {
        Show-Green "  OK -- E3 scenario test file is nonempty ($($E3TestContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E3 scenario test file is suspiciously short"
    }
}

Write-Host ""
Show-Info "--- [17] handle_outcome_finalization gates on matching_local_runtime_active before policy resolution and blocker persistence (E3 repair) ---"
$HandleOutcomeFinalizationBody = Get-ContentBetween -Content $CoordinatorContent `
    -StartNeedle "async fn handle_outcome_finalization(" `
    -EndNeedle "`n}`n`n/// E3.5"
if ($null -eq $HandleOutcomeFinalizationBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate handle_outcome_finalization's body"
} else {
    $ContextIdx    = $HandleOutcomeFinalizationBody.IndexOf("matching_local_runtime_active: matching_local_runtime_active(state, &operation).await", [System.StringComparison]::OrdinalIgnoreCase)
    $GateIdx       = $HandleOutcomeFinalizationBody.IndexOf("if context.matching_local_runtime_active {", [System.StringComparison]::OrdinalIgnoreCase)
    $AwaitingIdx   = $HandleOutcomeFinalizationBody.IndexOf("return Ok(AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization);", [System.StringComparison]::OrdinalIgnoreCase)
    $ConfigIdx     = $HandleOutcomeFinalizationBody.IndexOf("build_multi_symbol_runtime_config_from_env()", [System.StringComparison]::OrdinalIgnoreCase)
    $RuntimeCtxIdx = $HandleOutcomeFinalizationBody.IndexOf("resolve_autonomous_runtime_context(state).await", [System.StringComparison]::OrdinalIgnoreCase)
    $FirstPersistIdx = $HandleOutcomeFinalizationBody.IndexOf("persist_autonomous_daily_finalization_blocker(", [System.StringComparison]::OrdinalIgnoreCase)
    $ClassifyIdx   = $HandleOutcomeFinalizationBody.IndexOf("classify_and_finalize_autonomous_daily_operation(", [System.StringComparison]::OrdinalIgnoreCase)

    if ($ContextIdx -lt 0 -or $GateIdx -lt 0 -or $AwaitingIdx -lt 0) {
        $script:Violations++
        Show-Red "  FAIL -- handle_outcome_finalization does not contain the expected early matching-runtime gate returning AwaitingOutcomeFinalization"
    } elseif ($ConfigIdx -lt 0 -or $RuntimeCtxIdx -lt 0 -or $FirstPersistIdx -lt 0 -or $ClassifyIdx -lt 0) {
        $script:Violations++
        Show-Red "  FAIL -- could not locate the four gated calls (config resolution, runtime-context resolution, blocker persistence, classify) inside handle_outcome_finalization"
    } elseif ($GateIdx -lt $ContextIdx) {
        $script:Violations++
        Show-Red "  FAIL -- the matching-runtime gate must appear after context is computed"
    } elseif ($GateIdx -ge $ConfigIdx -or $GateIdx -ge $RuntimeCtxIdx -or $GateIdx -ge $FirstPersistIdx -or $GateIdx -ge $ClassifyIdx) {
        $script:Violations++
        Show-Red "  FAIL -- the matching-runtime gate does not precede config/runtime-context resolution, blocker persistence, and classify_and_finalize_autonomous_daily_operation (gate=$GateIdx, config=$ConfigIdx, runtime_ctx=$RuntimeCtxIdx, persist=$FirstPersistIdx, classify=$ClassifyIdx)"
    } elseif ($AwaitingIdx -lt $GateIdx -or $AwaitingIdx -gt $ConfigIdx) {
        $script:Violations++
        Show-Red "  FAIL -- the early return must produce AwaitingOutcomeFinalization strictly between the gate and the first policy-resolution call"
    } else {
        Show-Green "  OK -- handle_outcome_finalization returns AwaitingOutcomeFinalization before any config/runtime-context resolution, blocker persistence, or classification call when a matching local runtime is active"
    }
}

Write-Host ""
Show-Info "--- [18] persist_autonomous_daily_finalization_blocker refuses persistence via a shared eligibility gate (E3 repair) ---"
$PersistBlockerFnBody2 = Get-ContentBetween -Content $OutcomeContent `
    -StartNeedle "pub async fn persist_autonomous_daily_finalization_blocker(" `
    -EndNeedle "`n}"
if ($null -eq $PersistBlockerFnBody2) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate persist_autonomous_daily_finalization_blocker's body (repair)"
} else {
    Test-ContentContains "persist_autonomous_daily_finalization_blocker's signature takes an AutonomousDailyFinalizationContext" $PersistBlockerFnBody2 "context: AutonomousDailyFinalizationContext," | Out-Null
    $EligibilityIdx = $PersistBlockerFnBody2.IndexOf("finalization_blocker_persistence_eligible(operation, &context)", [System.StringComparison]::OrdinalIgnoreCase)
    $NotEligibleIdx = $PersistBlockerFnBody2.IndexOf("return Ok(AutonomousDailyFinalizationOutcome::NotEligible);", [System.StringComparison]::OrdinalIgnoreCase)
    $ApplyIdx       = $PersistBlockerFnBody2.IndexOf("apply_evidence_degraded_blocker(", [System.StringComparison]::OrdinalIgnoreCase)
    if ($EligibilityIdx -lt 0 -or $NotEligibleIdx -lt 0 -or $ApplyIdx -lt 0) {
        $script:Violations++
        Show-Red "  FAIL -- persist_autonomous_daily_finalization_blocker does not gate on a shared eligibility check before writing"
    } elseif ($EligibilityIdx -ge $ApplyIdx -or $NotEligibleIdx -ge $ApplyIdx) {
        $script:Violations++
        Show-Red "  FAIL -- the eligibility check/refusal must precede the real write call (eligibility=$EligibilityIdx, not_eligible=$NotEligibleIdx, apply=$ApplyIdx)"
    } else {
        Show-Green "  OK -- persist_autonomous_daily_finalization_blocker refuses persistence (NotEligible, zero writes) before ever calling apply_evidence_degraded_blocker"
    }
}
Test-ContentContains "the shared eligibility gate function exists" $OutcomeContent "fn finalization_blocker_persistence_eligible(" | Out-Null
Test-ContentContains "the eligibility gate refuses when a matching local runtime is active" $OutcomeContent "if context.matching_local_runtime_active {`n        return false;`n    }" | Out-Null

# Every coordinator call site to the wrapper must thread the finalization
# context through -- matched by scanning each call site's argument list
# (opening paren to the first following ');') for the word "context", so
# this check survives rustfmt reflowing the exact call-site formatting.
$WrapperCallRegex = [regex]'persist_autonomous_daily_finalization_blocker\(([\s\S]*?)\);'
$WrapperCallMatches = $null
if ($null -ne $CoordinatorContent) { $WrapperCallMatches = $WrapperCallRegex.Matches($CoordinatorContent) }
if ($null -eq $WrapperCallMatches -or $WrapperCallMatches.Count -lt 2) {
    $script:Violations++
    Show-Red "  FAIL -- expected at least 2 coordinator call sites to persist_autonomous_daily_finalization_blocker, found $(if ($WrapperCallMatches) { $WrapperCallMatches.Count } else { 0 })"
} else {
    $AllCallSitesThreadContext = $true
    foreach ($CallSite in $WrapperCallMatches) {
        if ($CallSite.Groups[1].Value -notmatch '(?i)\bcontext\b') {
            $AllCallSitesThreadContext = $false
        }
    }
    if ($AllCallSitesThreadContext) {
        Show-Green "  OK -- every persist_autonomous_daily_finalization_blocker call site in the coordinator threads the finalization context ($($WrapperCallMatches.Count) call sites)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- at least one persist_autonomous_daily_finalization_blocker call site does not pass context"
    }
}

Write-Host ""
Show-Info "--- [19] The required coordinator-level DB-backed proof test exists by exact name ---"
Test-ContentContains "ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification exists in the E3 test file" $E3TestContent "async fn ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification()" | Out-Null

Write-Host ""
Show-Info "--- [20] A direct E2B store-level wrapper eligibility proof test exists ---"
$E2BTestContent = $null
if (Test-FileExists "E2B classifier/finalization scenario test file" $PathE2BTest) {
    $E2BTestContent = Get-Content -Raw -Path $PathE2BTest
}
$E2BWrapperTestBody = Get-ContentBetween -Content $E2BTestContent `
    -StartNeedle "async fn store_59_persist_finalization_blocker_refuses_when_matching_runtime_active(" `
    -EndNeedle "`n}"
if ($null -eq $E2BWrapperTestBody) {
    $script:Violations++
    Show-Red "  FAIL -- store_59_persist_finalization_blocker_refuses_when_matching_runtime_active not found in the E2B test file"
} else {
    Test-ContentContains "the E2B wrapper proof test calls persist_autonomous_daily_finalization_blocker directly" $E2BWrapperTestBody "persist_autonomous_daily_finalization_blocker(" | Out-Null
    Test-ContentContains "the E2B wrapper proof test supplies a matching-runtime-active context" $E2BWrapperTestBody "matching_local_runtime_active: true," | Out-Null
    Test-ContentContains "the E2B wrapper proof test asserts NotEligible" $E2BWrapperTestBody "assert_eq!(outcome, AutonomousDailyFinalizationOutcome::NotEligible);" | Out-Null
}

Write-Host ""
Show-Info "--- Ledger truth ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
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
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
