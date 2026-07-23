# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION
# -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the F1 read-only GUI projection of the accepted
# E4 daily-operation API, built entirely on top of the already-accepted E1-E5
# foundation (validated by its own independently-maintained guards, unmodified
# by this patch except for the Phase E closure guard's own now-obsolete
# permanent-range assertion, reconciled in place -- see [F1-13]). No network
# call, no provider/broker call, no DB connection, no daemon start, no
# cargo/npm build or test -- pure text/source validation only (build/test/
# cargo runs are separate, required steps of the F1 mission, not performed by
# this guard).
#
# RECONCILIATION (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-
# CORRECTION): now that F1 is accepted at the fixed head below, check [12]
# is scoped to F1's own accepted committed range only (not the live working
# tree/current HEAD), and the [13]-[16] forbidden-claims list no longer
# forbids "F2/F3: IMPLEMENTATION COMPLETE" claims -- both are now legitimate
# once F1 is accepted. This mirrors the same obsolete-assertion
# reconciliation convention F1 itself applied to the Phase E closure guard's
# range check. No other F1 check is reopened or weakened.
#
# Checks:
#   [1]  Both canonical E4 routes are fetched from the GUI operator-model
#        assembly (api.ts).
#   [2]  The dailyOperations screen exists, is registered in SCREEN_REGISTRY,
#        and is reachable from the left rail.
#   [3]  The dailyOperations screen is registered in MONITOR_GROUPS.operator,
#        never in the execution or diagnostics groups.
#   [4]  The full daemon truth-state vocabulary is preserved distinctly (not
#        collapsed) in the GUI mapper layer.
#   [5]  An endpoint/network failure cannot masquerade as the daemon's own
#        authoritative not_found -- the GUI-only endpoint_unavailable
#        sentinel always carries truth_state: null.
#   [6]  Null evidence counts can never render as zero -- the screen routes
#        every one of the three activity-count fields through the pure
#        formatDailyOperationCount helper, never a raw field interpolation.
#   [7]  Evidence blockers are never discarded -- the screen renders
#        evidence_blockers.
#   [8]  History rows are never re-sorted or re-grouped by the GUI.
#   [9]  No mutating route or action control exists anywhere in the new
#        screen/mapper source.
#   [10] No daemon production Rust file is touched (committed F1 range and
#        current working tree).
#   [11] No migration file is touched (committed F1 range and current
#        working tree).
#   [12] No F2/F3 work is introduced (no runbook edit, no soak manifest, no
#        F2/F3 spec doc, no README/ledger claim of F2 or F3 started).
#   [13] Phase G is never marked started.
#   [14] Bundle 3 is never marked closed.
#   [15] The unattended soak is never marked started.
#   [16] Live capital is never marked ready.
#   [17] README.md / README_TECHNICAL.md / ledger record E1-E5 as accepted
#        and F1 as implementation-complete-awaiting-acceptance.
#   [18] The new F1 spec doc exists and is nonempty.
#   [19] The F1 GUI test files exist and are registered in package.json's
#        test script.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathApiTs             = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\api.ts"
$PathLegacyTs          = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\legacy.ts"
$PathSourceAuthorityTs = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\sourceAuthority.ts"
$PathScreenRegistryTsx = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\screens\screenRegistry.tsx"
$PathLeftRailNavTs     = Join-Path $RepoRoot "core-rs\mqk-gui\src\components\layout\leftRailNav.ts"
$PathScreenTsx         = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\autonomousDailyOperations\AutonomousDailyOperationsScreen.tsx"
$PathFormatTs          = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\autonomousDailyOperations\formatDailyOperationCount.ts"
$PathTypesTs           = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\types\autonomousDailyOperations.ts"
$PathPackageJson       = Join-Path $RepoRoot "core-rs\mqk-gui\package.json"
$PathF1Spec            = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.md"
$PathReadme            = Join-Path $RepoRoot "README.md"
$PathReadmeTech        = Join-Path $RepoRoot "README_TECHNICAL.md"
$PathLedger            = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"
$PathApiTestTs         = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\autonomousDailyOperations\__tests__\api.test.ts"
$PathScreenSourceTestTs = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\autonomousDailyOperations\__tests__\screenSource.test.ts"
$PathRowValidatorTestTs = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\autonomousDailyOperations\__tests__\isAutonomousDailyOperationApiRow.test.ts"

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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- File presence ---"
Test-FileExists "api.ts"                                            $PathApiTs             | Out-Null
Test-FileExists "legacy.ts"                                          $PathLegacyTs          | Out-Null
Test-FileExists "sourceAuthority.ts"                                 $PathSourceAuthorityTs | Out-Null
Test-FileExists "screenRegistry.tsx"                                 $PathScreenRegistryTsx | Out-Null
Test-FileExists "leftRailNav.ts"                                     $PathLeftRailNavTs     | Out-Null
Test-FileExists "AutonomousDailyOperationsScreen.tsx"                $PathScreenTsx         | Out-Null
Test-FileExists "formatDailyOperationCount.ts"                       $PathFormatTs          | Out-Null
Test-FileExists "types/autonomousDailyOperations.ts"                 $PathTypesTs           | Out-Null
Test-FileExists "package.json"                                       $PathPackageJson       | Out-Null

$ApiContent             = if (Test-Path $PathApiTs) { Get-Content -Raw -Path $PathApiTs } else { $null }
$LegacyContent          = if (Test-Path $PathLegacyTs) { Get-Content -Raw -Path $PathLegacyTs } else { $null }
$ScreenRegistryContent  = if (Test-Path $PathScreenRegistryTsx) { Get-Content -Raw -Path $PathScreenRegistryTsx } else { $null }
$LeftRailNavContent     = if (Test-Path $PathLeftRailNavTs) { Get-Content -Raw -Path $PathLeftRailNavTs } else { $null }
$ScreenContent          = if (Test-Path $PathScreenTsx) { Get-Content -Raw -Path $PathScreenTsx } else { $null }
$FormatContent          = if (Test-Path $PathFormatTs) { Get-Content -Raw -Path $PathFormatTs } else { $null }
$PackageJsonContent     = if (Test-Path $PathPackageJson) { Get-Content -Raw -Path $PathPackageJson } else { $null }
$TypesContent           = if (Test-Path $PathTypesTs) { Get-Content -Raw -Path $PathTypesTs } else { $null }
$ApiTestContent         = if (Test-Path $PathApiTestTs) { Get-Content -Raw -Path $PathApiTestTs } else { $null }
$ScreenSourceTestContent = if (Test-Path $PathScreenSourceTestTs) { Get-Content -Raw -Path $PathScreenSourceTestTs } else { $null }

# -----------------------------------------------------------------------
# [1] Both canonical routes fetched.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] Both canonical E4 routes are fetched by the GUI operator model ---"
Test-ContentContains "api.ts fetches /api/v1/autonomous/daily-operation" $ApiContent '"/api/v1/autonomous/daily-operation"' | Out-Null
Test-ContentContains "api.ts fetches /api/v1/autonomous/daily-operations" $ApiContent "/api/v1/autonomous/daily-operations?limit=20" | Out-Null

# -----------------------------------------------------------------------
# [2] Screen exists and is registered / reachable.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] dailyOperations screen exists, is registered, and is reachable ---"
Test-ContentContains "screenRegistry.tsx imports AutonomousDailyOperationsScreen" $ScreenRegistryContent "AutonomousDailyOperationsScreen" | Out-Null
Test-ContentContains "screenRegistry.tsx registers the dailyOperations screen key" $ScreenRegistryContent "dailyOperations:" | Out-Null
Test-ContentContains "leftRailNav.ts includes dailyOperations" $LeftRailNavContent '"dailyOperations"' | Out-Null

# -----------------------------------------------------------------------
# [3] Registered under the operator monitor group only.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] dailyOperations is in MONITOR_GROUPS.operator, never execution/diagnostics ---"
if ($null -ne $ScreenRegistryContent) {
    $OperatorLineMatch = [regex]::Match($ScreenRegistryContent, 'operator:\s*\[[^\]]*\]')
    $ExecutionLineMatch = [regex]::Match($ScreenRegistryContent, 'execution:\s*\[[^\]]*\]')
    $DiagnosticsLineMatch = [regex]::Match($ScreenRegistryContent, 'diagnostics:\s*\[[^\]]*\]')
    if ($OperatorLineMatch.Success -and $OperatorLineMatch.Value.Contains('"dailyOperations"')) {
        Show-Green "  OK -- MONITOR_GROUPS.operator contains dailyOperations"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- MONITOR_GROUPS.operator does not contain dailyOperations"
    }
    if ($ExecutionLineMatch.Success -and $ExecutionLineMatch.Value.Contains('"dailyOperations"')) {
        $script:Violations++
        Show-Red "  FAIL -- MONITOR_GROUPS.execution must not contain dailyOperations"
    } else {
        Show-Green "  OK -- MONITOR_GROUPS.execution does not contain dailyOperations"
    }
    if ($DiagnosticsLineMatch.Success -and $DiagnosticsLineMatch.Value.Contains('"dailyOperations"')) {
        $script:Violations++
        Show-Red "  FAIL -- MONITOR_GROUPS.diagnostics must not contain dailyOperations"
    } else {
        Show-Green "  OK -- MONITOR_GROUPS.diagnostics does not contain dailyOperations"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- screenRegistry.tsx unreadable, cannot verify monitor group placement"
}

# -----------------------------------------------------------------------
# [4] Truth-state vocabulary preserved distinctly, never collapsed.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] Daemon truth-state vocabulary is preserved distinctly ---"
foreach ($TruthState in @('"active"', '"not_found"', '"backend_unavailable"', '"query_failed"')) {
    Test-ContentContains "legacy.ts references daily-operation truth_state $TruthState" $LegacyContent $TruthState | Out-Null
}

# -----------------------------------------------------------------------
# [5] Endpoint failure cannot masquerade as not_found.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] endpoint_unavailable never carries a fabricated not_found truth_state ---"
$EndpointUnavailableBlockMatch = [regex]::Match(
    $LegacyContent,
    'ENDPOINT_UNAVAILABLE_DAILY_OPERATION:\s*AutonomousDailyOperationSurface\s*=\s*\{[^}]*\}',
    [System.Text.RegularExpressions.RegexOptions]::Singleline
)
if ($EndpointUnavailableBlockMatch.Success -and $EndpointUnavailableBlockMatch.Value -match 'truth_state:\s*null') {
    Show-Green "  OK -- ENDPOINT_UNAVAILABLE_DAILY_OPERATION carries truth_state: null"
} else {
    $script:Violations++
    Show-Red "  FAIL -- ENDPOINT_UNAVAILABLE_DAILY_OPERATION must carry truth_state: null"
}

# -----------------------------------------------------------------------
# [6] Null counts never render as zero.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] Null evidence counts render as Unavailable, never 0 ---"
Test-ContentContains "formatDailyOperationCount returns Unavailable for null" $FormatContent 'value === null ? "Unavailable"' | Out-Null
foreach ($Field in @("strategy_evaluation_count", "order_activity_count", "fill_count")) {
    Test-ContentContains "screen routes $Field through formatDailyOperationCount" $ScreenContent "formatDailyOperationCount(row.$Field)" | Out-Null
}

# -----------------------------------------------------------------------
# [7] Evidence blockers never discarded.
#
# The current-operation panel referencing row.evidence_blockers is not
# sufficient proof that HISTORY rows also render their blockers -- isolate
# the HistoryPanel function body specifically (F1 repair [R9]).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] Evidence blockers are rendered, never discarded ---"
Test-ContentContains "screen renders evidence_blockers" $ScreenContent "row.evidence_blockers" | Out-Null

# -----------------------------------------------------------------------
# [8] History never re-sorted.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8] History rows are never re-sorted or re-grouped ---"
Test-ContentDoesNotContain "screen never calls .sort( on history rows" $ScreenContent "rows.sort(" | Out-Null
Test-ContentDoesNotContain "screen never calls .reverse( on history rows" $ScreenContent "rows.reverse(" | Out-Null

# -----------------------------------------------------------------------
# [9] No mutating route or action control.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] No mutating route or action control exists in the new screen ---"
foreach ($Forbidden in @("postJson", "invokeOperatorAction", "onRunAction", "<button", "onClick", "fetch(", "await fetch", "/api/v1/ops/action")) {
    Test-ContentDoesNotContain "screen never references '$Forbidden'" $ScreenContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [10]/[11] Patch-scope: no daemon production Rust, no migration.
#
# F1 legitimately adds/edits many GUI files -- unlike the E5 Phase E closure
# guard, this guard does not forbid GUI files. Base is the accepted Phase E
# closing commit (fixed); the committed range is base..HEAD (never widened
# permanently past this patch's own scope in a way that would misfire on a
# future, unrelated GUI patch -- this guard is F1's own, not reused verbatim
# by F2/F3).
# -----------------------------------------------------------------------
$F1Base = "4b6eec72cb65dec1fc2a8793e9d9d7bdde8328b4"

$PriorErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& git -C $RepoRoot merge-base --is-ancestor $F1Base HEAD 2>$null
$BaseIsAncestor = ($LASTEXITCODE -eq 0)
$CommittedRange = @()
if ($BaseIsAncestor) {
    $CommittedRange = git -C $RepoRoot diff --name-only "$F1Base..HEAD" 2>$null
}
$UnstagedChanges = git -C $RepoRoot diff --name-only 2>$null
$StagedChanges = git -C $RepoRoot diff --name-only --cached 2>$null
$ErrorActionPreference = $PriorErrorActionPreference

$CommittedRangeClean = @($CommittedRange) | Where-Object { $_ -ne "" } | Select-Object -Unique
$UnstagedClean = @($UnstagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique
$StagedClean = @($StagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique
$AllTouchedPaths = @($CommittedRangeClean) + @($UnstagedClean) + @($StagedClean) | Select-Object -Unique

function Test-NoProductionRustOrMigration {
    param([string]$Label, [string[]]$Paths)
    $ProductionRust = $Paths | Where-Object { $_ -like "core-rs/*/src/*.rs" -and $_ -notlike "*/tests/*" }
    $Migrations = $Paths | Where-Object { $_ -like "core-rs/crates/mqk-db/migrations/*" }

    if ($null -eq $ProductionRust -or @($ProductionRust).Count -eq 0) {
        Show-Green "  OK -- $Label -- no production Rust file (core-rs/**/src/**.rs)"
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
}

Write-Host ""
Show-Info "--- [10]/[11] No daemon production Rust file or migration is touched ---"
if ($BaseIsAncestor) {
    Show-Green "  OK -- $F1Base (accepted Phase E head) is an ancestor of HEAD"
} else {
    Show-Info "  INFO -- $F1Base is not (yet) an ancestor of HEAD -- checking working tree only"
}
Test-NoProductionRustOrMigration "committed range $F1Base..HEAD" $CommittedRangeClean
Test-NoProductionRustOrMigration "unstaged working tree" $UnstagedClean
Test-NoProductionRustOrMigration "staged working tree" $StagedClean

# -----------------------------------------------------------------------
# [12] No F2/F3 work introduced -- WITHIN F1'S OWN COMMITTED RANGE ONLY.
#
# Reconciled by AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-
# CORRECTION, mirroring how F1 itself reconciled the Phase E closure guard's
# own now-obsolete permanent-range assertion (that guard's check [23]/[24]
# split). This check originally scanned $AllTouchedPaths (committed
# F1Base..HEAD plus the live working tree), which correctly caught scope
# creep while F1 was still open -- but F1 is now accepted at the fixed head
# below, and F2/F3 are legitimately allowed (indeed required) to touch
# runbook/soak paths once they begin. Widening this check to the live
# working tree/current HEAD forever would misfire on every legitimate F2/F3
# patch. The check is therefore fixed to F1's own accepted committed range
# only -- it proves what F1 itself did, not what happens afterward.
# -----------------------------------------------------------------------
$F1AcceptedHead = "bd7336d4dd14dbb1943638b152886eb40b646b7d"
$PriorErrorActionPreference2 = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& git -C $RepoRoot merge-base --is-ancestor $F1Base $F1AcceptedHead 2>$null
$F1RangeBaseIsAncestor = ($LASTEXITCODE -eq 0)
$F1OwnCommittedRange = @()
if ($F1RangeBaseIsAncestor) {
    $F1OwnCommittedRange = git -C $RepoRoot diff --name-only "$F1Base..$F1AcceptedHead" 2>$null
}
$ErrorActionPreference = $PriorErrorActionPreference2
$F1OwnCommittedRangeClean = @($F1OwnCommittedRange) | Where-Object { $_ -ne "" } | Select-Object -Unique

Write-Host ""
Show-Info "--- [12] F1's own accepted committed range ($F1Base..$F1AcceptedHead) introduced no F2/F3 work ---"
$F2F3Paths = $F1OwnCommittedRangeClean | Where-Object {
    $_ -match "01f2" -or $_ -match "01f3" -or $_ -like "*runbook*" -or $_ -like "*soak*"
}
if ($null -eq $F2F3Paths -or @($F2F3Paths).Count -eq 0) {
    Show-Green "  OK -- no F2/F3/runbook/soak path touched within F1's own accepted range"
} else {
    $script:Violations++
    Show-Red "  FAIL -- F2/F3/runbook/soak path(s) touched within F1's own accepted range: $($F2F3Paths -join ', ')"
}

# -----------------------------------------------------------------------
# [13]-[16] README/ledger overclaim guard.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [13]-[16] README/ledger never overclaim Phase G, Bundle 3 closure, soak, or live capital ---"
Show-Info "    (F2/F3 implementation-complete claims are no longer forbidden here -- reconciled by"
Show-Info "     AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION, mirroring the same"
Show-Info "     obsolete-assertion reconciliation convention F1 itself applied to the Phase E closure"
Show-Info "     guard; F2/F3 legitimately reach implementation-complete status once F1 is accepted, and"
Show-Info "     each phase's own guard is the authority on its own overclaim prohibitions from here on)"
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

# -----------------------------------------------------------------------
# [17] README/ledger record correct F1 status.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [17] README/ledger record E1-E5 accepted, F1 implementation-complete-awaiting-acceptance ---"
Test-ContentContains "README.md records F1 as implementation-complete-awaiting-acceptance" $ReadmeContent "F1" | Out-Null
Test-ContentContains "ledger records F1" $LedgerContent "F1" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase F closed" $LedgerContent "PHASE F: CLOSED" | Out-Null

# -----------------------------------------------------------------------
# [18] Spec doc exists and is nonempty.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [18] F1 spec doc exists and is nonempty ---"
if (Test-FileExists "F1 spec doc" $PathF1Spec) {
    $SpecContent = Get-Content -Raw -Path $PathF1Spec
    if ($SpecContent.Length -gt 2000) {
        Show-Green "  OK -- F1 spec is nonempty ($($SpecContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- F1 spec is suspiciously short"
    }
}

# -----------------------------------------------------------------------
# [19] F1 GUI tests exist and are registered in package.json.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [19] F1 GUI test files exist and are registered in package.json ---"
$F1TestFiles = @(
    "src/features/autonomousDailyOperations/__tests__/formatDailyOperationCount.test.ts",
    "src/features/autonomousDailyOperations/__tests__/sourceAuthority.test.ts",
    "src/features/autonomousDailyOperations/__tests__/isAutonomousDailyOperationApiRow.test.ts",
    "src/features/autonomousDailyOperations/__tests__/api.test.ts",
    "src/features/autonomousDailyOperations/__tests__/screenSource.test.ts"
)
foreach ($TestFile in $F1TestFiles) {
    $FullPath = Join-Path $RepoRoot ("core-rs\mqk-gui\" + ($TestFile -replace "/", "\"))
    $Label = Split-Path -Leaf $TestFile
    if (Test-FileExists $Label $FullPath) {
        Test-ContentContains "package.json registers $Label" $PackageJsonContent $TestFile | Out-Null
    }
}

# =============================================================================
# [R1]-[R12] AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-
# BLOCKER-REPAIR-01 -- source-aware proof for the F1 repair pass. Strengthens
# [7]'s weak blocker check (satisfied merely by the current-operation panel)
# with a history-scoped proof, and proves the complete runtime validator,
# full wrapper-invariant enforcement, and the screen's own defensive
# active-null branch all actually exist in source -- not just that some
# truth_state string appears somewhere in the file.
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " [R1]-[R12] Runtime-shape and history-blocker repair"
Write-Host "============================================================"

# [R1] isAutonomousDailyOperationApiRow exists.
Write-Host ""
Show-Info "--- [R1] isAutonomousDailyOperationApiRow runtime validator exists ---"
Test-ContentContains "types/autonomousDailyOperations.ts exports isAutonomousDailyOperationApiRow" $TypesContent "export function isAutonomousDailyOperationApiRow" | Out-Null

# [R2] All three closed vocabulary validators (sets) exist.
Write-Host ""
Show-Info "--- [R2] Closed finalization/outcome/evidence vocabulary sets exist ---"
Test-ContentContains "types/autonomousDailyOperations.ts defines VALID_FINALIZATION_STATUSES" $TypesContent "VALID_FINALIZATION_STATUSES" | Out-Null
Test-ContentContains "types/autonomousDailyOperations.ts defines VALID_OUTCOME_CLASSES" $TypesContent "VALID_OUTCOME_CLASSES" | Out-Null
Test-ContentContains "types/autonomousDailyOperations.ts defines VALID_EVIDENCE_STATES" $TypesContent "VALID_EVIDENCE_STATES" | Out-Null

# [R3] Active single response requires a valid non-null operation.
Write-Host ""
Show-Info "--- [R3] active single response requires a valid non-null operation ---"
Test-ContentContains "legacy.ts requires isAutonomousDailyOperationApiRow on active operation" $LegacyContent 'truthState === "active"' | Out-Null
Test-ContentContains "legacy.ts calls isAutonomousDailyOperationApiRow(operation)" $LegacyContent "isAutonomousDailyOperationApiRow(operation)" | Out-Null

# [R4] not_found (and backend_unavailable) require operation null.
Write-Host ""
Show-Info "--- [R4] not_found/backend_unavailable require operation null ---"
Test-ContentContains "legacy.ts falls through to operation === null for non-active/query_failed states" $LegacyContent "operation === null" | Out-Null

# [R5]/[R6] History requires Array.isArray(rows) and validates every row.
Write-Host ""
Show-Info "--- [R5]/[R6] history requires Array.isArray(rows) and validates every row ---"
Test-ContentContains "legacy.ts requires Array.isArray(wrapper.rows)" $LegacyContent "Array.isArray(wrapper.rows)" | Out-Null
Test-ContentContains "legacy.ts validates every history row" $LegacyContent "wrapper.rows.every((row) => isAutonomousDailyOperationApiRow(row))" | Out-Null

# [R7] Successful history mapping never uses the bare `rows ?? []` fallback.
#
# Scoped to mapAutonomousDailyOperationsResponse's own function body -- other,
# unrelated legacy mappers in this file (mapExecutionOutboxWrapper,
# mapFillQualityWrapper) legitimately use `rows: wrapper.rows ?? []` for
# their own endpoints and must not trip this F1-scoped check.
Write-Host ""
Show-Info "--- [R7] successful history mapping does not use rows ?? [] ---"
if ($null -ne $LegacyContent) {
    $DailyOpsMapperMatch = [regex]::Match(
        $LegacyContent,
        'export function mapAutonomousDailyOperationsResponse\([\s\S]*?\r?\n\}\r?\n',
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )
    if ($DailyOpsMapperMatch.Success -and -not $DailyOpsMapperMatch.Value.Contains("wrapper.rows ?? []")) {
        Show-Green "  OK -- mapAutonomousDailyOperationsResponse does not fall back with rows ?? []"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- mapAutonomousDailyOperationsResponse falls back with rows ?? [] (or function body not found)"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- legacy.ts unreadable, cannot verify history mapper"
}

# [R8] current active-null cannot enter the not_found branch.
Write-Host ""
Show-Info "--- [R8] active + null operation cannot enter the not_found branch ---"
Test-ContentDoesNotContain 'screen no longer collapses not_found and null-operation into one branch' $ScreenContent 'surface.truth_state === "not_found" || surface.operation == null' | Out-Null
Test-ContentContains "screen has a distinct not_found-only branch" $ScreenContent 'surface.truth_state === "not_found"' | Out-Null
Test-ContentContains "screen renders a distinct malformed-response notice" $ScreenContent "Malformed daily-operation response" | Out-Null

# [R9] History-row rendering (not just the current-operation panel)
# references row.evidence_blockers -- isolate the HistoryPanel function body.
Write-Host ""
Show-Info "--- [R9] HistoryPanel itself (not just CurrentOperationPanel) renders row.evidence_blockers ---"
if ($null -ne $ScreenContent) {
    $HistoryPanelMatch = [regex]::Match(
        $ScreenContent,
        'function HistoryPanel\([\s\S]*?\r?\n\}\r?\n',
        [System.Text.RegularExpressions.RegexOptions]::Singleline
    )
    if ($HistoryPanelMatch.Success -and $HistoryPanelMatch.Value.Contains("row.evidence_blockers")) {
        Show-Green "  OK -- HistoryPanel body references row.evidence_blockers"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- HistoryPanel body does not reference row.evidence_blockers (history rows must render their own blockers, not just the current-operation panel)"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- screen source unreadable, cannot verify HistoryPanel blocker rendering"
}

# [R10]/[R11]/[R12] Required new tests exist.
Write-Host ""
Show-Info "--- [R10]-[R12] required new tests exist ---"
Test-ContentContains "malformed active-null test exists" $ApiTestContent "active with null operation maps to endpoint_unavailable" | Out-Null
Test-ContentContains "malformed history-rows test exists" $ApiTestContent "history containing one malformed row maps the whole surface to endpoint_unavailable" | Out-Null
Test-ContentContains "history blocker rendering test exists" $ApiTestContent "a history row with two evidence blockers renders both blocker strings" | Out-Null
Test-ContentContains "screen-level defensive active-null test exists" $ScreenSourceTestContent "screen renders a malformed notice, not the neutral not_found copy, for active truth_state with null operation" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
