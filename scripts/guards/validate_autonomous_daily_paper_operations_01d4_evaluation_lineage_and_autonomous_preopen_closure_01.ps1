# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-
# PREOPEN-CLOSURE-01 -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the D4 repair layered on top of the already-
# accepted D3/D4 patch (whose own invariants remain validated by
# validate_autonomous_daily_paper_operations_01d_phase_d_closure.ps1,
# unmodified by this patch). No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test -- pure text/source
# validation only.
#
# Checks:
#   [1]  The completed-bar dispatch-claim path derives its evaluation
#        identity via the shared AppState::derive_strategy_signal_evaluation_id
#        helper -- never a second, independently-derived identity algorithm.
#   [2]  The claim path durably confirms an exact strategy_signal_evaluations
#        row (via mqk_db::fetch_strategy_signal_evaluation) before a claim may
#        complete.
#   [3]  claim_and_dispatch_observed_bar never passes a bare `None` evaluation
#        id to complete_autonomous_daily_bar_dispatch on the success path --
#        it always passes the confirmed Some(expected_evaluation_id).
#   [4]  The completion write's Result is captured and examined (never
#        `.await?;` and silently assumed to have succeeded).
#   [5]  The concurrency proof's decoy bar identity is a distinct `decoy_ts`,
#        never the claimed bar's own `expected_ts`.
#   [6]  The full-day lifecycle test's preopen phase never calls
#        apply_transition (no manual-intervention unstick in the happy path).
#   [7]  The full-day lifecycle test proves a real production-adapter
#        PrepareDataOnly + BarObserved outcome at the preopen instant.
#   [8]  A supervised-task test exists proving the real production adapter is
#        invoked under an injected (non-`Utc::now()`) clock.
#   [9]  The master patch ledger records legitimate D4/Phase D acceptance
#        while Bundle 3 remains open, Phase E is not marked complete, and the
#        unattended soak / live-capital readiness claims remain absent.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01d4_evaluation_lineage_and_autonomous_preopen_closure_01.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathStateRs     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$PathDriverRs    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_completed_bar_driver.rs"
$PathIntegration = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_phase_d_integration_01.rs"
$PathTaskTests   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_completed_bar_task_01.rs"
$PathClosureDoc  = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01d_phase_d_closure.md"
$PathLedger      = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-PREOPEN-CLOSURE-01 Validator"
Write-Host "============================================================"

$StateContent = $null
if (Test-FileExists "state.rs" $PathStateRs) {
    $StateContent = Get-Content -Raw -Path $PathStateRs
}
$DriverContent = $null
if (Test-FileExists "autonomous_completed_bar_driver.rs" $PathDriverRs) {
    $DriverContent = Get-Content -Raw -Path $PathDriverRs
}
$IntegrationContent = $null
if (Test-FileExists "Phase D integration scenario test" $PathIntegration) {
    $IntegrationContent = Get-Content -Raw -Path $PathIntegration
}
$TaskTestsContent = $null
if (Test-FileExists "completed-bar task scenario test" $PathTaskTests) {
    $TaskTestsContent = Get-Content -Raw -Path $PathTaskTests
}

Write-Host ""
Show-Info "--- [1] Shared evaluation-identity helper, never a second algorithm ---"
Test-ContentContains "state.rs defines derive_strategy_signal_evaluation_id" $StateContent "fn derive_strategy_signal_evaluation_id" | Out-Null
Test-ContentContains "record_signal_evaluation calls the shared helper" $StateContent "Self::derive_strategy_signal_evaluation_id(" | Out-Null
Test-ContentContains "the completed-bar driver calls the shared helper" $DriverContent "AppState::derive_strategy_signal_evaluation_id(" | Out-Null

Write-Host ""
Show-Info "--- [2] Exact durable evaluation-row confirmation before completion ---"
$ClaimFnBody = Get-ContentBetween -Content $DriverContent `
    -StartNeedle "async fn claim_and_dispatch_observed_bar(" `
    -EndNeedle "`n/// REPAIR 4: the mandatory authoritative re-read"
if ($null -eq $ClaimFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate claim_and_dispatch_observed_bar's body to scope checks [2]-[4]"
} else {
    Test-ContentContains "claim path calls fetch_strategy_signal_evaluation" $ClaimFnBody "fetch_strategy_signal_evaluation(" | Out-Null
    Test-ContentContains "claim path checks decision_stage" $ClaimFnBody "decision_stage ==" | Out-Null

    Write-Host ""
    Show-Info "--- [3] Completion never passes a bare None evaluation id on success ---"
    Test-ContentContains "claim path passes the confirmed Some(expected_evaluation_id)" $ClaimFnBody "Some(expected_evaluation_id)" | Out-Null
    Test-ContentDoesNotContain "claim path does not pass a bare None to complete_autonomous_daily_bar_dispatch" $ClaimFnBody "bar_end_ts,`n                input.now_utc,`n                None,`n            )" | Out-Null

    Write-Host ""
    Show-Info "--- [4] Completion Result is captured and examined, never ignored ---"
    Test-ContentContains "claim path captures the completion Result" $ClaimFnBody "let completion_result" | Out-Null
    Test-ContentContains "claim path matches on the captured completion Result" $ClaimFnBody "match completion_result" | Out-Null
}
Test-ContentContains "a distinguishable unconfirmed-completion outcome exists" $DriverContent "DispatchCompletionUnconfirmed" | Out-Null
Test-ContentContains "a distinguishable missing-evaluation-evidence outcome exists" $DriverContent "DispatchEvaluationEvidenceMissing" | Out-Null

Write-Host ""
Show-Info "--- [5] Concurrency decoy uses a distinct bar identity from claim A ---"
Test-ContentContains "the integration test defines a distinct decoy_ts" $IntegrationContent "let decoy_ts = expected_ts - 300;" | Out-Null
Test-ContentDoesNotContain "the decoy mailbox deposit does not reuse claim A's own expected_ts" $IntegrationContent "now_tick: 999,`n            end_ts: expected_ts," | Out-Null

Write-Host ""
Show-Info "--- [6] The full-day lifecycle preopen phase never manually unsticks ---"
$FullDayBody = Get-ContentBetween -Content $IntegrationContent `
    -StartNeedle "async fn phase_d_full_day_lifecycle() {" `
    -EndNeedle "`n#[tokio::test]"
if ($null -eq $FullDayBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate phase_d_full_day_lifecycle's body to scope checks [6]-[7]"
} else {
    Test-ContentDoesNotContain "phase_d_full_day_lifecycle never calls apply_transition" $FullDayBody "apply_transition(" | Out-Null
    Test-ContentContains "phase_d_full_day_lifecycle asserts preopen never enters manual_intervention_required" $FullDayBody "STATE_MANUAL_INTERVENTION_REQUIRED,`n        `"PD: real autonomous preopen must never enter manual_intervention_required`"" | Out-Null

    Write-Host ""
    Show-Info "--- [7] Real production-adapter PrepareDataOnly + BarObserved proof at preopen ---"
    Test-ContentContains "preopen proof asserts mode: PrepareDataOnly" $FullDayBody "mode: AutonomousCompletedBarDriverMode::PrepareDataOnly," | Out-Null
    Test-ContentContains "preopen proof asserts outcome: BarObserved" $FullDayBody "outcome: AutonomousCompletedBarDriverOutcome::BarObserved {" | Out-Null
    Test-ContentContains "preopen proof asserts zero provider calls" $FullDayBody "provider_poll_attempt_count, 0," | Out-Null
}

Write-Host ""
Show-Info "--- [8] Supervised task proven under an injected (non-production) clock ---"
Test-ContentContains "AppState exposes a completed-bar task clock override test seam" $StateContent "fn set_completed_bar_task_clock_override_for_test" | Out-Null
Test-ContentContains "the production tick adapter reads the clock seam" $StateContent "completed_bar_task_tick_clock" | Out-Null
Test-ContentContains "a task-level test drives the real adapter under the injected clock" $TaskTestsContent "set_completed_bar_task_clock_override_for_test" | Out-Null
Test-ContentContains "that test spawns the real production task (not a fake tick factory)" $TaskTestsContent "spawn_autonomous_completed_bar_driver_task(Arc::clone(&st))" | Out-Null
Test-ContentContains "that test awaits shutdown of the same supervised task" $TaskTestsContent "cancel_and_wait_completed_bar_task_for_shutdown" | Out-Null

Write-Host ""
Show-Info "--- [9] Ledger records legitimate D4/Phase D acceptance; Bundle 3/Phase E/soak/live-capital truth is not overclaimed ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
$ClosureDocContent = $null
if (Test-FileExists "Phase D closure document" $PathClosureDoc) {
    $ClosureDocContent = Get-Content -Raw -Path $PathClosureDoc
}
# Obsolete prohibition removed (was: "ledger does not mark D4 accepted"/"closure doc does not mark
# D4 accepted") -- D4 and Phase D acceptance were independently granted
# (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-ACCEPTANCE) and recording that truthfully in the ledger is
# legitimate, required content, not a violation. The closure doc itself
# (docs/specs/autonomous_daily_paper_operations_01d_phase_d_closure.md) is out of this patch's file
# scope and is not expected to be edited to add an acceptance claim -- it is left as the
# implementation-complete record it always was.
Test-ContentContains "ledger records D4 acceptance" $LedgerContent "D4: ACCEPTED" | Out-Null
Test-ContentContains "ledger records Phase D acceptance" $LedgerContent "PHASE D: ACCEPTED" | Out-Null
Test-ContentDoesNotContain "ledger does not contain 'Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED'" $LedgerContent "Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED" | Out-Null
Test-ContentDoesNotContain "ledger does not contain 'Phase D: CLOSED'" $LedgerContent "Phase D: CLOSED" | Out-Null
$ForbiddenPhaseEClaimsD4Guard = @(
    "PHASE E: CLOSED",
    "PHASE E: COMPLETE",
    "phase e runtime implementation: complete",
    "phase e outcome classifier implemented",
    "phase e is wired",
    "durable daily outcome classification is live"
)
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE:
# "PHASE E: IMPLEMENTATION COMPLETE" alone is retired from this forbidden
# list, matching the identical reconciliation in
# validate_autonomous_daily_paper_operations_01e_outcome_contract.ps1. This
# check was calibrated when D4 was accepted and Phase E runtime work had not
# started. As of E5, "PHASE E: IMPLEMENTATION COMPLETE -- AWAITING CHATGPT
# AND OPERATOR ACCEPTANCE" is the honest, required, non-closure status --
# never used bare as a closure claim (still forbidden above via "PHASE E:
# CLOSED"/"PHASE E: COMPLETE").
foreach ($Phrase in $ForbiddenPhaseEClaimsD4Guard) {
    Test-ContentDoesNotContain "ledger does not contain forbidden Phase E claim '$Phrase'" $LedgerContent $Phrase | Out-Null
}
Test-ContentContains "ledger states the unattended soak has not started" $LedgerContent "soak: NOT STARTED" | Out-Null
Test-ContentContains "ledger states live capital is not ready" $LedgerContent "live capital: NOT READY" | Out-Null
$ForbiddenLiveClaimsD4Guard = @(
    "live capital is ready",
    "live capital: ready",
    "approved for live capital",
    "live trading is approved",
    "cleared for live capital",
    "ready for live capital"
)
foreach ($Phrase in $ForbiddenLiveClaimsD4Guard) {
    Test-ContentDoesNotContain "ledger does not contain forbidden live-capital-readiness claim '$Phrase'" $LedgerContent $Phrase | Out-Null
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-PREOPEN-CLOSURE-01 evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
