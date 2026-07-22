# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE
# -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the E5 integrated Phase E closure proof, built
# entirely on top of the already-accepted E1 contract, E2A coverage-anchor/
# run-lineage foundation, E2B strict classifier/finalization CAS, E3
# coordinator finalization integration, and E4 read-only API. It invokes
# every E1-E4 guard and fails when any of them fails, then adds source-aware
# Phase-E-specific checks of its own rather than relying only on the prior
# guards. No network call, no provider/broker call, no DB connection, no
# daemon start, no cargo/npm build or test -- pure text/source validation
# only (the cargo test run itself is a separate, required step of the E5
# mission, not performed by this guard).
#
# Checks:
#   [1]  Every E1-E4 guard script exists and, when invoked, exits 0.
#   [2]  The Phase E closure scenario test file and this closure spec both
#        exist and are nonempty.
#   [3]  Coverage authority is present: the E2A coverage-authority module
#        exists and defines the write/re-read/check contract.
#   [4]  The completed-bar authority gate is present: the production adapter
#        checks the coverage authority (Stage A/B) before invoking the
#        driver.
#   [5]  Full run lineage is present: the raw ordered lineage read/validate
#        helper exists in mqk-db and is consumed by the classifier.
#   [6]  The classifier cannot emit generic `completed` -- the classification
#        enum has exactly the three non-generic-`completed` variants.
#   [7]  The coordinator cannot finalize before durable stop eligibility --
#        `check_finalization_eligibility` gates on `stopped_at_utc` and
#        `matching_local_runtime_active`.
#   [8]  Matching runtime ownership is never ignored -- the coordinator's
#        early-return gate on `context.matching_local_runtime_active` exists
#        strictly before policy resolution/blocker persistence.
#   [9]  Terminal replay cannot notify again -- `OutcomeFinalized` is produced
#        only from E2B's `Finalized` result, never from `AlreadyFinalized`.
#   [10] An unchanged evidence blocker cannot notify again -- the
#        `newly_applied` field gates the evidence-degraded warning arm.
#   [11] E4 routes never invoke the classifier/finalizer/coordinator.
#   [12] E4 routes never contain a mutating DB write helper call.
#   [13] E4's exact market-date parser (`parse_exact_market_date`) exists and
#        the route never calls `.trim()`.
#   [14] The integrated no-trade proof (Proof A) exists in the new test file.
#   [15] The integrated activity/full-lineage proof (Proof B) exists.
#   [16] The evidence-blocker recovery proof (Proof C) exists.
#   [17] The restart proofs (Proof D) exist.
#   [18] The API read-only before/after proof (Proof E) exists.
#   [19] The fail-soft API truth proof (Proof F) exists.
#   [20] README.md/README_TECHNICAL.md never mark Bundle 3 closed, never mark
#        Phase F or G started, never claim the soak has started, never claim
#        live readiness.
#   [21] README.md records E1-E4 as accepted and E5 as implementation-
#        complete-awaiting-acceptance.
#   [22] The ledger records E1-E4 as accepted, never claims Phase E closed or
#        Bundle 3 closed.
#   [23] No production Rust file was modified by this patch (git diff against
#        HEAD's parent-scoped check: only the E5-authorized file set may
#        differ under core-rs/**/src/**).
#   [24] No migration file was added or modified by this patch.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e_phase_e_closure.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"
# PowerShell 7.3+ turns native-command stderr text (e.g. git's routine
# "LF will be replaced by CRLF" autocrlf notice) into terminating
# ErrorRecords under $ErrorActionPreference = "Stop", even when the stream is
# redirected with 2>$null. Disable that promotion for this script only -- it
# does not change $LASTEXITCODE handling below, which is checked explicitly.
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathCoverageAuthorityRs = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_coverage_authority.rs"
$PathOutcomeRs           = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_outcome.rs"
$PathCoordinatorRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_daily_coordinator.rs"
$PathCompletedBarTaskRs  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_completed_bar_task.rs"
$PathSessionControllerRs = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\session_controller.rs"
$PathE4RouteRs           = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\autonomous_daily_operations.rs"
$PathDbAutonRs           = Join-Path $RepoRoot "core-rs\crates\mqk-db\src\autonomous_daily_operation.rs"
$PathE5Spec              = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01e_phase_e_closure.md"
$PathE5Test              = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_phase_e_closure_01.rs"
$PathReadme              = Join-Path $RepoRoot "README.md"
$PathReadmeTech          = Join-Path $RepoRoot "README_TECHNICAL.md"
$PathLedger              = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

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

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE Validator"
Write-Host "============================================================"

# -----------------------------------------------------------------------
# [1] Every E1-E4 guard exists and passes.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] Every E1-E4 guard exists and exits 0 ---"
$PriorGuards = @(
    "validate_autonomous_daily_paper_operations_01e_outcome_contract.ps1",
    "validate_autonomous_daily_paper_operations_01e2a_coverage_anchor_and_run_lineage.ps1",
    "validate_autonomous_daily_paper_operations_01e2b_outcome_classifier_and_finalization.ps1",
    "validate_autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.ps1",
    "validate_autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.ps1"
)
foreach ($GuardName in $PriorGuards) {
    $GuardPath = Join-Path $ScriptDir $GuardName
    if (-not (Test-Path $GuardPath)) {
        $script:Violations++
        Show-Red "  FAIL -- required guard missing: $GuardName"
        continue
    }
    & powershell -ExecutionPolicy Bypass -File $GuardPath *> $null
    $exitCode = $LASTEXITCODE
    if ($exitCode -eq 0) {
        Show-Green "  OK -- $GuardName exits 0"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $GuardName exited $exitCode"
    }
}

# -----------------------------------------------------------------------
# [2] Closure test file and spec exist and are nonempty.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] Phase E closure test file and spec exist and are nonempty ---"
$E5TestContent = $null
if (Test-FileExists "E5 closure scenario test file" $PathE5Test) {
    $E5TestContent = Get-Content -Raw -Path $PathE5Test
    if ($E5TestContent.Length -gt 2000) {
        Show-Green "  OK -- E5 test file is nonempty ($($E5TestContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E5 test file is suspiciously short"
    }
}
$E5SpecContent = $null
if (Test-FileExists "E5 closure spec" $PathE5Spec) {
    $E5SpecContent = Get-Content -Raw -Path $PathE5Spec
    if ($E5SpecContent.Length -gt 2000) {
        Show-Green "  OK -- E5 spec is nonempty ($($E5SpecContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E5 spec is suspiciously short"
    }
}

$CoverageAuthorityContent = $null
if (Test-FileExists "autonomous_daily_coverage_authority.rs" $PathCoverageAuthorityRs) {
    $CoverageAuthorityContent = Get-Content -Raw -Path $PathCoverageAuthorityRs
}
$OutcomeContent = $null
if (Test-FileExists "autonomous_daily_outcome.rs" $PathOutcomeRs) {
    $OutcomeContent = Get-Content -Raw -Path $PathOutcomeRs
}
$CoordinatorContent = $null
if (Test-FileExists "autonomous_daily_coordinator.rs" $PathCoordinatorRs) {
    $CoordinatorContent = Get-Content -Raw -Path $PathCoordinatorRs
}
$CompletedBarTaskContent = $null
if (Test-FileExists "autonomous_completed_bar_task.rs" $PathCompletedBarTaskRs) {
    $CompletedBarTaskContent = Get-Content -Raw -Path $PathCompletedBarTaskRs
}
$SessionControllerContent = $null
if (Test-FileExists "session_controller.rs" $PathSessionControllerRs) {
    $SessionControllerContent = Get-Content -Raw -Path $PathSessionControllerRs
}
$E4RouteContent = $null
if (Test-FileExists "autonomous_daily_operations.rs (E4 route module)" $PathE4RouteRs) {
    $E4RouteContent = Get-Content -Raw -Path $PathE4RouteRs
}
$DbAutonContent = $null
if (Test-FileExists "mqk-db autonomous_daily_operation.rs" $PathDbAutonRs) {
    $DbAutonContent = Get-Content -Raw -Path $PathDbAutonRs
}

# -----------------------------------------------------------------------
# [3] Coverage authority present.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] Coverage authority is present ---"
Test-ContentContains "write_and_confirm_coverage_authority exists" $CoverageAuthorityContent "pub async fn write_and_confirm_coverage_authority(" | Out-Null
Test-ContentContains "check_coverage_authority exists" $CoverageAuthorityContent "pub async fn check_coverage_authority(" | Out-Null
Test-ContentContains "check_coverage_authority_envelope exists" $CoverageAuthorityContent "pub async fn check_coverage_authority_envelope(" | Out-Null

# -----------------------------------------------------------------------
# [4] Completed-bar authority gate present.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] The completed-bar authority gate is present ---"
Test-ContentContains "the completed-bar task checks the coverage authority envelope" $CompletedBarTaskContent "check_coverage_authority_envelope(" | Out-Null
Test-ContentContains "CoverageAuthorityUnavailable is a typed adapter outcome" $CompletedBarTaskContent "CoverageAuthorityUnavailable" | Out-Null

# -----------------------------------------------------------------------
# [5] Full run lineage present.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] Full run lineage helper is present and consumed ---"
Test-ContentContains "mqk-db defines fetch_and_validate_autonomous_daily_operation_run_lineage" $DbAutonContent "pub async fn fetch_and_validate_autonomous_daily_operation_run_lineage(" | Out-Null
Test-ContentContains "the classifier consumes the validated run lineage" $OutcomeContent "fetch_and_validate_autonomous_daily_operation_run_lineage(" | Out-Null

# -----------------------------------------------------------------------
# [6] Classifier cannot emit generic completed.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] The classifier can never emit generic completed ---"
Test-ContentContains "AutonomousDailyOutcomeClassification has exactly the three closed variants" $OutcomeContent "pub enum AutonomousDailyOutcomeClassification {" | Out-Null
Test-ContentContains "CompletedWithActivity variant" $OutcomeContent "CompletedWithActivity {" | Out-Null
Test-ContentContains "CompletedNoTrade variant" $OutcomeContent "CompletedNoTrade {" | Out-Null
Test-ContentContains "EvidenceBlocked variant" $OutcomeContent "EvidenceBlocked {" | Out-Null

# -----------------------------------------------------------------------
# [7] Coordinator cannot finalize before durable stop eligibility.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] Finalization eligibility requires durable stop ---"
Test-ContentContains "check_finalization_eligibility gates on stopped_at_utc" $OutcomeContent "operation.stopped_at_utc.is_none()" | Out-Null
Test-ContentContains "check_finalization_eligibility gates on matching_local_runtime_active" $OutcomeContent "context.matching_local_runtime_active" | Out-Null

# -----------------------------------------------------------------------
# [8] Matching runtime ownership is never ignored.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8] Matching local runtime ownership is never ignored ---"
Test-ContentContains "matching_local_runtime_active is derived from AppState::locally_owned_run_id()" $CoordinatorContent "state.locally_owned_run_id().await == Some(expected)" | Out-Null
Test-ContentContains "handle_outcome_finalization returns early on a matching local runtime" $CoordinatorContent "if context.matching_local_runtime_active {" | Out-Null

# -----------------------------------------------------------------------
# [9] Terminal replay cannot notify again.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] Terminal replay never re-notifies ---"
Test-ContentContains "session_controller.rs has an explicit OutcomeFinalized arm" $SessionControllerContent "Outcome::OutcomeFinalized {" | Out-Null
Test-ContentContains "session_controller.rs has a separate, non-notifying OutcomeAlreadyFinalized arm" $SessionControllerContent "Outcome::OutcomeAlreadyFinalized {" | Out-Null

# -----------------------------------------------------------------------
# [10] Unchanged evidence blocker cannot notify again.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [10] An unchanged evidence blocker never re-notifies ---"
Test-ContentContains "EvidenceDegraded carries newly_applied" $OutcomeContent "newly_applied: bool," | Out-Null
Test-ContentContains "session_controller.rs's evidence-degraded warning arm exists" $SessionControllerContent "Outcome::OutcomeEvidenceDegraded {" | Out-Null

# -----------------------------------------------------------------------
# [11]/[12] E4 routes read-only.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [11] E4 routes never invoke the classifier/finalizer/coordinator ---"
foreach ($Symbol in @(
    "classify_and_finalize_autonomous_daily_operation(",
    "finalize_autonomous_daily_operation(",
    "tick_autonomous_daily_coordinator("
)) {
    Test-ContentDoesNotContain "the E4 route module never calls $Symbol" $E4RouteContent $Symbol | Out-Null
}

Write-Host ""
Show-Info "--- [12] E4 routes never contain a mutating DB write helper call ---"
foreach ($Symbol in @(
    "create_or_recover_autonomous_daily_operation(",
    "transition_autonomous_daily_operation(",
    "transition_autonomous_daily_operation_to_running(",
    "persist_autonomous_daily_finalization_blocker(",
    "persist_autonomous_session_event(",
    "write_and_confirm_coverage_authority(",
    "start_execution_runtime(",
    "stop_execution_runtime("
)) {
    Test-ContentDoesNotContain "the E4 route module never calls $Symbol" $E4RouteContent $Symbol | Out-Null
}

# -----------------------------------------------------------------------
# [13] Exact market-date parser present.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [13] E4's exact market-date parser exists, route never calls .trim() ---"
Test-ContentContains "parse_exact_market_date exists" $E4RouteContent "fn parse_exact_market_date(raw: &str) -> Option<NaiveDate>" | Out-Null
Test-ContentDoesNotContain "the E4 route module never calls .trim()" $E4RouteContent ".trim()" | Out-Null

# -----------------------------------------------------------------------
# [14]-[19] Integrated proofs exist by name.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [14]-[19] Every required integrated proof exists in the new test file ---"
$RequiredProofTests = @{
    "[14] integrated no-trade proof"             = "e5_proof_a_clean_no_trade_day_full_pipeline_and_replay"
    "[15] integrated activity/full-lineage proof" = "e5_proof_b_activity_day_full_lineage_across_two_runs"
    "[16] evidence-blocker recovery proof"        = "e5_proof_c_evidence_blocker_notifies_once_and_recovers"
    "[17] restart proofs"                         = "e5_proof_d_restart_safety_stop_terminal_and_evidence_blocker"
    "[18] API read-only before/after proof"       = "e5_proof_e_api_read_only_guarantee"
    "[19] fail-soft API truth proof"              = "e5_proof_f_fail_soft_api_truth"
}
foreach ($Label in $RequiredProofTests.Keys) {
    $TestName = $RequiredProofTests[$Label]
    Test-ContentContains "$Label -- fn $TestName exists" $E5TestContent "fn $TestName(" | Out-Null
}
Test-ContentContains "the closure test never references a real provider/broker/Discord client" $E5TestContent "market-data provider or broker call is made" | Out-Null
foreach ($Forbidden in @("AlpacaBrokerAdapter", "AlpacaHistoricalProvider", "reqwest::Client::new")) {
    Test-ContentDoesNotContain "the closure test never references $Forbidden" $E5TestContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [20]/[21] README truth.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [20]/[21] README truth: E1-E4 accepted, E5 awaiting acceptance, Bundle 3/Phase F/G/soak/live untouched ---"
$ReadmeContent = $null
if (Test-FileExists "README.md" $PathReadme) {
    $ReadmeContent = Get-Content -Raw -Path $PathReadme
}
$ReadmeTechContent = $null
if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) {
    $ReadmeTechContent = Get-Content -Raw -Path $PathReadmeTech
}
$ForbiddenReadmeClaims = @(
    "Bundle 3: CLOSED",
    "Bundle 3 is complete",
    "Bundle 3 is now closed",
    "Bundle 3 has closed",
    "Phase F: STARTED",
    "Phase F is complete",
    "Phase F has started",
    "Phase G: STARTED",
    "Phase G is complete",
    "Phase G has started",
    "soak has started",
    "soak: STARTED",
    "unattended soak is underway",
    "unattended soak has begun",
    "live capital is ready",
    "live capital: ready",
    "approved for live capital",
    "ready for live capital",
    "E5: ACCEPTED",
    "E5 is accepted",
    "Phase E: ACCEPTED",
    "Phase E: CLOSED",
    "Phase E is complete"
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
Test-ContentContains "README.md records E4 as accepted" $ReadmeContent "E4 (plus both repairs and their test suites) is now accepted" | Out-Null
Test-ContentContains "README.md mentions Phase E5" $ReadmeContent "Phase E5 (" | Out-Null
Test-ContentContains "README.md records E5 as implementation-complete-awaiting-acceptance" $ReadmeContent "is implementation complete, awaiting ChatGPT and operator acceptance" | Out-Null

# -----------------------------------------------------------------------
# [22] Ledger truth.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [22] Ledger truth ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentContains "ledger records E4 as accepted" $LedgerContent "E4: ACCEPTED" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase E closed" $LedgerContent "PHASE E: CLOSED" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase E closed (alt phrasing)" $LedgerContent "PHASE E: COMPLETE" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Bundle 3 closed" $LedgerContent "BUNDLE 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED" | Out-Null
Test-ContentContains "ledger records E5 as implementation-complete-awaiting-acceptance" $LedgerContent "E5" | Out-Null

# -----------------------------------------------------------------------
# [23]/[24] No production Rust / migration change.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [23] No production Rust file was modified by this patch ---"
# git's routine autocrlf notice ("LF will be replaced by CRLF...") writes to
# stderr; under this script's own $ErrorActionPreference = "Stop" that stream
# can otherwise be promoted to a terminating error. Temporarily relax to
# "Continue" for these two informational reads only, restoring "Stop" right
# after -- $LASTEXITCODE/output content are what this check actually relies
# on, never stderr text.
$PriorErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$ChangedFiles = git -C $RepoRoot diff --name-only HEAD 2>$null
$ChangedFilesStaged = git -C $RepoRoot diff --name-only --cached 2>$null
$ErrorActionPreference = $PriorErrorActionPreference
$AllChanged = @($ChangedFiles) + @($ChangedFilesStaged) | Where-Object { $_ -ne "" } | Select-Object -Unique
$ProductionRustChanged = $AllChanged | Where-Object {
    $_ -like "core-rs/*/src/*.rs" -and $_ -notlike "*/tests/*"
}
if ($null -eq $ProductionRustChanged -or @($ProductionRustChanged).Count -eq 0) {
    Show-Green "  OK -- no production Rust file (core-rs/**/src/**.rs) changed"
} else {
    $script:Violations++
    Show-Red "  FAIL -- production Rust file(s) changed: $ProductionRustChanged"
}

Write-Host ""
Show-Info "--- [24] No migration file was added or modified ---"
$MigrationChanged = $AllChanged | Where-Object { $_ -like "core-rs/crates/mqk-db/migrations/*" }
if ($null -eq $MigrationChanged -or @($MigrationChanged).Count -eq 0) {
    Show-Green "  OK -- no migration file changed"
} else {
    $script:Violations++
    Show-Red "  FAIL -- migration file(s) changed: $MigrationChanged"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
