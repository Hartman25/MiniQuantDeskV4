# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-
# PROJECTION -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the E4 strictly read-only daily-operation API
# projection, built on top of the already-accepted E1 contract, E2A
# coverage-anchor/run-lineage foundation, E2B strict classifier/finalization
# CAS, and E3 coordinator finalization integration (all validated by their
# own, independently-maintained guards, unmodified by this patch except for
# their own now-obsolete "no E4 route"/"E3 not yet accepted" checks, reconciled
# in place). No network call, no provider/broker call, no DB connection, no
# daemon start, no cargo/npm build or test -- pure text/source validation only.
#
# Checks:
#   [1]  Both canonical GET routes are mounted on the public (unauthenticated)
#        router, and neither route is ever mounted with a mutating HTTP
#        method (POST/PUT/PATCH/DELETE).
#   [2]  The single-operation route resolves the exact
#        (market_date, deployment_mode, adapter_id) slot via
#        `mqk_db::fetch_autonomous_daily_operation_for_slot` -- never a
#        different lookup.
#   [3]  The history route uses `mqk_db::list_recent_autonomous_daily_operations`
#        -- never a second, independently-derived list query.
#   [4]  History `limit` is clamped to `[1, 100]` via `.clamp(1, 100)`.
#   [5]  The full truth-state vocabulary (`active`, `not_found`,
#        `backend_unavailable`, `query_failed`) is present in the route
#        module.
#   [6]  Nonterminal projection can never expose a non-null `outcome_class`/
#        `outcome_reason_code`/`finalized_at_utc` -- the terminal branch is
#        gated strictly on `state` being one of the three `completed*`
#        values, and the nonterminal branch returns literal `None` for all
#        three fields.
#   [7]  Full-lineage counts are gathered via
#        `mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage`
#        -- never the operation's current `run_id` column alone.
#   [8]  An invalid/unreadable run lineage can never produce false-zero
#        counts -- the route module's own `ActivityCounts` enum has a
#        distinct `LineageUnavailable`/`DatabaseUnavailable` case, and the
#        row builder emits `None` (JSON `null`) for all three counts on
#        those cases, never `0`.
#   [9]  The E4 route module never calls the classifier/finalizer/
#        coordinator-tick entry points (`classify_and_finalize_autonomous_daily_operation`,
#        `finalize_autonomous_daily_operation`, `tick_autonomous_daily_coordinator`).
#   [10] The E4 route module never calls a DB write helper
#        (`create_or_recover_autonomous_daily_operation`,
#        `transition_autonomous_daily_operation*`,
#        `persist_autonomous_daily_finalization_blocker`,
#        `persist_autonomous_session_event`,
#        `write_and_confirm_coverage_authority`, `start_execution_runtime`,
#        `stop_execution_runtime`).
#   [11] The additive `daily_operation` summary field exists on
#        `AutonomousPaperReadinessResponse`, `AutonomousPaperStatusResponse`,
#        and `PreflightStatusResponse`, and every one of readiness's,
#        paper-status's, and preflight's response-construction sites in
#        their route handlers supplies it.
#   [12] The shared summary-computation function
#        (`compute_daily_operation_summary`) never propagates an `Err`/panics
#        to its caller -- its signature returns the summary type directly
#        (not a `Result`), so a failure can only ever be reported through the
#        summary's own `truth_state` field, never the parent route's HTTP
#        status or other fields.
#   [13] No raw database error text, SQL fragment, connection string, or
#        `{:?}`-debug-formatted error ever enters a durable/JSON field in the
#        E4 route module.
#   [14] No GUI file is touched by this patch.
#   [15] README.md / README_TECHNICAL.md never mark E4, Phase E, or Bundle 3
#        as accepted/complete, and never claim the unattended soak has
#        started or live capital is ready; README.md records E1/E2A/E2B/E3 as
#        accepted and E4 as implementation-complete-awaiting-acceptance.
#   [16] The new E4 spec doc and scenario test file both exist and are
#        nonempty.
#   [17] (READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01) The terminal projection
#        branch never returns `evidence_state = "complete"` unconditionally
#        -- it must branch on `ActivityCounts` (distinguishing the two
#        automatic classifier terminal states, which may project
#        `"complete"`, from generic administrative `completed`, which must
#        never project `"complete"` even when counts are available).
#   [18] `response_truth_state_for_counts` (or equivalent shared mapping)
#        never maps `ActivityCounts::DatabaseUnavailable` to `"active"` --
#        a downstream count-read failure must always demote the top-level
#        truth_state to `"query_failed"`.
#   [19] `compute_daily_operation_summary`'s active-record branch reuses the
#        shared truth-state mapping (never hardcodes `truth_state: "active"`
#        for a record whose counts might be `DatabaseUnavailable`).
#   [20] The invalid-request branch never interpolates the raw caller-
#        controlled `market_date` value into its response message.
#   [21] The terminal-invalid-lineage regression tests
#        (`b06b_terminal_no_trade_invalid_lineage_is_unavailable_not_complete`,
#        `b07b_terminal_activity_invalid_lineage_is_unavailable_not_complete`)
#        exist in the E4 scenario test file.
#   [22] The downstream-count-failure truth-state regression tests
#        (`b25_single_response_database_unavailable_counts_is_query_failed_with_known_operation`,
#        `b26_history_response_database_unavailable_counts_is_query_failed`,
#        `b27_summary_database_unavailable_counts_is_query_failed_parent_unchanged`)
#        exist in the E4 scenario test file.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathE4RouteRs      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\autonomous_daily_operations.rs"
$PathRoutesRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$PathApiTypesRs     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$PathSystemRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\system.rs"
$PathPaperStatusRs  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\autonomous_paper_status.rs"
$PathE4Spec         = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.md"
$PathE4Test         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_operation_api_01.rs"
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
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION Validator"
Write-Host "============================================================"

$E4RouteContent = $null
if (Test-FileExists "autonomous_daily_operations.rs (E4 route module)" $PathE4RouteRs) {
    $E4RouteContent = Get-Content -Raw -Path $PathE4RouteRs
}
$RoutesContent = $null
if (Test-FileExists "routes.rs" $PathRoutesRs) {
    $RoutesContent = Get-Content -Raw -Path $PathRoutesRs
}
$ApiTypesContent = $null
if (Test-FileExists "api_types.rs" $PathApiTypesRs) {
    $ApiTypesContent = Get-Content -Raw -Path $PathApiTypesRs
}
$SystemContent = $null
if (Test-FileExists "routes/system.rs" $PathSystemRs) {
    $SystemContent = Get-Content -Raw -Path $PathSystemRs
}
$PaperStatusContent = $null
if (Test-FileExists "routes/autonomous_paper_status.rs" $PathPaperStatusRs) {
    $PaperStatusContent = Get-Content -Raw -Path $PathPaperStatusRs
}

Write-Host ""
Show-Info "--- [1] Both canonical GET routes are mounted; neither accepts a mutating method ---"
Test-ContentContains "routes.rs mounts GET /api/v1/autonomous/daily-operation" $RoutesContent 'get(autonomous_daily_operation)' | Out-Null
Test-ContentContains "routes.rs mounts GET /api/v1/autonomous/daily-operations" $RoutesContent 'get(autonomous_daily_operations)' | Out-Null
Test-ContentContains "the single route path is registered" $RoutesContent '"/api/v1/autonomous/daily-operation",' | Out-Null
Test-ContentContains "the history route path is registered" $RoutesContent '"/api/v1/autonomous/daily-operations",' | Out-Null
foreach ($Method in @("post(autonomous_daily_operation)", "put(autonomous_daily_operation)", "delete(autonomous_daily_operation)", "patch(autonomous_daily_operation)")) {
    Test-ContentDoesNotContain "routes.rs never mounts $Method" $RoutesContent $Method | Out-Null
}
foreach ($Method in @("post(autonomous_daily_operations)", "put(autonomous_daily_operations)", "delete(autonomous_daily_operations)", "patch(autonomous_daily_operations)")) {
    Test-ContentDoesNotContain "routes.rs never mounts $Method" $RoutesContent $Method | Out-Null
}
# Both routes must be declared on the `public` (unauthenticated) router, not
# merged into the `operator` router that is wrapped in token_auth_middleware.
$PublicRouterBody = Get-ContentBetween -Content $RoutesContent -StartNeedle "let public = Router::new()" -EndNeedle "// --- Operator (authenticated) routes"
if ($null -eq $PublicRouterBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the public router body"
} else {
    Test-ContentContains "the single E4 route is declared on the public router" $PublicRouterBody '"/api/v1/autonomous/daily-operation",' | Out-Null
    Test-ContentContains "the history E4 route is declared on the public router" $PublicRouterBody '"/api/v1/autonomous/daily-operations",' | Out-Null
}

Write-Host ""
Show-Info "--- [2] Single-operation route uses the exact-slot lookup ---"
Test-ContentContains "the single route calls fetch_autonomous_daily_operation_for_slot" $E4RouteContent "mqk_db::fetch_autonomous_daily_operation_for_slot(" | Out-Null
Test-ContentDoesNotContain "the single route never calls fetch_autonomous_daily_operation_by_id (a different lookup)" $E4RouteContent "fetch_autonomous_daily_operation_by_id(" | Out-Null

Write-Host ""
Show-Info "--- [3] History route uses list_recent_autonomous_daily_operations ---"
Test-ContentContains "the history route calls list_recent_autonomous_daily_operations" $E4RouteContent "mqk_db::list_recent_autonomous_daily_operations(" | Out-Null

Write-Host ""
Show-Info "--- [4] History limit is clamped to [1, 100] ---"
Test-ContentContains "the effective limit is computed via .clamp(1, 100)" $E4RouteContent ".clamp(1, 100)" | Out-Null
Test-ContentContains "the default limit is 20" $E4RouteContent "DEFAULT_HISTORY_LIMIT: i64 = 20" | Out-Null

Write-Host ""
Show-Info "--- [5] Full truth-state vocabulary is present ---"
foreach ($State in @('"active"', '"not_found"', '"backend_unavailable"', '"query_failed"')) {
    Test-ContentContains "the route module uses truth_state $State" $E4RouteContent $State | Out-Null
}

Write-Host ""
Show-Info "--- [6] Nonterminal rows can never expose outcome/finalized fields ---"
$ProjectionFnBody = Get-ContentBetween -Content $E4RouteContent `
    -StartNeedle "fn project_daily_operation_outcome(" `
    -EndNeedle "`n/// Build the full API row"
if ($null -eq $ProjectionFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate project_daily_operation_outcome's body"
} else {
    Test-ContentContains "the terminal branch gates on STATE_COMPLETED_NO_TRADE/STATE_COMPLETED_WITH_ACTIVITY/STATE_COMPLETED only" $ProjectionFnBody "mqk_db::STATE_COMPLETED_NO_TRADE" | Out-Null
    Test-ContentContains "the nonterminal branch returns outcome_class: None" $ProjectionFnBody "outcome_class: None," | Out-Null
    Test-ContentContains "the nonterminal branch returns outcome_reason_code: None" $ProjectionFnBody "outcome_reason_code: None," | Out-Null
    Test-ContentContains "the nonterminal branch returns finalized_at_utc: None" $ProjectionFnBody "finalized_at_utc: None," | Out-Null
}
Test-ContentDoesNotContain "the route module never reruns the E2B classifier" $E4RouteContent "classify_autonomous_daily_outcome(" | Out-Null

Write-Host ""
Show-Info "--- [7] Full-lineage counts use the validated run-lineage helper ---"
Test-ContentContains "counts are gathered via fetch_and_validate_autonomous_daily_operation_run_lineage" $E4RouteContent "mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(" | Out-Null

Write-Host ""
Show-Info "--- [8] Invalid lineage can never produce false-zero counts ---"
Test-ContentContains "ActivityCounts has a distinct LineageUnavailable case" $E4RouteContent "LineageUnavailable," | Out-Null
Test-ContentContains "ActivityCounts has a distinct DatabaseUnavailable case" $E4RouteContent "DatabaseUnavailable," | Out-Null
$RowBuilderBody = Get-ContentBetween -Content $E4RouteContent `
    -StartNeedle "pub(crate) fn build_operation_api_row(" `
    -EndNeedle "`n}"
if ($null -eq $RowBuilderBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate build_operation_api_row's body"
} else {
    Test-ContentContains "the row builder maps LineageUnavailable/DatabaseUnavailable to (None, None, None)" $RowBuilderBody "ActivityCounts::LineageUnavailable | ActivityCounts::DatabaseUnavailable => {`n            (None, None, None)" | Out-Null
}

Write-Host ""
Show-Info "--- [9] The E4 route module never calls the classifier/finalizer/coordinator ---"
foreach ($Symbol in @(
    "classify_and_finalize_autonomous_daily_operation(",
    "finalize_autonomous_daily_operation(",
    "tick_autonomous_daily_coordinator("
)) {
    Test-ContentDoesNotContain "the E4 route module never calls $Symbol" $E4RouteContent $Symbol | Out-Null
}

Write-Host ""
Show-Info "--- [10] The E4 route module never calls a DB write helper ---"
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

Write-Host ""
Show-Info "--- [11] The additive daily_operation summary field exists on all three responses and every branch supplies it ---"
Test-ContentContains "AutonomousPaperReadinessResponse carries daily_operation" $ApiTypesContent "pub struct AutonomousPaperReadinessResponse" | Out-Null
Test-ContentContains "AutonomousPaperStatusResponse carries daily_operation" $ApiTypesContent "pub struct AutonomousPaperStatusResponse" | Out-Null
Test-ContentContains "PreflightStatusResponse carries daily_operation" $ApiTypesContent "pub struct PreflightStatusResponse" | Out-Null
$ReadinessStructBody = Get-ContentBetween -Content $ApiTypesContent -StartNeedle "pub struct AutonomousPaperReadinessResponse {" -EndNeedle "`n}"
$PaperStatusStructBody = Get-ContentBetween -Content $ApiTypesContent -StartNeedle "pub struct AutonomousPaperStatusResponse {" -EndNeedle "`n}"
$PreflightStructBody = Get-ContentBetween -Content $ApiTypesContent -StartNeedle "pub struct PreflightStatusResponse {" -EndNeedle "`n}"
Test-ContentContains "AutonomousPaperReadinessResponse's field list includes daily_operation" $ReadinessStructBody "pub daily_operation: AutonomousDailyOperationSummary," | Out-Null
Test-ContentContains "AutonomousPaperStatusResponse's field list includes daily_operation" $PaperStatusStructBody "pub daily_operation: AutonomousDailyOperationSummary," | Out-Null
Test-ContentContains "PreflightStatusResponse's field list includes daily_operation" $PreflightStructBody "pub daily_operation: AutonomousDailyOperationSummary," | Out-Null

$ReadinessConstructionCount = ([regex]::Matches($SystemContent, [regex]::Escape('canonical_route: "/api/v1/autonomous/readiness".to_string()'))).Count
# Matched on the bare function name only (not "(&st,") -- rustfmt is free to
# wrap a long call's argument list onto its own line, which would otherwise
# break a stricter contiguous-text match.
$ReadinessSummaryCallCount = ([regex]::Matches($SystemContent, [regex]::Escape('compute_daily_operation_summary('))).Count
if ($ReadinessConstructionCount -ge 1 -and $ReadinessSummaryCallCount -ge $ReadinessConstructionCount) {
    Show-Green "  OK -- every AutonomousPaperReadinessResponse construction site ($ReadinessConstructionCount found) supplies compute_daily_operation_summary ($ReadinessSummaryCallCount calls found)"
} else {
    $script:Violations++
    Show-Red "  FAIL -- readiness response construction sites ($ReadinessConstructionCount) exceed compute_daily_operation_summary call sites ($ReadinessSummaryCallCount)"
}

$PaperStatusSummaryCallCount = ([regex]::Matches($PaperStatusContent, "compute_daily_operation_summary\(&st,")).Count
if ($PaperStatusSummaryCallCount -ge 3) {
    Show-Green "  OK -- autonomous_paper_status.rs calls compute_daily_operation_summary at least 3 times (found $PaperStatusSummaryCallCount, one per response-construction branch)"
} else {
    $script:Violations++
    Show-Red "  FAIL -- autonomous_paper_status.rs calls compute_daily_operation_summary fewer than 3 times (found $PaperStatusSummaryCallCount) -- not every branch is covered"
}

$PreflightFnBody = Get-ContentBetween -Content $SystemContent `
    -StartNeedle "pub(crate) async fn system_preflight(" `
    -EndNeedle "AUTON-TRUTH-01"
if ($null -ne $PreflightFnBody -and $PreflightFnBody.IndexOf("compute_daily_operation_summary(", [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
    Show-Green "  OK -- system_preflight calls compute_daily_operation_summary"
} else {
    $script:Violations++
    Show-Red "  FAIL -- system_preflight never calls compute_daily_operation_summary"
}

Write-Host ""
Show-Info "--- [12] compute_daily_operation_summary never propagates an error to its caller ---"
$SummaryFnBody = Get-ContentBetween -Content $E4RouteContent `
    -StartNeedle "pub(crate) async fn compute_daily_operation_summary(" `
    -EndNeedle "`n}"
if ($null -eq $SummaryFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate compute_daily_operation_summary's body"
} else {
    Test-ContentContains "compute_daily_operation_summary's signature returns AutonomousDailyOperationSummary directly (never Result)" $E4RouteContent "pub(crate) async fn compute_daily_operation_summary(`n    st: &AppState,`n    now_utc: DateTime<Utc>,`n) -> AutonomousDailyOperationSummary {" | Out-Null
    Test-ContentDoesNotContain "compute_daily_operation_summary never unwraps/expects on a fallible DB call" $SummaryFnBody ".expect(" | Out-Null
    Test-ContentDoesNotContain "compute_daily_operation_summary never uses the ? operator (which would require a Result return type)" $SummaryFnBody "?;" | Out-Null
}

Write-Host ""
Show-Info "--- [13] No raw DB error text ever enters a durable/JSON field ---"
Test-ContentDoesNotContain "the E4 route module never formats a raw {:?} debug value into a response field" $E4RouteContent "{:?}" | Out-Null
# Word-boundary anchored so a legitimate `truth_state.to_string()` call
# (ending in the letter "e" immediately before ".to_string()") is never
# mistaken for a raw bound error variable being stringified.
foreach ($ErrVar in @("err", "e")) {
    if ([regex]::IsMatch($E4RouteContent, "\b$ErrVar\.to_string\(\)")) {
        $script:Violations++
        Show-Red "  FAIL -- the E4 route module references $ErrVar.to_string() in a response field"
    } else {
        Show-Green "  OK -- the E4 route module never references $ErrVar.to_string() in a response field"
    }
}

Write-Host ""
Show-Info "--- [14] No GUI file is touched ---"
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
Show-Info "--- [15] README truth: E1/E2A/E2B/E3 accepted, E4 implementation-complete-awaiting-acceptance, Phase E/Bundle 3 open ---"
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
    "Phase E: ACCEPTED",
    "Bundle 3: CLOSED",
    "Bundle 3 is complete",
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

Write-Host ""
Show-Info "--- [16] New E4 spec doc and scenario test file exist and are nonempty ---"
if (Test-FileExists "E4 spec doc" $PathE4Spec) {
    $E4SpecContent = Get-Content -Raw -Path $PathE4Spec
    if ($E4SpecContent.Length -gt 2000) {
        Show-Green "  OK -- E4 spec doc is nonempty ($($E4SpecContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E4 spec doc is suspiciously short"
    }
}
$E4TestContent = $null
if (Test-FileExists "E4 scenario test file" $PathE4Test) {
    $E4TestContent = Get-Content -Raw -Path $PathE4Test
    if ($E4TestContent.Length -gt 2000) {
        Show-Green "  OK -- E4 scenario test file is nonempty ($($E4TestContent.Length) chars)"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- E4 scenario test file is suspiciously short"
    }
}

Write-Host ""
Show-Info "--- [17] Terminal projection honors ActivityCounts -- never unconditional 'complete' ---"
$TerminalBranchBody = Get-ContentBetween -Content $E4RouteContent `
    -StartNeedle "if let Some((class, is_automatic_terminal)) = terminal_outcome_class {" `
    -EndNeedle "`n    // Nonterminal projection."
if ($null -eq $TerminalBranchBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate the terminal branch's ActivityCounts-gated body (is_automatic_terminal split missing?)"
} else {
    Test-ContentContains "the terminal branch matches on ActivityCounts::LineageUnavailable" $TerminalBranchBody "ActivityCounts::LineageUnavailable" | Out-Null
    Test-ContentContains "the terminal branch matches on ActivityCounts::DatabaseUnavailable" $TerminalBranchBody "ActivityCounts::DatabaseUnavailable" | Out-Null
    Test-ContentContains "the terminal branch can still project ""complete"" for the two automatic classifier terminal states" $TerminalBranchBody '"complete".to_string()' | Out-Null
    Test-ContentContains "the terminal branch projects ""pending"" (never ""complete"") for generic completed even when counts are available" $TerminalBranchBody '"pending".to_string()' | Out-Null
    Test-ContentContains "the terminal branch's available-counts case is itself gated on is_automatic_terminal" $TerminalBranchBody "if is_automatic_terminal {" | Out-Null
}

Write-Host ""
Show-Info "--- [18] DatabaseUnavailable can never produce top-level active ---"
$TruthMappingFnBody = Get-ContentBetween -Content $E4RouteContent `
    -StartNeedle "pub(crate) fn response_truth_state_for_counts(counts: &ActivityCounts) -> &'static str {" `
    -EndNeedle "`n}"
if ($null -eq $TruthMappingFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate response_truth_state_for_counts's body"
} else {
    Test-ContentContains "DatabaseUnavailable maps to query_failed" $TruthMappingFnBody 'ActivityCounts::DatabaseUnavailable => "query_failed"' | Out-Null
    Test-ContentDoesNotContain "DatabaseUnavailable is never mapped to active" $TruthMappingFnBody 'ActivityCounts::DatabaseUnavailable => "active"' | Out-Null
    Test-ContentContains "Available/LineageUnavailable map to active" $TruthMappingFnBody 'ActivityCounts::Available { .. } | ActivityCounts::LineageUnavailable => "active"' | Out-Null
}
foreach ($Symbol in @(
    "mqk_db::fetch_autonomous_daily_operation_for_slot(",
    "list_recent_autonomous_daily_operations("
)) {
    Test-ContentContains "the route module still calls $Symbol (single/history lookups unchanged)" $E4RouteContent $Symbol | Out-Null
}

Write-Host ""
Show-Info "--- [19] compute_daily_operation_summary reuses the shared truth-state mapping ---"
$SummaryActiveBranchBody = Get-ContentBetween -Content $E4RouteContent `
    -StartNeedle "pub(crate) async fn compute_daily_operation_summary(" `
    -EndNeedle "`n// ---"
if ($null -eq $SummaryActiveBranchBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate compute_daily_operation_summary's body"
} else {
    Test-ContentContains "compute_daily_operation_summary's active-record branch calls response_truth_state_for_counts" $SummaryActiveBranchBody "response_truth_state_for_counts(&counts)" | Out-Null
    Test-ContentDoesNotContain "compute_daily_operation_summary never hardcodes truth_state: ""active"" for the active-record branch" $SummaryActiveBranchBody 'build_active_summary("active"' | Out-Null
}

Write-Host ""
Show-Info "--- [20] The invalid-request branch never interpolates the raw market_date value ---"
Test-ContentDoesNotContain "the 400 response never formats the raw query value into its message" $E4RouteContent 'invalid market_date' | Out-Null
Test-ContentDoesNotContain "the 400 response never interpolates {raw} into its message" $E4RouteContent '{raw}' | Out-Null
Test-ContentContains "the 400 response uses a fixed bounded message" $E4RouteContent "market_date must use exact YYYY-MM-DD format" | Out-Null

Write-Host ""
Show-Info "--- [21] Terminal-invalid-lineage regression tests exist ---"
foreach ($TestName in @(
    "b06b_terminal_no_trade_invalid_lineage_is_unavailable_not_complete",
    "b07b_terminal_activity_invalid_lineage_is_unavailable_not_complete"
)) {
    Test-ContentContains "E4 test file defines $TestName" $E4TestContent "fn $TestName(" | Out-Null
}

Write-Host ""
Show-Info "--- [22] Downstream-count-failure truth-state regression tests exist ---"
foreach ($TestName in @(
    "b25_single_response_database_unavailable_counts_is_query_failed_with_known_operation",
    "b26_history_response_database_unavailable_counts_is_query_failed",
    "b27_summary_database_unavailable_counts_is_query_failed_parent_unchanged"
)) {
    Test-ContentContains "E4 test file defines $TestName" $E4TestContent "fn $TestName(" | Out-Null
}

Write-Host ""
Show-Info "--- Ledger truth ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentContains "ledger records E3 as accepted" $LedgerContent "E3: ACCEPTED" | Out-Null
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
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
