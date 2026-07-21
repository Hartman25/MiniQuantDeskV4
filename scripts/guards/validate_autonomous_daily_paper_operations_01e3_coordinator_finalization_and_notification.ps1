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
#   [14] No E4 API route, response field, or GUI surface is introduced.
#   [15] README.md / README_TECHNICAL.md never mark E3, Phase E, or Bundle 3
#        as accepted/complete, and record E1/E2A/E2B as accepted with E3
#        implementation-complete-awaiting-acceptance.
#   [16] The new E3 spec doc and scenario test file both exist and are
#        nonempty.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

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
Show-Info "--- [14] No E4 API route, response field, or GUI surface is introduced ---"
$RoutesContent = $null
if (Test-Path $PathRoutesRs) { $RoutesContent = Get-Content -Raw -Path $PathRoutesRs }
$ApiTypesContent = $null
if (Test-Path $PathApiTypesRs) { $ApiTypesContent = Get-Content -Raw -Path $PathApiTypesRs }
Test-ContentDoesNotContain "routes.rs never references a daily-operation read route" $RoutesContent "daily-operation" | Out-Null
Test-ContentDoesNotContain "api_types.rs never references the outcome classifier module" $ApiTypesContent "autonomous_daily_outcome" | Out-Null
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
Show-Info "--- [15] README truth: E1/E2A/E2B accepted, E3 awaiting acceptance, Phase E/Bundle 3 open ---"
$ReadmeContent = $null
if (Test-FileExists "README.md" $PathReadme) {
    $ReadmeContent = Get-Content -Raw -Path $PathReadme
}
$ReadmeTechContent = $null
if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) {
    $ReadmeTechContent = Get-Content -Raw -Path $PathReadmeTech
}
$ForbiddenReadmeClaims = @(
    "Phase E: CLOSED",
    "Phase E: COMPLETE",
    "Phase E is complete",
    "Bundle 3: CLOSED",
    "Bundle 3 is complete",
    "E3: ACCEPTED",
    "E3 is accepted",
    "E3 is complete",
    "E3: COMPLETE",
    "E4 implemented",
    "E4: ACCEPTED",
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
Test-ContentContains "README.md records E3 as implementation-complete-awaiting-acceptance" $ReadmeContent "awaiting ChatGPT and operator acceptance" | Out-Null

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
