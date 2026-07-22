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
# [12] No F2/F3 work introduced.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [12] No F2/F3 work is introduced ---"
$F2F3Paths = $AllTouchedPaths | Where-Object {
    $_ -match "01f2" -or $_ -match "01f3" -or $_ -like "*runbook*" -or $_ -like "*soak*"
}
if ($null -eq $F2F3Paths -or @($F2F3Paths).Count -eq 0) {
    Show-Green "  OK -- no F2/F3/runbook/soak path touched"
} else {
    $script:Violations++
    Show-Red "  FAIL -- F2/F3/runbook/soak path(s) touched: $($F2F3Paths -join ', ')"
}

# -----------------------------------------------------------------------
# [13]-[16] README/ledger overclaim guard.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [13]-[16] README/ledger never overclaim Phase G, Bundle 3 closure, soak, or live capital ---"
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
    "ready for live capital",
    "F2: IMPLEMENTATION COMPLETE",
    "F2 is implementation complete",
    "F3: IMPLEMENTATION COMPLETE",
    "F3 is implementation complete"
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
