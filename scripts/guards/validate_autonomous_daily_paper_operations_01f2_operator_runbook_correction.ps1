# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION
# -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the F2 operator runbook correction, built on
# top of the already-accepted F1 GUI daily-operation projection. Pure
# text/source validation only -- no network call, no provider/broker call,
# no DB connection, no daemon start, no cargo/npm build or test.
#
# RECONCILIATION (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-
# EVIDENCE-PREPARATION): the [15] forbidden-claims list no longer forbids
# "F3: IMPLEMENTATION COMPLETE" claims -- that is now legitimate once F2 is
# implementation complete and F3 begins. This mirrors the same obsolete-
# assertion reconciliation convention F1's guard applied for F2. No other
# F2 check is reopened or weakened.
#
# Checks:
#   [1]  The F1 guard exists and, when invoked, exits 0.
#   [2]  The runbook and F2 spec doc both exist and are nonempty.
#   [3]  Safety-boundary language is present: Paper + Alpaca only,
#        single-symbol/supervised scope, soak-not-started, live-not-ready.
#   [4]  A live-routing-disabled verification step is present.
#   [5]  All five authoritative read-only routes are documented.
#   [6]  The truth_state vocabulary (active/not_found/backend_unavailable/
#        query_failed) is preserved distinctly, and not_found is explicitly
#        stated to not be a backend failure.
#   [7]  Null counts are explicitly documented as unavailable, not zero.
#   [8]  evidence_degraded / evidence_state == "degraded" handling exists.
#   [9]  Restart-before-finalization and restart-after-finalization (terminal
#        commit) behavior are both documented.
#   [10] No manual finalization / DB-rewrite instruction exists (forbidden
#        instructional pattern scan), while the prohibition itself is
#        documented.
#   [11] The unattended soak is never claimed started.
#   [12] Live-capital readiness is never claimed.
#   [13] The operating-database port table exists and the forbidden ports
#        (5434 test DB, 5440 reality-test DB) are explicitly excluded from
#        operating-database use.
#   [14] No production Rust, migration, or GUI production file is touched by
#        this patch (working tree, staged and unstaged).
#   [15] README.md / README_TECHNICAL.md / ledger record correct F2 status
#        and never overclaim Phase G, Bundle 3 closure, soak, or live
#        capital.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01f2_operator_runbook_correction.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathRunbook  = Join-Path $RepoRoot "docs\runbooks\autonomous_paper_ops.md"
$PathF2Spec   = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01f2_operator_runbook_correction.md"
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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION Validator"
Write-Host "============================================================"

# -----------------------------------------------------------------------
# [1] F1 guard exists and exits 0.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] F1 guard exists and exits 0 ---"
$F1GuardPath = Join-Path $ScriptDir "validate_autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.ps1"
if (Test-Path $F1GuardPath) {
    & powershell -ExecutionPolicy Bypass -File $F1GuardPath *> $null
    $exitCode = $LASTEXITCODE
    if ($exitCode -eq 0) {
        Show-Green "  OK -- F1 guard exits 0"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- F1 guard exited $exitCode"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- F1 guard not found: $F1GuardPath"
}

# -----------------------------------------------------------------------
# [2] Runbook and F2 spec exist and are nonempty.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] Runbook and F2 spec doc exist and are nonempty ---"
$RunbookContent = $null
if (Test-FileExists "autonomous_paper_ops.md runbook" $PathRunbook) {
    $RunbookContent = Get-Content -Raw -Path $PathRunbook
    if ($RunbookContent.Length -gt 5000) {
        Show-Green "  OK -- runbook is nonempty ($($RunbookContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- runbook is suspiciously short"
    }
}
$F2SpecContent = $null
if (Test-FileExists "F2 spec doc" $PathF2Spec) {
    $F2SpecContent = Get-Content -Raw -Path $PathF2Spec
    if ($F2SpecContent.Length -gt 2000) {
        Show-Green "  OK -- F2 spec is nonempty ($($F2SpecContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- F2 spec is suspiciously short"
    }
}

# -----------------------------------------------------------------------
# [3] Safety-boundary language.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] Safety-boundary language is present ---"
Test-ContentContains "runbook states Paper + Alpaca only" $RunbookContent "Paper + Alpaca only" | Out-Null
Test-ContentContains "runbook states single-symbol scope" $RunbookContent "Single-symbol, long-only" | Out-Null
Test-ContentContains "runbook states active operator supervision is required" $RunbookContent "Active operator supervision is required" | Out-Null
Test-ContentContains "runbook states the unattended soak has not started" $RunbookContent "unattended 10" | Out-Null
Test-ContentContains "runbook states live capital is not ready" $RunbookContent "Live capital is not ready" | Out-Null

# -----------------------------------------------------------------------
# [4] Live-routing-disabled verification step.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] Live-routing-disabled verification step is present ---"
Test-ContentContains "runbook has a live-routing-disabled confirmation step" $RunbookContent "Confirm live routing is disabled" | Out-Null

# -----------------------------------------------------------------------
# [5] All five authoritative routes documented.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] All five authoritative read-only routes are documented ---"
foreach ($Route in @(
    "/api/v1/autonomous/readiness",
    "/api/v1/autonomous/paper-status",
    "/api/v1/system/preflight",
    "/api/v1/autonomous/daily-operation",
    "/api/v1/autonomous/daily-operations"
)) {
    Test-ContentContains "runbook documents route $Route" $RunbookContent $Route | Out-Null
}

# -----------------------------------------------------------------------
# [6] truth_state vocabulary preserved distinctly; not_found explained.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] truth_state vocabulary preserved distinctly; not_found is not a backend failure ---"
foreach ($TruthState in @('active', 'not_found', 'backend_unavailable', 'query_failed')) {
    Test-ContentContains "runbook documents truth_state value $TruthState" $RunbookContent $TruthState | Out-Null
}
Test-ContentContains "runbook documents truth_state value active (as a code span)" $RunbookContent '`active`' | Out-Null
Test-ContentContains "runbook states not_found is not a backend failure" $RunbookContent 'not_found` is not a backend failure' | Out-Null

# -----------------------------------------------------------------------
# [7] Null counts documented as unavailable, not zero.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] Null counts are documented as unavailable, not zero ---"
Test-ContentContains "runbook states null counts are unavailable, not zero" $RunbookContent "unavailable, not zero" | Out-Null

# -----------------------------------------------------------------------
# [8] evidence_degraded handling.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8] evidence_degraded handling is documented ---"
Test-ContentContains "runbook documents evidence_state degraded" $RunbookContent 'evidence_state == "degraded"' | Out-Null
Test-ContentContains "runbook documents blocked_insufficient_evidence" $RunbookContent "blocked_insufficient_evidence" | Out-Null

# -----------------------------------------------------------------------
# [9] Restart-before/after-finalization behavior documented.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] Restart-before-finalization and restart-after-finalization behavior are documented ---"
Test-ContentContains "runbook documents restart before finalization" $RunbookContent "before finalization" | Out-Null
Test-ContentContains "runbook documents restart after a terminal commit" $RunbookContent "terminal commit" | Out-Null

# -----------------------------------------------------------------------
# [10] No manual finalization / DB-rewrite instruction; prohibition present.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [10] No manual finalization/DB-rewrite instruction exists; prohibition is documented ---"
foreach ($Forbidden in @(
    "UPDATE sys_autonomous_daily_operations",
    "manually finalize the operation",
    "force the outcome to",
    "psql -c `"UPDATE"
)) {
    Test-ContentDoesNotContain "runbook never instructs '$Forbidden'" $RunbookContent $Forbidden | Out-Null
}
Test-ContentContains "runbook documents the prohibition on manually rewriting operation rows" $RunbookContent "Do not manually rewrite" | Out-Null
Test-ContentContains "runbook states there is no manual finalization command" $RunbookContent "no manual finalization command" | Out-Null

# -----------------------------------------------------------------------
# [11]/[12] Soak-not-started / live-capital-not-ready claims.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [11]/[12] Unattended soak never claimed started; live capital never claimed ready ---"
foreach ($Forbidden in @(
    "soak has started",
    "soak: STARTED",
    "unattended soak is underway",
    "unattended soak has begun",
    "live capital is ready",
    "live capital: ready",
    "approved for live capital",
    "ready for live capital"
)) {
    Test-ContentDoesNotContain "runbook does not contain forbidden claim '$Forbidden'" $RunbookContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [13] Operating-database port table; forbidden ports excluded.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [13] Operating-database port table exists; forbidden ports excluded from operating use ---"
Test-ContentContains "runbook documents the operating DB on port 5432" $RunbookContent "5432" | Out-Null
Test-ContentContains "runbook excludes port 5434 and 5440 from operating-database use" $RunbookContent "as the operating paper database" | Out-Null

# -----------------------------------------------------------------------
# [14] Patch-scope: no production Rust, migration, or GUI production file.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [14] No production Rust, migration, or GUI production file is touched ---"
$PriorErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$UnstagedChanges = git -C $RepoRoot diff --name-only 2>$null
$StagedChanges = git -C $RepoRoot diff --name-only --cached 2>$null
$ErrorActionPreference = $PriorErrorActionPreference

$UnstagedClean = @($UnstagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique
$StagedClean = @($StagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique

function Test-NoProductionRustMigrationOrGui {
    param([string]$Label, [string[]]$Paths)
    $ProductionRust = $Paths | Where-Object { $_ -like "core-rs/*/src/*.rs" -and $_ -notlike "*/tests/*" }
    $Migrations = $Paths | Where-Object { $_ -like "core-rs/crates/mqk-db/migrations/*" }
    $Gui = $Paths | Where-Object { $_ -like "core-rs/mqk-gui/src/*" }

    if ($null -eq $ProductionRust -or @($ProductionRust).Count -eq 0) {
        Show-Green "  OK -- $Label -- no production Rust file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- production Rust file(s): $($ProductionRust -join ', ')"
    }
    if ($null -eq $Migrations -or @($Migrations).Count -eq 0) {
        Show-Green "  OK -- $Label -- no migration file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- migration file(s): $($Migrations -join ', ')"
    }
    if ($null -eq $Gui -or @($Gui).Count -eq 0) {
        Show-Green "  OK -- $Label -- no GUI production file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- GUI production file(s): $($Gui -join ', ')"
    }
}
Test-NoProductionRustMigrationOrGui "unstaged working tree" $UnstagedClean
Test-NoProductionRustMigrationOrGui "staged working tree" $StagedClean

# -----------------------------------------------------------------------
# [15] README/ledger status and overclaim guard.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [15] README/ledger record correct F2 status and never overclaim ---"
$ReadmeContent = if (Test-FileExists "README.md" $PathReadme) { Get-Content -Raw -Path $PathReadme } else { $null }
$ReadmeTechContent = if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) { Get-Content -Raw -Path $PathReadmeTech } else { $null }
$LedgerContent = if (Test-FileExists "Master patch ledger" $PathLedger) { Get-Content -Raw -Path $PathLedger } else { $null }

$ForbiddenClaims = @(
    "PHASE G: STARTED",
    "Phase G has started",
    "Phase G is complete",
    "BUNDLE 3: CLOSED",
    "Bundle 3 is now closed",
    "Bundle 3 has closed",
    "Bundle 3 is complete",
    "SOAK: STARTED",
    "soak has started",
    "unattended soak is underway",
    "LIVE CAPITAL: READY",
    "live capital is ready",
    "approved for live capital",
    "ready for live capital"
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
Test-ContentContains "README.md records F2 status" $ReadmeContent "F2" | Out-Null
Test-ContentContains "ledger records F2" $LedgerContent "F2" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase F closed" $LedgerContent "PHASE F: CLOSED" | Out-Null

# -----------------------------------------------------------------------
# [16] BUNDLE-3-FINAL-OPERATIONAL-SAFETY-REPAIR: the runbook must never
#      contain the specific contradictory/unsafe phrases this repair
#      removed, and must contain the specific corrected safety language it
#      added (supervised-only truth, fail-closed WS-gap restart procedure,
#      legacy soak-harness boundary).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [16] Repaired runbook content: forbidden phrases absent, corrected language present ---"
foreach ($Forbidden in @(
    "designed for unsupervised intraday operation",
    "restart daemon to reset cursor to ColdStartUnproven",
    "use repair_ws_continuity seam",
    "the canonical one-day paper soak harness"
)) {
    Test-ContentDoesNotContain "runbook does not contain forbidden phrase '$Forbidden'" $RunbookContent $Forbidden | Out-Null
}
Test-ContentContains "runbook marks the legacy soak-harness boundary" $RunbookContent "is legacy/reference tooling" | Out-Null
Test-ContentContains "runbook states a persisted GapDetected remains fail-closed across restart" $RunbookContent "remains fail-closed across a daemon" | Out-Null
Test-ContentContains "runbook states restart alone is not a repair" $RunbookContent "restarting the daemon is not itself a repair" | Out-Null
Test-ContentContains "runbook states there is no operator-invocable repair command" $RunbookContent "no operator-invocable" | Out-Null
Test-ContentContains "runbook distinguishes no-routine-intervention from supervision-unnecessary" $RunbookContent "No routine intervention expected during a healthy session is not the same" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
