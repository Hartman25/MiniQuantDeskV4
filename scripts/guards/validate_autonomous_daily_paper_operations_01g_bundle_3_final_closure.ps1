# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01G-BUNDLE-3-FINAL-CLOSURE-AUDIT
# -- Source-aware static validator
# =============================================================================
# Scope: this guard performs the final Bundle 3 closure audit. It invokes
# the F1, F2, and F3 guards (which in turn invoke the Phase E closure guard,
# which invokes E1-E4) and adds its own independent, Phase-G-specific
# checks. Pure text/source validation only -- no network call, no
# provider/broker call, no DB connection, no daemon start, no cargo/npm
# build or test (those are separate, required steps of the G mission, run
# and recorded in the G spec doc, not performed by this guard).
#
# Checks:
#   [1]  The F1, F2, and F3 guards all exist and, when invoked, exit 0
#        (F3's guard transitively invokes F2's, which invokes F1's, which
#        invokes the Phase E closure guard, which invokes E1-E4).
#   [2]  All required specs (D-G) exist and are nonempty.
#   [3]  All required guards (D-G) exist.
#   [4]  All required focused test files exist.
#   [5]  F1 contains no mutating control.
#   [6]  F2/F3 contain no live or unattended authorization.
#   [7]  F3's capture tooling is GET-only and secret-safe.
#   [8]  F1-G introduced no migration file.
#   [9]  F2/F3/G changed no daemon production Rust file.
#   [10] Bundle 4 is not started.
#   [11] Multi-symbol autonomous rollout was not enabled.
#   [12] Unattended soak is not claimed started or complete.
#   [13] Live capital is not claimed ready.
#   [14] The G spec doc exists, is nonempty, and records the fixed
#        historical range authorities.
#   [15] README/ledger record the correct final combined status, and never
#        overclaim F2/F3/G/Bundle-3 as accepted.
#   [16] BUNDLE-3-FINAL-OPERATIONAL-SAFETY-REPAIR range reconciliation:
#        pre-commit, HEAD must equal the fixed pre-repair G head
#        (b70c5156); post-commit, the unique "fix: harden bundle 3
#        operational closeout" commit must exist, have that fixed head as
#        its direct parent, be an ancestor of current HEAD, and the fixed
#        b70c5156..<repair commit> range (never ..HEAD) must introduce no
#        migration or production Rust file.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathGSpec      = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01g_bundle_3_final_closure.md"
$PathReadme     = Join-Path $RepoRoot "README.md"
$PathReadmeTech = Join-Path $RepoRoot "README_TECHNICAL.md"
$PathLedger     = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01G-BUNDLE-3-FINAL-CLOSURE-AUDIT Validator"
Write-Host "============================================================"

# -----------------------------------------------------------------------
# [1] F1/F2/F3 guards exist and exit 0 (transitively covers E1-E5 too).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] F1, F2, and F3 guards all exist and exit 0 ---"
$RequiredPriorGuards = @(
    "validate_autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.ps1",
    "validate_autonomous_daily_paper_operations_01f2_operator_runbook_correction.ps1",
    "validate_autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.ps1"
)
foreach ($GuardName in $RequiredPriorGuards) {
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
# [2] All required specs (D-G) exist and are nonempty.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] All required specs (D-G) exist and are nonempty ---"
$RequiredSpecs = @(
    "docs\specs\autonomous_daily_paper_operations_01d_phase_d_closure.md",
    "docs\specs\autonomous_daily_paper_operations_01e_outcome_truth_contract.md",
    "docs\specs\autonomous_daily_paper_operations_01e2a_coverage_anchor_and_run_lineage.md",
    "docs\specs\autonomous_daily_paper_operations_01e2b_outcome_classifier_and_finalization.md",
    "docs\specs\autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.md",
    "docs\specs\autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.md",
    "docs\specs\autonomous_daily_paper_operations_01e_phase_e_closure.md",
    "docs\specs\autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.md",
    "docs\specs\autonomous_daily_paper_operations_01f2_operator_runbook_correction.md",
    "docs\specs\autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.md",
    "docs\specs\autonomous_daily_paper_operations_01g_bundle_3_final_closure.md"
)
foreach ($SpecRel in $RequiredSpecs) {
    $SpecPath = Join-Path $RepoRoot $SpecRel
    if (Test-Path $SpecPath) {
        $len = (Get-Content -Raw -Path $SpecPath).Length
        if ($len -gt 500) {
            Show-Green "  OK -- spec present and nonempty: $SpecRel"
        } else {
            $script:Violations++
            Show-Red "  FAIL -- spec suspiciously short: $SpecRel"
        }
    } else {
        $script:Violations++
        Show-Red "  FAIL -- required spec missing: $SpecRel"
    }
}

# -----------------------------------------------------------------------
# [3] All required guards (D-G) exist.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] All required guards (D-G) exist ---"
$RequiredGuards = @(
    "validate_autonomous_daily_paper_operations_01e_outcome_contract.ps1",
    "validate_autonomous_daily_paper_operations_01e2a_coverage_anchor_and_run_lineage.ps1",
    "validate_autonomous_daily_paper_operations_01e2b_outcome_classifier_and_finalization.ps1",
    "validate_autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.ps1",
    "validate_autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.ps1",
    "validate_autonomous_daily_paper_operations_01e_phase_e_closure.ps1",
    "validate_autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.ps1",
    "validate_autonomous_daily_paper_operations_01f2_operator_runbook_correction.ps1",
    "validate_autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.ps1",
    "validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1"
)
foreach ($GuardRel in $RequiredGuards) {
    Test-FileExists $GuardRel (Join-Path $ScriptDir $GuardRel) | Out-Null
}

# -----------------------------------------------------------------------
# [4] Required focused test files exist.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] Required focused test files exist ---"
$RequiredTests = @(
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_phase_e_closure_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_operation_api_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_outcome_coordinator_integration_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_session_coordinator_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_phase_d_integration_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_completed_bar_task_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_daily_data_readiness_start_gate_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_readiness_auton_truth01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_paper_status_summary_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_daemon_routes.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_route_contract_rt01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_gui_daemon_contract_gate.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_autonomous_completed_bar_driver_01.rs"
)
foreach ($TestRel in $RequiredTests) {
    Test-FileExists $TestRel (Join-Path $RepoRoot $TestRel) | Out-Null
}

# -----------------------------------------------------------------------
# [5] F1 contains no mutating control (re-assert, source-level).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] F1 screen contains no mutating control ---"
$PathF1Screen = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\autonomousDailyOperations\AutonomousDailyOperationsScreen.tsx"
$F1ScreenContent = if (Test-Path $PathF1Screen) { Get-Content -Raw -Path $PathF1Screen } else { $null }
foreach ($Forbidden in @("postJson", "invokeOperatorAction", "<button", "onClick", "await fetch")) {
    Test-ContentDoesNotContain "F1 screen never references '$Forbidden'" $F1ScreenContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [6] F2/F3 contain no live or unattended authorization.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] F2 runbook and F3 tooling contain no live or unattended authorization ---"
$PathRunbook = Join-Path $RepoRoot "docs\runbooks\autonomous_paper_ops.md"
$RunbookContent = if (Test-Path $PathRunbook) { Get-Content -Raw -Path $PathRunbook } else { $null }
$PathCaptureScript = Join-Path $RepoRoot "scripts\soak\capture_autonomous_paper_session_evidence.ps1"
$CaptureContent = if (Test-Path $PathCaptureScript) { Get-Content -Raw -Path $PathCaptureScript } else { $null }
# Note: a bare substring scan for phrases like "enable live mode" cannot
# distinguish an authorization from a prohibition ("Do not enable live
# mode") -- the runbook legitimately contains the prohibited phrase inside
# its own §23 prohibitions list. This check instead confirms the
# prohibition sentences themselves are present (positive assertions are not
# fooled by negation the way a forbidden-substring scan is).
Test-ContentContains "runbook prohibits enabling live mode" $RunbookContent "Do not enable live mode" | Out-Null
Test-ContentContains "runbook prohibits beginning unattended operation" $RunbookContent "Do not begin unattended" | Out-Null
Test-ContentDoesNotContain "runbook does not contain forbidden claim 'live capital is ready'" $RunbookContent "live capital is ready" | Out-Null
Test-ContentContains "runbook states live capital is not ready" $RunbookContent "Live capital is not ready" | Out-Null
Test-ContentContains "runbook states the unattended soak has not started" $RunbookContent "unattended 10" | Out-Null

# -----------------------------------------------------------------------
# [7] F3 capture tooling is GET-only and secret-safe (re-assert).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] F3 capture tooling is GET-only and secret-safe ---"
Test-ContentContains "capture script uses -Method Get" $CaptureContent "-Method Get" | Out-Null
# Comment lines legitimately document these prohibitions by name (e.g. "never
# touches ... MQK_OPERATOR_TOKEN"); scope this scan to executable code only,
# same convention as the F3 guard's own [6] .env.local check.
$CaptureCodeOnlyG = $null
if ($null -ne $CaptureContent) {
    $CaptureCodeOnlyG = ($CaptureContent -split "`r?`n" | Where-Object { $_.TrimStart() -notmatch '^#' }) -join "`n"
}
foreach ($Forbidden in @("-Method Post", "-Method Put", "-Method Patch", "-Method Delete")) {
    Test-ContentDoesNotContain "capture script's executable code never references '$Forbidden'" $CaptureCodeOnlyG $Forbidden | Out-Null
}
# ALPACA_API_KEY / ALPACA_API_SECRET / MQK_OPERATOR_TOKEN legitimately appear
# as literal strings in REPAIR E's secret-pattern-scan list
# (Find-SecretShapedPattern) -- a string comparison against captured
# evidence, not a credential read. The actual exposure risk is the script
# reading one of these as an environment variable; prove that instead.
$EnvVarAccessFound = $false
if ($null -ne $CaptureCodeOnlyG) {
    foreach ($EnvPattern in @(
        '\$env:ALPACA', '\$env:MQK_OPERATOR_TOKEN', '\$env:MQK_DATABASE_URL',
        '\benv:ALPACA', '\benv:MQK_OPERATOR_TOKEN', '\benv:MQK_DATABASE_URL',
        'GetEnvironmentVariable\([^\)]*ALPACA', 'GetEnvironmentVariable\([^\)]*MQK_OPERATOR_TOKEN',
        'GetEnvironmentVariable\([^\)]*MQK_DATABASE_URL'
    )) {
        if ($CaptureCodeOnlyG -match $EnvPattern) { $EnvVarAccessFound = $true }
    }
}
if (-not $EnvVarAccessFound) {
    Show-Green "  OK -- capture script never reads ALPACA_*/MQK_OPERATOR_TOKEN/MQK_DATABASE_URL as an environment variable"
} else {
    $script:Violations++
    Show-Red "  FAIL -- capture script appears to read a credential environment variable"
}
Test-ContentContains "capture script's secret-pattern list includes 'ALPACA_API_KEY' as a scan pattern (not an env read)" $CaptureContent "ALPACA_API_KEY" | Out-Null

# -----------------------------------------------------------------------
# [8]/[9] No migration; no daemon production Rust change (fixed range +
# working tree).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8]/[9] F1-G introduced no migration; F2/F3/G changed no daemon production Rust ---"
$PhaseEAcceptedHead = "4b6eec72cb65dec1fc2a8793e9d9d7bdde8328b4"
$PriorErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& git -C $RepoRoot merge-base --is-ancestor $PhaseEAcceptedHead HEAD 2>$null
$BaseIsAncestor = ($LASTEXITCODE -eq 0)
$CommittedRange = @()
if ($BaseIsAncestor) {
    $CommittedRange = git -C $RepoRoot diff --name-only "$PhaseEAcceptedHead..HEAD" 2>$null
}
$UnstagedChanges = git -C $RepoRoot diff --name-only 2>$null
$StagedChanges = git -C $RepoRoot diff --name-only --cached 2>$null
$ErrorActionPreference = $PriorErrorActionPreference

$CommittedRangeClean = @($CommittedRange) | Where-Object { $_ -ne "" } | Select-Object -Unique
$UnstagedClean = @($UnstagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique
$StagedClean = @($StagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique

function Test-NoMigrationOrProductionRust {
    param([string]$Label, [string[]]$Paths)
    $Migrations = $Paths | Where-Object { $_ -like "core-rs/crates/mqk-db/migrations/*" }
    $ProductionRust = $Paths | Where-Object { $_ -like "core-rs/*/src/*.rs" -and $_ -notlike "*/tests/*" }
    if ($null -eq $Migrations -or @($Migrations).Count -eq 0) {
        Show-Green "  OK -- $Label -- no migration file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- migration file(s): $($Migrations -join ', ')"
    }
    if ($null -eq $ProductionRust -or @($ProductionRust).Count -eq 0) {
        Show-Green "  OK -- $Label -- no production Rust file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- production Rust file(s): $($ProductionRust -join ', ')"
    }
}
if ($BaseIsAncestor) {
    Show-Green "  OK -- $PhaseEAcceptedHead (accepted Phase E head) is an ancestor of HEAD"
} else {
    $script:Violations++
    Show-Red "  FAIL -- $PhaseEAcceptedHead is not an ancestor of HEAD -- cannot establish the F1-G committed range"
}
Test-NoMigrationOrProductionRust "committed range $PhaseEAcceptedHead..HEAD (covers F1, F2, F3, G)" $CommittedRangeClean
Test-NoMigrationOrProductionRust "unstaged working tree" $UnstagedClean
Test-NoMigrationOrProductionRust "staged working tree" $StagedClean

# -----------------------------------------------------------------------
# [10] Bundle 4 is not started.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [10] Bundle 4 is not started ---"
$Bundle4Paths = @($CommittedRangeClean) + @($UnstagedClean) + @($StagedClean) | Where-Object {
    $_ -match "bundle.?4" -or $_ -match "BUNDLE-4"
} | Select-Object -Unique
if ($null -eq $Bundle4Paths -or @($Bundle4Paths).Count -eq 0) {
    Show-Green "  OK -- no Bundle 4 path touched"
} else {
    $script:Violations++
    Show-Red "  FAIL -- Bundle 4 path(s) touched: $($Bundle4Paths -join ', ')"
}

# -----------------------------------------------------------------------
# [11] Multi-symbol autonomous rollout was not enabled.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [11] Multi-symbol autonomous rollout was not enabled ---"
Test-ContentContains "runbook still documents single-symbol scope" $RunbookContent "single-symbol" | Out-Null
Test-ContentDoesNotContain "runbook does not claim multi-symbol autonomous rollout is enabled" $RunbookContent "multi-symbol autonomous rollout is enabled" | Out-Null

# -----------------------------------------------------------------------
# [12]/[13] Soak-started / live-capital-ready overclaim guard.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [12]/[13] Unattended soak and live-capital readiness are never claimed ---"
$ReadmeContent = if (Test-FileExists "README.md" $PathReadme) { Get-Content -Raw -Path $PathReadme } else { $null }
$ReadmeTechContent = if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) { Get-Content -Raw -Path $PathReadmeTech } else { $null }
$LedgerContent = if (Test-FileExists "Master patch ledger" $PathLedger) { Get-Content -Raw -Path $PathLedger } else { $null }
# Built via [char]0x2014 rather than a literal em-dash in source: Windows
# PowerShell 5.1 reads a BOM-less .ps1 using the system codepage, and a raw
# UTF-8 em-dash byte sequence embedded in a string literal can silently
# mis-tokenize (observed: a ParserError on this exact line). Runtime
# character construction is encoding-independent.
$EmDash = [char]0x2014
$ForbiddenClaims = @(
    "SOAK: STARTED",
    "soak has started",
    "unattended soak is underway",
    "LIVE CAPITAL: READY",
    "live capital is ready",
    "approved for live capital",
    "ready for live capital",
    "BUNDLE 3: CLOSED",
    "Bundle 3 is now closed",
    "Bundle 3 has closed",
    "BUNDLE 3: ACCEPTED",
    "F2: ACCEPTED $EmDash COMPLETE",
    "F3: ACCEPTED $EmDash COMPLETE",
    "PHASE G: ACCEPTED $EmDash COMPLETE"
)
foreach ($Doc in @(
    @{Name = "README.md"; Content = $ReadmeContent},
    @{Name = "README_TECHNICAL.md"; Content = $ReadmeTechContent},
    @{Name = "Master patch ledger"; Content = $LedgerContent}
)) {
    foreach ($Phrase in $ForbiddenClaims) {
        Test-ContentDoesNotContain "$($Doc.Name) does not contain forbidden claim '$Phrase'" $Doc.Content $Phrase | Out-Null
    }
}

# -----------------------------------------------------------------------
# [14] G spec exists, is nonempty, and records the fixed range authorities.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [14] G spec records the fixed historical range authorities ---"
$GSpecContent = $null
if (Test-FileExists "G spec doc" $PathGSpec) {
    $GSpecContent = Get-Content -Raw -Path $PathGSpec
    if ($GSpecContent.Length -gt 2000) { Show-Green "  OK -- G spec is nonempty" } else { $script:Violations++; Show-Red "  FAIL -- G spec is suspiciously short" }
}
Test-ContentContains "G spec records the accepted Phase E base" $GSpecContent "11664945e90a582e6984f0eab66cf89690120769" | Out-Null
Test-ContentContains "G spec records the accepted F1 head" $GSpecContent "bd7336d4dd14dbb1943638b152886eb40b646b7d" | Out-Null
Test-ContentContains "G spec records 47 passed / 9 pre-existing failures" $GSpecContent "47 passed" | Out-Null

# -----------------------------------------------------------------------
# [15] README/ledger record correct final combined status.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [15] README/ledger record correct final combined status ---"
Test-ContentContains "README.md mentions F3" $ReadmeContent "F3" | Out-Null
Test-ContentContains "ledger mentions Phase G" $LedgerContent "PHASE G" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Bundle 3 accepted" $LedgerContent "BUNDLE 3: ACCEPTED" | Out-Null

# -----------------------------------------------------------------------
# [16] BUNDLE-3-FINAL-OPERATIONAL-SAFETY-REPAIR range reconciliation.
#
# Pre-repair G head is fixed: b70c51563bbe51001d19f1ad9388655820124fb0
# ("docs: prepare autonomous daily paper bundle 3 closure").
#
# Pre-commit (this repair not yet committed): HEAD still equals that fixed
# pre-repair G head; the repair's own file scope is already covered by the
# staged/unstaged working-tree checks in [8]/[9] above.
#
# Post-commit (and all future validation, including once Bundle 4 begins):
# locate the unique commit whose exact subject is "fix: harden bundle 3
# operational closeout"; require the fixed pre-repair G head to be its
# direct parent; require that repair commit to be an ancestor of the
# current HEAD; use b70c5156..<repair commit> -- deliberately NEVER
# b70c5156..HEAD -- as the fixed repair range for the no-production-Rust/
# no-migration checks. This is a separate, narrower fixed range from the
# $PhaseEAcceptedHead..HEAD range checked in [8]/[9] above (which
# legitimately widens as later phases land); this one never widens past
# the single repair commit it identifies.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [16] Bundle-3-final-repair range reconciliation (pre-commit vs. post-commit) ---"
$PreRepairGHead = "b70c51563bbe51001d19f1ad9388655820124fb0"
$RepairCommitSubject = "fix: harden bundle 3 operational closeout"

$PriorErrorActionPreferenceR = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$CurrentHead = (git -C $RepoRoot rev-parse HEAD 2>$null | Select-Object -First 1)
$ErrorActionPreference = $PriorErrorActionPreferenceR

if ($CurrentHead -eq $PreRepairGHead) {
    Show-Green "  OK -- pre-commit mode: HEAD == $PreRepairGHead (the repair is not yet committed); repair scope is covered by the staged/unstaged working-tree checks above"
} else {
    $ErrorActionPreference = "Continue"
    $MatchingCommits = @(git -C $RepoRoot log --all --format='%H %s' 2>$null |
        Where-Object { $_ -match '^[0-9a-f]{40} fix: harden bundle 3 operational closeout$' } |
        ForEach-Object { ($_ -split ' ', 2)[0] } |
        Select-Object -Unique)
    $ErrorActionPreference = $PriorErrorActionPreferenceR

    if ($MatchingCommits.Count -ne 1) {
        $script:Violations++
        Show-Red "  FAIL -- expected exactly one commit with subject '$RepairCommitSubject', found $($MatchingCommits.Count)"
    } else {
        $RepairCommit = $MatchingCommits[0]
        Show-Green "  OK -- found the unique repair commit: $RepairCommit"

        $ErrorActionPreference = "Continue"
        $RepairParent = (git -C $RepoRoot rev-parse "$RepairCommit^" 2>$null | Select-Object -First 1)
        $ErrorActionPreference = $PriorErrorActionPreferenceR
        if ($RepairParent -eq $PreRepairGHead) {
            Show-Green "  OK -- $PreRepairGHead is the direct parent of the repair commit"
        } else {
            $script:Violations++
            Show-Red "  FAIL -- the repair commit's direct parent is '$RepairParent', expected '$PreRepairGHead'"
        }

        $ErrorActionPreference = "Continue"
        git -C $RepoRoot merge-base --is-ancestor $RepairCommit HEAD 2>$null
        $RepairIsAncestor = ($LASTEXITCODE -eq 0)
        $ErrorActionPreference = $PriorErrorActionPreferenceR
        if ($RepairIsAncestor) {
            Show-Green "  OK -- the repair commit is an ancestor of current HEAD"
        } else {
            $script:Violations++
            Show-Red "  FAIL -- the repair commit is not an ancestor of current HEAD"
        }

        $ErrorActionPreference = "Continue"
        $RepairRange = git -C $RepoRoot diff --name-only "$PreRepairGHead..$RepairCommit" 2>$null
        $ErrorActionPreference = $PriorErrorActionPreferenceR
        $RepairRangeClean = @($RepairRange) | Where-Object { $_ -ne "" } | Select-Object -Unique
        Test-NoMigrationOrProductionRust "fixed repair range $PreRepairGHead..$RepairCommit (never widened to ..HEAD)" $RepairRangeClean
    }
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01G-BUNDLE-3-FINAL-CLOSURE-AUDIT evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
